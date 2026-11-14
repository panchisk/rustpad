//! WASAPI loopback capture: records what the default output device plays.
//!
//! The capture owns its own thread and all COM objects live and die on it, so
//! the engine thread only flips an atomic when the user stops. The result is
//! buffered in memory (capped) and written as a 16-bit WAV when the capture
//! ends.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};

/// RAM-safety cap used when `max_secs == 0` (30 minutes).
pub const MAX_UNLIMITED_SECS: u32 = 30 * 60;
/// How often the capture thread drains the WASAPI packet queue.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
/// Upper bound on initialisation; keeps `start` from blocking forever.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A running loopback capture.
///
/// Dropping the handle stops the capture and joins the thread, so a handle can
/// never leak its thread.
pub struct LoopbackHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<u32>>,
}

impl LoopbackHandle {
    /// Starts capturing the default render endpoint into `path`.
    ///
    /// `max_secs == 0` records until stopped (capped at 30 minutes).
    /// Returns an error when no render device / capture session is available,
    /// so callers never have to deal with a silently dead thread.
    pub fn start(path: impl Into<PathBuf>, max_secs: u32) -> Result<Self> {
        let path = path.into();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread_stop = stop.clone();

        let join = std::thread::Builder::new()
            .name("rustpad-loopback".into())
            .spawn(move || capture_thread(path, max_secs, thread_stop, ready_tx))
            .map_err(|err| anyhow!("spawning loopback thread: {err}"))?;

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(())) => Ok(LoopbackHandle {
                stop,
                join: Some(join),
            }),
            Ok(Err(msg)) => {
                let _ = join.join();
                Err(anyhow!("{msg}"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                stop.store(true, Ordering::SeqCst);
                let _ = join.join();
                Err(anyhow!("starting loopback capture timed out"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = join.join();
                Err(anyhow!("loopback capture thread exited unexpectedly"))
            }
        }
    }

    /// Stops and joins the capture, returning the recorded duration in ms.
    pub fn stop(mut self) -> u32 {
        self.stop.store(true, Ordering::SeqCst);
        self.join
            .take()
            .and_then(|join| join.join().ok())
            .unwrap_or(0)
    }
}

impl Drop for LoopbackHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn capture_thread(
    path: PathBuf,
    max_secs: u32,
    stop: Arc<AtomicBool>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) -> u32 {
    #[cfg(windows)]
    {
        imp::capture(path, max_secs, stop, ready)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, max_secs, stop);
        let _ = ready.send(Err("loopback capture is only implemented on Windows".into()));
        0
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::{Duration, Instant};

    use windows::core::GUID;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    use crate::record::Recorder;
    use crate::wav::write_wav;

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

    /// `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`
    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    /// `RPC_E_CHANGED_MODE`: COM is already initialised on this thread.
    const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

    /// RAII COM initialisation (`MULTITHREADED`).
    struct ComInit {
        owned: bool,
    }

    impl ComInit {
        fn new() -> Result<Self> {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if hr.is_ok() {
                return Ok(ComInit { owned: true });
            }
            if hr.0 == RPC_E_CHANGED_MODE {
                // Someone else already initialised this thread; leave it alone.
                return Ok(ComInit { owned: false });
            }
            Err(anyhow!("CoInitializeEx failed (0x{:08x})", hr.0 as u32))
        }
    }

    impl Drop for ComInit {
        fn drop(&mut self) {
            if self.owned {
                unsafe { CoUninitialize() };
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SampleKind {
        F32,
        I16,
        I32,
        /// 24 valid bits left-justified in a 32-bit container.
        I24In32,
        /// Three packed bytes per sample.
        I24Packed,
    }

    /// The endpoint's mix format, reduced to what the conversion needs.
    #[derive(Clone, Copy)]
    struct PacketFormat {
        channels: usize,
        sample_rate: u32,
        kind: SampleKind,
    }

    impl PacketFormat {
        unsafe fn from_mix_format(fmt: *const WAVEFORMATEX) -> Result<Self> {
            if fmt.is_null() {
                return Err(anyhow!("GetMixFormat returned no format"));
            }
            // `WAVEFORMATEX` is packed, so every field is read into a plain
            // local before use (no references to packed fields).
            let base: WAVEFORMATEX = std::ptr::read_unaligned(fmt);
            let format_tag = base.wFormatTag;
            let channels = base.nChannels as usize;
            let sample_rate = base.nSamplesPerSec;
            let bits = base.wBitsPerSample;
            let block_align = base.nBlockAlign as usize;
            let cb_size = base.cbSize;

            if channels == 0 {
                return Err(anyhow!("mix format has zero channels"));
            }
            if sample_rate == 0 {
                return Err(anyhow!("mix format has zero sample rate"));
            }
            if block_align == 0 || block_align % channels != 0 {
                return Err(anyhow!(
                    "invalid mix format block alignment {}",
                    block_align
                ));
            }
            let bytes_per_sample = block_align / channels;

            let (is_float, valid_bits) = if format_tag == WAVE_FORMAT_EXTENSIBLE {
                if cb_size < 22 {
                    return Err(anyhow!("truncated WAVEFORMATEXTENSIBLE mix format"));
                }
                let ext: WAVEFORMATEXTENSIBLE =
                    std::ptr::read_unaligned(fmt as *const WAVEFORMATEXTENSIBLE);
                let sub_format: GUID = std::ptr::addr_of!(ext.SubFormat).read_unaligned();
                let valid: u16 =
                    std::ptr::addr_of!(ext.Samples.wValidBitsPerSample).read_unaligned();
                (
                    sub_format == SUBTYPE_IEEE_FLOAT,
                    if valid == 0 { bits } else { valid },
                )
            } else {
                (format_tag == WAVE_FORMAT_IEEE_FLOAT, bits)
            };

            let kind = if is_float && bits == 32 {
                SampleKind::F32
            } else if !is_float && bits == 16 {
                SampleKind::I16
            } else if !is_float && bits == 32 && valid_bits <= 24 {
                SampleKind::I24In32
            } else if !is_float && bits == 32 {
                SampleKind::I32
            } else if !is_float && bits == 24 && bytes_per_sample == 4 {
                SampleKind::I24In32
            } else if !is_float && bits == 24 && bytes_per_sample == 3 {
                SampleKind::I24Packed
            } else {
                return Err(anyhow!(
                    "unsupported loopback mix format: tag {}, {} bits, {} bytes/sample",
                    format_tag,
                    bits,
                    bytes_per_sample
                ));
            };

            Ok(PacketFormat {
                channels,
                sample_rate,
                kind,
            })
        }

        /// Converts `frames` interleaved frames into `out` as interleaved f32.
        unsafe fn append(&self, data: *const u8, frames: usize, out: &mut Vec<f32>) {
            let count = frames * self.channels;
            out.clear();
            out.reserve(count);
            match self.kind {
                SampleKind::F32 => {
                    let ptr = data as *const f32;
                    for i in 0..count {
                        let v = ptr.add(i).read_unaligned();
                        out.push(if v.is_finite() {
                            v.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        });
                    }
                }
                SampleKind::I16 => {
                    let ptr = data as *const i16;
                    for i in 0..count {
                        out.push(ptr.add(i).read_unaligned() as f32 / 32_768.0);
                    }
                }
                SampleKind::I32 => {
                    let ptr = data as *const i32;
                    for i in 0..count {
                        out.push(ptr.add(i).read_unaligned() as f32 / 2_147_483_648.0);
                    }
                }
                SampleKind::I24In32 => {
                    let ptr = data as *const i32;
                    for i in 0..count {
                        let v = ptr.add(i).read_unaligned() >> 8;
                        out.push(v as f32 / 8_388_608.0);
                    }
                }
                SampleKind::I24Packed => {
                    for i in 0..count {
                        let p = data.add(i * 3);
                        let raw = (u32::from(*p)
                            | (u32::from(*p.add(1)) << 8)
                            | (u32::from(*p.add(2)) << 16))
                            as i32;
                        // Sign-extend 24 -> 32 bits.
                        let v = (raw << 8) >> 8;
                        out.push(v as f32 / 8_388_608.0);
                    }
                }
            }
        }
    }

    struct Session {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        format: PacketFormat,
    }

    /// Opens a shared-mode loopback client on the default render endpoint.
    unsafe fn open_session() -> Result<Session> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|err| anyhow!("creating MMDeviceEnumerator: {err}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|err| anyhow!("no default output device: {err}"))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|err| anyhow!("activating audio client: {err}"))?;
        let mix = client
            .GetMixFormat()
            .map_err(|err| anyhow!("GetMixFormat: {err}"))?;
        let format = PacketFormat::from_mix_format(mix)?;

        // The mix format is what shared-mode loopback delivers; the conversion
        // flags merely let WASAPI resample if it wants to. Some endpoints reject
        // them, so fall back to a plain loopback stream.
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let converted = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, mix, None);
        let result = if converted.is_ok() {
            Ok(())
        } else {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    0,
                    0,
                    mix,
                    None,
                )
                .map_err(|err| anyhow!("initialising loopback stream: {err}"))
        };
        CoTaskMemFree(Some(mix as *const core::ffi::c_void));
        result?;

        let capture = client
            .GetService::<IAudioCaptureClient>()
            .map_err(|err| anyhow!("getting capture client: {err}"))?;
        client
            .Start()
            .map_err(|err| anyhow!("starting loopback stream: {err}"))?;

        Ok(Session {
            client,
            capture,
            format,
        })
    }

    pub fn capture(
        path: PathBuf,
        max_secs: u32,
        stop: Arc<AtomicBool>,
        ready: std::sync::mpsc::Sender<Result<(), String>>,
    ) -> u32 {
        let _com = match ComInit::new() {
            Ok(com) => com,
            Err(err) => {
                let _ = ready.send(Err(format!("{err:#}")));
                return 0;
            }
        };

        let session = match unsafe { open_session() } {
            Ok(session) => session,
            Err(err) => {
                let _ = ready.send(Err(format!("{err:#}")));
                return 0;
            }
        };
        // From here on the caller owns the handle and can stop us.
        let _ = ready.send(Ok(()));

        let cap_secs = if max_secs == 0 {
            MAX_UNLIMITED_SECS
        } else {
            max_secs
        };
        let mut recorder = Recorder::new(
            session.format.sample_rate,
            session.format.channels as u16,
            cap_secs,
        );
        let limit_frames = u64::from(session.format.sample_rate) * u64::from(cap_secs);
        let mut recorded_frames: u64 = 0;
        let mut converted: Vec<f32> = Vec::new();
        let mut device_error: Option<String> = None;
        let started = Instant::now();

        'capture: loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // Wall-clock guard in case the endpoint stops producing packets.
            if started.elapsed() > Duration::from_secs(u64::from(cap_secs) + 1) {
                break;
            }
            loop {
                let pending = match unsafe { session.capture.GetNextPacketSize() } {
                    Ok(pending) => pending,
                    Err(err) => {
                        device_error = Some(format!("{err}"));
                        break 'capture;
                    }
                };
                if pending == 0 {
                    break;
                }

                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                if let Err(err) = unsafe {
                    session
                        .capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                } {
                    device_error = Some(format!("{err}"));
                    break 'capture;
                }

                let frame_count = frames as usize;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                if silent || data.is_null() {
                    converted.clear();
                    converted.resize(frame_count * session.format.channels, 0.0);
                } else {
                    unsafe {
                        session.format.append(data, frame_count, &mut converted);
                    }
                }
                let _ = unsafe { session.capture.ReleaseBuffer(frames) };

                if frame_count == 0 {
                    continue;
                }
                recorder.push(&converted);
                recorded_frames += frame_count as u64;
                if recorded_frames >= limit_frames {
                    break 'capture;
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let _ = unsafe { session.client.Stop() };
        if let Some(err) = device_error {
            tracing::error!("loopback capture device error: {err}");
        }

        let clip = recorder.finish();
        let duration_ms = clip.audio.duration_ms();
        if clip.audio.samples.is_empty() {
            return 0;
        }
        match write_wav(
            &path,
            clip.audio.sample_rate,
            clip.audio.channels,
            &clip.audio.samples,
        ) {
            Ok(()) => duration_ms.min(u64::from(u32::MAX)) as u32,
            Err(err) => {
                tracing::error!("loopback: saving {}: {err:#}", path.display());
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_cap_is_thirty_minutes() {
        assert_eq!(MAX_UNLIMITED_SECS, 1_800);
    }

    #[test]
    fn start_on_windows_reports_missing_device_or_starts() {
        // Only checks that `start` never panics; the smoke test in
        // `tests/loopback_smoke.rs` covers a real capture when available.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let path = dir.path().join("probe.wav");
        match LoopbackHandle::start(&path, 1) {
            Ok(handle) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = handle.stop();
            }
            Err(err) => {
                assert!(!err.to_string().is_empty());
            }
        }
    }
}

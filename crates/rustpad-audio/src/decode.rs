//! Decoding of audio files to interleaved `f32` PCM using Symphonia.
//!
//! Supported by the enabled Symphonia features: WAV/PCM, MP3, FLAC, OGG-Vorbis,
//! AAC in MP4. Opus decoding is not enabled.

use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::types::DecodedSound;

/// Safety net against corrupt or adversarial files: 60 minutes at 192 kHz.
const MAX_FRAMES: u64 = 60 * 60 * 192_000;

/// Decodes a whole file into memory.
///
/// * Keeps the file's native sample rate and channel count.
/// * All sample formats are converted to interleaved `f32` in `[-1, 1]`.
/// * Returns an error for unsupported/corrupt files; callers should skip and report.
pub fn decode_file(path: &Path) -> Result<DecodedSound> {
    if !path.is_file() {
        bail!("audio file not found: {}", path.display());
    }

    let file =
        File::open(path).with_context(|| format!("opening audio file {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("unsupported or corrupt audio file {}", path.display()))?;

    let mut format = probed.format;

    let track = format
        .default_track()
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| anyhow!("no decodable audio track in {}", path.display()))?;

    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0) as u16;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .with_context(|| format!("unsupported codec in {}", path.display()))?;

    let mut output: Vec<f32> = Vec::new();
    let mut frames: u64 = 0;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut buf_capacity: usize = 0;
    let mut buf_channels: usize = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(err) => return Err(anyhow!("reading packet from {}: {err}", path.display())),
        };

        // Skip non-audio / other tracks.
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A single broken frame should not abort the whole file.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(err) => return Err(anyhow!("decoding {}: {err}", path.display())),
        };

        let spec = *decoded.spec();
        let n_ch = spec.channels.count();
        let cap = decoded.capacity();
        let n_frames = decoded.frames() as u64;

        if sample_rate == 0 {
            sample_rate = spec.rate;
        }
        if channels == 0 {
            channels = n_ch.min(u16::MAX as usize) as u16;
        }

        if frames.saturating_add(n_frames) > MAX_FRAMES {
            bail!(
                "audio file {} is too long or corrupt (more than {} frames)",
                path.display(),
                MAX_FRAMES
            );
        }

        if sample_buf.is_none() || cap > buf_capacity || n_ch != buf_channels {
            sample_buf = Some(SampleBuffer::<f32>::new(cap as u64, spec));
            buf_capacity = cap;
            buf_channels = n_ch;
        }
        let buf = sample_buf
            .as_mut()
            .expect("sample buffer initialised above");
        buf.copy_interleaved_ref(decoded);

        let n_samples = buf.len();
        output
            .try_reserve(n_samples)
            .map_err(|err| anyhow!("out of memory decoding {}: {err}", path.display()))?;
        output.extend_from_slice(buf.samples());
        frames += n_frames;
    }

    if frames == 0 || output.is_empty() {
        bail!("no audio frames decoded from {}", path.display());
    }
    if sample_rate == 0 || channels == 0 {
        bail!("missing stream format for {}", path.display());
    }

    // Drop any trailing partial frame.
    let ch = channels as usize;
    output.truncate(output.len() - output.len() % ch);

    Ok(DecodedSound {
        sample_rate,
        channels,
        samples: output,
    })
}

/// Extension check used by "Add files" dialogs and drag & drop.
///
/// `opus` is intentionally *not* supported: Symphonia's Opus support is not
/// enabled in this build, so claiming it would produce decode failures.
pub fn is_supported_extension(ext: &str) -> bool {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "wav" | "mp3" | "flac" | "ogg" | "oga" | "ogv" | "ogx" | "m4a" | "mp4" | "aac" | "wma"
    )
}

fn probe_track_duration_ms(track: &symphonia::core::formats::Track) -> Option<u64> {
    let n_frames = track.codec_params.n_frames?;
    let time_base = track.codec_params.time_base?;
    if time_base.numer == 0 || time_base.denom == 0 {
        return None;
    }
    let time = time_base.calc_time(n_frames);
    let ms = time
        .seconds
        .saturating_mul(1000)
        .saturating_add((time.frac * 1000.0).round() as u64);
    Some(ms)
}

/// Duration in milliseconds, using the container metadata when available and
/// falling back to a full decode.
pub fn probe_duration_ms(path: &Path) -> Result<u64> {
    if let Ok(ms) = probe_from_metadata(path) {
        return Ok(ms);
    }
    Ok(decode_file(path)?.duration_ms())
}

fn probe_from_metadata(path: &Path) -> Result<u64> {
    if !path.is_file() {
        bail!("audio file not found: {}", path.display());
    }

    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let format: Box<dyn FormatReader> = probed.format;
    let track = format
        .default_track()
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| anyhow!("no audio track in {}", path.display()))?;

    if let Some(ms) = probe_track_duration_ms(track) {
        return Ok(ms);
    }

    // Some containers omit `n_frames` but still know the sample rate. We can't
    // know the frame count without decoding, so report the gap as unknown.
    bail!("no duration metadata in {}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_test_wav(path: &Path, sr: u32, ch: u16, frames: usize) {
        let spec = hound::WavSpec {
            channels: ch,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for f in 0..frames {
            for c in 0..ch {
                let v = ((f as f32 * 0.01 + c as f32) * 1000.0).sin() * 0.5;
                writer.write_sample((v * 32767.0) as i16).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn supported_extensions() {
        for ext in [
            "wav", "WAV", ".mp3", "flac", "ogg", "oga", "ogv", "ogx", "m4a", "mp4", "aac", "wma",
        ] {
            assert!(is_supported_extension(ext), "{ext} should be supported");
        }
        for ext in ["opus", "txt", "exe", ""] {
            assert!(
                !is_supported_extension(ext),
                "{ext} should not be supported"
            );
        }
    }

    #[test]
    fn decodes_wav_native_rate_and_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.WAV");
        write_test_wav(&path, 22_050, 2, 4410);

        let sound = decode_file(&path).unwrap();
        assert_eq!(sound.sample_rate, 22_050);
        assert_eq!(sound.channels, 2);
        assert_eq!(sound.frames(), 4410);
        assert!(sound.samples.iter().all(|s| s.is_finite()));
        assert!(sound.duration_ms() >= 199 && sound.duration_ms() <= 200);
    }

    #[test]
    fn probe_uses_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.wav");
        write_test_wav(&path, 8_000, 1, 4_000);
        let ms = probe_duration_ms(&path).unwrap();
        assert_eq!(ms, 500);
    }

    #[test]
    fn rejects_missing_and_garbage_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.wav");
        assert!(decode_file(&missing).is_err());

        let garbage = dir.path().join("garbage.wav");
        let mut f = File::create(&garbage).unwrap();
        f.write_all(&[0u8; 64]).unwrap();
        drop(f);
        assert!(decode_file(&garbage).is_err());
        assert!(probe_duration_ms(&garbage).is_err());
    }
}

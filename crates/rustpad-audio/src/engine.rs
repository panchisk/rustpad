//! WASAPI device layer built on cpal.
//!
//! The engine owns all cpal streams on a dedicated thread. UI, hotkeys and the
//! remote-control server talk to it through [`EngineHandle`] (commands in,
//! events + status out). Two independent output streams are supported:
//!
//! * **Speakers** — normal playback.
//! * **Microphone target** — usually a virtual cable input device. The real
//!   microphone is captured and mixed in, so applications selecting the cable's
//!   output as their microphone hear both the voice and the soundboard.
//!
//! This replaces Soundpad's signed SFX APO; no driver installation is needed.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;

use crate::decode::decode_file;
use crate::loopback::LoopbackHandle;
use crate::mixer::Mixer;
use crate::record::Recorder;
use crate::resample::StreamResampler;
use crate::types::*;
use crate::wav::write_wav;

const MAX_MIC_RING_SECONDS: f32 = 3.0;
const ENGINE_TICK: Duration = Duration::from_millis(10);

struct Shared {
    cmd_tx: Sender<EngineCommand>,
    evt_rx: Receiver<EngineEvent>,
    status: Arc<Mutex<PlaybackStatus>>,
    mic_level: Arc<AtomicU32>,
    out_level: Arc<AtomicU32>,
    shutdown: AtomicBool,
    join: Mutex<Option<JoinHandle<()>>>,
}

/// Cheap, cloneable handle used by the UI, hotkey thread and pipe server.
#[derive(Clone)]
pub struct EngineHandle {
    shared: Arc<Shared>,
}

impl EngineHandle {
    pub fn spawn(config: EngineConfig) -> Self {
        let (cmd_tx, cmd_rx) = bounded(256);
        let (evt_tx, evt_rx) = bounded(512);
        let status = Arc::new(Mutex::new(PlaybackStatus::default()));
        let mic_level = Arc::new(AtomicU32::new(0));
        let out_level = Arc::new(AtomicU32::new(0));

        let thread_status = status.clone();
        let thread_mic = mic_level.clone();
        let thread_out = out_level.clone();
        let thread_evt = evt_tx.clone();
        let engine_cmd_tx = cmd_tx.clone();
        let join = std::thread::Builder::new()
            .name("rustpad-audio".into())
            .spawn(move || {
                match Engine::new(
                    config,
                    cmd_rx,
                    engine_cmd_tx,
                    thread_evt.clone(),
                    thread_status,
                    thread_mic,
                    thread_out,
                ) {
                    Ok(engine) => engine.run(),
                    Err(err) => {
                        let _ = thread_evt.send(EngineEvent::Error(format!("{err:#}")));
                    }
                }
            })
            .expect("spawn audio thread");

        EngineHandle {
            shared: Arc::new(Shared {
                cmd_tx,
                evt_rx,
                status,
                mic_level,
                out_level,
                shutdown: AtomicBool::new(false),
                join: Mutex::new(Some(join)),
            }),
        }
    }

    pub fn send(&self, cmd: EngineCommand) {
        let _ = self.shared.cmd_tx.try_send(cmd);
    }

    pub fn try_event(&self) -> Option<EngineEvent> {
        self.shared.evt_rx.try_recv().ok()
    }

    pub fn status(&self) -> PlaybackStatus {
        self.shared.status.lock().clone()
    }

    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.shared.mic_level.load(Ordering::Relaxed))
    }

    pub fn output_level(&self) -> f32 {
        f32::from_bits(self.shared.out_level.load(Ordering::Relaxed))
    }

    pub fn is_shutdown(&self) -> bool {
        self.shared.shutdown.load(Ordering::SeqCst)
    }

    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = self.shared.cmd_tx.send(EngineCommand::Shutdown);
        if let Some(join) = self.shared.join.lock().take() {
            let _ = join.join();
        }
    }

    /// Enumerates devices for the UI. Safe to call from any thread.
    pub fn devices() -> (Vec<DeviceInfo>, Vec<DeviceInfo>) {
        enumerate_devices()
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) == 1 {
            let _ = self.shared.cmd_tx.send(EngineCommand::Shutdown);
        }
    }
}

pub fn enumerate_devices() -> (Vec<DeviceInfo>, Vec<DeviceInfo>) {
    let host = cpal::default_host();
    let default_out_name = host.default_output_device().and_then(|d| d.name().ok());
    let default_in_name = host.default_input_device().and_then(|d| d.name().ok());

    let mut outputs = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            let (ch, sr) = d
                .default_output_config()
                .map(|c| (c.channels(), c.sample_rate().0))
                .unwrap_or((2, 48_000));
            outputs.push(DeviceInfo {
                is_default: Some(&name) == default_out_name.as_ref(),
                name,
                kind: DeviceKind::Output,
                channels: ch,
                sample_rate: sr,
            });
        }
    }
    let mut inputs = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            let (ch, sr) = d
                .default_input_config()
                .map(|c| (c.channels(), c.sample_rate().0))
                .unwrap_or((1, 48_000));
            inputs.push(DeviceInfo {
                is_default: Some(&name) == default_in_name.as_ref(),
                name,
                kind: DeviceKind::Input,
                channels: ch,
                sample_rate: sr,
            });
        }
    }
    (outputs, inputs)
}

fn find_output(host: &cpal::Host, name: &Option<String>) -> Option<Device> {
    if let Some(wanted) = name {
        if let Ok(devices) = host.output_devices() {
            for d in devices {
                if d.name().ok().as_deref() == Some(wanted.as_str()) {
                    return Some(d);
                }
            }
        }
    }
    host.default_output_device()
}

fn find_input(host: &cpal::Host, name: &Option<String>) -> Option<Device> {
    if let Some(wanted) = name {
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                if d.name().ok().as_deref() == Some(wanted.as_str()) {
                    return Some(d);
                }
            }
        }
    }
    host.default_input_device()
}

struct Output {
    _stream: cpal::Stream,
    mixer: Arc<Mutex<Mixer>>,
    sample_rate: u32,
    #[allow(dead_code)]
    target: Target,
}

struct MicBus {
    ring: VecDeque<f32>,
    capacity: usize,
    enabled: bool,
    block_voice: bool,
    gain: f32,
    delay_frames: usize,
}

impl MicBus {
    fn new(sample_rate: u32, delay_ms: u32) -> Self {
        let capacity = (sample_rate as f32 * MAX_MIC_RING_SECONDS) as usize * 2;
        let mut bus = MicBus {
            ring: VecDeque::with_capacity(capacity),
            capacity,
            enabled: false,
            block_voice: false,
            gain: 1.0,
            delay_frames: 0,
        };
        bus.set_delay(sample_rate, delay_ms);
        bus
    }

    fn set_delay(&mut self, sample_rate: u32, delay_ms: u32) {
        self.delay_frames = (sample_rate as u64 * delay_ms as u64 / 1000) as usize;
        self.ring.clear();
        for _ in 0..self.delay_frames * 2 {
            self.ring.push_back(0.0);
        }
    }

    fn push_mono(&mut self, samples: &[f32]) {
        for &s in samples {
            self.ring.push_back(s);
            self.ring.push_back(s);
            while self.ring.len() > self.capacity {
                self.ring.pop_front();
            }
        }
    }

    fn add_to(&mut self, out: &mut [f32], frames: usize, block: bool) {
        if !self.enabled || (block && self.block_voice) {
            // Keep the ring in sync with real time: discard instead of letting
            // stale audio accumulate while passthrough is off.
            for _ in 0..frames * 2 {
                self.ring.pop_front();
            }
            return;
        }
        for f in 0..frames {
            let l = self.ring.pop_front().unwrap_or(0.0) * self.gain;
            let r = self.ring.pop_front().unwrap_or(0.0) * self.gain;
            out[f * 2] += l;
            out[f * 2 + 1] += r;
        }
    }
}

struct Engine {
    cmd_rx: Receiver<EngineCommand>,
    cmd_tx: Sender<EngineCommand>,
    evt_tx: Sender<EngineEvent>,
    status: Arc<Mutex<PlaybackStatus>>,
    mic_level: Arc<AtomicU32>,
    out_level: Arc<AtomicU32>,

    config: EngineConfig,
    speakers: Option<Output>,
    mic_output: Option<Output>,
    mic_bus: Arc<Mutex<MicBus>>,

    recorder: Arc<Mutex<Option<Recorder>>>,
    recording_path: Option<PathBuf>,
    recording_limit_ms: u64,
    loopback: Option<LoopbackHandle>,
    loopback_path: Option<PathBuf>,
    loopback_started: Option<Instant>,
    capture: Option<cpal::Stream>,

    last_sound: Option<u32>,
    last_voice: Option<crate::mixer::VoiceId>,
    /// Bumped by every Play and StopAll; worker decodes from older generations
    /// must not start after a newer request or a stop.
    generation: u64,
    paused: bool,
}

impl Engine {
    fn new(
        config: EngineConfig,
        cmd_rx: Receiver<EngineCommand>,
        cmd_tx: Sender<EngineCommand>,
        evt_tx: Sender<EngineEvent>,
        status: Arc<Mutex<PlaybackStatus>>,
        mic_level: Arc<AtomicU32>,
        out_level: Arc<AtomicU32>,
    ) -> Result<Self> {
        let mut engine = Engine {
            cmd_rx,
            cmd_tx,
            evt_tx,
            status,
            mic_level,
            out_level,
            mic_bus: Arc::new(Mutex::new(MicBus::new(48_000, config.mic_delay_ms))),
            recorder: Arc::new(Mutex::new(None)),
            recording_path: None,
            recording_limit_ms: 0,
            loopback: None,
            loopback_path: None,
            loopback_started: None,
            capture: None,
            last_sound: None,
            last_voice: None,
            generation: 0,
            paused: false,
            config,
            speakers: None,
            mic_output: None,
        };
        engine.open_streams()?;
        Ok(engine)
    }

    fn open_streams(&mut self) -> Result<()> {
        self.speakers = None;
        self.mic_output = None;

        let host = cpal::default_host();

        if let Some(device) = find_output(&host, &self.config.output_device) {
            let mixer = Arc::new(Mutex::new(Mixer::new(48_000)));
            match build_output(
                device,
                Target::Speakers,
                mixer,
                self.mic_bus.clone(),
                self.out_level.clone(),
                false,
            ) {
                Ok(out) => self.speakers = Some(out),
                Err(err) => {
                    let _ = self
                        .evt_tx
                        .send(EngineEvent::Error(format!("output device: {err:#}")));
                }
            }
        }

        let mic_name = self.config.mic_output_device.clone();
        if mic_name.is_some() {
            if let Some(device) = find_output(&host, &mic_name) {
                let mixer = Arc::new(Mutex::new(Mixer::new(48_000)));
                match build_output(
                    device,
                    Target::Microphone,
                    mixer,
                    self.mic_bus.clone(),
                    self.out_level.clone(),
                    true,
                ) {
                    Ok(out) => self.mic_output = Some(out),
                    Err(err) => {
                        let _ = self
                            .evt_tx
                            .send(EngineEvent::Error(format!("microphone output: {err:#}")));
                    }
                }
            } else {
                let _ = self.evt_tx.send(EngineEvent::Error(format!(
                    "microphone output device not found: {}",
                    mic_name.unwrap_or_default()
                )));
            }
        }

        let mic_rate = self
            .mic_output
            .as_ref()
            .or(self.speakers.as_ref())
            .map(|o| o.sample_rate)
            .unwrap_or(48_000);
        {
            let mut bus = self.mic_bus.lock();
            *bus = MicBus::new(mic_rate, 0);
            bus.enabled = self.config.mic_passthrough;
            bus.gain = self.config.mic_passthrough_gain;
        }

        self.capture = None;
        if let Some(device) = find_input(&host, &self.config.capture_device) {
            match build_input(
                device,
                mic_rate,
                self.mic_bus.clone(),
                self.recorder.clone(),
                self.mic_level.clone(),
            ) {
                Ok(stream) => self.capture = Some(stream),
                Err(err) => {
                    let _ = self
                        .evt_tx
                        .send(EngineEvent::Error(format!("capture device: {err:#}")));
                }
            }
        }

        self.apply_volumes();
        Ok(())
    }

    fn apply_volumes(&mut self) {
        let master = self.config.master_volume.clamp(0.0, 4.0);
        if let Some(out) = &self.speakers {
            out.mixer
                .lock()
                .set_master(master * self.config.speakers_volume);
        }
        if let Some(out) = &self.mic_output {
            out.mixer.lock().set_master(master * self.config.mic_volume);
        }
        let mut bus = self.mic_bus.lock();
        if bus.enabled != self.config.mic_passthrough {
            // Drop anything captured while the routing was different.
            bus.ring.clear();
            for _ in 0..bus.delay_frames * 2 {
                bus.ring.push_back(0.0);
            }
        }
        bus.enabled = self.config.mic_passthrough;
        bus.block_voice = self.config.block_voice;
        bus.gain = self.config.mic_passthrough_gain;
    }

    fn mixer_for(&mut self, target: Target) -> Option<Arc<Mutex<Mixer>>> {
        match target {
            Target::Speakers => self.speakers.as_ref().map(|o| o.mixer.clone()),
            Target::Microphone => self
                .mic_output
                .as_ref()
                .or(self.speakers.as_ref())
                .map(|o| o.mixer.clone()),
        }
    }

    fn start_playback(&mut self, request: PlayRequest, audio: Arc<DecodedSound>) {
        if audio.samples.is_empty() {
            let _ = self.evt_tx.send(EngineEvent::Error("empty sound".into()));
            return;
        }
        let targets = if request.targets.is_empty() {
            vec![Target::Speakers]
        } else {
            request.targets.clone()
        };
        let mut last_voice = None;
        for target in targets {
            if let Some(mixer) = self.mixer_for(target) {
                let mut m = mixer.lock();
                let delay = if target == Target::Microphone {
                    self.config.mic_delay_ms
                } else {
                    0
                };
                last_voice = Some(m.play_tagged(
                    request.sound_id,
                    audio.clone(),
                    request.gain,
                    request.looping,
                    request.start_ms,
                    delay,
                ));
            }
        }
        if let Some(voice) = last_voice {
            self.last_voice = Some(voice);
            self.last_sound = Some(request.sound_id);
            self.paused = false;
            let _ = self.evt_tx.send(EngineEvent::Started {
                sound_id: request.sound_id,
            });
        } else {
            let _ = self
                .evt_tx
                .send(EngineEvent::Error("no output device available".to_string()));
        }
    }

    fn run(mut self) {
        loop {
            match self.cmd_rx.recv_timeout(ENGINE_TICK) {
                Ok(EngineCommand::Shutdown) => break,
                Ok(cmd) => self.handle(cmd),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            self.collect_finished();
            self.maybe_finish_recording();
            self.publish_status();
        }
        // Stop a running loopback capture cleanly before the thread ends.
        if let Some(handle) = self.loopback.take() {
            let _ = handle.stop();
        }
        self.loopback_path = None;
        self.loopback_started = None;
    }

    fn handle(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play(req) => {
                self.generation = self.generation.wrapping_add(1);
                let generation = self.generation;
                let source = req.source.clone();
                if let PlaySource::Decoded(audio) = &source {
                    self.start_playback(req, audio.clone());
                    return;
                }
                if let Some(path) = source.path().map(|p| p.to_path_buf()) {
                    // Decode off the engine thread so pause/stop stay responsive.
                    let evt_tx = self.evt_tx.clone();
                    let cmd_tx = self.cmd_tx.clone();
                    std::thread::spawn(move || match decode_file(&path) {
                        Ok(audio) => {
                            let _ = cmd_tx.send(EngineCommand::PlayDecodedAt {
                                request: req,
                                audio: Arc::new(audio),
                                generation,
                            });
                        }
                        Err(err) => {
                            let _ = evt_tx
                                .send(EngineEvent::Error(format!("{}: {err:#}", path.display())));
                        }
                    });
                }
            }
            EngineCommand::PlayDecoded { request, audio } => {
                self.generation = self.generation.wrapping_add(1);
                self.start_playback(request, audio);
            }
            EngineCommand::PlayDecodedAt {
                request,
                audio,
                generation,
            } => {
                if generation == self.generation {
                    self.start_playback(request, audio);
                }
            }
            EngineCommand::StopAll => {
                // Invalidate decodes that are still running in worker threads.
                self.generation = self.generation.wrapping_add(1);
                for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    out.mixer.lock().stop_all();
                }
                self.paused = false;
            }
            EngineCommand::StopSound(id) => {
                for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    out.mixer.lock().stop_sound(id);
                }
            }
            EngineCommand::TogglePause => {
                if self.paused {
                    self.handle(EngineCommand::ResumeAll);
                } else {
                    self.handle(EngineCommand::PauseAll);
                }
            }
            EngineCommand::PauseAll => {
                for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    out.mixer.lock().pause_all();
                }
                self.paused = true;
            }
            EngineCommand::ResumeAll => {
                for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    out.mixer.lock().resume_all();
                }
                self.paused = false;
            }
            EngineCommand::Seek(_sound_id, ms) => {
                if let Some(voice) = self.last_voice {
                    for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        out.mixer.lock().seek(voice, ms);
                    }
                }
            }
            EngineCommand::SetMasterVolume(v) => {
                self.config.master_volume = v;
                self.apply_volumes();
            }
            EngineCommand::SetTargetVolume(target, v) => {
                match target {
                    Target::Speakers => self.config.speakers_volume = v,
                    Target::Microphone => self.config.mic_volume = v,
                }
                self.apply_volumes();
            }
            EngineCommand::SetMicPassthrough { enabled, gain } => {
                self.config.mic_passthrough = enabled;
                self.config.mic_passthrough_gain = gain;
                self.apply_volumes();
            }
            EngineCommand::SetMicDelayMs(ms) => {
                // Applies to sounds started after this change (microphone target only).
                self.config.mic_delay_ms = ms;
            }
            EngineCommand::SetBlockVoice(v) => {
                self.config.block_voice = v;
                self.mic_bus.lock().block_voice = v;
            }
            EngineCommand::StartRecording { path, max_secs } => {
                if self.loopback.is_some() {
                    let _ = self
                        .evt_tx
                        .send(EngineEvent::Error("recording already in progress".into()));
                    return;
                }
                let sr = self
                    .mic_output
                    .as_ref()
                    .or(self.speakers.as_ref())
                    .map(|o| o.sample_rate)
                    .unwrap_or(48_000);
                *self.recorder.lock() = Some(Recorder::new(sr, 1, max_secs));
                self.recording_path = Some(path);
                self.recording_limit_ms = max_secs as u64 * 1000;
                let _ = self.evt_tx.send(EngineEvent::RecordingStarted);
            }
            EngineCommand::StopRecording => self.stop_recording(false),
            EngineCommand::StartLoopbackRecording { path, max_secs } => {
                if self.loopback.is_some() || self.recorder.lock().is_some() {
                    let _ = self
                        .evt_tx
                        .send(EngineEvent::Error("recording already in progress".into()));
                    return;
                }
                match LoopbackHandle::start(path.clone(), max_secs) {
                    Ok(handle) => {
                        self.loopback = Some(handle);
                        self.loopback_path = Some(path);
                        self.loopback_started = Some(Instant::now());
                        // `0` means "until stopped", still capped at 30 minutes.
                        let effective_secs = if max_secs == 0 {
                            crate::loopback::MAX_UNLIMITED_SECS
                        } else {
                            max_secs
                        };
                        self.recording_limit_ms = u64::from(effective_secs) * 1000;
                        let _ = self.evt_tx.send(EngineEvent::RecordingStarted);
                    }
                    Err(err) => {
                        let _ = self
                            .evt_tx
                            .send(EngineEvent::Error(format!("loopback: {err:#}")));
                    }
                }
            }
            EngineCommand::SetDevices(cfg) => {
                self.config = cfg;
                if let Err(err) = self.open_streams() {
                    let _ = self.evt_tx.send(EngineEvent::Error(format!("{err:#}")));
                }
            }
            EngineCommand::Shutdown => {}
        }
    }

    fn stop_recording(&mut self, _limit: bool) {
        if let Some(handle) = self.loopback.take() {
            let path = self.loopback_path.take();
            self.loopback_started = None;
            self.recording_limit_ms = 0;
            let duration_ms = u64::from(handle.stop());
            if let Some(path) = path {
                if duration_ms > 0 {
                    let _ = self
                        .evt_tx
                        .send(EngineEvent::RecordingFinished { path, duration_ms });
                } else {
                    let _ = self.evt_tx.send(EngineEvent::Error(
                        "loopback: no audio captured".to_string(),
                    ));
                }
            }
            return;
        }

        let recorder = self.recorder.lock().take();
        let Some(recorder) = recorder else { return };
        let path = self.recording_path.take();
        self.recording_limit_ms = 0;
        let clip = recorder.finish();
        let duration_ms = clip.audio.duration_ms();
        if let Some(path) = path {
            if duration_ms > 0 {
                match write_wav(
                    &path,
                    clip.audio.sample_rate,
                    clip.audio.channels,
                    &clip.audio.samples,
                ) {
                    Ok(()) => {
                        let _ = self
                            .evt_tx
                            .send(EngineEvent::RecordingFinished { path, duration_ms });
                    }
                    Err(err) => {
                        let _ = self
                            .evt_tx
                            .send(EngineEvent::Error(format!("saving recording: {err:#}")));
                    }
                }
            }
        }
    }

    fn maybe_finish_recording(&mut self) {
        if self.loopback.is_some() {
            if self.recording_limit_ms == 0 {
                return;
            }
            let reached = self
                .loopback_started
                .map(|started| started.elapsed().as_millis() as u64 >= self.recording_limit_ms)
                .unwrap_or(false);
            if reached {
                self.stop_recording(true);
            }
            return;
        }
        if self.recording_limit_ms == 0 {
            return;
        }
        let reached = self
            .recorder
            .lock()
            .as_ref()
            .map(|rec| rec.duration_ms() >= self.recording_limit_ms)
            .unwrap_or(false);
        if reached {
            self.stop_recording(true);
        }
    }

    fn collect_finished(&mut self) {
        let mut any_finished = false;
        let mut any_active = false;
        for out in [self.speakers.as_ref(), self.mic_output.as_ref()]
            .into_iter()
            .flatten()
        {
            let mut m = out.mixer.lock();
            if !m.take_finished().is_empty() {
                any_finished = true;
            }
            if m.voice_count() > 0 {
                any_active = true;
            }
        }
        if any_finished && !any_active {
            if let Some(sound_id) = self.last_sound.take() {
                let _ = self.evt_tx.send(EngineEvent::Finished { sound_id });
                self.last_voice = None;
            }
        }
    }

    fn publish_status(&self) {
        let mut position = 0;
        let mut duration = 0;
        let mut voices = 0;
        let mut paused = self.paused;
        if let Some(out) = self.speakers.as_ref().or(self.mic_output.as_ref()) {
            let m = out.mixer.lock();
            position = m.position_ms(self.last_voice);
            duration = m.duration_ms(self.last_voice);
            voices = m.voice_count();
            paused = paused || m.is_paused();
        }
        let mut s = self.status.lock();
        s.sound_id = self.last_sound;
        s.position_ms = position;
        s.duration_ms = duration;
        s.paused = paused;
        s.voices = voices;
    }
}

fn build_output(
    device: Device,
    target: Target,
    mixer: Arc<Mutex<Mixer>>,
    mic_bus: Arc<Mutex<MicBus>>,
    out_level: Arc<AtomicU32>,
    consumes_mic: bool,
) -> Result<Output> {
    let name = device.name().unwrap_or_default();
    let supported = device
        .default_output_config()
        .with_context(|| format!("default output config for {name}"))?;
    let sample_format = supported.sample_format();
    let channels = supported.channels().max(1) as usize;
    let sample_rate = supported.sample_rate().0;

    {
        let mut m = mixer.lock();
        if m.sample_rate() != sample_rate {
            *m = Mixer::new(sample_rate);
        }
    }

    let config = StreamConfig {
        channels: supported.channels(),
        sample_rate: SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let err_fn = move |err| tracing::error!(?target, "output stream error: {err}");
    let mixer_cb = mixer.clone();
    let bus_cb = mic_bus.clone();
    let level_cb = out_level.clone();
    let stream = match sample_format {
        SampleFormat::F32 => device.build_output_stream::<f32, _, _>(
            &config,
            move |data: &mut [f32], _| {
                render_output(data, channels, &mixer_cb, &bus_cb, &level_cb, consumes_mic);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_output_stream::<i16, _, _>(
            &config,
            move |data: &mut [i16], _| {
                let mut tmp = vec![0f32; data.len()];
                render_output(
                    &mut tmp,
                    channels,
                    &mixer_cb,
                    &bus_cb,
                    &level_cb,
                    consumes_mic,
                );
                for (d, s) in data.iter_mut().zip(tmp) {
                    *d = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                }
            },
            err_fn,
            None,
        )?,
        other => return Err(anyhow!("unsupported output sample format {other:?}")),
    };
    stream.play().context("starting output stream")?;
    Ok(Output {
        _stream: stream,
        mixer,
        sample_rate,
        target,
    })
}

fn render_output(
    data: &mut [f32],
    channels: usize,
    mixer: &Arc<Mutex<Mixer>>,
    mic_bus: &Arc<Mutex<MicBus>>,
    level: &Arc<AtomicU32>,
    consumes_mic: bool,
) {
    let frames = data.len() / channels;
    if frames == 0 {
        return;
    }
    let mut stereo = vec![0f32; frames * 2];
    let block = {
        let mut m = mixer.lock();
        let block = m.has_active();
        m.render(frames, &mut stereo);
        block
    };
    if consumes_mic {
        mic_bus.lock().add_to(&mut stereo, frames, block);
    }
    let mut peak = 0f32;
    if channels == 2 {
        for (i, s) in stereo.iter().enumerate() {
            data[i] = *s;
            peak = peak.max(s.abs());
        }
    } else {
        for f in 0..frames {
            let l = stereo[f * 2];
            let r = stereo[f * 2 + 1];
            peak = peak.max(l.abs()).max(r.abs());
            for c in 0..channels {
                data[f * channels + c] = match c {
                    0 => l,
                    1 => r,
                    _ => 0.0,
                };
            }
        }
    }
    level.store(peak.to_bits(), Ordering::Relaxed);
}

fn build_input(
    device: Device,
    out_rate: u32,
    mic_bus: Arc<Mutex<MicBus>>,
    recorder: Arc<Mutex<Option<Recorder>>>,
    mic_level: Arc<AtomicU32>,
) -> Result<cpal::Stream> {
    let name = device.name().unwrap_or_default();
    let supported = device
        .default_input_config()
        .with_context(|| format!("default input config for {name}"))?;
    let sample_format = supported.sample_format();
    let channels = supported.channels().max(1) as usize;
    let in_rate = supported.sample_rate().0;
    let config = StreamConfig {
        channels: supported.channels(),
        sample_rate: SampleRate(in_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let resampler = Arc::new(Mutex::new(StreamResampler::new(in_rate, out_rate, 1)));
    let peak = Arc::new(Mutex::new(0f32));

    fn process_block(
        data: &[f32],
        channels: usize,
        resampler: &Mutex<StreamResampler>,
        peak: &Mutex<f32>,
        mic_level: &AtomicU32,
        mic_bus: &Mutex<MicBus>,
        recorder: &Mutex<Option<Recorder>>,
    ) {
        let frames = data.len() / channels;
        let mono: Vec<f32> = (0..frames)
            .map(|f| {
                let mut sum = 0.0;
                for c in 0..channels {
                    sum += data[f * channels + c];
                }
                sum / channels as f32
            })
            .collect();
        let converted = resampler.lock().process(&mono);
        if converted.is_empty() {
            return;
        }
        {
            let mut p = peak.lock();
            *p = converted.iter().fold(*p * 0.7, |a, s| a.max(s.abs()));
            mic_level.store(p.to_bits(), Ordering::Relaxed);
        }
        mic_bus.lock().push_mono(&converted);
        if let Some(rec) = recorder.lock().as_mut() {
            rec.push(&converted);
        }
    }

    let err_fn = |err| tracing::error!("input stream error: {err}");
    let stream = match sample_format {
        SampleFormat::F32 => {
            let resampler = resampler.clone();
            let peak = peak.clone();
            let mic_level = mic_level.clone();
            let mic_bus = mic_bus.clone();
            let recorder = recorder.clone();
            device.build_input_stream::<f32, _, _>(
                &config,
                move |data: &[f32], _| {
                    process_block(
                        data, channels, &resampler, &peak, &mic_level, &mic_bus, &recorder,
                    )
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let resampler = resampler.clone();
            let peak = peak.clone();
            let mic_level = mic_level.clone();
            let mic_bus = mic_bus.clone();
            let recorder = recorder.clone();
            device.build_input_stream::<i16, _, _>(
                &config,
                move |data: &[i16], _| {
                    let tmp: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                    process_block(
                        &tmp, channels, &resampler, &peak, &mic_level, &mic_bus, &recorder,
                    )
                },
                err_fn,
                None,
            )?
        }
        other => return Err(anyhow!("unsupported input sample format {other:?}")),
    };
    stream.play().context("starting input stream")?;
    Ok(stream)
}

#[cfg(test)]
mod mic_bus_tests {
    use super::MicBus;

    #[test]
    fn microphone_keeps_flowing_while_a_sound_plays_by_default() {
        let mut bus = MicBus::new(48_000, 0);
        bus.enabled = true;
        bus.push_mono(&[0.5; 480]);
        let mut out = vec![0.0; 960];
        bus.add_to(&mut out, 480, true);
        assert!(
            out[0].abs() > 0.4,
            "mic must not be muted by active playback"
        );
    }

    #[test]
    fn block_voice_drops_microphone_only_when_enabled() {
        let mut bus = MicBus::new(48_000, 0);
        bus.enabled = true;
        bus.block_voice = true;
        bus.push_mono(&[0.5; 480]);
        let mut out = vec![0.0; 960];
        bus.add_to(&mut out, 480, true);
        assert_eq!(out[0], 0.0);
    }
}

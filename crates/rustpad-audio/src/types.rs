//! Shared audio types.

use std::path::Path;

/// Decoded PCM: interleaved `f32` samples in `[-1.0, 1.0]`.
#[derive(Clone, Debug)]
pub struct DecodedSound {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl DecodedSound {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            0
        } else {
            (self.frames() as u64 * 1000) / self.sample_rate as u64
        }
    }

    /// Returns a stereo interleaved copy (mono is duplicated, >2ch keeps L/R).
    pub fn to_stereo(&self) -> DecodedSound {
        let ch = self.channels.max(1) as usize;
        let mut out = Vec::with_capacity(self.frames() * 2);
        for f in 0..self.frames() {
            let base = f * ch;
            let l = self.samples[base];
            let r = if ch >= 2 { self.samples[base + 1] } else { l };
            out.push(l);
            out.push(r);
        }
        DecodedSound {
            sample_rate: self.sample_rate,
            channels: 2,
            samples: out,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub kind: DeviceKind,
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Speakers,
    Microphone,
}

impl Target {
    pub fn all() -> [Target; 2] {
        [Target::Speakers, Target::Microphone]
    }
}

/// Where the audio for a new voice comes from.
#[derive(Debug, Clone)]
pub enum PlaySource {
    File(std::path::PathBuf),
    Decoded(std::sync::Arc<DecodedSound>),
}

impl PlaySource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            PlaySource::File(p) => Some(p),
            PlaySource::Decoded(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub sound_id: u32,
    pub source: PlaySource,
    /// Linear gain already includes loudness normalisation.
    pub gain: f32,
    pub targets: Vec<Target>,
    pub looping: bool,
    pub start_ms: u64,
}

/// Commands accepted by the engine. Sent from UI, hotkeys and the remote-control server.
#[derive(Debug, Clone)]
pub enum EngineCommand {
    Play(PlayRequest),
    /// A request whose source was already decoded on a worker thread.
    PlayDecoded {
        request: PlayRequest,
        audio: std::sync::Arc<DecodedSound>,
    },
    /// Internal: a worker-decoded sound tagged with the play generation it was
    /// requested under. Stale generations are dropped (see `Engine`).
    PlayDecodedAt {
        request: PlayRequest,
        audio: std::sync::Arc<DecodedSound>,
        generation: u64,
    },
    StopAll,
    StopSound(u32),
    TogglePause,
    PauseAll,
    ResumeAll,
    Seek(u32, u64),
    SetMasterVolume(f32),
    SetTargetVolume(Target, f32),
    SetMicPassthrough {
        enabled: bool,
        gain: f32,
    },
    SetMicDelayMs(u32),
    SetBlockVoice(bool),
    StartRecording {
        path: std::path::PathBuf,
        max_secs: u32,
    },
    StopRecording,
    /// Records what the default output device plays (WASAPI loopback).
    StartLoopbackRecording {
        path: std::path::PathBuf,
        max_secs: u32,
    },
    /// Re-opens output/capture streams with a new routing configuration.
    SetDevices(EngineConfig),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    Started {
        sound_id: u32,
    },
    Finished {
        sound_id: u32,
    },
    RecordingStarted,
    RecordingFinished {
        path: std::path::PathBuf,
        duration_ms: u64,
    },
    Error(String),
    MicLevel(f32),
    OutputLevel(f32),
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub output_device: Option<String>,
    pub mic_output_device: Option<String>,
    pub capture_device: Option<String>,
    pub master_volume: f32,
    pub speakers_volume: f32,
    pub mic_volume: f32,
    pub mic_passthrough: bool,
    pub mic_passthrough_gain: f32,
    pub mic_delay_ms: u32,
    pub block_voice: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            output_device: None,
            mic_output_device: None,
            capture_device: None,
            master_volume: 0.8,
            speakers_volume: 0.8,
            mic_volume: 0.8,
            mic_passthrough: false,
            mic_passthrough_gain: 1.0,
            mic_delay_ms: 0,
            block_voice: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackStatus {
    pub sound_id: Option<u32>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub paused: bool,
    pub voices: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    NoOutputDevice,
    NoInputDevice,
    UnsupportedConfig,
    Decode,
    Device(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoOutputDevice => write!(f, "no output device"),
            AudioError::NoInputDevice => write!(f, "no input device"),
            AudioError::UnsupportedConfig => write!(f, "unsupported stream configuration"),
            AudioError::Decode => write!(f, "decode failed"),
            AudioError::Device(s) => write!(f, "audio device error: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

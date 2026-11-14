//! RustPad audio: decoding, DSP, mixing and WASAPI streaming.
//!
//! `types` holds the shared data structures, `mixer` the real-time mixer,
//! `engine` the cpal/WASAPI device layer. `decode`, `resample`, `dsp`, `wav` and
//! `record` are pure functions/objects with unit tests.

pub mod decode;
pub mod dsp;
pub mod engine;
pub mod mixer;
pub mod record;
pub mod resample;
pub mod types;
pub mod wav;

pub use decode::{decode_file, is_supported_extension, probe_duration_ms};
pub use engine::EngineHandle;
pub use mixer::Mixer;
pub use types::{
    AudioError, DecodedSound, DeviceInfo, DeviceKind, EngineCommand, EngineEvent, PlayRequest,
    PlaySource, PlaybackStatus, Target,
};

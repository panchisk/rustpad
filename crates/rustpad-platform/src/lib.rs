//! Windows integration: remote-control pipe, global keyboard hook, TTS,
//! autostart, audio ducking and single-instance guard.
//!
//! All modules are `#[cfg(windows)]`-gated implementations with non-Windows
//! stubs returning `Unsupported`, so the workspace still checks on CI hosts.

pub mod autokeys;
pub mod autostart;
pub mod ducking;
pub mod fileid;
pub mod hotkeys;
pub mod pipe;
pub mod single_instance;
pub mod soundpad_import;
pub mod tts;

pub use fileid::soundpad_hash;
pub use hotkeys::{KeyEvent, KeyboardHook};
pub use pipe::{parse_command, PipeServer, RemoteCommand, RemoteReply};

//! RustPad core: model, Soundpad-compatible `.spl` I/O, configuration, hotkeys.
//!
//! This crate is platform independent (it compiles and its tests run on any host).

pub mod config;
pub mod hotkey;
pub mod model;
pub mod spl;

pub use config::{AppConfig, NormalizeMode, Theme, WindowState};
pub use hotkey::{Hotkey, Modifiers};
pub use model::{
    default_category_icon, format_duration, parse_duration, synthetic_hash, today_iso, Category,
    CategoryKind, HotbarItem, HotbarPage, PlayMode, Rgb, Sound, Soundlist, StatEntry, Stats,
    SOUND_EXTENSIONS,
};

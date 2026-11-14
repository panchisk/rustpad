//! Persistent application configuration (`%APPDATA%\RustPad\config.json`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::hotkey::Hotkey;
use crate::model::PlayMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NormalizeMode {
    #[default]
    Off,
    /// Fixed dB offset, applied to every sound.
    Fixed,
    /// Per-sound gain derived from measured loudness.
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

/// What the recorder captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecordSource {
    #[default]
    Microphone,
    /// "What you hear": WASAPI loopback of the default output device.
    Speakers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub selected_category: usize,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            width: 1180.0,
            height: 760.0,
            maximized: false,
            selected_category: 0,
        }
    }
}

/// Hotkey action ids. Persisted as strings so the config stays forward compatible.
pub mod action {
    pub const PLAY_SELECTED: &str = "playSelected";
    pub const PLAY_PREVIOUS: &str = "playPrevious";
    pub const PLAY_NEXT: &str = "playNext";
    pub const PLAY_RANDOM: &str = "playRandom";
    pub const PLAY_RANDOM_ALL: &str = "playRandomAll";
    pub const PLAY_CURRENT_AGAIN: &str = "playCurrentAgain";
    pub const PLAY_PREVIOUSLY_PLAYED: &str = "playPreviouslyPlayed";
    pub const STOP: &str = "stop";
    pub const PAUSE_RESUME: &str = "pauseResume";
    pub const PLAY: &str = "play";
    pub const VOLUME_UP: &str = "volumeUp";
    pub const VOLUME_DOWN: &str = "volumeDown";
    pub const VOLUME_MUTE: &str = "volumeMute";
    pub const MODE_DEFAULT: &str = "modeDefault";
    pub const MODE_SPEAKERS: &str = "modeSpeakers";
    pub const MODE_MICROPHONE: &str = "modeMicrophone";
    pub const MODE_SWITCH: &str = "modeSwitch";
    pub const CATEGORY_PREV: &str = "categoryPrev";
    pub const CATEGORY_NEXT: &str = "categoryNext";
    pub const SOUND_PREV: &str = "soundPrev";
    pub const SOUND_NEXT: &str = "soundNext";
    pub const RECORD_MIC: &str = "recordMic";
    pub const RECORD_SPEAKERS: &str = "recordSpeakers";
    pub const RECORD_STOP: &str = "recordStop";
    pub const HOTKEYS_TOGGLE: &str = "hotkeysToggle";
    pub const AUTO_KEYS_TOGGLE: &str = "autoKeysToggle";
    pub const HOTBAR_PREV_PAGE: &str = "hotbarPrevPage";
    pub const HOTBAR_NEXT_PAGE: &str = "hotbarNextPage";
    pub const SHOW_WINDOW: &str = "showWindow";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub soundlist_path: PathBuf,
    pub stats_path: PathBuf,

    // Audio routing
    pub output_device: Option<String>,
    /// Device that receives microphone-bound audio (virtual cable input).
    pub mic_output_device: Option<String>,
    /// Real microphone captured for passthrough.
    pub capture_device: Option<String>,
    pub default_play_mode: PlayMode,
    pub master_volume: f32,
    pub speakers_volume: f32,
    pub microphone_volume: f32,
    pub mic_passthrough: bool,
    pub mic_passthrough_gain: f32,
    pub mic_passthrough_enabled_on_start: bool,
    /// Reduce the volume of other applications while a sound plays (Windows ducking).
    pub duck_others: bool,
    /// 50, 80 or 100 (= mute others).
    pub duck_percent: u8,

    // Loudness
    pub normalize: NormalizeMode,
    pub target_db: f32,
    pub fixed_adjust_db: f32,

    // Microphone behaviour
    pub delay_mic_ms: u32,
    pub seek_step_secs: u32,
    pub block_voice_while_playing: bool,
    /// Stop whatever is playing when a new sound starts (Soundpad's default).
    pub stop_previous_sound: bool,

    // Hotkeys
    pub hotkeys_enabled: bool,
    pub hotkey_index_modifier: String,
    pub hotkey_numcode_modifier: String,
    pub category_index_routing: bool,
    pub distinguish_left_right_modifiers: bool,
    pub pass_hotkeys: bool,
    pub auto_keys_enabled: bool,
    /// Virtual-key code held while a sound plays.
    pub auto_keys_vk: u32,
    /// Modifier bits (same layout as `hotkey::Modifiers`).
    pub auto_keys_modifiers: u8,
    /// Only simulate keys when the sound was started from a hotkey.
    pub auto_keys_only_hotkey: bool,
    pub hotkeys: HashMap<String, Hotkey>,

    // Recorder / TTS
    pub recording_dir: PathBuf,
    pub recording_max_secs: u32,
    pub record_source: RecordSource,
    pub trim_silence: bool,
    pub auto_normalize_recording: bool,
    pub tts_voice: Option<String>,
    pub tts_rate: i32,
    pub tts_pitch: i32,

    // UI / system
    pub autostart: bool,
    pub minimize_to_tray: bool,
    pub minimize_instead_of_close: bool,
    pub start_minimized: bool,
    pub always_on_top: bool,
    pub theme: Theme,
    pub language: String,
    pub window: WindowState,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            soundlist_path: default_soundlist_path(),
            stats_path: default_stats_path(),
            output_device: None,
            mic_output_device: None,
            capture_device: None,
            default_play_mode: PlayMode::Both,
            master_volume: 0.8,
            speakers_volume: 0.8,
            microphone_volume: 0.8,
            mic_passthrough: true,
            mic_passthrough_gain: 1.0,
            mic_passthrough_enabled_on_start: false,
            duck_others: false,
            duck_percent: 80,
            normalize: NormalizeMode::Dynamic,
            target_db: -14.0,
            fixed_adjust_db: -6.0,
            delay_mic_ms: 0,
            seek_step_secs: 5,
            block_voice_while_playing: false,
            stop_previous_sound: true,
            hotkeys_enabled: true,
            hotkey_index_modifier: "Ctrl".to_string(),
            hotkey_numcode_modifier: "Alt".to_string(),
            category_index_routing: false,
            distinguish_left_right_modifiers: false,
            pass_hotkeys: true,
            auto_keys_enabled: false,
            auto_keys_vk: 0x56, // 'V' — the common push-to-talk key
            auto_keys_modifiers: 0,
            auto_keys_only_hotkey: false,
            hotkeys: HashMap::new(),
            recording_dir: default_recording_dir(),
            recording_max_secs: 300,
            record_source: RecordSource::Microphone,
            trim_silence: true,
            auto_normalize_recording: true,
            tts_voice: None,
            tts_rate: 0,
            tts_pitch: 0,
            autostart: false,
            minimize_to_tray: false,
            minimize_instead_of_close: false,
            start_minimized: false,
            always_on_top: false,
            theme: Theme::System,
            language: "en".to_string(),
            window: WindowState::default(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: AppConfig = serde_json::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(cfg)
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                if path.exists() {
                    tracing::warn!("config load failed: {err:#}");
                }
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        save_json_atomic(path, self)
    }

    pub fn hotkey(&self, action: &str) -> Option<Hotkey> {
        self.hotkeys.get(action).copied().filter(|h| !h.is_empty())
    }

    pub fn set_hotkey(&mut self, action: &str, key: Option<Hotkey>) {
        match key {
            Some(h) if !h.is_empty() => {
                self.hotkeys.insert(action.to_string(), h);
            }
            _ => {
                self.hotkeys.remove(action);
            }
        }
    }
}

pub fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    // Windows: rename over an existing file works with MoveFileEx semantics via std.
    if std::fs::rename(&tmp, path).is_err() {
        std::fs::copy(&tmp, path)?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

pub fn app_data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustPad")
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

fn documents_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Documents")
}

/// Defaults to the list Soundpad itself uses, so an existing list loads unchanged.
///
/// Soundpad keeps its active list under `%APPDATA%\Leppsoft\`; older installs
/// (and the documented default) use `%USERPROFILE%\Documents\soundpad\`.
pub fn default_soundlist_path() -> PathBuf {
    if let Some(roaming) = std::env::var_os("APPDATA") {
        let candidate = PathBuf::from(roaming)
            .join("Leppsoft")
            .join("soundlist.spl");
        if candidate.is_file() {
            return candidate;
        }
    }
    documents_dir().join("soundpad").join("soundlist.spl")
}

pub fn default_stats_path() -> PathBuf {
    default_soundlist_path().with_file_name("stats.spl")
}

pub fn default_recording_dir() -> PathBuf {
    documents_dir().join("soundpad")
}

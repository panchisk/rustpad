//! Application state: wiring between the sound list, the audio engine, global
//! hotkeys, the remote-control pipe and the UI.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui;
use rustpad_audio::dsp;
use rustpad_audio::types::{
    DecodedSound, DeviceInfo, EngineCommand, EngineConfig, EngineEvent, PlayRequest, PlaySource,
    PlaybackStatus, Target,
};
use rustpad_audio::wav;
use rustpad_audio::EngineHandle;
use rustpad_core::config::{config_path, AppConfig, NormalizeMode, Theme};
use rustpad_core::hotkey::{Hotkey, Modifiers};
use rustpad_core::{
    synthetic_hash, today_iso, Category, HotbarItem, HotbarPage, PlayMode, Rgb, Sound, Soundlist,
    Stats,
};
use rustpad_platform::hotkeys::{KeyEvent, KeyboardHook};
use rustpad_platform::pipe::{PipeServer, RemoteCommand, RemoteReply};
use rustpad_platform::single_instance::InstanceGuard;
use rustpad_platform::tts::TtsVoice;

const STATS_SAVE_INTERVAL: Duration = Duration::from_secs(20);

pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

pub struct RemoteRequest {
    pub cmd: RemoteCommand,
    pub reply: Sender<RemoteReply>,
}

pub struct ImportResult {
    pub path: PathBuf,
    pub duration_ms: Option<u64>,
    pub loudness_db: Option<f32>,
    pub hash: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyTarget {
    Action(String),
    /// The simulated key held by Auto Keys.
    AutoKeys,
    Sound(u32),
    Category(usize),
    Hotbar {
        page: usize,
        item: usize,
    },
}

pub struct HotkeyCapture {
    pub target: HotkeyTarget,
    pub label: String,
    /// Text typed by the user for keys the hook cannot capture (mouse buttons).
    pub manual: String,
}

pub struct EditorState {
    pub sound_id: Option<u32>,
    pub title: String,
    pub audio: DecodedSound,
    pub undo: Vec<DecodedSound>,
    pub sel_start: f32,
    pub sel_end: f32,
    pub fade_ms: u32,
    pub speed: f32,
    pub boost_db: f32,
    pub dirty: bool,
    pub status: String,
}

impl EditorState {
    pub fn new(audio: DecodedSound, title: String, sound_id: Option<u32>) -> Self {
        EditorState {
            sound_id,
            title,
            audio,
            undo: Vec::new(),
            sel_start: 0.0,
            sel_end: 1.0,
            fade_ms: 500,
            speed: 1.0,
            boost_db: 0.0,
            dirty: false,
            status: String::new(),
        }
    }

    fn snapshot(&mut self) {
        self.undo.push(self.audio.clone());
        if self.undo.len() > 16 {
            self.undo.remove(0);
        }
        self.dirty = true;
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.audio = prev;
            self.dirty = true;
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.audio.duration_ms()
    }

    fn sel_frames(&self) -> (usize, usize) {
        let frames = self.audio.frames();
        let a = (self.sel_start.clamp(0.0, 1.0) * frames as f32) as usize;
        let b = (self.sel_end.clamp(0.0, 1.0) * frames as f32) as usize;
        (a.min(b), a.max(b))
    }

    pub fn apply_crop(&mut self) {
        let (a, b) = self.sel_frames();
        if b <= a {
            self.status = "Select a range first".into();
            return;
        }
        self.snapshot();
        let ch = self.audio.channels as usize;
        self.audio.samples = self.audio.samples[a * ch..b * ch].to_vec();
        self.sel_start = 0.0;
        self.sel_end = 1.0;
        self.status = "Cropped".into();
    }

    pub fn apply_fade_in(&mut self) {
        self.snapshot();
        dsp::fade_in(
            &mut self.audio.samples,
            self.audio.channels,
            self.audio.sample_rate,
            self.fade_ms,
        );
        self.status = "Fade in".into();
    }

    pub fn apply_fade_out(&mut self) {
        self.snapshot();
        dsp::fade_out(
            &mut self.audio.samples,
            self.audio.channels,
            self.audio.sample_rate,
            self.fade_ms,
        );
        self.status = "Fade out".into();
    }

    pub fn apply_reverse(&mut self) {
        self.snapshot();
        dsp::reverse(&mut self.audio.samples, self.audio.channels);
        self.status = "Reversed".into();
    }

    pub fn apply_normalize(&mut self) {
        self.snapshot();
        dsp::normalize_peak(&mut self.audio.samples, -1.0);
        self.status = "Normalized to -1 dBFS".into();
    }

    pub fn apply_boost(&mut self) {
        self.snapshot();
        dsp::boost(&mut self.audio.samples, self.boost_db);
        self.status = format!("Boost {:+.1} dB", self.boost_db);
    }

    pub fn apply_trim(&mut self) {
        self.snapshot();
        let (a, b) = dsp::trim_silence(
            &mut self.audio.samples,
            self.audio.channels,
            self.audio.sample_rate,
            -50.0,
            80,
        );
        self.status = format!("Trimmed {a} + {b} frames");
    }

    pub fn apply_speed(&mut self) {
        if (self.speed - 1.0).abs() < 0.01 {
            return;
        }
        self.snapshot();
        self.audio.samples = dsp::change_speed(
            &self.audio.samples,
            self.audio.channels,
            self.audio.sample_rate,
            self.speed,
        );
        self.status = format!("Speed x{:.2}", self.speed);
    }

    pub fn apply_karaoke(&mut self) {
        self.snapshot();
        dsp::karaoke(&mut self.audio.samples, self.audio.channels);
        self.status = "Karaoke (center removed)".into();
    }
}

pub struct AppState {
    pub cfg: AppConfig,
    pub cfg_path: PathBuf,
    pub list: Soundlist,
    pub stats: Stats,

    pub engine: EngineHandle,
    pub out_devices: Vec<DeviceInfo>,
    pub in_devices: Vec<DeviceInfo>,

    pub status: PlaybackStatus,
    pub mic_level: f32,
    pub out_level: f32,

    pub selected_category: usize,
    pub selected_sound: Option<u32>,
    pub search: String,
    pub current_sound: Option<u32>,
    pub recent_sounds: Vec<u32>,

    pub play_mode: PlayMode,
    pub hotkeys_enabled: bool,
    pub auto_keys_enabled: bool,

    pub show_settings: bool,
    pub show_tts: bool,
    pub show_recorder: bool,
    pub show_editor: bool,
    pub show_about: bool,
    pub show_hotkeys: bool,
    pub show_mic_setup: bool,
    pub show_category_rename: Option<(usize, String)>,

    pub tts_text: String,
    pub tts_voices: Vec<TtsVoice>,
    pub tts_voice: Option<String>,
    pub tts_rate: i32,
    pub tts_pitch: i32,
    pub tts_play_on: PlayMode,
    pub tts_generating: bool,

    pub recording: bool,
    pub last_recording: Option<PathBuf>,

    pub editor: Option<EditorState>,
    pub hotkey_capture: Option<HotkeyCapture>,

    pub toasts: Vec<Toast>,
    pub dirty: bool,
    pub stats_dirty: bool,
    last_stats_save: Instant,

    remote_rx: Receiver<RemoteRequest>,
    import_tx: Sender<ImportResult>,
    import_rx: Receiver<ImportResult>,
    tts_rx: Receiver<(PathBuf, anyhow::Result<()>)>,
    tts_tx: Sender<(PathBuf, anyhow::Result<()>)>,

    hook: Option<KeyboardHook>,
    pressed: HashSet<u32>,
    mic_hold: Option<u32>,
    duck_prev: Option<u32>,
    /// True while a global hotkey is being handled (used by Auto Keys filtering).
    hotkey_trigger: bool,
    /// Whether Auto Keys currently holds its simulated key down.
    auto_keys_held: bool,

    _pipe: Option<PipeServer>,
    _guard: Option<InstanceGuard>,
    pub quit_requested: bool,
    pub lang_ru: bool,
}

impl AppState {
    pub fn new(cc: &eframe::CreationContext<'_>, guard: Option<InstanceGuard>) -> Self {
        Self::new_with_list(cc, guard, None)
    }

    pub fn new_with_list(
        _cc: &eframe::CreationContext<'_>,
        guard: Option<InstanceGuard>,
        initial_list: Option<std::path::PathBuf>,
    ) -> Self {
        let cfg_path = config_path();
        let mut cfg = AppConfig::load_or_default(&cfg_path);
        if let Some(path) = initial_list {
            cfg.soundlist_path = path;
        }
        let mut list = Soundlist::load(&cfg.soundlist_path).unwrap_or_else(|err| {
            tracing::warn!("cannot load {}: {err:#}", cfg.soundlist_path.display());
            Soundlist::new()
        });
        list.file_path = Some(cfg.soundlist_path.clone());
        let stats = Stats::load(&cfg.stats_path).unwrap_or_default();

        let (out_devices, in_devices) = EngineHandle::devices();
        cfg.output_device = pick_device(&cfg.output_device, &out_devices);
        cfg.capture_device = pick_device(&cfg.capture_device, &in_devices);
        // A virtual-cable device must be chosen explicitly; never default to speakers.
        // When a known cable is installed, select it automatically so microphone
        // passthrough works out of the box (Soundpad uses its APO for this).
        cfg.mic_output_device = pick_mic_output(&cfg.mic_output_device, &out_devices);

        let engine = EngineHandle::spawn(engine_config(&cfg));

        let (remote_tx, remote_rx) = bounded(32);
        let pipe_handler: rustpad_platform::pipe::RemoteHandler = {
            let remote_tx = remote_tx.clone();
            Arc::new(move |cmd| {
                let (tx, rx) = bounded(1);
                if remote_tx.send(RemoteRequest { cmd, reply: tx }).is_err() {
                    return RemoteReply::error("application is shutting down");
                }
                rx.recv_timeout(Duration::from_secs(3))
                    .unwrap_or_else(|_| RemoteReply::error("request timed out"))
            })
        };
        let pipe = match PipeServer::start(pipe_handler) {
            Ok(server) => Some(server),
            Err(err) => {
                tracing::warn!("remote control unavailable: {err:#}");
                None
            }
        };

        let hook = match KeyboardHook::start() {
            Ok(hook) => Some(hook),
            Err(err) => {
                tracing::warn!("global hotkeys unavailable: {err:#}");
                None
            }
        };
        let tts_voices = rustpad_platform::tts::list_voices().unwrap_or_default();
        let tts_voice = cfg
            .tts_voice
            .clone()
            .or_else(|| tts_voices.first().map(|v| v.id.clone()));

        let (import_tx, import_rx) = bounded(64);
        let (tts_tx, tts_rx) = bounded(4);

        let selected_category = cfg
            .window
            .selected_category
            .min(list.categories.len().saturating_sub(1));

        let mut state = AppState {
            lang_ru: cfg.language.eq_ignore_ascii_case("ru"),
            cfg,
            cfg_path,
            list,
            stats,
            engine,
            out_devices,
            in_devices,
            status: PlaybackStatus::default(),
            mic_level: 0.0,
            out_level: 0.0,
            selected_category,
            selected_sound: None,
            search: String::new(),
            current_sound: None,
            recent_sounds: Vec::new(),
            play_mode: PlayMode::Default,
            hotkeys_enabled: true,
            auto_keys_enabled: false,
            show_settings: false,
            show_tts: false,
            show_recorder: false,
            show_editor: false,
            show_about: false,
            show_hotkeys: false,
            show_mic_setup: false,
            show_category_rename: None,
            tts_text: String::new(),
            tts_voices,
            tts_voice,
            tts_rate: 0,
            tts_pitch: 0,
            tts_play_on: PlayMode::Both,
            tts_generating: false,
            recording: false,
            last_recording: None,
            editor: None,
            hotkey_capture: None,
            toasts: Vec::new(),
            dirty: false,
            stats_dirty: false,
            last_stats_save: Instant::now(),
            remote_rx,
            import_tx,
            import_rx,
            tts_rx,
            tts_tx,
            hook,
            pressed: HashSet::new(),
            mic_hold: None,
            duck_prev: None,
            hotkey_trigger: false,
            auto_keys_held: false,
            _pipe: pipe,
            _guard: guard,
            quit_requested: false,
        };
        state.hotkeys_enabled = state.cfg.hotkeys_enabled;
        state.auto_keys_enabled = state.cfg.auto_keys_enabled;
        state
    }

    // ---------------------------------------------------------------- i18n

    pub fn tr(&self, en: &str, ru: &str) -> String {
        if self.lang_ru {
            ru.to_string()
        } else {
            en.to_string()
        }
    }

    // ------------------------------------------------------------- helpers

    pub fn visible_ids(&self) -> Vec<u32> {
        let q = self.search.trim().to_lowercase();
        self.list
            .sounds_for_category(self.selected_category)
            .into_iter()
            .filter(|s| {
                q.is_empty()
                    || s.title.to_lowercase().contains(&q)
                    || s.artist.to_lowercase().contains(&q)
                    || s.file_name().to_lowercase().contains(&q)
                    || s.tag.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .map(|s| s.id)
            .collect()
    }

    /// Sound ids in the selected category in list order (ignores the search filter).
    pub fn category_ids(&self) -> Vec<u32> {
        self.list
            .sounds_for_category(self.selected_category)
            .iter()
            .map(|s| s.id)
            .collect()
    }

    #[allow(dead_code)]
    pub fn selected_sound(&self) -> Option<&Sound> {
        self.selected_sound.and_then(|id| self.list.find(id))
    }

    pub fn sound_gain(&self, s: &Sound) -> f32 {
        match self.cfg.normalize {
            NormalizeMode::Off => 1.0,
            NormalizeMode::Fixed => db_to_gain(self.cfg.fixed_adjust_db),
            NormalizeMode::Dynamic => match s.loudness_db {
                Some(db) if db > -119.0 => db_to_gain(self.cfg.target_db - db).clamp(0.05, 6.0),
                _ => 1.0,
            },
        }
    }

    pub fn resolve_mode(&self, mode: PlayMode) -> PlayMode {
        match mode {
            PlayMode::Default => self.cfg.default_play_mode,
            other => other,
        }
    }

    pub fn targets(&self, mode: PlayMode) -> Vec<Target> {
        let mode = self.resolve_mode(mode);
        let mut v = Vec::new();
        if mode.on_speakers() {
            v.push(Target::Speakers);
        }
        if mode.on_microphone() {
            v.push(Target::Microphone);
        }
        if v.is_empty() {
            v.push(Target::Speakers);
        }
        v
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
            error,
        });
        if self.toasts.len() > 6 {
            self.toasts.remove(0);
        }
    }

    // ------------------------------------------------------------ playback

    pub fn play_sound(&mut self, id: u32, mode: PlayMode) {
        let Some(sound) = self.list.find(id).cloned() else {
            return;
        };
        if !sound.path.is_file() {
            self.toast(format!("File not found: {}", sound.path.display()), true);
            return;
        }
        if self.cfg.stop_previous_sound {
            self.engine.send(EngineCommand::StopAll);
            self.current_sound = None;
        }
        self.release_auto_keys();
        self.press_auto_keys();
        let gain = self.sound_gain(&sound);
        let req = PlayRequest {
            sound_id: id,
            source: PlaySource::File(sound.path.clone()),
            gain,
            targets: self.targets(mode),
            looping: false,
            start_ms: 0,
        };
        self.engine.send(EngineCommand::Play(req));
        self.current_sound = Some(id);
        self.selected_sound = Some(id);
        self.recent_sounds.retain(|x| *x != id);
        self.recent_sounds.push(id);
        if self.recent_sounds.len() > 50 {
            self.recent_sounds.remove(0);
        }
        if self.targets(mode).contains(&Target::Microphone) {
            self.apply_ducking();
        }
    }

    pub fn play_visible_index(&mut self, index1: usize, mode: PlayMode) {
        let ids = self.category_ids();
        if let Some(id) = ids.get(index1.saturating_sub(1)).copied() {
            self.play_sound(id, mode);
        }
    }

    pub fn stop(&mut self) {
        self.engine.send(EngineCommand::StopAll);
        self.release_auto_keys();
        self.restore_ducking();
    }

    pub fn set_auto_keys_enabled(&mut self, enabled: bool) {
        self.auto_keys_enabled = enabled;
        self.cfg.auto_keys_enabled = enabled;
        if !enabled {
            self.release_auto_keys();
        }
        self.cfg_dirty();
    }

    fn auto_keys_active(&self) -> bool {
        self.cfg.auto_keys_enabled
            && (!self.cfg.auto_keys_only_hotkey || self.hotkey_trigger)
            && self.cfg.auto_keys_vk != 0
    }

    fn press_auto_keys(&mut self) {
        if self.auto_keys_held || !self.auto_keys_active() {
            return;
        }
        let vk = self.cfg.auto_keys_vk.min(u16::MAX as u32) as u16;
        let mods = Modifiers(self.cfg.auto_keys_modifiers);
        match rustpad_platform::autokeys::press(vk, mods) {
            Ok(()) => self.auto_keys_held = true,
            Err(err) => self.toast(format!("Auto Keys: {err:#}"), true),
        }
    }

    fn release_auto_keys(&mut self) {
        if !self.auto_keys_held {
            return;
        }
        self.auto_keys_held = false;
        let vk = self.cfg.auto_keys_vk.min(u16::MAX as u32) as u16;
        let mods = Modifiers(self.cfg.auto_keys_modifiers);
        if let Err(err) = rustpad_platform::autokeys::release(vk, mods) {
            tracing::warn!("Auto Keys release failed: {err:#}");
        }
    }

    pub fn toggle_pause(&mut self) {
        self.engine.send(EngineCommand::TogglePause);
    }

    pub fn seek(&mut self, ms: u64) {
        if let Some(id) = self.current_sound.or(self.selected_sound) {
            self.engine.send(EngineCommand::Seek(id, ms));
        }
    }

    pub fn play_next(&mut self) {
        let ids = self.category_ids();
        if ids.is_empty() {
            return;
        }
        let cur = self
            .current_sound
            .and_then(|id| ids.iter().position(|x| *x == id))
            .unwrap_or(0);
        let next = ids[(cur + 1) % ids.len()];
        self.play_sound(next, self.play_mode);
    }

    pub fn play_previous(&mut self) {
        let ids = self.category_ids();
        if ids.is_empty() {
            return;
        }
        let cur = self
            .current_sound
            .and_then(|id| ids.iter().position(|x| *x == id))
            .unwrap_or(0);
        let prev = ids[(cur + ids.len() - 1) % ids.len()];
        self.play_sound(prev, self.play_mode);
    }

    pub fn play_random(&mut self, category: Option<usize>) {
        self.play_random_with(category, self.play_mode);
    }

    pub fn play_random_with(&mut self, category: Option<usize>, mode: PlayMode) {
        let ids: Vec<u32> = match category {
            Some(cat) => self
                .list
                .sounds_for_category(cat)
                .iter()
                .map(|s| s.id)
                .collect(),
            None => self.category_ids(),
        };
        if ids.is_empty() {
            return;
        }
        let idx = pseudo_random(ids.len());
        self.play_sound(ids[idx], mode);
    }

    pub fn play_current_again(&mut self) {
        if let Some(id) = self.current_sound.or(self.selected_sound) {
            self.play_sound(id, self.play_mode);
        }
    }

    pub fn play_previously_played(&mut self) {
        if self.recent_sounds.len() >= 2 {
            let id = self.recent_sounds[self.recent_sounds.len() - 2];
            self.play_sound(id, self.play_mode);
        }
    }

    pub fn set_play_mode(&mut self, mode: PlayMode) {
        self.play_mode = mode;
        self.toast(format!("Play mode: {}", mode.label()), false);
    }

    pub fn cycle_play_mode(&mut self) {
        self.play_mode = self.play_mode.next();
        self.toast(format!("Play mode: {}", self.play_mode.label()), false);
    }

    pub fn change_volume(&mut self, delta: f32) {
        self.cfg.master_volume = (self.cfg.master_volume + delta).clamp(0.0, 1.5);
        self.engine
            .send(EngineCommand::SetMasterVolume(self.cfg.master_volume));
        self.cfg_dirty();
    }

    pub fn apply_ducking(&mut self) {
        if !self.cfg.duck_others || self.duck_prev.is_some() {
            return;
        }
        let duck = rustpad_platform::ducking::Ducking::from_percent(self.cfg.duck_percent);
        match rustpad_platform::ducking::apply(duck) {
            Ok(prev) => self.duck_prev = Some(prev),
            Err(err) => tracing::warn!("ducking: {err:#}"),
        }
    }

    pub fn restore_ducking(&mut self) {
        if let Some(prev) = self.duck_prev.take() {
            if let Err(err) = rustpad_platform::ducking::restore(prev) {
                tracing::warn!("ducking restore: {err:#}");
            }
        }
    }

    // -------------------------------------------------------------- library

    pub fn import_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&path) {
                    let nested: Vec<PathBuf> = entries
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.is_file())
                        .collect();
                    self.import_files(nested);
                }
                continue;
            }
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !rustpad_audio::decode::is_supported_extension(&ext) {
                self.toast(format!("Unsupported format: {}", path.display()), true);
                continue;
            }
            let tx = self.import_tx.clone();
            std::thread::spawn(move || {
                let hash = hash_for_path(&path);
                match rustpad_audio::decode_file(&path) {
                    Ok(audio) => {
                        let loudness = dsp::rms_db(&audio.samples);
                        let _ = tx.send(ImportResult {
                            duration_ms: Some(audio.duration_ms()),
                            loudness_db: Some(loudness),
                            hash,
                            path,
                            error: None,
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(ImportResult {
                            path,
                            duration_ms: None,
                            loudness_db: None,
                            hash,
                            error: Some(format!("{err:#}")),
                        });
                    }
                }
            });
        }
    }

    pub fn add_sound_now(
        &mut self,
        path: PathBuf,
        duration_ms: u64,
        loudness: Option<f32>,
        hash: String,
    ) -> u32 {
        let id = self.list.add_path(path.clone(), duration_ms);
        if let Some(s) = self.list.find_mut(id) {
            s.hash = hash;
            s.loudness_db = loudness;
        }
        self.dirty = true;
        id
    }

    pub fn remove_selected(&mut self) {
        if let Some(id) = self.selected_sound {
            if let Some(sound) = self.list.remove(id) {
                // Ids are compacted by `Soundlist::remove`; drop selections that
                // could now point at a different sound.
                self.selected_sound = None;
                if self.editor.as_ref().and_then(|e| e.sound_id) == Some(id) {
                    if let Some(editor) = &mut self.editor {
                        editor.sound_id = None;
                    }
                }
                self.toast(format!("Removed {}", sound.title), false);
                self.dirty = true;
            }
        }
    }

    pub fn remove_dead_entries(&mut self) {
        self.selected_sound = None;
        let dead = self.list.remove_dead_entries();
        if !dead.is_empty() {
            self.dirty = true;
            self.toast(format!("Removed {} missing files", dead.len()), false);
        } else {
            self.toast("No missing files", false);
        }
    }

    /// Loads another `.spl` list, saving the current one first when it has
    /// unsaved changes. Statistics are loaded from a sibling `stats.spl`.
    pub fn open_soundlist(&mut self, path: std::path::PathBuf) {
        if self.dirty {
            self.save_soundlist();
        }
        match Soundlist::load(&path) {
            Ok(mut list) => {
                list.file_path = Some(path.clone());
                self.list = list;
                self.stats = Stats::load(&path.with_file_name("stats.spl")).unwrap_or_default();
                self.selected_category = 0;
                self.selected_sound = None;
                self.search.clear();
                self.cfg.soundlist_path = path.clone();
                self.cfg.stats_path = path.with_file_name("stats.spl");
                self.dirty = false;
                self.cfg_dirty();
                self.toast(
                    format!(
                        "{}: {} sounds",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "soundlist".into()),
                        self.list.sounds.len()
                    ),
                    false,
                );
            }
            Err(err) => self.toast(format!("Cannot open {}: {err:#}", path.display()), true),
        }
    }

    pub fn new_category(&mut self, name: String) {
        let mut cat = Category::new(name);
        cat.color = None;
        self.list.categories.push(cat);
        self.selected_category = self.list.categories.len() - 1;
        self.dirty = true;
    }

    pub fn delete_category(&mut self, index: usize) {
        if self.list.categories.get(index).is_some_and(|c| c.is_all()) {
            self.toast(
                self.tr(
                    "The \"All sounds\" view cannot be removed",
                    "Представление «Все звуки» удалить нельзя",
                ),
                true,
            );
            return;
        }
        if index < self.list.categories.len() && self.list.categories.len() > 1 {
            self.list.categories.remove(index);
            self.selected_category = self
                .selected_category
                .min(self.list.categories.len().saturating_sub(1));
            self.dirty = true;
        }
    }

    pub fn rename_category(&mut self, index: usize, name: String) {
        if let Some(cat) = self.list.categories.get_mut(index) {
            if cat.is_all() {
                return;
            }
            cat.name = name;
            self.dirty = true;
        }
    }

    pub fn add_sound_to_category(&mut self, sound_id: u32, category: usize) {
        let category = match self.list.categories.get(category) {
            Some(cat) if cat.is_all() => {
                match self
                    .list
                    .categories
                    .iter()
                    .position(|c| c.kind == rustpad_core::model::CategoryKind::Normal)
                {
                    Some(index) => index,
                    None => return,
                }
            }
            Some(_) => category,
            None => return,
        };
        if let Some(cat) = self.list.categories.get_mut(category) {
            if !cat.sound_ids.contains(&sound_id) {
                cat.sound_ids.push(sound_id);
                self.dirty = true;
            }
        }
    }

    pub fn remove_sound_from_category(&mut self, sound_id: u32, category: usize) {
        if self
            .list
            .categories
            .get(category)
            .is_some_and(|c| c.is_all())
        {
            return;
        }
        if let Some(cat) = self.list.categories.get_mut(category) {
            cat.sound_ids.retain(|id| *id != sound_id);
            self.dirty = true;
        }
    }

    pub fn new_hotbar_page(&mut self) {
        self.list.hotbar.push(HotbarPage::empty());
        self.dirty = true;
    }

    pub fn remove_hotbar_page(&mut self, index: usize) {
        if self.list.hotbar.len() > 1 && index < self.list.hotbar.len() {
            self.list.hotbar.remove(index);
            self.dirty = true;
        }
    }

    pub fn add_to_hotbar(&mut self, page: usize, sound_id: u32) {
        if let Some(p) = self.list.hotbar.get_mut(page) {
            if !p.items.iter().any(|i| i.sound_id == sound_id) {
                p.items.push(HotbarItem::new(sound_id));
                self.dirty = true;
            }
        }
    }

    // --------------------------------------------------------------- saving

    pub fn cfg_dirty(&mut self) {
        self.cfg.window.selected_category = self.selected_category;
    }

    pub fn save_config(&mut self) {
        self.cfg.window.selected_category = self.selected_category;
        self.cfg.hotkeys_enabled = self.hotkeys_enabled;
        self.cfg.auto_keys_enabled = self.auto_keys_enabled;
        self.cfg.language = if self.lang_ru {
            "ru".into()
        } else {
            "en".into()
        };
        self.cfg.mic_passthrough_enabled_on_start = self.cfg.mic_passthrough;
        if let Err(err) = self.cfg.save(&self.cfg_path) {
            self.toast(format!("Could not save config: {err:#}"), true);
        }
    }

    pub fn save_soundlist(&mut self) {
        let path = self
            .list
            .file_path
            .clone()
            .unwrap_or_else(|| self.cfg.soundlist_path.clone());
        if path.exists() {
            let backup = path.with_file_name(format!("backup-soundlist-{}.spl", today_iso()));
            if !backup.exists() {
                let _ = std::fs::copy(&path, &backup);
            }
        }
        match self.list.save(&path) {
            Ok(()) => {
                self.dirty = false;
                self.toast("Sound list saved", false);
            }
            Err(err) => self.toast(format!("Could not save sound list: {err:#}"), true),
        }
        if let Err(err) = self.stats.save(&self.cfg.stats_path) {
            tracing::warn!("stats save: {err:#}");
        }
    }

    pub fn save_stats_if_due(&mut self, force: bool) {
        if !self.stats_dirty {
            return;
        }
        if !force && self.last_stats_save.elapsed() < STATS_SAVE_INTERVAL {
            return;
        }
        if let Err(err) = self.stats.save(&self.cfg.stats_path) {
            tracing::warn!("stats save: {err:#}");
        } else {
            self.stats_dirty = false;
            self.last_stats_save = Instant::now();
        }
    }

    // ------------------------------------------------------------- recorder

    pub fn start_recording(&mut self) {
        if self.recording {
            return;
        }
        let dir = self.cfg.recording_dir.clone();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            self.toast(format!("Cannot create {}: {err}", dir.display()), true);
            return;
        }
        let stamp = timestamp();
        let path = dir.join(format!("recording-{stamp}.wav"));
        self.recording = true;
        match self.cfg.record_source {
            rustpad_core::RecordSource::Microphone => {
                self.engine.send(EngineCommand::StartRecording {
                    path,
                    max_secs: self.cfg.recording_max_secs,
                });
            }
            rustpad_core::RecordSource::Speakers => {
                self.engine.send(EngineCommand::StartLoopbackRecording {
                    path,
                    max_secs: self.cfg.recording_max_secs,
                });
            }
        }
    }

    pub fn stop_recording(&mut self) {
        if self.recording {
            self.engine.send(EngineCommand::StopRecording);
        }
    }

    pub fn toggle_recording(&mut self) {
        if self.recording {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    // ------------------------------------------------------------------ TTS

    pub fn speak_tts(&mut self, mode: PlayMode) {
        let text = self.tts_text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.tts_generating {
            return;
        }
        self.tts_generating = true;
        let voice = self.tts_voice.clone();
        let rate = self.tts_rate;
        let pitch = self.tts_pitch;
        let path = std::env::temp_dir().join(format!("rustpad-tts-{}.wav", timestamp()));
        let tx = self.tts_tx.clone();
        let path2 = path.clone();
        std::thread::spawn(move || {
            let result = rustpad_platform::tts::synthesize_to_wav(
                &text,
                voice.as_deref(),
                rate,
                pitch,
                &path2,
            );
            let _ = tx.send((path2, result));
        });
        self.tts_play_on = mode;
    }

    // --------------------------------------------------------------- editor

    pub fn open_editor(&mut self, sound_id: Option<u32>) {
        match sound_id {
            Some(id) => {
                if let Some(sound) = self.list.find(id).cloned() {
                    match rustpad_audio::decode_file(&sound.path) {
                        Ok(audio) => {
                            self.editor =
                                Some(EditorState::new(audio, sound.title.clone(), Some(id)));
                            self.show_editor = true;
                        }
                        Err(err) => self.toast(format!("Cannot open: {err:#}"), true),
                    }
                }
            }
            None => {
                let path = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "m4a", "aac"])
                    .pick_file();
                if let Some(path) = path {
                    if let Ok(audio) = rustpad_audio::decode_file(&path) {
                        let title = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.editor = Some(EditorState::new(audio, title, None));
                        self.show_editor = true;
                    }
                }
            }
        }
    }

    pub fn editor_save_as(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let path = rfd::FileDialog::new()
            .set_file_name(format!("{}.wav", ed.title))
            .add_filter("WAV", &["wav"])
            .save_file();
        let Some(path) = path else { return };
        match wav::write_wav(
            &path,
            ed.audio.sample_rate,
            ed.audio.channels,
            &ed.audio.samples,
        ) {
            Ok(()) => {
                ed.status = format!("Saved {}", path.display());
                ed.dirty = false;
            }
            Err(err) => ed.status = format!("Save failed: {err:#}"),
        }
    }

    pub fn editor_export_to_list(&mut self) {
        let Some(ed) = &mut self.editor else { return };
        let dir = self.cfg.recording_dir.clone();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.wav", sanitize(&ed.title)));
        if let Err(err) = wav::write_wav(
            &path,
            ed.audio.sample_rate,
            ed.audio.channels,
            &ed.audio.samples,
        ) {
            ed.status = format!("Save failed: {err:#}");
            return;
        }
        let loudness = dsp::rms_db(&ed.audio.samples);
        let hash = hash_for_path(&path);
        let duration = ed.audio.duration_ms();
        let title = ed.title.clone();
        self.add_sound_now(path, duration, Some(loudness), hash);
        self.dirty = true;
        if let Some(ed) = &mut self.editor {
            ed.status = format!("Added “{title}” to the list");
            ed.dirty = false;
        }
    }

    // ---------------------------------------------------- Soundpad migration

    /// Imports the parts of Soundpad's registry configuration that have a clear
    /// RustPad equivalent. Returns the number of applied settings.
    pub fn import_soundpad_settings(&mut self) -> usize {
        let Some(map) = rustpad_platform::soundpad_import::read_soundpad_settings() else {
            self.toast(
                "No Soundpad configuration found in the Soundpad registry key",
                true,
            );
            return 0;
        };
        let get = |key: &str| map.get(key).map(|s| s.trim().to_string());
        let get_bool = |key: &str| get(key).map(|v| v == "1").unwrap_or(false);
        let get_i32 = |key: &str| get(key).and_then(|v| v.parse::<i32>().ok());
        let mut applied = 0;

        if let Some(v) = get_i32("MainFrame/playVolume") {
            self.cfg.master_volume = (v as f32 / 100.0).clamp(0.0, 1.0);
            applied += 1;
        }
        if let Some(v) = get_i32("MainFrame/delayPlaybackInMs") {
            self.cfg.delay_mic_ms = v.max(0) as u32;
            applied += 1;
        }
        if map.contains_key("MainFrame/filterBlockVoice") {
            self.cfg.block_voice_while_playing = get_bool("MainFrame/filterBlockVoice");
            applied += 1;
        }
        if map.contains_key("MainFrame/reduceOtherVolume") {
            self.cfg.duck_others = get_bool("MainFrame/reduceOtherVolume");
            applied += 1;
        }
        if let Some(v) = get_i32("MainFrame/normalizeTargetDecibel") {
            self.cfg.target_db = v as f32;
            applied += 1;
        }
        if let Some(v) = get_i32("MainFrame/fixedVolumeAdjustmentDecibel") {
            self.cfg.fixed_adjust_db = v as f32;
            applied += 1;
        }
        if map.contains_key("MainFrame/normalizeVolume") {
            self.cfg.normalize = if get_bool("MainFrame/normalizeVolume") {
                NormalizeMode::Dynamic
            } else {
                NormalizeMode::Off
            };
            applied += 1;
        }
        if let Some(v) = get("MainFrame/recordingSaveLocation") {
            if !v.is_empty() {
                self.cfg.recording_dir = PathBuf::from(v);
                applied += 1;
            }
        }
        if let Some(v) = get_i32("MainFrame/recordingLimitInSeconds") {
            self.cfg.recording_max_secs = v.max(0) as u32;
            applied += 1;
        }
        if map.contains_key("MainFrame/trimRecordingSilence") {
            self.cfg.trim_silence = get_bool("MainFrame/trimRecordingSilence");
            applied += 1;
        }
        if map.contains_key("MainFrame/autoNormalizeRecordings") {
            self.cfg.auto_normalize_recording = get_bool("MainFrame/autoNormalizeRecordings");
            applied += 1;
        }
        if map.contains_key("MainFrame/minimizeInsteadOfClose") {
            self.cfg.minimize_instead_of_close = get_bool("MainFrame/minimizeInsteadOfClose");
            self.cfg.minimize_to_tray = get_bool("MainFrame/minimizeToTray");
            applied += 2;
        }
        if map.contains_key("MainFrame/startMinimized") {
            self.cfg.start_minimized = get_bool("MainFrame/startMinimized");
            applied += 1;
        }
        if map.contains_key("MainFrame/showAlwaysOnTop") {
            self.cfg.always_on_top = get_bool("MainFrame/showAlwaysOnTop");
            applied += 1;
        }
        if let Some(v) = get("MainFrame/language") {
            if v.to_ascii_lowercase().starts_with("ru") {
                self.lang_ru = true;
                applied += 1;
            }
        }

        // Special hotkeys: `xxxKeyModifiers` + `xxxKey` (VK code).
        let hotkey_actions: &[(&str, &str)] = &[
            ("stop", rustpad_core::config::action::STOP),
            ("play", rustpad_core::config::action::PLAY_SELECTED),
            ("pauseResume", rustpad_core::config::action::PAUSE_RESUME),
            ("playPrevious", rustpad_core::config::action::PLAY_PREVIOUS),
            ("playNext", rustpad_core::config::action::PLAY_NEXT),
            ("playSelected", rustpad_core::config::action::PLAY_SELECTED),
            (
                "playCurrent",
                rustpad_core::config::action::PLAY_CURRENT_AGAIN,
            ),
            (
                "playPreviouslyPlayed",
                rustpad_core::config::action::PLAY_PREVIOUSLY_PLAYED,
            ),
            ("randomPlayback", rustpad_core::config::action::PLAY_RANDOM),
            (
                "randomPlaybackAll",
                rustpad_core::config::action::PLAY_RANDOM_ALL,
            ),
            ("selectPrevious", rustpad_core::config::action::SOUND_PREV),
            ("selectNext", rustpad_core::config::action::SOUND_NEXT),
            (
                "selectPreviousCategory",
                rustpad_core::config::action::CATEGORY_PREV,
            ),
            (
                "selectNextCategory",
                rustpad_core::config::action::CATEGORY_NEXT,
            ),
            ("recordBegin", rustpad_core::config::action::RECORD_MIC),
            ("recordEnd", rustpad_core::config::action::RECORD_STOP),
            (
                "volumeToggleMute",
                rustpad_core::config::action::VOLUME_MUTE,
            ),
            (
                "playModeDefault",
                rustpad_core::config::action::MODE_DEFAULT,
            ),
            (
                "playModeSpeakers",
                rustpad_core::config::action::MODE_SPEAKERS,
            ),
            (
                "playModeMicrophone",
                rustpad_core::config::action::MODE_MICROPHONE,
            ),
            ("playModeSwitch", rustpad_core::config::action::MODE_SWITCH),
            (
                "hotkeysToggle",
                rustpad_core::config::action::HOTKEYS_TOGGLE,
            ),
            (
                "autoKeysToggle",
                rustpad_core::config::action::AUTO_KEYS_TOGGLE,
            ),
        ];
        for (prefix, action) in hotkey_actions {
            let key = get_i32(&format!("MainFrame/{prefix}Key")).unwrap_or(0) as u32;
            let mods = get_i32(&format!("MainFrame/{prefix}KeyModifiers")).unwrap_or(0) as u32;
            if let Some(hk) = rustpad_core::hotkey::from_soundpad(mods, key) {
                self.cfg.hotkeys.insert(action.to_string(), hk);
                applied += 1;
            }
        }

        self.save_config();
        self.toast(format!("Imported {applied} Soundpad settings"), false);
        applied
    }

    // -------------------------------------------------------------- hotkeys

    pub fn begin_hotkey_capture(&mut self, target: HotkeyTarget, label: impl Into<String>) {
        self.hotkey_capture = Some(HotkeyCapture {
            target,
            label: label.into(),
            manual: String::new(),
        });
    }

    pub fn commit_hotkey_text(&mut self) {
        let Some(cap) = self.hotkey_capture.take() else {
            return;
        };
        let Some(hk) = Hotkey::parse(&cap.manual) else {
            self.toast("Could not parse hotkey", true);
            return;
        };
        self.assign_hotkey(cap.target, hk);
    }

    pub fn assign_hotkey(&mut self, target: HotkeyTarget, hk: Hotkey) {
        match target {
            HotkeyTarget::Action(action) => {
                self.cfg.hotkeys.insert(action.clone(), hk);
                self.toast(format!("{action} = {hk}"), false);
            }
            HotkeyTarget::AutoKeys => {
                let Some(vk) = hk.key else {
                    self.toast(
                        self.tr(
                            "Auto Keys needs a non-modifier key",
                            "Для автоклавиши нужна обычная клавиша",
                        ),
                        true,
                    );
                    return;
                };
                self.cfg.auto_keys_vk = vk as u32;
                self.cfg.auto_keys_modifiers = hk.modifiers.0;
                self.toast(format!("Auto Keys = {hk}"), false);
                self.cfg_dirty();
                return;
            }
            HotkeyTarget::Sound(id) => {
                if let Some(s) = self.list.find_mut(id) {
                    s.hotkey = Some(hk);
                    self.dirty = true;
                }
            }
            HotkeyTarget::Category(i) => {
                if let Some(c) = self.list.categories.get_mut(i) {
                    c.hotkey = Some(hk);
                    self.dirty = true;
                }
            }
            HotkeyTarget::Hotbar { page, item } => {
                if let Some(p) = self.list.hotbar.get_mut(page) {
                    if let Some(it) = p.items.get_mut(item) {
                        it.hotkey = Some(hk);
                        self.dirty = true;
                    }
                }
            }
        }
        self.cfg_dirty();
    }

    pub fn clear_hotkey(&mut self, target: &HotkeyTarget) {
        match target {
            HotkeyTarget::Action(action) => {
                self.cfg.hotkeys.remove(action);
            }
            HotkeyTarget::AutoKeys => {
                self.cfg.auto_keys_vk = 0;
                self.cfg.auto_keys_modifiers = 0;
                self.cfg_dirty();
                return;
            }
            HotkeyTarget::Sound(id) => {
                if let Some(s) = self.list.find_mut(*id) {
                    s.hotkey = None;
                    self.dirty = true;
                }
            }
            HotkeyTarget::Category(i) => {
                if let Some(c) = self.list.categories.get_mut(*i) {
                    c.hotkey = None;
                    self.dirty = true;
                }
            }
            HotkeyTarget::Hotbar { page, item } => {
                if let Some(p) = self.list.hotbar.get_mut(*page) {
                    if let Some(it) = p.items.get_mut(*item) {
                        it.hotkey = None;
                        self.dirty = true;
                    }
                }
            }
        }
    }

    fn hotkey_targets(&self) -> Vec<(Hotkey, HotkeyTarget, String)> {
        let mut out = Vec::new();
        for (action, hk) in &self.cfg.hotkeys {
            out.push((
                *hk,
                HotkeyTarget::Action(action.clone()),
                action_label(action).to_string(),
            ));
        }
        for s in &self.list.sounds {
            if let Some(hk) = s.hotkey {
                out.push((hk, HotkeyTarget::Sound(s.id), s.title.clone()));
            }
        }
        for (i, c) in self.list.categories.iter().enumerate() {
            if let Some(hk) = c.hotkey {
                out.push((hk, HotkeyTarget::Category(i), c.name.clone()));
            }
        }
        for (pi, p) in self.list.hotbar.iter().enumerate() {
            for (ii, item) in p.items.iter().enumerate() {
                if let Some(hk) = item.hotkey {
                    let name = self
                        .list
                        .find(item.sound_id)
                        .map(|s| s.title.clone())
                        .unwrap_or_default();
                    out.push((hk, HotkeyTarget::Hotbar { page: pi, item: ii }, name));
                }
            }
        }
        out
    }

    fn handle_key(&mut self, ev: KeyEvent) {
        if self.hotkey_capture.is_some() {
            // Ignore bare modifier presses so Ctrl+F1 is captured as a combo, not as Ctrl.
            if ev.down && modifier_of(ev.vk).is_none() {
                if let Some(cap) = self.hotkey_capture.take() {
                    self.assign_hotkey(cap.target, Hotkey::new(ev.modifiers, ev.vk));
                }
            }
            return;
        }

        if !self.hotkeys_enabled {
            if !ev.down {
                self.pressed.remove(&ev.vk);
            }
            return;
        }

        if !ev.injected {
            self.update_modifier_hold(ev);
        }

        if !ev.down {
            self.pressed.remove(&ev.vk);
            return;
        }
        if !self.pressed.insert(ev.vk) {
            return; // auto-repeat
        }

        // Exact hotkey match (sound / category / hotbar / special action).
        for (hk, target, label) in self.hotkey_targets() {
            if hk.matches(ev.modifiers, Some(ev.vk)) {
                match &target {
                    HotkeyTarget::Action(action) => self.run_action(action),
                    HotkeyTarget::Sound(id) => self.play_sound(*id, self.play_mode),
                    HotkeyTarget::Category(cat) => self.play_random(Some(*cat)),
                    HotkeyTarget::Hotbar { page, item } => {
                        if let Some(it) =
                            self.list.hotbar.get(*page).and_then(|p| p.items.get(*item))
                        {
                            self.play_sound(it.sound_id, self.play_mode);
                        }
                    }
                    // Auto Keys is a simulated output key, not a trigger.
                    HotkeyTarget::AutoKeys => {}
                }
                let _ = label;
                return;
            }
        }

        // Index hotkeys: modifier + 1..9, 0 (=10th).
        let index_mod = Hotkey::parse(&self.cfg.hotkey_index_modifier)
            .map(|h| h.modifiers)
            .unwrap_or(Modifiers::CTRL);
        if ev.modifiers == index_mod {
            if let Some(idx) = digit_index(ev.vk) {
                self.play_visible_index(idx, self.play_mode);
                return;
            }
        }
        let num_mod = Hotkey::parse(&self.cfg.hotkey_numcode_modifier)
            .map(|h| h.modifiers)
            .unwrap_or(Modifiers::ALT);
        if ev.modifiers == num_mod {
            if let Some(idx) = numpad_index(ev.vk) {
                self.play_visible_index(idx, self.play_mode);
            }
        }
    }

    /// Modifier-only hotkeys: hold to play, release to stop (push-to-talk style).
    fn update_modifier_hold(&mut self, ev: KeyEvent) {
        let Some(modifier) = modifier_of(ev.vk) else {
            return;
        };
        if ev.down {
            if self.mic_hold.is_some() {
                return;
            }
            for (hk, target, _) in self.hotkey_targets() {
                if hk.key.is_none() && hk.modifiers.contains(modifier) {
                    if let HotkeyTarget::Sound(id) = target {
                        self.play_sound(id, self.play_mode);
                        self.mic_hold = Some(id);
                    }
                    return;
                }
            }
        } else if let Some(id) = self.mic_hold.take() {
            self.engine.send(EngineCommand::StopSound(id));
        }
    }

    pub fn run_action(&mut self, action: &str) {
        use rustpad_core::config::action as a;
        match action {
            a::PLAY_SELECTED => {
                if let Some(id) = self
                    .selected_sound
                    .or_else(|| self.visible_ids().first().copied())
                {
                    self.play_sound(id, self.play_mode);
                }
            }
            a::PLAY_PREVIOUS => self.play_previous(),
            a::PLAY_NEXT => self.play_next(),
            a::PLAY_RANDOM => self.play_random(None),
            a::PLAY_RANDOM_ALL => self.play_random(None),
            a::PLAY_CURRENT_AGAIN => self.play_current_again(),
            a::PLAY_PREVIOUSLY_PLAYED => self.play_previously_played(),
            a::STOP => self.stop(),
            a::PAUSE_RESUME => self.toggle_pause(),
            a::PLAY => self.play_current_again(),
            a::VOLUME_UP => self.change_volume(0.05),
            a::VOLUME_DOWN => self.change_volume(-0.05),
            a::VOLUME_MUTE => {
                self.cfg.master_volume = if self.cfg.master_volume > 0.0 {
                    0.0
                } else {
                    0.8
                };
                let v = self.cfg.master_volume;
                self.engine.send(EngineCommand::SetMasterVolume(v));
            }
            a::MODE_DEFAULT => self.set_play_mode(PlayMode::Default),
            a::MODE_SPEAKERS => self.set_play_mode(PlayMode::Speakers),
            a::MODE_MICROPHONE => self.set_play_mode(PlayMode::Microphone),
            a::MODE_SWITCH => self.cycle_play_mode(),
            a::CATEGORY_PREV => {
                if !self.list.categories.is_empty() {
                    self.selected_category = (self.selected_category + self.list.categories.len()
                        - 1)
                        % self.list.categories.len();
                }
            }
            a::CATEGORY_NEXT => {
                if !self.list.categories.is_empty() {
                    self.selected_category =
                        (self.selected_category + 1) % self.list.categories.len();
                }
            }
            a::SOUND_PREV => self.play_previous(),
            a::SOUND_NEXT => self.play_next(),
            a::RECORD_MIC => self.toggle_recording(),
            a::RECORD_SPEAKERS => self.toggle_recording(),
            a::RECORD_STOP => self.stop_recording(),
            a::HOTKEYS_TOGGLE => {
                self.hotkeys_enabled = !self.hotkeys_enabled;
                let on = self.hotkeys_enabled;
                self.toast(
                    if on {
                        "Hotkeys enabled"
                    } else {
                        "Hotkeys disabled"
                    },
                    false,
                );
            }
            a::AUTO_KEYS_TOGGLE => {
                self.auto_keys_enabled = !self.auto_keys_enabled;
                if !self.auto_keys_enabled {
                    self.release_auto_keys();
                }
                let on = self.auto_keys_enabled;
                self.toast(
                    if on {
                        "Auto Keys enabled"
                    } else {
                        "Auto Keys disabled"
                    },
                    false,
                );
            }
            a::HOTBAR_PREV_PAGE | a::HOTBAR_NEXT_PAGE => {}
            a::SHOW_WINDOW => {}
            _ => {}
        }
    }

    // -------------------------------------------------------- remote control

    fn handle_remote(&mut self, cmd: RemoteCommand) -> RemoteReply {
        match cmd {
            RemoteCommand::PlaySound {
                index,
                speakers,
                mic,
            } => {
                let mode = mode_from_flags(speakers, mic);
                self.play_visible_index(index as usize, mode);
                RemoteReply::ok()
            }
            RemoteCommand::PlaySoundFromCategory {
                category,
                index,
                speakers,
                mic,
            } => {
                let cat = if category < 0 {
                    self.selected_category
                } else {
                    (category as usize).saturating_sub(1)
                };
                let mode = mode_from_flags(speakers, mic);
                if let Some(s) = self
                    .list
                    .sounds_for_category(cat)
                    .get(index.saturating_sub(1) as usize)
                {
                    let id = s.id;
                    self.play_sound(id, mode);
                    RemoteReply::ok()
                } else {
                    RemoteReply::error("sound not found")
                }
            }
            RemoteCommand::PlayRandomSound { speakers, mic } => {
                self.play_random_with(None, mode_from_flags(speakers, mic));
                RemoteReply::ok()
            }
            RemoteCommand::PlayRandomSoundFromCategory {
                category,
                speakers,
                mic,
            } => {
                let cat = if category < 0 {
                    self.selected_category
                } else {
                    (category as usize).saturating_sub(1)
                };
                self.play_random_with(Some(cat), mode_from_flags(speakers, mic));
                RemoteReply::ok()
            }
            RemoteCommand::PlayPreviousSound => {
                self.play_previous();
                RemoteReply::ok()
            }
            RemoteCommand::PlayNextSound => {
                self.play_next();
                RemoteReply::ok()
            }
            RemoteCommand::PlaySelectedSound => {
                if let Some(id) = self.selected_sound {
                    self.play_sound(id, self.play_mode);
                }
                RemoteReply::ok()
            }
            RemoteCommand::PlayCurrentSoundAgain => {
                self.play_current_again();
                RemoteReply::ok()
            }
            RemoteCommand::PlayPreviouslyPlayedSound => {
                self.play_previously_played();
                RemoteReply::ok()
            }
            RemoteCommand::PauseResume => {
                self.toggle_pause();
                RemoteReply::ok()
            }
            RemoteCommand::Stop => {
                self.stop();
                RemoteReply::ok()
            }
            RemoteCommand::AddSound { path } => {
                self.import_files(vec![PathBuf::from(path)]);
                RemoteReply::ok()
            }
            RemoteCommand::RemoveSelectedEntries => {
                self.remove_selected();
                RemoteReply::ok()
            }
            RemoteCommand::RemoveCategory => {
                let idx = self.selected_category;
                self.delete_category(idx);
                RemoteReply::ok()
            }
            RemoteCommand::SaveSoundlist => {
                self.save_soundlist();
                RemoteReply::ok()
            }
            RemoteCommand::GetSoundFileCount => {
                RemoteReply::ok().with_line(self.list.sounds.len().to_string())
            }
            RemoteCommand::GetSoundlist => RemoteReply::ok().with_line(self.list.to_xml()),
            RemoteCommand::GetVersion => {
                RemoteReply::ok().with_line(env!("CARGO_PKG_VERSION").to_string())
            }
            RemoteCommand::GetRemoteControlVersion => RemoteReply::ok().with_line("1"),
        }
    }

    // ------------------------------------------------------------------ tick

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.status = self.engine.status();
        self.mic_level = self.engine.mic_level();
        self.out_level = self.engine.output_level();

        while let Some(ev) = self.engine.try_event() {
            match ev {
                EngineEvent::Started { sound_id } => {
                    self.current_sound = Some(sound_id);
                    if let Some(hash) = self.list.find(sound_id).map(|s| s.hash.clone()) {
                        if !hash.is_empty() {
                            self.stats.record_play(&hash);
                            self.stats_dirty = true;
                        }
                    }
                }
                EngineEvent::Finished { .. } => {
                    self.current_sound = None;
                    self.release_auto_keys();
                    self.restore_ducking();
                }
                EngineEvent::RecordingStarted => {
                    self.recording = true;
                }
                EngineEvent::RecordingFinished { path, duration_ms } => {
                    self.recording = false;
                    self.last_recording = Some(path.clone());
                    self.toast(
                        format!(
                            "Recording saved ({}s): {}",
                            duration_ms / 1000,
                            path.display()
                        ),
                        false,
                    );
                    self.postprocess_recording(path);
                }
                EngineEvent::Error(err) => {
                    if err.contains("loopback") || err.contains("recording") {
                        self.recording = false;
                    }
                    self.release_auto_keys();
                    self.toast(err, true);
                }
                EngineEvent::MicLevel(_) | EngineEvent::OutputLevel(_) => {}
            }
        }

        while let Ok(req) = self.remote_rx.try_recv() {
            let reply = self.handle_remote(req.cmd);
            let _ = req.reply.send(reply);
        }

        while let Ok(result) = self.import_rx.try_recv() {
            if let Some(err) = result.error {
                self.toast(format!("{}: {err}", result.path.display()), true);
            } else {
                let duration = result.duration_ms.unwrap_or(0);
                self.add_sound_now(result.path, duration, result.loudness_db, result.hash);
            }
        }

        while let Ok((path, result)) = self.tts_rx.try_recv() {
            self.tts_generating = false;
            match result {
                Ok(()) => {
                    let mode = self.tts_play_on;
                    let id = self.add_sound_now(
                        path.clone(),
                        rustpad_audio::decode::probe_duration_ms(&path).unwrap_or(0),
                        None,
                        hash_for_path(&path),
                    );
                    if let Ok(audio) = rustpad_audio::decode_file(&path) {
                        let gain = self.sound_gain(self.list.find(id).expect("just added"));
                        self.engine.send(EngineCommand::PlayDecoded {
                            request: PlayRequest {
                                sound_id: id,
                                source: PlaySource::Decoded(Arc::new(audio.clone())),
                                gain,
                                targets: self.targets(mode),
                                looping: false,
                                start_ms: 0,
                            },
                            audio: Arc::new(audio),
                        });
                    }
                    let _ = std::fs::remove_file(path);
                    self.dirty = true;
                }
                Err(err) => self.toast(format!("TTS failed: {err:#}"), true),
            }
        }

        let mut key_events = Vec::new();
        if let Some(hook) = &self.hook {
            while let Some(ev) = hook.try_event() {
                key_events.push(ev);
            }
        }
        for ev in key_events {
            self.hotkey_trigger = true;
            self.handle_key(ev);
            self.hotkey_trigger = false;
        }

        self.save_stats_if_due(false);
        self.toasts
            .retain(|t| t.at.elapsed() < Duration::from_secs(6));

        if self.recording {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn postprocess_recording(&mut self, path: PathBuf) {
        if !self.cfg.trim_silence && !self.cfg.auto_normalize_recording {
            return;
        }
        let Ok(mut clip) = wav::read_wav(&path) else {
            return;
        };
        if self.cfg.trim_silence {
            dsp::trim_silence(
                &mut clip.samples,
                clip.channels,
                clip.sample_rate,
                -50.0,
                80,
            );
        }
        if self.cfg.auto_normalize_recording {
            dsp::normalize_peak(&mut clip.samples, -1.0);
        }
        if let Err(err) = wav::write_wav(&path, clip.sample_rate, clip.channels, &clip.samples) {
            tracing::warn!("recording post-process: {err:#}");
        }
    }

    pub fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    pub fn shutdown(&mut self) {
        self.release_auto_keys();
        self.save_config();
        self.save_stats_if_due(true);
        if self.dirty {
            self.save_soundlist();
        }
        self.restore_ducking();
        self.engine.shutdown();
    }

    #[allow(dead_code)]
    pub fn theme_preference(&self) -> Theme {
        self.cfg.theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustpad_audio::types::DeviceInfo;

    fn device(name: &str, default: bool) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            is_default: default,
            kind: rustpad_audio::types::DeviceKind::Output,
            channels: 2,
            sample_rate: 48_000,
        }
    }

    #[test]
    fn recognizes_common_virtual_cables() {
        assert!(is_virtual_cable("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(is_virtual_cable(
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"
        ));
        assert!(is_virtual_cable("Virtual Audio Cable"));
        assert!(!is_virtual_cable("Динамики (MCHOSE X9)"));
        assert!(!is_virtual_cable("S23A950D (NVIDIA High Definition Audio)"));
    }

    #[test]
    fn mic_output_auto_selects_a_cable_but_never_speakers() {
        let speakers = vec![device("Динамики (MCHOSE X9)", true)];
        assert_eq!(pick_mic_output(&None, &speakers), None);

        let with_cable = vec![
            device("Динамики (MCHOSE X9)", true),
            device("CABLE Input (VB-Audio Virtual Cable)", false),
        ];
        assert_eq!(
            pick_mic_output(&None, &with_cable).as_deref(),
            Some("CABLE Input (VB-Audio Virtual Cable)")
        );
        // A valid explicit choice is kept even if another cable appears.
        let current = Some("CABLE Input (VB-Audio Virtual Cable)".to_string());
        assert_eq!(
            pick_mic_output(&current, &with_cable).as_deref(),
            Some("CABLE Input (VB-Audio Virtual Cable)")
        );
        // A stale choice is replaced by an installed cable.
        let stale = Some("CABLE Input (removed)".to_string());
        assert_eq!(
            pick_mic_output(&stale, &with_cable).as_deref(),
            Some("CABLE Input (VB-Audio Virtual Cable)")
        );
    }
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
        crate::ui::draw(self, ctx);
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}

fn engine_config(cfg: &AppConfig) -> EngineConfig {
    EngineConfig {
        output_device: cfg.output_device.clone(),
        mic_output_device: cfg.mic_output_device.clone(),
        capture_device: cfg.capture_device.clone(),
        master_volume: cfg.master_volume,
        speakers_volume: cfg.speakers_volume,
        mic_volume: cfg.microphone_volume,
        mic_passthrough: cfg.mic_passthrough,
        mic_passthrough_gain: cfg.mic_passthrough_gain,
        mic_delay_ms: cfg.delay_mic_ms,
        block_voice: cfg.block_voice_while_playing,
    }
}

/// Names used by the common virtual cables that expose a render ("Input") endpoint.
fn is_virtual_cable(name: &str) -> bool {
    let name = name.to_lowercase();
    [
        "cable input",
        "vb-audio",
        "vb-cable",
        "voicemeeter input",
        "voicemeeter vaio",
        "virtual audio cable",
        "virtual cable",
        "virtual mic",
        "virtual microphone",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

/// Keeps a valid microphone output, otherwise auto-selects a virtual cable and
/// never falls back to speakers (that would create feedback).
fn pick_mic_output(current: &Option<String>, devices: &[DeviceInfo]) -> Option<String> {
    if let Some(name) = current {
        if devices.iter().any(|d| &d.name == name) {
            return Some(name.clone());
        }
    }
    devices
        .iter()
        .find(|d| is_virtual_cable(&d.name))
        .map(|d| d.name.clone())
}

fn pick_device(current: &Option<String>, devices: &[DeviceInfo]) -> Option<String> {
    if let Some(name) = current {
        if devices.iter().any(|d| &d.name == name) {
            return Some(name.clone());
        }
    }
    devices
        .iter()
        .find(|d| d.is_default)
        .or_else(|| devices.first())
        .map(|d| d.name.clone())
}

pub fn apply_engine_settings(state: &mut AppState) {
    state.cfg.output_device = pick_device(&state.cfg.output_device, &state.out_devices);
    state.cfg.capture_device = pick_device(&state.cfg.capture_device, &state.in_devices);
    state.cfg.mic_output_device = pick_mic_output(&state.cfg.mic_output_device, &state.out_devices);
    state
        .engine
        .send(EngineCommand::SetDevices(engine_config(&state.cfg)));
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-6).log10()
}

fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect()
}

fn pseudo_random(len: usize) -> usize {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    nanos % len.max(1)
}

fn digit_index(vk: u32) -> Option<usize> {
    match vk {
        0x31..=0x39 => Some((vk - 0x30) as usize),
        0x30 => Some(10),
        _ => None,
    }
}

fn numpad_index(vk: u32) -> Option<usize> {
    match vk {
        0x61..=0x69 => Some((vk - 0x60) as usize),
        0x60 => Some(0),
        _ => None,
    }
}

fn modifier_of(vk: u32) -> Option<Modifiers> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some(Modifiers::SHIFT),
        0x11 | 0xA2 | 0xA3 => Some(Modifiers::CTRL),
        0x12 | 0xA4 | 0xA5 => Some(Modifiers::ALT),
        0x5B | 0x5C => Some(Modifiers::WIN),
        _ => None,
    }
}

fn mode_from_flags(speakers: Option<bool>, mic: Option<bool>) -> PlayMode {
    match (speakers, mic) {
        (Some(true), Some(true)) => PlayMode::Both,
        (Some(true), Some(false)) => PlayMode::Speakers,
        (Some(false), Some(true)) => PlayMode::Microphone,
        _ => PlayMode::Default,
    }
}

/// Human-readable action names for the hotkey overview.
pub fn action_label(action: &str) -> &'static str {
    use rustpad_core::config::action as a;
    match action {
        a::PLAY_SELECTED => "Play selected sound",
        a::PLAY_PREVIOUS => "Play previous sound",
        a::PLAY_NEXT => "Play next sound",
        a::PLAY_RANDOM => "Play random sound",
        a::PLAY_RANDOM_ALL => "Play random sound (all)",
        a::PLAY_CURRENT_AGAIN => "Play current sound again",
        a::PLAY_PREVIOUSLY_PLAYED => "Play previously played",
        a::STOP => "Stop playback",
        a::PAUSE_RESUME => "Pause / resume",
        a::PLAY => "Play",
        a::VOLUME_UP => "Volume up",
        a::VOLUME_DOWN => "Volume down",
        a::VOLUME_MUTE => "Mute",
        a::MODE_DEFAULT => "Play mode: default",
        a::MODE_SPEAKERS => "Play mode: speakers",
        a::MODE_MICROPHONE => "Play mode: microphone",
        a::MODE_SWITCH => "Switch play mode",
        a::CATEGORY_PREV => "Previous category",
        a::CATEGORY_NEXT => "Next category",
        a::SOUND_PREV => "Select previous sound",
        a::SOUND_NEXT => "Select next sound",
        a::RECORD_MIC => "Record microphone",
        a::RECORD_SPEAKERS => "Record speakers",
        a::RECORD_STOP => "Stop recording",
        a::HOTKEYS_TOGGLE => "Toggle hotkeys",
        a::AUTO_KEYS_TOGGLE => "Toggle Auto Keys",
        a::HOTBAR_PREV_PAGE => "Hotbar previous page",
        a::HOTBAR_NEXT_PAGE => "Hotbar next page",
        a::SHOW_WINDOW => "Show window",
        _ => "Unknown action",
    }
}

pub fn hotkey_label(hk: Option<Hotkey>) -> String {
    hk.map(|h| h.display()).unwrap_or_default()
}

/// Soundpad's own `hash` (volume serial + NTFS file id), falling back to the
/// synthetic hash only when the file id is unavailable.
pub fn hash_for_path(path: &std::path::Path) -> String {
    rustpad_platform::soundpad_hash(path).unwrap_or_else(|_| {
        synthetic_hash(path, std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    })
}

pub fn rgb_to_egui(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

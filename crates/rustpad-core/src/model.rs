//! Core data model shared by the audio engine, the GUI and the remote-control server.
//!
//! The model intentionally mirrors the on-disk `.spl` soundlist of Soundpad 4.x
//! (see `docs/EVIDENCE.md`), with a few additive extension fields that older
//! readers simply ignore.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hotkey::Hotkey;

/// Extensions we try to import. Decoding support is decided by `rustpad-audio`.
pub const SOUND_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "flac", "ogg", "oga", "ogv", "ogx", "m4a", "mp4", "aac", "wma", "opus",
];

/// 24-bit RGB colour as used by soundlist `<Category color="RRGGBB">`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Rgb(r, g, b))
    }

    pub fn to_hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    pub fn from_u32(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn to_u32(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }

    pub fn to_egui(self) -> egui_like::Color32 {
        egui_like::Color32::from_rgb(self.0, self.1, self.2)
    }
}

/// Tiny decoupling shim so `rustpad-core` does not depend on egui.
/// The GUI crate provides the real `Color32` via `From<Rgb>`.
pub mod egui_like {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Color32(pub u8, pub u8, pub u8, pub u8);
    impl Color32 {
        pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
            Color32(r, g, b, 255)
        }
        pub fn to_tuple_rgba(self) -> [u8; 4] {
            [self.0, self.1, self.2, self.3]
        }
    }
}

/// Where a sound is audible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PlayMode {
    /// Use the configured default target.
    #[default]
    Default,
    Speakers,
    Microphone,
    /// Both speakers and microphone.
    Both,
}

impl PlayMode {
    pub fn on_speakers(self) -> bool {
        matches!(self, PlayMode::Speakers | PlayMode::Both)
    }

    pub fn on_microphone(self) -> bool {
        matches!(self, PlayMode::Microphone | PlayMode::Both)
    }

    pub fn next(self) -> Self {
        match self {
            PlayMode::Default => PlayMode::Speakers,
            PlayMode::Speakers => PlayMode::Microphone,
            PlayMode::Microphone => PlayMode::Both,
            PlayMode::Both => PlayMode::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlayMode::Default => "Default",
            PlayMode::Speakers => "Speakers",
            PlayMode::Microphone => "Microphone",
            PlayMode::Both => "Speakers + Microphone",
        }
    }
}

/// One sound file in the list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sound {
    /// Stable index used by `<Sound id="..">` references and by the remote API.
    pub id: u32,
    /// Soundpad stats identity (`A-B-C`). Preserved verbatim; may be empty for new files.
    pub hash: String,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    /// `YYYY-MM-DD`
    pub added_on: String,
    /// Extension: per-sound colour for the list / hotbar.
    pub color: Option<Rgb>,
    /// Extension: free-form tag shown in the "Tag" column.
    pub tag: Option<String>,
    /// Cached loudness in dBFS (RMS). Filled by the audio layer on import.
    pub loudness_db: Option<f32>,
    /// Extension: per-sound hotkey (Soundpad stores these in an unrecovered format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<Hotkey>,
    /// Unknown attributes preserved from foreign `.spl` files (round-trip safety).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra: std::collections::BTreeMap<String, String>,
}

impl Sound {
    pub fn new(id: u32, path: impl Into<PathBuf>, duration_ms: u64, added_on: String) -> Self {
        let path = path.into();
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        Sound {
            id,
            hash: String::new(),
            path,
            title,
            artist: String::new(),
            duration_ms,
            added_on,
            color: None,
            tag: None,
            loudness_db: None,
            hotkey: None,
            extra: std::collections::BTreeMap::new(),
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    pub fn duration_label(&self) -> String {
        format_duration(self.duration_ms)
    }
}

/// Soundpad uses `m:ss` (or `h:mm:ss` for long files) in `.spl` files.
pub fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Parses `m:ss`, `h:mm:ss`, `mm:ss.mmm`, or a plain number of seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<f64>() {
        return Some((secs * 1000.0).round() as u64);
    }
    let mut parts = s.split(':').collect::<Vec<_>>();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    parts.reverse();
    let mut total = 0f64;
    let mut mult = 1f64;
    for p in parts {
        total += p.replace(',', ".").parse::<f64>().ok()? * mult;
        mult *= 60.0;
    }
    Some((total * 1000.0).round() as u64)
}

/// `YYYY-MM-DD` for today (local time, no chrono dependency).
pub fn today_iso() -> String {
    #[cfg(windows)]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let days = secs / 86_400;
        // civil_from_days (Howard Hinnant)
        let z = days as i64 + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}")
    }
    #[cfg(not(windows))]
    {
        String::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CategoryKind {
    /// A user category.
    #[default]
    Normal,
    /// Built-in "All sounds" view (not persisted).
    AllSounds,
    /// Soundpad's `type="1" hidden="true"` category.
    Hidden,
}

impl Category {
    /// True for the built-in "All sounds" view (synthetic or Soundpad's hidden
    /// `type="1"` category).
    pub fn is_all(&self) -> bool {
        matches!(self.kind, CategoryKind::AllSounds | CategoryKind::Hidden)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    /// `gen_A`, `stock_icon_cat_home`, or a file path.
    pub icon: String,
    pub color: Option<Rgb>,
    pub hidden: bool,
    pub kind: CategoryKind,
    /// Sound ids in display order.
    pub sound_ids: Vec<u32>,
    /// Extension: relative hotkey index (`01`..`99`) for category routing.
    pub index: Option<String>,
    /// Extension: hotkey that plays a random sound from this category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<Hotkey>,
    /// Unknown attributes preserved from foreign `.spl` files.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra: std::collections::BTreeMap<String, String>,
}

impl Category {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Category {
            icon: default_category_icon(&name),
            name,
            color: None,
            hidden: false,
            kind: CategoryKind::Normal,
            sound_ids: Vec::new(),
            index: None,
            hotkey: None,
            extra: std::collections::BTreeMap::new(),
        }
    }
}

pub fn default_category_icon(name: &str) -> String {
    let first = name.chars().next().unwrap_or('?');
    if first.is_ascii_alphanumeric() {
        format!("gen_{}", first.to_ascii_uppercase())
    } else {
        "stock_icon_cat_list".to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotbarItem {
    pub sound_id: u32,
    pub color: Option<Rgb>,
    /// Overrides the sound title when set.
    pub label: Option<String>,
    /// Extension: hotkey that triggers this hotbar cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<Hotkey>,
}

impl HotbarItem {
    pub fn new(sound_id: u32) -> Self {
        HotbarItem {
            sound_id,
            color: None,
            label: None,
            hotkey: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HotbarPage {
    pub title: String,
    pub icon: String,
    pub color: Option<Rgb>,
    pub items: Vec<HotbarItem>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra: std::collections::BTreeMap<String, String>,
}

impl HotbarPage {
    pub fn empty() -> Self {
        HotbarPage {
            title: String::new(),
            icon: String::new(),
            color: None,
            items: Vec::new(),
            extra: std::collections::BTreeMap::new(),
        }
    }
}

/// The whole sound list: files, categories, hotbar, UI selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Soundlist {
    pub sounds: Vec<Sound>,
    pub categories: Vec<Category>,
    pub hotbar: Vec<HotbarPage>,
    #[serde(default)]
    pub selected_category: usize,
    #[serde(default)]
    pub selected_sound: Option<u32>,
    /// Absolute path the list was loaded from / saved to.
    #[serde(default)]
    pub file_path: Option<PathBuf>,
}

impl Default for Soundlist {
    fn default() -> Self {
        Self::new()
    }
}

impl Soundlist {
    pub fn new() -> Self {
        Soundlist {
            sounds: Vec::new(),
            categories: vec![Category {
                name: "Default".to_string(),
                icon: "stock_icon_cat_home".to_string(),
                color: None,
                hidden: false,
                kind: CategoryKind::Normal,
                sound_ids: Vec::new(),
                index: None,
                hotkey: None,
                extra: std::collections::BTreeMap::new(),
            }],
            hotbar: vec![HotbarPage::empty()],
            selected_category: 0,
            selected_sound: None,
            file_path: None,
        }
    }

    pub fn next_id(&self) -> u32 {
        self.sounds
            .iter()
            .map(|s| s.id)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    pub fn find(&self, id: u32) -> Option<&Sound> {
        self.sounds.iter().find(|s| s.id == id)
    }

    pub fn find_mut(&mut self, id: u32) -> Option<&mut Sound> {
        self.sounds.iter_mut().find(|s| s.id == id)
    }

    /// Sound ids in the selected category, or every sound for `AllSounds`.
    pub fn visible_sounds(&self) -> Vec<&Sound> {
        self.sounds_for_category(self.selected_category)
    }

    pub fn sounds_for_category(&self, cat: usize) -> Vec<&Sound> {
        match self.categories.get(cat) {
            None => Vec::new(),
            // Soundpad's `type="1"` category is the built-in "All sounds" view: it
            // has no explicit members and always contains the whole list.
            Some(c) if c.is_all() => self.sounds.iter().collect(),
            Some(c) => c.sound_ids.iter().filter_map(|id| self.find(*id)).collect(),
        }
    }

    pub fn add_path(&mut self, path: impl Into<PathBuf>, duration_ms: u64) -> u32 {
        let id = self.next_id();
        let sound = Sound::new(id, path, duration_ms, today_iso());
        self.sounds.push(sound);
        if let Some(cat) = self.categories.get_mut(self.selected_category) {
            if cat.is_all() {
                if let Some(default) = self
                    .categories
                    .iter_mut()
                    .find(|c| c.kind == CategoryKind::Normal)
                {
                    default.sound_ids.push(id);
                }
            } else {
                cat.sound_ids.push(id);
            }
        }
        self.selected_sound = Some(id);
        id
    }

    /// Removes a sound from the list, categories and hotbar. Returns the removed sound.
    pub fn remove(&mut self, id: u32) -> Option<Sound> {
        let pos = self.sounds.iter().position(|s| s.id == id)?;
        let sound = self.sounds.remove(pos);
        // Soundpad identifies sounds by document order (0-based), not by an
        // `id` attribute, so ids are compacted after every removal. References
        // in categories/hotbar are remapped to keep both readers consistent.
        self.compact_ids();
        Some(sound)
    }

    /// Renumbers sounds to `0..n` in document order and remaps every reference.
    ///
    /// Soundpad has no per-sound `id` attribute: a sound's identity is its
    /// position. Compacting keeps RustPad's in-memory ids aligned with what
    /// Soundpad will infer after reading a written `.spl` file.
    pub fn compact_ids(&mut self) {
        let remap: std::collections::HashMap<u32, u32> = self
            .sounds
            .iter()
            .enumerate()
            .map(|(index, s)| (s.id, index as u32))
            .collect();
        for (index, sound) in self.sounds.iter_mut().enumerate() {
            sound.id = index as u32;
        }
        for cat in &mut self.categories {
            // Drop dangling references first, then remap the survivors.
            cat.sound_ids.retain(|id| remap.contains_key(id));
            for id in &mut cat.sound_ids {
                *id = remap[id];
            }
            cat.sound_ids.dedup();
        }
        for page in &mut self.hotbar {
            page.items.retain(|i| remap.contains_key(&i.sound_id));
            for item in &mut page.items {
                item.sound_id = remap[&item.sound_id];
            }
        }
        self.selected_sound = self.selected_sound.and_then(|s| remap.get(&s).copied());
    }

    /// Removes sounds whose files no longer exist, returning their ids.
    pub fn remove_dead_entries(&mut self) -> Vec<u32> {
        let dead: Vec<u32> = self
            .sounds
            .iter()
            .filter(|s| !s.exists())
            .map(|s| s.id)
            .collect();
        for id in &dead {
            self.remove(*id);
        }
        dead
    }
}

/// Playback statistics keyed by the Soundpad `hash` attribute.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatEntry {
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub last_played_on: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub entries: std::collections::HashMap<String, StatEntry>,
}

impl Stats {
    pub fn record_play(&mut self, hash: &str) {
        let e = self.entries.entry(hash.to_string()).or_default();
        e.play_count += 1;
        e.last_played_on = Some(today_iso());
    }

    pub fn play_count(&self, hash: &str) -> u32 {
        self.entries.get(hash).map(|e| e.play_count).unwrap_or(0)
    }

    pub fn last_played(&self, hash: &str) -> Option<&str> {
        self.entries
            .get(hash)
            .and_then(|e| e.last_played_on.as_deref())
    }
}

/// Rough file identity used when a Soundpad hash is unavailable.
/// Format mimics `A-B-C` but is not byte-compatible with Soundpad's algorithm
/// (which is unknown), so stats imported from Soundpad still match by stored hash.
pub fn synthetic_hash(path: &Path, size: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut h);
    let a = h.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    a.hash(&mut h2);
    size.hash(&mut h2);
    let b = h2.finish();
    format!(
        "{}-{}-{}",
        a % 1_000_000_000,
        (size / 65536) * 65536,
        b % 1_000_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_roundtrip() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(2_000), "0:02");
        assert_eq!(format_duration(65_000), "1:05");
        assert_eq!(format_duration(3_725_000), "1:02:05");
        assert_eq!(parse_duration("0:02"), Some(2_000));
        assert_eq!(parse_duration("1:02:05"), Some(3_725_000));
        assert_eq!(parse_duration("12.5"), Some(12_500));
    }

    #[test]
    fn rgb_hex() {
        assert_eq!(Rgb::from_hex("7300AB"), Some(Rgb(115, 0, 171)));
        assert_eq!(Rgb(115, 0, 171).to_hex(), "7300AB");
        assert_eq!(Rgb::from_u32(0x4DB05C).to_hex(), "4DB05C");
    }

    #[test]
    fn remove_cleans_references() {
        let mut l = Soundlist::new();
        let id = l.add_path("C:/x/y.mp3", 1000);
        l.categories[0].sound_ids.push(id);
        l.hotbar[0].items.push(HotbarItem::new(id));
        assert!(l.remove(id).is_some());
        assert!(l.sounds.is_empty());
        assert!(l.categories[0].sound_ids.is_empty());
        assert!(l.hotbar[0].items.is_empty());
    }
}

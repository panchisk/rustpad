//! Read/write the Soundpad 4.x `.spl` formats (sound list and statistics).
//!
//! Formats were recovered from live files on disk; see `docs/EVIDENCE.md` §5 and
//! `docs/evidence/sample-soundlist.spl`.
//!
//! * `Soundlist::to_xml` must reproduce the structure below:
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <Soundlist>
//!   <Sound hash=".." url="C:\abs\file.mp3" artist="" title="file"
//!          duration="0:02" addedOn="2026-10-04" id="0"/>
//!   <Categories>
//!     <Category type="1" icon="stock_icon_cat_list" hidden="true"/>
//!     <Category name="Home" icon="stock_icon_cat_home"><Sound id="1"/></Category>
//!   </Categories>
//!   <Hotbar><HotbarPage/></Hotbar>
//! </Soundlist>
//! ```
//!
//! Requirements:
//! - UTF-8, `\n` newlines, two-space indentation, XML-escaped attribute values.
//! - `duration` uses [`crate::model::format_duration`], `addedOn` is `YYYY-MM-DD`.
//! - Sound ids round-trip exactly; unknown attributes/elements on Sound, Category and
//!   HotbarPage are preserved in the model's `extra` map (do not drop them).
//! - Category `color` is uppercase `RRGGBB`. `type="1"` + `hidden="true"` maps to
//!   [`CategoryKind::Hidden`]; plain categories to `Normal`.
//! - Hotbar items are `<HotbarItem soundId=".." .../>` children of `<HotbarPage>`
//!   (element and attribute names observed in Soundpad's literal pool); the older
//!   `<Sound id=".."/>` form is accepted when reading.
//! - Parsing must be tolerant: missing categories/hotbar, dangling sound ids, missing
//!   `id` (use document order), malformed duration (0 ms) and unknown elements must not
//!   fail the whole load. A missing `id` attribute on a `<Sound>` reference is an error
//!   only if the reference cannot be resolved; then it is skipped.
//! - `Stats` uses `<Stats><Sound hash=".." playCount=".." lastPlayedOn=".."/></Stats>`.
//! - Saves are atomic (temp file in the same directory, then rename) and create parent
//!   directories.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::hotkey::Hotkey;
use crate::model::{
    default_category_icon, format_duration, parse_duration, today_iso, Category, CategoryKind,
    HotbarItem, HotbarPage, Rgb, Sound, Soundlist, StatEntry, Stats,
};

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// Attributes we consume explicitly; everything else is preserved in `extra`.
const SOUND_ATTRS: &[&str] = &[
    "hash",
    "url",
    "artist",
    "title",
    "duration",
    "addedOn",
    "id",
    "color",
    "hotkey",
    // Soundpad's own tag attribute is `customTag`; `tag` is accepted as an alias
    // for lists written by older RustPad builds.
    "customTag",
    "tag",
    "loudnessDb",
];
const CATEGORY_ATTRS: &[&str] = &["name", "icon", "type", "hidden", "color", "index", "hotkey"];
const PAGE_ATTRS: &[&str] = &["title", "icon", "color", "hotkey"];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Escapes a value for use inside a double-quoted XML attribute.
///
/// Windows paths contain backslashes, which are literal in XML and therefore
/// preserved verbatim. Whitespace controls become numeric references so that a
/// parser's attribute-value normalisation cannot eat them; other C0 controls
/// (which XML 1.0 cannot represent at all) are replaced with spaces.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn push_attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&escape_attr(value));
    out.push('"');
}

fn is_reserved(name: &str, reserved: &[&str]) -> bool {
    reserved.iter().any(|r| r.eq_ignore_ascii_case(name))
}

fn push_extra(out: &mut String, extra: &BTreeMap<String, String>, reserved: &[&str]) {
    for (k, v) in extra {
        if k.is_empty() || is_reserved(k, reserved) {
            continue;
        }
        push_attr(out, k, v);
    }
}

fn strip_bom(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

fn attr_value<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attribute(name)
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_color(node: roxmltree::Node<'_, '_>) -> Option<Rgb> {
    node.attribute("color").and_then(Rgb::from_hex)
}

fn parse_hotkey(node: roxmltree::Node<'_, '_>) -> Option<Hotkey> {
    node.attribute("hotkey").and_then(Hotkey::parse)
}

fn extra_attrs(node: roxmltree::Node<'_, '_>, reserved: &[&str]) -> BTreeMap<String, String> {
    let mut extra = BTreeMap::new();
    for attr in node.attributes() {
        let name = attr.name();
        if is_reserved(name, reserved) {
            continue;
        }
        extra.insert(name.to_string(), attr.value().to_string());
    }
    extra
}

/// Writes `contents` to `path` atomically: temp file in the same directory,
/// then rename (with a copy+remove fallback).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        _ => {}
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "soundlist.spl".to_string());
    let tmp_name = format!("{file_name}.tmp");
    let tmp = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(&tmp_name),
        _ => PathBuf::from(&tmp_name),
    };

    std::fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;

    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // e.g. a file lock or a cross-device quirk: copy over the target.
            std::fs::copy(&tmp, path)
                .with_context(|| format!("failed to replace {}", path.display()))?;
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Soundlist
// ---------------------------------------------------------------------------

impl Soundlist {
    /// Loads a `.spl` file from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let xml = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut list =
            Self::from_xml(&xml).with_context(|| format!("failed to parse {}", path.display()))?;
        list.file_path = Some(path.to_path_buf());
        Ok(list)
    }

    /// Saves a `.spl` file, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, self.to_xml().as_bytes())
    }

    /// Serializes the list to XML (no trailing newline guarantees).
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        out.push_str(XML_DECL);
        out.push_str("<Soundlist>\n");

        for (index, sound) in self.sounds.iter().enumerate() {
            out.push_str("  <Sound");
            push_attr(&mut out, "hash", &sound.hash);
            push_attr(&mut out, "url", &sound.path.to_string_lossy());
            push_attr(&mut out, "artist", &sound.artist);
            push_attr(&mut out, "title", &sound.title);
            push_attr(&mut out, "duration", &format_duration(sound.duration_ms));
            push_attr(&mut out, "addedOn", &sound.added_on);
            // Soundpad has no `id` attribute (position is the id); only write it
            // when it would be ambiguous.
            if sound.id != index as u32 {
                push_attr(&mut out, "id", &sound.id.to_string());
            }
            if let Some(color) = sound.color {
                push_attr(&mut out, "color", &color.to_hex());
            }
            if let Some(hotkey) = sound.hotkey {
                push_attr(&mut out, "hotkey", &hotkey.display());
            }
            if let Some(tag) = &sound.tag {
                push_attr(&mut out, "customTag", tag);
            }
            if let Some(db) = sound.loudness_db {
                push_attr(&mut out, "loudnessDb", &format!("{db:.2}"));
            }
            push_extra(&mut out, &sound.extra, SOUND_ATTRS);
            out.push_str("/>\n");
        }

        out.push_str("  <Categories>\n");
        for cat in &self.categories {
            if cat.kind == CategoryKind::AllSounds {
                continue;
            }
            let hidden = cat.hidden || cat.kind == CategoryKind::Hidden;
            out.push_str("    <Category");
            if !hidden || !cat.name.is_empty() {
                push_attr(&mut out, "name", &cat.name);
            }
            if hidden {
                push_attr(&mut out, "type", "1");
            }
            push_attr(&mut out, "icon", &cat.icon);
            if hidden {
                push_attr(&mut out, "hidden", "true");
            }
            if let Some(color) = cat.color {
                push_attr(&mut out, "color", &color.to_hex());
            }
            if let Some(index) = &cat.index {
                push_attr(&mut out, "index", index);
            }
            if let Some(hotkey) = cat.hotkey {
                push_attr(&mut out, "hotkey", &hotkey.display());
            }
            push_extra(&mut out, &cat.extra, CATEGORY_ATTRS);

            if cat.sound_ids.is_empty() {
                out.push_str("/>\n");
            } else {
                out.push_str(">\n");
                for id in &cat.sound_ids {
                    out.push_str("      <Sound");
                    push_attr(&mut out, "id", &id.to_string());
                    out.push_str("/>\n");
                }
                out.push_str("    </Category>\n");
            }
        }
        out.push_str("  </Categories>\n");

        out.push_str("  <Hotbar>\n");
        for page in &self.hotbar {
            out.push_str("    <HotbarPage");
            if !page.title.is_empty() {
                push_attr(&mut out, "title", &page.title);
            }
            if !page.icon.is_empty() {
                push_attr(&mut out, "icon", &page.icon);
            }
            if let Some(color) = page.color {
                push_attr(&mut out, "color", &color.to_hex());
            }
            push_extra(&mut out, &page.extra, PAGE_ATTRS);

            if page.items.is_empty() {
                out.push_str("/>\n");
            } else {
                out.push_str(">\n");
                for item in &page.items {
                    // Soundpad's own element/attribute names (recovered from the
                    // binary's UTF-16 literal pool): `<HotbarItem soundId="N">`.
                    out.push_str("      <HotbarItem");
                    push_attr(&mut out, "soundId", &item.sound_id.to_string());
                    if let Some(color) = item.color {
                        push_attr(&mut out, "color", &color.to_hex());
                    }
                    if let Some(label) = &item.label {
                        push_attr(&mut out, "label", label);
                    }
                    if let Some(hotkey) = item.hotkey {
                        push_attr(&mut out, "hotkey", &hotkey.display());
                    }
                    out.push_str("/>\n");
                }
                out.push_str("    </HotbarPage>\n");
            }
        }
        out.push_str("  </Hotbar>\n");
        out.push_str("</Soundlist>");

        out
    }

    /// Parses XML produced by Soundpad 4.x or by [`Self::to_xml`].
    pub fn from_xml(xml: &str) -> Result<Self> {
        let xml = strip_bom(xml);
        let doc = roxmltree::Document::parse(xml).context("invalid sound list XML")?;
        let root = doc.root_element();
        anyhow::ensure!(
            root.tag_name().name() == "Soundlist",
            "unexpected root element <{}>, expected <Soundlist>",
            root.tag_name().name()
        );

        let mut sounds: Vec<Sound> = Vec::new();
        let mut categories: Vec<Category> = Vec::new();
        let mut hotbar: Vec<HotbarPage> = Vec::new();
        let mut order: u32 = 0;

        for node in root.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "Sound" => {
                    sounds.push(parse_sound(node, order));
                    order = order.saturating_add(1);
                }
                "Categories" => {
                    for cat in node
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == "Category")
                    {
                        categories.push(parse_category(cat, &sounds));
                    }
                }
                "Hotbar" => {
                    for page in node
                        .children()
                        .filter(|n| n.is_element() && n.tag_name().name() == "HotbarPage")
                    {
                        hotbar.push(parse_hotbar_page(page, &sounds));
                    }
                }
                // Unknown root children are skipped for forward compatibility.
                _ => {}
            }
        }

        if categories.is_empty() {
            let mut default = Category::new("Default");
            default.icon = "stock_icon_cat_home".to_string();
            default.sound_ids = sounds.iter().map(|s| s.id).collect();
            categories.push(default);
        }
        if hotbar.is_empty() {
            hotbar.push(HotbarPage::empty());
        }

        let mut list = Soundlist {
            sounds,
            categories,
            hotbar,
            selected_category: 0,
            selected_sound: None,
            file_path: None,
        };
        // Soundpad identities are document positions; normalise explicit ids.
        list.compact_ids();
        Ok(list)
    }
}

fn parse_sound(node: roxmltree::Node<'_, '_>, order: u32) -> Sound {
    let id = node
        .attribute("id")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(order);
    let path = PathBuf::from(attr_value(node, "url").unwrap_or(""));
    let title = match attr_value(node, "title") {
        Some(t) => t.to_string(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    };
    Sound {
        id,
        hash: attr_value(node, "hash").unwrap_or("").to_string(),
        path,
        title,
        artist: attr_value(node, "artist").unwrap_or("").to_string(),
        duration_ms: attr_value(node, "duration")
            .and_then(parse_duration)
            .unwrap_or(0),
        added_on: match attr_value(node, "addedOn") {
            Some(v) if !v.trim().is_empty() => v.to_string(),
            _ => today_iso(),
        },
        color: parse_color(node),
        tag: node
            .attribute("customTag")
            .or_else(|| node.attribute("tag"))
            .map(|v| v.to_string()),
        loudness_db: node
            .attribute("loudnessDb")
            .and_then(|v| v.trim().parse::<f32>().ok()),
        hotkey: parse_hotkey(node),
        extra: extra_attrs(node, SOUND_ATTRS),
    }
}

fn parse_category(node: roxmltree::Node<'_, '_>, sounds: &[Sound]) -> Category {
    let name = node.attribute("name").unwrap_or("").to_string();
    let icon = match node.attribute("icon") {
        Some(v) if !v.trim().is_empty() => v.to_string(),
        _ => default_category_icon(&name),
    };
    let declared_hidden = node.attribute("hidden").map(parse_bool).unwrap_or(false);
    let type_one = node
        .attribute("type")
        .map(|v| v.trim() == "1")
        .unwrap_or(false);
    let hidden = declared_hidden || type_one;

    let known: HashSet<u32> = sounds.iter().map(|s| s.id).collect();
    let mut sound_ids = Vec::new();
    for child in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Sound")
    {
        // A reference without an id or to an unknown sound is skipped.
        let Some(id) = child
            .attribute("id")
            .and_then(|v| v.trim().parse::<u32>().ok())
        else {
            continue;
        };
        if known.contains(&id) {
            sound_ids.push(id);
        }
    }

    Category {
        name,
        icon,
        color: parse_color(node),
        hidden,
        kind: if hidden {
            CategoryKind::Hidden
        } else {
            CategoryKind::Normal
        },
        sound_ids,
        index: node
            .attribute("index")
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string),
        hotkey: parse_hotkey(node),
        extra: extra_attrs(node, CATEGORY_ATTRS),
    }
}

fn parse_hotbar_page(node: roxmltree::Node<'_, '_>, sounds: &[Sound]) -> HotbarPage {
    let known: HashSet<u32> = sounds.iter().map(|s| s.id).collect();
    let mut items = Vec::new();
    for child in node.children().filter(|n| n.is_element()) {
        let tag = child.tag_name().name();
        if tag != "HotbarItem" && tag != "Sound" {
            continue;
        }
        // Soundpad writes `soundId`; `id` is accepted for older RustPad files.
        let Some(sound_id) = child
            .attribute("soundId")
            .or_else(|| child.attribute("id"))
            .and_then(|v| v.trim().parse::<u32>().ok())
        else {
            continue;
        };
        if !known.contains(&sound_id) {
            continue;
        }
        items.push(HotbarItem {
            sound_id,
            color: parse_color(child),
            label: child.attribute("label").map(str::to_string),
            hotkey: parse_hotkey(child),
        });
    }

    HotbarPage {
        title: node.attribute("title").unwrap_or("").to_string(),
        icon: node.attribute("icon").unwrap_or("").to_string(),
        color: parse_color(node),
        items,
        extra: extra_attrs(node, PAGE_ATTRS),
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

impl Stats {
    /// Loads `stats.spl`; a missing file yields empty stats.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(xml) if xml.trim().is_empty() => Ok(Stats::default()),
            Ok(xml) => {
                Self::from_xml(&xml).with_context(|| format!("failed to parse {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Stats::default()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, self.to_xml().as_bytes())
    }

    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        out.push_str(XML_DECL);
        out.push_str("<Stats>\n");
        let mut entries: Vec<(&String, &StatEntry)> = self.entries.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (hash, entry) in entries {
            out.push_str("  <Sound");
            push_attr(&mut out, "hash", hash);
            push_attr(&mut out, "playCount", &entry.play_count.to_string());
            if let Some(date) = &entry.last_played_on {
                push_attr(&mut out, "lastPlayedOn", date);
            }
            out.push_str("/>\n");
        }
        out.push_str("</Stats>");
        out
    }

    pub fn from_xml(xml: &str) -> Result<Self> {
        let xml = strip_bom(xml);
        let doc = roxmltree::Document::parse(xml).context("invalid stats XML")?;
        let root = doc.root_element();
        anyhow::ensure!(
            root.tag_name().name() == "Stats",
            "unexpected root element <{}>, expected <Stats>",
            root.tag_name().name()
        );

        let mut entries = std::collections::HashMap::new();
        for node in root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Sound")
        {
            let Some(hash) = node.attribute("hash").filter(|h| !h.is_empty()) else {
                continue;
            };
            let play_count = node
                .attribute("playCount")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(0);
            let last_played_on = node
                .attribute("lastPlayedOn")
                .filter(|v| !v.trim().is_empty())
                .map(str::to_string);
            entries.insert(
                hash.to_string(),
                StatEntry {
                    play_count,
                    last_played_on,
                },
            );
        }
        Ok(Stats { entries })
    }
}

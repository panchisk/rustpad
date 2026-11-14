//! Integration tests for the Soundpad-compatible `.spl` reader/writer.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use rustpad_core::{
    Category, CategoryKind, HotbarItem, HotbarPage, Hotkey, Rgb, Sound, Soundlist, StatEntry, Stats,
};

// ---------------------------------------------------------------------------
// Temporary directory helper (avoids depending on `tempfile`).
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "rustpad-spl-{}-{label}-{n}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/evidence/sample-soundlist.spl")
        .canonicalize()
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/evidence/sample-soundlist.spl")
        })
}

// ---------------------------------------------------------------------------
// 1. Real Soundpad file
// ---------------------------------------------------------------------------

#[test]
fn loads_real_soundpad_file() {
    let list = Soundlist::load(&sample_path()).expect("load sample soundlist");

    assert_eq!(list.sounds.len(), 158);
    assert_eq!(list.categories.len(), 9);
    assert_eq!(list.hotbar.len(), 1);
    assert_eq!(list.file_path.as_deref(), Some(sample_path().as_path()));

    let first = &list.sounds[0];
    assert_eq!(first.id, 0);
    assert_eq!(first.hash, "649904814-131072-126609");
    assert_eq!(first.title, "artas - nikto ne smeet mne prikazyvat'");
    assert_eq!(
        first.path.to_string_lossy(),
        "C:\\Users\\admin\\Documents\\soundpad\\artas - nikto ne smeet mne prikazyvat'.mp3"
    );
    assert_eq!(first.artist, "");
    assert_eq!(first.duration_ms, 2_000);
    assert_eq!(first.added_on, "2026-10-04");

    // The built-in special category: type="1" hidden="true", no name, no sounds.
    let hidden = &list.categories[0];
    assert!(hidden.hidden);
    assert_eq!(hidden.kind, CategoryKind::Hidden);
    assert_eq!(hidden.name, "");
    assert_eq!(hidden.icon, "stock_icon_cat_list");
    assert!(hidden.sound_ids.is_empty());

    // Category colours are parsed as uppercase RRGGBB.
    let koch = list.categories.iter().find(|c| c.name == "КОЧ").unwrap();
    assert_eq!(koch.color, Some(Rgb(0x00, 0x00, 0x00)));
    let nigga = list.categories.iter().find(|c| c.name == "NIGGA").unwrap();
    assert_eq!(nigga.color, Some(Rgb(0x96, 0x4B, 0x00)));
    let amor = list
        .categories
        .iter()
        .find(|c| c.name == "аморльщина")
        .unwrap();
    assert_eq!(amor.color, Some(Rgb(0x33, 0x33, 0x33)));

    // A normal category keeps its sound references and has no colour.
    let mine = list
        .categories
        .iter()
        .find(|c| c.name == "Мои звуки")
        .unwrap();
    assert_eq!(mine.kind, CategoryKind::Normal);
    assert!(!mine.hidden);
    assert_eq!(mine.icon, "stock_icon_cat_home");
    assert_eq!(mine.color, None);
    assert!(!mine.sound_ids.is_empty());
    assert!(mine.sound_ids.iter().all(|id| list.find(*id).is_some()));
}

// ---------------------------------------------------------------------------
// 2. XML -> model -> XML -> model stability
// ---------------------------------------------------------------------------

fn assert_same_list(a: &Soundlist, b: &Soundlist) {
    assert_eq!(a.sounds.len(), b.sounds.len(), "sound count");
    for (x, y) in a.sounds.iter().zip(&b.sounds) {
        assert_eq!(x.id, y.id);
        assert_eq!(x.hash, y.hash);
        assert_eq!(x.path, y.path);
        assert_eq!(x.title, y.title);
        assert_eq!(x.artist, y.artist);
        assert_eq!(x.duration_ms, y.duration_ms);
        assert_eq!(x.added_on, y.added_on);
        assert_eq!(x.color, y.color);
        assert_eq!(x.hotkey, y.hotkey);
        assert_eq!(x.extra, y.extra);
    }

    assert_eq!(a.categories.len(), b.categories.len(), "category count");
    for (x, y) in a.categories.iter().zip(&b.categories) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.icon, y.icon);
        assert_eq!(x.color, y.color);
        assert_eq!(x.hidden, y.hidden);
        assert_eq!(x.kind, y.kind);
        assert_eq!(x.sound_ids, y.sound_ids);
        assert_eq!(x.index, y.index);
        assert_eq!(x.hotkey, y.hotkey);
        assert_eq!(x.extra, y.extra);
    }

    assert_eq!(a.hotbar.len(), b.hotbar.len(), "hotbar page count");
    for (x, y) in a.hotbar.iter().zip(&b.hotbar) {
        assert_eq!(x.title, y.title);
        assert_eq!(x.icon, y.icon);
        assert_eq!(x.color, y.color);
        assert_eq!(x.extra, y.extra);
        assert_eq!(x.items.len(), y.items.len());
        for (i, j) in x.items.iter().zip(&y.items) {
            assert_eq!(i.sound_id, j.sound_id);
            assert_eq!(i.color, j.color);
            assert_eq!(i.label, j.label);
            assert_eq!(i.hotkey, j.hotkey);
        }
    }
}

#[test]
fn sample_round_trips_through_xml() {
    let original = Soundlist::load(&sample_path()).unwrap();
    let xml = original.to_xml();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Soundlist>\n"));
    assert!(!xml.contains('\r'));

    let first = Soundlist::from_xml(&xml).expect("reparse sample");
    assert_same_list(&original, &first);

    let second = Soundlist::from_xml(&first.to_xml()).expect("reparse canonical sample");
    assert_same_list(&first, &second);
    // Serialisation is deterministic.
    assert_eq!(first.to_xml(), second.to_xml());
}

// ---------------------------------------------------------------------------
// 3. Hotkeys and unknown attributes survive a disk save/load cycle
// ---------------------------------------------------------------------------

#[test]
fn saves_and_reloads_hotkeys_and_extras() {
    let dir = TempDir::new("save");
    let path = dir.join("nested/deeper/list.spl");

    let mut list = Soundlist::new();
    list.sounds.clear();
    list.categories.clear();
    list.hotbar.clear();

    let mut one = Sound::new(0, r"C:\Music\one.mp3", 65_000, "2026-01-02".to_string());
    one.hash = "1-2-3".to_string();
    one.artist = "Artist".to_string();
    one.color = Some(Rgb(0x12, 0x34, 0x56));
    one.hotkey = Some(Hotkey::parse("Ctrl+Shift+F1").unwrap());
    one.extra
        .insert("customAttr".to_string(), "custom & value".to_string());
    one.extra.insert("zzz".to_string(), "last".to_string());
    let mut two = Sound::new(7, r"C:\Music\two.mp3", 1_000, "2026-01-03".to_string());
    two.hotkey = Some(Hotkey::parse("Alt+2").unwrap());
    list.sounds.push(one);
    list.sounds.push(two);

    let mut home = Category::new("Home");
    home.color = Some(Rgb(0xAB, 0xCD, 0xEF));
    home.index = Some("01".to_string());
    home.hotkey = Some(Hotkey::parse("Ctrl+1").unwrap());
    home.sound_ids = vec![7, 0];
    home.extra.insert("foreign".to_string(), "yes".to_string());

    let mut hidden = Category::new("Secret");
    hidden.kind = CategoryKind::Hidden;
    hidden.hidden = true;
    hidden.icon = "stock_icon_cat_list".to_string();
    hidden.color = Some(Rgb(0, 0, 0));
    list.categories.push(home);
    list.categories.push(hidden);

    let mut page = HotbarPage {
        title: "Page 1".to_string(),
        icon: "gen_P".to_string(),
        color: Some(Rgb(0x10, 0x20, 0x30)),
        items: Vec::new(),
        extra: BTreeMap::new(),
    };
    page.extra.insert("pageFlag".to_string(), "1".to_string());
    let mut item = HotbarItem::new(7);
    item.color = Some(Rgb(0xFF, 0x00, 0x00));
    item.label = Some("Label & <tag>".to_string());
    item.hotkey = Some(Hotkey::parse("Ctrl+Alt+7").unwrap());
    page.items.push(item);
    list.hotbar.push(page);

    // Soundpad infers sound identity from document order; normalise before the
    // round-trip comparison so in-memory ids match what a reload produces.
    list.compact_ids();
    list.save(&path).expect("save soundlist");
    assert!(path.is_file());
    assert!(!path.with_file_name("list.spl.tmp").exists());

    let loaded = Soundlist::load(&path).expect("reload soundlist");
    assert_eq!(loaded.file_path.as_deref(), Some(path.as_path()));
    assert_same_list(&list, &loaded);

    assert_eq!(
        loaded.sounds[0].hotkey,
        Some(Hotkey::parse("Ctrl+Shift+F1").unwrap())
    );
    assert_eq!(
        loaded.sounds[0].extra.get("customAttr").map(String::as_str),
        Some("custom & value")
    );
    assert_eq!(loaded.categories[0].index.as_deref(), Some("01"));
    assert_eq!(
        loaded.categories[0].hotkey,
        Some(Hotkey::parse("Ctrl+1").unwrap())
    );
    assert_eq!(loaded.categories[1].kind, CategoryKind::Hidden);
    assert_eq!(
        loaded.hotbar[0].items[0].hotkey,
        Some(Hotkey::parse("Ctrl+Alt+7").unwrap())
    );
    assert_eq!(loaded.hotbar[0].color, Some(Rgb(0x10, 0x20, 0x30)));
}

// ---------------------------------------------------------------------------
// 4. Stats
// ---------------------------------------------------------------------------

#[test]
fn stats_round_trip_and_missing_file() {
    let dir = TempDir::new("stats");
    let path = dir.join("stats.spl");

    let missing = Stats::load(&dir.join("does-not-exist.spl")).expect("missing stats");
    assert!(missing.entries.is_empty());

    let mut stats = Stats::default();
    stats.record_play("649904814-131072-126609");
    stats.entries.insert(
        "x-y-z".to_string(),
        StatEntry {
            play_count: 3,
            last_played_on: Some("2026-10-16".to_string()),
        },
    );

    stats.save(&path).expect("save stats");
    assert!(path.is_file());
    assert!(!path.with_file_name("stats.spl.tmp").exists());

    let loaded = Stats::load(&path).expect("reload stats");
    assert_eq!(loaded.entries.len(), 2);
    assert_eq!(loaded.play_count("649904814-131072-126609"), 1);
    assert_eq!(loaded.play_count("x-y-z"), 3);
    assert_eq!(loaded.last_played("x-y-z"), Some("2026-10-16"));

    let reparsed = Stats::from_xml(&loaded.to_xml()).expect("reparse stats");
    assert_eq!(reparsed.entries.len(), 2);
    assert_eq!(reparsed.play_count("x-y-z"), 3);
    assert_eq!(reparsed.last_played("x-y-z"), Some("2026-10-16"));

    let xml = stats.to_xml();
    assert!(xml.contains("<Sound hash=\"x-y-z\" playCount=\"3\" lastPlayedOn=\"2026-10-16\"/>"));

    assert!(Stats::from_xml("<NotStats/>").is_err());
}

// ---------------------------------------------------------------------------
// 5. Tolerant parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_tolerantly() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Soundlist>
  <Sound hash="h0" url="C:\a.mp3" title="A" duration="bogus"/>
  <Sound hash="h1" url="C:\b.mp3" title="B" duration="1:05" id="7"/>
  <Categories>
    <Category name="C" icon="gen_C">
      <Sound id="999"/>
      <Sound id="7"/>
      <Sound/>
      <Unknown child="ignored"/>
    </Category>
  </Categories>
</Soundlist>"#;

    let list = Soundlist::from_xml(xml).expect("tolerant parse");
    assert_eq!(list.sounds.len(), 2);
    // Missing id falls back to document order.
    assert_eq!(list.sounds[0].id, 0);
    // Malformed duration becomes 0, missing addedOn becomes today.
    assert_eq!(list.sounds[0].duration_ms, 0);
    assert!(!list.sounds[0].added_on.is_empty());
    // Ids are normalised to document order (Soundpad's identity model) and the
    // duration is parsed.
    assert_eq!(list.sounds[1].id, 1);
    assert_eq!(list.sounds[1].duration_ms, 65_000);
    // Dangling and id-less references are skipped.
    assert_eq!(list.categories.len(), 1);
    assert_eq!(list.categories[0].sound_ids, vec![1]);
    // Missing Hotbar yields the default single empty page.
    assert_eq!(list.hotbar.len(), 1);
    assert!(list.hotbar[0].items.is_empty());

    // No Categories and no Hotbar at all: defaults are created.
    let bare =
        Soundlist::from_xml(r#"<Soundlist><Sound hash="h" url="C:\x.mp3" id="3"/></Soundlist>"#)
            .expect("bare parse");
    assert_eq!(bare.sounds.len(), 1);
    assert_eq!(bare.categories.len(), 1);
    assert_eq!(bare.categories[0].name, "Default");
    assert_eq!(bare.categories[0].sound_ids, vec![0]);
    assert_eq!(bare.hotbar.len(), 1);

    // A wrong root is an error.
    assert!(Soundlist::from_xml("<Categories/>").is_err());
    assert!(Soundlist::from_xml("not xml at all").is_err());
}

// ---------------------------------------------------------------------------
// 6. Escaping
// ---------------------------------------------------------------------------

#[test]
fn escapes_special_characters_and_preserves_windows_paths() {
    let title = "A & B < C > \"D\" 'E'";
    let artist = "R&D <x>";
    let path_str = r#"C:\Music\a&b<c>\"d\e'.mp3"#;

    let mut list = Soundlist::new();
    list.sounds.clear();
    let mut sound = Sound::new(0, path_str, 2_000, "2026-01-02".to_string());
    sound.title = title.to_string();
    sound.artist = artist.to_string();
    list.sounds.push(sound);
    list.categories[0].sound_ids = vec![0];

    let xml = list.to_xml();
    assert!(xml.contains("&amp;"));
    assert!(xml.contains("&lt;"));
    assert!(xml.contains("&gt;"));
    assert!(xml.contains("&quot;"));
    assert!(xml.contains("&apos;"));
    assert!(!xml.contains("A & B < C"));

    let back = Soundlist::from_xml(&xml).expect("reparse escaped list");
    assert_eq!(back.sounds[0].title, title);
    assert_eq!(back.sounds[0].artist, artist);
    assert_eq!(back.sounds[0].path.to_string_lossy(), path_str);

    // Same through an actual file on disk (UTF-8 round trip).
    let dir = TempDir::new("escape");
    let path = dir.join("escaped.spl");
    list.save(&path).unwrap();
    let reloaded = Soundlist::load(&path).unwrap();
    assert_eq!(reloaded.sounds[0].title, title);
    assert_eq!(reloaded.sounds[0].artist, artist);
    assert_eq!(reloaded.sounds[0].path.to_string_lossy(), path_str);
}

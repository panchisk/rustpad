//! egui front-end: menu bar, toolbar, category sidebar, sound table, hotbar,
//! transport, settings and the auxiliary windows (TTS, recorder, editor,
//! hotkey overview, about). `draw` is called once per frame from
//! `AppState::update`.

use std::path::PathBuf;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rustpad_audio::{EngineCommand, Target};
use rustpad_core::config::action as action_id;
use rustpad_core::model::{format_duration, PlayMode, Rgb};
use rustpad_core::Hotkey;

use crate::app::{self, AppState, HotkeyTarget};
use crate::theme;

/// Extensions offered in the "add files" dialog.
const SOUND_FILTER: &[&str] = &[
    "wav", "mp3", "flac", "ogg", "oga", "m4a", "aac", "wma", "opus", "mp4",
];

/// Six preset category/hotbar colours (Soundpad's palette is not recovered).
const PRESET_COLORS: [Rgb; 6] = [
    Rgb(0x4D, 0xB0, 0x5C),
    Rgb(0x2E, 0x7D, 0xD1),
    Rgb(0x73, 0x00, 0xAB),
    Rgb(0xD9, 0x4F, 0x4F),
    Rgb(0xE0, 0xA0, 0x30),
    Rgb(0x8A, 0x8A, 0x8A),
];

const MODIFIER_CHOICES: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

/// Special actions offered in Settings ▸ Hotkeys and the overview window.
const SPECIAL_ACTIONS: &[&str] = &[
    action_id::PLAY_SELECTED,
    action_id::PLAY_PREVIOUS,
    action_id::PLAY_NEXT,
    action_id::PLAY_RANDOM,
    action_id::PLAY_CURRENT_AGAIN,
    action_id::PLAY_PREVIOUSLY_PLAYED,
    action_id::STOP,
    action_id::PAUSE_RESUME,
    action_id::VOLUME_UP,
    action_id::VOLUME_DOWN,
    action_id::VOLUME_MUTE,
    action_id::MODE_DEFAULT,
    action_id::MODE_SPEAKERS,
    action_id::MODE_MICROPHONE,
    action_id::MODE_SWITCH,
    action_id::CATEGORY_PREV,
    action_id::CATEGORY_NEXT,
    action_id::SOUND_PREV,
    action_id::SOUND_NEXT,
    action_id::RECORD_MIC,
    action_id::RECORD_STOP,
    action_id::HOTKEYS_TOGGLE,
    action_id::AUTO_KEYS_TOGGLE,
    action_id::HOTBAR_PREV_PAGE,
    action_id::HOTBAR_NEXT_PAGE,
];

// ---------------------------------------------------------------- UI state

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SortCol {
    #[default]
    Index,
    Title,
    Duration,
    Hotkey,
    Plays,
    Tag,
    File,
}

/// Per-window UI state that lives in egui's temporary data store.
#[derive(Clone, Default)]
struct UiState {
    sort_col: SortCol,
    sort_desc: bool,
    hotbar_page: usize,
    settings_tab: usize,
    search_id: u64,
    search_focus: bool,
    sidebar_collapsed: bool,
    hotbar_collapsed: bool,
    seek_drag: Option<f32>,
    initialized: bool,
    anim_start: f64,
    entrance: f32,
    last_category: usize,
    category_changed_at: f64,
    last_hotbar_page: usize,
    hotbar_changed_at: f64,
    meter_out: f32,
    meter_mic: f32,
    sidebar_rows: Vec<egui::Rect>,
}

// ------------------------------------------------------------- UI actions

/// Deferred UI action, applied after the frame has been drawn so that no
/// borrow of `AppState` is held while a method mutates it.
enum Act {
    Play(u32, PlayMode),
    Stop,
    Pause,
    Prev,
    Next,
    Random,
    Seek(u64),
    MasterVolume(f32),
    SpeakersVolume(f32),
    MicVolume(f32),
    ApplyEngine,
    RescanDevices,
    PickFiles,
    PickFolder,
    OpenList(PathBuf),
    SaveList,
    SaveListAs,
    ImportSoundpad,
    Quit,
    RemoveSelected,
    RemoveDead,
    RemoveSound(u32),
    NewCategory,
    RenameCategory(usize),
    DeleteCategory(usize),
    SetCategoryColor(usize, Option<Rgb>),
    RandomFromCategory(usize),
    AddToCategory(u32, usize),
    RemoveFromCategory(u32, usize),
    AddToHotbar(usize, u32),
    RemoveHotbarItem(usize, usize),
    SetHotbarColor(usize, usize, Option<Rgb>),
    SetSoundColor(u32, Option<Rgb>),
    Capture(HotkeyTarget, String),
    ClearHotkey(HotkeyTarget),
    ResetHotkeys,
    OpenEditor(Option<u32>),
    EditorSaveAs,
    EditorExport,
    ShowInExplorer(PathBuf),
    OpenFolder(PathBuf),
    NewHotbarPage,
    RemoveHotbarPage(usize),
    SaveConfig,
    ShowMicSetup,
    OpenUrl(&'static str),
    SetAutostart(bool),
    SetAlwaysOnTop(bool),
    PickRecordingDir,
    InstallCable,
}

// -------------------------------------------------------------- primitives

fn entrance_at(uis: &UiState, now: f64, delay: f64) -> f32 {
    ease_out(((now - uis.anim_start - delay) / 0.55) as f32)
}

fn gloss(painter: &egui::Painter, rect: egui::Rect, radius: f32, alpha: u8) {
    let top = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, rect.top() + 1.0),
        egui::pos2(rect.right() - 1.0, rect.top() + rect.height() * 0.48),
    );
    painter.rect_filled(
        top,
        egui::Rounding {
            nw: radius,
            ne: radius,
            sw: 0.0,
            se: 0.0,
        },
        theme::white(alpha),
    );
}

fn tint(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        (color.r() as u32 * alpha as u32 / 255) as u8,
        (color.g() as u32 * alpha as u32 / 255) as u8,
        (color.b() as u32 * alpha as u32 / 255) as u8,
        alpha,
    )
}

fn category_tint(name: &str) -> egui::Color32 {
    let mut h = 0u32;
    for b in name.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    [
        theme::ACCENT,
        theme::GREEN,
        theme::AMBER,
        theme::BLUE,
        theme::RED,
    ][h as usize % 5]
}

fn draw_folder(painter: &egui::Painter, c: egui::Pos2, color: egui::Color32) {
    painter.rect_filled(
        egui::Rect::from_min_max(c + egui::vec2(-5.5, -3.0), c + egui::vec2(-0.5, -1.0)),
        egui::Rounding::same(1.0),
        color,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(c + egui::vec2(-5.5, -1.2), c + egui::vec2(5.5, 4.2)),
        egui::Rounding::same(1.8),
        color,
    );
}

fn ghost_plus(ui: &mut egui::Ui, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    ui.painter().circle_filled(
        rect.center(),
        13.0,
        theme::white((8.0 + hover * 10.0) as u8),
    );
    ui.painter().circle_stroke(
        rect.center(),
        12.5,
        egui::Stroke::new(1.0_f32, theme::HAIRLINE),
    );
    let col = theme::MUTED.lerp_to_gamma(theme::TEXT, hover);
    let s = 4.0;
    let c = rect.center();
    ui.painter().line_segment(
        [c + egui::vec2(-s, 0.0), c + egui::vec2(s, 0.0)],
        egui::Stroke::new(1.5_f32, col),
    );
    ui.painter().line_segment(
        [c + egui::vec2(0.0, -s), c + egui::vec2(0.0, s)],
        egui::Stroke::new(1.5_f32, col),
    );
    resp.on_hover_text(tip)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn label(text: impl Into<String>, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .size(size)
        .color(color)
        .family(egui::FontFamily::Proportional)
}

fn display_label(text: impl Into<String>, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .size(size)
        .color(color)
        .family(egui::FontFamily::Name("display".into()))
}

fn card_highlight(ui: &egui::Ui) {
    let r = ui.max_rect();
    let y = r.top() + 1.0;
    ui.painter().line_segment(
        [
            egui::pos2(r.left() + 12.0, y),
            egui::pos2(r.right() - 12.0, y),
        ],
        egui::Stroke::new(1.0_f32, theme::white(9)),
    );
}

fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, theme::HAIRLINE))
        .rounding(egui::Rounding::same(16.0))
        .inner_margin(egui::Margin::same(0.0))
}

fn truncate_to(text: &str, chars: usize) -> String {
    truncate(text, chars)
}

/// A compact, animated pill button. `accent` makes it the primary action.
fn pill_button(ui: &mut egui::Ui, text: &str, active: bool, accent: bool) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        font.clone(),
        if accent {
            egui::Color32::WHITE
        } else if active {
            theme::TEXT
        } else {
            theme::MUTED
        },
    );
    let desired = egui::vec2(galley.size().x + 26.0, (galley.size().y + 12.0).max(28.0));
    let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let pressed = resp.is_pointer_button_down_on();
    let shrink = if pressed { 1.0 } else { 0.0 };
    let mut rect = rect.shrink(shrink);
    if accent {
        rect = rect.translate(egui::vec2(0.0, -hover * 0.8));
    }
    let fill = if accent {
        theme::ACCENT.lerp_to_gamma(theme::ACCENT_HOVER, hover)
    } else if active {
        theme::SURFACE_3.lerp_to_gamma(theme::SURFACE_3, hover)
    } else {
        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(rect.height() / 2.0), fill);
    if accent {
        theme::glow(
            ui.painter(),
            rect,
            egui::Rounding::same(rect.height() / 2.0),
            theme::ACCENT,
            hover,
        );
        gloss(ui.painter(), rect, rect.height() / 2.0, 10);
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 12.0, rect.top() + 1.0),
                egui::pos2(rect.right() - 12.0, rect.top() + 1.0),
            ],
            egui::Stroke::new(1.0_f32, theme::white(36)),
        );
    }
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        if accent {
            egui::Color32::WHITE
        } else if active || hover > 0.4 {
            theme::TEXT
        } else {
            theme::MUTED
        },
    );
    resp
}

/// Compact microphone activity indicator for the top bar.
fn mic_chip(ui: &mut egui::Ui, level: f32) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(64.0, 28.0), egui::Sense::hover());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let live = level > 0.015;
    ui.painter().rect_filled(
        rect,
        egui::Rounding::same(14.0),
        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover * 0.3),
    );
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(14.0),
        egui::Stroke::new(1.0_f32, theme::HAIRLINE),
    );
    let dot = rect.left_center() + egui::vec2(13.0, 0.0);
    if live {
        theme::glow(
            ui.painter(),
            egui::Rect::from_center_size(dot, egui::vec2(12.0, 12.0)),
            egui::Rounding::same(6.0),
            theme::GREEN,
            0.8,
        );
    }
    ui.painter()
        .circle_filled(dot, 3.5, if live { theme::GREEN } else { theme::FAINT });
    ui.painter().text(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "MIC",
        egui::FontId::proportional(9.0),
        if live { theme::TEXT } else { theme::FAINT },
    );
}

/// Amber-dot call-to-action used when the microphone cable is missing.
fn setup_chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 34.0, 28.0),
        egui::Sense::click(),
    );
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let fill = theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover);
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(14.0), fill);
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(14.0),
        egui::Stroke::new(1.0_f32, theme::HAIRLINE),
    );
    ui.painter().circle_filled(
        rect.left_center() + egui::vec2(13.0, 0.0),
        3.5,
        theme::AMBER,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 23.0, rect.center().y - galley.size().y / 2.0),
        galley,
        if hover > 0.4 {
            theme::TEXT
        } else {
            theme::MUTED
        },
    );
    resp
}

/// A full-width primary/secondary button used at the bottom of the sidebar.
fn wide_button(ui: &mut egui::Ui, text: &str, accent: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let pressed = resp.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink(1.0) } else { rect };
    let fill = if accent {
        theme::ACCENT.lerp_to_gamma(theme::ACCENT_HOVER, hover)
    } else {
        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(11.0), fill);
    if accent {
        theme::glow(
            ui.painter(),
            rect,
            egui::Rounding::same(11.0),
            theme::ACCENT,
            hover * 0.6,
        );
        gloss(ui.painter(), rect, 11.0, 9);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        if accent {
            egui::Color32::WHITE
        } else {
            theme::TEXT
        },
    );
    resp
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopIcon {
    Tts,
    Record,
    Editor,
    Settings,
    Hotkeys,
    Sidebar,
}

/// Hairline icon drawn with vector primitives so it looks the same in every
/// font/DPI and never falls back to an ugly emoji glyph.
fn top_icon(ui: &mut egui::Ui, icon: TopIcon, active: bool, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let pressed = resp.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink(1.0) } else { rect };
    let fill = if active {
        theme::accent_tint(44)
    } else if hover > 0.02 {
        theme::SURFACE_2
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(9.0), fill);
    let color = if active || hover > 0.4 {
        theme::TEXT
    } else {
        theme::MUTED
    };
    let c = rect.center();
    let bg = if active || hover > 0.02 {
        theme::SURFACE_2
    } else {
        theme::BG
    };
    match icon {
        TopIcon::Tts => {
            ui.painter().line_segment(
                [c + egui::vec2(-4.5, -4.0), c + egui::vec2(4.5, -4.0)],
                egui::Stroke::new(1.5_f32, color),
            );
            ui.painter().line_segment(
                [c + egui::vec2(0.0, -4.0), c + egui::vec2(0.0, 5.0)],
                egui::Stroke::new(1.5_f32, color),
            );
        }
        TopIcon::Record => {
            ui.painter()
                .circle_stroke(c, 5.6, egui::Stroke::new(1.4_f32, color));
            ui.painter().circle_filled(
                c,
                if active { 3.4 } else { 2.0 },
                if active { theme::RED } else { color },
            );
        }
        TopIcon::Editor => {
            let u = egui::vec2(0.7071, -0.7071);
            let v = egui::vec2(0.7071, 0.7071);
            let base = c - u * 4.0;
            let neck = c + u * 2.0;
            let tip = c + u * 6.2;
            let hw = 1.7;
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    base + v * hw,
                    neck + v * hw,
                    tip,
                    neck - v * hw,
                    base - v * hw,
                ],
                color,
                egui::Stroke::NONE,
            ));
            ui.painter().line_segment(
                [neck + v * hw, neck - v * hw],
                egui::Stroke::new(1.2_f32, bg),
            );
            ui.painter().line_segment(
                [base + v * 2.1, base - v * 2.1],
                egui::Stroke::new(1.6_f32, color),
            );
        }
        TopIcon::Settings => {
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0 + 0.26;
                let dir = egui::vec2(a.cos(), a.sin());
                ui.painter().line_segment(
                    [c + dir * 5.6, c + dir * 8.2],
                    egui::Stroke::new(2.6_f32, color),
                );
            }
            ui.painter().circle_filled(c, 5.6, bg);
            ui.painter()
                .circle_stroke(c, 5.2, egui::Stroke::new(1.5_f32, color));
            ui.painter().circle_filled(c, 1.8, color);
        }
        TopIcon::Hotkeys => {
            ui.painter().rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(16.0, 11.0)),
                egui::Rounding::same(2.5),
                egui::Stroke::new(1.3_f32, color),
            );
            for x in [-4.0f32, 0.0, 4.0] {
                ui.painter()
                    .circle_filled(c + egui::vec2(x, -2.0), 1.0, color);
            }
            ui.painter().line_segment(
                [c + egui::vec2(-3.0, 2.8), c + egui::vec2(3.0, 2.8)],
                egui::Stroke::new(1.3_f32, color),
            );
        }
        TopIcon::Sidebar => {
            let outer = egui::Rect::from_center_size(c, egui::vec2(16.0, 12.0));
            ui.painter().rect_filled(
                egui::Rect::from_min_max(outer.min, egui::pos2(outer.left() + 5.0, outer.bottom())),
                egui::Rounding {
                    nw: 2.5,
                    sw: 2.5,
                    ne: 0.0,
                    se: 0.0,
                },
                theme::accent_tint(60),
            );
            ui.painter().rect_stroke(
                outer,
                egui::Rounding::same(2.5),
                egui::Stroke::new(1.3_f32, color),
            );
            ui.painter().line_segment(
                [
                    egui::pos2(outer.left() + 5.0, outer.top()),
                    egui::pos2(outer.left() + 5.0, outer.bottom()),
                ],
                egui::Stroke::new(1.3_f32, color),
            );
        }
    }
    resp.on_hover_text(tooltip)
}

/// Square/round icon button used in the top bar and transport.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TransportIcon {
    Play,
    Pause,
    Stop,
    Previous,
    Next,
    Shuffle,
}

fn halo(
    painter: &egui::Painter,
    rect: egui::Rect,
    base_radius: f32,
    color: egui::Color32,
    strength: f32,
) {
    let a = (strength.clamp(0.0, 1.0) * 20.0) as u8;
    for (spread, f) in [(14.0f32, 0.22f32), (9.0, 0.42), (5.0, 0.7)] {
        painter.rect_filled(
            rect.expand(spread),
            egui::Rounding::same(base_radius + spread),
            tint(color, (a as f32 * f) as u8),
        );
    }
}

fn transport_button(
    ui: &mut egui::Ui,
    icon: TransportIcon,
    size: f32,
    primary: bool,
    live: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let pressed = resp.is_pointer_button_down_on();
    let rect = if pressed { rect.shrink(1.0) } else { rect };

    if primary {
        if live {
            let t = ui.ctx().input(|i| i.time) as f32;
            let pulse = (t * 3.2).sin() * 0.5 + 0.5;
            halo(
                ui.painter(),
                rect,
                size / 2.0,
                theme::ACCENT,
                0.55 + pulse * 0.45,
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        let fill = theme::ACCENT.lerp_to_gamma(theme::ACCENT_HOVER, hover);
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        ui.painter().circle_stroke(
            rect.center(),
            size / 2.0 - 0.5,
            egui::Stroke::new(1.0_f32, theme::white(48)),
        );
        gloss(ui.painter(), rect, size / 2.0, 12);
    } else {
        let fill = if hover > 0.0 {
            theme::SURFACE_3
        } else {
            theme::SURFACE_2
        };
        ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        ui.painter().circle_stroke(
            rect.center(),
            size / 2.0 - 0.5,
            egui::Stroke::new(1.0_f32, theme::HAIRLINE),
        );
    }

    let color = if primary {
        egui::Color32::WHITE
    } else {
        theme::TEXT
    };
    let c = rect.center();
    let s = size * 0.22;
    match icon {
        TransportIcon::Play => {
            let pts = vec![
                c + egui::vec2(-s * 0.40, -s),
                c + egui::vec2(-s * 0.40, s),
                c + egui::vec2(s * 1.10, 0.0),
            ];
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
        }
        TransportIcon::Pause => {
            let w = s * 0.42;
            let h = s * 1.1;
            ui.painter().rect_filled(
                egui::Rect::from_center_size(c + egui::vec2(-w * 0.8, 0.0), egui::vec2(w, h)),
                egui::Rounding::same(1.5),
                color,
            );
            ui.painter().rect_filled(
                egui::Rect::from_center_size(c + egui::vec2(w * 0.8, 0.0), egui::vec2(w, h)),
                egui::Rounding::same(1.5),
                color,
            );
        }
        TransportIcon::Stop => {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(s * 1.35, s * 1.35)),
                egui::Rounding::same(2.5),
                color,
            );
        }
        TransportIcon::Previous | TransportIcon::Next => {
            let dir = if icon == TransportIcon::Previous {
                -1.0
            } else {
                1.0
            };
            let tip = c + egui::vec2(dir * s * 0.85, 0.0);
            let base = -dir * s * 0.35;
            let pts = vec![
                tip,
                c + egui::vec2(base, -s * 0.8),
                c + egui::vec2(base, s * 0.8),
            ];
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
            ui.painter().rect_filled(
                egui::Rect::from_center_size(
                    c + egui::vec2(dir * s * 1.12, 0.0),
                    egui::vec2(s * 0.2, s * 1.6),
                ),
                egui::Rounding::same(1.0),
                color,
            );
        }
        TransportIcon::Shuffle => {
            let a = c + egui::vec2(-6.0, -3.0);
            let b = c + egui::vec2(5.0, 3.0);
            let d = c + egui::vec2(-6.0, 3.0);
            let e = c + egui::vec2(5.0, -3.0);
            let stroke = egui::Stroke::new(1.5_f32, color);
            ui.painter().line_segment([a, b], stroke);
            ui.painter().line_segment([d, e], stroke);
            for (p, dir) in [(b, egui::vec2(-1.0, -1.0)), (e, egui::vec2(-1.0, 1.0))] {
                ui.painter()
                    .line_segment([p, p + dir * 2.2 + egui::vec2(0.0, 2.2)], stroke);
                ui.painter()
                    .line_segment([p, p + dir * 2.2 + egui::vec2(2.2, 0.0)], stroke);
            }
        }
    }
    resp
}

fn draw_search(ui: &mut egui::Ui, state: &mut AppState, uis: &mut UiState) {
    let width = 250.0_f32;
    egui::Frame::none()
        .fill(theme::SURFACE_2)
        .stroke(egui::Stroke::new(1.0_f32, theme::HAIRLINE))
        .rounding(egui::Rounding::same(11.0))
        .inner_margin(egui::Margin::symmetric(12.0, 5.0))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                let ic = icon_rect.center();
                ui.painter().circle_stroke(
                    ic + egui::vec2(-1.5, -1.5),
                    4.5,
                    egui::Stroke::new(1.3_f32, theme::FAINT),
                );
                ui.painter().line_segment(
                    [ic + egui::vec2(2.0, 2.0), ic + egui::vec2(6.0, 6.0)],
                    egui::Stroke::new(1.5_f32, theme::FAINT),
                );
                ui.add_space(4.0);
                let search = egui::Id::new("rustpad-search");
                let hint = state.tr("Search sounds…", "Поиск звуков…");
                let resp = ui.add_sized(
                    [width - 52.0, 22.0],
                    egui::TextEdit::singleline(&mut state.search)
                        .id(search)
                        .frame(false)
                        .hint_text(hint),
                );
                uis.search_id = search.value();
                if std::mem::take(&mut uis.search_focus) {
                    resp.request_focus();
                }
                if !state.search.is_empty() {
                    if ui
                        .add(egui::Button::new("×").frame(false).small())
                        .on_hover_text(state.tr("Clear", "Очистить"))
                        .clicked()
                    {
                        state.search.clear();
                    }
                }
            });
        });
}

// ------------------------------------------------------------------ entry

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let ui_id = egui::Id::new("rustpad-ui");
    let mut uis: UiState = ctx.data_mut(|d| d.get_temp_mut_or_default::<UiState>(ui_id).clone());
    let mut acts: Vec<Act> = Vec::new();
    let now = ctx.input(|i| i.time);
    if uis.anim_start == 0.0 {
        uis.anim_start = now;
        uis.last_category = state.selected_category;
        uis.last_hotbar_page = uis.hotbar_page;
        uis.meter_out = state.out_level;
        uis.meter_mic = state.mic_level;
    }
    uis.entrance = ease_out(((now - uis.anim_start) / 0.55) as f32);
    if uis.last_category != state.selected_category {
        uis.last_category = state.selected_category;
        uis.category_changed_at = now;
    }
    if uis.last_hotbar_page != uis.hotbar_page {
        uis.last_hotbar_page = uis.hotbar_page;
        uis.hotbar_changed_at = now;
    }

    if !uis.initialized {
        uis.initialized = true;
        crate::theme::apply_preference(ctx, state.cfg.theme);
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            if state.cfg.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            },
        ));
    }

    handle_close_request(state, ctx);
    handle_shortcuts(state, ctx, &mut uis);
    handle_dropped_files(state, ctx);

    egui::TopBottomPanel::top("rustpad-topbar")
        .exact_height(62.0)
        .frame(
            egui::Frame::none()
                .fill(theme::BG)
                .stroke(egui::Stroke::new(1.0_f32, theme::HAIRLINE))
                .inner_margin(egui::Margin::symmetric(18.0, 9.0)),
        )
        .show(ctx, |ui| draw_topbar(state, ui, &mut uis, &mut acts));

    egui::TopBottomPanel::bottom("rustpad-dock")
        .resizable(false)
        .exact_height(if uis.hotbar_collapsed { 128.0 } else { 292.0 })
        .frame(
            egui::Frame::none()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(18.0, 0.0)),
        )
        .show(ctx, |ui| {
            let now = ui.ctx().input(|i| i.time);
            let entrance = entrance_at(&uis, now, 0.14);
            ui.multiply_opacity(entrance);
            ui.add_space((1.0 - entrance) * 14.0);
            draw_transport(state, ui, &mut uis, &mut acts);
            if !uis.hotbar_collapsed {
                ui.add_space(8.0);
                draw_hotbar(state, ui, &mut uis, &mut acts);
            }
        });

    if !uis.sidebar_collapsed {
        egui::SidePanel::left("rustpad-categories")
            .resizable(false)
            .exact_width(238.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin {
                        left: 18.0,
                        right: 10.0,
                        top: 8.0,
                        bottom: 12.0,
                    }),
            )
            .show(ctx, |ui| draw_sidebar(state, ui, &mut uis, &mut acts));
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(theme::BG)
                .inner_margin(egui::Margin {
                    left: 8.0,
                    right: 18.0,
                    top: 8.0,
                    bottom: 12.0,
                }),
        )
        .show(ctx, |ui| draw_center(state, ui, &mut uis, &mut acts));

    draw_settings(state, ctx, &mut uis, &mut acts);
    draw_tts_window(state, ctx);
    draw_recorder_window(state, ctx, &mut acts);
    draw_editor_window(state, ctx, &mut acts);
    draw_hotkeys_window(state, ctx, &mut acts);
    draw_about_window(state, ctx);
    draw_rename_category(state, ctx);
    draw_hotkey_capture(state, ctx);
    draw_mic_setup(state, ctx, &mut acts);
    draw_drop_overlay(state, ctx);
    draw_toasts(state, ctx);

    apply_actions(state, ctx, acts);

    let animating = (now - uis.anim_start) < 0.82
        || (now - uis.category_changed_at) < 0.30
        || (now - uis.hotbar_changed_at) < 0.24;
    if animating || ctx.input(|i| i.pointer.has_pointer()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    ctx.data_mut(|d| d.insert_temp(ui_id, uis));
}

// -------------------------------------------------------------- menu bar

fn draw_topbar(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    ui.multiply_opacity(uis.entrance);
    let bar = ui.max_rect();
    ui.painter().circle_filled(
        egui::pos2(bar.left() + 26.0, bar.top() - 26.0),
        110.0,
        theme::accent_tint(12),
    );
    ui.painter().line_segment(
        [
            egui::pos2(bar.left() + 18.0, bar.bottom() - 0.5),
            egui::pos2(bar.right() - 18.0, bar.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0_f32, theme::white(9)),
    );
    ui.horizontal_centered(|ui| {
        draw_logo(ui, state);
        ui.add_space(4.0);
        let side_tip = state.tr(
            "Show or hide the categories",
            "Показать или скрыть категории",
        );
        if top_icon(ui, TopIcon::Sidebar, !uis.sidebar_collapsed, &side_tip).clicked() {
            uis.sidebar_collapsed = !uis.sidebar_collapsed;
        }
        ui.add_space(2.0);
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(9.0, 6.0);
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
            draw_menu_buttons(state, ui, uis, acts);
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let settings_tip = state.tr("Settings", "Настройки");
            if top_icon(ui, TopIcon::Settings, state.show_settings, &settings_tip).clicked() {
                state.show_settings = !state.show_settings;
            }
            let editor_tip = state.tr("Editor", "Редактор");
            if top_icon(ui, TopIcon::Editor, state.show_editor, &editor_tip).clicked() {
                state.show_editor = !state.show_editor;
                if state.show_editor && state.editor.is_none() {
                    acts.push(Act::OpenEditor(None));
                }
            }
            let rec_tip = state.tr("Recorder", "Запись");
            if top_icon(ui, TopIcon::Record, state.show_recorder, &rec_tip).clicked() {
                state.show_recorder = !state.show_recorder;
            }
            let tts_tip = state.tr("Text to speech", "Синтез речи");
            if top_icon(ui, TopIcon::Tts, state.show_tts, &tts_tip).clicked() {
                state.show_tts = !state.show_tts;
            }
            let hk_tip = state.tr("Hotkey overview", "Обзор горячих клавиш");
            if top_icon(ui, TopIcon::Hotkeys, state.show_hotkeys, &hk_tip).clicked() {
                state.show_hotkeys = !state.show_hotkeys;
            }
            ui.add_space(6.0);
            draw_search(ui, state, uis);
            if state.cfg.mic_passthrough && state.cfg.mic_output_device.is_some() {
                ui.add_space(4.0);
                mic_chip(ui, state.mic_level);
            }
            if state.cfg.mic_passthrough && state.cfg.mic_output_device.is_none() {
                ui.add_space(4.0);
                let chip = state.tr("Set up mic", "Настроить микрофон");
                if setup_chip(ui, &chip).clicked() {
                    acts.push(Act::ShowMicSetup);
                }
            }
        });
    });
}

fn draw_logo(ui: &mut egui::Ui, state: &AppState) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    let c = rect.center();
    ui.painter().circle_filled(c, 14.0, theme::ACCENT);
    ui.painter()
        .circle_stroke(c, 14.0, egui::Stroke::new(1.0_f32, theme::white(60)));
    let t = ui.ctx().input(|i| i.time) as f32;
    for (i, h) in [7.0_f32, 13.0, 5.0].iter().enumerate() {
        let h = *h + (t * 2.6 + i as f32 * 1.4).sin() * 1.7;
        let x = c.x - 5.0 + i as f32 * 5.0;
        ui.painter().rect_filled(
            egui::Rect::from_center_size(egui::pos2(x, c.y), egui::vec2(2.4, h)),
            egui::Rounding::same(1.2),
            egui::Color32::WHITE,
        );
    }
    ui.add_space(8.0);
    ui.label(display_label("RustPad", 18.0, theme::TEXT));
    if state.dirty {
        ui.add_space(4.0);
        let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, theme::AMBER);
    }
}

fn draw_menu_buttons(
    state: &mut AppState,
    ui: &mut egui::Ui,
    uis: &mut UiState,
    acts: &mut Vec<Act>,
) {
    egui::menu::menu_custom_button(
        ui,
        egui::Button::new(state.tr("File", "Файл")).frame(false),
        |ui| {
            if ui
                .button(state.tr("Add sound files…", "Добавить звуковые файлы…"))
                .clicked()
            {
                ui.close_menu();
                acts.push(Act::PickFiles);
            }
            if ui
                .button(state.tr("Add folder…", "Добавить папку…"))
                .clicked()
            {
                ui.close_menu();
                acts.push(Act::PickFolder);
            }
            ui.separator();
            if ui
                .button(state.tr("Open sound list… (Ctrl+O)", "Открыть список… (Ctrl+O)"))
                .clicked()
            {
                ui.close_menu();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Soundpad list", &["spl"])
                    .pick_file()
                {
                    acts.push(Act::OpenList(path));
                }
            }
            ui.separator();
            let save_label = state.tr("Save sound list (Ctrl+S)", "Сохранить список (Ctrl+S)");
            if ui.button(save_label).clicked() {
                ui.close_menu();
                acts.push(Act::SaveList);
            }
            if ui.button(state.tr("Save as…", "Сохранить как…")).clicked() {
                ui.close_menu();
                acts.push(Act::SaveListAs);
            }
            ui.separator();
            if ui
                .button(state.tr(
                    "Import Soundpad settings",
                    "Импортировать настройки Soundpad",
                ))
                .clicked()
            {
                ui.close_menu();
                acts.push(Act::ImportSoundpad);
            }
            ui.separator();
            if ui.button(state.tr("Exit", "Выход")).clicked() {
                ui.close_menu();
                acts.push(Act::Quit);
            }
        },
    );

    egui::menu::menu_custom_button(
        ui,
        egui::Button::new(state.tr("Edit", "Правка")).frame(false),
        |ui| {
            let remove = state.tr("Remove selected", "Удалить выбранное");
            if ui
                .add_enabled(state.selected_sound.is_some(), egui::Button::new(remove))
                .clicked()
            {
                ui.close_menu();
                acts.push(Act::RemoveSelected);
            }
            if ui
                .button(state.tr("Find dead entries", "Найти отсутствующие файлы"))
                .clicked()
            {
                ui.close_menu();
                acts.push(Act::RemoveDead);
            }
            ui.separator();
            if ui
                .button(state.tr("Select all sounds", "Выбрать все звуки"))
                .clicked()
            {
                ui.close_menu();
                state.selected_category = state
                    .list
                    .categories
                    .iter()
                    .position(|c| c.is_all())
                    .unwrap_or(state.selected_category);
            }
        },
    );

    egui::menu::menu_custom_button(
        ui,
        egui::Button::new(state.tr("View", "Вид")).frame(false),
        |ui| {
            let l_settings = state.tr("Settings", "Настройки");
            let l_tts = state.tr("Text to speech", "Синтез речи");
            let l_recorder = state.tr("Recorder", "Запись");
            let l_editor = state.tr("Editor", "Редактор");
            let l_hotkeys = state.tr("Hotkey overview", "Обзор горячих клавиш");
            let l_about = state.tr("About", "О программе");
            ui.checkbox(&mut state.show_settings, l_settings);
            ui.checkbox(&mut state.show_tts, l_tts);
            ui.checkbox(&mut state.show_recorder, l_recorder);
            ui.checkbox(&mut state.show_editor, l_editor);
            if state.show_editor && state.editor.is_none() {
                acts.push(Act::OpenEditor(None));
            }
            ui.checkbox(&mut state.show_hotkeys, l_hotkeys);
            ui.checkbox(&mut state.show_about, l_about);
            ui.separator();
            let mut sidebar = !uis.sidebar_collapsed;
            if ui
                .checkbox(
                    &mut sidebar,
                    state.tr("Category sidebar", "Панель категорий"),
                )
                .changed()
            {
                uis.sidebar_collapsed = !sidebar;
            }
            let mut hotbar = !uis.hotbar_collapsed;
            if ui
                .checkbox(&mut hotbar, state.tr("Hotbar", "Хотбар"))
                .changed()
            {
                uis.hotbar_collapsed = !hotbar;
            }
            ui.separator();
            let mut on_top = state.cfg.always_on_top;
            if ui
                .checkbox(&mut on_top, state.tr("Always on top", "Поверх всех окон"))
                .changed()
            {
                acts.push(Act::SetAlwaysOnTop(on_top));
            }
        },
    );

    egui::menu::menu_custom_button(
        ui,
        egui::Button::new(state.tr("Help", "Справка")).frame(false),
        |ui| {
            if ui.button(state.tr("About", "О программе")).clicked() {
                ui.close_menu();
                state.show_about = true;
            }
            if ui
                .button(state.tr("Keyboard shortcuts", "Горячие клавиши"))
                .clicked()
            {
                ui.close_menu();
                state.show_hotkeys = true;
            }
        },
    );
}

// ---------------------------------------------------------------- toolbar

fn play_mode_name(state: &AppState, mode: PlayMode) -> String {
    match mode {
        PlayMode::Default => state.tr("Default", "По умолчанию"),
        PlayMode::Speakers => state.tr("Speakers", "Динамики"),
        PlayMode::Microphone => state.tr("Microphone", "Микрофон"),
        PlayMode::Both => state.tr("Speakers + Microphone", "Динамики + микрофон"),
    }
}

// ---------------------------------------------------------------- sidebar

fn draw_sidebar(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    let now = ui.ctx().input(|i| i.time);
    let entrance = entrance_at(uis, now, 0.04);
    ui.multiply_opacity(entrance);
    ui.add_space((1.0 - entrance) * 10.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(display_label(
                state.tr("Library", "Библиотека"),
                20.0,
                theme::TEXT,
            ));
            let n = state.list.categories.len();
            ui.label(label(
                format!("{} {}", n, state.tr("collections", "коллекций")),
                11.0,
                theme::FAINT,
            ));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let new_tip = state.tr("New category", "Новая категория");
            if ghost_plus(ui, &new_tip).clicked() {
                acts.push(Act::NewCategory);
            }
        });
    });
    ui.add_space(10.0);

    let footer_h = 96.0;
    let scroll_h = (ui.available_height() - footer_h).max(120.0);
    egui::ScrollArea::vertical()
        .max_height(scroll_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let count = state.list.categories.len();
            // One animated selection pill slides between rows instead of
            // hard-switching the background on every click.
            let sel_anim = ui.ctx().animate_value_with_time(
                egui::Id::new("rustpad-sidebar-selection"),
                state.selected_category as f32,
                0.32,
            );
            let row_left = ui.cursor().left();
            let row_width = ui.available_width();
            if uis.sidebar_rows.len() == count {
                let a = (sel_anim.floor() as usize).min(count.saturating_sub(1));
                let b = (a + 1).min(count.saturating_sub(1));
                let t = (sel_anim - a as f32).clamp(0.0, 1.0);
                let ya = uis.sidebar_rows.get(a).map(|r| r.top()).unwrap_or(0.0);
                let yb = uis.sidebar_rows.get(b).map(|r| r.top()).unwrap_or(ya);
                let row_top = ya + (yb - ya) * t;
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(row_left, row_top + 1.0),
                        egui::vec2(row_width, 38.0),
                    ),
                    egui::Rounding::same(10.0),
                    theme::accent_tint(32),
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(row_left, row_top + 10.0),
                        egui::vec2(3.0, 20.0),
                    ),
                    egui::Rounding::same(2.0),
                    theme::ACCENT,
                );
            }
            let mut new_rows = Vec::with_capacity(count);
            for i in 0..count {
                let (name, is_all) = state
                    .list
                    .categories
                    .get(i)
                    .map(|c| (c.name.clone(), c.is_all()))
                    .unwrap_or_default();
                let name = if is_all && name.trim().is_empty() {
                    state.tr("All sounds", "Все звуки").to_string()
                } else {
                    name
                };
                let color = state.list.categories.get(i).and_then(|c| c.color);
                let sounds = state.list.sounds_for_category(i).len();
                let selected = state.selected_category == i;

                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 40.0),
                    egui::Sense::click(),
                );
                new_rows.push(rect);
                let hover = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                if hover > 0.01 && !selected {
                    ui.painter().rect_filled(
                        rect.shrink2(egui::vec2(0.0, 1.0)),
                        egui::Rounding::same(10.0),
                        theme::white((hover * 7.0) as u8),
                    );
                }

                let dot = color
                    .map(app::rgb_to_egui)
                    .unwrap_or_else(|| category_tint(&name));
                let avatar = rect.left_center() + egui::vec2(16.0, 0.0);
                ui.painter()
                    .circle_filled(avatar, 10.0, tint(dot, if selected { 60 } else { 46 }));
                if selected {
                    ui.painter().circle_stroke(
                        avatar,
                        10.0,
                        egui::Stroke::new(1.0_f32, tint(dot, 180)),
                    );
                }
                draw_folder(
                    ui.painter(),
                    avatar,
                    if selected { theme::TEXT } else { dot },
                );
                ui.painter().text(
                    rect.left_center() + egui::vec2(34.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    truncate_to(&name, 19),
                    egui::FontId::proportional(13.0),
                    if selected { theme::TEXT } else { theme::MUTED },
                );
                ui.painter().text(
                    rect.right_center() - egui::vec2(4.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    sounds.to_string(),
                    egui::FontId::proportional(11.0),
                    if selected { theme::TEXT } else { theme::FAINT },
                );

                if resp.clicked() {
                    state.selected_category = i;
                    state.cfg_dirty();
                }

                resp.context_menu(|ui| {
                    if ui
                        .button(state.tr("Play random sound", "Играть случайный звук"))
                        .clicked()
                    {
                        ui.close_menu();
                        acts.push(Act::RandomFromCategory(i));
                    }
                    if !is_all
                        && ui
                            .button(state.tr("Set hotkey…", "Назначить клавишу…"))
                            .clicked()
                    {
                        ui.close_menu();
                        acts.push(Act::Capture(HotkeyTarget::Category(i), name.clone()));
                    }
                    ui.menu_button(state.tr("Color", "Цвет"), |ui| {
                        for c in PRESET_COLORS {
                            let text = format!("  {}", c.to_hex());
                            if ui
                                .button(
                                    egui::RichText::new(text).background_color(app::rgb_to_egui(c)),
                                )
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::SetCategoryColor(i, Some(c)));
                            }
                        }
                        if ui.button(state.tr("No color", "Без цвета")).clicked() {
                            ui.close_menu();
                            acts.push(Act::SetCategoryColor(i, None));
                        }
                    });
                    ui.separator();
                    if !is_all && ui.button(state.tr("Rename…", "Переименовать…")).clicked()
                    {
                        ui.close_menu();
                        acts.push(Act::RenameCategory(i));
                    }
                    if !is_all
                        && ui
                            .button(state.tr("Remove category", "Удалить категорию"))
                            .clicked()
                    {
                        ui.close_menu();
                        acts.push(Act::DeleteCategory(i));
                    }
                });
                ui.add_space(2.0);
            }
            uis.sidebar_rows = new_rows;
        });

    ui.add_space(8.0);
    let add = state.tr("Add sounds", "Добавить звуки");
    if wide_button(ui, &add, true).clicked() {
        acts.push(Act::PickFiles);
    }
    ui.add_space(6.0);
    let configured = state.cfg.mic_output_device.is_some();
    let mic_name = state
        .cfg
        .mic_output_device
        .clone()
        .map(|n| truncate_to(&n, 18))
        .unwrap_or_else(|| {
            state
                .tr("Microphone not set up", "Микрофон не настроен")
                .to_string()
        });
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    ui.painter().rect_filled(
        rect,
        egui::Rounding::same(9.0),
        theme::white((5.0 + hover * 7.0) as u8),
    );
    let dot = rect.left_center() + egui::vec2(12.0, 0.0);
    ui.painter().circle_filled(
        dot,
        3.2,
        if configured {
            theme::GREEN
        } else {
            theme::AMBER
        },
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(22.0, 0.0),
        egui::Align2::LEFT_CENTER,
        mic_name,
        egui::FontId::proportional(10.5),
        theme::FAINT,
    );
    ui.painter().text(
        rect.right_center() - egui::vec2(8.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        "›",
        egui::FontId::proportional(14.0),
        theme::FAINT,
    );
    if resp.clicked() {
        acts.push(Act::ShowMicSetup);
    }
}

// ----------------------------------------------------------------- center

fn draw_center(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    let vis = ui.visuals().clone();
    let now = ui.ctx().input(|i| i.time);
    let cat_t = ease_out(((now - uis.category_changed_at) / 0.26) as f32);
    let entrance = entrance_at(uis, now, 0.08);
    ui.multiply_opacity(entrance * (0.35 + 0.65 * cat_t));
    ui.add_space((1.0 - entrance) * 10.0 + (1.0 - cat_t) * 6.0);

    let (cat_name, is_all) = state
        .list
        .categories
        .get(state.selected_category)
        .map(|c| (c.name.clone(), c.is_all()))
        .unwrap_or_default();
    let cat_title = if is_all && cat_name.trim().is_empty() {
        state.tr("All sounds", "Все звуки").to_string()
    } else {
        cat_name
    };
    let visible = state.visible_ids().len();
    let total = state.list.sounds.len();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(display_label(&cat_title, 24.0, theme::TEXT));
            ui.label(label(
                format!(
                    "{visible} {} · {total} {}",
                    state.tr("sounds", "звуков"),
                    state.tr("total", "всего")
                ),
                12.0,
                theme::FAINT,
            ));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let add = state.tr("Add sounds", "Добавить звуки");
            if pill_button(ui, &add, false, true).clicked() {
                acts.push(Act::PickFiles);
            }
            ui.add_space(4.0);
            let folder = state.tr("Add folder", "Добавить папку");
            if pill_button(ui, &folder, false, false).clicked() {
                acts.push(Act::PickFolder);
            }
            if state.dirty {
                ui.add_space(6.0);
                ui.label(label(
                    state.tr("Unsaved", "Не сохранено"),
                    11.0,
                    theme::AMBER,
                ));
            }
        });
    });
    ui.add_space(10.0);

    let mut ids = state.visible_ids();
    sort_ids(state, &mut ids, uis.sort_col, uis.sort_desc);

    if ids.is_empty() {
        card_frame()
            .inner_margin(egui::Margin::symmetric(24.0, 40.0))
            .show(ui, |ui| {
                card_highlight(ui);
                ui.vertical_centered(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 30.0, theme::accent_tint(28));
                    ui.painter().circle_stroke(
                        rect.center(),
                        30.0,
                        egui::Stroke::new(1.0_f32, theme::accent_tint(70)),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "♪",
                        egui::FontId::proportional(28.0),
                        theme::ACCENT,
                    );
                    ui.add_space(14.0);
                    ui.label(display_label(
                        state.tr("Nothing here yet", "Здесь пока пусто"),
                        24.0,
                        theme::TEXT,
                    ));
                    ui.add_space(4.0);
                    ui.label(label(
                        state.tr(
                            "Drag audio files into the window, or add them from disk.",
                            "Перетащите аудиофайлы в окно или добавьте их с диска.",
                        ),
                        13.0,
                        theme::MUTED,
                    ));
                    ui.add_space(16.0);
                    let add = state.tr("Add sound files…", "Добавить звуковые файлы…");
                    if pill_button(ui, &add, false, true).clicked() {
                        acts.push(Act::PickFiles);
                    }
                });
            });
        return;
    }

    let selected = state.selected_sound;
    let playing = state.status.sound_id;
    let mut clicked: Option<u32> = None;
    let mut secondary: Option<u32> = None;

    card_frame()
        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        .show(ui, |ui| {
            card_highlight(ui);
            TableBuilder::new(ui)
                .id_salt("rustpad-sounds")
                .striped(true)
                .resizable(false)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(34.0))
                .column(Column::exact(36.0))
                .column(Column::initial(250.0).at_least(120.0).clip(true))
                .column(Column::exact(62.0))
                .column(Column::initial(86.0).at_least(58.0).clip(true))
                .column(Column::exact(52.0))
                .column(Column::initial(84.0).at_least(56.0).clip(true))
                .column(Column::remainder().at_least(80.0).clip(true))
                .header(32.0, |mut header| {
                    header.col(|ui| header_cell(ui, uis, SortCol::Index, state.tr("#", "№")));
                    header.col(|_ui| {});
                    header.col(|ui| {
                        header_cell(ui, uis, SortCol::Title, state.tr("Title", "Название"))
                    });
                    header.col(|ui| {
                        header_cell(ui, uis, SortCol::Duration, state.tr("Duration", "Время"))
                    });
                    header.col(|ui| {
                        header_cell(ui, uis, SortCol::Hotkey, state.tr("Hotkey", "Клавиша"))
                    });
                    header.col(|ui| {
                        header_cell(ui, uis, SortCol::Plays, state.tr("Plays", "Проигр."))
                    });
                    header.col(|ui| header_cell(ui, uis, SortCol::Tag, state.tr("Tag", "Метка")));
                    header.col(|ui| header_cell(ui, uis, SortCol::File, state.tr("File", "Файл")));
                })
                .body(|body| {
                    body.rows(36.0, ids.len(), |mut row| {
                        let index = row.index();
                        let Some(id) = ids.get(index).copied() else {
                            return;
                        };
                        let Some(sound) = state.list.find(id) else {
                            return;
                        };
                        let exists = sound.path.is_file();
                        row.set_selected(selected == Some(id));

                        let title = if exists {
                            sound.title.clone()
                        } else {
                            format!("⚠ {}", sound.title)
                        };
                        let title_color = if !exists {
                            vis.error_fg_color
                        } else if let Some(c) = sound.color {
                            app::rgb_to_egui(c)
                        } else {
                            vis.text_color()
                        };
                        let path = sound.path.display().to_string();
                        let duration = format_duration(sound.duration_ms);
                        let hotkey = app::hotkey_label(sound.hotkey);
                        let plays = state.stats.play_count(&sound.hash);
                        let tag = sound.tag.clone().unwrap_or_default();
                        let file = sound.file_name();
                        let is_playing = playing == Some(id);
                        let cover = sound.color;

                        row.col(|ui| {
                            if is_playing {
                                playing_indicator(ui);
                            } else {
                                ui.label(label(format!("{}", index + 1), 12.0, theme::FAINT));
                            }
                        });
                        row.col(|ui| sound_tile(ui, cover, id, is_playing));
                        row.col(|ui| {
                            ui.colored_label(title_color, title.clone())
                                .on_hover_text(path.clone());
                        });
                        row.col(|ui| {
                            ui.label(duration);
                        });
                        row.col(|ui| {
                            ui.label(hotkey);
                        });
                        row.col(|ui| {
                            ui.label(format!("{plays}"));
                        });
                        row.col(|ui| {
                            ui.label(tag);
                        });
                        row.col(|ui| {
                            ui.label(file).on_hover_text(path);
                        });

                        let resp = row.response();
                        if resp.clicked() {
                            clicked = Some(id);
                        } else if resp.secondary_clicked() {
                            secondary = Some(id);
                        }

                        let menu_title = sound.title.clone();
                        let menu_path = sound.path.clone();
                        let mode = state.play_mode;
                        resp.context_menu(|ui| {
                            if ui.button(state.tr("Play", "Играть")).clicked() {
                                ui.close_menu();
                                acts.push(Act::Play(id, mode));
                            }
                            if ui
                                .button(state.tr("Play on speakers", "Играть в динамики"))
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::Play(id, PlayMode::Speakers));
                            }
                            if ui
                                .button(state.tr("Play on microphone", "Играть в микрофон"))
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::Play(id, PlayMode::Microphone));
                            }
                            ui.separator();
                            if ui
                                .button(state.tr("Set hotkey…", "Назначить клавишу…"))
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::Capture(
                                    HotkeyTarget::Sound(id),
                                    menu_title.clone(),
                                ));
                            }
                            ui.menu_button(
                                state.tr("Add to category", "Добавить в категорию"),
                                |ui| {
                                    let cats: Vec<(usize, String)> = state
                                        .list
                                        .categories
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, c)| !c.is_all())
                                        .map(|(i, c)| (i, c.name.clone()))
                                        .collect();
                                    for (ci, cname) in cats {
                                        if ui.button(cname).clicked() {
                                            ui.close_menu();
                                            acts.push(Act::AddToCategory(id, ci));
                                        }
                                    }
                                },
                            );
                            let current_cat_all = state
                                .list
                                .categories
                                .get(state.selected_category)
                                .is_none_or(|c| c.is_all());
                            if !current_cat_all
                                && ui
                                    .button(state.tr(
                                        "Remove from this category",
                                        "Убрать из этой категории",
                                    ))
                                    .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::RemoveFromCategory(id, state.selected_category));
                            }
                            ui.menu_button(
                                state.tr("Add to hotbar page", "Добавить на страницу хотбара"),
                                |ui| {
                                    let pages: Vec<(usize, String)> = state
                                        .list
                                        .hotbar
                                        .iter()
                                        .enumerate()
                                        .map(|(i, p)| (i, hotbar_page_title(state, i, &p.title)))
                                        .collect();
                                    for (pi, pname) in pages {
                                        if ui.button(pname).clicked() {
                                            ui.close_menu();
                                            acts.push(Act::AddToHotbar(pi, id));
                                        }
                                    }
                                },
                            );
                            ui.menu_button(state.tr("Color", "Цвет"), |ui| {
                                for c in PRESET_COLORS {
                                    let text = format!("  {}", c.to_hex());
                                    if ui
                                        .button(
                                            egui::RichText::new(text)
                                                .background_color(app::rgb_to_egui(c)),
                                        )
                                        .clicked()
                                    {
                                        ui.close_menu();
                                        acts.push(Act::SetSoundColor(id, Some(c)));
                                    }
                                }
                                if ui.button(state.tr("No color", "Без цвета")).clicked() {
                                    ui.close_menu();
                                    acts.push(Act::SetSoundColor(id, None));
                                }
                            });
                            ui.separator();
                            if ui
                                .button(state.tr("Edit in editor", "Открыть в редакторе"))
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::OpenEditor(Some(id)));
                            }
                            if ui
                                .button(state.tr("Show in Explorer", "Показать в Проводнике"))
                                .clicked()
                            {
                                ui.close_menu();
                                acts.push(Act::ShowInExplorer(menu_path.clone()));
                            }
                            ui.separator();
                            if ui.button(state.tr("Remove", "Удалить")).clicked() {
                                ui.close_menu();
                                acts.push(Act::RemoveSound(id));
                            }
                        });
                    });
                });
        });

    if let Some(id) = secondary {
        state.selected_sound = Some(id);
    }
    if let Some(id) = clicked {
        state.selected_sound = Some(id);
        state.play_sound(id, state.play_mode);
    }
}

fn sound_tile(ui: &mut egui::Ui, color: Option<Rgb>, id: u32, playing: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    let base = color
        .map(app::rgb_to_egui)
        .unwrap_or_else(|| app::rgb_to_egui(PRESET_COLORS[id as usize % PRESET_COLORS.len()]));
    ui.painter().rect_filled(
        rect.shrink(1.0),
        egui::Rounding::same(8.0),
        base.gamma_multiply(0.28),
    );
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        egui::Rounding::same(8.0),
        egui::Stroke::new(
            1.0_f32,
            if playing {
                theme::ACCENT
            } else {
                base.gamma_multiply(0.55)
            },
        ),
    );
    if playing {
        theme::glow(
            ui.painter(),
            rect,
            egui::Rounding::same(8.0),
            theme::ACCENT,
            0.7,
        );
    }
    for (i, h) in [7.0_f32, 13.0, 9.0].iter().enumerate() {
        let x = rect.center().x - 5.0 + i as f32 * 5.0;
        ui.painter().rect_filled(
            egui::Rect::from_center_size(egui::pos2(x, rect.center().y), egui::vec2(2.2, *h)),
            egui::Rounding::same(1.1),
            base,
        );
    }
}

fn playing_indicator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    let t = ui.ctx().input(|i| i.time) as f32;
    for i in 0..3 {
        let phase = t * 5.0 + i as f32 * 1.9;
        let h = 5.0 + (phase.sin() * 0.5 + 0.5) * 11.0;
        let x = rect.center().x - 5.0 + i as f32 * 5.0;
        ui.painter().rect_filled(
            egui::Rect::from_center_size(egui::pos2(x, rect.center().y), egui::vec2(2.5, h)),
            egui::Rounding::same(1.25),
            theme::ACCENT,
        );
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(40));
}

fn header_cell(ui: &mut egui::Ui, uis: &mut UiState, col: SortCol, label: String) {
    let arrow = if uis.sort_col == col {
        if uis.sort_desc {
            " ↓"
        } else {
            " ↑"
        }
    } else {
        ""
    };
    let active = uis.sort_col == col;
    let text = egui::RichText::new(format!("{}{arrow}", label.to_uppercase()))
        .size(10.5)
        .color(if active { theme::TEXT } else { theme::FAINT });
    if ui.add(egui::Button::new(text).frame(false)).clicked() {
        if uis.sort_col == col {
            uis.sort_desc = !uis.sort_desc;
        } else {
            uis.sort_col = col;
            uis.sort_desc = false;
        }
    }
}

fn sort_ids(state: &AppState, ids: &mut Vec<u32>, col: SortCol, desc: bool) {
    let title = |id: &u32| {
        state
            .list
            .find(*id)
            .map(|s| s.title.to_lowercase())
            .unwrap_or_default()
    };
    match col {
        SortCol::Index => {
            if desc {
                ids.reverse();
            }
            return;
        }
        SortCol::Title => ids.sort_by_key(title),
        SortCol::Duration => ids.sort_by_key(|id| {
            state
                .list
                .find(*id)
                .map(|s| s.duration_ms)
                .unwrap_or_default()
        }),
        SortCol::Hotkey => ids.sort_by_key(|id| {
            state
                .list
                .find(*id)
                .and_then(|s| s.hotkey)
                .map(|h| h.display())
                .unwrap_or_default()
        }),
        SortCol::Plays => ids.sort_by_key(|id| {
            state
                .list
                .find(*id)
                .map(|s| state.stats.play_count(&s.hash))
                .unwrap_or_default()
        }),
        SortCol::Tag => ids.sort_by_key(|id| {
            state
                .list
                .find(*id)
                .and_then(|s| s.tag.clone())
                .unwrap_or_default()
                .to_lowercase()
        }),
        SortCol::File => ids.sort_by_key(|id| {
            state
                .list
                .find(*id)
                .map(|s| s.file_name().to_lowercase())
                .unwrap_or_default()
        }),
    }
    if desc {
        ids.reverse();
    }
}

// ---------------------------------------------------------------- hotbar

fn hotbar_page_title(state: &AppState, index: usize, title: &str) -> String {
    if title.trim().is_empty() {
        format!("{} {}", state.tr("Page", "Страница"), index + 1)
    } else {
        title.to_string()
    }
}

fn page_pill(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 22.0, 26.0),
        egui::Sense::click(),
    );
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let fill = if selected {
        theme::ACCENT
    } else {
        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(8.0), fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        if selected {
            egui::Color32::WHITE
        } else {
            theme::MUTED
        },
    );
    resp
}

fn toggle_chip(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 30.0, 26.0),
        egui::Sense::click(),
    );
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let fill = if on {
        theme::accent_tint(42)
    } else {
        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover)
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(13.0), fill);
    ui.painter().circle_filled(
        rect.left_center() + egui::vec2(13.0, 0.0),
        3.5,
        if on { theme::GREEN } else { theme::FAINT },
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0),
        galley,
        if on { theme::TEXT } else { theme::MUTED },
    );
    resp
}

fn segment_button(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 18.0, 24.0),
        egui::Sense::click(),
    );
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let active = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("active"), selected, 0.14);
    let fill = if active > 0.01 {
        theme::SURFACE_3.gamma_multiply(active)
    } else if hover > 0.4 {
        theme::white(10)
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(7.0), fill);
    if active > 0.5 {
        ui.painter().rect_stroke(
            rect,
            egui::Rounding::same(7.0),
            egui::Stroke::new(1.0_f32, theme::HAIRLINE_STRONG),
        );
    }
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        if selected { theme::TEXT } else { theme::MUTED },
    );
    resp
}

fn volume_slider(ui: &mut egui::Ui, value: f32) -> Option<f32> {
    let width = 92.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click_and_drag());
    let value = value.clamp(0.0, 1.5);
    let t = (value / 1.5).clamp(0.0, 1.0);
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(width, 4.0));
    ui.painter()
        .rect_filled(track, egui::Rounding::same(2.0), theme::white(20));
    let fill = egui::Rect::from_min_size(track.min, egui::vec2(track.width() * t, 4.0));
    ui.painter()
        .rect_filled(fill, egui::Rounding::same(2.0), theme::ACCENT);
    ui.painter().circle_filled(
        egui::pos2(track.left() + track.width() * t, track.center().y),
        5.0,
        egui::Color32::WHITE,
    );
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            return Some(t * 1.5);
        }
    }
    None
}

fn meter(ui: &mut egui::Ui, level: f32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(42.0, 24.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left(), rect.top() + 1.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(8.0),
        theme::FAINT,
    );
    let track = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - 8.0),
        egui::vec2(rect.width(), 4.0),
    );
    ui.painter()
        .rect_filled(track, egui::Rounding::same(2.0), theme::white(18));
    let v = level.clamp(0.0, 1.0);
    let color = if v > 0.97 {
        theme::RED
    } else if v > 0.85 {
        theme::AMBER
    } else {
        theme::GREEN
    };
    ui.painter().rect_filled(
        egui::Rect::from_min_size(track.min, egui::vec2(track.width() * v, 4.0)),
        egui::Rounding::same(2.0),
        color,
    );
}

fn seek_bar(ui: &mut egui::Ui, dur: u64, pos: u64, uis: &mut UiState, acts: &mut Vec<Act>) {
    let dur = dur.max(1);
    let mut value = uis
        .seek_drag
        .unwrap_or(pos.min(dur) as f32)
        .clamp(0.0, dur as f32);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 20.0),
        egui::Sense::click_and_drag(),
    );
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    ui.painter()
        .rect_filled(track, egui::Rounding::same(2.0), theme::white(20));
    let t = (value / dur as f32).clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_size(track.min, egui::vec2(track.width() * t, 4.0));
    ui.painter()
        .rect_filled(fill, egui::Rounding::same(2.0), theme::ACCENT);
    let knob_x = track.left() + track.width() * t;
    ui.painter().circle_filled(
        egui::pos2(knob_x, track.center().y),
        if resp.hovered() || resp.dragged() {
            7.0
        } else {
            5.5
        },
        egui::Color32::WHITE,
    );
    if resp.hovered() || resp.dragged() {
        ui.painter().circle_filled(
            egui::pos2(knob_x, track.center().y),
            12.0,
            theme::accent_tint(40),
        );
    }
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let nt = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
            value = nt * dur as f32;
            uis.seek_drag = Some(value);
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        acts.push(Act::Seek(value as u64));
        uis.seek_drag = None;
    }
}

fn draw_transport(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    let current = state.status.sound_id;
    let paused = state.status.paused;
    let playing = current.is_some() && !paused;
    let title = current
        .and_then(|id| state.list.find(id))
        .map(|s| s.title.clone())
        .unwrap_or_else(|| state.tr("Select a sound", "Выберите звук").to_string());
    let pos = state.status.position_ms;
    let dur = state.status.duration_ms.max(1);
    let voices = state.status.voices;

    card_frame()
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            card_highlight(ui);
            ui.horizontal(|ui| {
                if transport_button(ui, TransportIcon::Previous, 30.0, false, false)
                    .on_hover_text(state.tr("Previous", "Предыдущий"))
                    .clicked()
                {
                    acts.push(Act::Prev);
                }
                let (icon, live) = if playing {
                    (TransportIcon::Pause, true)
                } else {
                    (TransportIcon::Play, current.is_some() && paused)
                };
                if transport_button(ui, icon, 44.0, true, live)
                    .on_hover_text(if playing {
                        state.tr("Pause", "Пауза")
                    } else {
                        state.tr("Play", "Играть")
                    })
                    .clicked()
                {
                    if playing {
                        acts.push(Act::Pause);
                    } else if let Some(id) = state.selected_sound.or(current) {
                        acts.push(Act::Play(id, state.play_mode));
                    } else {
                        state.play_current_again();
                    }
                }
                if transport_button(ui, TransportIcon::Next, 30.0, false, false)
                    .on_hover_text(state.tr("Next", "Следующий"))
                    .clicked()
                {
                    acts.push(Act::Next);
                }
                if transport_button(ui, TransportIcon::Stop, 28.0, false, false)
                    .on_hover_text(state.tr("Stop", "Стоп"))
                    .clicked()
                {
                    acts.push(Act::Stop);
                }
                if transport_button(ui, TransportIcon::Shuffle, 28.0, false, false)
                    .on_hover_text(state.tr("Random sound", "Случайный звук"))
                    .clicked()
                {
                    acts.push(Act::Random);
                }

                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(label(truncate_to(&title, 42), 14.0, theme::TEXT));
                    ui.add_space(1.0);
                    ui.horizontal(|ui| {
                        ui.label(label(
                            format!("{} / {}", format_duration(pos), format_duration(dur)),
                            11.0,
                            theme::FAINT,
                        ));
                        if voices > 1 {
                            ui.label(label(
                                format!("· {} {}", voices, state.tr("voices", "голоса")),
                                11.0,
                                theme::FAINT,
                            ));
                        }
                    });
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let vol = state.cfg.master_volume;
                    if let Some(v) = volume_slider(ui, vol) {
                        acts.push(Act::MasterVolume(v));
                    }
                    ui.label(label(
                        format!("{:.0}%", vol.clamp(0.0, 1.5) / 1.5 * 100.0),
                        10.0,
                        theme::FAINT,
                    ));
                    ui.add_space(4.0);
                    uis.meter_out += (state.out_level - uis.meter_out) * 0.22;
                    uis.meter_mic += (state.mic_level - uis.meter_mic) * 0.22;
                    if (state.out_level - uis.meter_out).abs() > 0.004
                        || (state.mic_level - uis.meter_mic).abs() > 0.004
                    {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    meter(ui, uis.meter_out, "OUT");
                    meter(ui, uis.meter_mic, "MIC");
                    ui.add_space(4.0);

                    egui::Frame::none()
                        .fill(theme::SURFACE_2)
                        .rounding(egui::Rounding::same(9.0))
                        .inner_margin(egui::Margin::same(2.0))
                        .show(ui, |ui| {
                            ui.set_width(190.0);
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 1.0;
                                    for (mode, name) in [
                                        (PlayMode::Default, state.tr("Auto", "Авто")),
                                        (PlayMode::Speakers, state.tr("Sound", "Звук")),
                                        (PlayMode::Microphone, state.tr("Mic", "Мик")),
                                        (PlayMode::Both, state.tr("Both", "Оба")),
                                    ] {
                                        if segment_button(ui, &name, state.play_mode == mode)
                                            .clicked()
                                        {
                                            state.set_play_mode(mode);
                                        }
                                    }
                                },
                            );
                        });
                });
            });

            ui.add_space(2.0);
            seek_bar(ui, dur, pos, uis, acts);
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                let hk_label = state.tr("Hotkeys", "Клавиши");
                if toggle_chip(ui, &hk_label, state.hotkeys_enabled).clicked() {
                    state.hotkeys_enabled = !state.hotkeys_enabled;
                    state.cfg.hotkeys_enabled = state.hotkeys_enabled;
                    state.cfg_dirty();
                }
                let ak_label = state.tr("Auto Keys", "Автоклавиши");
                if toggle_chip(ui, &ak_label, state.auto_keys_enabled).clicked() {
                    let v = !state.auto_keys_enabled;
                    state.set_auto_keys_enabled(v);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if uis.hotbar_collapsed {
                        state.tr("Show hotbar", "Показать хотбар")
                    } else {
                        state.tr("Hide hotbar", "Скрыть хотбар")
                    };
                    if pill_button(ui, &label, false, false).clicked() {
                        uis.hotbar_collapsed = !uis.hotbar_collapsed;
                    }
                });
            });
        });
}

fn draw_hotbar(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    if state.list.hotbar.is_empty() {
        state.new_hotbar_page();
    }

    card_frame()
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            card_highlight(ui);
            ui.horizontal(|ui| {
                ui.label(display_label(
                    state.tr("Hotbar", "Хотбар"),
                    16.0,
                    theme::TEXT,
                ));
                ui.add_space(8.0);
                egui::ScrollArea::horizontal()
                    .id_salt("rustpad-hotbar-pages")
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for pi in 0..state.list.hotbar.len() {
                                let title = state
                                    .list
                                    .hotbar
                                    .get(pi)
                                    .map(|p| p.title.clone())
                                    .unwrap_or_default();
                                let name = hotbar_page_title(state, pi, &title);
                                if page_pill(ui, &name, uis.hotbar_page == pi).clicked() {
                                    uis.hotbar_page = pi;
                                }
                            }
                        });
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let page_now = uis
                        .hotbar_page
                        .min(state.list.hotbar.len().saturating_sub(1));
                    let remove = state.tr("Remove page", "Удалить страницу");
                    if ui
                        .add_enabled(
                            state.list.hotbar.len() > 1,
                            egui::Button::new(remove).frame(false),
                        )
                        .clicked()
                    {
                        acts.push(Act::RemoveHotbarPage(page_now));
                    }
                    let new_page = state.tr("New page", "Новая страница");
                    if ui.add(egui::Button::new(new_page).frame(false)).clicked() {
                        acts.push(Act::NewHotbarPage);
                    }
                    if ui
                        .add_enabled(
                            state.selected_sound.is_some(),
                            egui::Button::new(state.tr("Add selected", "Добавить выбранный"))
                                .frame(false),
                        )
                        .clicked()
                    {
                        if let Some(id) = state.selected_sound {
                            acts.push(Act::AddToHotbar(page_now, id));
                        }
                    }
                });
            });
            ui.add_space(6.0);

            let page = uis
                .hotbar_page
                .min(state.list.hotbar.len().saturating_sub(1));
            uis.hotbar_page = page;

            let items: Vec<(usize, u32, Option<String>, Option<Rgb>, Option<Hotkey>)> = state
                .list
                .hotbar
                .get(page)
                .map(|p| {
                    p.items
                        .iter()
                        .enumerate()
                        .map(|(i, it)| (i, it.sound_id, it.label.clone(), it.color, it.hotkey))
                        .collect()
                })
                .unwrap_or_default();

            if items.is_empty() {
                let gap = 7.0;
                let cols = 6;
                let cell_w =
                    ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).max(56.0);
                ui.horizontal(|ui| {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(cell_w, 46.0), egui::Sense::click());
                    let hover = ui
                        .ctx()
                        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    ui.painter().rect_filled(
                        rect,
                        egui::Rounding::same(11.0),
                        theme::SURFACE_2.lerp_to_gamma(theme::SURFACE_3, hover),
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::Rounding::same(11.0),
                        egui::Stroke::new(1.0_f32, theme::HAIRLINE),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        state.tr("+  Add selected", "+  Добавить выбранный"),
                        egui::FontId::proportional(11.0),
                        if hover > 0.4 {
                            theme::TEXT
                        } else {
                            theme::FAINT
                        },
                    );
                    if resp.clicked() {
                        if let Some(id) = state.selected_sound {
                            acts.push(Act::AddToHotbar(page, id));
                        } else {
                            state.toast(
                                state.tr("Select a sound first.", "Сначала выберите звук."),
                                false,
                            );
                        }
                    }
                });
                ui.add_space(6.0);
                ui.label(label(
                    state.tr(
                        "Hotbar cells stay one click away while you play.",
                        "Ячейки хотбара всегда в одном клике от звука.",
                    ),
                    11.0,
                    theme::FAINT,
                ));
                return;
            }

            let now = ui.ctx().input(|i| i.time);
            let page_t = ease_out(((now - uis.hotbar_changed_at) / 0.24) as f32);
            ui.multiply_opacity(0.35 + 0.65 * page_t);
            ui.add_space((1.0 - page_t) * 8.0);
            let gap = 7.0;
            let cols = 6;
            let cell_w =
                ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).max(56.0);
            egui::ScrollArea::vertical()
                .id_salt("rustpad-hotbar-grid")
                .max_height(132.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for chunk in items.chunks(cols) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for (idx, sound_id, item_label, color, hk) in chunk {
                                let title = item_label
                                    .clone()
                                    .filter(|s| !s.trim().is_empty())
                                    .or_else(|| state.list.find(*sound_id).map(|s| s.title.clone()))
                                    .unwrap_or_else(|| format!("#{sound_id}"));
                                let hk_text = hk
                                    .map(|h| h.display())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(|| "—".to_string());
                                let is_playing = state.status.sound_id == Some(*sound_id)
                                    && !state.status.paused;

                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(cell_w, 46.0),
                                    egui::Sense::click(),
                                );
                                let hover =
                                    ui.ctx()
                                        .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                                let base = color.map(app::rgb_to_egui).unwrap_or(theme::SURFACE_2);
                                let fill = base.lerp_to_gamma(theme::white(22), hover * 0.28);
                                ui.painter().rect_filled(
                                    rect.shrink(0.5),
                                    egui::Rounding::same(11.0),
                                    fill,
                                );
                                ui.painter().rect_stroke(
                                    rect.shrink(0.5),
                                    egui::Rounding::same(11.0),
                                    egui::Stroke::new(
                                        1.0_f32,
                                        if is_playing {
                                            theme::ACCENT
                                        } else {
                                            theme::HAIRLINE
                                        },
                                    ),
                                );
                                if is_playing {
                                    theme::glow(
                                        ui.painter(),
                                        rect,
                                        egui::Rounding::same(11.0),
                                        theme::ACCENT,
                                        0.8,
                                    );
                                }
                                ui.painter().text(
                                    egui::pos2(rect.left() + 11.0, rect.top() + 12.0),
                                    egui::Align2::LEFT_CENTER,
                                    truncate_to(&title, (cell_w / 8.0) as usize),
                                    egui::FontId::proportional(12.0),
                                    if color.is_some() {
                                        egui::Color32::WHITE
                                    } else {
                                        theme::TEXT
                                    },
                                );
                                ui.painter().text(
                                    egui::pos2(rect.left() + 11.0, rect.bottom() - 12.0),
                                    egui::Align2::LEFT_CENTER,
                                    hk_text,
                                    egui::FontId::proportional(9.5),
                                    if color.is_some() {
                                        theme::white(170)
                                    } else {
                                        theme::FAINT
                                    },
                                );

                                if resp.clicked() {
                                    acts.push(Act::Play(*sound_id, state.play_mode));
                                }
                                let menu_title = title.clone();
                                let (page_i, item_i) = (page, *idx);
                                resp.context_menu(|ui| {
                                    if ui
                                        .button(state.tr("Remove from hotbar", "Убрать из хотбара"))
                                        .clicked()
                                    {
                                        ui.close_menu();
                                        acts.push(Act::RemoveHotbarItem(page_i, item_i));
                                    }
                                    if ui
                                        .button(state.tr("Set hotkey…", "Назначить клавишу…"))
                                        .clicked()
                                    {
                                        ui.close_menu();
                                        acts.push(Act::Capture(
                                            HotkeyTarget::Hotbar {
                                                page: page_i,
                                                item: item_i,
                                            },
                                            menu_title.clone(),
                                        ));
                                    }
                                    ui.menu_button(state.tr("Color", "Цвет"), |ui| {
                                        for c in PRESET_COLORS {
                                            let t = format!("  {}", c.to_hex());
                                            if ui
                                                .button(
                                                    egui::RichText::new(t)
                                                        .background_color(app::rgb_to_egui(c)),
                                                )
                                                .clicked()
                                            {
                                                ui.close_menu();
                                                acts.push(Act::SetHotbarColor(
                                                    page_i,
                                                    item_i,
                                                    Some(c),
                                                ));
                                            }
                                        }
                                        if ui.button(state.tr("No color", "Без цвета")).clicked()
                                        {
                                            ui.close_menu();
                                            acts.push(Act::SetHotbarColor(page_i, item_i, None));
                                        }
                                    });
                                });
                            }
                        });
                        ui.add_space(gap);
                    }
                });
        });
}

// -------------------------------------------------------------- settings

fn draw_settings(
    state: &mut AppState,
    ctx: &egui::Context,
    uis: &mut UiState,
    acts: &mut Vec<Act>,
) {
    if !state.show_settings {
        return;
    }
    let mut open = state.show_settings;
    let title = state.tr("Settings", "Настройки");
    let tabs = [
        state.tr("Audio", "Аудио"),
        state.tr("Loudness", "Нормализация"),
        state.tr("Hotkeys", "Клавиши"),
        state.tr("Recorder", "Запись"),
        state.tr("TTS", "Синтез речи"),
        state.tr("Interface", "Интерфейс"),
    ];

    egui::Window::new(title)
        .open(&mut open)
        .default_width(640.0)
        .default_height(560.0)
        .resizable(true)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);
                for (i, tab) in tabs.iter().enumerate() {
                    if ui
                        .selectable_label(uis.settings_tab == i, tab.clone())
                        .clicked()
                    {
                        uis.settings_tab = i;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match uis.settings_tab {
                    0 => settings_audio(state, ui, acts),
                    1 => settings_loudness(state, ui),
                    2 => settings_hotkeys(state, ui, acts),
                    3 => settings_recorder(state, ui, acts),
                    4 => settings_tts(state, ui),
                    _ => settings_interface(state, ui, ctx, acts),
                });
        });
    state.show_settings = open;
}

fn settings_audio(state: &mut AppState, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let default_name = state.tr("System default", "Системное по умолчанию");

    ui.label(egui::RichText::new(state.tr("Output device", "Устройство вывода")).strong());
    let mut out_sel = state.cfg.output_device.clone();
    egui::ComboBox::from_id_salt("settings-out-device")
        .width(330.0)
        .selected_text(out_sel.clone().unwrap_or_else(|| default_name.clone()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(out_sel.is_none(), default_name.clone())
                .clicked()
            {
                out_sel = None;
            }
            for d in &state.out_devices {
                let label = device_label(&d.name, d.is_default);
                if ui
                    .selectable_label(out_sel.as_deref() == Some(d.name.as_str()), label)
                    .clicked()
                {
                    out_sel = Some(d.name.clone());
                }
            }
        });
    if out_sel != state.cfg.output_device {
        state.cfg.output_device = out_sel;
        acts.push(Act::ApplyEngine);
    }

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(state.tr(
            "Microphone output device (virtual cable)",
            "Устройство вывода для микрофона (виртуальный кабель)",
        ))
        .strong(),
    );
    let mut mic_sel = state.cfg.mic_output_device.clone();
    let mic_none_label = state.tr("Not selected", "Не выбрано");
    egui::ComboBox::from_id_salt("settings-mic-out-device")
        .width(330.0)
        .selected_text(mic_sel.clone().unwrap_or_else(|| mic_none_label.clone()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(mic_sel.is_none(), mic_none_label.clone())
                .clicked()
            {
                mic_sel = None;
            }
            for d in &state.out_devices {
                let label = device_label(&d.name, d.is_default);
                if ui
                    .selectable_label(mic_sel.as_deref() == Some(d.name.as_str()), label)
                    .clicked()
                {
                    mic_sel = Some(d.name.clone());
                }
            }
        });
    if mic_sel != state.cfg.mic_output_device {
        state.cfg.mic_output_device = mic_sel;
        acts.push(Act::ApplyEngine);
    }
    if state.cfg.mic_output_device.is_none() {
        let cable_ready = state
            .out_devices
            .iter()
            .any(|d| rustpad_platform::vbcable::looks_like_vbcable(&d.name));
        egui::Frame::none()
            .fill(theme::SURFACE_2)
            .stroke(egui::Stroke::new(1.0_f32, theme::HAIRLINE))
            .rounding(egui::Rounding::same(11.0))
            .inner_margin(egui::Margin::symmetric(12.0, 9.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(label("●", 10.0, theme::AMBER));
                    ui.label(label(
                        state.tr(
                            "No virtual cable selected: other apps cannot hear your microphone or the sounds.",
                            "Виртуальный кабель не выбран: другие программы не слышат ни микрофон, ни звуки.",
                        ),
                        12.0,
                        theme::MUTED,
                    ));
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if !cable_ready {
                        let install =
                            state.tr("Install VB-CABLE automatically", "Установить VB-CABLE автоматически");
                        if pill_button(ui, &install, false, true).clicked() {
                            acts.push(Act::InstallCable);
                        }
                    }
                    let setup = state.tr("How to set up", "Как настроить");
                    if pill_button(ui, &setup, false, false).clicked() {
                        acts.push(Act::ShowMicSetup);
                    }
                });
            });
    } else {
        ui.small(state.tr(
            "Keep the cable's output selected as the microphone in Discord/OBS/game.",
            "В Discord/OBS/игре выберите выход этого кабеля как микрофон.",
        ));
    }

    ui.add_space(6.0);
    ui.label(egui::RichText::new(state.tr("Capture device", "Устройство записи")).strong());
    let mut cap_sel = state.cfg.capture_device.clone();
    egui::ComboBox::from_id_salt("settings-capture-device")
        .width(330.0)
        .selected_text(cap_sel.clone().unwrap_or_else(|| default_name.clone()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(cap_sel.is_none(), default_name.clone())
                .clicked()
            {
                cap_sel = None;
            }
            for d in &state.in_devices {
                let label = device_label(&d.name, d.is_default);
                if ui
                    .selectable_label(cap_sel.as_deref() == Some(d.name.as_str()), label)
                    .clicked()
                {
                    cap_sel = Some(d.name.clone());
                }
            }
        });
    if cap_sel != state.cfg.capture_device {
        state.cfg.capture_device = cap_sel;
        acts.push(Act::ApplyEngine);
    }

    if ui
        .button(state.tr("Rescan devices", "Обновить список устройств"))
        .clicked()
    {
        acts.push(Act::RescanDevices);
    }

    ui.separator();

    let mut spk = state.cfg.speakers_volume.clamp(0.0, 1.5);
    let spk_label = state.tr("Speakers volume", "Громкость динамиков");
    if ui
        .add(egui::Slider::new(&mut spk, 0.0..=1.5).text(spk_label))
        .changed()
    {
        acts.push(Act::SpeakersVolume(spk));
    }
    let mut mic_vol = state.cfg.microphone_volume.clamp(0.0, 1.5);
    let mic_label = state.tr("Microphone volume", "Громкость микрофона");
    if ui
        .add(egui::Slider::new(&mut mic_vol, 0.0..=1.5).text(mic_label))
        .changed()
    {
        acts.push(Act::MicVolume(mic_vol));
    }

    ui.separator();

    let mut passthrough = state.cfg.mic_passthrough;
    let pt_label = state.tr("Microphone passthrough", "Проброс микрофона");
    if ui.checkbox(&mut passthrough, pt_label).changed() {
        state.cfg.mic_passthrough = passthrough;
        acts.push(Act::ApplyEngine);
    }
    ui.add_enabled_ui(state.cfg.mic_passthrough, |ui| {
        let mut gain = state.cfg.mic_passthrough_gain.clamp(0.0, 2.0);
        let gain_label = state.tr("Passthrough gain", "Усиление проброса");
        if ui
            .add(egui::Slider::new(&mut gain, 0.0..=2.0).text(gain_label))
            .changed()
        {
            state.cfg.mic_passthrough_gain = gain;
            acts.push(Act::ApplyEngine);
        }
        ui.small(format!("≈ {:.1} dB", app::gain_to_db(gain)));
        let mut delay = state.cfg.delay_mic_ms.min(2000);
        let delay_label = state.tr("Delay", "Задержка");
        if ui
            .add(
                egui::DragValue::new(&mut delay)
                    .range(0..=2000)
                    .suffix(" ms")
                    .prefix(format!("{delay_label}: ")),
            )
            .changed()
        {
            state.cfg.delay_mic_ms = delay;
            acts.push(Act::ApplyEngine);
        }
        let mut block = state.cfg.block_voice_while_playing;
        let block_label = state.tr(
            "Block the microphone while a sound plays",
            "Заглушать микрофон во время воспроизведения",
        );
        if ui.checkbox(&mut block, block_label).changed() {
            state.cfg.block_voice_while_playing = block;
            acts.push(Act::ApplyEngine);
        }
    });

    ui.separator();

    ui.label(egui::RichText::new(state.tr("Default play mode", "Режим по умолчанию")).strong());
    let mut def_mode = state.cfg.default_play_mode;
    egui::ComboBox::from_id_salt("settings-default-mode")
        .width(240.0)
        .selected_text(play_mode_name(state, def_mode))
        .show_ui(ui, |ui| {
            for m in [PlayMode::Speakers, PlayMode::Microphone, PlayMode::Both] {
                ui.selectable_value(&mut def_mode, m, play_mode_name(state, m));
            }
        });
    if def_mode != state.cfg.default_play_mode {
        state.cfg.default_play_mode = def_mode;
        acts.push(Act::ApplyEngine);
    }

    ui.separator();

    let mut stop_prev = state.cfg.stop_previous_sound;
    let stop_prev_label = state.tr(
        "Stop the previous sound when a new one starts",
        "Останавливать предыдущий звук при запуске нового",
    );
    if ui.checkbox(&mut stop_prev, stop_prev_label).changed() {
        state.cfg.stop_previous_sound = stop_prev;
        state.cfg_dirty();
    }

    ui.separator();

    let mut duck = state.cfg.duck_others;
    let duck_label = state.tr(
        "Reduce the volume of other applications while playing",
        "Приглушать другие приложения во время воспроизведения",
    );
    if ui.checkbox(&mut duck, duck_label).changed() {
        state.cfg.duck_others = duck;
    }
    ui.add_enabled_ui(state.cfg.duck_others, |ui| {
        let mut pct = state.cfg.duck_percent.min(100);
        let pct_label = state.tr("Reduction", "Снижение громкости");
        if ui
            .add(
                egui::Slider::new(&mut pct, 0..=100)
                    .text(pct_label)
                    .suffix(" %"),
            )
            .changed()
        {
            state.cfg.duck_percent = pct;
        }
    });
}

fn device_label(name: &str, is_default: bool) -> String {
    if is_default {
        format!("{name}  ★")
    } else {
        name.to_string()
    }
}

fn settings_loudness(state: &mut AppState, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(state.tr("Normalization", "Нормализация")).strong());
    let labels = [
        state.tr("Off", "Выкл."),
        state.tr("Fixed adjustment", "Фиксированная поправка"),
        state.tr("Dynamic (per sound)", "Динамическая (по звуку)"),
    ];
    let modes = [
        rustpad_core::NormalizeMode::Off,
        rustpad_core::NormalizeMode::Fixed,
        rustpad_core::NormalizeMode::Dynamic,
    ];
    for (i, mode) in modes.iter().enumerate() {
        ui.radio_value(&mut state.cfg.normalize, *mode, labels[i].clone());
    }
    ui.add_space(8.0);
    ui.add_enabled_ui(
        matches!(state.cfg.normalize, rustpad_core::NormalizeMode::Dynamic),
        |ui| {
            let mut target = state.cfg.target_db.clamp(-40.0, 0.0);
            let label = state.tr("Target level", "Целевой уровень");
            if ui
                .add(
                    egui::Slider::new(&mut target, -40.0..=0.0)
                        .text(label)
                        .suffix(" dB"),
                )
                .changed()
            {
                state.cfg.target_db = target;
            }
        },
    );
    ui.add_enabled_ui(
        matches!(state.cfg.normalize, rustpad_core::NormalizeMode::Fixed),
        |ui| {
            let mut adjust = state.cfg.fixed_adjust_db.clamp(-30.0, 30.0);
            let label = state.tr("Fixed adjustment", "Фиксированная поправка");
            if ui
                .add(
                    egui::Slider::new(&mut adjust, -30.0..=30.0)
                        .text(label)
                        .suffix(" dB"),
                )
                .changed()
            {
                state.cfg.fixed_adjust_db = adjust;
            }
        },
    );
    ui.add_space(8.0);
    ui.small(state.tr(
        "Dynamic mode measures the loudness of every imported file and evens out the volume.",
        "Динамический режим измеряет громкость каждого файла и выравнивает её.",
    ));
}

fn settings_hotkeys(state: &mut AppState, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.label(
        egui::RichText::new(state.tr(
            "Index hotkey modifier (Ctrl+1 … Ctrl+0)",
            "Модификатор индексных клавиш (Ctrl+1 … Ctrl+0)",
        ))
        .strong(),
    );
    let mut index_mod = state.cfg.hotkey_index_modifier.clone();
    egui::ComboBox::from_id_salt("settings-index-mod")
        .selected_text(index_mod.clone())
        .width(120.0)
        .show_ui(ui, |ui| {
            for choice in MODIFIER_CHOICES {
                ui.selectable_value(&mut index_mod, choice.to_string(), choice);
            }
        });
    if index_mod != state.cfg.hotkey_index_modifier {
        state.cfg.hotkey_index_modifier = index_mod;
    }

    ui.label(
        egui::RichText::new(state.tr(
            "Numpad hotkey modifier (Alt+Numpad1 …)",
            "Модификатор цифрового блока (Alt+Numpad1 …)",
        ))
        .strong(),
    );
    let mut num_mod = state.cfg.hotkey_numcode_modifier.clone();
    egui::ComboBox::from_id_salt("settings-num-mod")
        .selected_text(num_mod.clone())
        .width(120.0)
        .show_ui(ui, |ui| {
            for choice in MODIFIER_CHOICES {
                ui.selectable_value(&mut num_mod, choice.to_string(), choice);
            }
        });
    if num_mod != state.cfg.hotkey_numcode_modifier {
        state.cfg.hotkey_numcode_modifier = num_mod;
    }

    ui.add_space(6.0);
    let mut routing = state.cfg.category_index_routing;
    let routing_label = state.tr(
        "Category index routing (index keys play inside the selected category)",
        "Маршрутизация по индексу категории (клавиши играют внутри выбранной категории)",
    );
    if ui.checkbox(&mut routing, routing_label).changed() {
        state.cfg.category_index_routing = routing;
    }
    let mut pass = state.cfg.pass_hotkeys;
    let pass_label = state.tr(
        "Pass hotkeys through to other applications",
        "Пропускать горячие клавиши в другие приложения",
    );
    if ui.checkbox(&mut pass, pass_label).changed() {
        state.cfg.pass_hotkeys = pass;
    }
    let mut lr = state.cfg.distinguish_left_right_modifiers;
    let lr_label = state.tr(
        "Distinguish left and right modifier keys",
        "Различать левый и правый модификаторы",
    );
    if ui.checkbox(&mut lr, lr_label).changed() {
        state.cfg.distinguish_left_right_modifiers = lr;
    }

    ui.separator();
    ui.label(egui::RichText::new(state.tr("Special actions", "Специальные действия")).strong());
    let set_label = state.tr("Set", "Назначить");
    let clear_label = state.tr("Clear", "Сбросить");
    egui::ScrollArea::vertical()
        .id_salt("settings-hotkey-list")
        .max_height(300.0)
        .show(ui, |ui| {
            egui::Grid::new("settings-hotkey-grid")
                .num_columns(3)
                .striped(true)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for action in SPECIAL_ACTIONS {
                        let name = action_name(state, action);
                        let hk = state.cfg.hotkey(action);
                        let hk_text = app::hotkey_label(hk);
                        ui.label(name.clone());
                        ui.label(if hk_text.is_empty() {
                            "—".to_string()
                        } else {
                            hk_text
                        });
                        ui.horizontal(|ui| {
                            if ui.small_button(set_label.clone()).clicked() {
                                acts.push(Act::Capture(
                                    HotkeyTarget::Action(action.to_string()),
                                    name.clone(),
                                ));
                            }
                            if ui
                                .add_enabled(
                                    hk.is_some(),
                                    egui::Button::new(clear_label.clone()).small(),
                                )
                                .clicked()
                            {
                                acts.push(Act::ClearHotkey(HotkeyTarget::Action(
                                    action.to_string(),
                                )));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    if ui
        .button(state.tr("Reset all hotkeys", "Сбросить все клавиши"))
        .clicked()
    {
        acts.push(Act::ResetHotkeys);
    }

    ui.separator();
    ui.label(egui::RichText::new(state.tr("Auto Keys", "Автоклавиши")).strong());
    let mut auto = state.cfg.auto_keys_enabled;
    let auto_label = state.tr(
        "Hold a key while a sound plays (simulated push-to-talk)",
        "Удерживать клавишу во время звука (имитация PTT)",
    );
    if ui.checkbox(&mut auto, auto_label).changed() {
        state.cfg.auto_keys_enabled = auto;
        state.auto_keys_enabled = auto;
        if !auto {
            state.stop();
        }
        state.cfg_dirty();
    }
    ui.horizontal(|ui| {
        let hk = if state.cfg.auto_keys_vk == 0 {
            Hotkey::modifier_only(rustpad_core::hotkey::Modifiers(
                state.cfg.auto_keys_modifiers,
            ))
        } else {
            Hotkey::new(
                rustpad_core::hotkey::Modifiers(state.cfg.auto_keys_modifiers),
                state.cfg.auto_keys_vk,
            )
        };
        ui.label(format!(
            "{}: {}",
            state.tr("Key", "Клавиша"),
            if state.cfg.auto_keys_vk == 0 {
                "—".to_string()
            } else {
                hk.display()
            }
        ));
        if ui
            .button(state.tr("Set key…", "Назначить клавишу…"))
            .clicked()
        {
            acts.push(Act::Capture(
                HotkeyTarget::AutoKeys,
                state.tr("Auto Keys", "Автоклавиши").to_string(),
            ));
        }
        if state.cfg.auto_keys_vk != 0 && ui.button(state.tr("Clear", "Сбросить")).clicked()
        {
            acts.push(Act::ClearHotkey(HotkeyTarget::AutoKeys));
        }
    });
    let mut only_hotkey = state.cfg.auto_keys_only_hotkey;
    if ui
        .checkbox(
            &mut only_hotkey,
            state.tr(
                "Only when the sound was started by a hotkey",
                "Только если звук запущен клавишей",
            ),
        )
        .changed()
    {
        state.cfg.auto_keys_only_hotkey = only_hotkey;
        state.cfg_dirty();
    }
    ui.small(state.tr(
        "The key is pressed when playback starts and released when it stops.",
        "Клавиша нажимается при старте воспроизведения и отпускается при остановке.",
    ));
}

fn settings_recorder(state: &mut AppState, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.label(egui::RichText::new(state.tr("Source", "Источник")).strong());
    let mut source = state.cfg.record_source;
    ui.radio_value(
        &mut source,
        rustpad_core::RecordSource::Microphone,
        state.tr("Microphone", "Микрофон"),
    );
    ui.radio_value(
        &mut source,
        rustpad_core::RecordSource::Speakers,
        state.tr("Speakers (what you hear)", "Динамики (что слышно)"),
    );
    if source != state.cfg.record_source {
        state.cfg.record_source = source;
        state.cfg_dirty();
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(state.tr("Recordings folder", "Папка записей")).strong());
    ui.horizontal(|ui| {
        ui.label(state.cfg.recording_dir.display().to_string());
        if ui.button(state.tr("Choose…", "Выбрать…")).clicked() {
            acts.push(Act::PickRecordingDir);
        }
    });
    ui.add_space(6.0);
    let mut max_secs = state.cfg.recording_max_secs.clamp(1, 3600);
    let max_label = state.tr("Maximum length", "Максимальная длительность");
    if ui
        .add(
            egui::DragValue::new(&mut max_secs)
                .range(1..=3600)
                .suffix(" s")
                .prefix(format!("{max_label}: ")),
        )
        .changed()
    {
        state.cfg.recording_max_secs = max_secs;
    }
    let mut trim = state.cfg.trim_silence;
    let trim_label = state.tr(
        "Trim silence after recording",
        "Обрезать тишину после записи",
    );
    if ui.checkbox(&mut trim, trim_label).changed() {
        state.cfg.trim_silence = trim;
    }
    let mut norm = state.cfg.auto_normalize_recording;
    let norm_label = state.tr("Normalize recordings", "Нормализовать записи");
    if ui.checkbox(&mut norm, norm_label).changed() {
        state.cfg.auto_normalize_recording = norm;
    }
    ui.add_space(6.0);
    ui.small(state.tr(
        "Recordings are saved as WAV files into the folder above.",
        "Записи сохраняются в формате WAV в папку выше.",
    ));
}

fn settings_tts(state: &mut AppState, ui: &mut egui::Ui) {
    if state.tts_voices.is_empty() {
        ui.label(state.tr(
            "No system voices were found.",
            "Системные голоса не найдены.",
        ));
        return;
    }
    ui.label(egui::RichText::new(state.tr("Voice", "Голос")).strong());
    let mut sel = state
        .tts_voice
        .clone()
        .or_else(|| state.tts_voices.first().map(|v| v.id.clone()));
    let selected_text = state
        .tts_voices
        .iter()
        .find(|v| Some(&v.id) == sel.as_ref())
        .map(|v| v.name.clone())
        .unwrap_or_else(|| state.tr("Default voice", "Голос по умолчанию"));
    egui::ComboBox::from_id_salt("settings-tts-voice")
        .width(320.0)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for v in &state.tts_voices {
                let label = if v.language.is_empty() {
                    v.name.clone()
                } else {
                    format!("{} ({})", v.name, v.language)
                };
                if ui
                    .selectable_label(sel.as_deref() == Some(v.id.as_str()), label)
                    .clicked()
                {
                    sel = Some(v.id.clone());
                }
            }
        });
    if sel != state.tts_voice {
        state.tts_voice = sel;
        state.cfg.tts_voice = state.tts_voice.clone();
    }
    let rate_label = state.tr("Rate", "Скорость");
    if ui
        .add(egui::Slider::new(&mut state.tts_rate, -10..=10).text(rate_label))
        .changed()
    {
        state.cfg.tts_rate = state.tts_rate;
    }
    let pitch_label = state.tr("Pitch", "Тон");
    if ui
        .add(egui::Slider::new(&mut state.tts_pitch, -10..=10).text(pitch_label))
        .changed()
    {
        state.cfg.tts_pitch = state.tts_pitch;
    }
}

fn settings_interface(
    state: &mut AppState,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    acts: &mut Vec<Act>,
) {
    ui.label(egui::RichText::new(state.tr("Theme", "Тема")).strong());
    let mut theme = state.cfg.theme;
    let theme_before = theme;
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut theme,
            rustpad_core::Theme::Light,
            state.tr("Light", "Светлая"),
        );
        ui.radio_value(
            &mut theme,
            rustpad_core::Theme::Dark,
            state.tr("Dark", "Тёмная"),
        );
        ui.radio_value(
            &mut theme,
            rustpad_core::Theme::System,
            state.tr("System", "Системная"),
        );
    });
    if theme != theme_before {
        state.cfg.theme = theme;
        crate::theme::apply_preference(ctx, theme);
    }

    ui.add_space(6.0);
    ui.label(egui::RichText::new(state.tr("Language", "Язык")).strong());
    let mut ru = state.lang_ru;
    ui.horizontal(|ui| {
        ui.radio_value(&mut ru, false, "English");
        ui.radio_value(&mut ru, true, "Русский");
    });
    if ru != state.lang_ru {
        state.lang_ru = ru;
        state.cfg.language = if ru { "ru".into() } else { "en".into() };
    }

    ui.separator();
    let mut start_min = state.cfg.start_minimized;
    let start_label = state.tr("Start minimized", "Запускать свёрнутым");
    if ui.checkbox(&mut start_min, start_label).changed() {
        state.cfg.start_minimized = start_min;
    }
    let mut min_close = state.cfg.minimize_instead_of_close;
    let min_label = state.tr("Minimize instead of closing", "Сворачивать вместо закрытия");
    if ui.checkbox(&mut min_close, min_label).changed() {
        state.cfg.minimize_instead_of_close = min_close;
    }
    let mut tray = state.cfg.minimize_to_tray;
    let tray_label = state.tr("Minimize to the tray", "Сворачивать в трей");
    if ui.checkbox(&mut tray, tray_label).changed() {
        state.cfg.minimize_to_tray = tray;
    }
    let mut autostart = state.cfg.autostart;
    let autostart_label = state.tr("Start with Windows", "Запускать вместе с Windows");
    if ui.checkbox(&mut autostart, autostart_label).changed() {
        state.cfg.autostart = autostart;
        acts.push(Act::SetAutostart(autostart));
    }
    let mut on_top = state.cfg.always_on_top;
    let top_label = state.tr("Always on top", "Поверх всех окон");
    if ui.checkbox(&mut on_top, top_label).changed() {
        acts.push(Act::SetAlwaysOnTop(on_top));
    }

    ui.separator();
    ui.small(format!(
        "{}: {}",
        state.tr("Sound list", "Список звуков"),
        state.cfg.soundlist_path.display()
    ));
    ui.small(format!(
        "{}: {}",
        state.tr("Configuration", "Конфигурация"),
        state.cfg_path.display()
    ));
    if ui
        .button(state.tr("Save configuration", "Сохранить настройки"))
        .clicked()
    {
        acts.push(Act::SaveConfig);
    }
}

// ------------------------------------------------------------------- TTS

fn draw_tts_window(state: &mut AppState, ctx: &egui::Context) {
    if !state.show_tts {
        return;
    }
    let mut open = state.show_tts;
    let title = state.tr("Text to speech", "Синтез речи");
    let text_label = state.tr("Text", "Текст");
    let voice_label = state.tr("Voice", "Голос");
    let default_voice = state.tr("Default voice", "Голос по умолчанию");
    let rate_label = state.tr("Rate", "Скорость");
    let pitch_label = state.tr("Pitch", "Тон");
    let speakers = state.tr("Speak on speakers", "Озвучить в динамики");
    let mic = state.tr("Speak on microphone", "Озвучить в микрофон");
    let both = state.tr("Speak on both", "Озвучить в оба");
    let generating = state.tr("Generating…", "Синтез…");

    egui::Window::new(title)
        .open(&mut open)
        .default_width(480.0)
        .resizable(true)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.label(text_label);
            ui.add(
                egui::TextEdit::multiline(&mut state.tts_text)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .hint_text("Hello! This is RustPad."),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(voice_label);
                let mut sel = state.tts_voice.clone();
                let selected_text = state
                    .tts_voices
                    .iter()
                    .find(|v| Some(&v.id) == sel.as_ref())
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| default_voice.clone());
                egui::ComboBox::from_id_salt("tts-voice")
                    .width(240.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for v in &state.tts_voices {
                            let label = if v.language.is_empty() {
                                v.name.clone()
                            } else {
                                format!("{} ({})", v.name, v.language)
                            };
                            if ui
                                .selectable_label(sel.as_deref() == Some(v.id.as_str()), label)
                                .clicked()
                            {
                                sel = Some(v.id.clone());
                            }
                        }
                    });
                if sel != state.tts_voice {
                    state.tts_voice = sel;
                    state.cfg.tts_voice = state.tts_voice.clone();
                }
            });
            if ui
                .add(egui::Slider::new(&mut state.tts_rate, -10..=10).text(rate_label))
                .changed()
            {
                state.cfg.tts_rate = state.tts_rate;
            }
            if ui
                .add(egui::Slider::new(&mut state.tts_pitch, -10..=10).text(pitch_label))
                .changed()
            {
                state.cfg.tts_pitch = state.tts_pitch;
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(speakers).clicked() {
                    state.speak_tts(PlayMode::Speakers);
                }
                if ui.button(mic).clicked() {
                    state.speak_tts(PlayMode::Microphone);
                }
                if ui.button(both).clicked() {
                    state.speak_tts(PlayMode::Both);
                }
                if state.tts_generating {
                    ui.spinner();
                    ui.label(generating);
                }
            });
        });
    state.show_tts = open;
}

// -------------------------------------------------------------- recorder

fn draw_recorder_window(state: &mut AppState, ctx: &egui::Context, acts: &mut Vec<Act>) {
    if !state.show_recorder {
        return;
    }
    let mut open = state.show_recorder;
    let title = state.tr("Recorder", "Запись");
    let start = state.tr("Start recording", "Начать запись");
    let stop = state.tr("Stop recording", "Остановить запись");
    let recording = state.tr("Recording…", "Идёт запись…");
    let last = state.tr("Last recording", "Последняя запись");
    let open_folder = state.tr("Open folder", "Открыть папку");
    let none = state.tr("Nothing recorded yet.", "Пока ничего не записано.");
    let note = state.tr(
        "The loudness, folder and length settings live in Settings ▸ Recorder.",
        "Папка, длительность и обработка настраиваются в «Настройки ▸ Запись».",
    );

    egui::Window::new(title)
        .open(&mut open)
        .default_width(430.0)
        .resizable(true)
        .fade_in(true)
        .show(ctx, |ui| {
            let mut source = state.cfg.record_source;
            ui.add_enabled_ui(!state.recording, |ui| {
                ui.horizontal(|ui| {
                    ui.label(state.tr("Source:", "Источник:"));
                    ui.radio_value(
                        &mut source,
                        rustpad_core::RecordSource::Microphone,
                        state.tr("Microphone", "Микрофон"),
                    );
                    ui.radio_value(
                        &mut source,
                        rustpad_core::RecordSource::Speakers,
                        state.tr("Speakers", "Динамики"),
                    );
                });
            });
            if source != state.cfg.record_source {
                state.cfg.record_source = source;
                state.cfg_dirty();
            }
            ui.horizontal(|ui| {
                if state.recording {
                    if ui.button(stop).clicked() {
                        state.stop_recording();
                    }
                    let t = ctx.input(|i| i.time);
                    let alpha = if (t * 2.0) as i64 % 2 == 0 { 255 } else { 80 };
                    ui.colored_label(
                        egui::Color32::from_rgba_unmultiplied(215, 60, 60, alpha as u8),
                        format!("● {recording}"),
                    );
                } else if ui.button(start).clicked() {
                    state.start_recording();
                }
            });
            ui.separator();
            ui.label(egui::RichText::new(last).strong());
            match state.last_recording.clone() {
                Some(path) => {
                    ui.label(path.display().to_string());
                    let dir = path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| state.cfg.recording_dir.clone());
                    if ui.button(open_folder).clicked() {
                        acts.push(Act::OpenFolder(dir));
                    }
                }
                None => {
                    ui.weak(none);
                }
            }
            ui.separator();
            ui.small(note);
        });
    state.show_recorder = open;
}

// ---------------------------------------------------------------- editor

struct EditorLabels {
    title: String,
    name: String,
    duration: String,
    unsaved: String,
    selection: String,
    fade: String,
    speed: String,
    boost: String,
    apply: String,
    crop: String,
    fade_in: String,
    fade_out: String,
    reverse: String,
    normalize: String,
    trim: String,
    karaoke: String,
    undo: String,
    save_as: String,
    add_list: String,
}

impl EditorLabels {
    fn new(state: &AppState) -> Self {
        EditorLabels {
            title: state.tr("Sound editor", "Редактор звука"),
            name: state.tr("Title", "Название"),
            duration: state.tr("Duration", "Время"),
            unsaved: state.tr("unsaved", "не сохранено"),
            selection: state.tr("Selection", "Выделение"),
            fade: state.tr("Fade", "Плавность"),
            speed: state.tr("Speed", "Скорость"),
            boost: state.tr("Boost", "Усиление"),
            apply: state.tr("Apply", "Применить"),
            crop: state.tr("Crop", "Обрезать"),
            fade_in: state.tr("Fade in", "Нарастание"),
            fade_out: state.tr("Fade out", "Затухание"),
            reverse: state.tr("Reverse", "Реверс"),
            normalize: state.tr("Normalize", "Нормализовать"),
            trim: state.tr("Trim silence", "Убрать тишину"),
            karaoke: state.tr("Karaoke", "Караоке"),
            undo: state.tr("Undo", "Отменить"),
            save_as: state.tr("Save as WAV…", "Сохранить как WAV…"),
            add_list: state.tr("Add to sound list", "Добавить в список звуков"),
        }
    }
}

fn draw_editor_window(state: &mut AppState, ctx: &egui::Context, acts: &mut Vec<Act>) {
    if state.editor.is_none() {
        if state.show_editor {
            state.show_editor = false;
        }
        return;
    }
    let mut open = state.show_editor;
    let labels = EditorLabels::new(state);
    let window_title = labels.title.clone();

    egui::Window::new(window_title)
        .open(&mut open)
        .default_width(780.0)
        .default_height(560.0)
        .resizable(true)
        .fade_in(true)
        .show(ctx, |ui| {
            let Some(ed) = state.editor.as_mut() else {
                return;
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(labels.name.clone());
                ui.add(
                    egui::TextEdit::singleline(&mut ed.title)
                        .desired_width(260.0)
                        .id(egui::Id::new("editor-title")),
                );
                ui.separator();
                ui.label(format!(
                    "{}: {}",
                    labels.duration,
                    format_duration(ed.duration_ms())
                ));
                if ed.dirty {
                    ui.colored_label(ui.visuals().warn_fg_color, labels.unsaved.clone());
                }
            });

            paint_waveform(ui, ed);

            let duration_ms = ed.duration_ms();
            let start_label = format!(
                "{:.2}s",
                ed.sel_start.clamp(0.0, 1.0) * duration_ms as f32 / 1000.0
            );
            let end_label = format!(
                "{:.2}s",
                ed.sel_end.clamp(0.0, 1.0) * duration_ms as f32 / 1000.0
            );
            ui.horizontal(|ui| {
                ui.label(labels.selection.clone());
                ui.add(
                    egui::Slider::new(&mut ed.sel_start, 0.0..=1.0)
                        .show_value(false)
                        .text(start_label),
                );
                ui.add(
                    egui::Slider::new(&mut ed.sel_end, 0.0..=1.0)
                        .show_value(false)
                        .text(end_label),
                );
                if ed.sel_start > ed.sel_end {
                    std::mem::swap(&mut ed.sel_start, &mut ed.sel_end);
                }
            });

            ui.horizontal(|ui| {
                let fade_label = labels.fade.clone();
                ui.add(
                    egui::DragValue::new(&mut ed.fade_ms)
                        .range(0..=10_000)
                        .suffix(" ms")
                        .prefix(format!("{fade_label}: ")),
                );
            });

            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(labels.crop.clone()).clicked() {
                    ed.apply_crop();
                }
                if ui.button(labels.fade_in.clone()).clicked() {
                    ed.apply_fade_in();
                }
                if ui.button(labels.fade_out.clone()).clicked() {
                    ed.apply_fade_out();
                }
                if ui.button(labels.reverse.clone()).clicked() {
                    ed.apply_reverse();
                }
                if ui.button(labels.normalize.clone()).clicked() {
                    ed.apply_normalize();
                }
                if ui.button(labels.trim.clone()).clicked() {
                    ed.apply_trim();
                }
                if ui.button(labels.karaoke.clone()).clicked() {
                    ed.apply_karaoke();
                }
                if ui
                    .add_enabled(!ed.undo.is_empty(), egui::Button::new(labels.undo.clone()))
                    .clicked()
                {
                    ed.undo();
                }
            });

            ui.separator();
            ui.horizontal_wrapped(|ui| {
                let speed_label = labels.speed.clone();
                ui.add(
                    egui::Slider::new(&mut ed.speed, 0.25..=4.0)
                        .text(speed_label)
                        .suffix("×"),
                );
                if ui.button(labels.apply.clone()).clicked() {
                    ed.apply_speed();
                }
                ui.separator();
                let boost_label = labels.boost.clone();
                ui.add(
                    egui::Slider::new(&mut ed.boost_db, -24.0..=24.0)
                        .text(boost_label)
                        .suffix(" dB"),
                );
                if ui.button(labels.apply.clone()).clicked() {
                    ed.apply_boost();
                }
            });

            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(labels.save_as.clone()).clicked() {
                    acts.push(Act::EditorSaveAs);
                }
                if ui.button(labels.add_list.clone()).clicked() {
                    acts.push(Act::EditorExport);
                }
            });

            if !ed.status.is_empty() {
                ui.separator();
                ui.small(ed.status.clone());
            }
        });
    state.show_editor = open;
}

fn paint_waveform(ui: &mut egui::Ui, ed: &app::EditorState) {
    let width = ui.available_width().max(160.0);
    let (resp, painter) = ui.allocate_painter(egui::vec2(width, 150.0), egui::Sense::hover());
    let rect = resp.rect;
    let visuals = ui.visuals().clone();

    painter.rect_filled(rect, egui::Rounding::same(4.0), visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        egui::Rounding::same(4.0),
        visuals.widgets.noninteractive.bg_stroke,
    );

    let mid = rect.center().y;
    painter.line_segment(
        [egui::pos2(rect.left(), mid), egui::pos2(rect.right(), mid)],
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
    );

    let start = ed.sel_start.clamp(0.0, 1.0);
    let end = ed.sel_end.clamp(0.0, 1.0);
    let sx = rect.left() + rect.width() * start.min(end);
    let ex = rect.left() + rect.width() * start.max(end);
    let sel_color = visuals.selection.bg_fill;
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(sx, rect.top() + 1.0),
            egui::pos2(ex, rect.bottom() - 1.0),
        ),
        0.0,
        egui::Color32::from_rgba_unmultiplied(sel_color.r(), sel_color.g(), sel_color.b(), 48),
    );

    let frames = ed.audio.frames();
    if frames == 0 {
        return;
    }
    let channels = ed.audio.channels.max(1) as usize;
    let sample_count = ed.audio.samples.len();
    let cols = (rect.width() as usize).clamp(32, 1200);
    let half = (rect.height() / 2.0 - 3.0).max(1.0);
    let wave_color = visuals.widgets.noninteractive.fg_stroke.color;

    for c in 0..cols {
        let f0 = c * frames / cols;
        let f1 = (((c + 1) * frames) / cols).max(f0 + 1).min(frames);
        let step = ((f1 - f0) / 64).max(1);
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        let mut f = f0;
        while f < f1 {
            let base = f * channels;
            let mut acc = 0.0f32;
            let n = channels.min(2);
            for k in 0..n {
                acc += ed.audio.samples.get(base + k).copied().unwrap_or(0.0);
            }
            let v = acc / n as f32;
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
            f += step;
            if base >= sample_count {
                break;
            }
        }
        let x = rect.left() + (c as f32 + 0.5) / cols as f32 * rect.width();
        let y0 = mid - hi.clamp(-1.0, 1.0) * half;
        let y1 = mid - lo.clamp(-1.0, 1.0) * half;
        painter.line_segment(
            [egui::pos2(x, y0), egui::pos2(x, y1)],
            egui::Stroke::new(1.0_f32, wave_color),
        );
    }
}

// -------------------------------------------------------- hotkey overview

fn draw_hotkeys_window(state: &mut AppState, ctx: &egui::Context, acts: &mut Vec<Act>) {
    if !state.show_hotkeys {
        return;
    }
    let mut open = state.show_hotkeys;
    let title = state.tr("Hotkey overview", "Обзор горячих клавиш");
    let set_label = state.tr("Set", "Назначить");
    let clear_label = state.tr("Clear", "Сбросить");
    let actions_header = state.tr("Special actions", "Специальные действия");
    let sounds_header = state.tr("Sounds", "Звуки");
    let cats_header = state.tr("Categories", "Категории");
    let hotbar_header = state.tr("Hotbar", "Хотбар");
    let hint = state.tr(
        "Click Set, then press the key combination you want.",
        "Нажмите «Назначить», затем нужную комбинацию клавиш.",
    );

    let sounds: Vec<(u32, String, Option<Hotkey>)> = state
        .list
        .sounds
        .iter()
        .filter(|s| s.hotkey.is_some() || state.selected_sound == Some(s.id))
        .map(|s| (s.id, s.title.clone(), s.hotkey))
        .collect();
    let categories: Vec<(usize, String, Option<Hotkey>)> = state
        .list
        .categories
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.name.clone(), c.hotkey))
        .collect();
    let hotbar: Vec<(usize, usize, String, Option<Hotkey>)> = state
        .list
        .hotbar
        .iter()
        .enumerate()
        .flat_map(|(pi, page)| {
            page.items.iter().enumerate().map(move |(ii, item)| {
                (pi, ii, item.label.clone().unwrap_or_default(), item.hotkey)
            })
        })
        .collect();

    egui::Window::new(title)
        .open(&mut open)
        .default_width(580.0)
        .default_height(520.0)
        .resizable(true)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.small(hint);
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(actions_header).strong());
                    egui::Grid::new("hk-actions-grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for action in SPECIAL_ACTIONS {
                                let name = action_name(state, action);
                                let hk = state.cfg.hotkey(action);
                                ui.label(name.clone());
                                let hk_text = app::hotkey_label(hk);
                                ui.label(if hk_text.is_empty() {
                                    "—".to_string()
                                } else {
                                    hk_text
                                });
                                ui.horizontal(|ui| {
                                    if ui.small_button(set_label.clone()).clicked() {
                                        acts.push(Act::Capture(
                                            HotkeyTarget::Action(action.to_string()),
                                            name.clone(),
                                        ));
                                    }
                                    if ui
                                        .add_enabled(
                                            hk.is_some(),
                                            egui::Button::new(clear_label.clone()).small(),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Act::ClearHotkey(HotkeyTarget::Action(
                                            action.to_string(),
                                        )));
                                    }
                                });
                                ui.end_row();
                            }
                        });

                    ui.separator();
                    ui.label(egui::RichText::new(cats_header).strong());
                    egui::Grid::new("hk-categories-grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for (ci, cname, chk) in &categories {
                                ui.label(cname.clone());
                                ui.label(display_hotkey(*chk));
                                ui.horizontal(|ui| {
                                    if ui.small_button(set_label.clone()).clicked() {
                                        acts.push(Act::Capture(
                                            HotkeyTarget::Category(*ci),
                                            cname.clone(),
                                        ));
                                    }
                                    if ui
                                        .add_enabled(
                                            chk.is_some(),
                                            egui::Button::new(clear_label.clone()).small(),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Act::ClearHotkey(HotkeyTarget::Category(*ci)));
                                    }
                                });
                                ui.end_row();
                            }
                        });

                    ui.separator();
                    ui.label(egui::RichText::new(sounds_header).strong());
                    egui::Grid::new("hk-sounds-grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for (sid, stitle, shk) in &sounds {
                                ui.label(stitle.clone());
                                ui.label(display_hotkey(*shk));
                                ui.horizontal(|ui| {
                                    if ui.small_button(set_label.clone()).clicked() {
                                        acts.push(Act::Capture(
                                            HotkeyTarget::Sound(*sid),
                                            stitle.clone(),
                                        ));
                                    }
                                    if ui
                                        .add_enabled(
                                            shk.is_some(),
                                            egui::Button::new(clear_label.clone()).small(),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Act::ClearHotkey(HotkeyTarget::Sound(*sid)));
                                    }
                                });
                                ui.end_row();
                            }
                        });

                    ui.separator();
                    ui.label(egui::RichText::new(hotbar_header).strong());
                    egui::Grid::new("hk-hotbar-grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for (pi, ii, label, hhk) in &hotbar {
                                let name = if label.trim().is_empty() {
                                    state
                                        .list
                                        .hotbar
                                        .get(*pi)
                                        .and_then(|p| p.items.get(*ii))
                                        .and_then(|it| state.list.find(it.sound_id))
                                        .map(|s| s.title.clone())
                                        .unwrap_or_else(|| format!("#{}", ii + 1))
                                } else {
                                    label.clone()
                                };
                                ui.label(name.clone());
                                ui.label(display_hotkey(*hhk));
                                ui.horizontal(|ui| {
                                    if ui.small_button(set_label.clone()).clicked() {
                                        acts.push(Act::Capture(
                                            HotkeyTarget::Hotbar {
                                                page: *pi,
                                                item: *ii,
                                            },
                                            name.clone(),
                                        ));
                                    }
                                    if ui
                                        .add_enabled(
                                            hhk.is_some(),
                                            egui::Button::new(clear_label.clone()).small(),
                                        )
                                        .clicked()
                                    {
                                        acts.push(Act::ClearHotkey(HotkeyTarget::Hotbar {
                                            page: *pi,
                                            item: *ii,
                                        }));
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    state.show_hotkeys = open;
}

fn display_hotkey(hk: Option<Hotkey>) -> String {
    let text = app::hotkey_label(hk);
    if text.is_empty() {
        "—".to_string()
    } else {
        text
    }
}

fn action_name(state: &AppState, action: &str) -> String {
    if state.lang_ru {
        ru_action_name(action).to_string()
    } else {
        app::action_label(action).to_string()
    }
}

fn ru_action_name(action: &str) -> &'static str {
    match action {
        action_id::PLAY_SELECTED => "Играть выбранный звук",
        action_id::PLAY_PREVIOUS => "Предыдущий звук",
        action_id::PLAY_NEXT => "Следующий звук",
        action_id::PLAY_RANDOM => "Случайный звук",
        action_id::PLAY_RANDOM_ALL => "Случайный звук (все)",
        action_id::PLAY_CURRENT_AGAIN => "Повторить текущий звук",
        action_id::PLAY_PREVIOUSLY_PLAYED => "Предыдущий проигранный",
        action_id::STOP => "Остановить воспроизведение",
        action_id::PAUSE_RESUME => "Пауза / продолжить",
        action_id::PLAY => "Играть",
        action_id::VOLUME_UP => "Громче",
        action_id::VOLUME_DOWN => "Тише",
        action_id::VOLUME_MUTE => "Выключить звук",
        action_id::MODE_DEFAULT => "Режим: по умолчанию",
        action_id::MODE_SPEAKERS => "Режим: динамики",
        action_id::MODE_MICROPHONE => "Режим: микрофон",
        action_id::MODE_SWITCH => "Переключить режим",
        action_id::CATEGORY_PREV => "Предыдущая категория",
        action_id::CATEGORY_NEXT => "Следующая категория",
        action_id::SOUND_PREV => "Предыдущий звук в списке",
        action_id::SOUND_NEXT => "Следующий звук в списке",
        action_id::RECORD_MIC => "Запись микрофона",
        action_id::RECORD_SPEAKERS => "Запись динамиков",
        action_id::RECORD_STOP => "Остановить запись",
        action_id::HOTKEYS_TOGGLE => "Вкл/выкл горячие клавиши",
        action_id::AUTO_KEYS_TOGGLE => "Вкл/выкл автоклавиши",
        action_id::HOTBAR_PREV_PAGE => "Хотбар: предыдущая страница",
        action_id::HOTBAR_NEXT_PAGE => "Хотбар: следующая страница",
        action_id::SHOW_WINDOW => "Показать окно",
        _ => "Неизвестное действие",
    }
}

// ------------------------------------------------------------------ about

fn draw_about_window(state: &mut AppState, ctx: &egui::Context) {
    if !state.show_about {
        return;
    }
    let mut open = state.show_about;
    let title = state.tr("About RustPad", "О программе RustPad");
    let description = state.tr(
        "A Rust soundboard for Windows with Soundpad-compatible sound lists and remote control.",
        "Звуковая панель на Rust для Windows со звуковыми списками и удалённым управлением, совместимыми с Soundpad.",
    );
    let version = state.tr("Version", "Версия");
    let soundlist = state.tr("Sound list", "Список звуков");
    let config = state.tr("Configuration", "Конфигурация");
    let stats = state.tr("Statistics", "Статистика");
    let devices = state.tr("Devices", "Устройства");

    egui::Window::new(title)
        .open(&mut open)
        .default_width(440.0)
        .resizable(false)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.heading("RustPad");
            ui.label(format!("{version} {}", env!("CARGO_PKG_VERSION")));
            ui.label(description);
            ui.separator();
            ui.small(format!(
                "{soundlist}: {}",
                state.cfg.soundlist_path.display()
            ));
            ui.small(format!("{config}: {}", state.cfg_path.display()));
            ui.small(format!("{stats}: {}", state.cfg.stats_path.display()));
            ui.small(format!(
                "{devices}: {} out / {} in",
                state.out_devices.len(),
                state.in_devices.len()
            ));
            ui.small(format!(
                "{}: {} / {}",
                state.tr("Sounds in the list", "Звуков в списке"),
                state.list.sounds.len(),
                state.visible_ids().len()
            ));
        });
    state.show_about = open;
}

// ------------------------------------------------------- rename / capture

fn draw_rename_category(state: &mut AppState, ctx: &egui::Context) {
    let Some((index, mut name)) = state.show_category_rename.take() else {
        return;
    };
    let mut open = true;
    let mut commit = false;
    let mut cancel = false;
    let title = state.tr("Rename category", "Переименовать категорию");
    let name_label = state.tr("Name", "Название");
    let ok = state.tr("OK", "ОК");
    let cancel_label = state.tr("Cancel", "Отмена");

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.label(name_label);
            let resp = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
            if !resp.has_focus() {
                resp.request_focus();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            ui.horizontal(|ui| {
                if ui.button(ok).clicked() {
                    commit = true;
                }
                if ui.button(cancel_label).clicked() {
                    cancel = true;
                }
            });
        });

    if commit && !name.trim().is_empty() {
        state.rename_category(index, name.trim().to_string());
        open = false;
    }
    if cancel || !open {
        return;
    }
    state.show_category_rename = Some((index, name));
}

fn draw_hotkey_capture(state: &mut AppState, ctx: &egui::Context) {
    if state.hotkey_capture.is_none() {
        return;
    }
    let title = state.tr("Assign a hotkey", "Назначение горячей клавиши");
    let explain = state.tr(
        "Press the key combination you want to assign — it is captured automatically.",
        "Нажмите нужную комбинацию клавиш — она будет назначена автоматически.",
    );
    let manual_hint = state.tr(
        "Or type it manually, e.g. Mouse4 or Ctrl+F1",
        "Или введите вручную, например Mouse4 или Ctrl+F1",
    );
    let apply = state.tr("Apply", "Применить");
    let cancel_label = state.tr("Cancel", "Отмена");

    let mut do_apply = false;
    let mut do_cancel = false;
    if let Some(cap) = state.hotkey_capture.as_mut() {
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fade_in(true)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(format!("{}: {}", manual_hint, cap.label)).strong());
                ui.label(explain);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut cap.manual)
                        .desired_width(220.0)
                        .id(egui::Id::new("hotkey-manual")),
                );
                if !resp.has_focus() {
                    resp.request_focus();
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_apply = true;
                }
                ui.horizontal(|ui| {
                    if ui.button(apply).clicked() {
                        do_apply = true;
                    }
                    if ui.button(cancel_label).clicked() {
                        do_cancel = true;
                    }
                });
            });
    }
    if do_apply {
        state.commit_hotkey_text();
    } else if do_cancel {
        state.hotkey_capture = None;
    }
}

// ------------------------------------------------------------ toasts / DnD

fn draw_mic_setup(state: &mut AppState, ctx: &egui::Context, acts: &mut Vec<Act>) {
    if !state.show_mic_setup {
        return;
    }
    let mut open = state.show_mic_setup;
    let title = state.tr("Microphone setup", "Настройка микрофона");
    let cable_ready = state
        .out_devices
        .iter()
        .any(|d| rustpad_platform::vbcable::looks_like_vbcable(&d.name));
    egui::Window::new(title)
        .open(&mut open)
        .default_width(560.0)
        .resizable(false)
        .fade_in(true)
        .show(ctx, |ui| {
            ui.label(display_label(
                state.tr("Send sounds into your microphone", "Звуки — прямо в микрофон"),
                22.0,
                theme::TEXT,
            ));
            ui.add_space(4.0);
            ui.label(label(
                state.tr(
                    "Soundpad uses a signed audio driver. RustPad does the same in user mode with the free VB-Audio virtual cable — no Soundpad, no extra drivers of our own.",
                    "Soundpad использует подписанный аудиодрайвер. RustPad делает то же самое в пользовательском режиме через бесплатный виртуальный кабель VB-Audio — без Soundpad и без собственных драйверов.",
                ),
                13.0,
                theme::MUTED,
            ));
            ui.add_space(12.0);

            egui::Frame::none()
                .fill(theme::SURFACE_2)
                .stroke(egui::Stroke::new(1.0_f32, theme::HAIRLINE))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if cable_ready {
                            ui.label(label("●", 11.0, theme::GREEN));
                            ui.label(label(
                                state.tr(
                                    "Virtual cable detected — you are ready to go.",
                                    "Виртуальный кабель найден — всё готово.",
                                ),
                                13.0,
                                theme::TEXT,
                            ));
                        } else if state.cable_install.is_some() {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(label(
                                state.tr(
                                    "Installing VB-CABLE… confirm the Windows prompt.",
                                    "Устанавливаю VB-CABLE… подтвердите запрос Windows.",
                                ),
                                13.0,
                                theme::MUTED,
                            ));
                        } else {
                            ui.label(label(
                                state.tr(
                                    "VB-CABLE is not installed yet.",
                                    "VB-CABLE пока не установлен.",
                                ),
                                13.0,
                                theme::MUTED,
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let install = state.tr(
                                        "Install automatically",
                                        "Установить автоматически",
                                    );
                                    if pill_button(ui, &install, false, true).clicked() {
                                        acts.push(Act::InstallCable);
                                    }
                                },
                            );
                        }
                    });
                });

            ui.add_space(12.0);
            for (n, en, ru) in [
                (
                    "1",
                    "Click “Install automatically”, confirm the Windows prompt and wait for the installer to finish.",
                    "Нажмите «Установить автоматически», подтвердите запрос Windows и дождитесь окончания установки.",
                ),
                (
                    "2",
                    "Restart Windows if the installer asks. After reboot RustPad selects “CABLE Input” by itself.",
                    "Перезагрузите Windows, если попросит установщик. После перезагрузки RustPad сам выберет «CABLE Input».",
                ),
                (
                    "3",
                    "In Discord / OBS / game choose “CABLE Output” as the microphone. Your voice and the sounds will be mixed together.",
                    "В Discord / OBS / игре выберите «CABLE Output» как микрофон. Голос и звуки будут микшироваться вместе.",
                ),
            ] {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(22.0, 22.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().circle_filled(
                        rect.center(),
                        10.0,
                        theme::accent_tint(40),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        n,
                        egui::FontId::proportional(10.0),
                        theme::ACCENT,
                    );
                    ui.label(label(state.tr(en, ru), 12.5, theme::MUTED));
                });
                ui.add_space(6.0);
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let site = state.tr("Open VB-CABLE website", "Открыть сайт VB-CABLE");
                if pill_button(ui, &site, false, false).clicked() {
                    acts.push(Act::OpenUrl("https://vb-audio.com/Cable/"));
                }
                let rescan = state.tr("Rescan devices", "Обновить устройства");
                if pill_button(ui, &rescan, false, false).clicked() {
                    acts.push(Act::RescanDevices);
                }
            });
        });
    state.show_mic_setup = open;
}

fn draw_toasts(state: &mut AppState, ctx: &egui::Context) {
    if state.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("rustpad-toasts"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -308.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                let total = state.toasts.len();
                let skip = total.saturating_sub(4);
                for toast in state.toasts.iter().skip(skip) {
                    let age = toast.at.elapsed().as_secs_f32();
                    let fade = ((6.0 - age) / 0.5).clamp(0.0, 1.0);
                    let alpha = (fade * 255.0) as u8;
                    let tint = if toast.error {
                        theme::RED
                    } else {
                        theme::GREEN
                    };
                    let fill = theme::SURFACE.gamma_multiply(0.96);
                    let fill = egui::Color32::from_rgba_premultiplied(
                        (fill.r() as u32 * alpha as u32 / 255) as u8,
                        (fill.g() as u32 * alpha as u32 / 255) as u8,
                        (fill.b() as u32 * alpha as u32 / 255) as u8,
                        alpha,
                    );
                    ui.add_space(4.0 * (1.0 - fade));
                    egui::Frame::none()
                        .fill(fill)
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            theme::white((alpha as f32 * 0.10) as u8),
                        ))
                        .rounding(egui::Rounding::same(12.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                        .show(ui, |ui| {
                            ui.set_max_width(340.0);
                            ui.horizontal(|ui| {
                                ui.label(label("●", 9.0, tint.gamma_multiply(fade)));
                                ui.label(label(
                                    &toast.text,
                                    12.0,
                                    theme::TEXT.gamma_multiply(fade),
                                ));
                            });
                        });
                }
            });
        });
}

fn handle_dropped_files(state: &mut AppState, ctx: &egui::Context) {
    let dropped: Vec<PathBuf> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .collect()
    });
    if dropped.is_empty() {
        return;
    }
    let count = dropped.len();
    state.import_files(dropped);
    state.toast(
        format!(
            "{} {}",
            count,
            state.tr("file(s) queued for import", "файл(ов) в очереди на импорт")
        ),
        false,
    );
}

fn draw_drop_overlay(state: &mut AppState, ctx: &egui::Context) {
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let rect = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("rustpad-drop-overlay"),
    ));
    painter.rect_filled(
        rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(20, 90, 160, 90),
    );
    painter.rect_stroke(
        rect.shrink(12.0),
        egui::Rounding::same(10.0),
        egui::Stroke::new(3.0_f32, egui::Color32::from_rgb(200, 230, 255)),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        state.tr(
            "Drop audio files to add them",
            "Отпустите файлы, чтобы добавить их",
        ),
        egui::FontId::proportional(24.0),
        egui::Color32::WHITE,
    );
}

// ------------------------------------------------- system input handling

fn handle_close_request(state: &mut AppState, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) {
        return;
    }
    if state.cfg.minimize_instead_of_close {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        state.toast(
            state.tr(
                "RustPad keeps running in the background",
                "RustPad продолжает работать в фоне",
            ),
            false,
        );
    }
}

fn handle_shortcuts(state: &mut AppState, ctx: &egui::Context, uis: &mut UiState) {
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Comma)) {
        state.show_settings = true;
    }
    if ctx.wants_keyboard_input() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            state.save_soundlist();
        }
        return;
    }

    let saving = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));
    let opening = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::O));
    let finding = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::F));
    let entering = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let deleting = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
    let pausing = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
    let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    if opening {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Soundpad list", &["spl"])
            .pick_file()
        {
            state.open_soundlist(path);
        }
    }
    if saving {
        state.save_soundlist();
    }
    if finding {
        uis.search_focus = true;
    }
    if state.hotkey_capture.is_some() {
        return;
    }
    if escape {
        if state.show_settings
            || state.show_tts
            || state.show_recorder
            || state.show_editor
            || state.show_about
            || state.show_hotkeys
        {
            state.show_settings = false;
            state.show_tts = false;
            state.show_recorder = false;
            state.show_editor = false;
            state.show_about = false;
            state.show_hotkeys = false;
        }
        return;
    }
    if entering {
        if let Some(id) = state.selected_sound {
            state.play_sound(id, state.play_mode);
        }
    }
    if deleting {
        state.remove_selected();
    }
    if pausing {
        state.toggle_pause();
    }
}

// --------------------------------------------------------- action runner

fn apply_actions(state: &mut AppState, ctx: &egui::Context, acts: Vec<Act>) {
    for act in acts {
        match act {
            Act::Play(id, mode) => state.play_sound(id, mode),
            Act::Stop => state.stop(),
            Act::Pause => state.toggle_pause(),
            Act::Prev => state.play_previous(),
            Act::Next => state.play_next(),
            Act::Random => state.play_random(None),
            Act::Seek(ms) => state.seek(ms),
            Act::MasterVolume(v) => {
                let v = v.clamp(0.0, 1.5);
                state.cfg.master_volume = v;
                state.engine.send(EngineCommand::SetMasterVolume(v));
                state.cfg_dirty();
            }
            Act::SpeakersVolume(v) => {
                let v = v.clamp(0.0, 1.5);
                state.cfg.speakers_volume = v;
                state
                    .engine
                    .send(EngineCommand::SetTargetVolume(Target::Speakers, v));
            }
            Act::MicVolume(v) => {
                let v = v.clamp(0.0, 1.5);
                state.cfg.microphone_volume = v;
                state
                    .engine
                    .send(EngineCommand::SetTargetVolume(Target::Microphone, v));
            }
            Act::ApplyEngine => app::apply_engine_settings(state),
            Act::RescanDevices => {
                let (out, inp) = rustpad_audio::EngineHandle::devices();
                state.out_devices = out;
                state.in_devices = inp;
                let before = state.cfg.mic_output_device.clone();
                app::apply_engine_settings(state);
                match (&before, &state.cfg.mic_output_device) {
                    (_, Some(name)) if before.is_none() => state.toast(
                        format!(
                            "{}: {name}",
                            state.tr("Microphone output selected", "Выбран вывод для микрофона")
                        ),
                        false,
                    ),
                    _ => state.toast(
                        state.tr("Audio devices rescanned", "Список устройств обновлён"),
                        false,
                    ),
                }
            }
            Act::PickFiles => {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Audio", SOUND_FILTER)
                    .pick_files()
                {
                    state.import_files(files);
                }
            }
            Act::PickFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    state.import_files(vec![dir]);
                }
            }
            Act::OpenList(path) => state.open_soundlist(path),
            Act::ShowMicSetup => state.show_mic_setup = true,
            Act::OpenUrl(url) => {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", url])
                    .spawn();
            }
            Act::SaveList => state.save_soundlist(),
            Act::SaveListAs => {
                let path = rfd::FileDialog::new()
                    .add_filter("Soundpad list", &["spl"])
                    .set_file_name("soundlist.spl")
                    .save_file();
                if let Some(path) = path {
                    state.list.file_path = Some(path.clone());
                    state.cfg.soundlist_path = path;
                    state.save_soundlist();
                    state.save_config();
                }
            }
            Act::ImportSoundpad => {
                let applied = state.import_soundpad_settings();
                let _ = applied;
            }
            Act::Quit => state.request_quit(),
            Act::RemoveSelected => state.remove_selected(),
            Act::RemoveDead => state.remove_dead_entries(),
            Act::RemoveSound(id) => {
                state.selected_sound = Some(id);
                state.remove_selected();
            }
            Act::NewCategory => {
                let name = state.tr("New category", "Новая категория");
                state.new_category(name);
            }
            Act::RenameCategory(index) => {
                if let Some(name) = state.list.categories.get(index).map(|c| c.name.clone()) {
                    state.show_category_rename = Some((index, name));
                }
            }
            Act::DeleteCategory(index) => state.delete_category(index),
            Act::SetCategoryColor(index, color) => {
                if let Some(cat) = state.list.categories.get_mut(index) {
                    cat.color = color;
                    state.dirty = true;
                }
            }
            Act::RandomFromCategory(index) => {
                let mode = state.play_mode;
                state.play_random_with(Some(index), mode);
            }
            Act::AddToCategory(id, cat) => state.add_sound_to_category(id, cat),
            Act::RemoveFromCategory(id, cat) => state.remove_sound_from_category(id, cat),
            Act::AddToHotbar(page, id) => state.add_to_hotbar(page, id),
            Act::RemoveHotbarItem(page, item) => {
                let mut removed = false;
                if let Some(p) = state.list.hotbar.get_mut(page) {
                    if item < p.items.len() {
                        p.items.remove(item);
                        removed = true;
                    }
                }
                if removed {
                    state.dirty = true;
                }
            }
            Act::SetHotbarColor(page, item, color) => {
                let mut changed = false;
                if let Some(p) = state.list.hotbar.get_mut(page) {
                    if let Some(it) = p.items.get_mut(item) {
                        it.color = color;
                        changed = true;
                    }
                }
                if changed {
                    state.dirty = true;
                }
            }
            Act::SetSoundColor(id, color) => {
                if let Some(sound) = state.list.find_mut(id) {
                    sound.color = color;
                    state.dirty = true;
                }
            }
            Act::Capture(target, label) => state.begin_hotkey_capture(target, label),
            Act::ClearHotkey(target) => state.clear_hotkey(&target),
            Act::ResetHotkeys => {
                state.cfg.hotkeys.clear();
                state.save_config();
                state.toast(
                    state.tr("All hotkeys were reset", "Все горячие клавиши сброшены"),
                    false,
                );
            }
            Act::OpenEditor(id) => state.open_editor(id),
            Act::ShowInExplorer(path) => {
                if std::process::Command::new("explorer")
                    .arg(format!("/select,{}", path.display()))
                    .spawn()
                    .is_err()
                {
                    state.toast(
                        state.tr("Could not open Explorer", "Не удалось открыть Проводник"),
                        true,
                    );
                }
            }
            Act::OpenFolder(dir) => {
                if std::process::Command::new("explorer")
                    .arg(dir.as_os_str())
                    .spawn()
                    .is_err()
                {
                    state.toast(
                        state.tr("Could not open the folder", "Не удалось открыть папку"),
                        true,
                    );
                }
            }
            Act::NewHotbarPage => state.new_hotbar_page(),
            Act::RemoveHotbarPage(index) => state.remove_hotbar_page(index),
            Act::SaveConfig => {
                state.save_config();
                state.toast(
                    state.tr("Configuration saved", "Настройки сохранены"),
                    false,
                );
            }
            Act::SetAutostart(enabled) => {
                state.cfg.autostart = enabled;
                let exe = std::env::current_exe().unwrap_or_default();
                match rustpad_platform::autostart::set_autostart(&exe, enabled) {
                    Ok(()) => state.toast(
                        if enabled {
                            state.tr("Autostart enabled", "Автозапуск включён")
                        } else {
                            state.tr("Autostart disabled", "Автозапуск выключен")
                        },
                        false,
                    ),
                    Err(err) => state.toast(format!("Autostart: {err:#}"), true),
                }
                state.save_config();
            }
            Act::SetAlwaysOnTop(on_top) => {
                state.cfg.always_on_top = on_top;
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                }));
                state.save_config();
            }
            Act::InstallCable => {
                if state.cable_install.is_none() {
                    state.cable_install = Some(rustpad_platform::vbcable::start_install());
                    state.toast(
                        state.tr(
                            "Downloading VB-CABLE — confirm the Windows prompt, then wait for the installer.",
                            "Скачиваю VB-CABLE — подтвердите запрос Windows и дождитесь установки.",
                        ),
                        false,
                    );
                } else {
                    state.toast(
                        state.tr(
                            "The installer is already running.",
                            "Установщик уже запущен.",
                        ),
                        false,
                    );
                }
            }
            Act::PickRecordingDir => {
                let start = state.cfg.recording_dir.clone();
                if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                    state.cfg.recording_dir = dir;
                }
            }
            Act::EditorSaveAs => state.editor_save_as(),
            Act::EditorExport => state.editor_export_to_list(),
        }
    }
}

// ---------------------------------------------------------------- helpers

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

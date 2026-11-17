//! Visual language for RustPad.
//!
//! The interface is built like a piece of studio hardware: a black field,
//! warm graphite surfaces, one-pixel rules and a single warm-white interaction
//! colour. Colour is reserved for state (signal green, recording red, warning
//! amber) and for the user's own sound tags. Headings use Segoe UI Semibold so
//! hierarchy comes from weight, not from oversized decorative type.

use eframe::egui;
use egui::epaint::Shadow;
use rustpad_core::config::Theme;

// ------------------------------------------------------------------ palette

/// Window background. Pure black, by request.
pub const BG: egui::Color32 = egui::Color32::from_rgb(0, 0, 0);
/// Cards, sidebars and the transport dock.
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(10, 10, 12);
/// Slightly raised controls inside a card.
pub const SURFACE_2: egui::Color32 = egui::Color32::from_rgb(17, 17, 20);
/// Hovered control.
pub const SURFACE_3: egui::Color32 = egui::Color32::from_rgb(25, 25, 29);
/// Hairline border.
pub const HAIRLINE: egui::Color32 = egui::Color32::from_rgba_premultiplied(16, 16, 16, 16);
/// Stronger border for focus / hover.
pub const HAIRLINE_STRONG: egui::Color32 = egui::Color32::from_rgba_premultiplied(34, 34, 34, 34);
/// Primary text.
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(244, 242, 237);
/// Secondary text.
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(164, 165, 171);
/// Tertiary text (numbers, hints).
pub const FAINT: egui::Color32 = egui::Color32::from_rgb(126, 128, 135);
/// The interaction colour. Warm white keeps the chrome quiet; the sound
/// colours, the signal meters and the live indicators carry the energy.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(238, 236, 230);
/// Accent hover.
pub const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
/// Text and icons drawn on top of the accent fill.
pub const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(9, 9, 10);
/// Signal colours. These are semantic, not decorative.
pub const BLUE: egui::Color32 = egui::Color32::from_rgb(110, 166, 230);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(64, 214, 150);
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(240, 183, 90);
pub const RED: egui::Color32 = egui::Color32::from_rgb(255, 94, 94);

/// White at a given opacity, premultiplied so it composites correctly on black.
pub fn white(alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
}

/// Soft accent tint for selected rows. The accent is warm white, so this is a
/// neutral wash rather than a coloured pill.
pub fn accent_tint(alpha: u8) -> egui::Color32 {
    white(alpha)
}

// ------------------------------------------------------------------- fonts

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let display_paths = [
        "C:/Windows/Fonts/seguisb.ttf",
        "C:/Windows/Fonts/segoeuib.ttf",
        "C:/Windows/Fonts/SegUIVar.ttf",
    ];
    let text_paths = [
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/tahoma.ttf",
    ];
    let mono_paths = ["C:/Windows/Fonts/consola.ttf", "C:/Windows/Fonts/cour.ttf"];

    let mut display_key = None;
    for path in display_paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("display".to_owned(), egui::FontData::from_owned(bytes));
            display_key = Some("display".to_owned());
            break;
        }
    }
    let mut text_key = None;
    for path in text_paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("text".to_owned(), egui::FontData::from_owned(bytes));
            text_key = Some("text".to_owned());
            break;
        }
    }
    let mut mono_key = None;
    for path in mono_paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("mono".to_owned(), egui::FontData::from_owned(bytes));
            mono_key = Some("mono".to_owned());
            break;
        }
    }

    let default_family = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallback = default_family
        .first()
        .cloned()
        .unwrap_or_else(|| "Ubuntu-Light".to_owned());

    let mut proportional = Vec::new();
    if let Some(key) = &text_key {
        proportional.push(key.clone());
    }
    if let Some(key) = &display_key {
        if !proportional.contains(key) {
            proportional.push(key.clone());
        }
    }
    proportional.extend(default_family.iter().cloned());
    if proportional.is_empty() {
        proportional.push(fallback.clone());
    }

    let mut display = Vec::new();
    if let Some(key) = &display_key {
        display.push(key.clone());
    }
    if let Some(key) = &text_key {
        display.push(key.clone());
    }
    display.extend(default_family.iter().cloned());
    if display.is_empty() {
        display.push(fallback);
    }

    fonts
        .families
        .insert(egui::FontFamily::Proportional, proportional);
    fonts
        .families
        .insert(egui::FontFamily::Name("display".into()), display);
    if let Some(key) = mono_key {
        let mut mono = vec![key];
        mono.extend(default_family.iter().cloned());
        fonts.families.insert(egui::FontFamily::Monospace, mono);
    }
    // Keep symbol coverage for the small geometric glyphs used in the UI.
    for fallback in ["Segoe UI Symbol", "NotoEmoji-Regular"] {
        if fonts.font_data.contains_key(fallback) {
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                if !list.iter().any(|n| n == fallback) {
                    list.push(fallback.to_owned());
                }
            }
        }
    }

    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    apply_theme(ctx);
}

/// The interface is dark only. A soundboard is a creative/performance tool that
/// lives on screen next to a game or stream, and the owner asked for a black
/// surface. A light variant was previously exposed but every custom control is
/// painted for the dark palette, so shipping the toggle would ship a broken
/// mode. The preference is kept in config for migration, but it resolves to the
/// one verified theme.
pub fn apply_preference(ctx: &egui::Context, _theme: Theme) {
    apply_theme(ctx);
}

pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_visuals(custom_dark());

    let mut style = (*ctx.style()).clone();
    style.animation_time = 0.14;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size = egui::vec2(34.0, 30.0);
    style.spacing.menu_margin = egui::Margin::symmetric(8.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    style.spacing.slider_width = 168.0;
    style.spacing.indent = 18.0;
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 3.0,
        handle_min_length: 24.0,
        bar_inner_margin: 2.0,
        floating_width: 3.0,
        floating_allocated_width: 4.0,
        foreground_color: false,
        ..Default::default()
    };
    style.text_styles = [
        (egui::TextStyle::Small, egui::FontId::proportional(11.5)),
        (egui::TextStyle::Body, egui::FontId::proportional(13.5)),
        (egui::TextStyle::Button, egui::FontId::proportional(13.0)),
        (
            egui::TextStyle::Heading,
            egui::FontId::new(21.0, egui::FontFamily::Name("display".into())),
        ),
        (egui::TextStyle::Monospace, egui::FontId::monospace(12.0)),
    ]
    .into();
    style.visuals.menu_rounding = egui::Rounding::same(10.0);
    style.visuals.window_rounding = egui::Rounding::same(14.0);
    style.visuals.popup_shadow = Shadow {
        offset: egui::vec2(0.0, 10.0),
        blur: 28.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(160),
    };
    ctx.set_style(style);
}

fn custom_dark() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = egui::Color32::from_rgb(5, 5, 7);
    v.faint_bg_color = egui::Color32::from_rgb(9, 9, 12);
    v.code_bg_color = egui::Color32::from_rgb(14, 14, 18);
    v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(13, 13, 13, 13);
    v.selection.stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.hyperlink_color = TEXT;
    v.warn_fg_color = AMBER;
    v.error_fg_color = RED;
    v.window_rounding = egui::Rounding::same(14.0);
    v.window_stroke = egui::Stroke::new(1.0_f32, HAIRLINE);
    v.window_shadow = Shadow {
        offset: egui::vec2(0.0, 18.0),
        blur: 40.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(200),
    };
    v.menu_rounding = egui::Rounding::same(12.0);
    v.popup_shadow = Shadow {
        offset: egui::vec2(0.0, 10.0),
        blur: 26.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(180),
    };
    v.override_text_color = Some(TEXT);

    let r = egui::Rounding::same(8.0);
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.noninteractive.rounding = r;

    v.widgets.inactive.bg_fill = SURFACE_2;
    v.widgets.inactive.weak_bg_fill = SURFACE_2;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, HAIRLINE);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.rounding = r;

    v.widgets.hovered.bg_fill = SURFACE_3;
    v.widgets.hovered.weak_bg_fill = SURFACE_3;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, HAIRLINE_STRONG);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    v.widgets.hovered.rounding = r;
    v.widgets.hovered.expansion = 0.0;

    v.widgets.active.bg_fill = egui::Color32::from_rgb(34, 34, 38);
    v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(34, 34, 38);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    v.widgets.active.rounding = r;

    v.widgets.open.bg_fill = SURFACE_3;
    v.widgets.open.weak_bg_fill = SURFACE_3;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, HAIRLINE_STRONG);
    v.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.open.rounding = r;

    v.slider_trailing_fill = true;
    v.striped = false;
    v.window_highlight_topmost = false;
    v
}

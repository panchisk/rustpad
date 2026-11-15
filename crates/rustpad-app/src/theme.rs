//! Visual language for RustPad.
//!
//! The dark theme is deliberately *true black*: the window, panels and the
//! space between cards are `#000000`. Content sits on near-black surfaces with
//! hairline white borders, large radii and a single vivid indigo accent, which
//! is what gives the app its calm, premium macOS-like feel. Everything is
//! drawn with vector primitives, so the look costs no extra textures or RAM.

use eframe::egui;
use egui::epaint::Shadow;
use rustpad_core::config::Theme;

// ------------------------------------------------------------------ palette

/// Window background. Pure black, by request.
pub const BG: egui::Color32 = egui::Color32::from_rgb(0, 0, 0);
/// Cards, sidebars and the transport dock.
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(11, 11, 14);
/// Slightly raised controls inside a card.
pub const SURFACE_2: egui::Color32 = egui::Color32::from_rgb(19, 19, 24);
/// Hovered control.
pub const SURFACE_3: egui::Color32 = egui::Color32::from_rgb(28, 28, 34);
/// Hairline border.
pub const HAIRLINE: egui::Color32 = egui::Color32::from_rgba_premultiplied(22, 22, 22, 22);
/// Stronger border for focus / hover.
pub const HAIRLINE_STRONG: egui::Color32 = egui::Color32::from_rgba_premultiplied(46, 46, 46, 46);
/// Primary text.
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(246, 246, 248);
/// Secondary text.
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(148, 148, 158);
/// Tertiary text (numbers, hints).
pub const FAINT: egui::Color32 = egui::Color32::from_rgb(96, 96, 106);
/// The one accent used for primary actions and selection.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 108, 255);
/// Accent hover.
pub const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(142, 128, 255);
/// Warm secondary accent for "live" indicators.
pub const BLUE: egui::Color32 = egui::Color32::from_rgb(96, 168, 255);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(72, 214, 146);
pub const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 184, 76);
pub const RED: egui::Color32 = egui::Color32::from_rgb(255, 92, 92);

/// White at a given opacity, premultiplied so it composites correctly on black.
pub fn white(alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha)
}

/// Soft accent tint for selected rows.
pub fn accent_tint(alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        (ACCENT.r() as u32 * alpha as u32 / 255) as u8,
        (ACCENT.g() as u32 * alpha as u32 / 255) as u8,
        (ACCENT.b() as u32 * alpha as u32 / 255) as u8,
        alpha,
    )
}

// ------------------------------------------------------------------- fonts

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let display_paths = [
        "C:/Windows/Fonts/SegUIVar.ttf",
        "C:/Windows/Fonts/segoeuib.ttf",
    ];
    let text_paths = [
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/tahoma.ttf",
    ];

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

pub fn system_prefers_dark() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        if let Ok(hkcu) = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        {
            if let Ok(v) = hkcu.get_value::<u32, _>("AppsUseLightTheme") {
                return v == 0;
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    apply_theme(ctx, system_prefers_dark());
}

/// Applies a persisted theme preference (`Light`, `Dark` or follow the system).
pub fn apply_preference(ctx: &egui::Context, theme: Theme) {
    let dark = match theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::System => system_prefers_dark(),
    };
    apply_theme(ctx, dark);
}

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let visuals = if dark { custom_dark() } else { custom_light() };
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.animation_time = 0.16;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size = egui::vec2(34.0, 30.0);
    style.spacing.menu_margin = egui::Margin::symmetric(8.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    style.spacing.slider_width = 168.0;
    style.spacing.indent = 18.0;
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 8.0,
        handle_min_length: 28.0,
        bar_inner_margin: 4.0,
        floating_width: 3.0,
        ..Default::default()
    };
    style.text_styles = [
        (egui::TextStyle::Small, egui::FontId::proportional(12.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(14.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(14.0)),
        (
            egui::TextStyle::Heading,
            egui::FontId::new(22.0, egui::FontFamily::Name("display".into())),
        ),
        (egui::TextStyle::Monospace, egui::FontId::monospace(13.0)),
    ]
    .into();
    style.visuals.menu_rounding = egui::Rounding::same(12.0);
    style.visuals.window_rounding = egui::Rounding::same(18.0);
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
    v.selection.bg_fill = accent_tint(58);
    v.selection.stroke = egui::Stroke::NONE;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = AMBER;
    v.error_fg_color = RED;
    v.window_rounding = egui::Rounding::same(18.0);
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

    let r = egui::Rounding::same(10.0);
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

    v.widgets.active.bg_fill = egui::Color32::from_rgb(38, 38, 46);
    v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(38, 38, 46);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
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

fn custom_light() -> egui::Visuals {
    // A calm, paper-like counterpart used only when the user explicitly asks
    // for the light theme. The default experience is true black.
    let mut v = egui::Visuals::light();
    v.panel_fill = egui::Color32::from_rgb(250, 250, 252);
    v.window_fill = egui::Color32::WHITE;
    v.extreme_bg_color = egui::Color32::WHITE;
    v.faint_bg_color = egui::Color32::from_rgb(244, 244, 247);
    v.selection.bg_fill = egui::Color32::from_rgb(108, 92, 231);
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    v.hyperlink_color = egui::Color32::from_rgb(86, 70, 220);
    v.window_rounding = egui::Rounding::same(18.0);
    v.window_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(226, 226, 232));
    v.menu_rounding = egui::Rounding::same(12.0);
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(228, 228, 234));
    v
}

/// Draws a soft, layered glow behind a shape. Cheap: a handful of rounded
/// rectangles, no blur textures.
pub fn glow(
    painter: &egui::Painter,
    rect: egui::Rect,
    rounding: egui::Rounding,
    color: egui::Color32,
    strength: f32,
) {
    let alpha = (strength * 60.0).clamp(0.0, 60.0) as u8;
    for (i, spread) in [6.0f32, 4.0, 2.0].iter().enumerate() {
        let a = (alpha as f32 * (1.0 - i as f32 * 0.28)).max(0.0) as u8;
        let c = egui::Color32::from_rgba_premultiplied(
            (color.r() as u32 * a as u32 / 255) as u8,
            (color.g() as u32 * a as u32 / 255) as u8,
            (color.b() as u32 * a as u32 / 255) as u8,
            a,
        );
        painter.rect_stroke(
            rect.expand(*spread),
            rounding,
            egui::Stroke::new(2.0_f32, c),
        );
    }
}

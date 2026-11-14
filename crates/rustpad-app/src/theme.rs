//! Fonts and visual styling.
//!
//! The default egui font is replaced with the Windows UI font (Segoe UI) so the
//! app looks native and renders Cyrillic, Greek and CJK text on every system.

use eframe::egui;
use rustpad_core::config::Theme;

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    apply_theme(ctx, system_prefers_dark());
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidates = [
        ("segoe", "C:/Windows/Fonts/segoeui.ttf"),
        ("segoe", "C:/Windows/Fonts/segoeuib.ttf"),
        ("segoe", "C:/Windows/Fonts/segoeuil.ttf"),
        ("tahoma", "C:/Windows/Fonts/tahoma.ttf"),
        ("symbols", "C:/Windows/Fonts/seguisym.ttf"),
    ];
    for (name, path) in candidates {
        if fonts.font_data.contains_key(name) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(bytes));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push(name.to_owned());
            }
        }
    }
    // Windows font should win over the bundled Latin-only fonts.
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.sort_by_key(|n| {
                if n.starts_with("segoe") {
                    0
                } else if n == "tahoma" {
                    1
                } else {
                    2
                }
            });
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
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.menu_margin = egui::Margin::symmetric(6.0, 6.0);
    style.spacing.slider_width = 140.0;
    style.visuals.menu_rounding = egui::Rounding::same(6.0);
    ctx.set_style(style);
}

fn custom_dark() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = egui::Color32::from_rgb(30, 32, 36);
    v.window_fill = egui::Color32::from_rgb(36, 38, 43);
    v.extreme_bg_color = egui::Color32::from_rgb(22, 24, 27);
    v.faint_bg_color = egui::Color32::from_rgb(40, 42, 48);
    v.selection.bg_fill = egui::Color32::from_rgb(58, 110, 180);
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(220, 232, 255));
    v.hyperlink_color = egui::Color32::from_rgb(120, 170, 245);
    v.window_rounding = egui::Rounding::same(8.0);
    v.menu_rounding = egui::Rounding::same(6.0);
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(60, 63, 70));
    v
}

fn custom_light() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.panel_fill = egui::Color32::from_rgb(246, 247, 249);
    v.window_fill = egui::Color32::from_rgb(255, 255, 255);
    v.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);
    v.faint_bg_color = egui::Color32::from_rgb(238, 240, 243);
    v.selection.bg_fill = egui::Color32::from_rgb(66, 120, 190);
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    v.window_rounding = egui::Rounding::same(8.0);
    v.menu_rounding = egui::Rounding::same(6.0);
    v
}

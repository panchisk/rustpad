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
    SetAutostart(bool),
    SetAlwaysOnTop(bool),
    PickRecordingDir,
}

// ------------------------------------------------------------------ entry

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let ui_id = egui::Id::new("rustpad-ui");
    let mut uis: UiState = ctx.data_mut(|d| d.get_temp_mut_or_default::<UiState>(ui_id).clone());
    let mut acts: Vec<Act> = Vec::new();

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

    egui::TopBottomPanel::top("rustpad-menu")
        .show(ctx, |ui| draw_menu(state, ui, &mut uis, &mut acts));
    egui::TopBottomPanel::top("rustpad-toolbar")
        .show(ctx, |ui| draw_toolbar(state, ui, &mut uis, &mut acts));

    if !uis.hotbar_collapsed {
        egui::TopBottomPanel::bottom("rustpad-hotbar")
            .resizable(true)
            .default_height(168.0)
            .min_height(96.0)
            .max_height(460.0)
            .show(ctx, |ui| {
                draw_transport(state, ui, &mut uis, &mut acts);
                ui.separator();
                draw_hotbar(state, ui, &mut uis, &mut acts);
            });
    }

    if !uis.sidebar_collapsed {
        egui::SidePanel::left("rustpad-categories")
            .resizable(true)
            .default_width(190.0)
            .width_range(150.0..=340.0)
            .show(ctx, |ui| draw_sidebar(state, ui, &mut acts));
    }

    egui::CentralPanel::default().show(ctx, |ui| draw_center(state, ui, &mut uis, &mut acts));

    draw_settings(state, ctx, &mut uis, &mut acts);
    draw_tts_window(state, ctx);
    draw_recorder_window(state, ctx, &mut acts);
    draw_editor_window(state, ctx, &mut acts);
    draw_hotkeys_window(state, ctx, &mut acts);
    draw_about_window(state, ctx);
    draw_rename_category(state, ctx);
    draw_hotkey_capture(state, ctx);
    draw_drop_overlay(state, ctx);
    draw_toasts(state, ctx);

    apply_actions(state, ctx, acts);

    ctx.data_mut(|d| d.insert_temp(ui_id, uis));
}

// -------------------------------------------------------------- menu bar

fn draw_menu(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    egui::menu::bar(ui, |ui| {
        ui.menu_button(state.tr("File", "Файл"), |ui| {
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
        });

        ui.menu_button(state.tr("Edit", "Правка"), |ui| {
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
        });

        ui.menu_button(state.tr("View", "Вид"), |ui| {
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
        });

        ui.menu_button(state.tr("Help", "Справка"), |ui| {
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
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if state.dirty {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    state.tr("unsaved changes", "есть несохранённые изменения"),
                );
            }
            ui.weak(format!(
                "{} / {}",
                state.visible_ids().len(),
                state.list.sounds.len()
            ));
        });
    });
}

// ---------------------------------------------------------------- toolbar

fn draw_toolbar(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    ui.horizontal_wrapped(|ui| {
        let side_icon = if uis.sidebar_collapsed { "▶" } else { "◀" };
        if ui
            .small_button(side_icon)
            .on_hover_text(state.tr(
                "Show or hide the categories",
                "Показать или скрыть категории",
            ))
            .clicked()
        {
            uis.sidebar_collapsed = !uis.sidebar_collapsed;
        }

        if ui
            .button(format!("▶ {}", state.tr("Play", "Играть")))
            .on_hover_text(state.tr("Play the selected sound", "Проиграть выбранный звук"))
            .clicked()
        {
            if let Some(id) = state.selected_sound {
                state.play_sound(id, state.play_mode);
            } else {
                state.play_current_again();
            }
        }
        if ui
            .button(format!("■ {}", state.tr("Stop", "Стоп")))
            .on_hover_text(state.tr("Stop all playback", "Остановить воспроизведение"))
            .clicked()
        {
            acts.push(Act::Stop);
        }
        let pause_text = if state.status.paused {
            state.tr("Resume", "Продолжить").to_string()
        } else {
            state.tr("Pause", "Пауза").to_string()
        };
        if ui.button(pause_text).clicked() {
            acts.push(Act::Pause);
        }
        if ui
            .button(state.tr("Prev", "Пред."))
            .on_hover_text(state.tr("Previous sound", "Предыдущий звук"))
            .clicked()
        {
            acts.push(Act::Prev);
        }
        if ui
            .button(state.tr("Next", "След."))
            .on_hover_text(state.tr("Next sound", "Следующий звук"))
            .clicked()
        {
            acts.push(Act::Next);
        }
        if ui
            .button(state.tr("Random", "Случайный"))
            .on_hover_text(state.tr("Random sound", "Случайный звук"))
            .clicked()
        {
            acts.push(Act::Random);
        }

        ui.separator();

        let mode_label = play_mode_name(state, state.play_mode);
        let mut mode = state.play_mode;
        egui::ComboBox::from_id_salt("toolbar-play-mode")
            .width(150.0)
            .selected_text(mode_label)
            .show_ui(ui, |ui| {
                for m in [
                    PlayMode::Default,
                    PlayMode::Speakers,
                    PlayMode::Microphone,
                    PlayMode::Both,
                ] {
                    ui.selectable_value(&mut mode, m, play_mode_name(state, m));
                }
            });
        if mode != state.play_mode {
            state.set_play_mode(mode);
        }

        ui.separator();

        let mut vol = state.cfg.master_volume.clamp(0.0, 1.5);
        let master_label = state.tr("Master", "Общая");
        if ui
            .add(egui::Slider::new(&mut vol, 0.0..=1.5).text(master_label))
            .changed()
        {
            acts.push(Act::MasterVolume(vol));
        }

        ui.separator();

        let out = state.out_level.clamp(0.0, 1.0);
        let mic = state.mic_level.clamp(0.0, 1.0);
        ui.add(
            egui::ProgressBar::new(out)
                .desired_width(72.0)
                .fill(level_color(out))
                .text(format!("{:.0}%", out * 100.0)),
        )
        .on_hover_text(state.tr("Output level", "Уровень выхода"));
        ui.add(
            egui::ProgressBar::new(mic)
                .desired_width(72.0)
                .fill(level_color(mic))
                .text(format!("{:.0}%", mic * 100.0)),
        )
        .on_hover_text(state.tr("Microphone level", "Уровень микрофона"));

        ui.separator();

        let hk_label = state.tr("Hotkeys", "Клавиши");
        if ui
            .toggle_value(&mut state.hotkeys_enabled, hk_label)
            .changed()
        {
            state.cfg.hotkeys_enabled = state.hotkeys_enabled;
        }
        let ak_label = state.tr("Auto Keys", "Автоклавиши");
        let mut auto_keys = state.auto_keys_enabled;
        if ui.toggle_value(&mut auto_keys, ak_label).changed() {
            state.set_auto_keys_enabled(auto_keys);
        }

        ui.separator();

        let tts_label = state.tr("TTS", "Озвучка").to_string();
        let record_label = state.tr("Record", "Запись").to_string();
        let settings_label = state.tr("Settings", "Настройки").to_string();
        let editor_label = state.tr("Editor", "Редактор").to_string();
        ui.toggle_value(&mut state.show_tts, tts_label);
        ui.toggle_value(&mut state.show_recorder, record_label);
        ui.toggle_value(&mut state.show_settings, settings_label);
        if ui
            .toggle_value(&mut state.show_editor, editor_label)
            .clicked()
            && state.show_editor
            && state.editor.is_none()
        {
            acts.push(Act::OpenEditor(None));
        }
        let hotbar_icon = if uis.hotbar_collapsed { "▲" } else { "▼" };
        if ui
            .small_button(hotbar_icon)
            .on_hover_text(state.tr("Show or hide the hotbar", "Показать или скрыть хотбар"))
            .clicked()
        {
            uis.hotbar_collapsed = !uis.hotbar_collapsed;
        }
    });
}

fn level_color(level: f32) -> egui::Color32 {
    if level > 0.97 {
        egui::Color32::from_rgb(210, 60, 60)
    } else if level > 0.85 {
        egui::Color32::from_rgb(220, 170, 40)
    } else {
        egui::Color32::from_rgb(70, 160, 90)
    }
}

fn play_mode_name(state: &AppState, mode: PlayMode) -> String {
    match mode {
        PlayMode::Default => state.tr("Default", "По умолчанию"),
        PlayMode::Speakers => state.tr("Speakers", "Динамики"),
        PlayMode::Microphone => state.tr("Microphone", "Микрофон"),
        PlayMode::Both => state.tr("Speakers + Microphone", "Динамики + микрофон"),
    }
}

// ---------------------------------------------------------------- sidebar

fn draw_sidebar(state: &mut AppState, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.strong(state.tr("Categories", "Категории"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("−")
                .on_hover_text(state.tr("Delete category", "Удалить категорию"))
                .clicked()
            {
                acts.push(Act::DeleteCategory(state.selected_category));
            }
            if ui
                .small_button("✎")
                .on_hover_text(state.tr("Rename category", "Переименовать категорию"))
                .clicked()
            {
                acts.push(Act::RenameCategory(state.selected_category));
            }
            if ui
                .small_button("+")
                .on_hover_text(state.tr("New category", "Новая категория"))
                .clicked()
            {
                acts.push(Act::NewCategory);
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let count = state.list.categories.len();
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
                let icon = category_icon(
                    &state
                        .list
                        .categories
                        .get(i)
                        .map(|c| c.icon.clone())
                        .unwrap_or_default(),
                    &name,
                );
                let color = state.list.categories.get(i).and_then(|c| c.color);
                let sounds = state.list.sounds_for_category(i).len();
                let label = format!("{icon}  {name}  ({sounds})");
                let selected = state.selected_category == i;

                let resp = ui
                    .horizontal(|ui| {
                        if let Some(c) = color {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 4.0, app::rgb_to_egui(c));
                        }
                        ui.selectable_label(selected, label)
                    })
                    .inner;

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
            }
        });
}

fn category_icon(icon: &str, name: &str) -> String {
    if let Some(letter) = icon.strip_prefix("gen_") {
        if let Some(c) = letter.chars().next() {
            return c.to_uppercase().to_string();
        }
    }
    if icon.contains("home") {
        return "⌂".into();
    }
    if icon.contains("list") || icon.contains("cat") {
        return "≡".into();
    }
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "•".into())
}

// ----------------------------------------------------------------- center

fn draw_center(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    let vis = ui.visuals().clone();

    ui.horizontal(|ui| {
        let hint = state.tr(
            "Search title, artist, tag or file…",
            "Поиск по названию, метке, файлу…",
        );
        let search = egui::Id::new("rustpad-search");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .id(search)
                .hint_text(hint)
                .desired_width(280.0),
        );
        uis.search_id = search.value();
        if std::mem::take(&mut uis.search_focus) {
            resp.request_focus();
        }
        if !state.search.is_empty() {
            if ui
                .button("✖")
                .on_hover_text(state.tr("Clear the search", "Очистить поиск"))
                .clicked()
            {
                state.search.clear();
            }
        }
        ui.label(
            egui::RichText::new(format!(
                "{} / {}",
                state.visible_ids().len(),
                state.list.sounds.len()
            ))
            .small(),
        );
    });
    ui.separator();

    let mut ids = state.visible_ids();
    sort_ids(state, &mut ids, uis.sort_col, uis.sort_desc);

    if ids.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(state.tr(
                    "No sounds here yet.\nUse File ▸ Add sound files… or drag files into the window.",
                    "Здесь пока нет звуков.\nИспользуйте «Файл ▸ Добавить звуковые файлы…» или перетащите файлы в окно.",
                ))
                .size(15.0),
            );
            if ui
                .button(state.tr("Add sound files…", "Добавить звуковые файлы…"))
                .clicked()
            {
                acts.push(Act::PickFiles);
            }
        });
        return;
    }

    let selected = state.selected_sound;
    let playing = state.status.sound_id;
    let mut clicked: Option<u32> = None;
    let mut double: Option<u32> = None;

    TableBuilder::new(ui)
        .id_salt("rustpad-sounds")
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(46.0))
        .column(Column::initial(280.0).at_least(120.0).clip(true))
        .column(Column::exact(80.0))
        .column(Column::initial(96.0).at_least(60.0).clip(true))
        .column(Column::exact(70.0))
        .column(Column::initial(100.0).at_least(60.0).clip(true))
        .column(Column::remainder().at_least(110.0).clip(true))
        .header(22.0, |mut header| {
            header.col(|ui| header_cell(ui, uis, SortCol::Index, state.tr("#", "№")));
            header.col(|ui| header_cell(ui, uis, SortCol::Title, state.tr("Title", "Название")));
            header.col(|ui| header_cell(ui, uis, SortCol::Duration, state.tr("Duration", "Время")));
            header.col(|ui| header_cell(ui, uis, SortCol::Hotkey, state.tr("Hotkey", "Клавиша")));
            header.col(|ui| header_cell(ui, uis, SortCol::Plays, state.tr("Plays", "Проигр.")));
            header.col(|ui| header_cell(ui, uis, SortCol::Tag, state.tr("Tag", "Метка")));
            header.col(|ui| header_cell(ui, uis, SortCol::File, state.tr("File", "Файл")));
        })
        .body(|body| {
            body.rows(22.0, ids.len(), |mut row| {
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

                row.col(|ui| {
                    if is_playing {
                        ui.colored_label(vis.hyperlink_color, "▶");
                    } else {
                        ui.weak(format!("{}", index + 1));
                    }
                });
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
                if resp.clicked() || resp.secondary_clicked() {
                    clicked = Some(id);
                }
                if resp.double_clicked() {
                    double = Some(id);
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
                        acts.push(Act::Capture(HotkeyTarget::Sound(id), menu_title.clone()));
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
                            .button(
                                state.tr("Remove from this category", "Убрать из этой категории"),
                            )
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
                                    egui::RichText::new(text).background_color(app::rgb_to_egui(c)),
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

    if let Some(id) = clicked {
        state.selected_sound = Some(id);
    }
    if let Some(id) = double {
        state.play_sound(id, state.play_mode);
    }
}

fn header_cell(ui: &mut egui::Ui, uis: &mut UiState, col: SortCol, label: String) {
    let arrow = if uis.sort_col == col {
        if uis.sort_desc {
            " ▼"
        } else {
            " ▲"
        }
    } else {
        ""
    };
    if ui
        .add(egui::Button::new(format!("{label}{arrow}")).frame(false))
        .clicked()
    {
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

fn draw_hotbar(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    if state.list.hotbar.is_empty() {
        state.new_hotbar_page();
    }
    ui.horizontal_wrapped(|ui| {
        for pi in 0..state.list.hotbar.len() {
            let title = state
                .list
                .hotbar
                .get(pi)
                .map(|p| p.title.clone())
                .unwrap_or_default();
            let label = hotbar_page_title(state, pi, &title);
            if ui.selectable_label(uis.hotbar_page == pi, label).clicked() {
                uis.hotbar_page = pi;
            }
        }
        if ui
            .small_button("+")
            .on_hover_text(state.tr("New page", "Новая страница"))
            .clicked()
        {
            acts.push(Act::NewHotbarPage);
        }
        let page_now = uis
            .hotbar_page
            .min(state.list.hotbar.len().saturating_sub(1));
        if ui
            .add_enabled(state.list.hotbar.len() > 1, egui::Button::new("−").small())
            .on_hover_text(state.tr("Remove page", "Удалить страницу"))
            .clicked()
        {
            acts.push(Act::RemoveHotbarPage(page_now));
        }
    });

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

    egui::ScrollArea::vertical()
        .max_height(220.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if items.is_empty() {
                ui.weak(state.tr(
                    "This hotbar page is empty. Add the selected sound below or use a sound's context menu.",
                    "Страница хотбара пуста. Добавьте выбранный звук ниже или через меню звука.",
                ));
            } else {
                let spacing = 6.0;
                let cell_w = ((ui.available_width() - 5.0 * spacing) / 6.0).max(90.0);
                egui::Grid::new("rustpad-hotbar-grid")
                    .num_columns(6)
                    .spacing([spacing, spacing])
                    .show(ui, |ui| {
                        for (n, (idx, sound_id, label, color, hk)) in items.iter().enumerate() {
                            let title = label
                                .clone()
                                .filter(|s| !s.trim().is_empty())
                                .or_else(|| state.list.find(*sound_id).map(|s| s.title.clone()))
                                .unwrap_or_else(|| format!("#{sound_id}"));
                            let hk_text = hk.map(|h| h.display()).unwrap_or_default();
                            let subtitle = if hk_text.is_empty() {
                                "—".to_string()
                            } else {
                                truncate(&hk_text, 18)
                            };
                            let text = format!("{}\n{}", truncate(&title, 24), subtitle);
                            let mut button = egui::Button::new(text)
                                .min_size(egui::vec2(cell_w, 40.0));
                            if let Some(c) = color {
                                button = button.fill(app::rgb_to_egui(*c));
                            }
                            let resp = ui.add(button);
                            if resp.clicked() {
                                acts.push(Act::Play(*sound_id, state.play_mode));
                            }
                            let menu_title = title.clone();
                            resp.context_menu(|ui| {
                                if ui
                                    .button(state.tr("Remove from hotbar", "Убрать из хотбара"))
                                    .clicked()
                                {
                                    ui.close_menu();
                                    acts.push(Act::RemoveHotbarItem(page, *idx));
                                }
                                if ui
                                    .button(state.tr("Set hotkey…", "Назначить клавишу…"))
                                    .clicked()
                                {
                                    ui.close_menu();
                                    acts.push(Act::Capture(
                                        HotkeyTarget::Hotbar { page, item: *idx },
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
                                            acts.push(Act::SetHotbarColor(page, *idx, Some(c)));
                                        }
                                    }
                                    if ui.button(state.tr("No color", "Без цвета")).clicked() {
                                        ui.close_menu();
                                        acts.push(Act::SetHotbarColor(page, *idx, None));
                                    }
                                });
                            });
                            if (n + 1) % 6 == 0 {
                                ui.end_row();
                            }
                        }
                        if items.len() % 6 != 0 {
                            ui.end_row();
                        }
                    });
            }
        });

    ui.horizontal(|ui| {
        let add_label = state.tr("Add selected sound", "Добавить выбранный звук");
        let has_selection = state.selected_sound.is_some();
        if ui
            .add_enabled(has_selection, egui::Button::new(add_label))
            .clicked()
        {
            if let Some(id) = state.selected_sound {
                acts.push(Act::AddToHotbar(page, id));
            }
        }
        if ui.button(state.tr("New page", "Новая страница")).clicked() {
            acts.push(Act::NewHotbarPage);
        }
        if ui
            .add_enabled(
                state.list.hotbar.len() > 1,
                egui::Button::new(state.tr("Remove page", "Удалить страницу")),
            )
            .clicked()
        {
            acts.push(Act::RemoveHotbarPage(page));
        }
    });
}

// ------------------------------------------------------------- transport

fn draw_transport(state: &mut AppState, ui: &mut egui::Ui, uis: &mut UiState, acts: &mut Vec<Act>) {
    let Some(id) = state.status.sound_id else {
        return;
    };
    let title = state
        .list
        .find(id)
        .map(|s| s.title.clone())
        .unwrap_or_else(|| format!("#{id}"));
    let pos = state.status.position_ms;
    let dur = state.status.duration_ms.max(1);
    let paused = state.status.paused;
    let voices = state.status.voices;
    let mut seek = uis
        .seek_drag
        .unwrap_or_else(|| pos.min(dur) as f32)
        .clamp(0.0, dur as f32);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.label(format!(
            "{} / {}",
            format_duration(pos),
            format_duration(dur)
        ));
        if voices > 1 {
            ui.weak(format!("{} {}", voices, state.tr("voices", "голоса")));
        }
        ui.spacing_mut().slider_width = (ui.available_width() - 230.0).clamp(80.0, 900.0);
        let resp = ui.add(egui::Slider::new(&mut seek, 0.0..=dur as f32).show_value(false));
        if resp.dragged() {
            uis.seek_drag = Some(seek);
        } else if resp.drag_stopped() {
            uis.seek_drag = None;
            acts.push(Act::Seek(seek as u64));
        } else if resp.changed() {
            acts.push(Act::Seek(seek as u64));
        }
        if ui.button(state.tr("Stop", "Стоп")).clicked() {
            acts.push(Act::Stop);
        }
        let pause_text = if paused {
            state.tr("Resume", "Продолжить")
        } else {
            state.tr("Pause", "Пауза")
        };
        if ui.button(pause_text).clicked() {
            acts.push(Act::Pause);
        }
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
        .default_width(600.0)
        .default_height(520.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
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
    ui.small(state.tr(
        "Choose a virtual cable input (e.g. CABLE Input) so other apps hear the sounds.",
        "Выберите вход виртуального кабеля (например, CABLE Input), чтобы звуки слышали другие программы.",
    ));

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

fn draw_toasts(state: &mut AppState, ctx: &egui::Context) {
    if state.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("rustpad-toasts"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -14.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                let total = state.toasts.len();
                let skip = total.saturating_sub(4);
                for toast in state.toasts.iter().skip(skip) {
                    let fill = if toast.error {
                        egui::Color32::from_rgb(140, 40, 40)
                    } else {
                        ui.visuals().window_fill
                    };
                    let text_color = if toast.error {
                        egui::Color32::from_rgb(255, 235, 235)
                    } else {
                        ui.visuals().text_color()
                    };
                    egui::Frame::popup(ui.style())
                        .fill(fill)
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.colored_label(text_color, toast.text.clone());
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
                app::apply_engine_settings(state);
                state.toast(
                    state.tr("Audio devices rescanned", "Список устройств обновлён"),
                    false,
                );
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

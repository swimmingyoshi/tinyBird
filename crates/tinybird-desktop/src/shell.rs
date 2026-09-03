//! The classic application shell: menu bar, status bar, and command dispatch.
//!
//! This is the conventional emulator frame that sits *around* the stream
//! dashboard rather than replacing it. Every feature reachable by a function
//! key is also reachable here by mouse, which is what makes the frontend
//! discoverable — previously the only way to learn the controls was the README.
//!
//! The module is a child of the crate root so it can reach `App`'s private
//! fields directly. Phase 7 of `docs/ARCHITECTURE.md` moves `App` itself into
//! an `app/` module tree; until then this keeps the shell out of `main.rs`.

use std::path::PathBuf;

use crate::settings::SAVE_STATE_SLOTS;
use crate::ui::input::{MouseButton as UiMouseButton, UiInput, UiKey};
use crate::ui::menubar::{self, Menu, MenuBarState, MenuEntry, UiCommand};
use crate::ui::theme::{self, Palette, THEME_COUNT};
use crate::ui::widget::Ui;
use crate::ui::{
    Align, Canvas, ChromeMode, Rect, CHROME_REVEAL_BAND, MENU_BAR_HEIGHT, STATUS_BAR_HEIGHT,
};
use crate::{overlay, App, SaveStateLoadResult};

/// Speed multipliers offered in `Emulation > Speed`.
const SPEED_CHOICES: [u32; 3] = [1, 2, 4];
/// Window scales offered in `Video > Window Scale`.
const SCALE_CHOICES: [u32; 4] = [1, 2, 3, 4];
/// Volume steps offered in `Audio > Volume`.
const VOLUME_CHOICES: [u32; 6] = [0, 20, 40, 60, 80, 100];

/// Where the shell puts its bars and how much room is left for content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromeLayout {
    pub menu_bar: Rect,
    /// The region the game viewport and dashboard may use.
    pub content: Rect,
    pub status_bar: Rect,
}

impl ChromeLayout {
    /// Split a window into bars and content.
    ///
    /// When either bar is hidden its rect is empty and the content grows into
    /// it, so callers never need to special-case the chrome being off.
    pub fn compute(window: Rect, show_menu_bar: bool, show_status_bar: bool) -> Self {
        // A window too small to hold the bars keeps the content instead; a
        // zero-height viewport is worse than missing chrome.
        let can_fit = window.h > MENU_BAR_HEIGHT + STATUS_BAR_HEIGHT + 32;
        let menu_h = if show_menu_bar && can_fit {
            MENU_BAR_HEIGHT
        } else {
            0
        };
        let status_h = if show_status_bar && can_fit {
            STATUS_BAR_HEIGHT
        } else {
            0
        };

        let (menu_bar, rest) = window.split_top(menu_h);
        let (content, status_bar) = rest.split_bottom(status_h);
        Self {
            menu_bar,
            content,
            status_bar,
        }
    }
}

impl App {
    // ---------------------------------------------------------------- chrome

    pub(crate) fn chrome_mode(&self) -> ChromeMode {
        self.settings.get().ui.chrome
    }

    /// Whether the chrome should be drawn this frame.
    pub(crate) fn chrome_visible(&self) -> bool {
        match self.chrome_mode() {
            ChromeMode::Always => true,
            ChromeMode::Off => false,
            // Auto-hide stays up while a menu is open, otherwise the menu would
            // vanish out from under the pointer mid-navigation.
            ChromeMode::Auto => self.chrome_revealed || self.menu_state.is_open(),
        }
    }

    pub(crate) fn chrome_layout(&self, win_w: usize, win_h: usize) -> ChromeLayout {
        let window = Rect::new(0, 0, win_w as i32, win_h as i32);
        if !self.chrome_visible() {
            return ChromeLayout::compute(window, false, false);
        }
        ChromeLayout::compute(window, true, self.settings.get().ui.show_status_bar)
    }

    /// Update auto-hide reveal state from the pointer position.
    fn update_chrome_reveal(&mut self) {
        if self.chrome_mode() != ChromeMode::Auto {
            self.chrome_revealed = false;
            return;
        }
        let Some((_, y)) = self.ui_input.pointer() else {
            self.chrome_revealed = false;
            return;
        };
        // Reveal near the top edge; keep it up while the pointer is over the bar
        // itself so it does not flicker as the pointer travels down into it.
        let threshold = if self.chrome_revealed {
            MENU_BAR_HEIGHT
        } else {
            CHROME_REVEAL_BAND
        };
        self.chrome_revealed = y <= threshold;
    }

    // ------------------------------------------------------------ menu model

    /// Build the menu tree from current state.
    ///
    /// Rebuilt every frame on purpose: check marks, enabled states, and the
    /// recent-ROM list are all derived, so there is nothing to keep in sync.
    pub(crate) fn build_menus(&self) -> Vec<Menu> {
        vec![
            self.file_menu(),
            self.emulation_menu(),
            self.video_menu(),
            self.audio_menu(),
            self.addons_menu(),
            self.tools_menu(),
            self.help_menu(),
        ]
    }

    fn file_menu(&self) -> Menu {
        let slots = self.save_state_slot_statuses();
        let recent = &self.settings.get().recent_roms;

        let recent_entries: Vec<MenuEntry> = if recent.is_empty() {
            vec![MenuEntry::action("(none)", UiCommand::ClearRecentRoms).enabled(false)]
        } else {
            recent
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let label = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    MenuEntry::action(App::short_label(&label, 40), UiCommand::OpenRecent(index))
                })
                .chain([
                    MenuEntry::separator(),
                    MenuEntry::action("Clear List", UiCommand::ClearRecentRoms),
                ])
                .collect()
        };

        let save_entries: Vec<MenuEntry> = (1..=SAVE_STATE_SLOTS)
            .map(|slot| {
                let used = slots[slot as usize - 1];
                MenuEntry::action(
                    format!("Slot {slot}{}", if used { " (in use)" } else { "" }),
                    UiCommand::SaveStateSlot(slot),
                )
                .enabled(self.rom_loaded)
            })
            .collect();

        let load_entries: Vec<MenuEntry> = (1..=SAVE_STATE_SLOTS)
            .map(|slot| {
                let used = slots[slot as usize - 1];
                MenuEntry::action(
                    format!("Slot {slot}{}", if used { "" } else { " (empty)" }),
                    UiCommand::LoadStateSlot(slot),
                )
                .enabled(self.rom_loaded && used)
            })
            .collect();

        Menu::new(
            "File",
            vec![
                MenuEntry::action("Open ROM...", UiCommand::OpenRom).shortcut("Ctrl+O"),
                MenuEntry::submenu("Open Recent", recent_entries),
                MenuEntry::action("Reload ROM", UiCommand::ReloadRom).enabled(self.rom_loaded),
                MenuEntry::separator(),
                MenuEntry::submenu("Save State", save_entries),
                MenuEntry::submenu("Load State", load_entries),
                MenuEntry::action("Save State Manager...", UiCommand::OpenSaveStateManager)
                    .shortcut("F5")
                    .enabled(self.rom_loaded),
                MenuEntry::action("Load State Manager...", UiCommand::OpenLoadStateManager)
                    .shortcut("F8")
                    .enabled(self.rom_loaded),
                MenuEntry::separator(),
                MenuEntry::action("Take Screenshot", UiCommand::TakeScreenshot)
                    .enabled(self.rom_loaded),
                MenuEntry::action("Open Data Folder", UiCommand::OpenDataFolder),
                MenuEntry::separator(),
                MenuEntry::action("Exit", UiCommand::Exit),
            ],
        )
    }

    fn emulation_menu(&self) -> Menu {
        let paused = self.gba.state == tinybird_core::GbaState::Paused;
        let speed_entries = SPEED_CHOICES
            .iter()
            .map(|&speed| {
                MenuEntry::action(format!("{speed}x"), UiCommand::SetSpeed(speed))
                    .radio(self.speed_multiplier == speed)
            })
            .collect();

        Menu::new(
            "Emulation",
            vec![
                MenuEntry::action(
                    if paused { "Resume" } else { "Pause" },
                    UiCommand::TogglePause,
                )
                .shortcut("Esc")
                .enabled(self.rom_loaded),
                MenuEntry::action("Reset", UiCommand::Reset)
                    .shortcut("R")
                    .enabled(self.rom_loaded),
                MenuEntry::separator(),
                MenuEntry::submenu("Speed", speed_entries),
                MenuEntry::action("Fast Forward (hold)", UiCommand::SetSpeed(4))
                    .shortcut("Tab")
                    .enabled(self.rom_loaded),
            ],
        )
    }

    fn video_menu(&self) -> Menu {
        let video = &self.settings.get().video;
        let scale_entries = SCALE_CHOICES
            .iter()
            .map(|&scale| {
                MenuEntry::action(format!("{scale}x"), UiCommand::SetWindowScale(scale))
                    .radio(video.window_scale == scale)
            })
            .collect();

        Menu::new(
            "Video",
            vec![
                MenuEntry::action("Fullscreen", UiCommand::ToggleFullscreen)
                    .shortcut("F9")
                    .checked(self.fullscreen),
                MenuEntry::separator(),
                MenuEntry::submenu("Window Scale", scale_entries),
                MenuEntry::action("Integer Scaling", UiCommand::ToggleIntegerScaling)
                    .checked(video.integer_scaling),
                MenuEntry::separator(),
                MenuEntry::action("LCD Color Correction", UiCommand::ToggleColorCorrection)
                    .shortcut("C")
                    .checked(self.color_correction),
                MenuEntry::action("Show HUD", UiCommand::ToggleHud)
                    .shortcut("F1")
                    .checked(self.show_overlay),
            ],
        )
    }

    fn audio_menu(&self) -> Menu {
        let current = self.settings.get().volume_percent();
        let volume_entries = VOLUME_CHOICES
            .iter()
            .map(|&percent| {
                MenuEntry::action(format!("{percent}%"), UiCommand::SetVolumePercent(percent))
                    // Nearest step wins, so a volume nudged with `[` / `]` still
                    // shows a sensible mark rather than none at all.
                    .radio(current.abs_diff(percent) < 10)
            })
            .collect();

        Menu::new(
            "Audio",
            vec![
                MenuEntry::action("Mute", UiCommand::ToggleMute)
                    .shortcut("M")
                    .checked(self.muted),
                MenuEntry::separator(),
                MenuEntry::action("Volume Up", UiCommand::VolumeUp).shortcut("]"),
                MenuEntry::action("Volume Down", UiCommand::VolumeDown).shortcut("["),
                MenuEntry::submenu("Volume", volume_entries),
            ],
        )
    }

    fn addons_menu(&self) -> Menu {
        let layout_entries = (0..overlay::DASHBOARD_LAYOUT_COUNT)
            .map(|layout| {
                MenuEntry::action(
                    overlay::dashboard_layout_label(layout),
                    UiCommand::SetDashboardLayout(layout),
                )
                .radio(self.dashboard_layout == layout)
            })
            .collect();
        let size_entries = (0..overlay::GAME_PREVIEW_SIZE_COUNT)
            .map(|size| {
                MenuEntry::action(
                    overlay::game_preview_size_label(size),
                    UiCommand::SetGameSize(size),
                )
                .radio(self.game_preview_size == size)
            })
            .collect();
        let side_entries = (0..overlay::SIDE_PANEL_SIZE_COUNT)
            .map(|size| {
                MenuEntry::action(
                    overlay::side_panel_size_label(size),
                    UiCommand::SetSidePanelWidth(size),
                )
                .radio(self.side_panel_size == size)
            })
            .collect();
        let theme_entries = (0..THEME_COUNT)
            .map(|index| {
                MenuEntry::action(theme::theme_label(index), UiCommand::SetTheme(index))
                    .radio(self.dashboard_theme == index)
            })
            .collect();

        Menu::new(
            "Addons",
            vec![
                MenuEntry::action("Dashboard", UiCommand::ToggleDashboard)
                    .shortcut("F6")
                    .checked(self.addon_panel_expanded),
                MenuEntry::action("Team Panel", UiCommand::ShowTeamPanel).shortcut("F2"),
                MenuEntry::action("Encounters Panel", UiCommand::ShowEncountersPanel)
                    .shortcut("F4"),
                MenuEntry::action("Hide Addon UI", UiCommand::HideAddonUi)
                    .shortcut("F3")
                    .enabled(self.show_addon_panel),
                MenuEntry::separator(),
                MenuEntry::submenu("Dashboard Layout", layout_entries),
                MenuEntry::submenu("Game Size", size_entries),
                MenuEntry::submenu("Side Panel Width", side_entries),
                MenuEntry::submenu("Theme", theme_entries),
                MenuEntry::action("Choose Wallpaper...", UiCommand::ChooseWallpaper).shortcut("W"),
                MenuEntry::separator(),
                MenuEntry::action("Export Snapshot JSON", UiCommand::ToggleSnapshotExport)
                    .checked(self.settings.get().dashboard.export_snapshot),
            ],
        )
    }

    fn tools_menu(&self) -> Menu {
        let chrome = self.chrome_mode();
        let chrome_entries = ChromeMode::ALL
            .iter()
            .map(|&mode| {
                MenuEntry::action(mode.label(), UiCommand::SetChromeMode(mode))
                    .radio(chrome == mode)
            })
            .collect();

        Menu::new(
            "Tools",
            vec![
                MenuEntry::action("Settings...", UiCommand::OpenSettings).shortcut("Ctrl+,"),
                MenuEntry::submenu("Menu Bar", chrome_entries),
                MenuEntry::separator(),
                MenuEntry::action("ROM Information...", UiCommand::ShowRomInfo)
                    .enabled(self.rom_loaded),
                MenuEntry::action("Addon Status...", UiCommand::ShowAddonStatus),
            ],
        )
    }

    fn help_menu(&self) -> Menu {
        Menu::new(
            "Help",
            vec![
                MenuEntry::action("Controls...", UiCommand::ShowControls),
                MenuEntry::action("About tinyBird...", UiCommand::ShowAbout),
            ],
        )
    }

    // -------------------------------------------------------- command dispatch

    /// Run one UI command. Every input path — menu, shortcut, dialog — funnels
    /// through here, so behaviour cannot drift between them.
    pub(crate) fn run_command(&mut self, command: UiCommand) {
        match command {
            // --- File ---
            UiCommand::OpenRom => self.open_rom(),
            UiCommand::OpenRecent(index) => {
                if let Some(path) = self.settings.get().recent_roms.get(index).cloned() {
                    if path.is_file() {
                        self.load_rom_from_path(path);
                    } else {
                        self.set_status("That ROM is no longer there", overlay::ToastTone::Warning);
                    }
                }
            }
            UiCommand::ClearRecentRoms => {
                self.settings.edit().clear_recent_roms();
                self.set_status("Recent ROM list cleared", overlay::ToastTone::Info);
            }
            UiCommand::ReloadRom => {
                if let Some(path) = self.rom_path.clone() {
                    self.load_rom_from_path(path);
                }
            }
            UiCommand::SaveStateSlot(slot) => self.write_save_state_slot(slot),
            UiCommand::LoadStateSlot(slot) => {
                let result = self.try_load_save_state_slot(slot);
                self.report_state_load(slot, result);
            }
            UiCommand::OpenSaveStateManager => {
                self.open_save_state_menu(overlay::SaveStateMenuMode::Save)
            }
            UiCommand::OpenLoadStateManager => {
                self.open_save_state_menu(overlay::SaveStateMenuMode::Load)
            }
            UiCommand::TakeScreenshot => self.take_screenshot(),
            UiCommand::OpenDataFolder => self.open_data_folder(),
            UiCommand::Exit => self.exit_requested = true,

            // --- Emulation ---
            UiCommand::TogglePause => self.toggle_pause(),
            UiCommand::Reset => {
                self.gba.reset();
                self.clear_audio_output();
                self.reset_timing_state();
                self.refresh_window_title();
                self.refresh_game_addon_state(true);
                self.set_status("ROM reset", overlay::ToastTone::Info);
            }
            UiCommand::SetSpeed(speed) => {
                self.set_speed_multiplier(speed);
                self.set_status(format!("Speed set to {speed}x"), overlay::ToastTone::Info);
            }

            // --- Video ---
            UiCommand::ToggleFullscreen => self.toggle_fullscreen(),
            UiCommand::SetWindowScale(scale) => self.set_window_scale(scale),
            UiCommand::ToggleIntegerScaling => {
                let next = !self.settings.get().video.integer_scaling;
                self.settings.edit().video.integer_scaling = next;
                self.set_status(
                    if next {
                        "Integer scaling on"
                    } else {
                        "Integer scaling off"
                    },
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::ToggleColorCorrection => {
                self.color_correction = !self.color_correction;
                self.settings.edit().video.color_correction = self.color_correction;
                self.set_status(
                    if self.color_correction {
                        "LCD color correction on"
                    } else {
                        "LCD color correction off"
                    },
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::ToggleHud => {
                self.show_overlay = !self.show_overlay;
                self.settings.edit().video.show_hud = self.show_overlay;
            }

            // --- Audio ---
            UiCommand::ToggleMute => {
                self.muted = !self.muted;
                self.settings.edit().audio.muted = self.muted;
                self.update_audio_emulation_state();
                self.set_status(
                    if self.muted {
                        "Audio muted"
                    } else {
                        "Audio unmuted"
                    },
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::SetVolumePercent(percent) => {
                self.apply_volume(percent as f32 / 100.0);
            }
            UiCommand::VolumeUp => {
                let volume = self.volume + 0.1;
                self.apply_volume(volume);
            }
            UiCommand::VolumeDown => {
                let volume = self.volume - 0.1;
                self.apply_volume(volume);
            }

            // --- Addons ---
            UiCommand::ToggleDashboard => self.toggle_addon_dashboard(),
            UiCommand::ShowTeamPanel => self.show_docked_addon_view(overlay::AddonViewMode::Team),
            UiCommand::ShowEncountersPanel => {
                self.show_docked_addon_view(overlay::AddonViewMode::Encounters)
            }
            UiCommand::HideAddonUi => self.hide_addon_ui(),
            UiCommand::SetDashboardLayout(layout) => {
                self.dashboard_layout = layout % overlay::DASHBOARD_LAYOUT_COUNT;
                self.settings.edit().dashboard.layout = self.dashboard_layout;
                self.set_status(
                    format!(
                        "Dashboard: {}",
                        overlay::dashboard_layout_label(self.dashboard_layout)
                    ),
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::SetGameSize(size) => {
                self.game_preview_size = size % overlay::GAME_PREVIEW_SIZE_COUNT;
                self.settings.edit().dashboard.game_size = self.game_preview_size;
                self.set_status(
                    format!(
                        "Game size: {}",
                        overlay::game_preview_size_label(self.game_preview_size)
                    ),
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::SetSidePanelWidth(size) => {
                self.side_panel_size = size % overlay::SIDE_PANEL_SIZE_COUNT;
                self.settings.edit().dashboard.side_panel_size = self.side_panel_size;
                self.set_status(
                    format!(
                        "Side panel: {}",
                        overlay::side_panel_size_label(self.side_panel_size)
                    ),
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::SetTheme(index) => {
                self.dashboard_theme = index % THEME_COUNT;
                self.settings.edit().dashboard.theme = self.dashboard_theme;
                self.set_status(
                    format!("Theme: {}", theme::theme_label(self.dashboard_theme)),
                    overlay::ToastTone::Info,
                );
            }
            UiCommand::ChooseWallpaper => self.choose_dashboard_wallpaper(),
            UiCommand::ToggleSnapshotExport => {
                let next = !self.settings.get().dashboard.export_snapshot;
                self.settings.edit().dashboard.export_snapshot = next;
                self.set_status(
                    if next {
                        "Snapshot export on"
                    } else {
                        "Snapshot export off"
                    },
                    overlay::ToastTone::Info,
                );
            }

            // --- Tools ---
            UiCommand::OpenSettings => {
                // The full tabbed dialog is phase 5; until it lands, point at
                // the file so the setting is still reachable.
                match crate::settings::settings_path() {
                    Some(path) => self.set_status(
                        format!("Settings file: {}", path.display()),
                        overlay::ToastTone::Info,
                    ),
                    None => self
                        .set_status("No config directory available", overlay::ToastTone::Warning),
                }
            }
            UiCommand::ShowRomInfo => {
                let info = match self.addon_snapshot.rom.as_ref() {
                    Some(rom) => format!(
                        "{} [{}] {} rev {} - save: {}",
                        rom.title,
                        rom.game_code,
                        rom.region_name(),
                        rom.revision,
                        // A missing or wrong backup type is the usual reason a
                        // new game boots but cannot save, so surface it here.
                        self.gba.bus.save_type_label()
                    ),
                    None => "No ROM header available".to_string(),
                };
                println!("ROM information: {info}");
                self.set_status(info, overlay::ToastTone::Info);
            }
            UiCommand::ShowAddonStatus => {
                let status =
                    tinybird_games::describe_addon_status(self.rom_loaded.then_some(&*self.gba));

                // The toast fits one line; the full report goes to stdout,
                // where it is the first thing to check when bringing up a game.
                println!("Addon status: {}", status.detection_line);
                println!("  registered:");
                for info in &status.registered {
                    println!(
                        "    {:<20} {:<8} {}",
                        info.addon_id, info.version, info.supported_games
                    );
                }
                println!("  claiming this ROM:");
                if status.matching.is_empty() {
                    println!("    (none)");
                }
                for info in &status.matching {
                    println!("    {}", info.addon_id);
                }

                self.set_status(
                    format!(
                        "{} - {} of {} addons claim this ROM",
                        status.detection_line,
                        status.matching.len(),
                        status.registered.len()
                    ),
                    overlay::ToastTone::Info,
                );
            }

            // --- Help ---
            UiCommand::ShowControls => self.set_status(
                "Arrows move, Z/X = A/B, A/S = L/R, Enter = Start, Space = Select",
                overlay::ToastTone::Info,
            ),
            UiCommand::ShowAbout => self.set_status(
                concat!("tinyBird ", env!("CARGO_PKG_VERSION"), " - GBA emulator"),
                overlay::ToastTone::Info,
            ),

            // --- Chrome ---
            UiCommand::SetChromeMode(mode) => {
                self.settings.edit().ui.chrome = mode;
                self.chrome_revealed = false;
                self.set_status(
                    format!("Menu bar: {}", mode.label()),
                    overlay::ToastTone::Info,
                );
            }
        }

        self.request_redraw();
    }

    fn report_state_load(&mut self, slot: u8, result: SaveStateLoadResult) {
        match result {
            SaveStateLoadResult::Loaded => {
                self.set_status(format!("Loaded slot {slot}"), overlay::ToastTone::Success)
            }
            SaveStateLoadResult::Missing => {
                self.set_status(format!("Slot {slot} is empty"), overlay::ToastTone::Warning)
            }
            SaveStateLoadResult::Failed => self.set_status(
                format!("Could not load slot {slot}"),
                overlay::ToastTone::Warning,
            ),
        }
    }

    fn toggle_pause(&mut self) {
        match self.gba.state {
            tinybird_core::GbaState::Running => {
                self.gba.pause();
                self.set_status("Paused", overlay::ToastTone::Info);
            }
            tinybird_core::GbaState::Paused => {
                self.gba.start();
                self.set_status("Resumed", overlay::ToastTone::Info);
            }
            _ => return,
        }
        self.clear_audio_output();
        self.reset_timing_state();
        self.refresh_window_title();
    }

    fn apply_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.settings.edit().audio.volume = self.volume;
        if !self.muted {
            if let Some(audio_handler) = &self.audio_handler {
                audio_handler.set_volume(self.volume);
            }
        }
        self.set_status(
            format!("Volume {}%", (self.volume * 100.0).round() as u32),
            overlay::ToastTone::Info,
        );
    }

    fn set_window_scale(&mut self, scale: u32) {
        let scale = scale.clamp(1, 8);
        self.settings.edit().video.window_scale = scale;
        if self.fullscreen {
            self.set_status(
                format!("Window scale {scale}x applies when leaving fullscreen"),
                overlay::ToastTone::Info,
            );
            return;
        }
        if let Some(window) = &self.window {
            let size = winit::dpi::LogicalSize::new(
                crate::SCREEN_WIDTH * scale,
                crate::SCREEN_HEIGHT * scale,
            );
            let _ = window.request_inner_size(size);
        }
        self.set_status(format!("Window scale {scale}x"), overlay::ToastTone::Info);
    }

    fn take_screenshot(&mut self) {
        let Some(path) = self.screenshot_path() else {
            self.set_status(
                "No screenshot folder available",
                overlay::ToastTone::Warning,
            );
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("Failed to create screenshot folder: {err}");
                self.set_status(
                    "Could not create screenshot folder",
                    overlay::ToastTone::Warning,
                );
                return;
            }
        }

        let framebuffer = self.gba.ppu.get_framebuffer();
        let lookup = crate::rgb555_lookup(self.color_correction);
        let mut rgb =
            Vec::with_capacity(crate::SCREEN_WIDTH as usize * crate::SCREEN_HEIGHT as usize * 3);
        for pixel in framebuffer.as_slice() {
            let color = lookup[pixel.color.to_rgb555() as usize];
            rgb.push(((color >> 16) & 0xFF) as u8);
            rgb.push(((color >> 8) & 0xFF) as u8);
            rgb.push((color & 0xFF) as u8);
        }

        let saved = image::RgbImage::from_raw(crate::SCREEN_WIDTH, crate::SCREEN_HEIGHT, rgb)
            .ok_or_else(|| "framebuffer size mismatch".to_string())
            .and_then(|image| image.save(&path).map_err(|err| err.to_string()));

        match saved {
            Ok(()) => self.set_status(
                format!(
                    "Saved {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                overlay::ToastTone::Success,
            ),
            Err(err) => {
                eprintln!("Failed to write screenshot '{}': {err}", path.display());
                self.set_status("Could not save screenshot", overlay::ToastTone::Warning);
            }
        }
    }

    fn screenshot_path(&self) -> Option<PathBuf> {
        let dir = self
            .settings
            .get()
            .paths
            .screenshot_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("stream-data/screenshots"));
        let stem = self
            .rom_title
            .as_deref()
            .unwrap_or("tinybird")
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>();
        // Wall-clock naming would need another dependency; a monotonic counter
        // derived from the emulated frame count is enough to stay unique.
        let stamp = self.gba.frame_count;
        Some(dir.join(format!("{stem}-{stamp}.png")))
    }

    fn open_data_folder(&mut self) {
        let path = PathBuf::from("stream-data");
        if let Err(err) = std::fs::create_dir_all(&path) {
            eprintln!("Failed to create data folder: {err}");
        }
        let opened = std::process::Command::new(if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        })
        .arg(&path)
        .spawn();

        match opened {
            // `explorer` exits non-zero even on success, so only a spawn failure
            // is worth reporting.
            Ok(_) => self.set_status("Opened data folder", overlay::ToastTone::Info),
            Err(err) => {
                eprintln!("Failed to open data folder: {err}");
                self.set_status(
                    format!("Data folder: {}", path.display()),
                    overlay::ToastTone::Info,
                );
            }
        }
    }

    // ------------------------------------------------------------- input glue

    /// Feed a pointer position, in physical pixels, to the UI.
    pub(crate) fn on_cursor_moved(&mut self, x: f64, y: f64) {
        self.ui_input
            .set_pointer(x.round() as i32, y.round() as i32);
        self.update_chrome_reveal();
        if self.chrome_visible() || self.menu_state.is_open() {
            self.request_redraw();
        }
    }

    pub(crate) fn on_cursor_left(&mut self) {
        self.ui_input.clear_pointer();
        self.chrome_revealed = false;
        self.request_redraw();
    }

    pub(crate) fn on_mouse_button(&mut self, button: UiMouseButton, pressed: bool) {
        self.ui_input.set_button(button, pressed);
        self.request_redraw();
    }

    pub(crate) fn on_mouse_wheel(&mut self, delta: f32) {
        self.ui_input.add_wheel(delta);
        self.request_redraw();
    }

    /// Give the menu bar first refusal on a key press.
    ///
    /// Returns `true` when the shell consumed it, which is what stops an open
    /// menu from leaking arrow keys into the running game.
    pub(crate) fn shell_handle_key(&mut self, key: UiKey) -> bool {
        let was_open = self.menu_state.is_open();
        let menus = self.build_menus();
        let outcome = menubar::handle_key(&mut self.menu_state, &menus, key);

        // Opening a menu releases every held GBA button, so a direction held
        // when the menu appears does not keep steering the game underneath.
        if !was_open && self.menu_state.is_open() {
            self.release_all_buttons();
        }

        if let Some(command) = outcome.command {
            self.run_command(command);
        }
        if outcome.handled {
            self.request_redraw();
        }
        outcome.handled
    }

    /// Drop all held input. Used whenever the UI takes over from the game.
    pub(crate) fn release_all_buttons(&mut self) {
        self.keyboard_buttons = tinybird_core::GbaButton::empty();
        self.sync_input_state();
    }

    /// Whether the shell currently owns keyboard and pointer input.
    pub(crate) fn shell_capturing_input(&self) -> bool {
        self.menu_state.is_open()
    }

    // ---------------------------------------------------------------- drawing

    /// Snapshot everything the chrome needs to draw, or `None` when it is off.
    ///
    /// Building this up front means the presentation path can hold a mutable
    /// borrow of the surface and the menu state at the same time, which a
    /// `&self` draw method could not.
    pub(crate) fn build_chrome_frame(&self) -> Option<ChromeFrameData> {
        let (win_w, win_h) = self.surface_size;
        let layout = self.chrome_layout(win_w as usize, win_h as usize);
        if layout.menu_bar.is_empty() && layout.status_bar.is_empty() {
            return None;
        }

        Some(ChromeFrameData {
            layout,
            palette: *theme::palette(self.dashboard_theme),
            menus: self.build_menus(),
            status: self.status_bar_text(),
        })
    }

    /// Commands bound to modifier shortcuts.
    ///
    /// Only `Ctrl`-modified keys are handled here; plain keys stay with the
    /// legacy `handle_key` path so existing muscle memory keeps working.
    pub(crate) fn shortcut_command(&self, key: &winit::keyboard::Key) -> Option<UiCommand> {
        if !self.ui_input.modifiers().ctrl {
            return None;
        }
        let winit::keyboard::Key::Character(text) = key else {
            return None;
        };
        match text.as_str() {
            "o" | "O" => Some(UiCommand::OpenRom),
            "r" | "R" => Some(UiCommand::Reset),
            "s" | "S" => Some(UiCommand::TakeScreenshot),
            "," => Some(UiCommand::OpenSettings),
            _ => None,
        }
    }

    /// Flush anything that must survive the process exiting.
    pub(crate) fn shutdown(&mut self) {
        self.flush_battery_save(true);
        self.settings.flush();
    }

    /// The four status-bar fields: ROM, performance, audio, save slot.
    fn status_bar_text(&self) -> [String; 4] {
        let rom = self
            .rom_title
            .as_deref()
            .map(|title| App::short_label(title, 32))
            .unwrap_or_else(|| "No ROM loaded".to_string());

        let performance = if !self.rom_loaded {
            "--".to_string()
        } else if self.gba.state == tinybird_core::GbaState::Paused {
            "Paused".to_string()
        } else {
            format!("{:.1} FPS   {}x", self.current_fps, self.speed_multiplier)
        };

        let audio = if self.muted {
            "Muted".to_string()
        } else {
            format!("Vol {}%", (self.volume * 100.0).round() as u32)
        };

        let slot = format!("Slot {}", self.save_state_slot);

        [rom, performance, audio, slot]
    }
}

/// Owned chrome data for one frame, produced by [`App::build_chrome_frame`].
pub(crate) struct ChromeFrameData {
    pub layout: ChromeLayout,
    palette: Palette,
    menus: Vec<Menu>,
    status: [String; 4],
}

impl ChromeFrameData {
    /// Pair the snapshot with this frame's live input and menu state.
    pub(crate) fn borrow<'a>(
        &'a self,
        input: &'a UiInput,
        state: &'a mut MenuBarState,
    ) -> ChromeFrame<'a> {
        ChromeFrame {
            layout: self.layout,
            palette: &self.palette,
            menus: &self.menus,
            status: &self.status,
            input,
            state,
        }
    }
}

/// A borrowed view of the chrome, ready to draw.
pub(crate) struct ChromeFrame<'a> {
    pub layout: ChromeLayout,
    palette: &'a Palette,
    menus: &'a [Menu],
    status: &'a [String; 4],
    input: &'a UiInput,
    state: &'a mut MenuBarState,
}

/// Paint the menu bar and status bar over the already-composed frame.
///
/// Returns the command the menu bar produced; the caller runs it after the
/// surface borrow has been released.
pub(crate) fn draw_chrome(
    buffer: &mut [u32],
    win_w: usize,
    win_h: usize,
    frame: &mut ChromeFrame<'_>,
) -> Option<UiCommand> {
    let mut canvas = Canvas::new(buffer, win_w, win_h);
    let mut ui = Ui::new(&mut canvas, frame.input, frame.palette);

    if !frame.layout.status_bar.is_empty() {
        draw_status_bar(&mut ui, frame.layout.status_bar, frame.status);
    }
    if frame.layout.menu_bar.is_empty() {
        return None;
    }
    menubar::menu_bar(&mut ui, frame.layout.menu_bar, frame.menus, frame.state).command
}

/// Draw the status bar: ROM on the left, then performance, audio, and slot.
fn draw_status_bar(ui: &mut Ui<'_, '_>, rect: Rect, fields: &[String; 4]) {
    ui.canvas.fill_rect(rect, ui.palette.chrome);
    ui.canvas
        .hline(rect.x, rect.y, rect.w, ui.palette.chrome_border);

    let pad = 8;
    ui.canvas.text_in(
        Rect::new(rect.x + pad, rect.y, rect.w, rect.h),
        Align::Left,
        &fields[0],
        1,
        ui.palette.ink,
    );

    // Right-aligned fields, laid out right to left so none can overlap the title.
    let mut right = rect.right() - pad;
    for field in fields[1..].iter().rev() {
        let width = crate::ui::canvas::text_width(field, 1);
        let cell = Rect::new(right - width, rect.y, width, rect.h);
        ui.canvas
            .text_in(cell, Align::Right, field, 1, ui.palette.ink_muted);
        right = cell.x - 16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Rect = Rect::new(0, 0, 960, 640);

    #[test]
    fn chrome_reserves_space_at_the_top_and_bottom() {
        let layout = ChromeLayout::compute(WINDOW, true, true);
        assert_eq!(layout.menu_bar, Rect::new(0, 0, 960, MENU_BAR_HEIGHT));
        assert_eq!(layout.content.y, MENU_BAR_HEIGHT);
        assert_eq!(layout.status_bar.bottom(), WINDOW.bottom());
        assert_eq!(
            layout.content.h,
            WINDOW.h - MENU_BAR_HEIGHT - STATUS_BAR_HEIGHT
        );
    }

    #[test]
    fn hidden_chrome_gives_the_whole_window_to_the_content() {
        let layout = ChromeLayout::compute(WINDOW, false, false);
        assert_eq!(layout.content, WINDOW);
        assert!(layout.menu_bar.is_empty());
        assert!(layout.status_bar.is_empty());
    }

    #[test]
    fn the_status_bar_can_be_hidden_independently() {
        let layout = ChromeLayout::compute(WINDOW, true, false);
        assert_eq!(layout.menu_bar.h, MENU_BAR_HEIGHT);
        assert!(layout.status_bar.is_empty());
        assert_eq!(layout.content.bottom(), WINDOW.bottom());
    }

    #[test]
    fn a_window_too_short_for_the_bars_keeps_its_content() {
        let tiny = Rect::new(0, 0, 240, 40);
        let layout = ChromeLayout::compute(tiny, true, true);
        assert_eq!(layout.content, tiny, "content must never be squeezed away");
    }

    #[test]
    fn the_three_regions_tile_the_window_exactly() {
        for (menu, status) in [(true, true), (true, false), (false, true), (false, false)] {
            let layout = ChromeLayout::compute(WINDOW, menu, status);
            assert_eq!(
                layout.menu_bar.h + layout.content.h + layout.status_bar.h,
                WINDOW.h,
                "regions must tile for menu={menu} status={status}"
            );
        }
    }
}

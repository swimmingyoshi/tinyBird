//! Persisted user settings.
//!
//! Until now nothing survived a restart: theme, dashboard layout, panel sizes,
//! volume, mute, color correction, and fullscreen all reset to defaults on
//! every launch, and there was no config file anywhere in the workspace.
//!
//! The file is JSON at `%APPDATA%/tinyBird/settings.json` on Windows, or
//! `$XDG_CONFIG_HOME/tinybird/settings.json` (falling back to
//! `~/.config/tinybird`) elsewhere. Every field is `#[serde(default)]`, so an
//! older or hand-edited file that is missing keys still loads — a settings file
//! must never be able to stop the emulator from starting.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ui::ChromeMode;

/// Directory name used under the platform config root.
const APP_DIR: &str = "tinyBird";
const FILE_NAME: &str = "settings.json";

/// How many recently opened ROMs to remember by default.
pub const DEFAULT_RECENT_LIMIT: usize = 10;

/// Number of save-state slots.
pub const SAVE_STATE_SLOTS: u8 = 5;

fn default_true() -> bool {
    true
}

/// Video and presentation settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct VideoSettings {
    /// Window scale multiplier applied to the 240x160 GBA framebuffer.
    pub window_scale: u32,
    /// Restrict the game blit to whole-pixel multiples.
    pub integer_scaling: bool,
    /// Apply the LCD gamma/color-correction lookup.
    pub color_correction: bool,
    /// Draw the FPS/speed HUD.
    pub show_hud: bool,
    /// Start in borderless fullscreen.
    pub fullscreen: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            window_scale: 3,
            integer_scaling: true,
            color_correction: false,
            show_hud: true,
            fullscreen: false,
        }
    }
}

/// Audio output settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Output volume, 0.0..=1.0.
    pub volume: f32,
    pub muted: bool,
    /// Fast-forward mutes output; frame pacing stays stable either way.
    pub mute_on_fast_forward: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            volume: 0.8,
            muted: false,
            mute_on_fast_forward: true,
        }
    }
}

/// Classic-shell settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiSettings {
    /// Whether the menu bar and status bar are drawn.
    pub chrome: ChromeMode,
    pub show_status_bar: bool,
    /// Ask before writing over a save-state slot that already exists.
    pub confirm_overwrite: bool,
    /// Pause emulation when the window loses focus.
    pub pause_on_focus_loss: bool,
    /// How many entries `File > Open Recent` keeps.
    pub recent_limit: usize,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            chrome: ChromeMode::Always,
            show_status_bar: true,
            confirm_overwrite: true,
            pause_on_focus_loss: false,
            recent_limit: DEFAULT_RECENT_LIMIT,
        }
    }
}

/// Stream dashboard appearance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct DashboardSettings {
    /// Theme index into `ui::theme::THEMES`.
    pub theme: u8,
    /// Layout index: Classic / Left Info / Split Sidebars / Cozy Game.
    pub layout: u8,
    /// Game preview size: Compact / Balanced / Game Focus.
    pub game_size: u8,
    /// Side panel width: Slim / Normal / Wide.
    pub side_panel_size: u8,
    /// Open the dashboard expanded rather than docked.
    pub start_expanded: bool,
    /// Write `stream-data/current-game.json` for overlays and external tools.
    #[serde(default = "default_true")]
    pub export_snapshot: bool,
    /// Wallpaper image used by the Wallpaper theme.
    pub wallpaper: Option<PathBuf>,
}

impl Default for DashboardSettings {
    fn default() -> Self {
        Self {
            theme: 0,
            layout: 0,
            game_size: 1,
            side_panel_size: 1,
            start_expanded: false,
            export_snapshot: true,
            wallpaper: None,
        }
    }
}

/// Filesystem locations. `None` means "use the built-in default".
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct PathSettings {
    pub bios: Option<PathBuf>,
    pub rom_dir: Option<PathBuf>,
    pub save_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub screenshot_dir: Option<PathBuf>,
}

/// The whole persisted configuration.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub ui: UiSettings,
    pub dashboard: DashboardSettings,
    pub paths: PathSettings,
    /// Most recently opened first.
    pub recent_roms: Vec<PathBuf>,
}

impl Settings {
    /// Load from `path`, falling back to defaults if it is missing or corrupt.
    ///
    /// A broken settings file is reported and ignored rather than fatal: losing
    /// preferences is annoying, refusing to launch is not acceptable.
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => settings.sanitized(),
            Err(err) => {
                eprintln!(
                    "Ignoring unreadable settings file '{}': {}",
                    path.display(),
                    err
                );
                Self::default()
            }
        }
    }

    /// Load from the platform default location.
    pub fn load() -> Self {
        match settings_path() {
            Some(path) => Self::load_from(&path),
            None => Self::default(),
        }
    }

    /// Write to `path`, creating parent directories as needed.
    ///
    /// Writes to a temporary file first and renames over the target, so an
    /// interrupted save cannot leave a truncated config behind.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

        let temp = path.with_extension("json.tmp");
        fs::write(&temp, json)?;
        // Windows rename fails if the destination exists, so clear it first.
        let _ = fs::remove_file(path);
        fs::rename(&temp, path)
    }

    /// Save to the platform default location, reporting failures without
    /// interrupting the session.
    pub fn save(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        if let Err(err) = self.save_to(&path) {
            eprintln!("Failed to save settings to '{}': {}", path.display(), err);
        }
    }

    /// Clamp values that a hand-edited or stale file could have put out of range.
    fn sanitized(mut self) -> Self {
        self.video.window_scale = self.video.window_scale.clamp(1, 8);
        self.audio.volume = if self.audio.volume.is_finite() {
            self.audio.volume.clamp(0.0, 1.0)
        } else {
            AudioSettings::default().volume
        };
        self.ui.recent_limit = self.ui.recent_limit.min(50);
        self.recent_roms.truncate(self.ui.recent_limit);
        self
    }

    /// Record a ROM as most-recently-used, de-duplicating and trimming.
    pub fn push_recent_rom(&mut self, path: &Path) {
        let limit = self.ui.recent_limit;
        if limit == 0 {
            self.recent_roms.clear();
            return;
        }
        self.recent_roms.retain(|existing| existing != path);
        self.recent_roms.insert(0, path.to_path_buf());
        self.recent_roms.truncate(limit);
    }

    pub fn clear_recent_roms(&mut self) {
        self.recent_roms.clear();
    }

    /// Volume as a whole percentage, for menus and the status bar.
    pub fn volume_percent(&self) -> u32 {
        (self.audio.volume * 100.0).round().clamp(0.0, 100.0) as u32
    }
}

/// Directory the settings file lives in, or `None` if no config root is known.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(appdata) = env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(appdata).join(APP_DIR));
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(xdg).join(APP_DIR.to_lowercase()));
    }
    let home = env::var_os("HOME").filter(|value| !value.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join(APP_DIR.to_lowercase()),
    )
}

/// Full path to the settings file.
pub fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join(FILE_NAME))
}

/// Tracks whether settings changed since the last write, so we persist on
/// change rather than every frame.
#[derive(Debug)]
pub struct SettingsStore {
    settings: Settings,
    dirty: bool,
}

impl SettingsStore {
    pub fn load() -> Self {
        Self {
            settings: Settings::load(),
            dirty: false,
        }
    }

    pub fn from_settings(settings: Settings) -> Self {
        Self {
            settings,
            dirty: false,
        }
    }

    pub fn get(&self) -> &Settings {
        &self.settings
    }

    /// Mutate settings and mark them for the next flush.
    pub fn edit(&mut self) -> &mut Settings {
        self.dirty = true;
        &mut self.settings
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Write to disk if anything changed.
    pub fn flush(&mut self) {
        if !self.is_dirty() {
            return;
        }
        self.settings.save();
        self.dirty = false;
    }
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::from_settings(Settings::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!("tinybird-settings-test-{name}.json"));
        path
    }

    #[test]
    fn defaults_round_trip_through_json() {
        let path = temp_path("round-trip");
        let original = Settings::default();
        original.save_to(&path).expect("save settings");
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, original);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let path = temp_path("definitely-absent");
        let _ = fs::remove_file(&path);
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn a_corrupt_file_yields_defaults_instead_of_failing_to_launch() {
        let path = temp_path("corrupt");
        fs::write(&path, "{ this is not json").expect("write corrupt file");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let path = temp_path("partial");
        // An older config that only knew about the volume.
        fs::write(&path, r#"{ "audio": { "volume": 0.25 } }"#).expect("write partial file");
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.audio.volume, 0.25);
        assert_eq!(loaded.video, VideoSettings::default());
        assert_eq!(loaded.ui.chrome, ChromeMode::Always);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn out_of_range_values_are_clamped_on_load() {
        let path = temp_path("out-of-range");
        fs::write(
            &path,
            r#"{ "audio": { "volume": 42.0 }, "video": { "window_scale": 999 } }"#,
        )
        .expect("write file");
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.audio.volume, 1.0);
        assert_eq!(loaded.video.window_scale, 8);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn non_finite_volume_falls_back_to_the_default() {
        let path = temp_path("nan-volume");
        fs::write(&path, r#"{ "audio": { "volume": null } }"#).expect("write file");
        // serde rejects null for f32, so this exercises the corrupt-file path.
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn recent_roms_move_to_the_front_without_duplicating() {
        let mut settings = Settings::default();
        settings.push_recent_rom(Path::new("a.gba"));
        settings.push_recent_rom(Path::new("b.gba"));
        settings.push_recent_rom(Path::new("a.gba"));

        assert_eq!(
            settings.recent_roms,
            vec![PathBuf::from("a.gba"), PathBuf::from("b.gba")]
        );
    }

    #[test]
    fn recent_roms_respect_the_configured_limit() {
        let mut settings = Settings::default();
        settings.ui.recent_limit = 3;
        for index in 0..10 {
            settings.push_recent_rom(Path::new(&format!("rom{index}.gba")));
        }
        assert_eq!(settings.recent_roms.len(), 3);
        assert_eq!(settings.recent_roms[0], PathBuf::from("rom9.gba"));
    }

    #[test]
    fn a_zero_recent_limit_keeps_no_history() {
        let mut settings = Settings::default();
        settings.ui.recent_limit = 0;
        settings.push_recent_rom(Path::new("a.gba"));
        assert!(settings.recent_roms.is_empty());
    }

    #[test]
    fn volume_percent_rounds_to_whole_numbers() {
        let mut settings = Settings::default();
        settings.audio.volume = 0.756;
        assert_eq!(settings.volume_percent(), 76);
        settings.audio.volume = 0.0;
        assert_eq!(settings.volume_percent(), 0);
    }

    #[test]
    fn the_store_only_flags_writes_after_an_edit() {
        let mut store = SettingsStore::from_settings(Settings::default());
        assert!(!store.is_dirty());
        store.edit().audio.muted = true;
        assert!(store.is_dirty());
        assert!(store.get().audio.muted);
    }

    #[test]
    fn saving_replaces_an_existing_file_atomically() {
        let path = temp_path("overwrite");
        let mut settings = Settings::default();
        settings.save_to(&path).expect("first save");
        settings.dashboard.theme = 2;
        settings.save_to(&path).expect("second save");

        assert_eq!(Settings::load_from(&path).dashboard.theme, 2);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "temp file must not be left behind"
        );
        let _ = fs::remove_file(&path);
    }
}

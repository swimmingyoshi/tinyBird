//! tinyBird Desktop - GBA Emulator Desktop Frontend

mod audio;
mod game_addons;
mod input_map;
mod overlay;
mod pokemon_assets;

use std::env;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gilrs::{Axis as GamepadAxis, EventType, GamepadId, Gilrs};
use softbuffer::Surface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

use tinybird_core::{Color, Framebuffer, Gba, GbaButton, GbaState, CLOCK_SPEED, CYCLES_PER_FRAME};

const SCREEN_WIDTH: u32 = 240;
const SCREEN_HEIGHT: u32 = 160;
const SCALE: u32 = 3;
const FRAME_DURATION: Duration = Duration::from_nanos(
    (CYCLES_PER_FRAME as u64 * 1_000_000_000 + (CLOCK_SPEED as u64 / 2)) / CLOCK_SPEED as u64,
);
const FRAME_PACING_TOLERANCE: Duration = Duration::from_micros(750);
const FRAME_CATCHUP_LIMIT: u32 = 3;
const AUDIO_BACKPRESSURE_MILLIS: u32 = 94;
const ADDON_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const RGB555_COLOR_COUNT: usize = 1 << 15;

fn build_rgb555_lookup(color_correction: bool) -> [u32; RGB555_COLOR_COUNT] {
    let mut table = [0u32; RGB555_COLOR_COUNT];
    for (rgb555, slot) in table.iter_mut().enumerate() {
        let r = (rgb555 as u32) & 0x1F;
        let g = ((rgb555 as u32) >> 5) & 0x1F;
        let b = ((rgb555 as u32) >> 10) & 0x1F;
        let r = (r << 3) | (r >> 2);
        let g = (g << 3) | (g >> 2);
        let b = (b << 3) | (b >> 2);

        *slot = if color_correction {
            let r2 = ((r * 26 + g * 4 + b * 2) / 32).min(255);
            let g2 = ((g * 24 + b * 8) / 32).min(255);
            let b2 = ((r * 6 + g * 4 + b * 22) / 32).min(255);
            (r2 << 16) | (g2 << 8) | b2
        } else {
            (r << 16) | (g << 8) | b
        };
    }
    table
}

fn rgb555_lookup(color_correction: bool) -> &'static [u32; RGB555_COLOR_COUNT] {
    static RGB555_LOOKUP: OnceLock<[u32; RGB555_COLOR_COUNT]> = OnceLock::new();
    static RGB555_LOOKUP_CC: OnceLock<[u32; RGB555_COLOR_COUNT]> = OnceLock::new();

    if color_correction {
        RGB555_LOOKUP_CC.get_or_init(|| build_rgb555_lookup(true))
    } else {
        RGB555_LOOKUP.get_or_init(|| build_rgb555_lookup(false))
    }
}

fn is_rom_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gba") || ext.eq_ignore_ascii_case("bin"))
}

fn discover_rom_candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut candidates: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_rom_file(path))
        .collect();
    candidates.sort();
    candidates
}

fn default_bios_path() -> Option<PathBuf> {
    let path = PathBuf::from("gba_bios.bin");
    path.is_file().then_some(path)
}

enum QuicksaveLoadResult {
    Loaded,
    Missing,
    Failed,
}

struct StatusMessage {
    text: String,
    tone: overlay::ToastTone,
    expires_at: Instant,
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    surface_size: (u32, u32),
    addon_window: Option<Arc<Window>>,
    addon_surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    addon_surface_size: (u32, u32),
    gba: Gba,
    speed_multiplier: u32,
    next_frame_deadline: Instant,
    audio_handler: Option<audio::AudioHandler>,
    rom_loaded: bool,
    rom_title: Option<String>,
    rom_path: Option<PathBuf>,
    save_path: Option<PathBuf>,
    state_path: Option<PathBuf>,
    gilrs: Option<Gilrs>,
    active_gamepad: Option<GamepadId>,
    keyboard_buttons: GbaButton,
    gamepad_buttons: GbaButton,
    gamepad_axis_buttons: GbaButton,
    gamepad_left_x: f32,
    gamepad_left_y: f32,
    gamepad_dpad_x: f32,
    gamepad_dpad_y: f32,
    fps_frame_count: u32,
    fps_timer: Instant,
    current_fps: f64,
    show_overlay: bool,
    show_addon_panel: bool,
    muted: bool,
    volume: f32,
    color_correction: bool,
    cart_save_dirty: bool,
    last_save_flush: Instant,
    quicksave_slot: Option<Gba>,
    hovered_file: Option<PathBuf>,
    status_message: Option<StatusMessage>,
    addon_snapshot: game_addons::StreamSnapshot,
    pokemon_sprites: pokemon_assets::PokemonSpriteStore,
    last_addon_refresh: Instant,
    last_addon_export_json: Option<String>,
}

impl App {
    fn new(rom: Option<(PathBuf, Vec<u8>)>, bios: Option<Vec<u8>>) -> Self {
        let mut gba = Gba::new();
        if let Some(bios_data) = bios {
            gba.load_bios(bios_data);
        }
        let (rom_loaded, rom_path, rom_title, save_path, state_path, quicksave_slot) =
            if let Some((path, rom_data)) = rom {
                gba.load_rom(rom_data);
                let title = Self::rom_title_from_path(&path);
                let save_path = Some(Self::save_path_for_rom(&path));
                let state_path = Some(Self::state_path_for_rom(&path));
                if let Some(save_path) = &save_path {
                    if let Ok(save_data) = fs::read(save_path) {
                        gba.load_save_data(&save_data);
                    }
                }
                let quicksave_slot = state_path
                    .as_ref()
                    .and_then(|path| fs::read(path).ok())
                    .and_then(|bytes| {
                        let mut state = gba.clone();
                        state.load_state_bytes(&bytes).ok()?;
                        Some(state)
                    });
                (
                    true,
                    Some(path),
                    Some(title),
                    save_path,
                    state_path,
                    quicksave_slot,
                )
            } else {
                (false, None, None, None, None, None)
            };
        let addon_snapshot = if rom_loaded {
            game_addons::capture_stream_snapshot(Some(&gba))
        } else {
            game_addons::capture_stream_snapshot(None)
        };
        let mut pokemon_sprites = pokemon_assets::PokemonSpriteStore::new();
        pokemon_sprites.queue_snapshot(&addon_snapshot);

        Self {
            window: None,
            surface: None,
            surface_size: (0, 0),
            addon_window: None,
            addon_surface: None,
            addon_surface_size: (0, 0),
            gba,
            speed_multiplier: 1,
            next_frame_deadline: Instant::now() + FRAME_DURATION,
            audio_handler: None,
            rom_loaded,
            rom_title,
            rom_path,
            save_path,
            state_path,
            gilrs: Gilrs::new().ok(),
            active_gamepad: None,
            keyboard_buttons: GbaButton::empty(),
            gamepad_buttons: GbaButton::empty(),
            gamepad_axis_buttons: GbaButton::empty(),
            gamepad_left_x: 0.0,
            gamepad_left_y: 0.0,
            gamepad_dpad_x: 0.0,
            gamepad_dpad_y: 0.0,
            fps_frame_count: 0,
            fps_timer: Instant::now(),
            current_fps: 0.0,
            show_overlay: false,
            show_addon_panel: false,
            muted: false,
            volume: 1.0,
            color_correction: false,
            cart_save_dirty: false,
            last_save_flush: Instant::now(),
            quicksave_slot,
            hovered_file: None,
            status_message: None,
            addon_snapshot,
            pokemon_sprites,
            last_addon_refresh: Instant::now(),
            last_addon_export_json: None,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(window) = &self.addon_window {
            window.request_redraw();
        }
    }

    fn refresh_window_title(&self) {
        let base = match &self.rom_title {
            Some(name) => format!("tinyBird - {}", name),
            None => "tinyBird - GBA Emulator".to_string(),
        };

        let title = if self.rom_loaded && self.gba.state == GbaState::Paused {
            format!("{base} | Paused")
        } else if self.rom_loaded && self.current_fps > 0.0 && self.gba.state == GbaState::Running {
            format!("{base} | {:.1} FPS", self.current_fps)
        } else {
            base
        };

        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        self.refresh_addon_window_title();
    }

    fn refresh_addon_window_title(&self) {
        let Some(window) = &self.addon_window else {
            return;
        };

        let title = self
            .addon_snapshot
            .addon
            .as_ref()
            .map(|addon| format!("tinyBird - {}", addon.display_name))
            .or_else(|| {
                self.rom_title
                    .as_ref()
                    .map(|name| format!("tinyBird - Addons | {name}"))
            })
            .unwrap_or_else(|| "tinyBird - Addons".to_string());
        window.set_title(&title);
    }

    fn set_status(&mut self, text: impl Into<String>, tone: overlay::ToastTone) {
        let duration = match tone {
            overlay::ToastTone::Warning => Duration::from_secs(4),
            overlay::ToastTone::Info | overlay::ToastTone::Success => Duration::from_secs(3),
        };

        self.status_message = Some(StatusMessage {
            text: text.into(),
            tone,
            expires_at: Instant::now() + duration,
        });
        self.request_redraw();
    }

    fn active_toast(&mut self) -> Option<(String, overlay::ToastTone)> {
        if self
            .status_message
            .as_ref()
            .is_some_and(|status| Instant::now() >= status.expires_at)
        {
            self.status_message = None;
        }

        self.status_message
            .as_ref()
            .map(|status| (status.text.clone(), status.tone))
    }

    fn short_label(text: &str, max_chars: usize) -> String {
        let mut chars = text.chars();
        let mut shortened = String::new();
        for _ in 0..max_chars {
            let Some(ch) = chars.next() else {
                return text.to_string();
            };
            shortened.push(ch);
        }

        if chars.next().is_some() && max_chars >= 3 {
            shortened.truncate(max_chars - 3);
            shortened.push_str("...");
        }

        shortened
    }

    fn reset_timing_state(&mut self) {
        self.next_frame_deadline = Instant::now() + FRAME_DURATION;
    }

    fn clear_audio_output(&self) {
        if let Some(audio_handler) = &self.audio_handler {
            audio_handler.clear();
        }
    }

    fn set_speed_multiplier(&mut self, speed_multiplier: u32) {
        let speed_multiplier = speed_multiplier.max(1);
        if self.speed_multiplier == speed_multiplier {
            return;
        }

        self.speed_multiplier = speed_multiplier;
        // Playback rate changes invalidate any queued samples from the prior mode.
        self.clear_audio_output();
        self.reset_timing_state();
        self.update_audio_emulation_state();
    }

    fn refresh_game_addon_state(&mut self, force: bool) {
        if !force && self.last_addon_refresh.elapsed() < ADDON_REFRESH_INTERVAL {
            return;
        }

        let snapshot = if self.rom_loaded {
            game_addons::capture_stream_snapshot(Some(&self.gba))
        } else {
            game_addons::capture_stream_snapshot(None)
        };
        self.pokemon_sprites.queue_snapshot(&snapshot);

        if self.addon_snapshot != snapshot {
            self.addon_snapshot = snapshot;
            self.refresh_addon_window_title();
            self.request_redraw();
        }

        game_addons::write_stream_snapshot(&self.addon_snapshot, &mut self.last_addon_export_json);
        self.last_addon_refresh = Instant::now();
    }

    fn rom_title_from_path(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    fn save_path_for_rom(path: &Path) -> PathBuf {
        path.with_extension("sav")
    }

    fn state_path_for_rom(path: &Path) -> PathBuf {
        path.with_extension("state")
    }

    fn write_quicksave_file(&mut self) {
        let Some(state_path) = &self.state_path else {
            eprintln!("No ROM loaded for savestate");
            self.set_status("No ROM loaded for save state", overlay::ToastTone::Warning);
            return;
        };

        match self.gba.save_state_bytes() {
            Ok(bytes) => match fs::write(state_path, bytes) {
                Ok(()) => {
                    self.quicksave_slot = Some(self.gba.clone());
                    println!("Quicksave written to {}", state_path.display());
                    self.set_status("Quicksave written", overlay::ToastTone::Success);
                }
                Err(err) => {
                    eprintln!(
                        "Failed to write savestate '{}': {}",
                        state_path.display(),
                        err
                    );
                    self.set_status("Failed to write quicksave", overlay::ToastTone::Warning);
                }
            },
            Err(err) => {
                eprintln!("Failed to serialize savestate: {}", err);
                self.set_status("Failed to serialize quicksave", overlay::ToastTone::Warning);
            }
        }
    }

    fn try_load_quicksave_file(&mut self) -> QuicksaveLoadResult {
        let Some(state_path) = &self.state_path else {
            return QuicksaveLoadResult::Missing;
        };

        let Ok(bytes) = fs::read(state_path) else {
            return QuicksaveLoadResult::Missing;
        };

        let mut state = self.gba.clone();
        if let Err(err) = state.load_state_bytes(&bytes) {
            eprintln!(
                "Failed to load savestate '{}': {}",
                state_path.display(),
                err
            );
            self.set_status("Savestate could not be loaded", overlay::ToastTone::Warning);
            return QuicksaveLoadResult::Failed;
        }

        self.quicksave_slot = Some(state);
        QuicksaveLoadResult::Loaded
    }

    fn maybe_mark_cart_save_dirty(&mut self) {
        if self.gba.take_save_dirty() {
            self.cart_save_dirty = true;
        }
    }

    fn flush_battery_save(&mut self, force: bool) {
        self.maybe_mark_cart_save_dirty();
        if !self.cart_save_dirty {
            return;
        }

        if !force && self.last_save_flush.elapsed() < Duration::from_secs(1) {
            return;
        }

        let Some(save_path) = &self.save_path else {
            return;
        };
        let Some(save_data) = self.gba.save_data() else {
            return;
        };

        match fs::write(save_path, save_data) {
            Ok(()) => {
                self.cart_save_dirty = false;
                self.last_save_flush = Instant::now();
            }
            Err(err) => {
                eprintln!(
                    "Failed to write save file '{}': {}",
                    save_path.display(),
                    err
                );
            }
        }
    }

    fn load_rom_from_path(&mut self, path: PathBuf) {
        self.flush_battery_save(true);

        let Ok(rom_data) = fs::read(&path) else {
            eprintln!("Failed to read ROM '{}'", path.display());
            self.set_status("Could not read ROM file", overlay::ToastTone::Warning);
            return;
        };

        println!("Loading ROM: {}", path.display());
        let name = Self::rom_title_from_path(&path);
        let save_path = Self::save_path_for_rom(&path);
        let state_path = Self::state_path_for_rom(&path);
        let save_data = fs::read(&save_path).ok();
        let loaded_save = save_data.is_some();

        self.gba.load_rom(rom_data);
        if let Some(save_data) = &save_data {
            self.gba.load_save_data(save_data);
        }

        self.rom_loaded = true;
        self.rom_path = Some(path);
        self.save_path = Some(save_path);
        self.state_path = Some(state_path.clone());
        self.rom_title = Some(name.clone());
        self.fps_frame_count = 0;
        self.fps_timer = Instant::now();
        self.current_fps = 0.0;
        self.keyboard_buttons = GbaButton::empty();
        self.gamepad_buttons = GbaButton::empty();
        self.gamepad_axis_buttons = GbaButton::empty();
        self.quicksave_slot = fs::read(&state_path).ok().and_then(|bytes| {
            let mut state = self.gba.clone();
            state.load_state_bytes(&bytes).ok()?;
            Some(state)
        });
        self.cart_save_dirty = false;
        self.last_save_flush = Instant::now();
        self.hovered_file = None;
        self.sync_input_state();
        self.update_audio_emulation_state();
        self.clear_audio_output();
        self.reset_timing_state();
        self.refresh_window_title();
        let short_name = Self::short_label(&name, 24);
        let status = if loaded_save {
            format!("Loaded {short_name} with save data")
        } else {
            format!("Loaded {short_name}")
        };
        self.set_status(status, overlay::ToastTone::Success);
        self.refresh_game_addon_state(true);
        self.request_redraw();
    }

    fn run_emulation_batch(&mut self, base_frames: u32) -> u32 {
        self.pump_gamepad_input();

        if !self.rom_loaded {
            return 0;
        }

        let mut frames_ran = 0;
        if self.gba.state == GbaState::Running {
            let frame_debug = std::env::var("TINYBIRD_FRAME_DEBUG").is_ok();
            let frames_to_run = base_frames.saturating_mul(self.speed_multiplier.max(1));

            for _ in 0..frames_to_run {
                if frame_debug {
                    println!(
                        "Running frame {}, pc={:08x}",
                        self.gba.frame_count,
                        self.gba.pc()
                    );
                }
                self.gba.run_frame();
                frames_ran += 1;
            }

            self.maybe_mark_cart_save_dirty();

            self.refresh_game_addon_state(false);

            if let Some(audio_handler) = &self.audio_handler {
                let samples = self.gba.apu.drain_samples();
                if self.speed_multiplier == 1 && !samples.is_empty() && !self.muted {
                    audio_handler.push_samples(&samples, self.gba.apu.output_sample_rate());
                }
            } else {
                self.gba.apu.drain_samples();
            }

            // Track emulated frames, not redraws, so turbo mode reports useful numbers.
            self.fps_frame_count += frames_ran;
            let fps_elapsed = self.fps_timer.elapsed();
            if fps_elapsed >= Duration::from_secs(1) {
                self.current_fps = self.fps_frame_count as f64 / fps_elapsed.as_secs_f64();
                self.fps_frame_count = 0;
                self.fps_timer = Instant::now();
                self.refresh_window_title();
            }

            // Debug: check PPU state
            if frame_debug
                && (self.gba.frame_count <= 3 || self.gba.frame_count.is_multiple_of(300))
            {
                let bus_dispcnt = self.gba.bus.read_io_direct_u16(0x000);
                let pc = self.gba.pc();
                let thumb = self.gba.cpu.is_thumb_mode();
                let non_black = self
                    .gba
                    .ppu
                    .get_framebuffer()
                    .as_slice()
                    .iter()
                    .filter(|pixel| pixel.color != Color::BLACK)
                    .count();
                let vcount = self.gba.ppu.scanline;
                let ppu_cycle = self.gba.ppu.cycle;
                let total_cycles = self.gba.total_cycles;
                eprintln!(
                    "Frame {} DISPCNT={:04x} pc={:08x} t={} vcount={} ppu_cy={} total_cy={} non_black={}",
                    self.gba.frame_count, bus_dispcnt, pc, thumb, vcount, ppu_cycle, total_cycles, non_black
                );
            }
        }

        frames_ran
    }

    fn update_audio_emulation_state(&mut self) {
        let audio_emulation_active = self.audio_handler.is_some();
        self.gba.set_audio_enabled(audio_emulation_active);

        if let Some(audio_handler) = &self.audio_handler {
            audio_handler.set_volume(if self.muted || self.speed_multiplier > 1 {
                0.0
            } else {
                self.volume
            });
        }

        if !audio_emulation_active {
            self.gba.apu.drain_samples();
            self.clear_audio_output();
        } else if self.muted || self.speed_multiplier > 1 {
            self.clear_audio_output();
        }
    }

    fn present_current_frame(&mut self) {
        if self.pokemon_sprites.drain_updates() {
            self.request_redraw();
        }

        let toast = self.active_toast();
        let hovered_file = self.hovered_file.as_ref().and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| Self::short_label(name, 24))
        });

        if !self.rom_loaded {
            let Some(surface) = &mut self.surface else {
                return;
            };
            let home = overlay::HomeScreen {
                bios_loaded: self.gba.use_bios,
                hovered_file: hovered_file.as_deref(),
            };
            let toast = toast.as_ref().map(|(text, tone)| overlay::Toast {
                text: text.as_str(),
                tone: *tone,
            });
            Self::present_buffer(
                surface,
                self.surface_size,
                None,
                false,
                None,
                None,
                Some(home),
                None,
                toast,
            );
            self.present_addon_window();
            return;
        }

        let framebuffer = self.gba.ppu.get_framebuffer();
        let addon_panel = self
            .show_addon_panel
            .then_some((&self.addon_snapshot, &self.pokemon_sprites));
        let Some(surface) = &mut self.surface else {
            return;
        };
        let overlay_params = if self.show_overlay && self.gba.state == GbaState::Running {
            Some((
                self.current_fps,
                self.speed_multiplier,
                self.muted,
                (self.volume * 100.0).round() as u32,
                self.color_correction,
                self.speed_multiplier > 1,
            ))
        } else {
            None
        };
        let pause_screen = if self.gba.state == GbaState::Paused {
            Some(overlay::PauseScreen {
                rom_title: self.rom_title.as_deref(),
            })
        } else {
            None
        };
        let toast = toast.as_ref().map(|(text, tone)| overlay::Toast {
            text: text.as_str(),
            tone: *tone,
        });
        Self::present_buffer(
            surface,
            self.surface_size,
            Some(framebuffer),
            self.color_correction,
            overlay_params,
            addon_panel,
            None,
            pause_screen,
            toast,
        );
        self.present_addon_window();
    }

    fn render_frame(&mut self, base_frames: u32) {
        self.run_emulation_batch(base_frames);
        self.present_current_frame();
    }

    fn present_buffer(
        surface: &mut Surface<Arc<Window>, Arc<Window>>,
        surface_size: (u32, u32),
        framebuffer: Option<&Framebuffer>,
        color_correction: bool,
        overlay: Option<(f64, u32, bool, u32, bool, bool)>,
        addon_panel: Option<(
            &game_addons::StreamSnapshot,
            &pokemon_assets::PokemonSpriteStore,
        )>,
        home_screen: Option<overlay::HomeScreen<'_>>,
        pause_screen: Option<overlay::PauseScreen<'_>>,
        toast: Option<overlay::Toast<'_>>,
    ) {
        if surface_size.0 == 0 || surface_size.1 == 0 {
            return;
        }

        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        let win_w = surface_size.0 as usize;
        let win_h = surface_size.1 as usize;

        let Some(framebuffer) = framebuffer else {
            if let Some(home) = home_screen {
                overlay::draw_home_screen(&mut buffer, win_w, win_h, home);
            } else {
                buffer.fill(0x000000);
            }
            if let Some(toast) = toast {
                overlay::draw_toast(&mut buffer, win_w, win_h, toast);
            }
            let _ = buffer.present();
            return;
        };

        let src_w = SCREEN_WIDTH as usize;
        let src_h = SCREEN_HEIGHT as usize;

        let (draw_w, draw_h) = {
            let integer_scale = (win_w / src_w).min(win_h / src_h);
            if integer_scale >= 1 {
                (src_w * integer_scale, src_h * integer_scale)
            } else if win_w * src_h <= win_h * src_w {
                (win_w, win_w * src_h / src_w)
            } else {
                (win_h * src_w / src_h, win_h)
            }
        };
        let offset_x = (win_w.saturating_sub(draw_w)) / 2;
        let offset_y = (win_h.saturating_sub(draw_h)) / 2;
        let pixels = framebuffer.as_slice();
        let color_lookup = rgb555_lookup(color_correction);
        let needs_clear = offset_x != 0 || offset_y != 0 || draw_w != win_w || draw_h != win_h;
        if needs_clear {
            buffer.fill(0x000000);
        }

        if draw_w % src_w == 0 && draw_h % src_h == 0 && draw_w / src_w == draw_h / src_h {
            let scale = draw_w / src_w;
            if scale > 0 {
                let mut converted_row = [0u32; SCREEN_WIDTH as usize];
                for src_y in 0..src_h {
                    let src_row = src_y * src_w;
                    let dst_y_base = offset_y + src_y * scale;
                    let first_dst_row = dst_y_base * win_w + offset_x;

                    for src_x in 0..src_w {
                        converted_row[src_x] =
                            color_lookup[pixels[src_row + src_x].color.to_rgb555() as usize];
                    }

                    let mut dst_x = 0;
                    for color in converted_row.iter().take(src_w) {
                        buffer[first_dst_row + dst_x..first_dst_row + dst_x + scale].fill(*color);
                        dst_x += scale;
                    }

                    for dy in 1..scale {
                        let dst_row = (dst_y_base + dy) * win_w + offset_x;
                        buffer.copy_within(first_dst_row..first_dst_row + draw_w, dst_row);
                    }
                }

                if let Some((fps, speed, muted, volume_pct, cc, fast_forward)) = overlay {
                    overlay::draw_overlay(
                        &mut buffer,
                        win_w,
                        win_h,
                        fps,
                        speed,
                        muted,
                        volume_pct,
                        cc,
                        fast_forward,
                    );
                }
                if let Some((snapshot, sprites)) = addon_panel {
                    overlay::draw_addon_panel(&mut buffer, win_w, win_h, snapshot, sprites, false);
                }
                if let Some(paused) = pause_screen {
                    overlay::dim_screen(&mut buffer);
                    overlay::draw_pause_screen(&mut buffer, win_w, win_h, paused);
                }
                if let Some(toast) = toast {
                    overlay::draw_toast(&mut buffer, win_w, win_h, toast);
                }
                let _ = buffer.present();
                return;
            }
        }

        for y in 0..draw_h {
            let src_y = y * src_h / draw_h;
            let src_row = src_y * src_w;
            let dst_row = (offset_y + y) * win_w + offset_x;
            for x in 0..draw_w {
                let src_x = x * src_w / draw_w;
                buffer[dst_row + x] =
                    color_lookup[pixels[src_row + src_x].color.to_rgb555() as usize];
            }
        }

        if let Some((fps, speed, muted, volume_pct, cc, fast_forward)) = overlay {
            overlay::draw_overlay(
                &mut buffer,
                win_w,
                win_h,
                fps,
                speed,
                muted,
                volume_pct,
                cc,
                fast_forward,
            );
        }
        if let Some((snapshot, sprites)) = addon_panel {
            overlay::draw_addon_panel(&mut buffer, win_w, win_h, snapshot, sprites, false);
        }
        if let Some(paused) = pause_screen {
            overlay::dim_screen(&mut buffer);
            overlay::draw_pause_screen(&mut buffer, win_w, win_h, paused);
        }
        if let Some(toast) = toast {
            overlay::draw_toast(&mut buffer, win_w, win_h, toast);
        }
        let _ = buffer.present();
    }

    fn present_addon_window(&mut self) {
        let Some(surface) = &mut self.addon_surface else {
            return;
        };
        if self.addon_surface_size.0 == 0 || self.addon_surface_size.1 == 0 {
            return;
        }

        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        overlay::draw_addon_panel(
            &mut buffer,
            self.addon_surface_size.0 as usize,
            self.addon_surface_size.1 as usize,
            &self.addon_snapshot,
            &self.pokemon_sprites,
            true,
        );
        let _ = buffer.present();
    }

    fn sync_input_state(&mut self) {
        self.gba
            .input
            .set_buttons(self.keyboard_buttons | self.gamepad_buttons | self.gamepad_axis_buttons);
    }

    fn recompute_gamepad_axis_buttons(&mut self) {
        self.gamepad_axis_buttons = input_map::buttons_from_axes(
            self.gamepad_left_x,
            self.gamepad_left_y,
            self.gamepad_dpad_x,
            self.gamepad_dpad_y,
        );
    }

    fn update_gamepad_axis(&mut self, axis: GamepadAxis, value: f32) {
        match axis {
            GamepadAxis::LeftStickX => self.gamepad_left_x = value,
            GamepadAxis::LeftStickY => self.gamepad_left_y = value,
            GamepadAxis::DPadX => self.gamepad_dpad_x = value,
            GamepadAxis::DPadY => self.gamepad_dpad_y = value,
            _ => {}
        }
        self.recompute_gamepad_axis_buttons();
    }

    fn pump_gamepad_input(&mut self) {
        let Some(mut gilrs) = self.gilrs.take() else {
            return;
        };

        let mut changed = false;
        while let Some(event) = gilrs.next_event() {
            if self.active_gamepad.is_none() {
                self.active_gamepad = Some(event.id);
            }

            match event.event {
                EventType::Connected => {
                    self.active_gamepad.get_or_insert(event.id);
                }
                EventType::Disconnected => {
                    if self.active_gamepad == Some(event.id) {
                        self.active_gamepad = None;
                        self.gamepad_buttons = GbaButton::empty();
                        self.gamepad_axis_buttons = GbaButton::empty();
                        self.gamepad_left_x = 0.0;
                        self.gamepad_left_y = 0.0;
                        self.gamepad_dpad_x = 0.0;
                        self.gamepad_dpad_y = 0.0;
                        changed = true;
                    }
                }
                _ => {}
            }

            if self.active_gamepad != Some(event.id) {
                continue;
            }

            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(mapped) = input_map::map_gamepad_button(button) {
                        self.gamepad_buttons.insert(mapped);
                        changed = true;
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(mapped) = input_map::map_gamepad_button(button) {
                        self.gamepad_buttons.remove(mapped);
                        changed = true;
                    }
                }
                EventType::AxisChanged(axis, value, _) if input_map::is_direction_axis(axis) => {
                    self.update_gamepad_axis(axis, value);
                    changed = true;
                }
                _ => {}
            }
        }

        if changed {
            self.sync_input_state();
        }
        self.gilrs = Some(gilrs);
    }

    fn handle_key(
        &mut self,
        event_loop: &ActiveEventLoop,
        physical_key: &PhysicalKey,
        logical_key: &Key,
        pressed: bool,
    ) {
        if let Some(btn) = input_map::map_physical_key(physical_key) {
            if pressed {
                self.keyboard_buttons.insert(btn);
            } else {
                self.keyboard_buttons.remove(btn);
            }
            self.sync_input_state();
        }

        // Handle emulator controls
        if pressed {
            if !self.rom_loaded && matches!(logical_key, Key::Named(NamedKey::Enter)) {
                self.open_rom();
                return;
            }

            match logical_key {
                Key::Named(NamedKey::Tab) => {
                    self.set_speed_multiplier(4);
                }
                Key::Named(NamedKey::Escape) => {
                    if self.gba.state == GbaState::Running {
                        self.gba.pause();
                        self.clear_audio_output();
                        self.reset_timing_state();
                        self.refresh_window_title();
                        self.set_status("Paused", overlay::ToastTone::Info);
                    } else if self.gba.state == GbaState::Paused {
                        self.gba.start();
                        self.clear_audio_output();
                        self.reset_timing_state();
                        self.refresh_window_title();
                        self.set_status("Resumed", overlay::ToastTone::Info);
                    }
                }
                Key::Named(NamedKey::F5) => {
                    self.write_quicksave_file();
                }
                Key::Named(NamedKey::F8) => {
                    if self.quicksave_slot.is_none()
                        && matches!(self.try_load_quicksave_file(), QuicksaveLoadResult::Failed)
                    {
                        return;
                    }
                    if let Some(state) = &self.quicksave_slot {
                        self.gba = state.clone();
                        self.cart_save_dirty = true;
                        self.sync_input_state();
                        self.clear_audio_output();
                        self.reset_timing_state();
                        self.update_audio_emulation_state();
                        self.refresh_window_title();
                        self.refresh_game_addon_state(true);
                        println!("Quicksave loaded");
                        self.set_status("Quicksave loaded", overlay::ToastTone::Success);
                    } else {
                        eprintln!("No quicksave available yet");
                        self.set_status("No quicksave available", overlay::ToastTone::Warning);
                    }
                }
                Key::Named(NamedKey::F1) => {
                    self.show_overlay = !self.show_overlay;
                    self.request_redraw();
                }
                Key::Named(NamedKey::F2) => {
                    self.show_addon_panel = !self.show_addon_panel;
                    self.request_redraw();
                    self.set_status(
                        if self.show_addon_panel {
                            "Team panel shown"
                        } else {
                            "Team panel hidden"
                        },
                        overlay::ToastTone::Info,
                    );
                }
                Key::Named(NamedKey::F3) => {
                    if self.addon_window.is_some() {
                        self.close_addon_window();
                        self.set_status("Addon popout closed", overlay::ToastTone::Info);
                    } else {
                        self.ensure_addon_window(event_loop);
                        self.set_status("Addon popout opened", overlay::ToastTone::Info);
                    }
                }
                Key::Character(c) if c.as_str() == "1" => {
                    self.set_speed_multiplier(1);
                    self.set_status("Speed set to 1x", overlay::ToastTone::Info);
                }
                Key::Character(c) if c.as_str() == "2" => {
                    self.set_speed_multiplier(2);
                    self.set_status("Speed set to 2x, audio muted", overlay::ToastTone::Info);
                }
                Key::Character(c) if c.as_str() == "3" || c.as_str() == "4" => {
                    self.set_speed_multiplier(4);
                    self.set_status("Speed set to 4x, audio muted", overlay::ToastTone::Info);
                }
                Key::Character(c) if c.as_str() == "m" || c.as_str() == "M" => {
                    self.muted = !self.muted;
                    self.update_audio_emulation_state();
                    if self.muted {
                        self.set_status("Audio muted", overlay::ToastTone::Info);
                    } else {
                        self.set_status("Audio unmuted", overlay::ToastTone::Info);
                    }
                }
                Key::Character(c) if c.as_str() == "-" || c.as_str() == "[" => {
                    self.volume = (self.volume - 0.1).clamp(0.0, 1.0);
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
                Key::Character(c) if c.as_str() == "=" || c.as_str() == "]" => {
                    self.volume = (self.volume + 0.1).clamp(0.0, 1.0);
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
                Key::Character(c) if c.as_str() == "c" || c.as_str() == "C" => {
                    self.color_correction = !self.color_correction;
                    if self.color_correction {
                        self.set_status("LCD color correction on", overlay::ToastTone::Info);
                    } else {
                        self.set_status("LCD color correction off", overlay::ToastTone::Info);
                    }
                }
                Key::Character(c) if c.as_str() == "r" || c.as_str() == "R" => {
                    self.gba.reset();
                    self.clear_audio_output();
                    self.reset_timing_state();
                    self.refresh_window_title();
                    self.refresh_game_addon_state(true);
                    self.set_status("ROM reset", overlay::ToastTone::Info);
                }
                Key::Character(c) if c.as_str() == "o" || c.as_str() == "O" => {
                    self.open_rom();
                }
                _ => {}
            }
        } else if matches!(logical_key, Key::Named(NamedKey::Tab)) {
            self.set_speed_multiplier(1);
        }
    }

    fn handle_addon_window_key(
        &mut self,
        event_loop: &ActiveEventLoop,
        logical_key: &Key,
        pressed: bool,
    ) {
        if !pressed {
            return;
        }

        match logical_key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F3) => {
                self.close_addon_window();
                self.set_status("Addon popout closed", overlay::ToastTone::Info);
            }
            Key::Named(NamedKey::F2) => {
                self.show_addon_panel = !self.show_addon_panel;
                self.request_redraw();
                self.set_status(
                    if self.show_addon_panel {
                        "Team panel shown"
                    } else {
                        "Team panel hidden"
                    },
                    overlay::ToastTone::Info,
                );
            }
            Key::Named(NamedKey::F1) => {
                self.show_overlay = !self.show_overlay;
                self.request_redraw();
            }
            Key::Character(c) if c.as_str() == "o" || c.as_str() == "O" => {
                self.open_rom();
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) if !self.rom_loaded => {
                self.open_rom();
                self.request_redraw();
            }
            Key::Named(NamedKey::Tab) => {
                self.ensure_addon_window(event_loop);
            }
            _ => {}
        }
    }

    fn open_rom(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GBA ROMs", &["gba", "bin"])
            .pick_file()
        {
            self.load_rom_from_path(path);
        }
    }

    fn ensure_addon_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.addon_window.is_some() {
            self.request_redraw();
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("tinyBird - Addons")
            .with_inner_size(LogicalSize::new(420.0, 360.0))
            .with_min_inner_size(LogicalSize::new(260.0, 220.0))
            .with_resizable(true);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create addon window"),
        );
        let context =
            softbuffer::Context::new(window.clone()).expect("Failed to create addon context");
        let surface =
            Surface::new(&context, window.clone()).expect("Failed to create addon surface");

        self.addon_window = Some(window);
        self.addon_surface = Some(surface);
        self.resize_addon_surface(420, 360);
        self.refresh_addon_window_title();
        self.request_redraw();
    }

    fn close_addon_window(&mut self) {
        self.addon_surface = None;
        self.addon_window = None;
        self.addon_surface_size = (0, 0);
    }

    fn resize_main_surface(&mut self, width: u32, height: u32) {
        self.surface_size = (width, height);
        if width == 0 || height == 0 {
            return;
        }
        if let Some(surface) = &mut self.surface {
            let _ = surface.resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            );
        }
    }

    fn resize_addon_surface(&mut self, width: u32, height: u32) {
        self.addon_surface_size = (width, height);
        if width == 0 || height == 0 {
            return;
        }
        if let Some(surface) = &mut self.addon_surface {
            let _ = surface.resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            );
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window_size = LogicalSize::new(SCREEN_WIDTH * SCALE, SCREEN_HEIGHT * SCALE);
        let attrs = Window::default_attributes()
            .with_title("tinyBird - GBA Emulator")
            .with_inner_size(window_size)
            .with_min_inner_size(LogicalSize::new(SCREEN_WIDTH, SCREEN_HEIGHT))
            .with_resizable(true);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        );

        let context = softbuffer::Context::new(window.clone()).expect("Failed to create context");
        let surface = Surface::new(&context, window.clone()).expect("Failed to create surface");

        self.window = Some(window);
        self.surface = Some(surface);
        self.resize_main_surface(SCREEN_WIDTH * SCALE, SCREEN_HEIGHT * SCALE);
        self.refresh_window_title();

        // Try to init audio
        self.audio_handler = audio::AudioHandler::new().ok();
        self.update_audio_emulation_state();
        self.reset_timing_state();
        self.refresh_game_addon_state(true);
        self.request_redraw();

        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame_deadline));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_addon_window = self
            .addon_window
            .as_ref()
            .is_some_and(|window| window.id() == window_id);

        match event {
            WindowEvent::CloseRequested => {
                if is_addon_window {
                    self.close_addon_window();
                    self.set_status("Addon popout closed", overlay::ToastTone::Info);
                } else {
                    self.flush_battery_save(true);
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if is_addon_window {
                    self.resize_addon_surface(size.width, size.height);
                } else {
                    self.resize_main_surface(size.width, size.height);
                }
                if is_addon_window {
                    if let Some(window) = &self.addon_window {
                        window.request_redraw();
                    }
                } else if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::HoveredFile(path) => {
                if is_addon_window {
                    return;
                }
                self.hovered_file = Some(path);
                self.request_redraw();
            }
            WindowEvent::HoveredFileCancelled => {
                if is_addon_window {
                    return;
                }
                self.hovered_file = None;
                self.request_redraw();
            }
            WindowEvent::DroppedFile(path) => {
                if is_addon_window {
                    return;
                }
                self.hovered_file = None;
                self.load_rom_from_path(path);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key,
                        logical_key,
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                if is_addon_window {
                    self.handle_addon_window_key(event_loop, &logical_key, pressed);
                } else {
                    self.handle_key(event_loop, &physical_key, &logical_key, pressed);
                }
            }
            WindowEvent::RedrawRequested => {
                if is_addon_window {
                    self.present_addon_window();
                    return;
                }

                if !self.rom_loaded || self.gba.state != GbaState::Running {
                    self.render_frame(0);
                    return;
                }

                let now = Instant::now();
                if now + FRAME_PACING_TOLERANCE < self.next_frame_deadline {
                    return;
                }

                let max_catchup_frames = if self
                    .audio_handler
                    .as_ref()
                    .map(|audio_handler| audio_handler.buffered_millis())
                    .unwrap_or(0)
                    >= AUDIO_BACKPRESSURE_MILLIS
                {
                    1
                } else {
                    FRAME_CATCHUP_LIMIT
                };

                let mut frames_due = 0;
                while frames_due < max_catchup_frames
                    && now + FRAME_PACING_TOLERANCE >= self.next_frame_deadline
                {
                    frames_due += 1;
                    self.next_frame_deadline += FRAME_DURATION;
                }

                if frames_due == 0 {
                    return;
                }

                if now + FRAME_PACING_TOLERANCE >= self.next_frame_deadline {
                    self.next_frame_deadline = now + FRAME_DURATION;
                }

                self.render_frame(frames_due);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.flush_battery_save(false);
        if self.pokemon_sprites.drain_updates() {
            self.request_redraw();
        }
        let next_tick = if let Some(status) = &self.status_message {
            if Instant::now() >= status.expires_at {
                self.request_redraw();
            }
            ControlFlow::WaitUntil(status.expires_at)
        } else if !self.rom_loaded || self.gba.state != GbaState::Running {
            ControlFlow::Wait
        } else {
            ControlFlow::WaitUntil(self.next_frame_deadline)
        };
        event_loop.set_control_flow(next_tick);
        if self.rom_loaded && self.gba.state == GbaState::Running {
            self.request_redraw();
        }
    }
}

fn main() {
    let mut rom_path: Option<PathBuf> = None;
    let mut bios_path: Option<PathBuf> = None;
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--bios" {
            bios_path = iter.next().map(PathBuf::from);
        } else if rom_path.is_none() {
            rom_path = Some(PathBuf::from(arg));
        }
    }

    let rom_path = rom_path.or_else(|| {
        let candidates = discover_rom_candidates(Path::new("roms"));
        match candidates.as_slice() {
            [path] => {
                println!("Auto-loading ROM: {}", path.display());
                Some(path.clone())
            }
            [] => None,
            _ => {
                println!(
                    "Found {} ROMs in roms/; pass a ROM path or press 'O' after launch.",
                    candidates.len()
                );
                None
            }
        }
    });
    let bios_path = bios_path.or_else(default_bios_path);

    let bios = if let Some(path) = bios_path.as_ref() {
        match fs::read(path) {
            Ok(data) => {
                println!("Loaded BIOS: {} ({} bytes)", path.display(), data.len());
                Some(data)
            }
            Err(e) => {
                eprintln!("Failed to load BIOS '{}': {}", path.display(), e);
                None
            }
        }
    } else {
        None
    };

    let rom = if let Some(path) = rom_path.as_ref() {
        match fs::read(path) {
            Ok(data) => {
                println!("Loaded ROM: {} ({} bytes)", path.display(), data.len());
                Some((path.clone(), data))
            }
            Err(e) => {
                eprintln!("Failed to load ROM '{}': {}", path.display(), e);
                None
            }
        }
    } else {
        println!("tinyBird - GBA Emulator");
        println!("Usage: tinybird [--bios gba_bios.bin] [rom.gba]");
        println!("Press 'O' to open a ROM file");
        println!("If exactly one ROM is in ./roms/, it will be loaded automatically");
        None
    };

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new(rom, bios);
    event_loop.run_app(&mut app).expect("Event loop error");
}

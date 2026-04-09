//! Pixel Processing Unit (PPU)
//!
//! This module implements the GBA's display system, including:
//! - Background layers (text, affine, bitmap modes)
//! - Sprite/object rendering
//! - Window effects
//! - Color effects (blending, brightness)
//! - Mosaic effects
//!
//! The GBA PPU is scanline-based, rendering one line at a time.
//! Each frame consists of 228 scanlines (160 visible + 68 VBlank).
//! Each scanline takes 1232 cycles to render.

pub mod backgrounds;
pub mod effects;
pub mod render;
pub mod sprites;

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};

pub use backgrounds::{Background, BackgroundControl};
pub use effects::{BlendMode, Effects, Window};
pub use render::{Color, Framebuffer, Renderer};
pub use sprites::{ObjectAttribute, Sprite, SpriteRenderer};

/// Screen width in pixels
pub const SCREEN_WIDTH: usize = 240;
/// Screen height in pixels
pub const SCREEN_HEIGHT: usize = 160;
/// Total scanlines per frame (including VBlank)
pub const TOTAL_SCANLINES: u32 = 228;
/// Visible scanlines
pub const VISIBLE_SCANLINES: u32 = 160;
/// Cycles per scanline
pub const CYCLES_PER_SCANLINE: u32 = 1232;
/// Visible cycles per scanline
pub const VISIBLE_CYCLES: u32 = 960;
/// HBlank starts at this cycle
pub const HBLANK_START: u32 = 960;
/// VBlank starts at this scanline
pub const VBLANK_START: u32 = 160;

/// Display control register (DISPCNT)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DisplayControl {
    /// Background mode (0-5)
    pub bg_mode: u8,
    /// Use CGB mode (unused on GBA)
    pub cgb_mode: bool,
    /// Display AFB select (for modes 4 & 5)
    pub display_frame: u8,
    /// Allow HBlank interval free access to VRAM
    pub hblank_interval_free: bool,
    /// Object character mapping mode
    pub obj_mapping: bool,
    /// Forced blank
    pub forced_blank: bool,
    /// Background enable flags (BG0-BG3)
    pub bg_enable: [bool; 4],
    /// Object display enable
    pub obj_enable: bool,
    /// Window enable flags (WIN0, WIN1, OBJ)
    pub window_enable: [bool; 3],
}

impl DisplayControl {
    /// Parse from raw register value
    pub fn from_value(value: u16) -> Self {
        Self {
            bg_mode: (value & 0x7) as u8,
            cgb_mode: (value & (1 << 3)) != 0,
            display_frame: ((value >> 4) & 1) as u8,
            hblank_interval_free: (value & (1 << 5)) != 0,
            obj_mapping: (value & (1 << 6)) != 0,
            forced_blank: (value & (1 << 7)) != 0,
            bg_enable: [
                (value & (1 << 8)) != 0,
                (value & (1 << 9)) != 0,
                (value & (1 << 10)) != 0,
                (value & (1 << 11)) != 0,
            ],
            obj_enable: (value & (1 << 12)) != 0,
            window_enable: [
                (value & (1 << 13)) != 0,
                (value & (1 << 14)) != 0,
                (value & (1 << 15)) != 0,
            ],
        }
    }

    /// Convert to raw register value
    pub fn to_value(&self) -> u16 {
        let mut value = self.bg_mode as u16;
        if self.cgb_mode {
            value |= 1 << 3;
        }
        if self.display_frame != 0 {
            value |= 1 << 4;
        }
        if self.hblank_interval_free {
            value |= 1 << 5;
        }
        if self.obj_mapping {
            value |= 1 << 6;
        }
        if self.forced_blank {
            value |= 1 << 7;
        }
        for (i, &enabled) in self.bg_enable.iter().enumerate() {
            if enabled {
                value |= 1 << (8 + i);
            }
        }
        if self.obj_enable {
            value |= 1 << 12;
        }
        for (i, &enabled) in self.window_enable.iter().enumerate() {
            if enabled {
                value |= 1 << (13 + i);
            }
        }
        value
    }

    /// Get the current display frame for page-flipped modes
    pub fn get_display_frame(&self) -> usize {
        self.display_frame as usize
    }
}

/// Display status register (DISPSTAT)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DisplayStatus {
    /// V-Blank flag (read-only)
    pub vblank: bool,
    /// H-Blank flag (read-only)
    pub hblank: bool,
    /// V-Counter flag (read-only)
    pub vcounter: bool,
    /// V-Blank IRQ enable
    pub vblank_irq: bool,
    /// H-Blank IRQ enable
    pub hblank_irq: bool,
    /// V-Counter IRQ enable
    pub vcounter_irq: bool,
    /// V-Counter comparison value
    pub vcounter_compare: u8,
}

impl DisplayStatus {
    /// Parse from raw register value (lower 16 bits)
    pub fn from_value_low(value: u16) -> Self {
        Self {
            vblank: (value & 1) != 0,
            hblank: (value & 2) != 0,
            vcounter: (value & 4) != 0,
            vblank_irq: (value & (1 << 3)) != 0,
            hblank_irq: (value & (1 << 4)) != 0,
            vcounter_irq: (value & (1 << 5)) != 0,
            vcounter_compare: 0, // Not in low part
        }
    }

    /// Parse from raw register value (high 16 bits - VCounter compare)
    pub fn from_value_high(value: u16, mut status: Self) -> Self {
        status.vcounter_compare = ((value >> 8) & 0xFF) as u8;
        status
    }

    /// Convert to raw register value (lower 16 bits)
    pub fn to_value_low(&self) -> u16 {
        let mut value = 0;
        if self.vblank {
            value |= 1;
        }
        if self.hblank {
            value |= 2;
        }
        if self.vcounter {
            value |= 4;
        }
        if self.vblank_irq {
            value |= 1 << 3;
        }
        if self.hblank_irq {
            value |= 1 << 4;
        }
        if self.vcounter_irq {
            value |= 1 << 5;
        }
        value
    }

    /// Convert to raw register value (high 16 bits)
    pub fn to_value_high(&self) -> u16 {
        (self.vcounter_compare as u16) << 8
    }
}

/// Main PPU structure
#[derive(Clone)]
pub struct Ppu {
    /// Display control register
    pub display_control: DisplayControl,
    /// Display status register
    pub display_status: DisplayStatus,
    /// Current scanline (0-227)
    pub scanline: u32,
    /// Current cycle within scanline
    pub cycle: u32,
    /// Background layers
    pub backgrounds: [Background; 4],
    /// Sprite renderer
    pub sprite_renderer: SpriteRenderer,
    /// Framebuffer for current frame
    pub framebuffer: Framebuffer,
    /// Color effects
    pub effects: Effects,
    /// Windows (WIN0 and WIN1)
    pub windows: [Window; 2],
    /// Layer enables for pixels outside all windows (WINOUT bits 0-5)
    pub winout_control: crate::ppu::effects::WindowControl,
    /// Layer enables inside OBJ window regions (WINOUT bits 8-13)
    pub objwin_control: crate::ppu::effects::WindowControl,
    /// Palette RAM (512 colors, 9-bit addressable)
    pub palette: [u16; 512],
    /// VRAM (96KB)
    pub vram: Vec<u8>,
    /// OAM (Object Attribute Memory, 128 sprites * 8 bytes)
    pub oam: [u8; 1024],
}

#[derive(Serialize, Deserialize)]
struct PpuSerde {
    display_control: DisplayControl,
    display_status: DisplayStatus,
    scanline: u32,
    cycle: u32,
    backgrounds: [Background; 4],
    sprite_renderer: SpriteRenderer,
    framebuffer: Framebuffer,
    effects: Effects,
    windows: [Window; 2],
    winout_control: crate::ppu::effects::WindowControl,
    objwin_control: crate::ppu::effects::WindowControl,
    palette: Vec<u16>,
    vram: Vec<u8>,
    oam: Vec<u8>,
}

impl Serialize for Ppu {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PpuSerde {
            display_control: self.display_control,
            display_status: self.display_status,
            scanline: self.scanline,
            cycle: self.cycle,
            backgrounds: self.backgrounds,
            sprite_renderer: self.sprite_renderer.clone(),
            framebuffer: self.framebuffer.clone(),
            effects: self.effects.clone(),
            windows: self.windows,
            winout_control: self.winout_control,
            objwin_control: self.objwin_control,
            palette: self.palette.to_vec(),
            vram: self.vram.clone(),
            oam: self.oam.to_vec(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Ppu {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = PpuSerde::deserialize(deserializer)?;
        let palette: [u16; 512] = helper.palette.try_into().map_err(|v: Vec<u16>| {
            D::Error::custom(format!("palette had length {}, expected 512", v.len()))
        })?;
        let oam: [u8; 1024] = helper.oam.try_into().map_err(|v: Vec<u8>| {
            D::Error::custom(format!("oam had length {}, expected 1024", v.len()))
        })?;
        Ok(Self {
            display_control: helper.display_control,
            display_status: helper.display_status,
            scanline: helper.scanline,
            cycle: helper.cycle,
            backgrounds: helper.backgrounds,
            sprite_renderer: helper.sprite_renderer,
            framebuffer: helper.framebuffer,
            effects: helper.effects,
            windows: helper.windows,
            winout_control: helper.winout_control,
            objwin_control: helper.objwin_control,
            palette,
            vram: helper.vram,
            oam,
        })
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    /// Map a VRAM bus address to the underlying 96KB physical VRAM layout.
    fn vram_index(addr: u32) -> usize {
        let mut offset = (addr & 0x1_FFFF) as usize;
        if offset >= 0x18_000 {
            offset -= 0x8_000;
        }
        offset
    }

    /// Create a new PPU
    pub fn new() -> Self {
        Self {
            display_control: DisplayControl::default(),
            display_status: DisplayStatus::default(),
            scanline: 0,
            cycle: 0,
            backgrounds: Default::default(),
            sprite_renderer: SpriteRenderer::new(),
            framebuffer: Framebuffer::new(),
            effects: Effects::default(),
            windows: Default::default(),
            winout_control: crate::ppu::effects::WindowControl::default(),
            objwin_control: crate::ppu::effects::WindowControl::default(),
            palette: [0; 512],
            vram: vec![0; VRAM_SIZE],
            oam: [0; 1024],
        }
    }

    /// Reset the PPU
    pub fn reset(&mut self) {
        self.scanline = 0;
        self.cycle = 0;
        self.display_control = DisplayControl::default();
        self.display_status = DisplayStatus::default();
        self.framebuffer.clear();
    }

    /// Step the PPU by one cycle
    pub fn step(&mut self) -> PpuEvents {
        self.step_cycles(1)
    }

    /// Step the PPU by multiple cycles, stopping only at timing boundaries.
    pub fn step_cycles(&mut self, mut cycles: u32) -> PpuEvents {
        let mut events = PpuEvents::default();

        while cycles != 0 {
            let boundary = if self.cycle < HBLANK_START {
                HBLANK_START
            } else {
                CYCLES_PER_SCANLINE
            };
            let advance = (boundary - self.cycle).min(cycles);
            self.cycle += advance;
            cycles -= advance;

            if self.cycle == HBLANK_START {
                self.display_status.hblank = true;

                if self.scanline < VISIBLE_SCANLINES {
                    self.render_scanline();
                }

                events.insert(PpuEvent::HBlank);
            }

            if self.cycle >= CYCLES_PER_SCANLINE {
                self.cycle = 0;
                self.display_status.hblank = false;
                self.scanline += 1;

                if self.scanline == VBLANK_START {
                    self.display_status.vblank = true;
                    events.insert(PpuEvent::VBlank);
                }

                if self.scanline >= TOTAL_SCANLINES {
                    self.scanline = 0;
                    self.display_status.vblank = false;
                    self.framebuffer.swap_buffers();
                    events.insert(PpuEvent::FrameComplete);
                }

                if self.scanline == self.display_status.vcounter_compare as u32 {
                    self.display_status.vcounter = true;
                    events.insert(PpuEvent::VCounter);
                } else {
                    self.display_status.vcounter = false;
                }
            }
        }

        events
    }

    /// Render the current scanline
    fn render_scanline(&mut self) {
        if self.display_control.forced_blank {
            return;
        }

        let y = self.scanline as usize;
        let backdrop = crate::ppu::render::Color::from_rgb555(self.palette[0]);

        // Precompute window visibility/effect masks before rendering layers so
        // disabled layers never overwrite lower visible content.
        self.apply_windows(y);

        // The GBA backdrop color comes from BG palette entry 0, not hardcoded black.
        self.framebuffer.fill_scanline(y, backdrop);

        // Render backgrounds based on mode
        match self.display_control.bg_mode {
            0 => self.render_mode0(y),
            1 => self.render_mode1(y),
            2 => self.render_mode2(y),
            3 => self.render_mode3(y),
            4 => self.render_mode4(y),
            5 => self.render_mode5(y),
            _ => {} // Mode 6-7 are invalid
        }

        // Render sprites
        if self.display_control.obj_enable {
            self.sprite_renderer.render_sprites(
                &mut self.framebuffer,
                &self.vram,
                &self.oam,
                &self.palette,
                y,
                self.display_control.bg_mode,
                self.display_control.obj_mapping,
            );
        }

        // Apply color effects
        self.apply_effects(y, backdrop);
    }

    /// Render mode 0 (4 text backgrounds)
    fn render_mode0(&mut self, y: usize) {
        for bg_id in 0..4 {
            if self.display_control.bg_enable[bg_id] {
                self.backgrounds[bg_id].render_text(
                    &mut self.framebuffer,
                    &self.vram,
                    &self.palette,
                    y,
                    bg_id,
                );
            }
        }
    }

    /// Render mode 1 (2 text + 1 affine)
    fn render_mode1(&mut self, y: usize) {
        // BG0 and BG1 are text
        for bg_id in 0..2 {
            if self.display_control.bg_enable[bg_id] {
                self.backgrounds[bg_id].render_text(
                    &mut self.framebuffer,
                    &self.vram,
                    &self.palette,
                    y,
                    bg_id,
                );
            }
        }
        // BG2 is affine
        if self.display_control.bg_enable[2] {
            self.backgrounds[2].render_affine(
                &mut self.framebuffer,
                &self.vram,
                &self.palette,
                y,
                2,
            );
        }
    }

    /// Render mode 2 (2 affine backgrounds)
    fn render_mode2(&mut self, y: usize) {
        // BG2 and BG3 are affine
        for bg_id in 2..4 {
            if self.display_control.bg_enable[bg_id] {
                self.backgrounds[bg_id].render_affine(
                    &mut self.framebuffer,
                    &self.vram,
                    &self.palette,
                    y,
                    bg_id,
                );
            }
        }
    }

    /// Render mode 3 (16-bit bitmap)
    fn render_mode3(&mut self, y: usize) {
        if self.display_control.bg_enable[2] {
            self.backgrounds[2].render_bitmap_mode3(
                &mut self.framebuffer,
                &self.vram,
                y,
                self.display_control.get_display_frame(),
            );
        }
    }

    /// Render mode 4 (8-bit bitmap, double-buffered)
    fn render_mode4(&mut self, y: usize) {
        if self.display_control.bg_enable[2] {
            self.backgrounds[2].render_bitmap_mode4(
                &mut self.framebuffer,
                &self.vram,
                &self.palette,
                y,
                self.display_control.get_display_frame(),
            );
        }
    }

    /// Render mode 5 (16-bit bitmap, 160x128, double-buffered)
    fn render_mode5(&mut self, y: usize) {
        if self.display_control.bg_enable[2] {
            self.backgrounds[2].render_bitmap_mode5(
                &mut self.framebuffer,
                &self.vram,
                y,
                self.display_control.get_display_frame(),
            );
        }
    }

    /// Precompute window masks for the scanline.
    fn apply_windows(&mut self, y: usize) {
        let any_win = self.display_control.window_enable[0]
            || self.display_control.window_enable[1]
            || self.display_control.window_enable[2];
        if !any_win {
            self.framebuffer
                .set_scanline_window_masks(y, &[0x3F; SCREEN_WIDTH]);
            return;
        }

        let mut obj_window_mask = [false; SCREEN_WIDTH];
        if self.display_control.window_enable[2] {
            self.sprite_renderer.render_obj_window_mask(
                &mut obj_window_mask,
                &self.vram,
                &self.oam,
                y,
                self.display_control.bg_mode,
                self.display_control.obj_mapping,
            );
        }

        let mut window_masks = [0u8; SCREEN_WIDTH];
        for x in 0..SCREEN_WIDTH {
            let window_control = if self.display_control.window_enable[0]
                && self.windows[0].contains(x as u16, y as u16)
            {
                self.windows[0].control
            } else if self.display_control.window_enable[1]
                && self.windows[1].contains(x as u16, y as u16)
            {
                self.windows[1].control
            } else if self.display_control.window_enable[2] && obj_window_mask[x] {
                self.objwin_control
            } else {
                // Outside all windows: use WINOUT control
                self.winout_control
            };

            let mut mask = 0u8;
            for layer in 0..5 {
                if window_control.is_enabled(layer) {
                    mask |= 1 << layer;
                }
            }
            if window_control.special_effect {
                mask |= 1 << 5;
            }
            window_masks[x] = mask;
        }

        self.framebuffer.set_scanline_window_masks(y, &window_masks);
    }

    /// Apply color effects (blending / brightness) to the scanline.
    fn apply_effects(&mut self, y: usize, backdrop: crate::ppu::render::Color) {
        self.effects.apply(&mut self.framebuffer, y, backdrop);
    }

    /// Read display control register
    pub fn read_dispcnt(&self) -> u16 {
        self.display_control.to_value()
    }

    /// Write display control register
    pub fn write_dispcnt(&mut self, value: u16) {
        self.display_control = DisplayControl::from_value(value);
    }

    /// Read display status register (low 16 bits)
    pub fn read_dispstat_low(&self) -> u16 {
        self.display_status.to_value_low()
    }

    /// Read display status register (high 16 bits)
    pub fn read_dispstat_high(&self) -> u16 {
        self.display_status.to_value_high()
    }

    /// Write display status register
    pub fn write_dispstat(&mut self, value: u16) {
        self.display_status = DisplayStatus::from_value_low(value);
    }

    /// Read V-Counter
    pub fn read_vcount(&self) -> u16 {
        self.scanline as u16
    }

    /// Read background control register
    pub fn read_bgcnt(&self, bg_id: usize) -> u16 {
        self.backgrounds[bg_id].control.to_value()
    }

    /// Write background control register
    pub fn write_bgcnt(&mut self, bg_id: usize, value: u16) {
        self.backgrounds[bg_id].control = BackgroundControl::from_value(value);
    }

    /// Read background horizontal offset
    pub fn read_bghofs(&self, bg_id: usize) -> u16 {
        self.backgrounds[bg_id].hoffset
    }

    /// Write background horizontal offset
    pub fn write_bghofs(&mut self, bg_id: usize, value: u16) {
        self.backgrounds[bg_id].hoffset = value & 0x1FF;
    }

    /// Read background vertical offset
    pub fn read_bgvofs(&self, bg_id: usize) -> u16 {
        self.backgrounds[bg_id].voffset
    }

    /// Write background vertical offset
    pub fn write_bgvofs(&mut self, bg_id: usize, value: u16) {
        self.backgrounds[bg_id].voffset = value & 0x1FF;
    }

    /// Read palette RAM
    pub fn read_palette(&self, addr: u32) -> u16 {
        let index = ((addr & 0x3FF) >> 1) as usize;
        self.palette[index]
    }

    /// Write palette RAM
    pub fn write_palette(&mut self, addr: u32, value: u16) {
        let index = ((addr & 0x3FF) >> 1) as usize;
        self.palette[index] = value;
    }

    /// Read VRAM
    pub fn read_vram(&self, addr: u32) -> u8 {
        let index = Self::vram_index(addr);
        if index < self.vram.len() {
            self.vram[index]
        } else {
            0
        }
    }

    /// Write VRAM
    pub fn write_vram(&mut self, addr: u32, value: u8) {
        let index = Self::vram_index(addr);
        if index < self.vram.len() {
            self.vram[index] = value;
        }
    }

    /// Read OAM
    pub fn read_oam(&self, addr: u32) -> u8 {
        let index = (addr & 0x3FF) as usize;
        self.oam[index]
    }

    /// Write OAM
    pub fn write_oam(&mut self, addr: u32, value: u8) {
        let index = (addr & 0x3FF) as usize;
        self.oam[index] = value;
        self.sprite_renderer.invalidate_cache();
    }

    /// Write WIN0H register (horizontal bounds of window 0)
    pub fn write_win0h(&mut self, value: u16) {
        self.windows[0].h_range = crate::ppu::effects::WindowH::from_value(value);
    }

    /// Write WIN1H register (horizontal bounds of window 1)
    pub fn write_win1h(&mut self, value: u16) {
        self.windows[1].h_range = crate::ppu::effects::WindowH::from_value(value);
    }

    /// Write WIN0V register (vertical bounds of window 0)
    pub fn write_win0v(&mut self, value: u16) {
        self.windows[0].v_range = crate::ppu::effects::WindowV::from_value(value);
    }

    /// Write WIN1V register (vertical bounds of window 1)
    pub fn write_win1v(&mut self, value: u16) {
        self.windows[1].v_range = crate::ppu::effects::WindowV::from_value(value);
    }

    /// Write WININ register (layer enables inside WIN0 and WIN1)
    pub fn write_winin(&mut self, value: u16) {
        self.windows[0].control = crate::ppu::effects::WindowControl::from_value(value & 0xFF);
        self.windows[1].control = crate::ppu::effects::WindowControl::from_value(value >> 8);
    }

    /// Write WINOUT register (layer enables outside windows and inside WINOBJ)
    pub fn write_winout(&mut self, value: u16) {
        self.winout_control = crate::ppu::effects::WindowControl::from_value(value & 0xFF);
        self.objwin_control = crate::ppu::effects::WindowControl::from_value(value >> 8);
    }

    /// Write BLDCNT register
    pub fn write_bldcnt(&mut self, value: u16) {
        self.effects.write_bldcnt(value);
    }

    /// Write BLDALPHA register
    pub fn write_bldalpha(&mut self, value: u16) {
        self.effects.write_bldalpha(value);
    }

    /// Write BLDY register
    pub fn write_bldy(&mut self, value: u16) {
        self.effects.write_bldy(value);
    }

    /// Get the framebuffer
    pub fn get_framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }
}

/// PPU events that can trigger interrupts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PpuEvent {
    /// HBlank started
    HBlank,
    /// VBlank started
    VBlank,
    /// V-Counter match
    VCounter,
    /// Frame complete
    FrameComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
/// Compact set of PPU edge events raised during a cycle step.
pub struct PpuEvents(u8);

impl PpuEvents {
    /// Return true when no PPU events were raised this step.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Add a PPU event to the set.
    pub fn insert(&mut self, event: PpuEvent) {
        let bit = match event {
            PpuEvent::HBlank => 1 << 0,
            PpuEvent::VBlank => 1 << 1,
            PpuEvent::VCounter => 1 << 2,
            PpuEvent::FrameComplete => 1 << 3,
        };
        self.0 |= bit;
    }

    /// Return true when the set contains the given event.
    pub fn contains(self, event: PpuEvent) -> bool {
        let bit = match event {
            PpuEvent::HBlank => 1 << 0,
            PpuEvent::VBlank => 1 << 1,
            PpuEvent::VCounter => 1 << 2,
            PpuEvent::FrameComplete => 1 << 3,
        };
        (self.0 & bit) != 0
    }
}

/// VRAM size in bytes
const VRAM_SIZE: usize = 96 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ppu_new() {
        let ppu = Ppu::new();
        assert_eq!(ppu.scanline, 0);
        assert_eq!(ppu.cycle, 0);
        assert_eq!(ppu.vram.len(), VRAM_SIZE);
    }

    #[test]
    fn test_display_control() {
        let value = 0x0F00; // BG0-BG3 enabled (bits 8-11)
        let dc = DisplayControl::from_value(value);
        assert_eq!(dc.bg_mode, 0);
        assert!(dc.bg_enable[0]);
        assert!(dc.bg_enable[1]);
        assert!(dc.bg_enable[2]);
        assert!(dc.bg_enable[3]);
        assert!(!dc.obj_enable);
    }

    #[test]
    fn test_display_status() {
        let mut ds = DisplayStatus::default();
        ds.vblank = true;
        ds.hblank = true;
        ds.vblank_irq = true;

        let value = ds.to_value_low();
        assert_eq!(value & 1, 1);
        assert_eq!(value & 2, 2);
        assert_eq!(value & (1 << 3), (1 << 3));
    }

    #[test]
    fn test_ppu_step() {
        let mut ppu = Ppu::new();
        // Enable HBlank IRQ so step() returns the event
        ppu.display_status.hblank_irq = true;

        // Step through HBlank boundary
        ppu.cycle = HBLANK_START - 1;
        let event = ppu.step();
        assert!(event.contains(PpuEvent::HBlank));
        assert!(ppu.display_status.hblank);
    }

    #[test]
    fn test_ppu_step_cycles_crosses_multiple_boundaries() {
        let mut ppu = Ppu::new();
        ppu.scanline = VBLANK_START - 1;
        ppu.cycle = HBLANK_START - 2;
        ppu.display_status.vcounter_compare = VBLANK_START as u8;

        let events = ppu.step_cycles((CYCLES_PER_SCANLINE - HBLANK_START) + 2);

        assert!(events.contains(PpuEvent::HBlank));
        assert!(events.contains(PpuEvent::VBlank));
        assert!(events.contains(PpuEvent::VCounter));
        assert_eq!(ppu.scanline, VBLANK_START);
        assert_eq!(ppu.cycle, 0);
    }

    #[test]
    fn test_vblank_and_vcounter_can_coincide() {
        let mut ppu = Ppu::new();
        ppu.scanline = VBLANK_START - 1;
        ppu.cycle = CYCLES_PER_SCANLINE - 1;
        ppu.display_status.vcounter_compare = VBLANK_START as u8;

        let events = ppu.step();

        assert!(events.contains(PpuEvent::VBlank));
        assert!(events.contains(PpuEvent::VCounter));
        assert!(ppu.display_status.vblank);
        assert!(ppu.display_status.vcounter);
    }

    #[test]
    fn test_frame_complete_and_vcounter_can_coincide_on_line_zero() {
        let mut ppu = Ppu::new();
        ppu.scanline = TOTAL_SCANLINES - 1;
        ppu.cycle = CYCLES_PER_SCANLINE - 1;
        ppu.display_status.vblank = true;
        ppu.display_status.vcounter_compare = 0;

        let events = ppu.step();

        assert!(events.contains(PpuEvent::FrameComplete));
        assert!(events.contains(PpuEvent::VCounter));
        assert_eq!(ppu.scanline, 0);
        assert!(!ppu.display_status.vblank);
        assert!(ppu.display_status.vcounter);
    }

    #[test]
    fn test_write_winout_updates_outside_and_obj_window_controls() {
        let mut ppu = Ppu::new();
        ppu.write_winout(0x3f12);

        assert_eq!(ppu.winout_control.to_value(), 0x12);
        assert_eq!(ppu.objwin_control.to_value(), 0x3f);
    }

    #[test]
    fn test_ppu_vram_upper_window_mirrors_to_upper_physical_block() {
        let mut ppu = Ppu::new();

        ppu.write_vram(0x0601_0000, 0x12);
        assert_eq!(ppu.read_vram(0x0601_8000), 0x12);

        ppu.write_vram(0x0601_8000, 0xAB);
        assert_eq!(ppu.read_vram(0x0601_0000), 0xAB);
    }
}

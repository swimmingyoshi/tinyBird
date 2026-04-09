//! Background Rendering
//!
//! This module implements the GBA's background layers:
//! - Text backgrounds (Modes 0-1): Tile-based with 8x8 tiles
//! - Affine backgrounds (Modes 1-2): Rotation/scaling transformation
//! - Bitmap backgrounds (Modes 3-5): Direct pixel data

use super::{Color, Framebuffer, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

/// Background control register
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BackgroundControl {
    /// Background priority (0-3)
    pub priority: u8,
    /// Character base address (0-3, multiplied by 16KB)
    pub character_base: u8,
    /// Mosaic enable
    pub mosaic: bool,
    /// Color mode (false = 16 colors, true = 256 colors)
    pub color_mode: bool,
    /// Screen base address (0-31, multiplied by 2KB)
    pub screen_base: u8,
    /// Area overflow mode (affine only)
    pub area_overflow: bool,
    /// Screen size (affine only, 0-3)
    pub screen_size: u8,
}

impl BackgroundControl {
    /// Parse from raw register value
    pub fn from_value(value: u16) -> Self {
        Self {
            priority: (value & 0x3) as u8,
            character_base: ((value >> 2) & 0x3) as u8,
            mosaic: (value & (1 << 6)) != 0,
            color_mode: (value & (1 << 7)) != 0,
            screen_base: ((value >> 8) & 0x1F) as u8,
            area_overflow: (value & (1 << 13)) != 0,
            screen_size: ((value >> 14) & 0x3) as u8,
        }
    }

    /// Convert to raw register value
    pub fn to_value(&self) -> u16 {
        let mut value = self.priority as u16;
        value |= (self.character_base as u16) << 2;
        if self.mosaic {
            value |= 1 << 6;
        }
        if self.color_mode {
            value |= 1 << 7;
        }
        value |= (self.screen_base as u16) << 8;
        if self.area_overflow {
            value |= 1 << 13;
        }
        value |= (self.screen_size as u16) << 14;
        value
    }

    /// Get character base address
    pub fn get_character_base(&self) -> usize {
        (self.character_base as usize) * 16 * 1024
    }

    /// Get screen base address
    pub fn get_screen_base(&self) -> usize {
        (self.screen_base as usize) * 2 * 1024
    }

    /// Get screen size in pixels (for affine backgrounds)
    pub fn get_screen_size(&self) -> (usize, usize) {
        match self.screen_size {
            0 => (128, 128),
            1 => (256, 256),
            2 => (512, 512),
            3 => (1024, 1024),
            _ => unreachable!(),
        }
    }

    /// Get tiles per row (for text backgrounds)
    pub fn get_tiles_per_row(&self) -> usize {
        match self.screen_size {
            0 | 1 => 32,
            2 | 3 => 64,
            _ => unreachable!(),
        }
    }
}

/// Text background (Modes 0-1)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Background {
    /// Background control register
    pub control: BackgroundControl,
    /// Horizontal offset (9 bits)
    pub hoffset: u16,
    /// Vertical offset (9 bits)
    pub voffset: u16,
    /// Affine transformation matrix PA
    pub pa: i16,
    /// Affine transformation matrix PB
    pub pb: i16,
    /// Affine transformation matrix PC
    pub pc: i16,
    /// Affine transformation matrix PD
    pub pd: i16,
    /// Affine reference point X
    pub ref_x: i32,
    /// Affine reference point Y
    pub ref_y: i32,
}

impl Background {
    /// Create a new background
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the background
    pub fn reset(&mut self) {
        self.control = BackgroundControl::default();
        self.hoffset = 0;
        self.voffset = 0;
        self.pa = 0;
        self.pb = 0;
        self.pc = 0;
        self.pd = 0;
        self.ref_x = 0;
        self.ref_y = 0;
    }

    /// Render a text background
    pub fn render_text(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        palette: &[u16],
        y: usize,
        bg_id: usize,
    ) {
        let priority = self.control.priority;
        let char_base = self.control.get_character_base();
        let screen_base = self.control.get_screen_base();
        let color_mode = self.control.color_mode;

        // Text BG map dimensions (in pixels), determined by screen_size
        let (map_width, map_height) = match self.control.screen_size {
            0 => (256usize, 256usize),
            1 => (512, 256),
            2 => (256, 512),
            3 => (512, 512),
            _ => (256, 256),
        };

        let scroll_x = self.hoffset as usize;
        let scroll_y = self.voffset as usize;

        for x in 0..SCREEN_WIDTH {
            // Wrap coordinates to the actual map size
            let map_x = (x + scroll_x) % map_width;
            let map_y = (y + scroll_y) % map_height;

            let tile_col = map_x / 8;
            let tile_row = map_y / 8;
            let pixel_x = map_x % 8;
            let pixel_y = map_y % 8;

            // Determine which screen block and local tile coordinates.
            // Each screen block is 32×32 tiles = 2KB.
            let (block_idx, local_col, local_row) = match self.control.screen_size {
                0 => (0usize, tile_col, tile_row),
                1 => {
                    let block = tile_col / 32;
                    (block, tile_col % 32, tile_row)
                }
                2 => {
                    let block = tile_row / 32;
                    (block, tile_col, tile_row % 32)
                }
                3 => {
                    let h_block = tile_col / 32;
                    let v_block = tile_row / 32;
                    // Layout: TL=0, TR=1, BL=2, BR=3
                    (v_block * 2 + h_block, tile_col % 32, tile_row % 32)
                }
                _ => (0, tile_col, tile_row),
            };

            let screen_offset = block_idx * 2048 + (local_row * 32 + local_col) * 2;
            if screen_base + screen_offset + 1 >= vram.len() {
                continue;
            }

            let map_entry = vram[screen_base + screen_offset] as u16
                | ((vram[screen_base + screen_offset + 1] as u16) << 8);

            let tile_num = (map_entry & 0x3FF) as usize;
            let h_flip = (map_entry & (1 << 10)) != 0;
            let v_flip = (map_entry & (1 << 11)) != 0;
            let palette_bank = ((map_entry >> 12) & 0xF) as usize;

            // Apply flipping
            let px = if h_flip { 7 - pixel_x } else { pixel_x };
            let py = if v_flip { 7 - pixel_y } else { pixel_y };

            // Get tile data
            let (color_index, pal_offset) = if color_mode {
                // 256 colors (8bpp) - 64 bytes per tile
                let tile_data_offset = char_base + tile_num * 64;
                let pixel_offset = py * 8 + px;
                if tile_data_offset + pixel_offset >= vram.len() {
                    continue;
                }
                (vram[tile_data_offset + pixel_offset] as usize, 0)
            } else {
                // 16 colors (4bpp) - 32 bytes per tile
                let tile_data_offset = char_base + tile_num * 32;
                let byte_offset = (py * 4) + (px / 2);
                if tile_data_offset + byte_offset >= vram.len() {
                    continue;
                }
                let byte = vram[tile_data_offset + byte_offset];
                let idx = if px % 2 == 0 {
                    (byte & 0x0F) as usize
                } else {
                    ((byte >> 4) & 0x0F) as usize
                };
                (idx, palette_bank * 16)
            };

            if color_index != 0 {
                let pal_idx = pal_offset + color_index;
                if pal_idx < palette.len() {
                    let color = Color::from_rgb555(palette[pal_idx]);
                    framebuffer.set_pixel_with_priority(x, y, color, priority, bg_id as u8, false);
                }
            }
        }
    }

    /// Render an affine background (rotation/scaling)
    pub fn render_affine(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        palette: &[u16],
        y: usize,
        bg_id: usize,
    ) {
        let (screen_width, screen_height) = self.control.get_screen_size();
        let priority = self.control.priority;
        let char_base = self.control.get_character_base();
        let screen_base = self.control.get_screen_base();

        // Calculate initial coordinates using affine transformation
        let mut x_coord = self.ref_x as i32;
        let mut y_coord = self.ref_y as i32;

        // Advance y coordinate by one scanline
        for _ in 0..y {
            x_coord += self.pb as i32;
            y_coord += self.pd as i32;
        }

        let dx = self.pa as i32;
        let dy = self.pc as i32;

        for x in 0..SCREEN_WIDTH {
            let tex_x = x_coord >> 8;
            let tex_y = y_coord >> 8;

            let (tex_x, tex_y) = if self.control.area_overflow {
                (
                    tex_x.rem_euclid(screen_width as i32) as usize,
                    tex_y.rem_euclid(screen_height as i32) as usize,
                )
            } else if tex_x < 0
                || tex_x >= screen_width as i32
                || tex_y < 0
                || tex_y >= screen_height as i32
            {
                x_coord += dx;
                y_coord += dy;
                continue;
            } else {
                (tex_x as usize, tex_y as usize)
            };

            let tile_x = tex_x / 8;
            let tile_y = tex_y / 8;
            // Affine BG maps are laid out as a flat 8-bit tilemap, unlike text
            // BGs which are split across 32x32 tile screen blocks.
            let tiles_per_row = screen_width / 8;
            let map_offset = tile_y * tiles_per_row + tile_x;
            if screen_base + map_offset >= vram.len() {
                x_coord += dx;
                y_coord += dy;
                continue;
            }

            let tile_num = vram[screen_base + map_offset] as usize;
            let tile_data_offset = char_base + tile_num * 64;
            let pixel_offset = (tex_y % 8) * 8 + (tex_x % 8);
            if tile_data_offset + pixel_offset >= vram.len() {
                x_coord += dx;
                y_coord += dy;
                continue;
            }

            let color_index = vram[tile_data_offset + pixel_offset] as usize;
            if color_index != 0 && color_index < palette.len() {
                let color = Color::from_rgb555(palette[color_index]);
                framebuffer.set_pixel_with_priority(x, y, color, priority, bg_id as u8, false);
            }

            // Advance x coordinate
            x_coord += dx;
            y_coord += dy;
        }
    }

    /// Render bitmap mode 3 (16-bit color, 240x160, single buffer)
    #[allow(unused_variables)]
    pub fn render_bitmap_mode3(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        y: usize,
        frame: usize,
    ) {
        // Mode 3: Single 16-bit bitmap starting at VRAM offset 0
        let base_addr = 0usize;
        let row_offset = y * SCREEN_WIDTH * 2;

        for x in 0..SCREEN_WIDTH {
            let addr = base_addr + row_offset + (x * 2);
            if addr + 1 < vram.len() {
                let color = vram[addr] as u16 | ((vram[addr + 1] as u16) << 8);
                framebuffer.set_pixel(x, y, Color::from_rgb555(color));
            }
        }
    }

    /// Render bitmap mode 4 (8-bit indexed, double-buffered, 240x160)
    pub fn render_bitmap_mode4(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        palette: &[u16],
        y: usize,
        frame: usize,
    ) {
        // Mode 4: 8-bit bitmap with page flipping
        // Frame 0 at VRAM offset 0x0000, frame 1 at 0xA000
        let base_addr = if frame == 0 { 0x0000usize } else { 0xA000 };
        let row_offset = y * SCREEN_WIDTH;

        for x in 0..SCREEN_WIDTH {
            let addr = base_addr + row_offset + x;
            if addr < vram.len() {
                let color_index = vram[addr] as usize;
                let color = Color::from_rgb555(palette[color_index]);
                framebuffer.set_pixel(x, y, color);
            }
        }
    }

    /// Render bitmap mode 5 (16-bit color, double-buffered, 160x128)
    pub fn render_bitmap_mode5(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        y: usize,
        frame: usize,
    ) {
        // Mode 5: 16-bit bitmap with page flipping, 160x128
        // Frame 0 at VRAM offset 0x0000, frame 1 at 0xA000
        if y >= 128 {
            return;
        }

        let base_addr = if frame == 0 { 0x0000usize } else { 0xA000 };
        let row_offset = y * 160 * 2;

        for x in 0..160 {
            let addr = base_addr + row_offset + (x * 2);
            if addr + 1 < vram.len() {
                let color = vram[addr] as u16 | ((vram[addr + 1] as u16) << 8);
                framebuffer.set_pixel(x, y, Color::from_rgb555(color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_background_control() {
        // bits 14-15 = 11 (screen size 3), bit 13 = area overflow
        let value = 0xE000;
        let bc = BackgroundControl::from_value(value);
        assert_eq!(bc.screen_size, 3);
        assert!(bc.area_overflow);

        let (w, h) = bc.get_screen_size();
        assert_eq!(w, 1024);
        assert_eq!(h, 1024);
    }

    #[test]
    fn test_background_control_flag_bits() {
        let value = 0xE0C0;
        let bc = BackgroundControl::from_value(value);
        assert!(bc.mosaic);
        assert!(bc.color_mode);
        assert!(bc.area_overflow);
        assert_eq!(bc.screen_size, 3);
        assert_eq!(bc.to_value() & 0xE0C0, value);
    }

    #[test]
    fn test_background_new() {
        let bg = Background::new();
        assert_eq!(bg.hoffset, 0);
        assert_eq!(bg.voffset, 0);
    }

    #[test]
    fn test_background_reset() {
        let mut bg = Background::new();
        bg.hoffset = 100;
        bg.voffset = 200;
        bg.reset();
        assert_eq!(bg.hoffset, 0);
        assert_eq!(bg.voffset, 0);
    }

    #[test]
    fn test_tiles_per_row() {
        let mut bc = BackgroundControl::default();
        bc.screen_size = 0;
        assert_eq!(bc.get_tiles_per_row(), 32);

        bc.screen_size = 2;
        assert_eq!(bc.get_tiles_per_row(), 64);
    }

    #[test]
    fn test_render_text_8bpp_uses_full_palette_index() {
        let mut bg = Background::new();
        bg.control.color_mode = true;

        let mut vram = vec![0u8; 128];
        let mut palette = [0u16; 512];
        let mut framebuffer = Framebuffer::new();

        vram[0] = 1;
        vram[64] = 5;
        palette[5] = Color::new(1, 2, 3).to_rgb555();

        bg.render_text(&mut framebuffer, &vram, &palette, 0, 0);
        framebuffer.swap_buffers();

        assert_eq!(framebuffer.get_pixel(0, 0), Some(Color::new(1, 2, 3)));
    }

    #[test]
    fn test_render_affine_uses_tilemap_entries() {
        let mut bg = Background::new();
        bg.control.area_overflow = true;
        bg.pa = 1 << 8;
        bg.pd = 1 << 8;

        let mut vram = vec![0u8; 256];
        let mut palette = [0u16; 512];
        let mut framebuffer = Framebuffer::new();

        vram[0] = 1;
        vram[64] = 7;
        palette[7] = Color::new(4, 5, 6).to_rgb555();

        bg.render_affine(&mut framebuffer, &vram, &palette, 0, 2);
        framebuffer.swap_buffers();

        assert_eq!(framebuffer.get_pixel(0, 0), Some(Color::new(4, 5, 6)));
    }

    #[test]
    fn test_render_affine_uses_linear_tilemap_for_large_maps() {
        let mut bg = Background::new();
        bg.control.area_overflow = true;
        bg.control.screen_size = 1; // 256x256, 32x32 affine map entries
        bg.ref_x = 128 << 8;
        bg.pa = 1 << 8;
        bg.pd = 1 << 8;

        let mut vram = vec![0u8; 512];
        let mut palette = [0u16; 512];
        let mut framebuffer = Framebuffer::new();

        // tex_x=128 selects tile_x=16 in a flat 32-entry row.
        vram[16] = 1;
        vram[64] = 9;
        palette[9] = Color::new(7, 8, 9).to_rgb555();

        bg.render_affine(&mut framebuffer, &vram, &palette, 0, 2);
        framebuffer.swap_buffers();

        assert_eq!(framebuffer.get_pixel(0, 0), Some(Color::new(7, 8, 9)));
    }
}

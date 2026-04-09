//! Sprite (OBJ) Rendering
//!
//! This module implements the GBA's sprite system:
//! - 128 sprites (objects)
//! - 8x8 to 64x64 pixel sizes
//! - 4bpp or 8bpp color modes
//! - Affine transformation support
//! - Priority and blending

use super::{Color, Framebuffer, SCREEN_HEIGHT, SCREEN_WIDTH};
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};

/// Number of OAM entries
pub const OAM_COUNT: usize = 128;
/// Size of each OAM entry in bytes
pub const OAM_ENTRY_SIZE: usize = 8;

/// Object attribute data
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ObjectAttribute {
    /// X coordinate (bits 0-8 for attr0, bits 0-7 of attr1)
    pub x: u16,
    /// Y coordinate
    pub y: u16,
    /// Affine matrix index (0-31)
    pub affine_index: u8,
    /// Double size flag (affine sprites only)
    pub double_size: bool,
    /// Disable sprite
    pub disabled: bool,
    /// Object mode (normal, affine, disabled, double-size affine)
    pub obj_mode: u8,
    /// Graphics mode (normal, semi-transparent, OBJ window, prohibited)
    pub gfx_mode: u8,
    /// Color mode (false = 16 colors, true = 256 colors)
    pub color_mode: bool,
    /// Shape (0-3)
    pub shape: u8,
    /// Size (0-3)
    pub size: u8,
    /// Tile number
    pub tile_num: u16,
    /// Priority (0-3)
    pub priority: u8,
    /// Palette number (for 16-color mode)
    pub palette: u8,
    /// Horizontal flip
    pub hflip: bool,
    /// Vertical flip
    pub vflip: bool,
    /// Mosaic enable
    pub mosaic: bool,
}

impl ObjectAttribute {
    /// Parse from OAM data (8 bytes)
    pub fn from_oam(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        let attr0 = (data[0] as u16) | ((data[1] as u16) << 8);
        let attr1 = (data[2] as u16) | ((data[3] as u16) << 8);
        let attr2 = (data[4] as u16) | ((data[5] as u16) << 8);
        let _attr3 = (data[6] as u16) | ((data[7] as u16) << 8);

        let y = attr0 & 0xFF;
        let obj_mode = ((attr0 >> 8) & 0x3) as u8;
        let affine = (attr0 & (1 << 8)) != 0;
        let gfx_mode = ((attr0 >> 10) & 0x3) as u8;
        let mosaic = (attr0 & (1 << 12)) != 0;
        let color_mode = (attr0 & (1 << 13)) != 0;
        let shape = ((attr0 >> 14) & 0x3) as u8;

        let x = attr1 & 0x1FF;
        let affine_index = ((attr1 >> 9) & 0x1F) as u8;
        let double_size = affine && (attr0 & (1 << 9)) != 0;
        let disabled = !affine && (attr0 & (1 << 9)) != 0;
        let size = ((attr1 >> 14) & 0x3) as u8;

        let tile_num = attr2 & 0x3FF;
        let priority = ((attr2 >> 10) & 0x3) as u8;
        let palette = ((attr2 >> 12) & 0xF) as u8;
        let hflip = obj_mode == 0 && (attr1 & (1 << 12)) != 0;
        let vflip = obj_mode == 0 && (attr1 & (1 << 13)) != 0;

        Some(Self {
            x,
            y,
            affine_index,
            double_size,
            disabled,
            obj_mode,
            gfx_mode,
            color_mode,
            shape,
            size,
            tile_num,
            priority,
            palette,
            hflip,
            vflip,
            mosaic,
        })
    }

    /// Return true when this OBJ uses affine coordinates.
    pub fn is_affine(&self) -> bool {
        self.obj_mode == 1 || self.obj_mode == 3
    }

    /// Return true when this OBJ should be alpha blended.
    pub fn is_semi_transparent(&self) -> bool {
        self.gfx_mode == 1
    }

    /// Return true when this OBJ is an OBJ-window mask and not a visible sprite.
    pub fn is_obj_window(&self) -> bool {
        self.gfx_mode == 2
    }

    /// Get sprite dimensions based on shape and size
    pub fn get_dimensions(&self) -> (u16, u16) {
        let size_table = [8, 16, 32, 64];

        match self.shape {
            0 => {
                // Square
                let size = size_table[self.size as usize];
                (size, size)
            }
            1 => {
                // Wide (horizontal): wider than tall
                let sizes = [(16, 8), (32, 8), (32, 16), (64, 32)];
                sizes[self.size as usize]
            }
            2 => {
                // Tall (vertical): taller than wide
                let sizes = [(8, 16), (8, 32), (16, 32), (32, 64)];
                sizes[self.size as usize]
            }
            3 => {
                // Invalid/prohibited
                (8, 8)
            }
            _ => unreachable!(),
        }
    }

    /// Check if this sprite is on the given scanline
    pub fn is_on_scanline(&self, y: u16) -> bool {
        if self.disabled {
            return false;
        }

        let (_width, mut height) = self.get_dimensions();
        if self.double_size && self.is_affine() {
            height *= 2;
        }

        let sprite_y = if self.y >= 160 {
            self.y as i32 - 256
        } else {
            self.y as i32
        };
        let y = y as i32;
        y >= sprite_y && y < sprite_y + height as i32
    }
}

/// Sprite renderer
#[derive(Clone)]
pub struct SpriteRenderer {
    /// Cached sprite attributes
    sprites: [Option<ObjectAttribute>; OAM_COUNT],
    /// Cache validity flag
    cache_valid: bool,
}

#[derive(Serialize, Deserialize)]
struct SpriteRendererSerde {
    sprites: Vec<Option<ObjectAttribute>>,
    cache_valid: bool,
}

impl Serialize for SpriteRenderer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SpriteRendererSerde {
            sprites: self.sprites.to_vec(),
            cache_valid: self.cache_valid,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SpriteRenderer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = SpriteRendererSerde::deserialize(deserializer)?;
        let sprites: [Option<ObjectAttribute>; OAM_COUNT] = helper
            .sprites
            .try_into()
            .map_err(|v: Vec<Option<ObjectAttribute>>| {
                D::Error::custom(format!(
                    "sprite cache had length {}, expected {}",
                    v.len(),
                    OAM_COUNT
                ))
            })?;
        Ok(Self {
            sprites,
            cache_valid: helper.cache_valid,
        })
    }
}

impl Default for SpriteRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpriteRenderer {
    /// Create a new sprite renderer
    pub fn new() -> Self {
        Self {
            sprites: [None; OAM_COUNT],
            cache_valid: false,
        }
    }

    /// Invalidate the sprite cache
    pub fn invalidate_cache(&mut self) {
        self.cache_valid = false;
    }

    /// Update sprite cache from OAM
    fn update_cache(&mut self, oam: &[u8]) {
        for i in 0..OAM_COUNT {
            let offset = i * OAM_ENTRY_SIZE;
            self.sprites[i] = ObjectAttribute::from_oam(&oam[offset..offset + OAM_ENTRY_SIZE]);
        }
        self.cache_valid = true;
    }

    /// Render all sprites for a scanline
    pub fn render_sprites(
        &mut self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        oam: &[u8],
        palette: &[u16],
        y: usize,
        bg_mode: u8,
        obj_mapping: bool,
    ) {
        if !self.cache_valid {
            self.update_cache(oam);
        }

        let y = y as u16;

        // Collect all sprites on this scanline
        let mut sprite_list: Vec<(usize, ObjectAttribute)> = Vec::new();

        for (i, sprite_opt) in self.sprites.iter().enumerate() {
            if let Some(sprite) = sprite_opt {
                if !sprite.is_obj_window() && sprite.is_on_scanline(y) {
                    sprite_list.push((i, *sprite));
                }
            }
        }

        // Sort by priority and OAM index (lower index = higher priority)
        sprite_list.sort_by(|a, b| {
            let prio_a = a.1.priority;
            let prio_b = b.1.priority;
            if prio_a != prio_b {
                prio_a.cmp(&prio_b)
            } else {
                // Lower OAM index has higher priority — sort ascending so rev()
                // renders index 0 last (on top).
                a.0.cmp(&b.0)
            }
        });

        // Render sprites (reverse order so higher priority renders on top)
        for (_, sprite) in sprite_list.iter().rev() {
            self.render_sprite(framebuffer, vram, oam, palette, sprite, y, bg_mode, obj_mapping);
        }
    }

    /// Render OBJ-window sprites into a per-pixel coverage mask for one scanline.
    pub fn render_obj_window_mask(
        &mut self,
        mask: &mut [bool; SCREEN_WIDTH],
        vram: &[u8],
        oam: &[u8],
        y: usize,
        bg_mode: u8,
        obj_mapping: bool,
    ) {
        if !self.cache_valid {
            self.update_cache(oam);
        }

        let y = y as u16;
        for sprite in self.sprites.iter().flatten() {
            if sprite.is_obj_window() && sprite.is_on_scanline(y) {
                self.mark_obj_window_sprite(mask, vram, oam, sprite, y, bg_mode, obj_mapping);
            }
        }
    }

    /// Read the affine matrix (PA, PB, PC, PD) for the given affine_index.
    ///
    /// The GBA stores each 2×2 matrix spread across the `attr3` halfword (bytes
    /// 6-7) of four consecutive OAM entries:
    ///   PA → entry (index*4 + 0), bytes 6-7
    ///   PB → entry (index*4 + 1), bytes 6-7
    ///   PC → entry (index*4 + 2), bytes 6-7
    ///   PD → entry (index*4 + 3), bytes 6-7
    fn read_affine_matrix(&self, oam: &[u8], index: u8) -> (i16, i16, i16, i16) {
        let base = (index as usize) * 4 * OAM_ENTRY_SIZE;
        let read = |off: usize| -> i16 {
            if off + 1 < oam.len() {
                i16::from_le_bytes([oam[off], oam[off + 1]])
            } else {
                0
            }
        };
        let pa = read(base + 6);
        let pb = read(base + OAM_ENTRY_SIZE + 6);
        let pc = read(base + OAM_ENTRY_SIZE * 2 + 6);
        let pd = read(base + OAM_ENTRY_SIZE * 3 + 6);
        (pa, pb, pc, pd)
    }

    /// Render a single sprite
    fn render_sprite(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        oam: &[u8],
        palette: &[u16],
        sprite: &ObjectAttribute,
        scanline: u16,
        bg_mode: u8,
        obj_mapping: bool,
    ) {
        let (orig_w, orig_h) = sprite.get_dimensions();
        let (mut width, mut height) = (orig_w, orig_h);
        if sprite.double_size && sprite.is_affine() {
            width *= 2;
            height *= 2;
        }

        let sprite_x = if sprite.x >= 256 {
            sprite.x as i32 - 512
        } else {
            sprite.x as i32
        };
        let sprite_y = if sprite.y >= 160 {
            sprite.y as i32 - 256
        } else {
            sprite.y as i32
        };

        // Handle affine transformation
        if sprite.is_affine() {
            let render_x = sprite_x - ((width as i32 - orig_w as i32) / 2);
            let render_y = sprite_y - ((height as i32 - orig_h as i32) / 2);
            let local_y = scanline as i32 - render_y;
            let (pa, pb, pc, pd) = self.read_affine_matrix(oam, sprite.affine_index);
            self.render_affine_sprite_pixels(
                framebuffer,
                vram,
                palette,
                sprite,
                render_x,
                scanline as i32,
                local_y,
                width as i32,
                height as i32,
                orig_w as i32,
                orig_h as i32,
                bg_mode,
                obj_mapping,
                pa,
                pb,
                pc,
                pd,
            );
        } else {
            // Normal sprite with flip support
            let local_y = scanline as i32 - sprite_y;
            let flip_x = sprite.hflip;
            let flip_y = sprite.vflip;

            self.render_sprite_pixels(
                framebuffer,
                vram,
                palette,
                sprite,
                sprite_x,
                scanline as i32,
                local_y,
                width as i32,
                height as i32,
                bg_mode,
                obj_mapping,
                flip_x,
                flip_y,
            );
        }
    }

    /// Mark the scanline pixels covered by an OBJ-window sprite.
    fn mark_obj_window_sprite(
        &self,
        mask: &mut [bool; SCREEN_WIDTH],
        vram: &[u8],
        oam: &[u8],
        sprite: &ObjectAttribute,
        scanline: u16,
        bg_mode: u8,
        obj_mapping: bool,
    ) {
        let (orig_w, orig_h) = sprite.get_dimensions();
        let (mut width, mut height) = (orig_w, orig_h);
        if sprite.double_size && sprite.is_affine() {
            width *= 2;
            height *= 2;
        }

        let sprite_x = if sprite.x >= 256 {
            sprite.x as i32 - 512
        } else {
            sprite.x as i32
        };
        let sprite_y = if sprite.y >= 160 {
            sprite.y as i32 - 256
        } else {
            sprite.y as i32
        };

        if sprite.is_affine() {
            let render_x = sprite_x - ((width as i32 - orig_w as i32) / 2);
            let render_y = sprite_y - ((height as i32 - orig_h as i32) / 2);
            let local_y = scanline as i32 - render_y;
            let (pa, pb, pc, pd) = self.read_affine_matrix(oam, sprite.affine_index);
            self.mark_affine_obj_window_pixels(
                mask,
                vram,
                sprite,
                render_x,
                scanline as i32,
                local_y,
                width as i32,
                height as i32,
                orig_w as i32,
                orig_h as i32,
                bg_mode,
                obj_mapping,
                pa,
                pb,
                pc,
                pd,
            );
        } else {
            let local_y = scanline as i32 - sprite_y;
            self.mark_obj_window_pixels(
                mask,
                vram,
                sprite,
                sprite_x,
                scanline as i32,
                local_y,
                width as i32,
                height as i32,
                bg_mode,
                obj_mapping,
                sprite.hflip,
                sprite.vflip,
            );
        }
    }

    /// Render an affine-transformed sprite for one scanline.
    ///
    /// For each pixel in the (possibly double-sized) bounding box, the texture
    /// coordinate is computed by applying the inverse affine matrix supplied by
    /// the game.  The matrix coefficients are 8.8 signed fixed-point.
    #[allow(clippy::too_many_arguments)]
    fn render_affine_sprite_pixels(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        palette: &[u16],
        sprite: &ObjectAttribute,
        start_x: i32,
        scanline_y: i32,
        local_y: i32,
        box_width: i32,
        box_height: i32,
        orig_width: i32,
        orig_height: i32,
        bg_mode: u8,
        obj_mapping: bool,
        pa: i16,
        pb: i16,
        pc: i16,
        pd: i16,
    ) {
        if local_y < 0 || local_y >= box_height {
            return;
        }
        if scanline_y < 0 || scanline_y >= super::SCREEN_HEIGHT as i32 {
            return;
        }

        let color_mode = sprite.color_mode;
        let tile_num = if color_mode {
            (sprite.tile_num as usize) & !1
        } else {
            sprite.tile_num as usize
        };
        let priority = sprite.priority;
        let bytes_per_tile = if color_mode { 64usize } else { 32 };
        let obj_base: usize = if bg_mode <= 2 { 0x10000 } else { 0x14000 };
        let tiles_wide = (orig_width as usize / 8).max(1);
        let row_stride_bytes = if obj_mapping {
            tiles_wide * bytes_per_tile
        } else {
            32 * 32
        };

        // Centre of the bounding box in local (sprite-relative) coordinates.
        let box_cx = box_width / 2;
        let box_cy = box_height / 2;
        // dy is fixed for the whole scanline.
        let dy = local_y - box_cy;

        // Texture centre (half of the unscaled sprite dimensions).
        let tex_cx = orig_width / 2;
        let tex_cy = orig_height / 2;

        for px in 0..box_width {
            let x = start_x + px;
            if x < 0 || x >= super::SCREEN_WIDTH as i32 {
                continue;
            }

            let dx = px - box_cx;

            // Inverse affine transform → texture coordinates (8.8 fixed-point).
            let tex_x = ((pa as i32 * dx + pb as i32 * dy) >> 8) + tex_cx;
            let tex_y = ((pc as i32 * dx + pd as i32 * dy) >> 8) + tex_cy;

            if tex_x < 0 || tex_x >= orig_width || tex_y < 0 || tex_y >= orig_height {
                continue;
            }

            let tex_x = tex_x as usize;
            let tex_y = tex_y as usize;

            let tile_col = tex_x / 8;
            let tile_row = tex_y / 8;
            let tile_addr = obj_base
                + tile_num * 32
                + tile_row * row_stride_bytes
                + tile_col * bytes_per_tile;

            let pixel_x = tex_x % 8;
            let pixel_y = tex_y % 8;

            let color_index = if color_mode {
                let addr = tile_addr + pixel_y * 8 + pixel_x;
                if addr >= vram.len() {
                    continue;
                }
                vram[addr]
            } else {
                let addr = tile_addr + pixel_y * 4 + (pixel_x / 2);
                if addr >= vram.len() {
                    continue;
                }
                let byte = vram[addr];
                if (pixel_x & 1) == 0 {
                    byte & 0x0F
                } else {
                    (byte >> 4) & 0x0F
                }
            };

            if color_index != 0 {
                let color = if color_mode {
                    Color::from_rgb555(palette[256 + color_index as usize])
                } else {
                    Color::from_rgb555(
                        palette[256 + sprite.palette as usize * 16 + color_index as usize],
                    )
                };
                framebuffer.set_pixel_with_attrs(
                    x as usize,
                    scanline_y as usize,
                    color,
                    priority,
                    4,
                    true,
                    sprite.is_semi_transparent(),
                );
            }
        }
    }

    /// Mark affine OBJ-window coverage for one scanline.
    #[allow(clippy::too_many_arguments)]
    fn mark_affine_obj_window_pixels(
        &self,
        mask: &mut [bool; SCREEN_WIDTH],
        vram: &[u8],
        sprite: &ObjectAttribute,
        start_x: i32,
        scanline_y: i32,
        local_y: i32,
        box_width: i32,
        box_height: i32,
        orig_width: i32,
        orig_height: i32,
        bg_mode: u8,
        obj_mapping: bool,
        pa: i16,
        pb: i16,
        pc: i16,
        pd: i16,
    ) {
        if local_y < 0 || local_y >= box_height {
            return;
        }
        if scanline_y < 0 || scanline_y >= super::SCREEN_HEIGHT as i32 {
            return;
        }

        let color_mode = sprite.color_mode;
        let tile_num = if color_mode {
            (sprite.tile_num as usize) & !1
        } else {
            sprite.tile_num as usize
        };
        let bytes_per_tile = if color_mode { 64usize } else { 32 };
        let obj_base: usize = if bg_mode <= 2 { 0x10000 } else { 0x14000 };
        let tiles_wide = (orig_width as usize / 8).max(1);
        let row_stride_bytes = if obj_mapping {
            tiles_wide * bytes_per_tile
        } else {
            32 * 32
        };

        let box_cx = box_width / 2;
        let box_cy = box_height / 2;
        let dy = local_y - box_cy;

        let tex_cx = orig_width / 2;
        let tex_cy = orig_height / 2;

        for px in 0..box_width {
            let x = start_x + px;
            if x < 0 || x >= super::SCREEN_WIDTH as i32 {
                continue;
            }

            let dx = px - box_cx;
            let tex_x = ((pa as i32 * dx + pb as i32 * dy) >> 8) + tex_cx;
            let tex_y = ((pc as i32 * dx + pd as i32 * dy) >> 8) + tex_cy;

            if tex_x < 0 || tex_x >= orig_width || tex_y < 0 || tex_y >= orig_height {
                continue;
            }

            let tex_x = tex_x as usize;
            let tex_y = tex_y as usize;
            let tile_col = tex_x / 8;
            let tile_row = tex_y / 8;
            let tile_addr = obj_base
                + tile_num * 32
                + tile_row * row_stride_bytes
                + tile_col * bytes_per_tile;

            let pixel_x = tex_x % 8;
            let pixel_y = tex_y % 8;

            let color_index = if color_mode {
                let addr = tile_addr + pixel_y * 8 + pixel_x;
                if addr >= vram.len() {
                    continue;
                }
                vram[addr]
            } else {
                let addr = tile_addr + pixel_y * 4 + (pixel_x / 2);
                if addr >= vram.len() {
                    continue;
                }
                let byte = vram[addr];
                if (pixel_x & 1) == 0 {
                    byte & 0x0F
                } else {
                    (byte >> 4) & 0x0F
                }
            };

            if color_index != 0 {
                mask[x as usize] = true;
            }
        }
    }

    /// Render sprite pixels
    fn render_sprite_pixels(
        &self,
        framebuffer: &mut Framebuffer,
        vram: &[u8],
        palette: &[u16],
        sprite: &ObjectAttribute,
        start_x: i32,
        scanline_y: i32,
        local_y: i32,
        width: i32,
        height: i32,
        bg_mode: u8,
        obj_mapping: bool,
        flip_x: bool,
        flip_y: bool,
    ) {
        let color_mode = sprite.color_mode;
        let tile_num = if color_mode {
            (sprite.tile_num as usize) & !1
        } else {
            sprite.tile_num as usize
        };
        let priority = sprite.priority;
        let is_obj = true;
        let bytes_per_tile = if color_mode { 64 } else { 32 };
        let obj_base = if bg_mode <= 2 { 0x10000 } else { 0x14000 };
        let tiles_wide = (width as usize / 8).max(1);
        let row_stride_bytes = if obj_mapping {
            tiles_wide * bytes_per_tile
        } else {
            32 * 32
        };
        if local_y < 0 || local_y >= height {
            return;
        }
        let tile_y = if flip_y {
            (height - 1 - local_y) as usize
        } else {
            local_y as usize
        };

        for px in 0..width {
            let x = start_x + px;

            // Check bounds
            if x < 0 || x >= SCREEN_WIDTH as i32 || scanline_y < 0 || scanline_y >= SCREEN_HEIGHT as i32 {
                continue;
            }

            let x = x as usize;
            let y = scanline_y as usize;

            // Calculate tile coordinates
            let tile_x = if flip_x {
                (width - 1 - px) as usize
            } else {
                px as usize
            };

            // Get tile number (handle 2D vs 1D mapping)
            let tile_row = tile_y / 8;
            let tile_col = tile_x / 8;
            let tile_addr = obj_base
                + tile_num * 32
                + tile_row * row_stride_bytes
                + tile_col * bytes_per_tile;

            // Get pixel within tile
            let pixel_x = tile_x % 8;
            let pixel_y = tile_y % 8;

            // Get color index
            let color_index = if color_mode {
                // 256 colors (8bpp)
                let pixel_offset = pixel_y * 8 + pixel_x;
                let addr = tile_addr + pixel_offset;
                if addr >= vram.len() {
                    continue;
                }
                vram[addr]
            } else {
                // 16 colors (4bpp)
                let byte_offset = pixel_y * 4 + (pixel_x / 2);
                let addr = tile_addr + byte_offset;
                if addr >= vram.len() {
                    continue;
                }
                let byte = vram[addr];
                if (pixel_x & 1) == 0 {
                    byte & 0x0F
                } else {
                    (byte >> 4) & 0x0F
                }
            };

            if color_index != 0 {
                let color = if color_mode {
                    Color::from_rgb555(palette[256 + color_index as usize])
                } else {
                    Color::from_rgb555(
                        palette[256 + sprite.palette as usize * 16 + color_index as usize],
                    )
                };
                framebuffer.set_pixel_with_attrs(
                    x,
                    y,
                    color,
                    priority,
                    4,
                    is_obj,
                    sprite.is_semi_transparent(),
                );
            }
        }
    }

    /// Mark non-affine OBJ-window coverage for one scanline.
    #[allow(clippy::too_many_arguments)]
    fn mark_obj_window_pixels(
        &self,
        mask: &mut [bool; SCREEN_WIDTH],
        vram: &[u8],
        sprite: &ObjectAttribute,
        start_x: i32,
        scanline_y: i32,
        local_y: i32,
        width: i32,
        height: i32,
        bg_mode: u8,
        obj_mapping: bool,
        flip_x: bool,
        flip_y: bool,
    ) {
        let color_mode = sprite.color_mode;
        let tile_num = if color_mode {
            (sprite.tile_num as usize) & !1
        } else {
            sprite.tile_num as usize
        };
        let bytes_per_tile = if color_mode { 64 } else { 32 };
        let obj_base = if bg_mode <= 2 { 0x10000 } else { 0x14000 };
        let tiles_wide = (width as usize / 8).max(1);
        let row_stride_bytes = if obj_mapping {
            tiles_wide * bytes_per_tile
        } else {
            32 * 32
        };
        if local_y < 0 || local_y >= height || scanline_y < 0 || scanline_y >= SCREEN_HEIGHT as i32
        {
            return;
        }
        let tile_y = if flip_y {
            (height - 1 - local_y) as usize
        } else {
            local_y as usize
        };

        for px in 0..width {
            let x = start_x + px;
            if x < 0 || x >= SCREEN_WIDTH as i32 {
                continue;
            }

            let tile_x = if flip_x {
                (width - 1 - px) as usize
            } else {
                px as usize
            };
            let tile_row = tile_y / 8;
            let tile_col = tile_x / 8;
            let tile_addr = obj_base
                + tile_num * 32
                + tile_row * row_stride_bytes
                + tile_col * bytes_per_tile;
            let pixel_x = tile_x % 8;
            let pixel_y = tile_y % 8;

            let color_index = if color_mode {
                let addr = tile_addr + pixel_y * 8 + pixel_x;
                if addr >= vram.len() {
                    continue;
                }
                vram[addr]
            } else {
                let addr = tile_addr + pixel_y * 4 + (pixel_x / 2);
                if addr >= vram.len() {
                    continue;
                }
                let byte = vram[addr];
                if (pixel_x & 1) == 0 {
                    byte & 0x0F
                } else {
                    (byte >> 4) & 0x0F
                }
            };

            if color_index != 0 {
                mask[x as usize] = true;
            }
        }
    }
}

/// Sprite data for rendering
#[derive(Debug, Clone)]
pub struct Sprite {
    /// Sprite attributes
    pub attr: ObjectAttribute,
    /// Pixel data (flattened)
    pub pixels: Vec<u8>,
}

impl Sprite {
    /// Create a new sprite
    pub fn new(attr: ObjectAttribute) -> Self {
        let (width, height) = attr.get_dimensions();
        let pixel_count = (width * height) as usize;

        Self {
            attr,
            pixels: vec![0; pixel_count],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_attribute_from_oam() {
        let data = [
            0x00, 0x00, // attr0: y=0, mode=normal, color=16
            0x00, 0x00, // attr1: x=0, size=8x8
            0x00, 0x00, // attr2: tile=0, priority=0
            0x00, 0x00, // attr3: palette=0
        ];

        let sprite = ObjectAttribute::from_oam(&data).unwrap();
        assert_eq!(sprite.x, 0);
        assert_eq!(sprite.y, 0);
        assert_eq!(sprite.size, 0);
        assert!(!sprite.disabled);
        assert_eq!(sprite.gfx_mode, 0);
        assert!(!sprite.is_semi_transparent());
    }

    #[test]
    fn test_object_attribute_semi_transparent_mode() {
        let data = [
            0x00, 0x04, // attr0: y=0, gfx mode=semi-transparent
            0x00, 0x00, // attr1: x=0, size=8x8
            0x00, 0x00, // attr2: tile=0, priority=0
            0x00, 0x00,
        ];

        let sprite = ObjectAttribute::from_oam(&data).unwrap();
        assert_eq!(sprite.gfx_mode, 1);
        assert!(sprite.is_semi_transparent());
    }

    #[test]
    fn test_sprite_dimensions() {
        let mut sprite = ObjectAttribute::default();

        // Square shapes
        sprite.shape = 0;
        sprite.size = 0;
        assert_eq!(sprite.get_dimensions(), (8, 8));

        sprite.size = 1;
        assert_eq!(sprite.get_dimensions(), (16, 16));

        sprite.size = 2;
        assert_eq!(sprite.get_dimensions(), (32, 32));

        sprite.size = 3;
        assert_eq!(sprite.get_dimensions(), (64, 64));

        // Wide (horizontal) shapes
        sprite.shape = 1;
        sprite.size = 0;
        assert_eq!(sprite.get_dimensions(), (16, 8));

        sprite.size = 1;
        assert_eq!(sprite.get_dimensions(), (32, 8));
    }

    #[test]
    fn test_sprite_on_scanline() {
        let mut sprite = ObjectAttribute::default();
        sprite.y = 10;
        sprite.disabled = false;

        sprite.shape = 0;
        sprite.size = 0; // 8x8

        assert!(sprite.is_on_scanline(10));
        assert!(sprite.is_on_scanline(15));
        assert!(!sprite.is_on_scanline(18));
        assert!(!sprite.is_on_scanline(9));
    }

    #[test]
    fn test_sprite_renderer_new() {
        let renderer = SpriteRenderer::new();
        assert!(!renderer.cache_valid);
    }

    #[test]
    fn test_obj_window_sprites_populate_window_mask() {
        let mut renderer = SpriteRenderer::new();
        let mut vram = vec![0u8; 0x18000];
        let mut oam = [0u8; 1024];
        let mut mask = [false; SCREEN_WIDTH];

        // OBJ tile 0 with visible texels in mode 0 OBJ VRAM.
        vram[0x10000..0x10020].fill(0x11);

        // Sprite 0 at (0, 0), normal shape/size, gfx mode = OBJ window.
        let attr0 = 0x0800u16;
        let attr1 = 0x0000u16;
        let attr2 = 0x0000u16;
        oam[0..2].copy_from_slice(&attr0.to_le_bytes());
        oam[2..4].copy_from_slice(&attr1.to_le_bytes());
        oam[4..6].copy_from_slice(&attr2.to_le_bytes());

        renderer.render_obj_window_mask(&mut mask, &vram, &oam, 0, 0, true);

        assert!(mask[..8].iter().all(|&covered| covered));
        assert!(mask[8..].iter().all(|&covered| !covered));
    }

}

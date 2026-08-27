//! Framebuffer and Rendering
//!
//! This module provides the framebuffer structure and color conversion utilities.

use super::SCREEN_HEIGHT;
use super::SCREEN_WIDTH;
use serde::{Deserialize, Serialize};

/// Number of pixels in the framebuffer
pub const PIXEL_COUNT: usize = SCREEN_WIDTH * SCREEN_HEIGHT;
const WINDOW_MASK_ALL_LAYERS: u8 = 0x3F;

/// RGB555 color (15-bit + 1 unused bit)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    /// Red component (5 bits)
    pub r: u8,
    /// Green component (5 bits)
    pub g: u8,
    /// Blue component (5 bits)
    pub b: u8,
}

impl Color {
    /// Create a new color from RGB555 components
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r & 0x1F,
            g: g & 0x1F,
            b: b & 0x1F,
        }
    }

    /// Create from RGB555 u16 value
    pub fn from_rgb555(value: u16) -> Self {
        Self {
            r: (value & 0x1F) as u8,
            g: ((value >> 5) & 0x1F) as u8,
            b: ((value >> 10) & 0x1F) as u8,
        }
    }

    /// Convert to RGB555 u16 value
    pub fn to_rgb555(&self) -> u16 {
        (self.r as u16) | ((self.g as u16) << 5) | ((self.b as u16) << 10)
    }

    /// Convert to RGB888 (for display)
    pub fn to_rgb888(&self) -> (u8, u8, u8) {
        // Expand 5-bit channels with bit replication. This matches the common
        // RGB555 -> RGB888 conversion used by emulators while avoiding a divide
        // in the hot display path.
        let scale = |c: u8| (c << 3) | (c >> 2);
        (scale(self.r), scale(self.g), scale(self.b))
    }

    /// Create from RGB888
    pub fn from_rgb888(r: u8, g: u8, b: u8) -> Self {
        // Scale 8-bit to 5-bit (divide by 255/31 ≈ 8.225)
        let scale = |c: u8| ((c as u16 * 31) / 255) as u8;
        Self::new(scale(r), scale(g), scale(b))
    }

    /// Black color (transparent)
    pub const BLACK: Self = Self::new(0, 0, 0);

    /// White color
    pub const WHITE: Self = Self::new(31, 31, 31);
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

/// Pixel with priority information for compositing
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pixel {
    /// Color
    pub color: Color,
    /// Priority (0-3, lower = higher priority)
    pub priority: u8,
    /// Layer type (0-3 = BG, 4 = OBJ, 5 = backdrop)
    pub layer: u8,
    /// Is object (sprite)
    pub is_obj: bool,
    /// Semi-transparent (for OBJ or special effects)
    pub semi_transparent: bool,
    /// Whether windowing allows special effects on this pixel
    pub effects_enabled: bool,
    /// Underlying pixel color captured when a semi-transparent OBJ overwrote it
    pub under_color: Color,
    /// Underlying pixel layer
    pub under_layer: u8,
    /// Whether the underlying pixel is an OBJ
    pub under_is_obj: bool,
    /// Whether the underlying pixel info is valid
    pub has_under_pixel: bool,
}

impl Default for Pixel {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            priority: 4, // Lowest priority (backdrop)
            layer: 5,    // Backdrop
            is_obj: false,
            semi_transparent: false,
            effects_enabled: true,
            under_color: Color::BLACK,
            under_layer: 5,
            under_is_obj: false,
            has_under_pixel: false,
        }
    }
}

/// Framebuffer with double buffering
#[derive(Clone, Serialize, Deserialize)]
pub struct Framebuffer {
    /// Front buffer (currently displayed)
    front_buffer: Vec<Pixel>,
    /// Back buffer (being rendered)
    back_buffer: Vec<Pixel>,
    /// Per-pixel window layer/effect mask for the current frame.
    window_masks: Vec<u8>,
    /// Line buffers for each priority level
    line_buffers: Vec<Vec<Pixel>>,
}

impl Default for Framebuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framebuffer {
    /// Create a new framebuffer
    pub fn new() -> Self {
        let pixel_count = PIXEL_COUNT;
        Self {
            front_buffer: vec![Pixel::default(); pixel_count],
            back_buffer: vec![Pixel::default(); pixel_count],
            window_masks: vec![WINDOW_MASK_ALL_LAYERS; pixel_count],
            line_buffers: vec![Vec::with_capacity(SCREEN_WIDTH); 4], // 4 priority levels
        }
    }

    /// Drop the rendered pixels, keeping the shape of the buffers.
    ///
    /// Everything in here is the output of drawing a frame rather than state
    /// of the machine, and it is by far the largest thing in a savestate: over
    /// a megabyte of pixels against about half that for all of the console's
    /// actual memory. A restored state runs a frame straight away, which fills
    /// all of it again from VRAM, so writing it down is pure weight.
    ///
    /// The fields stay where they are rather than being skipped, because a
    /// length-prefixed empty vector still reads back as one: savestates
    /// written before this keep loading, pixels and all.
    pub fn forget_pixels(&mut self) {
        self.front_buffer = Vec::new();
        self.back_buffer = Vec::new();
        self.window_masks = Vec::new();
        self.line_buffers = Vec::new();
    }

    /// Put the buffers back to their proper size if they arrived empty.
    ///
    /// The counterpart to [`Framebuffer::forget_pixels`]: rendering indexes
    /// straight into these, so they have to be the right length before the
    /// next frame rather than after it.
    pub fn ensure_sized(&mut self) {
        if self.front_buffer.len() != PIXEL_COUNT {
            self.front_buffer = vec![Pixel::default(); PIXEL_COUNT];
        }
        if self.back_buffer.len() != PIXEL_COUNT {
            self.back_buffer = vec![Pixel::default(); PIXEL_COUNT];
        }
        if self.window_masks.len() != PIXEL_COUNT {
            self.window_masks = vec![WINDOW_MASK_ALL_LAYERS; PIXEL_COUNT];
        }
        if self.line_buffers.len() != 4 {
            self.line_buffers = vec![Vec::with_capacity(SCREEN_WIDTH); 4];
        }
    }

    /// Clear the back buffer
    pub fn clear(&mut self) {
        for pixel in &mut self.back_buffer {
            *pixel = Pixel::default();
        }
    }

    /// Clear a specific scanline
    pub fn clear_scanline(&mut self, y: usize) {
        let start = y * SCREEN_WIDTH;
        let end = start + SCREEN_WIDTH;
        for pixel in &mut self.back_buffer[start..end] {
            *pixel = Pixel::default();
        }
    }

    /// Fill a scanline with the backdrop pixel.
    pub fn fill_scanline(&mut self, y: usize, color: Color) {
        let start = y * SCREEN_WIDTH;
        let end = start + SCREEN_WIDTH;
        for (offset, pixel) in self.back_buffer[start..end].iter_mut().enumerate() {
            let mask = self.window_masks[start + offset];
            *pixel = Pixel {
                color,
                priority: 4,
                layer: 5,
                is_obj: false,
                semi_transparent: false,
                effects_enabled: (mask & (1 << 5)) != 0,
                under_color: Color::BLACK,
                under_layer: 5,
                under_is_obj: false,
                has_under_pixel: false,
            };
        }
    }

    /// Set the precomputed window mask for a scanline.
    pub fn set_scanline_window_masks(&mut self, y: usize, masks: &[u8; SCREEN_WIDTH]) {
        let start = y * SCREEN_WIDTH;
        let end = start + SCREEN_WIDTH;
        self.window_masks[start..end].copy_from_slice(masks);
    }

    /// Set a pixel in the back buffer
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
            return;
        }

        let index = y * SCREEN_WIDTH + x;
        self.back_buffer[index].color = color;
    }

    /// Set a pixel with priority for compositing
    pub fn set_pixel_with_priority(
        &mut self,
        x: usize,
        y: usize,
        color: Color,
        priority: u8,
        layer: u8,
        is_obj: bool,
    ) {
        self.set_pixel_with_attrs(x, y, color, priority, layer, is_obj, false);
    }

    /// Set a pixel with full compositing metadata.
    pub fn set_pixel_with_attrs(
        &mut self,
        x: usize,
        y: usize,
        color: Color,
        priority: u8,
        layer: u8,
        is_obj: bool,
        semi_transparent: bool,
    ) {
        if x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
            return;
        }

        let index = y * SCREEN_WIDTH + x;
        let layer_index = if is_obj { 4 } else { layer as usize };
        let mask = self.window_masks[index];
        if layer_index < 5 && (mask & (1 << layer_index)) == 0 {
            return;
        }
        let current = &self.back_buffer[index];

        // Priority comparison: lower priority value = higher priority
        // Objects are rendered after backgrounds, so they can overwrite
        let should_draw = if is_obj {
            // OBJ vs BG: compare priorities
            // OBJ wins if same priority (rendered later)
            priority <= current.priority || current.layer == 5
        } else {
            // BG vs BG: lower priority value wins
            priority < current.priority || current.layer == 5
        };

        if should_draw {
            let under_pixel = *current;
            self.back_buffer[index] = Pixel {
                color,
                priority,
                layer,
                is_obj,
                semi_transparent,
                effects_enabled: (mask & (1 << 5)) != 0,
                under_color: under_pixel.color,
                under_layer: under_pixel.layer,
                under_is_obj: under_pixel.is_obj,
                has_under_pixel: semi_transparent,
            };
        }
    }

    /// Get a pixel from the front buffer
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
            return None;
        }

        let index = y * SCREEN_WIDTH + x;
        Some(self.front_buffer[index].color)
    }

    /// Get all pixels from the front buffer as RGB555
    pub fn get_pixels_rgb555(&self) -> Vec<u16> {
        self.front_buffer
            .iter()
            .map(|p| p.color.to_rgb555())
            .collect()
    }

    /// Get all pixels from the front buffer as RGB888
    pub fn get_pixels_rgb888(&self) -> Vec<(u8, u8, u8)> {
        self.front_buffer
            .iter()
            .map(|p| p.color.to_rgb888())
            .collect()
    }

    /// Swap front and back buffers
    pub fn swap_buffers(&mut self) {
        std::mem::swap(&mut self.front_buffer, &mut self.back_buffer);
    }

    /// Get the framebuffer as a slice of pixels (front buffer)
    pub fn as_slice(&self) -> &[Pixel] {
        &self.front_buffer
    }

    /// Get the back buffer as a slice of pixels
    pub fn back_buffer_slice(&self) -> &[Pixel] {
        &self.back_buffer
    }

    /// Set a pixel color directly in the back buffer by index
    pub fn set_back_pixel_color(&mut self, index: usize, color: Color) {
        if index < self.back_buffer.len() {
            self.back_buffer[index].color = color;
        }
    }

    /// Replace a back-buffer pixel, including layer metadata.
    pub fn set_back_pixel(&mut self, index: usize, pixel: Pixel) {
        if index < self.back_buffer.len() {
            self.back_buffer[index] = pixel;
        }
    }

    /// Toggle whether special effects are allowed for a back-buffer pixel.
    pub fn set_back_pixel_effects_enabled(&mut self, index: usize, enabled: bool) {
        if index < self.back_buffer.len() {
            self.back_buffer[index].effects_enabled = enabled;
        }
    }

    /// Get raw pixel data as u32 (ABGR format for easy display)
    pub fn get_pixels_u32(&self) -> Vec<u32> {
        self.front_buffer
            .iter()
            .map(|p| {
                let (r, g, b) = p.color.to_rgb888();
                // ABGR format (little-endian RGBA)
                0xFF000000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
            })
            .collect()
    }
}

/// Renderer trait for different output targets
pub trait Renderer {
    /// Update the display with new framebuffer data
    fn update(&mut self, framebuffer: &Framebuffer);

    /// Clear the display
    fn clear(&mut self);

    /// Get the framebuffer dimensions
    fn dimensions(&self) -> (u32, u32);
}

/// Mosaic effect parameters
#[derive(Debug, Clone, Copy, Default)]
pub struct Mosaic {
    /// Background mosaic X size (1-16)
    pub bg_x: u8,
    /// Background mosaic Y size (1-16)
    pub bg_y: u8,
    /// Object mosaic X size (1-16)
    pub obj_x: u8,
    /// Object mosaic Y size (1-16)
    pub obj_y: u8,
}

impl Mosaic {
    /// Parse from MOSAIC register (0x0400000C)
    pub fn from_value(value: u16) -> Self {
        Self {
            bg_x: ((value & 0xF) + 1) as u8,
            bg_y: (((value >> 4) & 0xF) + 1) as u8,
            obj_x: (((value >> 8) & 0xF) + 1) as u8,
            obj_y: (((value >> 12) & 0xF) + 1) as u8,
        }
    }

    /// Apply mosaic effect to a scanline
    pub fn apply_bg(&self, framebuffer: &mut Framebuffer, y: usize) {
        if self.bg_x <= 1 && self.bg_y <= 1 {
            return; // No effect
        }

        let mosaic_y = (y / self.bg_y as usize) * self.bg_y as usize;

        for x in 0..SCREEN_WIDTH {
            let mosaic_x = (x / self.bg_x as usize) * self.bg_x as usize;
            let src_index = mosaic_y * SCREEN_WIDTH + mosaic_x;
            let dst_index = y * SCREEN_WIDTH + x;

            if src_index < framebuffer.back_buffer_slice().len() {
                let color = framebuffer.back_buffer_slice()[src_index].color;
                framebuffer.set_back_pixel_color(dst_index, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_rgb555() {
        let value: u16 = 0x03E0; // Green (0, 31, 0)
        let color = Color::from_rgb555(value);
        assert_eq!(color.r, 0);
        assert_eq!(color.g, 31);
        assert_eq!(color.b, 0);
        assert_eq!(color.to_rgb555(), value);
    }

    #[test]
    fn test_color_rgb888() {
        let color = Color::new(31, 31, 31);
        let (r, g, b) = color.to_rgb888();
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);
    }

    #[test]
    fn test_framebuffer_new() {
        let fb = Framebuffer::new();
        assert_eq!(fb.front_buffer.len(), PIXEL_COUNT);
        assert_eq!(fb.back_buffer.len(), PIXEL_COUNT);
    }

    #[test]
    fn test_framebuffer_set_pixel() {
        let mut fb = Framebuffer::new();
        let color = Color::new(31, 0, 0); // Red

        fb.set_pixel(10, 20, color);
        let retrieved = fb.get_pixel(10, 20).unwrap();

        // Note: set_pixel doesn't update until swap_buffers
        // This test would need set_pixel_with_priority for immediate effect
        assert_eq!(retrieved.to_rgb555(), 0); // Should be black before swap
    }

    #[test]
    fn test_framebuffer_swap() {
        let mut fb = Framebuffer::new();
        let color = Color::new(0, 31, 0); // Green

        fb.set_pixel_with_priority(5, 5, color, 0, 0, false);
        fb.swap_buffers();

        let retrieved = fb.get_pixel(5, 5).unwrap();
        assert_eq!(retrieved.to_rgb555(), color.to_rgb555());
    }

    #[test]
    fn test_window_mask_blocks_obj_and_preserves_bg_pixel() {
        let mut fb = Framebuffer::new();
        let mut masks = [0x3F; SCREEN_WIDTH];
        masks[0] = 0b0010_0001; // BG0 enabled, OBJ disabled, effects enabled.
        fb.set_scanline_window_masks(0, &masks);
        fb.fill_scanline(0, Color::BLACK);

        let bg = Color::new(0, 31, 0);
        let obj = Color::new(31, 0, 0);
        fb.set_pixel_with_attrs(0, 0, bg, 1, 0, false, false);
        fb.set_pixel_with_attrs(0, 0, obj, 0, 4, true, false);

        assert_eq!(fb.back_buffer_slice()[0].color, bg);
        assert!(!fb.back_buffer_slice()[0].is_obj);
    }

    #[test]
    fn test_mosaic() {
        let mosaic = Mosaic::from_value(0x0000);
        assert_eq!(mosaic.bg_x, 1);
        assert_eq!(mosaic.bg_y, 1);

        let mosaic = Mosaic::from_value(0x1234);
        assert_eq!(mosaic.bg_x, 5); // (0x4 + 1)
        assert_eq!(mosaic.bg_y, 4); // (0x3 + 1)
        assert_eq!(mosaic.obj_x, 3); // (0x2 + 1)
        assert_eq!(mosaic.obj_y, 2); // (0x1 + 1)
    }
}

//! Color Effects and Windows
//!
//! This module implements:
//! - Alpha blending between layers
//! - Brightness increase/decrease
//! - Window clipping (WIN0, WIN1, WINOBJ)

use super::{Color, Framebuffer, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

/// Blend mode for color effects
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlendMode {
    /// No effect
    #[default]
    None,
    /// Alpha blending (1st target * 1 - alpha + 2nd target * alpha)
    Alpha,
    /// Brightness increase
    BrightnessIncrease,
    /// Brightness decrease
    BrightnessDecrease,
}

/// Blend control register (BLDCNT)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BlendControl {
    /// First target layers (BG0-3, OBJ, Backdrop)
    pub first_target: [bool; 6],
    /// Blend mode
    pub blend_mode: BlendMode,
    /// Second target layers (BG0-3, OBJ, Backdrop)
    pub second_target: [bool; 6],
}

impl BlendControl {
    /// Parse from BLDCNT register (0x04000050)
    pub fn from_value(value: u16) -> Self {
        Self {
            first_target: [
                (value & (1 << 0)) != 0,
                (value & (1 << 1)) != 0,
                (value & (1 << 2)) != 0,
                (value & (1 << 3)) != 0,
                (value & (1 << 4)) != 0,
                (value & (1 << 5)) != 0,
            ],
            blend_mode: match (value >> 6) & 0x3 {
                0 => BlendMode::None,
                1 => BlendMode::Alpha,
                2 => BlendMode::BrightnessIncrease,
                3 => BlendMode::BrightnessDecrease,
                _ => unreachable!(),
            },
            second_target: [
                (value & (1 << 8)) != 0,
                (value & (1 << 9)) != 0,
                (value & (1 << 10)) != 0,
                (value & (1 << 11)) != 0,
                (value & (1 << 12)) != 0,
                (value & (1 << 13)) != 0,
            ],
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        let mut value = 0;

        for (i, &enabled) in self.first_target.iter().enumerate() {
            if enabled {
                value |= 1 << i;
            }
        }

        match self.blend_mode {
            BlendMode::None => {}
            BlendMode::Alpha => value |= 1 << 6,
            BlendMode::BrightnessIncrease => value |= 2 << 6,
            BlendMode::BrightnessDecrease => value |= 3 << 6,
        }

        for (i, &enabled) in self.second_target.iter().enumerate() {
            if enabled {
                value |= 1 << (8 + i);
            }
        }

        value
    }

    /// Check if a layer is in the first target
    pub fn is_first_target(&self, layer: usize) -> bool {
        layer < 6 && self.first_target[layer]
    }

    /// Check if a layer is in the second target
    pub fn is_second_target(&self, layer: usize) -> bool {
        layer < 6 && self.second_target[layer]
    }
}

/// Blend alpha register (BLDALPHA)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BlendAlpha {
    /// Alpha coefficient for first target (0-16)
    pub alpha_a: u8,
    /// Alpha coefficient for second target (0-16)
    pub alpha_b: u8,
}

impl BlendAlpha {
    /// Parse from BLDALPHA register (0x04000052)
    pub fn from_value(value: u16) -> Self {
        Self {
            alpha_a: (value & 0x1F).min(16) as u8,
            alpha_b: ((value >> 8) & 0x1F).min(16) as u8,
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        (self.alpha_a as u16 & 0x1F) | ((self.alpha_b as u16 & 0x1F) << 8)
    }

    /// Get alpha as float (0.0 - 1.0)
    pub fn get_alpha(&self) -> f32 {
        self.alpha_a as f32 / 16.0
    }

    /// Get beta as float (0.0 - 1.0)
    pub fn get_beta(&self) -> f32 {
        self.alpha_b as f32 / 16.0
    }
}

/// Blend brightness register (BLDY)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BlendY {
    /// Brightness factor (0-16)
    pub factor: u8,
}

impl BlendY {
    /// Parse from BLDY register (0x04000054)
    pub fn from_value(value: u16) -> Self {
        Self {
            factor: (value & 0x1F).min(16) as u8,
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        self.factor as u16 & 0x1F
    }
}

/// Window horizontal range
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WindowH {
    /// Left coordinate (0-240)
    pub left: u8,
    /// Right coordinate (0-240)
    pub right: u8,
}

impl WindowH {
    /// Parse from WIN0H/WIN1H register
    pub fn from_value(value: u16) -> Self {
        Self {
            left: ((value >> 8) & 0xFF) as u8,
            right: (value & 0xFF) as u8,
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        ((self.left as u16) << 8) | (self.right as u16)
    }

    /// Check if x coordinate is inside the window
    pub fn contains_x(&self, x: u16) -> bool {
        if self.left < self.right {
            x >= self.left as u16 && x < self.right as u16
        } else {
            // Wraps around
            x >= self.left as u16 || x < self.right as u16
        }
    }
}

/// Window vertical range
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WindowV {
    /// Top coordinate (0-160)
    pub top: u8,
    /// Bottom coordinate (0-160)
    pub bottom: u8,
}

impl WindowV {
    /// Parse from WIN0V/WIN1V register
    pub fn from_value(value: u16) -> Self {
        Self {
            top: ((value >> 8) & 0xFF) as u8,
            bottom: (value & 0xFF) as u8,
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        ((self.top as u16) << 8) | (self.bottom as u16)
    }

    /// Check if y coordinate is inside the window
    pub fn contains_y(&self, y: u16) -> bool {
        if self.top < self.bottom {
            y >= self.top as u16 && y < self.bottom as u16
        } else {
            // Wraps around
            y >= self.top as u16 || y < self.bottom as u16
        }
    }
}

/// Window layer control
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WindowControl {
    /// Enable flags for each layer (BG0-3, OBJ)
    pub layers: [bool; 5],
    /// Enable color special effects inside this window region
    pub special_effect: bool,
}

impl WindowControl {
    /// Parse from WININ/WINOUT register
    pub fn from_value(value: u16) -> Self {
        Self {
            layers: [
                (value & (1 << 0)) != 0,
                (value & (1 << 1)) != 0,
                (value & (1 << 2)) != 0,
                (value & (1 << 3)) != 0,
                (value & (1 << 4)) != 0,
            ],
            special_effect: (value & (1 << 5)) != 0,
        }
    }

    /// Convert to register value
    pub fn to_value(&self) -> u16 {
        let mut value = 0;
        for (i, &enabled) in self.layers.iter().enumerate() {
            if enabled {
                value |= 1 << i;
            }
        }
        if self.special_effect {
            value |= 1 << 5;
        }
        value
    }

    /// Check if a layer is enabled
    pub fn is_enabled(&self, layer: usize) -> bool {
        layer < 5 && self.layers[layer]
    }
}

/// Window definition
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Window {
    /// Horizontal range
    pub h_range: WindowH,
    /// Vertical range
    pub v_range: WindowV,
    /// Layer control
    pub control: WindowControl,
}

impl Window {
    /// Create a new window
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a point is inside the window
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.h_range.contains_x(x) && self.v_range.contains_y(y)
    }

    /// Check if a layer is enabled in this window
    pub fn is_layer_enabled(&self, layer: usize) -> bool {
        self.control.is_enabled(layer)
    }
}

/// Color effects controller
#[derive(Clone, Serialize, Deserialize)]
pub struct Effects {
    /// Blend control
    pub blend_control: BlendControl,
    /// Blend alpha coefficients
    pub blend_alpha: BlendAlpha,
    /// Blend brightness factor
    pub blend_y: BlendY,
    /// Special effects enable per scanline
    pub effects_enabled: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Self::new()
    }
}

impl Effects {
    /// Create a new effects controller
    pub fn new() -> Self {
        Self {
            blend_control: BlendControl::default(),
            blend_alpha: BlendAlpha::default(),
            blend_y: BlendY::default(),
            effects_enabled: false,
        }
    }

    /// Reset effects
    pub fn reset(&mut self) {
        self.blend_control = BlendControl::default();
        self.blend_alpha = BlendAlpha::default();
        self.blend_y = BlendY::default();
        self.effects_enabled = false;
    }

    /// Apply color effects to a scanline
    pub fn apply(&self, framebuffer: &mut Framebuffer, y: usize, backdrop: Color) {
        if self.blend_control.blend_mode == BlendMode::None {
            return;
        }

        for x in 0..SCREEN_WIDTH {
            let index = y * SCREEN_WIDTH + x;
            let pixel = framebuffer.back_buffer_slice()[index];

            // Determine if this pixel is a first target
            let layer = if pixel.is_obj {
                4
            } else {
                pixel.layer as usize
            };
            let is_first = self.blend_control.is_first_target(layer);

            if !is_first || !pixel.effects_enabled {
                continue;
            }

            match self.blend_control.blend_mode {
                BlendMode::Alpha => {
                    if pixel.semi_transparent && pixel.has_under_pixel {
                        let second_layer = if pixel.under_is_obj {
                            4
                        } else {
                            pixel.under_layer as usize
                        };
                        let second_color = if self.blend_control.is_second_target(second_layer) {
                            pixel.under_color
                        } else {
                            backdrop
                        };
                        let blended = self.alpha_blend(pixel.color, second_color);
                        framebuffer.set_back_pixel_color(index, blended);
                    }
                }
                BlendMode::BrightnessIncrease => {
                    let brightened = self.increase_brightness(pixel.color);
                    framebuffer.set_back_pixel_color(index, brightened);
                }
                BlendMode::BrightnessDecrease => {
                    let darkened = self.decrease_brightness(pixel.color);
                    framebuffer.set_back_pixel_color(index, darkened);
                }
                BlendMode::None => {}
            }
        }
    }

    /// Perform alpha blending between two colors.
    ///
    /// GBA formula: result = min(31, first * eva/16 + second * evb/16)
    pub fn alpha_blend(&self, first: Color, second: Color) -> Color {
        let eva = self.blend_alpha.alpha_a as u16;
        let evb = self.blend_alpha.alpha_b as u16;

        let blend = |a: u8, b: u8| {
            ((a as u16 * eva + b as u16 * evb) / 16).min(31) as u8
        };

        Color::new(
            blend(first.r, second.r),
            blend(first.g, second.g),
            blend(first.b, second.b),
        )
    }

    /// Increase brightness
    fn increase_brightness(&self, color: Color) -> Color {
        let factor = self.blend_y.factor as u16;

        let adjust = |c: u8| {
            let result = (c as u16) + factor * (31 - (c as u16)) / 16;
            result.min(31) as u8
        };

        Color::new(adjust(color.r), adjust(color.g), adjust(color.b))
    }

    /// Decrease brightness
    fn decrease_brightness(&self, color: Color) -> Color {
        let factor = self.blend_y.factor as u16;

        let adjust = |c: u8| {
            let result = (c as u16) - factor * (c as u16) / 16;
            result.min(31) as u8
        };

        Color::new(adjust(color.r), adjust(color.g), adjust(color.b))
    }

    /// Read BLDCNT register
    pub fn read_bldcnt(&self) -> u16 {
        self.blend_control.to_value()
    }

    /// Write BLDCNT register
    pub fn write_bldcnt(&mut self, value: u16) {
        self.blend_control = BlendControl::from_value(value);
    }

    /// Read BLDALPHA register
    pub fn read_bldalpha(&self) -> u16 {
        self.blend_alpha.to_value()
    }

    /// Write BLDALPHA register
    pub fn write_bldalpha(&mut self, value: u16) {
        self.blend_alpha = BlendAlpha::from_value(value);
    }

    /// Read BLDY register
    pub fn read_bldy(&self) -> u16 {
        self.blend_y.to_value()
    }

    /// Write BLDY register
    pub fn write_bldy(&mut self, value: u16) {
        self.blend_y = BlendY::from_value(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_control() {
        // Bit 0 = BG0 first target, bits 6-7 = 01 (Alpha), bit 9 = BG1 second target
        let value = 0x0241;
        let bc = BlendControl::from_value(value);
        assert_eq!(bc.blend_mode, BlendMode::Alpha);
        assert!(bc.is_first_target(0));
        assert!(bc.is_second_target(1));
    }

    #[test]
    fn test_blend_alpha() {
        let value = 0x0010; // Alpha = 16, Beta = 0
        let ba = BlendAlpha::from_value(value);
        assert_eq!(ba.alpha_a, 16);
        assert_eq!(ba.alpha_b, 0);
        assert_eq!(ba.get_alpha(), 1.0);
        assert_eq!(ba.get_beta(), 0.0);
    }

    #[test]
    fn test_blend_y() {
        let value = 0x0008; // Factor = 8
        let by = BlendY::from_value(value);
        assert_eq!(by.factor, 8);
    }

    #[test]
    fn test_window_h() {
        let value = 0x0050; // Left = 0, Right = 80
        let wh = WindowH::from_value(value);
        assert_eq!(wh.left, 0);
        assert_eq!(wh.right, 80);
        assert!(wh.contains_x(40));
        assert!(!wh.contains_x(100));
    }

    #[test]
    fn test_window_v() {
        let value = 0x00A0; // Top = 0, Bottom = 160
        let wv = WindowV::from_value(value);
        assert_eq!(wv.top, 0);
        assert_eq!(wv.bottom, 160);
        assert!(wv.contains_y(80));
        assert!(!wv.contains_y(160));
    }

    #[test]
    fn test_window_contains() {
        let mut window = Window::new();
        window.h_range = WindowH::from_value(0x00F0); // 0-240
        window.v_range = WindowV::from_value(0x00A0); // 0-160

        assert!(window.contains(120, 80));
        assert!(!window.contains(240, 80));
        assert!(!window.contains(120, 160));
    }

    #[test]
    fn test_window_control_special_effect_bit() {
        let control = WindowControl::from_value(0x003F);
        assert!(control.is_enabled(0));
        assert!(control.is_enabled(4));
        assert!(control.special_effect);
        assert_eq!(control.to_value() & 0x003F, 0x003F);
    }

    #[test]
    fn test_effects_alpha_blend() {
        let mut effects = Effects::new();
        effects.blend_alpha = BlendAlpha::from_value(0x0808); // Alpha = Beta = 0.5

        let color1 = Color::new(31, 0, 0); // Red
        let color2 = Color::new(0, 0, 31); // Blue

        let blended = effects.alpha_blend(color1, color2);

        // Should be approximately purple
        assert!(blended.r > 10);
        assert!(blended.b > 10);
    }

    #[test]
    fn test_effects_brightness() {
        let mut effects = Effects::new();

        let color = Color::new(16, 16, 16);

        effects.blend_y = BlendY::from_value(0x0008); // Factor = 8
        let brightened = effects.increase_brightness(color);
        assert!(brightened.r > color.r);

        let darkened = effects.decrease_brightness(color);
        assert!(darkened.r < color.r);
    }

    #[test]
    fn test_semi_transparent_obj_blends_with_under_pixel() {
        let mut effects = Effects::new();
        effects.blend_control = BlendControl::from_value(0x0150); // OBJ first target, alpha, BG0 second
        effects.blend_alpha = BlendAlpha::from_value(0x0808); // 50/50

        let mut framebuffer = Framebuffer::new();
        framebuffer.set_pixel_with_attrs(0, 0, Color::new(31, 0, 0), 0, 0, false, false);
        framebuffer.set_pixel_with_attrs(0, 0, Color::new(0, 0, 31), 0, 4, true, true);

        effects.apply(&mut framebuffer, 0, Color::BLACK);

        let blended = framebuffer.back_buffer_slice()[0].color;
        assert!(blended.r > 10);
        assert!(blended.b > 10);
        assert_eq!(blended.g, 0);
    }
}

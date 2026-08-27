//! `Canvas`: a clipped, translatable drawing surface over the softbuffer pixel
//! buffer.
//!
//! The original overlay threaded `(buf, buf_w, buf_h, x, y, w, h, color, ...)`
//! through every primitive by hand, which is why several draw functions grew
//! past ten positional parameters. A `Canvas` owns the buffer plus a clip
//! rectangle and an origin, so panels can be handed a sub-region and draw in
//! local coordinates without knowing where they sit on screen.
//!
//! Coordinates are `i32` on purpose: widgets routinely compute positions that
//! land off-screen, and clipping is cheaper to reason about than saturating
//! `usize` arithmetic at every call site.

use super::font::{self, CHAR_H, CHAR_W};

/// An axis-aligned rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Build a rect from two corners, in any order.
    pub fn from_corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let (left, right) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let (top, bottom) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        Self::new(left, top, right - left, bottom - top)
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub const fn center_x(&self) -> i32 {
        self.x + self.w / 2
    }

    pub const fn center_y(&self) -> i32 {
        self.y + self.h / 2
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Shrink by `amount` on every side. Clamps to empty rather than inverting.
    pub fn inset(&self, amount: i32) -> Self {
        self.inset_xy(amount, amount)
    }

    pub fn inset_xy(&self, dx: i32, dy: i32) -> Self {
        Self::new(
            self.x + dx,
            self.y + dy,
            (self.w - dx * 2).max(0),
            (self.h - dy * 2).max(0),
        )
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn intersect(&self, other: Rect) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }

    /// Split `amount` pixels off the top, returning `(top, remainder)`.
    pub fn split_top(&self, amount: i32) -> (Rect, Rect) {
        let amount = amount.clamp(0, self.h);
        (
            Rect::new(self.x, self.y, self.w, amount),
            Rect::new(self.x, self.y + amount, self.w, self.h - amount),
        )
    }

    /// Split `amount` pixels off the bottom, returning `(remainder, bottom)`.
    pub fn split_bottom(&self, amount: i32) -> (Rect, Rect) {
        let amount = amount.clamp(0, self.h);
        (
            Rect::new(self.x, self.y, self.w, self.h - amount),
            Rect::new(self.x, self.bottom() - amount, self.w, amount),
        )
    }

    /// Split `amount` pixels off the left, returning `(left, remainder)`.
    pub fn split_left(&self, amount: i32) -> (Rect, Rect) {
        let amount = amount.clamp(0, self.w);
        (
            Rect::new(self.x, self.y, amount, self.h),
            Rect::new(self.x + amount, self.y, self.w - amount, self.h),
        )
    }

    /// Split `amount` pixels off the right, returning `(remainder, right)`.
    pub fn split_right(&self, amount: i32) -> (Rect, Rect) {
        let amount = amount.clamp(0, self.w);
        (
            Rect::new(self.x, self.y, self.w - amount, self.h),
            Rect::new(self.right() - amount, self.y, amount, self.h),
        )
    }
}

/// Horizontal placement for [`Canvas::text_aligned`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Width in pixels of `text` rendered at `scale`, excluding the trailing gap.
pub fn text_width(text: &str, scale: i32) -> i32 {
    let chars = text.chars().count() as i32;
    if chars == 0 {
        0
    } else {
        chars * cell_width(scale) - scale
    }
}

/// Advance between glyph origins: the glyph itself plus a one-scaled-pixel gap.
pub const fn cell_width(scale: i32) -> i32 {
    CHAR_W as i32 * scale + scale
}

/// Height of a line of text at `scale`.
pub const fn line_height(scale: i32) -> i32 {
    CHAR_H as i32 * scale
}

/// Linear interpolation between two packed 0xRRGGBB colors.
pub fn lerp_color(from: u32, to: u32, num: i32, den: i32) -> u32 {
    let den = den.max(1);
    let num = num.clamp(0, den);
    let channel = |shift: u32| {
        let a = ((from >> shift) & 0xFF) as i32;
        let b = ((to >> shift) & 0xFF) as i32;
        (((a * (den - num) + b * num) / den) as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Blend `src` over `dst` at `alpha` (0..=255).
pub fn blend_color(dst: u32, src: u32, alpha: u32) -> u32 {
    let alpha = alpha.min(255);
    if alpha == 0 {
        return dst;
    }
    if alpha == 255 {
        return src & 0x00FF_FFFF;
    }
    let inv = 255 - alpha;
    let channel = |shift: u32| {
        let a = (dst >> shift) & 0xFF;
        let b = (src >> shift) & 0xFF;
        ((a * inv + b * alpha) / 255) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Scale every channel of `color` by `numerator / denominator`.
pub fn scale_color(color: u32, numerator: u32, denominator: u32) -> u32 {
    let denominator = denominator.max(1);
    let channel = |shift: u32| {
        let value = (color >> shift) & 0xFF;
        ((value * numerator / denominator).min(255)) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// A clipped view onto the frame buffer.
///
/// Drawing coordinates are relative to the canvas origin; anything outside the
/// clip rectangle is discarded. Create sub-canvases with [`Canvas::sub`] to give
/// a panel its own coordinate space.
pub struct Canvas<'a> {
    buf: &'a mut [u32],
    stride: usize,
    /// Clip region in absolute buffer coordinates.
    clip: Rect,
    /// Absolute buffer position of this canvas's (0, 0).
    origin_x: i32,
    origin_y: i32,
    /// Size of this canvas's local coordinate space.
    width: i32,
    height: i32,
}

impl<'a> Canvas<'a> {
    /// Wrap a full frame buffer. `buf.len()` must be at least `width * height`.
    pub fn new(buf: &'a mut [u32], width: usize, height: usize) -> Self {
        debug_assert!(buf.len() >= width * height);
        Self {
            buf,
            stride: width,
            clip: Rect::new(0, 0, width as i32, height as i32),
            origin_x: 0,
            origin_y: 0,
            width: width as i32,
            height: height as i32,
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The full local-coordinate area of this canvas, ignoring clipping.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// A sub-canvas covering `rect`, with its own origin and an intersected
    /// clip. Children draw at (0, 0) without knowing where they sit on screen.
    pub fn sub(&mut self, rect: Rect) -> Canvas<'_> {
        let absolute = rect.translate(self.origin_x, self.origin_y);
        Canvas {
            clip: self.clip.intersect(absolute),
            origin_x: absolute.x,
            origin_y: absolute.y,
            width: rect.w,
            height: rect.h,
            stride: self.stride,
            buf: self.buf,
        }
    }

    /// Same coordinate space, but clipped to `rect`. Use when a widget must not
    /// paint outside its box yet still wants shared coordinates with its parent.
    pub fn clipped(&mut self, rect: Rect) -> Canvas<'_> {
        let absolute = rect.translate(self.origin_x, self.origin_y);
        Canvas {
            clip: self.clip.intersect(absolute),
            origin_x: self.origin_x,
            origin_y: self.origin_y,
            width: self.width,
            height: self.height,
            stride: self.stride,
            buf: self.buf,
        }
    }

    /// Raw access to the backing buffer for blitters that write whole rows.
    /// Returns the buffer and its stride; callers must clip themselves.
    pub fn raw_buffer(&mut self) -> (&mut [u32], usize) {
        (self.buf, self.stride)
    }

    #[inline]
    fn put(&mut self, abs_x: i32, abs_y: i32, color: u32) {
        if !self.clip.contains(abs_x, abs_y) {
            return;
        }
        let index = abs_y as usize * self.stride + abs_x as usize;
        if let Some(slot) = self.buf.get_mut(index) {
            *slot = color;
        }
    }

    #[inline]
    fn get(&self, abs_x: i32, abs_y: i32) -> u32 {
        if !self.clip.contains(abs_x, abs_y) {
            return 0;
        }
        let index = abs_y as usize * self.stride + abs_x as usize;
        self.buf.get(index).copied().unwrap_or(0)
    }

    /// Set a single pixel in local coordinates.
    pub fn pixel(&mut self, x: i32, y: i32, color: u32) {
        self.put(self.origin_x + x, self.origin_y + y, color);
    }

    /// Fill the entire canvas.
    pub fn fill(&mut self, color: u32) {
        let bounds = self.bounds();
        self.fill_rect(bounds, color);
    }

    /// Fill `rect` with a solid color.
    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        let area = rect
            .translate(self.origin_x, self.origin_y)
            .intersect(self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let start = y as usize * self.stride + area.x as usize;
            let end = start + area.w as usize;
            if let Some(row) = self.buf.get_mut(start..end) {
                row.fill(color);
            }
        }
    }

    /// Alpha-blend a solid color over `rect`. `alpha` is 0..=255.
    pub fn blend_rect(&mut self, rect: Rect, color: u32, alpha: u32) {
        if alpha == 0 {
            return;
        }
        if alpha >= 255 {
            self.fill_rect(rect, color);
            return;
        }
        let area = rect
            .translate(self.origin_x, self.origin_y)
            .intersect(self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let start = y as usize * self.stride + area.x as usize;
            let end = start + area.w as usize;
            if let Some(row) = self.buf.get_mut(start..end) {
                for slot in row {
                    *slot = blend_color(*slot, color, alpha);
                }
            }
        }
    }

    /// Multiply every pixel in `rect` toward black. `amount` is 0..=255, where
    /// 255 leaves the pixel untouched.
    pub fn dim_rect(&mut self, rect: Rect, amount: u32) {
        let area = rect
            .translate(self.origin_x, self.origin_y)
            .intersect(self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let start = y as usize * self.stride + area.x as usize;
            let end = start + area.w as usize;
            if let Some(row) = self.buf.get_mut(start..end) {
                for slot in row {
                    *slot = scale_color(*slot, amount, 255);
                }
            }
        }
    }

    /// Draw a 1px-thick (or `thickness`-thick) border just inside `rect`.
    pub fn stroke_rect(&mut self, rect: Rect, thickness: i32, color: u32) {
        let thickness = thickness.max(1);
        if rect.is_empty() {
            return;
        }
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, thickness), color);
        self.fill_rect(
            Rect::new(rect.x, rect.bottom() - thickness, rect.w, thickness),
            color,
        );
        self.fill_rect(Rect::new(rect.x, rect.y, thickness, rect.h), color);
        self.fill_rect(
            Rect::new(rect.right() - thickness, rect.y, thickness, rect.h),
            color,
        );
    }

    /// Horizontal rule.
    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, w, 1), color);
    }

    /// Vertical rule.
    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: u32) {
        self.fill_rect(Rect::new(x, y, 1, h), color);
    }

    /// Vertical gradient across `rect`.
    pub fn gradient_rect(&mut self, rect: Rect, top: u32, bottom: u32) {
        if rect.is_empty() {
            return;
        }
        for row in 0..rect.h {
            let color = lerp_color(top, bottom, row, rect.h.max(1) - 1);
            self.fill_rect(Rect::new(rect.x, rect.y + row, rect.w, 1), color);
        }
    }

    /// Draw one glyph. `bg` of `None` leaves untouched pixels alone.
    pub fn glyph(&mut self, x: i32, y: i32, ch: char, scale: i32, fg: u32, bg: Option<u32>) {
        let Some(bitmap) = font::glyph(ch) else {
            return;
        };
        let scale = scale.max(1);
        for row in 0..CHAR_H as i32 {
            let bits = bitmap[row as usize];
            for col in 0..CHAR_W as i32 {
                let lit = (bits >> (7 - col)) & 1 != 0;
                let color = match (lit, bg) {
                    (true, _) => fg,
                    (false, Some(bg)) => bg,
                    (false, None) => continue,
                };
                let px = x + col * scale;
                let py = y + row * scale;
                self.fill_rect(Rect::new(px, py, scale, scale), color);
            }
        }
    }

    /// Draw `text` at (x, y) with a transparent background.
    /// Returns the x coordinate just past the last glyph cell.
    pub fn text(&mut self, x: i32, y: i32, text: &str, scale: i32, fg: u32) -> i32 {
        self.text_bg(x, y, text, scale, fg, None)
    }

    /// Draw `text` at (x, y), optionally filling the glyph cell background.
    pub fn text_bg(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        scale: i32,
        fg: u32,
        bg: Option<u32>,
    ) -> i32 {
        let scale = scale.max(1);
        let advance = cell_width(scale);
        let mut cx = x;
        for ch in text.chars() {
            self.glyph(cx, y, ch, scale, fg, bg);
            cx += advance;
        }
        cx
    }

    /// Draw `text` horizontally aligned within `rect`, vertically centered.
    pub fn text_in(&mut self, rect: Rect, align: Align, text: &str, scale: i32, fg: u32) {
        let scale = scale.max(1);
        let width = text_width(text, scale);
        let x = match align {
            Align::Left => rect.x,
            Align::Center => rect.x + (rect.w - width) / 2,
            Align::Right => rect.right() - width,
        };
        let y = rect.y + (rect.h - line_height(scale)) / 2;
        self.text(x, y, text, scale, fg);
    }

    /// Draw `text` centered horizontally on `center_x`.
    pub fn text_centered(&mut self, center_x: i32, y: i32, text: &str, scale: i32, fg: u32) {
        let scale = scale.max(1);
        let x = center_x - text_width(text, scale) / 2;
        self.text(x, y, text, scale, fg);
    }

    /// Draw `text` ending at `right_x`.
    pub fn text_right(&mut self, right_x: i32, y: i32, text: &str, scale: i32, fg: u32) {
        let scale = scale.max(1);
        let x = right_x - text_width(text, scale);
        self.text(x, y, text, scale, fg);
    }

    /// Alpha-blend a single pixel, reading the existing value underneath.
    pub fn blend_pixel(&mut self, x: i32, y: i32, color: u32, alpha: u32) {
        let abs_x = self.origin_x + x;
        let abs_y = self.origin_y + y;
        if !self.clip.contains(abs_x, abs_y) {
            return;
        }
        let existing = self.get(abs_x, abs_y);
        self.put(abs_x, abs_y, blend_color(existing, color, alpha));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas_buffer(w: usize, h: usize) -> Vec<u32> {
        vec![0u32; w * h]
    }

    #[test]
    fn fill_rect_clips_to_canvas_bounds() {
        let mut buf = canvas_buffer(4, 4);
        {
            let mut canvas = Canvas::new(&mut buf, 4, 4);
            canvas.fill_rect(Rect::new(-2, -2, 4, 4), 0x00FF_0000);
        }
        // Only the bottom-right 2x2 of the requested rect lands on screen.
        assert_eq!(buf[0], 0x00FF_0000);
        assert_eq!(buf[1], 0x00FF_0000);
        assert_eq!(buf[2], 0);
        assert_eq!(buf[4 * 2], 0);
    }

    #[test]
    fn sub_canvas_translates_and_clips() {
        let mut buf = canvas_buffer(8, 8);
        {
            let mut canvas = Canvas::new(&mut buf, 8, 8);
            let mut panel = canvas.sub(Rect::new(4, 4, 2, 2));
            // Overflows the sub-canvas; must not paint outside it.
            panel.fill_rect(Rect::new(0, 0, 100, 100), 0x0000_FF00);
        }
        assert_eq!(buf[4 * 8 + 4], 0x0000_FF00);
        assert_eq!(buf[5 * 8 + 5], 0x0000_FF00);
        assert_eq!(buf[4 * 8 + 6], 0, "must not bleed right of the sub-canvas");
        assert_eq!(buf[6 * 8 + 4], 0, "must not bleed below the sub-canvas");
    }

    #[test]
    fn rect_split_helpers_partition_without_overlap() {
        let rect = Rect::new(10, 20, 100, 50);
        let (top, rest) = rect.split_top(12);
        assert_eq!(top, Rect::new(10, 20, 100, 12));
        assert_eq!(rest, Rect::new(10, 32, 100, 38));

        let (left, right) = rect.split_left(30);
        assert_eq!(left.right(), right.x);
        assert_eq!(left.w + right.w, rect.w);
    }

    #[test]
    fn split_clamps_oversized_amounts() {
        let rect = Rect::new(0, 0, 10, 10);
        let (top, rest) = rect.split_top(999);
        assert_eq!(top.h, 10);
        assert_eq!(rest.h, 0);
    }

    #[test]
    fn text_width_excludes_the_trailing_gap() {
        assert_eq!(text_width("", 1), 0);
        assert_eq!(text_width("A", 1), CHAR_W as i32);
        assert_eq!(text_width("AB", 1), CHAR_W as i32 * 2 + 1);
        assert_eq!(text_width("AB", 2), (CHAR_W as i32 * 2 + 1) * 2);
    }

    #[test]
    fn transparent_text_leaves_background_pixels_alone() {
        let mut buf = canvas_buffer(16, 16);
        buf.fill(0x0012_3456);
        {
            let mut canvas = Canvas::new(&mut buf, 16, 16);
            canvas.text(0, 0, " ", 1, 0x00FF_FFFF);
        }
        // A space has no lit pixels, so a transparent draw is a no-op.
        assert!(buf.iter().all(|&px| px == 0x0012_3456));
    }

    #[test]
    fn blend_color_endpoints_are_exact() {
        assert_eq!(blend_color(0x0000_0000, 0x00FF_FFFF, 0), 0x0000_0000);
        assert_eq!(blend_color(0x0000_0000, 0x00FF_FFFF, 255), 0x00FF_FFFF);
    }

    #[test]
    fn contains_is_half_open_on_the_far_edges() {
        let rect = Rect::new(0, 0, 4, 4);
        assert!(rect.contains(0, 0));
        assert!(rect.contains(3, 3));
        assert!(!rect.contains(4, 3));
        assert!(!rect.contains(3, 4));
    }
}

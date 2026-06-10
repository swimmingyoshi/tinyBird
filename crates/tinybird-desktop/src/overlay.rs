//! On-screen overlay renderer using an embedded 8×8 pixel-art font.
//!
//! Draws directly into the ARGB u32 pixel buffer produced by softbuffer.

use crate::game_addons::{
    AddonData, FireRedAreaSnapshot, FireRedBattleSnapshot, FireRedEncounterEntry,
    FireRedEncounterGroup, FireRedMoveSlot, FireRedPartyMember, FireRedSnapshot, FireRedStatSpread,
    StreamSnapshot,
};
use crate::pokemon_assets::{PokemonSpriteStore, SpriteBitmap};

/// Character width and height (pixels per glyph cell).
pub const CHAR_W: usize = 8;
pub const CHAR_H: usize = 8;

#[derive(Clone, Copy)]
pub enum ToastTone {
    Info,
    Success,
    Warning,
}

pub struct HomeScreen<'a> {
    pub bios_loaded: bool,
    pub hovered_file: Option<&'a str>,
}

pub struct PauseScreen<'a> {
    pub rom_title: Option<&'a str>,
}

pub struct Toast<'a> {
    pub text: &'a str,
    pub tone: ToastTone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddonViewMode {
    Team,
    Encounters,
}

impl AddonViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Team => "Team",
            Self::Encounters => "Encounters",
        }
    }
}

/// 8×8 bitmap font, ASCII 0x20–0x7E (95 printable characters).
/// Each entry is 8 bytes: row 0 (top) … row 7 (bottom).
/// Within each byte, bit 7 = leftmost column, bit 0 = rightmost.
#[rustfmt::skip]
const FONT: [[u8; 8]; 95] = [
    // 0x20 ' '
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 0x21 '!'
    [0x18,0x18,0x18,0x18,0x18,0x00,0x18,0x00],
    // 0x22 '"'
    [0x36,0x36,0x00,0x00,0x00,0x00,0x00,0x00],
    // 0x23 '#'
    [0x36,0x36,0x7f,0x36,0x7f,0x36,0x36,0x00],
    // 0x24 '$'
    [0x0c,0x3e,0x60,0x3c,0x06,0x7c,0x18,0x00],
    // 0x25 '%'
    [0x62,0x66,0x0c,0x18,0x30,0x66,0x46,0x00],
    // 0x26 '&'
    [0x3c,0x66,0x38,0x38,0x67,0x66,0x3f,0x00],
    // 0x27 '\''
    [0x0c,0x0c,0x18,0x00,0x00,0x00,0x00,0x00],
    // 0x28 '('
    [0x0c,0x18,0x30,0x30,0x30,0x18,0x0c,0x00],
    // 0x29 ')'
    [0x30,0x18,0x0c,0x0c,0x0c,0x18,0x30,0x00],
    // 0x2A '*'
    [0x00,0x66,0x3c,0xff,0x3c,0x66,0x00,0x00],
    // 0x2B '+'
    [0x00,0x18,0x18,0x7e,0x18,0x18,0x00,0x00],
    // 0x2C ','
    [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30],
    // 0x2D '-'
    [0x00,0x00,0x00,0x7e,0x00,0x00,0x00,0x00],
    // 0x2E '.'
    [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00],
    // 0x2F '/'
    [0x00,0x06,0x0c,0x18,0x30,0x60,0x00,0x00],
    // 0x30 '0'
    [0x3c,0x66,0x6e,0x76,0x66,0x66,0x3c,0x00],
    // 0x31 '1'
    [0x18,0x38,0x18,0x18,0x18,0x18,0x7e,0x00],
    // 0x32 '2'
    [0x3c,0x66,0x06,0x1c,0x30,0x60,0x7e,0x00],
    // 0x33 '3'
    [0x3c,0x66,0x06,0x1c,0x06,0x66,0x3c,0x00],
    // 0x34 '4'
    [0x06,0x0e,0x1e,0x66,0x7f,0x06,0x06,0x00],
    // 0x35 '5'
    [0x7e,0x60,0x7c,0x06,0x06,0x66,0x3c,0x00],
    // 0x36 '6'
    [0x3c,0x66,0x60,0x7c,0x66,0x66,0x3c,0x00],
    // 0x37 '7'
    [0x7e,0x06,0x0c,0x18,0x30,0x30,0x30,0x00],
    // 0x38 '8'
    [0x3c,0x66,0x66,0x3c,0x66,0x66,0x3c,0x00],
    // 0x39 '9'
    [0x3c,0x66,0x66,0x3e,0x06,0x66,0x3c,0x00],
    // 0x3A ':'
    [0x00,0x18,0x18,0x00,0x18,0x18,0x00,0x00],
    // 0x3B ';'
    [0x00,0x18,0x18,0x00,0x18,0x18,0x30,0x00],
    // 0x3C '<'
    [0x0c,0x18,0x30,0x60,0x30,0x18,0x0c,0x00],
    // 0x3D '='
    [0x00,0x00,0x7e,0x00,0x7e,0x00,0x00,0x00],
    // 0x3E '>'
    [0x30,0x18,0x0c,0x06,0x0c,0x18,0x30,0x00],
    // 0x3F '?'
    [0x3c,0x66,0x06,0x1c,0x18,0x00,0x18,0x00],
    // 0x40 '@'
    [0x3e,0x63,0x6f,0x69,0x6f,0x60,0x3e,0x00],
    // 0x41 'A'
    [0x18,0x3c,0x66,0x7e,0x66,0x66,0x66,0x00],
    // 0x42 'B'
    [0x7c,0x66,0x66,0x7c,0x66,0x66,0x7c,0x00],
    // 0x43 'C'
    [0x3c,0x66,0x60,0x60,0x60,0x66,0x3c,0x00],
    // 0x44 'D'
    [0x78,0x6c,0x66,0x66,0x66,0x6c,0x78,0x00],
    // 0x45 'E'
    [0x7e,0x60,0x60,0x7c,0x60,0x60,0x7e,0x00],
    // 0x46 'F'
    [0x7e,0x60,0x60,0x7c,0x60,0x60,0x60,0x00],
    // 0x47 'G'
    [0x3c,0x66,0x60,0x6e,0x66,0x66,0x3c,0x00],
    // 0x48 'H'
    [0x66,0x66,0x66,0x7e,0x66,0x66,0x66,0x00],
    // 0x49 'I'
    [0x3c,0x18,0x18,0x18,0x18,0x18,0x3c,0x00],
    // 0x4A 'J'
    [0x1e,0x06,0x06,0x06,0x06,0x66,0x3c,0x00],
    // 0x4B 'K'
    [0x66,0x6c,0x78,0x70,0x78,0x6c,0x66,0x00],
    // 0x4C 'L'
    [0x60,0x60,0x60,0x60,0x60,0x60,0x7e,0x00],
    // 0x4D 'M'
    [0x63,0x77,0x7f,0x6b,0x63,0x63,0x63,0x00],
    // 0x4E 'N'
    [0x66,0x76,0x7e,0x7e,0x6e,0x66,0x66,0x00],
    // 0x4F 'O'
    [0x3c,0x66,0x66,0x66,0x66,0x66,0x3c,0x00],
    // 0x50 'P'
    [0x7c,0x66,0x66,0x7c,0x60,0x60,0x60,0x00],
    // 0x51 'Q'
    [0x3c,0x66,0x66,0x66,0x6e,0x3c,0x06,0x00],
    // 0x52 'R'
    [0x7c,0x66,0x66,0x7c,0x6c,0x66,0x66,0x00],
    // 0x53 'S'
    [0x3c,0x66,0x60,0x3c,0x06,0x66,0x3c,0x00],
    // 0x54 'T'
    [0x7e,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
    // 0x55 'U'
    [0x66,0x66,0x66,0x66,0x66,0x66,0x3c,0x00],
    // 0x56 'V'
    [0x66,0x66,0x66,0x66,0x66,0x3c,0x18,0x00],
    // 0x57 'W'
    [0x63,0x63,0x63,0x6b,0x7f,0x77,0x63,0x00],
    // 0x58 'X'
    [0x66,0x66,0x3c,0x18,0x3c,0x66,0x66,0x00],
    // 0x59 'Y'
    [0x66,0x66,0x66,0x3c,0x18,0x18,0x18,0x00],
    // 0x5A 'Z'
    [0x7e,0x06,0x0c,0x18,0x30,0x60,0x7e,0x00],
    // 0x5B '['
    [0x3c,0x30,0x30,0x30,0x30,0x30,0x3c,0x00],
    // 0x5C '\\'
    [0x00,0x60,0x30,0x18,0x0c,0x06,0x00,0x00],
    // 0x5D ']'
    [0x3c,0x0c,0x0c,0x0c,0x0c,0x0c,0x3c,0x00],
    // 0x5E '^'
    [0x18,0x3c,0x66,0x00,0x00,0x00,0x00,0x00],
    // 0x5F '_'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x7e,0x00],
    // 0x60 '`'
    [0x18,0x18,0x0c,0x00,0x00,0x00,0x00,0x00],
    // 0x61 'a'
    [0x00,0x00,0x3c,0x06,0x3e,0x66,0x3e,0x00],
    // 0x62 'b'
    [0x60,0x60,0x7c,0x66,0x66,0x66,0x7c,0x00],
    // 0x63 'c'
    [0x00,0x00,0x3c,0x66,0x60,0x66,0x3c,0x00],
    // 0x64 'd'
    [0x06,0x06,0x3e,0x66,0x66,0x66,0x3e,0x00],
    // 0x65 'e'
    [0x00,0x00,0x3c,0x66,0x7e,0x60,0x3c,0x00],
    // 0x66 'f'
    [0x1c,0x30,0x30,0x7c,0x30,0x30,0x30,0x00],
    // 0x67 'g'
    [0x00,0x00,0x3e,0x66,0x3e,0x06,0x06,0x3c],
    // 0x68 'h'
    [0x60,0x60,0x7c,0x66,0x66,0x66,0x66,0x00],
    // 0x69 'i'
    [0x18,0x00,0x18,0x18,0x18,0x18,0x18,0x00],
    // 0x6A 'j'
    [0x06,0x00,0x06,0x06,0x06,0x06,0x66,0x3c],
    // 0x6B 'k'
    [0x60,0x60,0x66,0x6c,0x78,0x6c,0x66,0x00],
    // 0x6C 'l'
    [0x18,0x18,0x18,0x18,0x18,0x18,0x0e,0x00],
    // 0x6D 'm'
    [0x00,0x00,0x63,0x77,0x7f,0x6b,0x63,0x00],
    // 0x6E 'n'
    [0x00,0x00,0x7c,0x66,0x66,0x66,0x66,0x00],
    // 0x6F 'o'
    [0x00,0x00,0x3c,0x66,0x66,0x66,0x3c,0x00],
    // 0x70 'p'
    [0x00,0x00,0x7c,0x66,0x66,0x7c,0x60,0x60],
    // 0x71 'q'
    [0x00,0x00,0x3e,0x66,0x66,0x3e,0x06,0x06],
    // 0x72 'r'
    [0x00,0x00,0x6c,0x76,0x60,0x60,0x60,0x00],
    // 0x73 's'
    [0x00,0x00,0x3e,0x60,0x3c,0x06,0x7c,0x00],
    // 0x74 't'
    [0x30,0x30,0x7e,0x30,0x30,0x30,0x1c,0x00],
    // 0x75 'u'
    [0x00,0x00,0x66,0x66,0x66,0x66,0x3e,0x00],
    // 0x76 'v'
    [0x00,0x00,0x66,0x66,0x66,0x3c,0x18,0x00],
    // 0x77 'w'
    [0x00,0x00,0x63,0x6b,0x7f,0x77,0x63,0x00],
    // 0x78 'x'
    [0x00,0x00,0x66,0x3c,0x18,0x3c,0x66,0x00],
    // 0x79 'y'
    [0x00,0x00,0x66,0x66,0x3e,0x06,0x66,0x3c],
    // 0x7A 'z'
    [0x00,0x00,0x7e,0x0c,0x18,0x30,0x7e,0x00],
    // 0x7B '{'
    [0x0e,0x18,0x18,0x70,0x18,0x18,0x0e,0x00],
    // 0x7C '|'
    [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
    // 0x7D '}'
    [0x70,0x18,0x18,0x0e,0x18,0x18,0x70,0x00],
    // 0x7E '~'
    [0x76,0xdc,0x00,0x00,0x00,0x00,0x00,0x00],
];

/// Fill a rectangle with a solid color.
pub fn fill_rect(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    for row in 0..h {
        let py = y + row;
        if py >= buf_h {
            break;
        }
        for col in 0..w {
            let px = x + col;
            if px >= buf_w {
                break;
            }
            buf[py * buf_w + px] = color;
        }
    }
}

/// Draw a single character at pixel position (x, y), scaled by `scale`.
fn draw_char_scaled(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    ch: char,
    scale: usize,
    fg: u32,
    bg: u32,
) {
    let code = ch as usize;
    if code < 0x20 || code > 0x7E {
        return;
    }
    let glyph = &FONT[code - 0x20];
    for row in 0..CHAR_H {
        let byte = glyph[row];
        for col in 0..CHAR_W {
            let on = (byte >> (7 - col)) & 1 != 0;
            let color = if on { fg } else { bg };
            for sy in 0..scale {
                let py = y + row * scale + sy;
                if py >= buf_h {
                    continue;
                }
                for sx in 0..scale {
                    let px = x + col * scale + sx;
                    if px >= buf_w {
                        continue;
                    }
                    buf[py * buf_w + px] = color;
                }
            }
        }
    }
}

/// Draw a string starting at (x, y), returning the x after the last glyph.
/// `scale` is a pixel multiplier (1 = 8×8, 2 = 16×16, …).
pub fn draw_text(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
    fg: u32,
    bg: u32,
) -> usize {
    let cell = CHAR_W * scale + scale; // glyph width + 1-scaled-pixel gap
    let mut cx = x;
    for ch in text.chars() {
        draw_char_scaled(buf, buf_w, buf_h, cx, y, ch, scale, fg, bg);
        cx += cell;
    }
    cx
}

fn text_width(text: &str, scale: usize) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        chars * (CHAR_W * scale + scale) - scale
    }
}

fn draw_text_centered(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    center_x: usize,
    y: usize,
    text: &str,
    scale: usize,
    fg: u32,
    bg: u32,
) {
    let width = text_width(text, scale);
    let x = center_x.saturating_sub(width / 2);
    draw_text(buf, buf_w, buf_h, x, y, text, scale, fg, bg);
}

fn lerp_color(top: u32, bottom: u32, num: usize, den: usize) -> u32 {
    let den = den.max(1) as u32;
    let num = num.min(den as usize) as u32;
    let lerp = |shift: u32| {
        let a = (top >> shift) & 0xFF;
        let b = (bottom >> shift) & 0xFF;
        ((a * (den - num) + b * num) / den) << shift
    };
    lerp(16) | lerp(8) | lerp(0)
}

fn draw_gradient_background(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    top: u32,
    bottom: u32,
    accent: u32,
) {
    for y in 0..buf_h {
        let row_color = lerp_color(top, bottom, y, buf_h.saturating_sub(1));
        let row_start = y * buf_w;
        buf[row_start..row_start + buf_w].fill(row_color);

        if y % 28 == 0 {
            let stripe = lerp_color(accent, row_color, 1, 3);
            buf[row_start..row_start + buf_w].fill(stripe);
        }
    }

    let glow_x = buf_w / 6;
    let glow_w = (buf_w / 10).max(6);
    for y in 0..buf_h {
        let glow_color = lerp_color(accent, top, y, buf_h.saturating_mul(2));
        for x in glow_x..(glow_x + glow_w).min(buf_w) {
            if (x + y / 6) % 7 == 0 {
                buf[y * buf_w + x] = glow_color;
            }
        }
    }
}

fn draw_detached_addon_background(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    top: u32,
    bottom: u32,
    accent: u32,
) {
    for y in 0..buf_h {
        let row_color = lerp_color(top, bottom, y, buf_h.saturating_sub(1));
        let row_start = y * buf_w;
        buf[row_start..row_start + buf_w].fill(row_color);
    }

    let top_band_h = (buf_h / 10).max(12).min(40);
    for y in 0..top_band_h {
        let band = lerp_color(accent, top, y, top_band_h.saturating_mul(3));
        let row_start = y * buf_w;
        for x in 0..buf_w {
            if x % 3 == 0 {
                buf[row_start + x] = band;
            }
        }
    }
}

fn draw_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    bg: u32,
    border: u32,
    accent: u32,
) {
    fill_rect(buf, buf_w, buf_h, x, y, w, h, bg);
    fill_rect(buf, buf_w, buf_h, x, y, w, 2, accent);
    fill_rect(buf, buf_w, buf_h, x, y + h.saturating_sub(2), w, 2, border);
    fill_rect(buf, buf_w, buf_h, x, y, 2, h, border);
    fill_rect(buf, buf_w, buf_h, x + w.saturating_sub(2), y, 2, h, border);
}

pub fn dim_screen(buf: &mut [u32]) {
    for pixel in buf.iter_mut() {
        let r = ((*pixel >> 16) & 0xFF) * 3 / 8;
        let g = ((*pixel >> 8) & 0xFF) * 3 / 8;
        let b = (*pixel & 0xFF) * 3 / 8;
        *pixel = (r << 16) | (g << 8) | b;
    }
}

/// Draw the full settings overlay in the top-left corner.
pub fn draw_overlay(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    fps: f64,
    speed: u32,
    muted: bool,
    volume_pct: u32,
    color_correction: bool,
    fast_forward: bool,
) {
    const SCALE: usize = 2;
    const CELL: usize = CHAR_W * SCALE + SCALE; // 18 px per char
    const LINE: usize = CHAR_H * SCALE + SCALE; // 18 px per line

    // Panel dimensions
    let cols = 30usize; // characters wide
    let rows = 12usize; // lines tall
    let pad = 6usize;
    let panel_w = cols * CELL + pad * 2;
    let panel_h = rows * LINE + pad * 2;
    let px0 = 8usize;
    let py0 = 8usize;

    // Background + border
    fill_rect(buf, buf_w, buf_h, px0, py0, panel_w, panel_h, 0xFF_10_10_10);
    // Top / bottom border
    fill_rect(buf, buf_w, buf_h, px0, py0, panel_w, 2, 0xFF_00_AA_FF);
    fill_rect(
        buf,
        buf_w,
        buf_h,
        px0,
        py0 + panel_h - 2,
        panel_w,
        2,
        0xFF_00_AA_FF,
    );
    // Left / right border
    fill_rect(buf, buf_w, buf_h, px0, py0, 2, panel_h, 0xFF_00_AA_FF);
    fill_rect(
        buf,
        buf_w,
        buf_h,
        px0 + panel_w - 2,
        py0,
        2,
        panel_h,
        0xFF_00_AA_FF,
    );

    let tx = px0 + pad;
    let mut ty = py0 + pad;

    let white = 0xFF_FF_FF_FF;
    let grey = 0xFF_88_88_88;
    let green = 0xFF_00_EE_66;
    let red = 0xFF_EE_44_44;
    let cyan = 0xFF_00_CC_FF;
    let bg = 0xFF_10_10_10;

    // Title
    draw_text(buf, buf_w, buf_h, tx, ty, "tinyBird", SCALE, cyan, bg);
    ty += LINE + SCALE;

    // Separator
    fill_rect(buf, buf_w, buf_h, tx, ty, panel_w - pad * 2, 1, grey);
    ty += SCALE * 2;

    // FPS line
    let fps_str = format!("FPS: {:.1}", fps);
    draw_text(buf, buf_w, buf_h, tx, ty, &fps_str, SCALE, white, bg);
    ty += LINE;

    // Speed line
    let speed_str = format!("Speed: {}x  [1] [2] [4]", speed);
    draw_text(buf, buf_w, buf_h, tx, ty, &speed_str, SCALE, white, bg);
    ty += LINE;

    // Audio line
    let (audio_label, audio_color) = if muted {
        ("Audio: MUTE  [M]", red)
    } else if fast_forward {
        ("Audio: FAST MUTE", grey)
    } else {
        ("Audio: ON    [M]", green)
    };
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        audio_label,
        SCALE,
        audio_color,
        bg,
    );
    ty += LINE;

    // Volume line
    let vol_str = format!("Vol:   {}%  [-/+]", volume_pct);
    draw_text(buf, buf_w, buf_h, tx, ty, &vol_str, SCALE, white, bg);
    ty += LINE;

    // Color correction line
    let (cc_label, cc_color) = if color_correction {
        ("LCD color fix: ON  [C]", green)
    } else {
        ("LCD color fix: OFF [C]", grey)
    };
    draw_text(buf, buf_w, buf_h, tx, ty, cc_label, SCALE, cc_color, bg);
    ty += LINE + SCALE;

    // Separator
    fill_rect(buf, buf_w, buf_h, tx, ty, panel_w - pad * 2, 1, grey);
    ty += SCALE * 2;

    // Key bindings
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "[Esc] Pause/Resume",
        SCALE,
        grey,
        bg,
    );
    ty += LINE;
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "[R] Reset  [O] Open ROM",
        SCALE,
        grey,
        bg,
    );
    ty += LINE;
    draw_text(buf, buf_w, buf_h, tx, ty, "[F1] Close HUD", SCALE, grey, bg);
    ty += LINE;
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "[F2] Team  [F4] Encounters",
        SCALE,
        grey,
        bg,
    );
    ty += LINE;
    draw_text(buf, buf_w, buf_h, tx, ty, "[F6] Popout", SCALE, grey, bg);
}

pub fn draw_addon_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    snapshot: &StreamSnapshot,
    sprites: &PokemonSpriteStore,
    detached: bool,
    view_mode: AddonViewMode,
) {
    let panel_bg = 0xFF_12_17_26;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;

    if detached {
        draw_detached_addon_background(
            buf,
            buf_w,
            buf_h,
            0xFF_0A_0F_19,
            0xFF_141B_2A,
            0xFF_1E_2A_3F,
        );
    }

    let panel_w = if detached {
        buf_w.saturating_sub(24).clamp(280, 480)
    } else if view_mode == AddonViewMode::Encounters {
        buf_w.saturating_sub(16).clamp(260, 360)
    } else {
        buf_w.saturating_sub(20).clamp(220, 296)
    };
    let panel_h = if detached {
        buf_h.saturating_sub(24).clamp(180, 520)
    } else if view_mode == AddonViewMode::Encounters {
        buf_h.saturating_sub(12).clamp(220, 520)
    } else {
        buf_h.saturating_sub(16).clamp(220, 520)
    };
    let panel_x = if detached {
        (buf_w.saturating_sub(panel_w)) / 2
    } else {
        buf_w.saturating_sub(panel_w + 8)
    };
    let panel_y = if detached {
        (buf_h.saturating_sub(panel_h)) / 2
    } else {
        8
    };

    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
    );

    let center_x = panel_x + panel_w / 2;
    let mut y = panel_y + 14;
    let title_scale = if detached || panel_w > 260 { 2 } else { 1 };
    let title = snapshot
        .addon
        .as_ref()
        .map(|addon| match view_mode {
            AddonViewMode::Team => addon.display_name.to_string(),
            AddonViewMode::Encounters => addon.display_name.replace("Party", "Encounters"),
        })
        .unwrap_or_else(|| "Game Addons".to_string());
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        &title,
        title_scale,
        accent,
        panel_bg,
    );
    y += title_scale * 10 + 4;

    let subtitle = match &snapshot.rom {
        Some(rom) => format!("{}  {}", rom.title, rom.game_code),
        None => "Load a supported game to enable addons".to_string(),
    };
    draw_text_centered(
        buf, buf_w, buf_h, center_x, y, &subtitle, 1, muted, panel_bg,
    );
    y += 16;

    fill_rect(
        buf,
        buf_w,
        buf_h,
        panel_x + 12,
        y,
        panel_w.saturating_sub(24),
        1,
        muted,
    );
    y += 12;

    let footer_y = panel_y + panel_h.saturating_sub(18);
    match snapshot.addon.as_ref().map(|addon| &addon.data) {
        Some(AddonData::FireRed(data)) => match view_mode {
            AddonViewMode::Team => {
                draw_firered_party_panel(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x + 12,
                    y,
                    panel_w.saturating_sub(24),
                    footer_y.saturating_sub(8),
                    data,
                    sprites,
                );
            }
            AddonViewMode::Encounters => {
                draw_firered_encounter_panel(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x + 12,
                    y,
                    panel_w.saturating_sub(24),
                    footer_y.saturating_sub(8),
                    data,
                    sprites,
                );
            }
        },
        None => {
            draw_text_centered(
                buf,
                buf_w,
                buf_h,
                center_x,
                y + 8,
                "No addon data yet",
                1,
                ink,
                panel_bg,
            );
            draw_text_centered(
                buf,
                buf_w,
                buf_h,
                center_x,
                y + 24,
                "Load FireRed or LeafGreen and move in-game to populate the live team view.",
                1,
                muted,
                panel_bg,
            );
        }
    }

    let footer = if detached {
        match view_mode {
            AddonViewMode::Team => "[F4] Open Encounters  [F6] Close",
            AddonViewMode::Encounters => "[F2] Open Team  [F6] Close",
        }
    } else if view_mode == AddonViewMode::Team {
        "[F2] Hide  [F4] Encounters  [F6] Popout"
    } else {
        "[F4] Hide  [F2] Team  [F6] Popout"
    };
    draw_text_centered(
        buf, buf_w, buf_h, center_x, footer_y, footer, 1, muted, panel_bg,
    );
}

fn draw_firered_party_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    panel_x: usize,
    panel_y: usize,
    panel_w: usize,
    panel_bottom: usize,
    snapshot: &FireRedSnapshot,
    sprites: &PokemonSpriteStore,
) {
    let panel_bg = 0xFF_12_17_26;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;

    let summary = format!(
        "{} live slots  base ${:08X}",
        snapshot.party.len(),
        snapshot.party_base_address
    );
    draw_text(
        buf, buf_w, buf_h, panel_x, panel_y, &summary, 1, muted, panel_bg,
    );

    let card_gap = 6usize;
    let card_h = 70usize;
    let mut y = panel_y + 18;
    for member in &snapshot.party {
        if y + card_h > panel_bottom {
            break;
        }
        draw_firered_party_card(
            buf, buf_w, buf_h, panel_x, y, panel_w, card_h, member, sprites,
        );
        y += card_h + card_gap;
    }

    if snapshot.party.is_empty() {
        draw_text(
            buf,
            buf_w,
            buf_h,
            panel_x,
            panel_y + 22,
            "Your party is currently empty.",
            1,
            ink,
            panel_bg,
        );
    }
}

fn draw_firered_encounter_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    panel_x: usize,
    panel_y: usize,
    panel_w: usize,
    panel_bottom: usize,
    snapshot: &FireRedSnapshot,
    sprites: &PokemonSpriteStore,
) {
    let panel_bg = 0xFF_12_17_26;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;

    if let Some(battle) = &snapshot.battle {
        draw_firered_battle_panel(
            buf,
            buf_w,
            buf_h,
            panel_x,
            panel_y,
            panel_w,
            panel_bottom,
            battle,
            sprites,
        );
        return;
    }

    let Some(area) = &snapshot.area else {
        draw_text(
            buf,
            buf_w,
            buf_h,
            panel_x,
            panel_y,
            "Area data pending",
            1,
            accent,
            panel_bg,
        );
        return;
    };

    let y = draw_firered_area_panel(
        buf,
        buf_w,
        buf_h,
        panel_x,
        panel_y,
        panel_w,
        panel_bottom,
        area,
    );
    if area.encounter_groups.is_empty() && y + 12 <= panel_bottom {
        draw_text(
            buf,
            buf_w,
            buf_h,
            panel_x,
            y,
            "Try a route, cave, water, or grass area.",
            1,
            muted,
            panel_bg,
        );
    }
}

fn draw_firered_battle_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    panel_x: usize,
    panel_y: usize,
    panel_w: usize,
    panel_bottom: usize,
    battle: &FireRedBattleSnapshot,
    sprites: &PokemonSpriteStore,
) -> usize {
    let card_h = panel_bottom.saturating_sub(panel_y).min(180);
    if card_h < 158 || panel_y + card_h > panel_bottom {
        return panel_y;
    }

    let card_bg = 0xFF_1A_23_36;
    let border = 0xFF_FF_9F_1C;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let danger = 0xFF_FC_76_6A;
    let sprite_bg = 0xFF_0D_12_1F;

    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, card_h, card_bg, border, accent_2,
    );

    let title = format!("Now Fighting  {}", battle.battle_kind);
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 7,
        &short_label(&title, if panel_w >= 280 { 31 } else { 23 }),
        1,
        accent,
        card_bg,
    );

    let opponent = &battle.opponent;
    let sprite_box = if panel_w >= 300 { 64 } else { 54 };
    let sprite_x = panel_x + panel_w.saturating_sub(sprite_box + 8);
    let sprite_y = panel_y + 62;
    let text_w = sprite_x.saturating_sub(panel_x + 18);
    let level = format!("Lv{}", opponent.level);
    let level_x = panel_x + panel_w.saturating_sub(8 + text_width(&level, 1));
    let name_max = if panel_w >= 280 { 18 } else { 12 };
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 22,
        &short_label(&opponent.species_name, name_max),
        1,
        ink,
        card_bg,
    );

    draw_species_sprite_box(
        buf,
        buf_w,
        buf_h,
        sprite_x,
        sprite_y,
        sprite_box,
        opponent.species_id,
        "Foe",
        &format!("#{:03}", opponent.species_id),
        sprites,
        sprite_bg,
        accent_2,
        accent,
        muted,
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        level_x,
        panel_y + 22,
        &level,
        1,
        ink,
        card_bg,
    );

    let hp = format!("{}/{} HP", opponent.current_hp, opponent.max_hp);
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 35,
        &short_label(&hp, if panel_w >= 280 { 15 } else { 11 }),
        1,
        muted,
        card_bg,
    );

    let catch = match (battle.catchable, opponent.catch_rate) {
        (true, Some(rate)) => format!("Catch {}", rate),
        (true, None) => "Catch ?".to_string(),
        (false, _) => "Not catchable".to_string(),
    };
    let catch_color = if battle.catchable { accent_2 } else { danger };
    let catch_x = panel_x + panel_w.saturating_sub(8 + text_width(&catch, 1));
    draw_text(
        buf,
        buf_w,
        buf_h,
        catch_x,
        panel_y + 35,
        &catch,
        1,
        catch_color,
        card_bg,
    );

    let profile = format!(
        "{}  {}",
        opponent.nature,
        short_label(&opponent.ability_name, 14)
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 50,
        &short_label(&profile, if panel_w >= 280 { 28 } else { 21 }),
        1,
        muted,
        card_bg,
    );

    let item = opponent
        .held_item
        .as_ref()
        .map(|item| item.name.as_str())
        .unwrap_or("No item");
    let item_x = panel_x + panel_w.saturating_sub(8 + text_width(item, 1));
    draw_text(
        buf,
        buf_w,
        buf_h,
        item_x,
        panel_y + 50,
        &short_label(item, if panel_w >= 280 { 14 } else { 10 }),
        1,
        muted,
        card_bg,
    );

    draw_stat_column(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 66,
        card_bg,
        ink,
        muted,
        &[
            ("HP", opponent.stats.hp),
            ("Atk", opponent.stats.attack),
            ("Def", opponent.stats.defense),
        ],
    );
    draw_stat_column(
        buf,
        buf_w,
        buf_h,
        panel_x + 8 + (text_w / 2).max(78),
        panel_y + 66,
        card_bg,
        ink,
        muted,
        &[
            ("Spd", opponent.stats.speed),
            ("SpA", opponent.stats.sp_attack),
            ("SpD", opponent.stats.sp_def),
        ],
    );

    let ivs = format_spread_line("IV", &opponent.ivs, opponent.iv_total);
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 110,
        &short_label(&ivs, if panel_w >= 280 { 28 } else { 20 }),
        1,
        accent_2,
        card_bg,
    );

    let moves = format_move_summary(&opponent.move_slots, if panel_w >= 280 { 2 } else { 1 });
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 124,
        &short_label(&moves, if panel_w >= 280 { 36 } else { 25 }),
        1,
        muted,
        card_bg,
    );

    let bar_y = panel_y + card_h.saturating_sub(10);
    draw_hp_bar(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        bar_y,
        panel_w.saturating_sub(16),
        5,
        opponent.current_hp,
        opponent.max_hp.max(1),
    );

    panel_y + card_h
}

#[allow(clippy::too_many_arguments)]
fn draw_species_sprite_box(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    size: usize,
    species_id: u16,
    fallback_top: &str,
    fallback_bottom: &str,
    sprites: &PokemonSpriteStore,
    bg: u32,
    border: u32,
    accent: u32,
    muted: u32,
) {
    draw_panel(buf, buf_w, buf_h, x, y, size, size, bg, border, accent);

    if let Some(sprite) = sprites.sprite(species_id) {
        draw_sprite_bitmap(
            buf,
            buf_w,
            buf_h,
            x + 2,
            y + 2,
            size.saturating_sub(4),
            size.saturating_sub(4),
            sprite,
        );
        return;
    }

    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        x + size / 2,
        y + 8,
        fallback_top,
        1,
        accent,
        bg,
    );
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        x + size / 2,
        y + size.saturating_sub(16),
        fallback_bottom,
        1,
        muted,
        bg,
    );
}

fn draw_stat_column(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    bg: u32,
    value_color: u32,
    label_color: u32,
    stats: &[(&str, u16)],
) {
    for (idx, (label, value)) in stats.iter().enumerate() {
        let row_y = y + idx * 13;
        draw_text(buf, buf_w, buf_h, x, row_y, label, 1, label_color, bg);
        draw_text(
            buf,
            buf_w,
            buf_h,
            x + 30,
            row_y,
            &value.to_string(),
            1,
            value_color,
            bg,
        );
    }
}

fn draw_firered_area_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    panel_x: usize,
    panel_y: usize,
    panel_w: usize,
    panel_bottom: usize,
    area: &FireRedAreaSnapshot,
) -> usize {
    if panel_y + 36 > panel_bottom {
        return panel_y;
    }

    let panel_bg = 0xFF_12_17_26;
    let section_bg = 0xFF_18_20_33;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;

    fill_rect(buf, buf_w, buf_h, panel_x, panel_y, panel_w, 1, accent_2);
    let mut y = panel_y + 8;
    let area_title = short_label(&area.name, if panel_w >= 280 { 26 } else { 20 });
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x,
        y,
        &area_title,
        1,
        accent,
        panel_bg,
    );

    let map_id = format!("{}:{}", area.map_group, area.map_num);
    let map_x = panel_x + panel_w.saturating_sub(text_width(&map_id, 1));
    draw_text(buf, buf_w, buf_h, map_x, y, &map_id, 1, muted, panel_bg);
    y += 14;

    if area.encounter_groups.is_empty() {
        if y + 10 <= panel_bottom {
            draw_text(
                buf,
                buf_w,
                buf_h,
                panel_x,
                y,
                "No wild encounters listed here yet.",
                1,
                muted,
                panel_bg,
            );
        }
        return y + 14;
    }

    let mut groups_drawn = 0usize;
    for group in &area.encounter_groups {
        if y + 28 > panel_bottom {
            break;
        }
        let remaining_groups = area.encounter_groups.len().saturating_sub(groups_drawn + 1);
        let reserve_for_later_groups = remaining_groups * 34;
        let group_bottom = panel_bottom.saturating_sub(reserve_for_later_groups).max(y);
        y = draw_encounter_group(
            buf,
            buf_w,
            buf_h,
            panel_x,
            y,
            panel_w,
            group_bottom,
            group,
            section_bg,
            ink,
            muted,
            accent,
            accent_2,
        );
        groups_drawn += 1;
        y += 6;
    }

    if groups_drawn == 0 && y + 10 <= panel_bottom {
        draw_text(
            buf,
            buf_w,
            buf_h,
            panel_x,
            y,
            "Resize popout to show encounters.",
            1,
            muted,
            panel_bg,
        );
    }

    y
}

#[allow(clippy::too_many_arguments)]
fn draw_encounter_group(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    bottom: usize,
    group: &FireRedEncounterGroup,
    bg: u32,
    ink: u32,
    muted: u32,
    accent: u32,
    border: u32,
) -> usize {
    let row_h = 12usize;
    let available_rows = (bottom.saturating_sub(y + 20)) / row_h;
    let row_count = group.entries.len().min(available_rows);
    if row_count == 0 {
        return y;
    }
    let needs_more = group.entries.len() > row_count;
    let card_h = 20 + row_h * row_count + usize::from(needs_more) * 10;
    if y + card_h > bottom {
        return y;
    }

    draw_panel(buf, buf_w, buf_h, x, y, w, card_h, bg, border, accent);
    let title = format!("{}  area {}", group.method, group.encounter_rate);
    draw_text(
        buf,
        buf_w,
        buf_h,
        x + 8,
        y + 6,
        &short_label(&title, if w >= 280 { 30 } else { 22 }),
        1,
        accent,
        bg,
    );

    let mut row_y = y + 20;
    for entry in group.entries.iter().take(row_count) {
        draw_encounter_row(
            buf,
            buf_w,
            buf_h,
            x + 8,
            row_y,
            w.saturating_sub(16),
            entry,
            bg,
            ink,
            muted,
        );
        row_y += row_h;
    }
    if needs_more && row_y + 10 <= y + card_h {
        let remaining = format!("+{} more", group.entries.len() - row_count);
        draw_text(buf, buf_w, buf_h, x + 8, row_y, &remaining, 1, muted, bg);
    }

    y + card_h
}

#[allow(clippy::too_many_arguments)]
fn draw_encounter_row(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    entry: &FireRedEncounterEntry,
    bg: u32,
    ink: u32,
    muted: u32,
) {
    let name_w = if w >= 280 { 112 } else { 82 };
    let name = short_label(entry.species_name, if w >= 280 { 14 } else { 10 });
    draw_text(buf, buf_w, buf_h, x, y, &name, 1, ink, bg);

    let level = if entry.min_level == entry.max_level {
        format!("L{}", entry.min_level)
    } else {
        format!("L{}-{}", entry.min_level, entry.max_level)
    };
    draw_text(buf, buf_w, buf_h, x + name_w, y, &level, 1, muted, bg);

    let slot = format!("{}%", entry.slot_rate);
    let slot_x = x + w.saturating_sub(84);
    draw_text(buf, buf_w, buf_h, slot_x, y, &slot, 1, 0xFF_2E_C4_B6, bg);

    let catch = format!("C{}", entry.catch_rate);
    let catch_x = x + w.saturating_sub(text_width(&catch, 1));
    draw_text(buf, buf_w, buf_h, catch_x, y, &catch, 1, muted, bg);
}

fn format_spread_line(label: &str, spread: &FireRedStatSpread, total: u16) -> String {
    format!(
        "{} {}/{}/{}/{}/{}/{} T{}",
        label,
        spread.hp,
        spread.attack,
        spread.defense,
        spread.speed,
        spread.sp_attack,
        spread.sp_def,
        total
    )
}

fn format_move_summary(move_slots: &[FireRedMoveSlot], max_moves: usize) -> String {
    let mut parts = move_slots
        .iter()
        .take(max_moves)
        .map(|slot| format!("{} {}", short_label(&slot.name, 10), slot.pp))
        .collect::<Vec<_>>();
    if move_slots.len() > max_moves {
        parts.push(format!("+{}", move_slots.len() - max_moves));
    }
    if parts.is_empty() {
        "Moves pending".to_string()
    } else {
        parts.join("  ")
    }
}

fn draw_firered_party_card(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    member: &FireRedPartyMember,
    sprites: &PokemonSpriteStore,
) {
    let card_bg = 0xFF_1A_23_36;
    let card_border = 0xFF_2E_C4_B6;
    let accent = 0xFF_FF_9F_1C;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let sprite_bg = 0xFF_0D_12_1F;

    draw_panel(buf, buf_w, buf_h, x, y, w, h, card_bg, card_border, accent);

    let sprite_box = h.saturating_sub(10).clamp(26, 38);
    let sprite_x = x + 8;
    let sprite_y = y + (h.saturating_sub(sprite_box)) / 2;
    draw_species_sprite_box(
        buf,
        buf_w,
        buf_h,
        sprite_x,
        sprite_y,
        sprite_box,
        member.species_id,
        &format!("S{}", member.slot),
        &format!("#{:03}", member.species_id),
        sprites,
        sprite_bg,
        card_border,
        accent,
        muted,
    );

    let info_x = sprite_x + sprite_box + 10;
    let right_pad = 10usize;
    let level = if member.is_egg {
        "EGG".to_string()
    } else {
        format!("Lv{}", member.level)
    };
    let level_x = x + w.saturating_sub(right_pad + text_width(&level, 1));
    let nickname = short_label(&member.nickname, if w >= 280 { 14 } else { 10 });
    draw_text(buf, buf_w, buf_h, info_x, y + 6, &nickname, 1, ink, card_bg);
    draw_text(
        buf,
        buf_w,
        buf_h,
        level_x,
        y + 6,
        &level,
        1,
        accent,
        card_bg,
    );

    let hp_text = format!("{}/{}", member.current_hp, member.max_hp);
    draw_text(
        buf,
        buf_w,
        buf_h,
        info_x,
        y + h.saturating_sub(18),
        &hp_text,
        1,
        muted,
        card_bg,
    );

    let meta = format!(
        "S{} #{:03} {}",
        member.slot,
        member.species_id,
        short_label(&member.ability_name, 9)
    );
    draw_text(buf, buf_w, buf_h, info_x, y + 18, &meta, 1, muted, card_bg);

    let ivs = format_spread_line("IV", &member.ivs, member.iv_total);
    draw_text(
        buf,
        buf_w,
        buf_h,
        info_x,
        y + 31,
        &short_label(&ivs, if w >= 280 { 30 } else { 22 }),
        1,
        0xFF_2E_C4_B6,
        card_bg,
    );

    let evs = format_spread_line("EV", &member.evs, member.ev_total);
    draw_text(
        buf,
        buf_w,
        buf_h,
        info_x,
        y + 44,
        &short_label(&evs, if w >= 280 { 30 } else { 22 }),
        1,
        accent,
        card_bg,
    );

    let bar_x = info_x;
    let bar_y = y + h.saturating_sub(8);
    let bar_w = level_x.saturating_sub(info_x + 8);
    draw_hp_bar(
        buf,
        buf_w,
        buf_h,
        bar_x,
        bar_y,
        bar_w.max(32),
        6,
        member.current_hp,
        member.max_hp.max(1),
    );
}

fn draw_hp_bar(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    current_hp: u16,
    max_hp: u16,
) {
    let border = 0xFF_35_3F_58;
    let empty = 0xFF_0B_0F_18;
    let hp_color = hp_bar_color(current_hp, max_hp);

    fill_rect(buf, buf_w, buf_h, x, y, w, h, border);
    fill_rect(
        buf,
        buf_w,
        buf_h,
        x + 1,
        y + 1,
        w.saturating_sub(2),
        h.saturating_sub(2),
        empty,
    );

    let inner_w = w.saturating_sub(2);
    let fill_w = if max_hp == 0 {
        0
    } else {
        inner_w * current_hp as usize / max_hp as usize
    };
    if fill_w > 0 {
        fill_rect(
            buf,
            buf_w,
            buf_h,
            x + 1,
            y + 1,
            fill_w,
            h.saturating_sub(2),
            hp_color,
        );
    }
}

fn hp_bar_color(current_hp: u16, max_hp: u16) -> u32 {
    if max_hp == 0 {
        return 0xFF_80_87_99;
    }
    let ratio = current_hp as f32 / max_hp as f32;
    if ratio <= 0.2 {
        0xFF_E7_43_4B
    } else if ratio <= 0.5 {
        0xFF_FF_9F_1C
    } else {
        0xFF_3D_DC_97
    }
}

fn draw_sprite_bitmap(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    sprite: &SpriteBitmap,
) {
    if w == 0 || h == 0 || sprite.width == 0 || sprite.height == 0 {
        return;
    }

    let sprite_w = sprite.width;
    let sprite_h = sprite.height;
    let (draw_w, draw_h) = if sprite_w * h > sprite_h * w {
        let draw_h = (w * sprite_h / sprite_w).max(1);
        (w, draw_h)
    } else {
        let draw_w = (h * sprite_w / sprite_h).max(1);
        (draw_w, h)
    };
    let offset_x = x + (w.saturating_sub(draw_w)) / 2;
    let offset_y = y + (h.saturating_sub(draw_h)) / 2;

    for dy in 0..draw_h {
        let src_y = dy * sprite_h / draw_h;
        let dst_y = offset_y + dy;
        if dst_y >= buf_h {
            continue;
        }

        for dx in 0..draw_w {
            let src_x = dx * sprite_w / draw_w;
            let dst_x = offset_x + dx;
            if dst_x >= buf_w {
                continue;
            }

            let src = sprite.pixels[src_y * sprite_w + src_x];
            let alpha = (src >> 24) & 0xFF;
            if alpha == 0 {
                continue;
            }

            let idx = dst_y * buf_w + dst_x;
            buf[idx] = if alpha == 0xFF {
                src & 0x00FF_FFFF
            } else {
                alpha_blend(buf[idx], src)
            };
        }
    }
}

fn alpha_blend(dst: u32, src: u32) -> u32 {
    let src_a = (src >> 24) & 0xFF;
    let inv_a = 255 - src_a;
    let blend = |shift: u32| {
        let src_c = (src >> shift) & 0xFF;
        let dst_c = (dst >> shift) & 0xFF;
        ((src_c * src_a + dst_c * inv_a) / 255) << shift
    };
    blend(16) | blend(8) | blend(0)
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

pub fn draw_home_screen(buf: &mut [u32], buf_w: usize, buf_h: usize, screen: HomeScreen<'_>) {
    let compact = buf_h < 190 || buf_w < 300;
    let ink = 0xFF_F6_EE_DF;
    let muted = 0xFF_8C_92_A0;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;
    let panel_bg = 0xFF_11_16_27;

    draw_gradient_background(buf, buf_w, buf_h, 0xFF_0A_10_1E, 0xFF_1C_22_37, accent_2);

    let panel_w = buf_w.saturating_sub(24).clamp(180, 420);
    let panel_h = buf_h.saturating_sub(24).clamp(128, 220);
    let panel_x = (buf_w.saturating_sub(panel_w)) / 2;
    let panel_y = (buf_h.saturating_sub(panel_h)) / 2;
    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
    );

    let center_x = panel_x + panel_w / 2;
    let mut y = panel_y + 14;
    let title_scale = if compact {
        2
    } else if buf_w >= 420 {
        3
    } else {
        2
    };
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "tinyBird",
        title_scale,
        accent,
        panel_bg,
    );
    y += title_scale * 10 + 8;
    if !compact {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            y,
            "Game Boy Advance emulator",
            1,
            ink,
            panel_bg,
        );
        y += 18;
    }

    fill_rect(
        buf,
        buf_w,
        buf_h,
        panel_x + 14,
        y,
        panel_w.saturating_sub(28),
        1,
        muted,
    );
    y += 12;

    let hero = if screen.hovered_file.is_some() {
        "Drop now to load"
    } else {
        "Open ROM: [O] [Enter]"
    };
    draw_text_centered(buf, buf_w, buf_h, center_x, y, hero, 1, ink, panel_bg);
    y += 14;
    if !compact {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            y,
            "Drag and drop also works",
            1,
            accent_2,
            panel_bg,
        );
        y += 18;
    } else {
        y += 8;
    }

    if let Some(file) = screen.hovered_file {
        draw_text_centered(buf, buf_w, buf_h, center_x, y, file, 1, accent, panel_bg);
        y += 16;
    }

    let bios_text = if screen.bios_loaded {
        "BIOS: detected"
    } else {
        "BIOS: HLE mode active"
    };
    let bios_color = if screen.bios_loaded { accent_2 } else { accent };
    draw_text_centered(
        buf, buf_w, buf_h, center_x, y, bios_text, 1, bios_color, panel_bg,
    );
    y += 18;

    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Move: arrows  A/B: Z/X",
        1,
        ink,
        panel_bg,
    );
    y += 14;
    if compact {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            y,
            "Start: Enter",
            1,
            ink,
            panel_bg,
        );
    } else {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            y,
            "L/R: A/S  Start: Enter",
            1,
            ink,
            panel_bg,
        );
        y += 14;
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            y,
            "Place one ROM in ./roms/",
            1,
            muted,
            panel_bg,
        );
    }
}

pub fn draw_pause_screen(buf: &mut [u32], buf_w: usize, buf_h: usize, screen: PauseScreen<'_>) {
    let panel_w = buf_w.saturating_sub(36).clamp(160, 300);
    let panel_h = buf_h.saturating_sub(40).clamp(124, 168);
    let panel_x = (buf_w.saturating_sub(panel_w)) / 2;
    let panel_y = (buf_h.saturating_sub(panel_h)) / 2;
    let panel_bg = 0xFF_16_12_1C;
    let ink = 0xFF_F7_F1_E8;
    let muted = 0xFF_A4_9B_AE;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_72_E0_D1;

    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
    );

    let center_x = panel_x + panel_w / 2;
    let mut y = panel_y + 16;
    draw_text_centered(
        buf, buf_w, buf_h, center_x, y, "PAUSED", 2, accent, panel_bg,
    );
    y += 28;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        screen.rom_title.unwrap_or("No ROM"),
        1,
        ink,
        panel_bg,
    );
    y += 18;

    fill_rect(
        buf,
        buf_w,
        buf_h,
        panel_x + 12,
        y,
        panel_w.saturating_sub(24),
        1,
        muted,
    );
    y += 12;

    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Resume: [Esc]",
        1,
        ink,
        panel_bg,
    );
    y += 14;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Open ROM: [O]",
        1,
        ink,
        panel_bg,
    );
    y += 14;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Save: [F5]  Load: [F8]",
        1,
        ink,
        panel_bg,
    );
    y += 14;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Speed: [1] [2] [4]",
        1,
        muted,
        panel_bg,
    );
    y += 14;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "HUD: [F1]  Team: [F2]",
        1,
        muted,
        panel_bg,
    );
    y += 14;
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        "Reset: [R]  Popout: [F6]",
        1,
        muted,
        panel_bg,
    );
}

pub fn draw_toast(buf: &mut [u32], buf_w: usize, buf_h: usize, toast: Toast<'_>) {
    let scale = 1usize;
    let pad_x = 10usize;
    let pad_y = 8usize;
    let text_w = text_width(toast.text, scale);
    let width = (text_w + pad_x * 2).min(buf_w.saturating_sub(12)).max(72);
    let height = CHAR_H * scale + scale + pad_y * 2;
    let x = (buf_w.saturating_sub(width)) / 2;
    let y = buf_h.saturating_sub(height + 10);

    let (bg, border, text) = match toast.tone {
        ToastTone::Info => (0xFF_13_1B_2F, 0xFF_2E_C4_B6, 0xFF_F4_F8_FB),
        ToastTone::Success => (0xFF_11_22_1B, 0xFF_3D_DC_97, 0xFF_F4_F8_FB),
        ToastTone::Warning => (0xFF_2A_17_14, 0xFF_FF_9F_1C, 0xFF_F9_F3_E6),
    };

    draw_panel(buf, buf_w, buf_h, x, y, width, height, bg, border, border);
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        x + width / 2,
        y + pad_y,
        toast.text,
        scale,
        text,
        bg,
    );
}

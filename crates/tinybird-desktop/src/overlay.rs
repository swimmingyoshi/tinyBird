//! On-screen overlay renderer using an embedded 8×8 pixel-art font.
//!
//! Draws directly into the ARGB u32 pixel buffer produced by softbuffer.

use tinybird_addons::schema::{AddonSection, AddonSectionContent};

use crate::pokemon_assets::{PokemonSpriteStore, SpriteBitmap};
use tinybird_games::{
    AddonData, FireRedAreaSnapshot, FireRedBattleSnapshot, FireRedEncounterEntry,
    FireRedEncounterGroup, FireRedMoveSlot, FireRedPartyMember, FireRedSnapshot, FireRedStatSpread,
    StreamSnapshot,
};

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

#[derive(Clone, Copy)]
pub enum SaveStateMenuMode {
    Save,
    Load,
}

pub struct SaveStateMenu {
    pub mode: SaveStateMenuMode,
    pub selected_slot: u8,
    pub slot_exists: [bool; 5],
    pub confirm_overwrite: bool,
}

pub struct ThemeMenu {
    pub selected_theme: u8,
    pub active_theme: u8,
    pub has_wallpaper: bool,
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

fn rom_display_label(snapshot: &StreamSnapshot) -> String {
    if let Some(addon) = &snapshot.addon {
        if addon.display_name.starts_with("FireRed") {
            return "Pokemon FireRed".to_string();
        }
        if addon.display_name.starts_with("LeafGreen") {
            return "Pokemon LeafGreen".to_string();
        }
    }

    let Some(rom) = &snapshot.rom else {
        return tinybird_games::game_display_label(snapshot);
    };

    if rom.game_code.starts_with("BPR") || rom.title.eq_ignore_ascii_case("POKEMON FIRE") {
        "Pokemon FireRed".to_string()
    } else if rom.game_code.starts_with("BPG") || rom.title.eq_ignore_ascii_case("POKEMON LEAF") {
        "Pokemon LeafGreen".to_string()
    } else if rom.game_code.is_empty() {
        "Unknown cartridge".to_string()
    } else {
        // The raw 12-byte header title is often a build tag rather than a name
        // ("FFTA_USVER."), so lead with the game code and region instead.
        format!("{} - {}", rom.game_code, rom.region_name())
    }
}

/// Widest the generic section column is allowed to get.
///
/// Without a cap, a key/value row on a full-width dashboard puts its label at
/// the far left and its value at the far right, which reads as two unrelated
/// columns instead of one field.
const GENERIC_CONTENT_MAX_W: usize = 380;

pub const GAME_PREVIEW_SIZE_COUNT: u8 = 3;
pub const SIDE_PANEL_SIZE_COUNT: u8 = 3;
pub const DASHBOARD_LAYOUT_COUNT: u8 = 4;
pub const DASHBOARD_THEME_COUNT: u8 = 4;

pub struct DashboardWallpaper {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

pub fn game_preview_size_label(size: u8) -> &'static str {
    match size % GAME_PREVIEW_SIZE_COUNT {
        0 => "Compact",
        1 => "Balanced",
        _ => "Game Focus",
    }
}

pub fn side_panel_size_label(size: u8) -> &'static str {
    match size % SIDE_PANEL_SIZE_COUNT {
        0 => "Slim",
        1 => "Normal",
        _ => "Wide",
    }
}

pub fn dashboard_layout_label(layout: u8) -> &'static str {
    match layout % DASHBOARD_LAYOUT_COUNT {
        0 => "Classic",
        1 => "Left Info",
        2 => "Split Sidebars",
        _ => "Cozy Game",
    }
}

pub fn dashboard_theme_label(theme: u8) -> &'static str {
    match theme % DASHBOARD_THEME_COUNT {
        0 => "Midnight",
        1 => "Verdant",
        2 => "Cobalt",
        _ => "Wallpaper",
    }
}

fn side_panel_reserved_width(buf_w: usize, size: u8) -> usize {
    match size % SIDE_PANEL_SIZE_COUNT {
        0 => {
            if buf_w >= 980 {
                300
            } else {
                240
            }
        }
        1 => {
            if buf_w >= 980 {
                420
            } else {
                260
            }
        }
        _ => {
            if buf_w >= 1280 {
                560
            } else if buf_w >= 980 {
                480
            } else {
                320
            }
        }
    }
}

pub fn game_preview_dimensions(
    buf_w: usize,
    buf_h: usize,
    size: u8,
    side_panel_size: u8,
    dashboard_layout: u8,
) -> (usize, usize) {
    if buf_w < 24 || buf_h < 42 {
        return (0, 0);
    }

    let layout = dashboard_layout % DASHBOARD_LAYOUT_COUNT;
    let right_min = side_panel_reserved_width(buf_w, side_panel_size);
    let side_columns = match layout {
        2 => 2,
        3 => 0,
        _ => 1,
    };
    let bottom_min = if layout >= 2 {
        24
    } else if buf_w >= 1280 {
        142
    } else if buf_h >= 760 {
        172
    } else {
        132
    };
    let footer_min = 46usize;
    let max_w_by_height = buf_h
        .saturating_sub(bottom_min + footer_min + 42)
        .saturating_mul(3)
        / 2;
    let max_w_by_side_panel = if layout == 3 {
        buf_w.saturating_sub(40)
    } else if layout == 0 {
        buf_w.saturating_sub((right_min + 32) * 2)
    } else {
        buf_w.saturating_sub(right_min * side_columns + 80)
    };
    let layout_cap = max_w_by_height
        .min(max_w_by_side_panel)
        .min(buf_w.saturating_sub(24));
    if layout_cap < 96 {
        return (0, 0);
    }

    let remaining_for_game = if layout == 3 {
        buf_w.saturating_sub(40)
    } else if layout == 0 {
        buf_w.saturating_sub((right_min + 32) * 2)
    } else {
        buf_w.saturating_sub(right_min * side_columns + 80)
    };
    let preferred = match size % GAME_PREVIEW_SIZE_COUNT {
        0 => remaining_for_game * 45 / 100,
        1 => remaining_for_game * 72 / 100,
        _ => remaining_for_game,
    };
    let preview_w = preferred.clamp(96, usize::MAX).min(layout_cap);
    let preview_h = preview_w * 2 / 3;
    (preview_w, preview_h)
}

pub fn game_preview_frame_rect(
    buf_w: usize,
    buf_h: usize,
    size: u8,
    side_panel_size: u8,
    dashboard_layout: u8,
) -> Option<(usize, usize, usize, usize)> {
    let (preview_w, preview_h) =
        game_preview_dimensions(buf_w, buf_h, size, side_panel_size, dashboard_layout);
    if preview_w == 0 || preview_h == 0 {
        return None;
    }

    let outer_w = preview_w + 8;
    let layout = dashboard_layout % DASHBOARD_LAYOUT_COUNT;
    let outer_x = match layout {
        1 => side_panel_reserved_width(buf_w, side_panel_size) + 40,
        2 => {
            let side_w = side_panel_reserved_width(buf_w, side_panel_size);
            let left = side_w + 32;
            let right = buf_w.saturating_sub(side_w + 12);
            let available = right.saturating_sub(left);
            left + available.saturating_sub(outer_w) / 2
        }
        3 => buf_w.saturating_sub(outer_w) / 2,
        _ => {
            let side_w = side_panel_reserved_width(buf_w, side_panel_size);
            let centered = buf_w.saturating_sub(outer_w) / 2;
            let max_x = buf_w.saturating_sub(side_w + 28 + outer_w);
            centered.min(max_x)
        }
    }
    .min(buf_w.saturating_sub(outer_w + 4));
    let outer_h = preview_h + 28;
    let outer_y = if layout >= 2 {
        let top = 12usize;
        let bottom = buf_h.saturating_sub(34);
        let available_h = bottom.saturating_sub(top);
        top + available_h.saturating_sub(outer_h) / 2
    } else {
        12
    };

    Some((outer_x + 4, outer_y + 24, preview_w, preview_h))
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

fn draw_dashboard_background(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    theme: u8,
    wallpaper: Option<&DashboardWallpaper>,
) {
    if theme % DASHBOARD_THEME_COUNT == 3 {
        if let Some(wallpaper) = wallpaper {
            draw_wallpaper_cover(buf, buf_w, buf_h, wallpaper);
            tint_buffer(buf, 0xFF_06_0A_12, 72);
            return;
        }
    }

    let (top, bottom, accent) = match theme % DASHBOARD_THEME_COUNT {
        1 => (0xFF_0714_10, 0xFF_1024_1B, 0xFF_3D_DC_97),
        2 => (0xFF_0810_1F, 0xFF_1424_3A, 0xFF_6C_A8_FF),
        _ => (0xFF_0A_0F_19, 0xFF_141B_2A, 0xFF_1E_2A_3F),
    };
    draw_detached_addon_background(buf, buf_w, buf_h, top, bottom, accent);
}

fn draw_wallpaper_cover(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    wallpaper: &DashboardWallpaper,
) {
    if buf_w == 0 || buf_h == 0 || wallpaper.width == 0 || wallpaper.height == 0 {
        return;
    }

    let scale_by_w = buf_w * wallpaper.height >= buf_h * wallpaper.width;
    let (draw_w, draw_h) = if scale_by_w {
        (buf_w, (buf_w * wallpaper.height / wallpaper.width).max(1))
    } else {
        ((buf_h * wallpaper.width / wallpaper.height).max(1), buf_h)
    };
    let crop_x = draw_w.saturating_sub(buf_w) / 2;
    let crop_y = draw_h.saturating_sub(buf_h) / 2;

    for y in 0..buf_h {
        let src_y = (y + crop_y) * wallpaper.height / draw_h;
        for x in 0..buf_w {
            let src_x = (x + crop_x) * wallpaper.width / draw_w;
            buf[y * buf_w + x] = wallpaper.pixels[src_y * wallpaper.width + src_x];
        }
    }
}

fn tint_buffer(buf: &mut [u32], tint: u32, amount: u32) {
    let tr = (tint >> 16) & 0xFF;
    let tg = (tint >> 8) & 0xFF;
    let tb = tint & 0xFF;
    for pixel in buf.iter_mut() {
        let r = (*pixel >> 16) & 0xFF;
        let g = (*pixel >> 8) & 0xFF;
        let b = *pixel & 0xFF;
        let r = (r * (255 - amount) + tr * amount) / 255;
        let g = (g * (255 - amount) + tg * amount) / 255;
        let b = (b * (255 - amount) + tb * amount) / 255;
        *pixel = 0xFF_00_00_00 | (r << 16) | (g << 8) | b;
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
    let scale = if buf_w < 620 || buf_h < 280 { 1 } else { 2 };
    let cell = CHAR_W * scale + scale;
    let line = CHAR_H * scale + scale;

    // Panel dimensions
    let cols = if scale == 1 { 31usize } else { 30usize };
    let rows = 12usize; // lines tall
    let pad = 6usize;
    let panel_w = (cols * cell + pad * 2).min(buf_w.saturating_sub(16));
    let panel_h = (rows * line + pad * 2).min(buf_h.saturating_sub(16));
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
    draw_text(buf, buf_w, buf_h, tx, ty, "tinyBird", scale, cyan, bg);
    ty += line + scale;

    // Separator
    fill_rect(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        panel_w.saturating_sub(pad * 2),
        1,
        grey,
    );
    ty += scale * 2;

    // FPS line
    let fps_str = format!("FPS: {:.1}", fps);
    draw_text(buf, buf_w, buf_h, tx, ty, &fps_str, scale, white, bg);
    ty += line;

    // Speed line
    let speed_str = format!("Speed: {}x  1/2/4", speed);
    draw_text(buf, buf_w, buf_h, tx, ty, &speed_str, scale, white, bg);
    ty += line;

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
        scale,
        audio_color,
        bg,
    );
    ty += line;

    // Volume line
    let vol_str = format!("Volume: {}%  -/+", volume_pct);
    draw_text(buf, buf_w, buf_h, tx, ty, &vol_str, scale, white, bg);
    ty += line;

    // Color correction line
    let (cc_label, cc_color) = if color_correction {
        ("LCD color fix: ON  [C]", green)
    } else {
        ("LCD color fix: OFF [C]", grey)
    };
    draw_text(buf, buf_w, buf_h, tx, ty, cc_label, scale, cc_color, bg);
    ty += line + scale;

    // Separator
    fill_rect(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        panel_w.saturating_sub(pad * 2),
        1,
        grey,
    );
    ty += scale * 2;

    // Key bindings
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "Esc Pause   O Open",
        scale,
        grey,
        bg,
    );
    ty += line;
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "R Reset     F1 HUD",
        scale,
        grey,
        bg,
    );
    ty += line;
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "F2 Team    F4 Encounters",
        scale,
        grey,
        bg,
    );
    ty += line;
    draw_text(
        buf,
        buf_w,
        buf_h,
        tx,
        ty,
        "F6 Dash  F7 Game  F10 Menu",
        scale,
        grey,
        bg,
    );
}

pub fn draw_addon_panel(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    snapshot: &StreamSnapshot,
    sprites: &PokemonSpriteStore,
    expanded: bool,
    view_mode: AddonViewMode,
    preview_size: u8,
    side_panel_size: u8,
    dashboard_layout: u8,
    dashboard_theme: u8,
    wallpaper: Option<&DashboardWallpaper>,
) {
    let panel_bg = 0xFF_12_17_26;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;

    if expanded {
        draw_dashboard_background(buf, buf_w, buf_h, dashboard_theme, wallpaper);
    }

    let panel_w = if expanded {
        buf_w.saturating_sub(24).max(360).min(buf_w)
    } else if view_mode == AddonViewMode::Encounters {
        side_panel_reserved_width(buf_w, side_panel_size)
            .clamp(260, 420)
            .min(buf_w.saturating_sub(16))
    } else {
        side_panel_reserved_width(buf_w, side_panel_size)
            .clamp(240, 380)
            .min(buf_w.saturating_sub(20))
    };
    let panel_h = if expanded {
        buf_h.saturating_sub(24).max(260).min(buf_h)
    } else if view_mode == AddonViewMode::Encounters {
        buf_h.saturating_sub(12).clamp(220, 520)
    } else {
        buf_h.saturating_sub(16).clamp(220, 520)
    };
    let panel_x = if expanded {
        (buf_w.saturating_sub(panel_w)) / 2
    } else {
        buf_w.saturating_sub(panel_w + 8)
    };
    let panel_y = if expanded {
        (buf_h.saturating_sub(panel_h)) / 2
    } else {
        8
    };

    if expanded {
        fill_rect(buf, buf_w, buf_h, panel_x, panel_y, panel_w, 2, accent);
        fill_rect(
            buf,
            buf_w,
            buf_h,
            panel_x,
            panel_y + panel_h.saturating_sub(2),
            panel_w,
            2,
            accent_2,
        );
        fill_rect(buf, buf_w, buf_h, panel_x, panel_y, 2, panel_h, accent_2);
        fill_rect(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w.saturating_sub(2),
            panel_y,
            2,
            panel_h,
            accent_2,
        );
    } else {
        draw_panel(
            buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
        );
    }

    let (preview_w, preview_frame_h) = game_preview_dimensions(
        buf_w,
        buf_h,
        preview_size,
        side_panel_size,
        dashboard_layout,
    );
    let preview_h = preview_frame_h + 28;

    if expanded {
        if let Some(AddonData::FireRed(data)) = snapshot.addon.as_ref().map(|addon| &addon.data) {
            draw_firered_dashboard(
                buf,
                buf_w,
                buf_h,
                panel_x,
                panel_y,
                panel_w,
                panel_h,
                preview_w,
                preview_h,
                side_panel_size,
                dashboard_layout,
                snapshot,
                data,
                sprites,
            );
            return;
        }
    }

    // A game with a hand-written renderer lays its content out around the
    // preview. Everything else gets a fixed card on the right, because the
    // preview is centred in the panel and would otherwise be drawn straight
    // over a full-width column.
    let has_rich_renderer = matches!(
        snapshot.addon.as_ref().map(|addon| &addon.data),
        Some(AddonData::FireRed(_))
    );
    let side_by_side_preview =
        has_rich_renderer && expanded && panel_w >= preview_w + 460 && panel_h >= 300;

    let (content_x, content_w) = if expanded && !has_rich_renderer {
        let card_w = panel_w
            .saturating_sub(32)
            .min(GENERIC_CONTENT_MAX_W)
            .max(180);
        (panel_x + panel_w.saturating_sub(card_w + 16), card_w)
    } else {
        let start = if side_by_side_preview {
            panel_x + preview_w + 36
        } else {
            panel_x + 12
        };
        let width = panel_x
            .saturating_add(panel_w)
            .saturating_sub(start)
            .saturating_sub(12)
            .max(180);
        (start, width)
    };
    let center_x = content_x + content_w / 2;
    let mut y = if expanded && has_rich_renderer && !side_by_side_preview {
        panel_y + preview_h + 18
    } else {
        panel_y + 14
    };
    let mut title_scale = if expanded || panel_w > 260 { 2 } else { 1 };
    let title = snapshot
        .addon
        .as_ref()
        .map(|addon| match view_mode {
            AddonViewMode::Team => addon.display_name.to_string(),
            AddonViewMode::Encounters => addon.display_name.replace("Party", "Encounters"),
        })
        .unwrap_or_else(|| "Game Addons".to_string());
    if title_scale > 1 && text_width(&title, title_scale) > content_w {
        title_scale = 1;
    }
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        center_x,
        y,
        &short_label(&title, content_w / (9 * title_scale)),
        title_scale,
        accent,
        panel_bg,
    );
    y += title_scale * 10 + 4;

    let subtitle = rom_display_label(snapshot);
    draw_text_centered(
        buf, buf_w, buf_h, center_x, y, &subtitle, 1, muted, panel_bg,
    );
    y += 16;

    fill_rect(buf, buf_w, buf_h, content_x, y, content_w, 1, muted);
    y += 12;

    let footer_y = panel_y + panel_h.saturating_sub(18);
    match snapshot.addon.as_ref().map(|addon| &addon.data) {
        Some(AddonData::FireRed(data)) => match view_mode {
            AddonViewMode::Team => {
                draw_firered_party_panel(
                    buf,
                    buf_w,
                    buf_h,
                    content_x,
                    y,
                    content_w,
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
                    content_x,
                    y,
                    content_w,
                    footer_y.saturating_sub(8),
                    data,
                    sprites,
                );
            }
        },
        // Every other payload describes itself through schema sections, which
        // the generic renderer below can draw without knowing the game.
        Some(_) => {
            let sections = snapshot
                .addon
                .as_ref()
                .map(|addon| addon.sections.as_slice())
                .unwrap_or(&[]);
            draw_addon_sections(
                buf,
                buf_w,
                buf_h,
                content_x,
                y,
                content_w,
                footer_y.saturating_sub(8),
                sections,
            );
        }
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
                "Load a ROM to populate this view.",
                1,
                muted,
                panel_bg,
            );
        }
    }

    let footer = if expanded {
        match view_mode {
            AddonViewMode::Team => "F4 Encounters   F7 Game   F10 Menu   F11 Move",
            AddonViewMode::Encounters => "F2 Team   F7 Game   F10 Menu   F11 Move",
        }
    } else if view_mode == AddonViewMode::Team {
        "F4 Encounters   F6 Dash   F3 Hide"
    } else {
        "F2 Team   F6 Dash   F3 Hide"
    };
    if !expanded && content_w < text_width(footer, 1) + 8 {
        let first = match view_mode {
            AddonViewMode::Team => "F4 Encounters",
            AddonViewMode::Encounters => "F2 Team",
        };
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            footer_y.saturating_sub(12),
            first,
            1,
            muted,
            panel_bg,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            footer_y,
            "F6 Dash   F3 Hide",
            1,
            muted,
            panel_bg,
        );
    } else {
        draw_text_centered(
            buf, buf_w, buf_h, center_x, footer_y, footer, 1, muted, panel_bg,
        );
    }
}

/// Draw generic addon sections: the fallback renderer for any game.
///
/// Games without a hand-written dashboard still export `key_value`, `list`, and
/// `table` sections through the shared schema. Before this existed, loading
/// anything but FireRed drew "No addon data yet" and nothing else — the schema
/// was consumed only by the web overlay.
#[allow(clippy::too_many_arguments)]
fn draw_addon_sections(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    top: usize,
    width: usize,
    bottom: usize,
    sections: &[AddonSection],
) {
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let bg = 0xFF_12_17_26;
    let row_h = 12;

    if sections.is_empty() {
        draw_text(
            buf,
            buf_w,
            buf_h,
            x,
            top,
            "No sections reported",
            1,
            muted,
            bg,
        );
        return;
    }

    let mut y = top;
    for section in sections {
        // Stop cleanly rather than drawing a half-clipped section.
        if y + row_h * 2 > bottom {
            draw_text(buf, buf_w, buf_h, x, y, "...", 1, muted, bg);
            return;
        }

        draw_text(buf, buf_w, buf_h, x, y, &section.title, 1, accent, bg);
        y += row_h;
        fill_rect(buf, buf_w, buf_h, x, y, width, 1, 0xFF_2E_3A_57);
        y += 5;

        // The note says what the section is reading. Worth a line here only
        // while there is room for content under it as well.
        if let Some(note) = &section.note {
            if y + row_h * 2 <= bottom {
                let text = short_label(note, width / 9);
                draw_text(buf, buf_w, buf_h, x, y, &text, 1, muted, bg);
                y += row_h;
            }
        }

        match &section.content {
            AddonSectionContent::KeyValue(fields) => {
                for field in fields {
                    if y + row_h > bottom {
                        break;
                    }
                    draw_text(buf, buf_w, buf_h, x, y, &field.label, 1, muted, bg);
                    let value_w = text_width(&field.value, 1);
                    let value_x = x + width.saturating_sub(value_w);
                    // Only right-align when the two do not collide.
                    if value_x > x + text_width(&field.label, 1) + 8 {
                        draw_text(buf, buf_w, buf_h, value_x, y, &field.value, 1, ink, bg);
                    }
                    y += row_h;
                }
            }
            AddonSectionContent::List(items) => {
                for item in items {
                    if y + row_h > bottom {
                        break;
                    }
                    let text = short_label(item, width / 9);
                    draw_text(buf, buf_w, buf_h, x, y, &text, 1, ink, bg);
                    y += row_h;
                }
            }
            AddonSectionContent::Table(table) => {
                let columns = table.columns.len().max(1);
                let column_w = width / columns;
                let cell_chars = (column_w / 9).max(3);

                for (index, heading) in table.columns.iter().enumerate() {
                    draw_text(
                        buf,
                        buf_w,
                        buf_h,
                        x + index * column_w,
                        y,
                        &short_label(heading, cell_chars),
                        1,
                        muted,
                        bg,
                    );
                }
                y += row_h;

                for row in &table.rows {
                    if y + row_h > bottom {
                        break;
                    }
                    for (index, cell) in row.iter().take(columns).enumerate() {
                        draw_text(
                            buf,
                            buf_w,
                            buf_h,
                            x + index * column_w,
                            y,
                            &short_label(cell, cell_chars),
                            1,
                            ink,
                            bg,
                        );
                    }
                    y += row_h;
                }
            }
            // A card is a heading with its own headline stat, some flags, and
            // detail rows. At this size the bars a graphical consumer draws
            // are not worth the pixels, so meters fall back to their text.
            AddonSectionContent::Cards(cards) => {
                for card in cards {
                    if y + row_h * 2 > bottom {
                        break;
                    }

                    let mut heading = card.title.clone();
                    if let Some(lead) = &card.lead {
                        heading.push_str(&format!("  {}", lead.value));
                    }
                    draw_text(
                        buf,
                        buf_w,
                        buf_h,
                        x,
                        y,
                        &short_label(&heading, width / 9),
                        1,
                        ink,
                        bg,
                    );
                    y += row_h;

                    // Subtitle and badges share one line: both are short, and
                    // neither is worth a row of its own in a stream overlay.
                    let mut chips: Vec<String> = Vec::new();
                    if let Some(subtitle) = &card.subtitle {
                        chips.push(subtitle.clone());
                    }
                    chips.extend(card.badges.iter().map(|badge| badge.text.clone()));
                    if !chips.is_empty() && y + row_h <= bottom {
                        let text = short_label(&chips.join(" - "), width / 9);
                        draw_text(buf, buf_w, buf_h, x, y, &text, 1, muted, bg);
                        y += row_h;
                    }

                    for field in &card.fields {
                        if y + row_h > bottom {
                            break;
                        }
                        let text =
                            short_label(&format!("{}: {}", field.label, field.value), width / 9);
                        draw_text(buf, buf_w, buf_h, x + 6, y, &text, 1, ink, bg);
                        y += row_h;
                    }
                    y += 4;
                }
            }
        }
        y += 8;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_firered_dashboard(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    panel_x: usize,
    panel_y: usize,
    panel_w: usize,
    panel_h: usize,
    preview_w: usize,
    preview_h: usize,
    side_panel_size: u8,
    dashboard_layout: u8,
    snapshot: &StreamSnapshot,
    data: &FireRedSnapshot,
    sprites: &PokemonSpriteStore,
) {
    let panel_bg = 0xFF_12_17_26;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;

    let footer_y = panel_y + panel_h.saturating_sub(18);
    let content_top = panel_y + 14;
    let content_bottom = footer_y.saturating_sub(24);
    let preview_outer_w = preview_w + 8;
    let preview_outer_h = preview_h;
    let requested_right_w = side_panel_reserved_width(buf_w, side_panel_size);
    let layout = dashboard_layout % DASHBOARD_LAYOUT_COUNT;
    let max_right_w = panel_w.saturating_sub(preview_outer_w + 52);
    let right_w = requested_right_w.clamp(240, max_right_w.max(240));
    let right_x = if layout == 1 {
        panel_x + 12
    } else {
        panel_x + panel_w.saturating_sub(right_w + 12)
    };
    let right_available = right_w >= 240;
    let party_x = panel_x + 12;
    let party_w = panel_w.saturating_sub(24);
    let preview_bottom = panel_y + preview_outer_h + 18;

    if layout == 3 {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w / 2,
            footer_y.saturating_sub(12),
            "F11 Layout   F7 Game   F9 Fullscreen   F12 Theme",
            1,
            muted,
            panel_bg,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w / 2,
            footer_y,
            "F2 Team   F4 Encounters   W Wallpaper",
            1,
            muted,
            panel_bg,
        );
        return;
    }

    if layout == 2 {
        let max_side_w = (panel_w.saturating_sub(preview_outer_w + 72) / 2).max(240);
        let side_w = requested_right_w.clamp(240, max_side_w);
        let left_x = panel_x + 12;
        let side_bottom = content_bottom;
        let encounter_x = panel_x + panel_w.saturating_sub(side_w + 12);
        let column_h = side_bottom.saturating_sub(content_top);
        let party_body_h = estimated_party_panel_height(side_w, data.party.len());
        let party_group_h = (58 + party_body_h).min(column_h);
        let party_group_y = centered_group_y(content_top, side_bottom, party_group_h);
        let party_panel_y = party_group_y + 58;
        let party_panel_bottom = party_panel_y.saturating_add(party_body_h).min(side_bottom);
        let encounter_body_h = estimated_encounter_panel_height(data);
        let encounter_group_h = (58 + encounter_body_h).min(column_h);
        let encounter_group_y = centered_group_y(content_top, side_bottom, encounter_group_h);
        let encounter_panel_y = encounter_group_y + 58;
        let encounter_panel_bottom = encounter_panel_y
            .saturating_add(encounter_body_h)
            .min(side_bottom);

        fill_rect(
            buf,
            buf_w,
            buf_h,
            left_x,
            party_group_y + 46,
            side_w,
            1,
            muted,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            left_x + side_w / 2,
            party_group_y,
            "Party",
            2,
            accent,
            panel_bg,
        );
        draw_firered_party_panel(
            buf,
            buf_w,
            buf_h,
            left_x,
            party_panel_y,
            side_w,
            party_panel_bottom,
            data,
            sprites,
        );

        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            encounter_x + side_w / 2,
            encounter_group_y,
            "Encounters",
            2,
            accent,
            panel_bg,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            encounter_x + side_w / 2,
            encounter_group_y + 24,
            &rom_display_label(snapshot),
            1,
            muted,
            panel_bg,
        );
        fill_rect(
            buf,
            buf_w,
            buf_h,
            encounter_x,
            encounter_group_y + 46,
            side_w,
            1,
            muted,
        );
        draw_firered_encounter_panel(
            buf,
            buf_w,
            buf_h,
            encounter_x,
            encounter_panel_y,
            side_w,
            encounter_panel_bottom,
            data,
            sprites,
        );

        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w / 2,
            footer_y.saturating_sub(12),
            "F11 Layout   F7 Game   F9 Fullscreen   F12 Theme",
            1,
            muted,
            panel_bg,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w / 2,
            footer_y,
            "F2 Team   F4 Encounters   W Wallpaper",
            1,
            muted,
            panel_bg,
        );
        return;
    }

    let min_party_h = if party_w >= 1200 {
        126
    } else if party_w >= 420 {
        190
    } else {
        224
    };
    let party_bottom = content_bottom;
    let party_y = party_bottom
        .saturating_sub(min_party_h)
        .max(preview_bottom)
        .min(party_bottom);
    let encounter_bottom = if right_available {
        party_y.saturating_sub(12).max(content_top + 82)
    } else {
        party_y.saturating_sub(12)
    };

    if right_available {
        let center_x = right_x + right_w / 2;
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            content_top,
            "Encounters",
            2,
            accent,
            panel_bg,
        );
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            center_x,
            content_top + 24,
            &rom_display_label(snapshot),
            1,
            muted,
            panel_bg,
        );
        fill_rect(
            buf,
            buf_w,
            buf_h,
            right_x,
            content_top + 46,
            right_w,
            1,
            muted,
        );
        if content_top + 120 < encounter_bottom {
            draw_firered_encounter_panel(
                buf,
                buf_w,
                buf_h,
                right_x,
                content_top + 58,
                right_w,
                encounter_bottom,
                data,
                sprites,
            );
        }
    }

    if party_y + 52 < party_bottom {
        fill_rect(
            buf,
            buf_w,
            buf_h,
            party_x,
            party_y.saturating_sub(10),
            party_w,
            1,
            accent_2,
        );
        draw_text(
            buf, buf_w, buf_h, party_x, party_y, "Party", 1, accent, panel_bg,
        );
        draw_firered_party_panel(
            buf,
            buf_w,
            buf_h,
            party_x,
            party_y + 16,
            party_w,
            party_bottom,
            data,
            sprites,
        );
    } else if !right_available {
        draw_text_centered(
            buf,
            buf_w,
            buf_h,
            panel_x + panel_w / 2,
            panel_y + preview_h + 20,
            "Use F7 to favor addon space in this window.",
            1,
            muted,
            panel_bg,
        );
    }

    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        footer_y.saturating_sub(12),
        "F7 Game   F9 Fullscreen   F11 Move   F12 Theme",
        1,
        muted,
        panel_bg,
    );
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        footer_y,
        "F2 Team   F4 Encounters   W Wallpaper",
        1,
        muted,
        panel_bg,
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
        return;
    }

    let card_gap = 6usize;
    let content_y = panel_y + 18;
    let available_h = panel_bottom.saturating_sub(content_y);
    let desired_grid = if panel_w >= 1200 {
        [(6usize, 1usize), (3usize, 2usize), (2usize, 3usize)].as_slice()
    } else if panel_w >= 360 {
        [(3usize, 2usize), (2usize, 3usize)].as_slice()
    } else {
        &[]
    }
    .iter()
    .copied()
    .find_map(|(cols, rows)| {
        let card_w = panel_w.saturating_sub(card_gap * (cols - 1)) / cols;
        let card_h = available_h
            .saturating_sub(card_gap * (rows - 1))
            .min(70 * rows + card_gap * (rows - 1))
            / rows;
        (card_w >= 150 && card_h >= 46).then_some((cols, rows, card_w, card_h))
    });

    if let Some((grid_cols, grid_rows, grid_card_w, grid_card_h)) = desired_grid {
        for (idx, member) in snapshot
            .party
            .iter()
            .take(grid_cols * grid_rows)
            .enumerate()
        {
            let col = idx % grid_cols;
            let row = idx / grid_cols;
            let x = panel_x + col * (grid_card_w + card_gap);
            let y = content_y + row * (grid_card_h + card_gap);
            if y + grid_card_h > panel_bottom {
                break;
            }
            draw_firered_party_card(
                buf,
                buf_w,
                buf_h,
                x,
                y,
                grid_card_w,
                grid_card_h,
                member,
                sprites,
            );
        }
        return;
    }

    let card_h = if panel_w < 320 { 64usize } else { 70usize };
    let mut y = content_y;
    for member in &snapshot.party {
        if y + card_h > panel_bottom {
            break;
        }
        draw_firered_party_card(
            buf, buf_w, buf_h, panel_x, y, panel_w, card_h, member, sprites,
        );
        y += card_h + card_gap;
    }
}

fn centered_group_y(top: usize, bottom: usize, desired_h: usize) -> usize {
    let available_h = bottom.saturating_sub(top);
    if desired_h >= available_h {
        top
    } else {
        top + (available_h - desired_h) / 2
    }
}

fn estimated_party_panel_height(panel_w: usize, party_len: usize) -> usize {
    if party_len == 0 {
        return 44;
    }

    let card_gap = 6usize;
    if panel_w >= 1200 {
        return 18 + 70;
    }
    if panel_w >= 360 {
        let rows = party_len.min(6).div_ceil(3).clamp(1, 2);
        return 18 + rows * 70 + rows.saturating_sub(1) * card_gap;
    }

    let card_h = if panel_w < 320 { 64usize } else { 70usize };
    let cards = party_len.min(6);
    18 + cards * card_h + cards.saturating_sub(1) * card_gap
}

fn estimated_encounter_panel_height(snapshot: &FireRedSnapshot) -> usize {
    let area_h = snapshot
        .area
        .as_ref()
        .map(estimated_area_panel_height)
        .unwrap_or(0);

    match (&snapshot.area, &snapshot.battle) {
        (Some(_), Some(_)) => area_h + 12 + 14 + 180,
        (Some(area), None) if area.encounter_groups.is_empty() => area_h + 14,
        (Some(_), None) => area_h + 32,
        (None, Some(_)) => 14 + 180,
        (None, None) => 34,
    }
}

fn estimated_area_panel_height(area: &FireRedAreaSnapshot) -> usize {
    let mut h = 24usize;
    for group in &area.encounter_groups {
        h += 14 + group.entries.len().min(8) * 12 + 8;
    }
    h.max(44)
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
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;

    match (&snapshot.area, &snapshot.battle) {
        (Some(area), Some(battle)) => {
            let available_h = panel_bottom.saturating_sub(panel_y);
            let battle_reserve = if available_h >= 390 { 206 } else { 176 };
            let area_bottom = panel_bottom
                .saturating_sub(battle_reserve)
                .max(panel_y + 72);
            let area_end = draw_firered_area_panel(
                buf,
                buf_w,
                buf_h,
                panel_x,
                panel_y,
                panel_w,
                area_bottom,
                area,
            );
            let battle_y = area_end
                .saturating_add(12)
                .min(area_bottom.saturating_add(8));
            if battle_y + 158 <= panel_bottom {
                fill_rect(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x,
                    battle_y.saturating_sub(8),
                    panel_w,
                    1,
                    accent_2,
                );
                draw_text(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x,
                    battle_y,
                    "Encountered Pokemon",
                    1,
                    accent,
                    panel_bg,
                );
                draw_firered_battle_panel(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x,
                    battle_y + 14,
                    panel_w,
                    panel_bottom,
                    battle,
                    sprites,
                );
            }
        }
        (Some(area), None) => {
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
            } else if y + 28 <= panel_bottom {
                fill_rect(buf, buf_w, buf_h, panel_x, y + 4, panel_w, 1, accent_2);
                draw_text(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x,
                    y + 12,
                    "No active encounter",
                    1,
                    muted,
                    panel_bg,
                );
            }
        }
        (None, Some(battle)) => {
            draw_text(
                buf,
                buf_w,
                buf_h,
                panel_x,
                panel_y,
                "Encountered Pokemon",
                1,
                accent,
                panel_bg,
            );
            draw_firered_battle_panel(
                buf,
                buf_w,
                buf_h,
                panel_x,
                panel_y + 14,
                panel_w,
                panel_bottom,
                battle,
                sprites,
            );
        }
        (None, None) => {
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
            if panel_y + 18 <= panel_bottom {
                draw_text(
                    buf,
                    buf_w,
                    buf_h,
                    panel_x,
                    panel_y + 14,
                    "Move in-game to refresh encounter data.",
                    1,
                    ink,
                    panel_bg,
                );
            }
        }
    }
}

#[allow(dead_code)]
fn draw_firered_battle_panel_compact(
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
        panel_y + 36,
        &hp,
        1,
        muted,
        card_bg,
    );
    let catch = match opponent.catch_rate {
        Some(rate) => format!("Catch {}", rate),
        None => "Catch --".to_string(),
    };
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w.saturating_sub(8 + text_width(&catch, 1)),
        panel_y + 36,
        &catch,
        1,
        accent_2,
        card_bg,
    );

    let held = opponent
        .held_item
        .as_ref()
        .map(|item| item.name.as_str())
        .unwrap_or("No item");
    draw_text(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + 50,
        &short_label(held, if panel_w >= 280 { 22 } else { 14 }),
        1,
        muted,
        card_bg,
    );

    let left_x = panel_x + 8;
    let right_x = panel_x + (panel_w / 2).max(120);
    let mut stat_y = panel_y + 66;
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        stat_y,
        &format!("HP  {}", opponent.stats.hp),
        1,
        ink,
        card_bg,
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        right_x,
        stat_y,
        &format!("Spd {}", opponent.stats.speed),
        1,
        muted,
        card_bg,
    );
    stat_y += 12;
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        stat_y,
        &format!("Atk {}", opponent.stats.attack),
        1,
        ink,
        card_bg,
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        right_x,
        stat_y,
        &format!("SpA {}", opponent.stats.sp_attack),
        1,
        muted,
        card_bg,
    );
    stat_y += 12;
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        stat_y,
        &format!("Def {}", opponent.stats.defense),
        1,
        ink,
        card_bg,
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        right_x,
        stat_y,
        &format!("SpD {}", opponent.stats.sp_def),
        1,
        muted,
        card_bg,
    );

    let iv_line = format_spread_line("IV", &opponent.ivs, opponent.iv_total);
    let ev_line = format_spread_line("EV", &opponent.evs, opponent.ev_total);
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        panel_y + 104,
        &short_label(&iv_line, if text_w > 170 { 26 } else { 19 }),
        1,
        accent_2,
        card_bg,
    );
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        panel_y + 118,
        &short_label(&ev_line, if text_w > 170 { 26 } else { 19 }),
        1,
        danger,
        card_bg,
    );

    let moves = opponent
        .move_slots
        .iter()
        .map(|slot| format!("{} {}", slot.name, slot.pp))
        .collect::<Vec<_>>()
        .join("  ");
    draw_text(
        buf,
        buf_w,
        buf_h,
        left_x,
        panel_y + 134,
        &short_label(&moves, if panel_w >= 320 { 38 } else { 26 }),
        1,
        ink,
        card_bg,
    );

    draw_hp_bar(
        buf,
        buf_w,
        buf_h,
        panel_x + 8,
        panel_y + card_h.saturating_sub(12),
        panel_w.saturating_sub(16),
        4,
        opponent.current_hp,
        opponent.max_hp,
    );

    panel_y + card_h
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
            "Expand dashboard to show encounters.",
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

    if h >= 68 {
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
    } else {
        let nature = format!(
            "{} {}",
            short_label(member.nature, 7),
            format_move_summary(&member.move_slots, 1)
        );
        draw_text(
            buf,
            buf_w,
            buf_h,
            info_x,
            y + 32,
            &short_label(&nature, if w >= 280 { 28 } else { 20 }),
            1,
            0xFF_2E_C4_B6,
            card_bg,
        );
    }

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
    let save_line = "States: F5 Save Menu   F8 Load Menu";
    draw_text_centered(buf, buf_w, buf_h, center_x, y, &save_line, 1, ink, panel_bg);
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
        "Fullscreen: [F9]  Theme: [F12]",
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
        "Reset: [R]  Dashboard: [F6]",
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

pub fn draw_save_state_menu(buf: &mut [u32], buf_w: usize, buf_h: usize, menu: SaveStateMenu) {
    let panel_bg = 0xFF_12_17_26;
    let row_bg = 0xFF_1A_23_36;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;
    let danger = 0xFF_FC_76_6A;

    let panel_w = buf_w.saturating_sub(32).clamp(260, 420);
    let panel_h = 188usize.min(buf_h.saturating_sub(24)).max(150);
    let panel_x = (buf_w.saturating_sub(panel_w)) / 2;
    let panel_y = (buf_h.saturating_sub(panel_h)) / 2;
    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
    );

    let title = match menu.mode {
        SaveStateMenuMode::Save => "Save State",
        SaveStateMenuMode::Load => "Load State",
    };
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        panel_y + 12,
        title,
        2,
        accent,
        panel_bg,
    );

    let mut y = panel_y + 42;
    for slot in 1..=5u8 {
        let idx = (slot - 1) as usize;
        let selected = slot == menu.selected_slot;
        let bg = if selected { 0xFF_22_2D_44 } else { row_bg };
        let border = if selected { accent } else { accent_2 };
        draw_panel(
            buf,
            buf_w,
            buf_h,
            panel_x + 16,
            y,
            panel_w.saturating_sub(32),
            20,
            bg,
            border,
            border,
        );
        let line = format!("{slot}. Slot {slot}");
        draw_text(buf, buf_w, buf_h, panel_x + 26, y + 6, &line, 1, ink, bg);

        let status = if menu.slot_exists[idx] {
            "Saved"
        } else {
            "Empty"
        };
        let status_color = if menu.slot_exists[idx] {
            accent_2
        } else {
            muted
        };
        let status_x = panel_x + panel_w.saturating_sub(26 + text_width(status, 1));
        draw_text(
            buf,
            buf_w,
            buf_h,
            status_x,
            y + 6,
            status,
            1,
            status_color,
            bg,
        );
        y += 24;
    }

    let help = match menu.mode {
        SaveStateMenuMode::Save if menu.confirm_overwrite => {
            "Enter confirms overwrite   Esc cancels"
        }
        SaveStateMenuMode::Save => "1-5 select   Enter saves   Esc cancels",
        SaveStateMenuMode::Load => "1-5 load   Enter loads   Esc cancels",
    };
    let help_color = if menu.confirm_overwrite {
        danger
    } else {
        muted
    };
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        panel_y + panel_h.saturating_sub(18),
        help,
        1,
        help_color,
        panel_bg,
    );
}

pub fn draw_theme_menu(buf: &mut [u32], buf_w: usize, buf_h: usize, menu: ThemeMenu) {
    let panel_bg = 0xFF_12_17_26;
    let row_bg = 0xFF_1A_23_36;
    let ink = 0xFF_F7_F3_EC;
    let muted = 0xFF_96_A0_B4;
    let accent = 0xFF_FF_9F_1C;
    let accent_2 = 0xFF_2E_C4_B6;
    let danger = 0xFF_FC_76_6A;

    let panel_w = buf_w.saturating_sub(32).clamp(280, 460);
    let panel_h = 176usize.min(buf_h.saturating_sub(24)).max(142);
    let panel_x = (buf_w.saturating_sub(panel_w)) / 2;
    let panel_y = (buf_h.saturating_sub(panel_h)) / 2;
    draw_panel(
        buf, buf_w, buf_h, panel_x, panel_y, panel_w, panel_h, panel_bg, accent_2, accent,
    );

    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        panel_y + 12,
        "Theme",
        2,
        accent,
        panel_bg,
    );

    let mut y = panel_y + 42;
    for theme in 0..DASHBOARD_THEME_COUNT {
        let selected = theme == menu.selected_theme % DASHBOARD_THEME_COUNT;
        let active = theme == menu.active_theme % DASHBOARD_THEME_COUNT;
        let bg = if selected { 0xFF_22_2D_44 } else { row_bg };
        let border = if selected { accent } else { accent_2 };
        draw_panel(
            buf,
            buf_w,
            buf_h,
            panel_x + 16,
            y,
            panel_w.saturating_sub(32),
            22,
            bg,
            border,
            border,
        );

        let number = theme + 1;
        let label = format!("{number}. {}", dashboard_theme_label(theme));
        draw_text(buf, buf_w, buf_h, panel_x + 26, y + 7, &label, 1, ink, bg);

        let status = if theme == 3 && !menu.has_wallpaper {
            "Needs W"
        } else if active {
            "Active"
        } else {
            ""
        };
        if !status.is_empty() {
            let status_color = if theme == 3 && !menu.has_wallpaper {
                danger
            } else {
                accent_2
            };
            let status_x = panel_x + panel_w.saturating_sub(26 + text_width(status, 1));
            draw_text(
                buf,
                buf_w,
                buf_h,
                status_x,
                y + 7,
                status,
                1,
                status_color,
                bg,
            );
        }
        y += 26;
    }

    let help = "1-4 select   Enter applies   W wallpaper   Esc cancels";
    draw_text_centered(
        buf,
        buf_w,
        buf_h,
        panel_x + panel_w / 2,
        panel_y + panel_h.saturating_sub(18),
        help,
        1,
        muted,
        panel_bg,
    );
}

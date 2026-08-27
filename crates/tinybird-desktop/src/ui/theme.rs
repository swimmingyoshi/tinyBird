//! Named color tokens for every UI layer.
//!
//! Previously each draw function invented its own hex literals (`0xFF_12_17_26`
//! for panel backgrounds, `0xFF_FF_9F_1C` for accents, and so on) and the theme
//! variants lived in `match theme % 4` arms scattered across the overlay. Here a
//! theme is a value: one `Palette` struct, one static table, one lookup.
//!
//! Colors are stored as `0x00RRGGBB`. The softbuffer surface ignores the high
//! byte and `App::normalize_softbuffer_pixels` masks it off before present, so
//! we keep it zero throughout rather than carrying a fake alpha.

/// Number of selectable themes. Kept in sync with [`THEMES`].
pub const THEME_COUNT: u8 = 4;

/// The wallpaper theme index, which paints a user image instead of a gradient.
pub const THEME_WALLPAPER: u8 = 3;

/// A complete set of color tokens for one theme.
///
/// Tokens are named by *role*, not by appearance, so a new theme only has to
/// answer "what is my surface color?" rather than re-deriving every call site.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub name: &'static str,

    // --- Backdrop: the full-window gradient behind everything ---
    pub backdrop_top: u32,
    pub backdrop_bottom: u32,
    /// Tint used for the decorative pinstripe band along the top edge.
    pub backdrop_band: u32,

    // --- Surfaces: panels, cards, and menus that sit on the backdrop ---
    pub surface: u32,
    /// A card or row sitting on top of `surface`.
    pub surface_raised: u32,
    /// A well or inset area, e.g. a list background.
    pub surface_sunken: u32,
    pub border: u32,

    // --- Chrome: the classic menu bar and status bar ---
    pub chrome: u32,
    pub chrome_hover: u32,
    pub chrome_active: u32,
    pub chrome_border: u32,

    // --- Ink ---
    pub ink: u32,
    pub ink_muted: u32,
    pub ink_disabled: u32,
    /// Text drawn on top of an `accent`-filled area.
    pub ink_on_accent: u32,

    // --- Accents ---
    pub accent: u32,
    pub accent_alt: u32,
    pub selection: u32,
    pub focus_ring: u32,

    // --- Semantics ---
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
}

/// Shared chrome/ink tokens. Every built-in theme uses the same neutral shell so
/// the classic UI stays legible regardless of the dashboard backdrop; themes
/// override the tokens that actually carry their identity.
const BASE: Palette = Palette {
    name: "",

    backdrop_top: 0x000A_0F19,
    backdrop_bottom: 0x0014_1B2A,
    backdrop_band: 0x001E_2A3F,

    surface: 0x0012_1726,
    surface_raised: 0x001A_2133,
    surface_sunken: 0x000C_101B,
    border: 0x002E_C4B6,

    chrome: 0x0016_1C2B,
    chrome_hover: 0x0023_2C42,
    chrome_active: 0x002E_3A57,
    chrome_border: 0x0031_3C55,

    ink: 0x00F7_F3EC,
    ink_muted: 0x0096_A0B4,
    ink_disabled: 0x005A_6375,
    ink_on_accent: 0x0010_141F,

    accent: 0x00FF_9F1C,
    accent_alt: 0x002E_C4B6,
    selection: 0x002E_3A57,
    focus_ring: 0x00FF_9F1C,

    success: 0x005A_DC80,
    warning: 0x00FF_C24D,
    danger: 0x00E5_5A5A,
};

/// The built-in themes, indexed by the persisted theme id.
pub static THEMES: [Palette; THEME_COUNT as usize] = [
    Palette {
        name: "Midnight",
        ..BASE
    },
    Palette {
        name: "Verdant",
        backdrop_top: 0x0007_1410,
        backdrop_bottom: 0x0010_241B,
        backdrop_band: 0x003D_DC97,
        surface: 0x000D_1D17,
        surface_raised: 0x0015_2A21,
        surface_sunken: 0x0008_1510,
        border: 0x003D_DC97,
        accent: 0x009B_E564,
        accent_alt: 0x003D_DC97,
        focus_ring: 0x009B_E564,
        ..BASE
    },
    Palette {
        name: "Cobalt",
        backdrop_top: 0x0008_101F,
        backdrop_bottom: 0x0014_243A,
        backdrop_band: 0x006C_A8FF,
        surface: 0x000E_182C,
        surface_raised: 0x0017_243D,
        surface_sunken: 0x0009_111F,
        border: 0x006C_A8FF,
        accent: 0x006C_A8FF,
        accent_alt: 0x0053_E1E8,
        focus_ring: 0x006C_A8FF,
        ..BASE
    },
    Palette {
        name: "Wallpaper",
        // Backdrop tokens are unused when a wallpaper image is loaded, but they
        // are the fallback when the user selects this theme with no image set.
        backdrop_top: 0x000A_0F19,
        backdrop_bottom: 0x0014_1B2A,
        backdrop_band: 0x001E_2A3F,
        // Surfaces run darker so panels stay readable over an arbitrary photo.
        surface: 0x000B_0E17,
        surface_raised: 0x0014_1927,
        surface_sunken: 0x0007_0910,
        ..BASE
    },
];

/// Look up a theme by persisted id, wrapping out-of-range values rather than
/// panicking, so a stale settings file cannot crash startup.
pub fn palette(theme: u8) -> &'static Palette {
    &THEMES[(theme % THEME_COUNT) as usize]
}

/// Display name for a theme id, for menus and the theme picker.
pub fn theme_label(theme: u8) -> &'static str {
    palette(theme).name
}

/// Whether this theme paints a user wallpaper behind the dashboard.
pub fn theme_uses_wallpaper(theme: u8) -> bool {
    theme % THEME_COUNT == THEME_WALLPAPER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_a_name() {
        for theme in 0..THEME_COUNT {
            assert!(!palette(theme).name.is_empty(), "theme {theme} is unnamed");
        }
    }

    #[test]
    fn palette_lookup_wraps_instead_of_panicking() {
        assert_eq!(palette(0).name, palette(THEME_COUNT).name);
        assert_eq!(palette(1).name, palette(THEME_COUNT + 1).name);
        assert_eq!(theme_label(255), palette(255 % THEME_COUNT).name);
    }

    #[test]
    fn only_the_wallpaper_theme_reports_wallpaper_use() {
        for theme in 0..THEME_COUNT {
            assert_eq!(theme_uses_wallpaper(theme), theme == THEME_WALLPAPER);
        }
    }

    #[test]
    fn colors_leave_the_high_byte_clear() {
        // The surface expects 0x00RRGGBB; a stray alpha byte would survive the
        // present-time mask on some platforms and shift the visible color.
        for theme in 0..THEME_COUNT {
            let p = palette(theme);
            for (label, color) in [
                ("backdrop_top", p.backdrop_top),
                ("backdrop_bottom", p.backdrop_bottom),
                ("surface", p.surface),
                ("chrome", p.chrome),
                ("ink", p.ink),
                ("accent", p.accent),
                ("success", p.success),
                ("warning", p.warning),
                ("danger", p.danger),
            ] {
                assert_eq!(
                    color & 0xFF00_0000,
                    0,
                    "{} token {label} has a non-zero high byte",
                    p.name
                );
            }
        }
    }
}

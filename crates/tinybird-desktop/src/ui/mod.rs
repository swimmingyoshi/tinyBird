//! The tinyBird desktop UI layer.
//!
//! Two shells live here and they are deliberately separate:
//!
//! - **Classic chrome** ([`menubar`], [`widget`]) — the conventional emulator
//!   shell: menu bar, status bar, and mouse-driven dialogs. Optional; see
//!   [`ChromeMode`].
//! - **Stream dashboard** (still in `crate::overlay` pending phase 6) — the
//!   bespoke, pixel-art addon dashboard.
//!
//! Everything rasterises into the same softbuffer `u32` buffer through
//! [`Canvas`], and every color comes from a [`theme::Palette`].
//!
//! See `docs/UI_UX_PLAN.md` for the full design and work sequencing.

// The widget toolkit is deliberately complete ahead of its consumers: the
// modal dialogs in phase 5 of `docs/UI_UX_PLAN.md` need `list`, `slider`,
// `tab_strip`, and the `Canvas` helpers that nothing calls yet. Keeping them
// here with tests is better than dribbling them in one dialog at a time.
#![allow(dead_code)]

pub mod canvas;
pub mod font;
pub mod input;
pub mod menubar;
pub mod theme;
pub mod widget;

pub use canvas::{Align, Canvas, Rect};

/// How much of the classic shell to draw.
///
/// The dashboard is a product feature, not legacy, so the classic chrome has to
/// be switchable all the way off for stream and fullscreen use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeMode {
    /// Menu bar and status bar are always visible.
    #[default]
    Always,
    /// Hidden until the pointer approaches the top edge or `Alt`/`F10` is hit.
    Auto,
    /// Never drawn.
    Off,
}

impl ChromeMode {
    pub const ALL: [ChromeMode; 3] = [ChromeMode::Always, ChromeMode::Auto, ChromeMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            ChromeMode::Always => "Always",
            ChromeMode::Auto => "Auto-hide",
            ChromeMode::Off => "Off",
        }
    }

    /// Cycle to the next mode, for a single-key toggle.
    pub fn next(self) -> Self {
        match self {
            ChromeMode::Always => ChromeMode::Auto,
            ChromeMode::Auto => ChromeMode::Off,
            ChromeMode::Off => ChromeMode::Always,
        }
    }
}

/// Height of the menu bar strip in pixels.
pub const MENU_BAR_HEIGHT: i32 = 22;

/// Height of the status bar strip in pixels.
pub const STATUS_BAR_HEIGHT: i32 = 18;

/// How close to the top edge the pointer must come to reveal auto-hidden chrome.
pub const CHROME_REVEAL_BAND: i32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_mode_cycles_through_every_variant() {
        let mut seen = Vec::new();
        let mut mode = ChromeMode::Always;
        for _ in 0..ChromeMode::ALL.len() {
            seen.push(mode);
            mode = mode.next();
        }
        assert_eq!(mode, ChromeMode::Always, "cycle must return to the start");
        for variant in ChromeMode::ALL {
            assert!(seen.contains(&variant), "{variant:?} was never reached");
        }
    }

    #[test]
    fn chrome_mode_labels_are_distinct() {
        let labels: Vec<_> = ChromeMode::ALL.iter().map(|m| m.label()).collect();
        for (index, label) in labels.iter().enumerate() {
            assert!(!label.is_empty());
            assert!(
                !labels[index + 1..].contains(label),
                "duplicate label {label}"
            );
        }
    }
}

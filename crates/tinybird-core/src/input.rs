//! GBA Input Handler
//!
//! Handles the GBA's 10-button input system through two I/O registers:
//! - KEYINPUT (0x04000130): Read-only register reflecting current button state
//! - KEYCNT (0x04000132): Key interrupt control register
//!
//! The GBA uses inverted logic: a bit value of 0 means the button is pressed,
//! and 1 means it is released.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

bitflags! {
    /// GBA button flags, matching the bit layout of the KEYINPUT register.
    ///
    /// Each button corresponds to a single bit (0-9). In KEYINPUT, a cleared
    /// bit means pressed and a set bit means released.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct GbaButton: u16 {
        /// A button (bit 0)
        const A      = 1 << 0;
        /// B button (bit 1)
        const B      = 1 << 1;
        /// Select button (bit 2)
        const SELECT = 1 << 2;
        /// Start button (bit 3)
        const START  = 1 << 3;
        /// D-pad Right (bit 4)
        const RIGHT  = 1 << 4;
        /// D-pad Left (bit 5)
        const LEFT   = 1 << 5;
        /// D-pad Up (bit 6)
        const UP     = 1 << 6;
        /// D-pad Down (bit 7)
        const DOWN   = 1 << 7;
        /// R shoulder button (bit 8)
        const R      = 1 << 8;
        /// L shoulder button (bit 9)
        const L      = 1 << 9;
    }
}

/// Mask covering all valid button bits (bits 0-9).
const BUTTON_MASK: u16 = 0x03FF;

/// Bit 14 of KEYCNT: IRQ enable flag.
const KEYCNT_IRQ_ENABLE: u16 = 1 << 14;

/// Bit 15 of KEYCNT: AND/OR condition select (0 = OR, 1 = AND).
const KEYCNT_AND_CONDITION: u16 = 1 << 15;

/// GBA input state manager.
///
/// Tracks the current button state and key interrupt control settings.
/// The KEYINPUT register uses inverted logic where 0 = pressed, 1 = released.
#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    /// Current button state (bits are 0 when pressed, 1 when released - GBA convention)
    keyinput: u16,
    /// Key interrupt control register (KEYCNT at 0x04000132)
    keycnt: u16,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// Create a new input handler with all buttons released.
    ///
    /// All bits in KEYINPUT are set to 1 (0x03FF), indicating no buttons pressed.
    pub fn new() -> Self {
        Self {
            keyinput: BUTTON_MASK,
            keycnt: 0,
        }
    }

    /// Press a button (clear its bit in KEYINPUT).
    ///
    /// Multiple buttons can be pressed by OR-ing `GbaButton` flags together.
    pub fn press(&mut self, button: GbaButton) {
        self.keyinput &= !button.bits();
    }

    /// Release a button (set its bit in KEYINPUT).
    ///
    /// Multiple buttons can be released by OR-ing `GbaButton` flags together.
    pub fn release(&mut self, button: GbaButton) {
        self.keyinput |= button.bits() & BUTTON_MASK;
    }

    /// Set the complete button state at once.
    ///
    /// The `pressed` parameter specifies which buttons are currently held down.
    /// Bits corresponding to pressed buttons will be cleared in KEYINPUT; all
    /// others will be set to 1.
    pub fn set_buttons(&mut self, pressed: GbaButton) {
        self.keyinput = (!pressed.bits()) & BUTTON_MASK;
    }

    /// Read the KEYINPUT register (0x04000130).
    ///
    /// Returns a 16-bit value where bits 0-9 represent button state
    /// (0 = pressed, 1 = released). Bits 10-15 are unused and read as 0.
    pub fn read_keyinput(&self) -> u16 {
        self.keyinput
    }

    /// Read the KEYCNT register (0x04000132).
    pub fn read_keycnt(&self) -> u16 {
        self.keycnt
    }

    /// Write to the KEYCNT register (0x04000132).
    ///
    /// - Bits 0-9: Button select mask (which buttons to monitor for IRQ)
    /// - Bit 14: IRQ enable (1 = enable keypad IRQ)
    /// - Bit 15: AND/OR condition (0 = fire if any selected button pressed,
    ///   1 = fire only if all selected buttons pressed)
    pub fn write_keycnt(&mut self, value: u16) {
        self.keycnt = value;
    }

    /// Check whether a keypad IRQ should fire based on the current KEYCNT
    /// settings and button state.
    ///
    /// Returns `true` if:
    /// 1. IRQ is enabled (bit 14 of KEYCNT), AND
    /// 2. The condition is met:
    ///    - OR mode (bit 15 = 0): any selected button is pressed
    ///    - AND mode (bit 15 = 1): all selected buttons are pressed
    pub fn check_irq(&self) -> bool {
        // IRQ must be enabled
        if self.keycnt & KEYCNT_IRQ_ENABLE == 0 {
            return false;
        }

        let selected = self.keycnt & BUTTON_MASK;
        if selected == 0 {
            return false;
        }

        // Pressed buttons have bit = 0 in keyinput, so invert to get a mask
        // where bit = 1 means pressed.
        let pressed = (!self.keyinput) & BUTTON_MASK;

        if self.keycnt & KEYCNT_AND_CONDITION != 0 {
            // AND mode: all selected buttons must be pressed
            (pressed & selected) == selected
        } else {
            // OR mode: any selected button must be pressed
            (pressed & selected) != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_all_released() {
        let input = Input::new();
        assert_eq!(input.read_keyinput(), 0x03FF);
    }

    #[test]
    fn test_press_single_button() {
        let mut input = Input::new();
        input.press(GbaButton::A);
        assert_eq!(input.read_keyinput() & GbaButton::A.bits(), 0);
        // Other buttons still released
        assert_ne!(input.read_keyinput() & GbaButton::B.bits(), 0);
    }

    #[test]
    fn test_press_multiple_buttons() {
        let mut input = Input::new();
        input.press(GbaButton::A | GbaButton::B);
        assert_eq!(input.read_keyinput() & GbaButton::A.bits(), 0);
        assert_eq!(input.read_keyinput() & GbaButton::B.bits(), 0);
        assert_ne!(input.read_keyinput() & GbaButton::START.bits(), 0);
    }

    #[test]
    fn test_release_button() {
        let mut input = Input::new();
        input.press(GbaButton::A);
        assert_eq!(input.read_keyinput() & GbaButton::A.bits(), 0);
        input.release(GbaButton::A);
        assert_ne!(input.read_keyinput() & GbaButton::A.bits(), 0);
    }

    #[test]
    fn test_set_buttons() {
        let mut input = Input::new();
        input.set_buttons(GbaButton::START | GbaButton::SELECT);
        // START and SELECT pressed (bit = 0)
        assert_eq!(input.read_keyinput() & GbaButton::START.bits(), 0);
        assert_eq!(input.read_keyinput() & GbaButton::SELECT.bits(), 0);
        // All others released (bit = 1)
        assert_ne!(input.read_keyinput() & GbaButton::A.bits(), 0);
        assert_ne!(input.read_keyinput() & GbaButton::L.bits(), 0);
    }

    #[test]
    fn test_set_buttons_replaces_previous() {
        let mut input = Input::new();
        input.set_buttons(GbaButton::A);
        input.set_buttons(GbaButton::B);
        // A should now be released, B pressed
        assert_ne!(input.read_keyinput() & GbaButton::A.bits(), 0);
        assert_eq!(input.read_keyinput() & GbaButton::B.bits(), 0);
    }

    #[test]
    fn test_keycnt_read_write() {
        let mut input = Input::new();
        assert_eq!(input.read_keycnt(), 0);
        input.write_keycnt(0xC003);
        assert_eq!(input.read_keycnt(), 0xC003);
    }

    #[test]
    fn test_irq_disabled() {
        let mut input = Input::new();
        // Select A button but don't enable IRQ
        input.write_keycnt(GbaButton::A.bits());
        input.press(GbaButton::A);
        assert!(!input.check_irq());
    }

    #[test]
    fn test_irq_or_mode() {
        let mut input = Input::new();
        // Enable IRQ, OR mode, select A and B
        input.write_keycnt(KEYCNT_IRQ_ENABLE | GbaButton::A.bits() | GbaButton::B.bits());
        // No buttons pressed
        assert!(!input.check_irq());
        // Press only A -> should fire (OR mode)
        input.press(GbaButton::A);
        assert!(input.check_irq());
    }

    #[test]
    fn test_irq_and_mode() {
        let mut input = Input::new();
        // Enable IRQ, AND mode, select A and B
        input.write_keycnt(
            KEYCNT_IRQ_ENABLE | KEYCNT_AND_CONDITION | GbaButton::A.bits() | GbaButton::B.bits(),
        );
        // Press only A -> should NOT fire (AND mode)
        input.press(GbaButton::A);
        assert!(!input.check_irq());
        // Press A and B -> should fire
        input.press(GbaButton::B);
        assert!(input.check_irq());
    }

    #[test]
    fn test_irq_no_buttons_selected() {
        let mut input = Input::new();
        // Enable IRQ but select no buttons
        input.write_keycnt(KEYCNT_IRQ_ENABLE);
        input.press(GbaButton::A);
        assert!(!input.check_irq());
    }

    #[test]
    fn test_default_trait() {
        let input = Input::default();
        assert_eq!(input.read_keyinput(), 0x03FF);
    }

    #[test]
    fn test_all_buttons_pressed() {
        let mut input = Input::new();
        input.set_buttons(GbaButton::all());
        assert_eq!(input.read_keyinput(), 0x0000);
    }

    #[test]
    fn test_all_buttons_released() {
        let mut input = Input::new();
        input.set_buttons(GbaButton::all());
        input.set_buttons(GbaButton::empty());
        assert_eq!(input.read_keyinput(), 0x03FF);
    }
}

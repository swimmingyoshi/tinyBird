//! Pointer and keyboard state for the immediate-mode UI.
//!
//! Before this module the frontend handled no mouse events at all — every
//! feature was bound to a function key, and `F1`–`F12` were exhausted. Here
//! winit events accumulate into a [`UiInput`] which widgets query during the
//! draw pass; [`UiInput::end_frame`] then clears the edge-triggered fields.
//!
//! Click semantics match what people expect from a desktop app: a click counts
//! only if the press *and* the release both land inside the same rectangle, so
//! pressing a button and dragging away cancels it.

use super::canvas::Rect;

/// Mouse buttons the UI cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        match self {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }
}

/// Keyboard events the UI consumes, normalised away from winit's key model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Space,
    Function(u8),
    Char(char),
}

/// Modifier keys held at the time an event arrived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub fn none(&self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }
}

/// One key press delivered to the UI this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: UiKey,
    pub modifiers: Modifiers,
}

/// Where the pointer is and what it is doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct PointerState {
    pub position: Option<(i32, i32)>,
    pub delta: (i32, i32),
}

/// Accumulated input for one UI frame.
///
/// The owning `App` feeds this from `window_event`, widgets read it during the
/// draw pass, and `end_frame` resets everything that is edge-triggered.
#[derive(Debug, Default)]
pub struct UiInput {
    pointer: Option<(i32, i32)>,
    previous_pointer: Option<(i32, i32)>,
    down: [bool; MouseButton::COUNT],
    pressed: [bool; MouseButton::COUNT],
    released: [bool; MouseButton::COUNT],
    /// Where each button went down, so a drag-away cancels the click.
    press_origin: [Option<(i32, i32)>; MouseButton::COUNT],
    wheel: f32,
    keys: Vec<KeyStroke>,
    modifiers: Modifiers,
    /// Set when the UI wants this frame's input kept away from the emulator.
    capturing: bool,
}

impl UiInput {
    pub fn new() -> Self {
        Self::default()
    }

    // --- event ingestion -------------------------------------------------

    pub fn set_pointer(&mut self, x: i32, y: i32) {
        self.pointer = Some((x, y));
    }

    /// The pointer left the window; hover states must clear.
    pub fn clear_pointer(&mut self) {
        self.pointer = None;
    }

    pub fn set_button(&mut self, button: MouseButton, down: bool) {
        let index = button.index();
        if down && !self.down[index] {
            self.pressed[index] = true;
            self.press_origin[index] = self.pointer;
        } else if !down && self.down[index] {
            self.released[index] = true;
        }
        self.down[index] = down;
    }

    pub fn add_wheel(&mut self, delta: f32) {
        self.wheel += delta;
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    pub fn push_key(&mut self, key: UiKey) {
        let modifiers = self.modifiers;
        self.keys.push(KeyStroke { key, modifiers });
    }

    // --- queries ---------------------------------------------------------

    pub fn pointer(&self) -> Option<(i32, i32)> {
        self.pointer
    }

    pub fn pointer_state(&self) -> PointerState {
        let delta = match (self.pointer, self.previous_pointer) {
            (Some((x, y)), Some((px, py))) => (x - px, y - py),
            _ => (0, 0),
        };
        PointerState {
            position: self.pointer,
            delta,
        }
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn is_down(&self, button: MouseButton) -> bool {
        self.down[button.index()]
    }

    pub fn just_pressed(&self, button: MouseButton) -> bool {
        self.pressed[button.index()]
    }

    pub fn just_released(&self, button: MouseButton) -> bool {
        self.released[button.index()]
    }

    pub fn wheel(&self) -> f32 {
        self.wheel
    }

    pub fn keys(&self) -> &[KeyStroke] {
        &self.keys
    }

    /// Whether the pointer is currently inside `rect`.
    pub fn hovering(&self, rect: Rect) -> bool {
        self.pointer.is_some_and(|(x, y)| rect.contains(x, y))
    }

    /// Whether `button` went down inside `rect` this frame.
    pub fn pressed_in(&self, rect: Rect, button: MouseButton) -> bool {
        self.just_pressed(button)
            && self.press_origin[button.index()].is_some_and(|(x, y)| rect.contains(x, y))
    }

    /// A completed click: press and release both inside `rect`.
    pub fn clicked(&self, rect: Rect) -> bool {
        self.clicked_with(rect, MouseButton::Left)
    }

    pub fn clicked_with(&self, rect: Rect, button: MouseButton) -> bool {
        let index = button.index();
        if !self.released[index] {
            return false;
        }
        let origin_inside = self.press_origin[index].is_some_and(|(x, y)| rect.contains(x, y));
        origin_inside && self.hovering(rect)
    }

    /// True while `button` is held after having gone down inside `rect`, which
    /// is what a slider needs to keep tracking once the pointer leaves the
    /// track.
    pub fn dragging_from(&self, rect: Rect, button: MouseButton) -> bool {
        let index = button.index();
        self.down[index] && self.press_origin[index].is_some_and(|(x, y)| rect.contains(x, y))
    }

    /// Consume the first queued key matching `predicate`, if any.
    pub fn take_key(&mut self, predicate: impl Fn(&KeyStroke) -> bool) -> Option<KeyStroke> {
        let position = self.keys.iter().position(&predicate)?;
        Some(self.keys.remove(position))
    }

    // --- capture ---------------------------------------------------------

    /// Mark this frame's input as owned by the UI. The emulator input path
    /// checks this so an open menu never leaks key presses into the game.
    pub fn capture(&mut self) {
        self.capturing = true;
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    // --- frame lifecycle -------------------------------------------------

    /// Clear edge-triggered state. Call once after the UI has been drawn and
    /// every widget has had a chance to read this frame's input.
    pub fn end_frame(&mut self) {
        self.previous_pointer = self.pointer;
        self.pressed = [false; MouseButton::COUNT];
        self.released = [false; MouseButton::COUNT];
        for (index, origin) in self.press_origin.iter_mut().enumerate() {
            if !self.down[index] {
                *origin = None;
            }
        }
        self.wheel = 0.0;
        self.keys.clear();
        self.capturing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX_RECT: Rect = Rect::new(10, 10, 20, 20);

    fn press_and_release(input: &mut UiInput, press: (i32, i32), release: (i32, i32)) {
        input.set_pointer(press.0, press.1);
        input.set_button(MouseButton::Left, true);
        input.end_frame();
        input.set_pointer(release.0, release.1);
        input.set_button(MouseButton::Left, false);
    }

    #[test]
    fn click_requires_press_and_release_in_the_same_rect() {
        let mut input = UiInput::new();
        press_and_release(&mut input, (15, 15), (15, 15));
        assert!(input.clicked(BOX_RECT));
    }

    #[test]
    fn dragging_off_a_button_cancels_the_click() {
        let mut input = UiInput::new();
        press_and_release(&mut input, (15, 15), (100, 100));
        assert!(!input.clicked(BOX_RECT));
    }

    #[test]
    fn releasing_over_a_button_pressed_elsewhere_does_not_click() {
        let mut input = UiInput::new();
        press_and_release(&mut input, (100, 100), (15, 15));
        assert!(!input.clicked(BOX_RECT));
    }

    #[test]
    fn drag_tracking_survives_the_pointer_leaving_the_rect() {
        let mut input = UiInput::new();
        input.set_pointer(15, 15);
        input.set_button(MouseButton::Left, true);
        input.end_frame();
        input.set_pointer(500, 500);
        assert!(
            input.dragging_from(BOX_RECT, MouseButton::Left),
            "a slider must keep tracking once the pointer leaves the track"
        );
    }

    #[test]
    fn end_frame_clears_edges_but_keeps_held_state() {
        let mut input = UiInput::new();
        input.set_pointer(15, 15);
        input.set_button(MouseButton::Left, true);
        input.add_wheel(1.0);
        input.push_key(UiKey::Enter);
        assert!(input.just_pressed(MouseButton::Left));

        input.end_frame();

        assert!(!input.just_pressed(MouseButton::Left));
        assert!(input.is_down(MouseButton::Left), "held state must persist");
        assert_eq!(input.wheel(), 0.0);
        assert!(input.keys().is_empty());
    }

    #[test]
    fn hover_clears_when_the_pointer_leaves_the_window() {
        let mut input = UiInput::new();
        input.set_pointer(15, 15);
        assert!(input.hovering(BOX_RECT));
        input.clear_pointer();
        assert!(!input.hovering(BOX_RECT));
    }

    #[test]
    fn keys_carry_the_modifiers_active_when_they_arrived() {
        let mut input = UiInput::new();
        input.set_modifiers(Modifiers {
            ctrl: true,
            ..Default::default()
        });
        input.push_key(UiKey::Char('o'));
        let stroke = input.keys()[0];
        assert!(stroke.modifiers.ctrl);
        assert!(!stroke.modifiers.alt);
    }

    #[test]
    fn take_key_removes_only_the_first_match() {
        let mut input = UiInput::new();
        input.push_key(UiKey::Down);
        input.push_key(UiKey::Enter);
        input.push_key(UiKey::Down);

        let taken = input.take_key(|stroke| stroke.key == UiKey::Down);
        assert_eq!(taken.map(|s| s.key), Some(UiKey::Down));
        assert_eq!(input.keys().len(), 2);
        assert_eq!(input.keys()[0].key, UiKey::Enter);
    }

    #[test]
    fn capture_resets_every_frame() {
        let mut input = UiInput::new();
        input.capture();
        assert!(input.is_capturing());
        input.end_frame();
        assert!(!input.is_capturing());
    }
}

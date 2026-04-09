//! Desktop input mappings for keyboard and gamepad.

use gilrs::{Axis, Button as GamepadButton};
use tinybird_core::GbaButton;
use winit::keyboard::{KeyCode, PhysicalKey};

const AXIS_DEADZONE: f32 = 0.5;

/// Map a physical keyboard key to a GBA button.
pub fn map_physical_key(key: &PhysicalKey) -> Option<GbaButton> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };

    match code {
        KeyCode::KeyZ | KeyCode::KeyJ => Some(GbaButton::A),
        KeyCode::KeyX | KeyCode::KeyK => Some(GbaButton::B),
        KeyCode::KeyA | KeyCode::KeyQ => Some(GbaButton::L),
        KeyCode::KeyS | KeyCode::KeyW => Some(GbaButton::R),
        KeyCode::Enter => Some(GbaButton::START),
        KeyCode::Space | KeyCode::ShiftRight | KeyCode::Backspace => Some(GbaButton::SELECT),
        KeyCode::ArrowUp => Some(GbaButton::UP),
        KeyCode::ArrowDown => Some(GbaButton::DOWN),
        KeyCode::ArrowLeft => Some(GbaButton::LEFT),
        KeyCode::ArrowRight => Some(GbaButton::RIGHT),
        _ => None,
    }
}

/// Map a gamepad button to a GBA button.
pub fn map_gamepad_button(button: GamepadButton) -> Option<GbaButton> {
    match button {
        GamepadButton::South => Some(GbaButton::A),
        GamepadButton::East => Some(GbaButton::B),
        GamepadButton::Select => Some(GbaButton::SELECT),
        GamepadButton::Start => Some(GbaButton::START),
        GamepadButton::DPadUp => Some(GbaButton::UP),
        GamepadButton::DPadDown => Some(GbaButton::DOWN),
        GamepadButton::DPadLeft => Some(GbaButton::LEFT),
        GamepadButton::DPadRight => Some(GbaButton::RIGHT),
        GamepadButton::LeftTrigger | GamepadButton::LeftTrigger2 => Some(GbaButton::L),
        GamepadButton::RightTrigger | GamepadButton::RightTrigger2 => Some(GbaButton::R),
        _ => None,
    }
}

/// Return the stick axes we treat as the primary directional input.
pub fn is_direction_axis(axis: Axis) -> bool {
    matches!(axis, Axis::LeftStickX | Axis::LeftStickY | Axis::DPadX | Axis::DPadY)
}

/// Convert stick and d-pad axes into directional button presses.
pub fn buttons_from_axes(left_x: f32, left_y: f32, dpad_x: f32, dpad_y: f32) -> GbaButton {
    let x = strongest_axis(left_x, dpad_x);
    let y = strongest_axis(left_y, dpad_y);

    let mut buttons = GbaButton::empty();
    if x <= -AXIS_DEADZONE {
        buttons |= GbaButton::LEFT;
    } else if x >= AXIS_DEADZONE {
        buttons |= GbaButton::RIGHT;
    }

    if y <= -AXIS_DEADZONE {
        buttons |= GbaButton::UP;
    } else if y >= AXIS_DEADZONE {
        buttons |= GbaButton::DOWN;
    }

    buttons
}

fn strongest_axis(a: f32, b: f32) -> f32 {
    if a.abs() >= b.abs() { a } else { b }
}

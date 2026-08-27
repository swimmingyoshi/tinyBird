//! tinyBird Core - GBA Emulator Core Library
//!
//! This crate provides the core emulation logic for the tinyBird Game Boy Advance emulator.
//! It includes implementations of:
//! - ARM7TDMI CPU core (ARM and Thumb instruction sets)
//! - Memory bus with all GBA memory regions
//! - Event scheduler for timed events
//! - Top-level GBA emulator struct

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]

pub mod apu;
pub mod bios;
pub mod bus;
pub mod cpu;
pub mod debug;
pub mod dma;
pub mod eeprom;
pub mod gba;
pub mod input;
pub mod memory_map;
pub mod ppu;
pub mod scheduler;
pub mod sio;
pub mod timers;

// Re-exports for convenient access
pub use apu::Apu;
pub use bios::Bios;
pub use bus::{Bus, BusError, BusResult, SimpleBus};
pub use cpu::{Cpu, CpuMode, Instruction, InstructionCategory, Pipeline, Registers};
pub use dma::DmaController;
pub use gba::{
    EmulationSpeed, Gba, GbaState, GbaStatus, CLOCK_SPEED, CYCLES_PER_FRAME, SCREEN_HEIGHT,
    SCREEN_WIDTH,
};
pub use input::{GbaButton, Input};
pub use memory_map::*;
pub use ppu::{
    Background, Color, DisplayControl, DisplayStatus, Effects, Framebuffer, Ppu, Sprite, Window,
};
pub use scheduler::{Event, EventType, Scheduler};
pub use timers::TimerController;

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the emulator with default settings
pub fn init() -> Gba {
    Gba::new()
}

/// Create an emulator with a ROM
pub fn with_rom(rom: Vec<u8>) -> Gba {
    Gba::with_rom(rom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        let gba = init();
        assert_eq!(gba.state, GbaState::Stopped);
    }

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }
}

//! Host-independent, synchronous automation above the emulator core.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tinybird_core::{Gba, GbaButton};

/// A named, little-endian memory observation. Names may be semantic dotted paths.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryField {
    pub name: String,
    pub address: u32,
    /// Width in bytes: 1, 2, or 4. Unaligned addresses are supported.
    pub width: u8,
}

/// Versioned Workshop export. Game matching uses the cartridge header.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationConfig {
    pub schema_version: u32,
    pub game: ObservationGame,
    pub fields: Vec<MemoryField>,
}

/// Exact game code (including region) and revision this map was discovered on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationGame {
    pub code: String,
    pub revision: u8,
}

fn semantic_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.split('.').all(|part| {
            let mut bytes = part.bytes();
            matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
                && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

/// One JSON command. A step holds the supplied buttons for the entire action.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Reset,
    Step { frames: u32, buttons: Vec<String> },
    GetState,
    GetFrame,
    GetMemory { address: u32, length: u32 },
    SetObservations { fields: Vec<MemoryField> },
    ConfigureObservations { config: ObservationConfig },
    SaveState { slot: u8 },
    LoadState { slot: u8 },
}

struct Checkpoint {
    machine: Vec<u8>,
    // Core states intentionally omit rendered pixels.
    pixels: Vec<u8>,
}

/// One isolated emulator with a fixed initial state and bounded checkpoints.
pub struct Runtime {
    gba: Gba,
    initial: Checkpoint,
    slots: BTreeMap<u8, Checkpoint>,
    fields: Vec<MemoryField>,
    pixels: Vec<u8>,
}

impl Runtime {
    /// Capture the episode baseline after loading ROM, optional BIOS/save, and clock.
    pub fn new(mut gba: Gba) -> Result<Self, String> {
        gba.start();
        gba.set_audio_enabled(false);
        gba.input.set_buttons(GbaButton::empty());
        let pixels = gba
            .ppu
            .get_framebuffer()
            .get_pixels_rgb888()
            .into_iter()
            .flat_map(|(r, g, b)| [r, g, b])
            .collect::<Vec<u8>>();
        let initial = Checkpoint {
            machine: gba.save_state_bytes().map_err(|e| e.to_string())?,
            pixels: pixels.clone(),
        };
        Ok(Self {
            gba,
            initial,
            slots: BTreeMap::new(),
            fields: Vec::new(),
            pixels,
        })
    }

    fn observe(&self) -> Value {
        let memory: BTreeMap<_, _> = self
            .fields
            .iter()
            .map(|field| {
                let value = (0..field.width).fold(0u32, |value, offset| {
                    value
                        | ((self.gba.read_u8(field.address + u32::from(offset)) as u32)
                            << (8 * offset))
                });
                (field.name.clone(), value)
            })
            .collect();
        let status = self.gba.status();
        json!({"frame": status.frame_count, "cycles": status.total_cycles,
            "pc": status.pc, "memory": memory})
    }

    /// Execute one command; invalid arguments are rejected before mutation.
    pub fn execute(&mut self, command: Command) -> Result<Value, String> {
        match command {
            Command::ConfigureObservations { config } => {
                if config.schema_version != 1 {
                    return Err("unsupported observation schema version (expected 1)".into());
                }
                if config.game.code.len() != 4
                    || !config
                        .game
                        .code
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                {
                    return Err(
                        "game code must contain four uppercase ASCII letters or digits".into(),
                    );
                }
                if config
                    .game
                    .code
                    .bytes()
                    .enumerate()
                    .any(|(i, b)| self.gba.read_u8(0x080000ac + i as u32) != b)
                    || self.gba.read_u8(0x080000bc) != config.game.revision
                {
                    return Err(format!("observation config requires game {} revision {}; loaded cartridge does not match",
                        config.game.code, config.game.revision));
                }
                if config.fields.is_empty() || config.fields.iter().any(|f| !semantic_name(&f.name))
                {
                    return Err("config needs at least one field; names must be dot-separated identifiers, at most 128 characters".into());
                }
                // Reuse range/width/uniqueness validation; replacement is atomic.
                self.execute(Command::SetObservations {
                    fields: config.fields,
                })
            }
            Command::GetState => Ok(self.observe()),
            Command::GetFrame => Ok(json!({"width": 240, "height": 160,
                "format": "rgb888", "pixels": self.pixels})),
            Command::GetMemory { address, length } => {
                if length > 65536 || (length > 0 && address.checked_add(length - 1).is_none()) {
                    return Err("memory range must fit u32 and contain at most 65536 bytes".into());
                }
                Ok(json!((0..length)
                    .map(|i| self.gba.read_u8(address + i))
                    .collect::<Vec<_>>()))
            }
            Command::SetObservations { fields } => {
                let mut names = std::collections::BTreeSet::new();
                if fields.len() > 256
                    || fields.iter().any(|f| {
                        f.name.is_empty()
                            || f.name.len() > 128
                            || !names.insert(&f.name)
                            || !matches!(f.width, 1 | 2 | 4)
                            || f.address.checked_add(u32::from(f.width) - 1).is_none()
                    })
                {
                    return Err("observations need unique names, widths 1/2/4, valid ranges; maximum 256 fields".into());
                }
                self.fields = fields;
                Ok(self.observe())
            }
            Command::Step { frames, buttons } => {
                if !(1..=600).contains(&frames) {
                    return Err("frames must be between 1 and 600".into());
                }
                let mut pressed = GbaButton::empty();
                for name in buttons {
                    pressed |= match name.to_ascii_uppercase().as_str() {
                        "A" => GbaButton::A,
                        "B" => GbaButton::B,
                        "SELECT" => GbaButton::SELECT,
                        "START" => GbaButton::START,
                        "RIGHT" => GbaButton::RIGHT,
                        "LEFT" => GbaButton::LEFT,
                        "UP" => GbaButton::UP,
                        "DOWN" => GbaButton::DOWN,
                        "R" => GbaButton::R,
                        "L" => GbaButton::L,
                        _ => return Err(format!("unknown button: {name}")),
                    };
                }
                self.gba.input.set_buttons(pressed);
                let mut failed = false;
                for _ in 0..frames {
                    if self.gba.run_frame_with_budget(2_000_000).is_none() {
                        failed = true;
                        break;
                    }
                }
                self.gba.input.set_buttons(GbaButton::empty());
                self.pixels = self
                    .gba
                    .ppu
                    .get_framebuffer()
                    .get_pixels_rgb888()
                    .into_iter()
                    .flat_map(|(r, g, b)| [r, g, b])
                    .collect::<Vec<u8>>();
                if failed {
                    return Err("frame did not complete (execution budget or pending link); state advanced, reset to recover".into());
                }
                Ok(self.observe())
            }
            Command::SaveState { slot } => {
                if slot >= 16 {
                    return Err("slot must be between 0 and 15".into());
                }
                self.slots.insert(
                    slot,
                    Checkpoint {
                        machine: self.gba.save_state_bytes().map_err(|e| e.to_string())?,
                        pixels: self.pixels.clone(),
                    },
                );
                Ok(self.observe())
            }
            Command::LoadState { slot } => {
                let saved = self.slots.get(&slot).ok_or("empty checkpoint slot")?;
                self.gba
                    .load_state_bytes(&saved.machine)
                    .map_err(|e| e.to_string())?;
                self.pixels = saved.pixels.clone();
                Ok(self.observe())
            }
            Command::Reset => {
                self.gba
                    .load_state_bytes(&self.initial.machine)
                    .map_err(|e| e.to_string())?;
                self.pixels = self.initial.pixels.clone();
                Ok(self.observe())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn runtime() -> Runtime {
        // ARM branch-to-self: no commercial fixtures needed.
        let mut rom = vec![0; 192];
        rom[..4].copy_from_slice(&0xeafffffeu32.to_le_bytes());
        rom[0xac..0xb0].copy_from_slice(b"TBST");
        rom[0xbc] = 2;
        let mut gba = Gba::with_rom(rom);
        gba.start();
        gba.write_u8(0x02000001, 0x34);
        gba.write_u8(0x02000002, 0x12);
        Runtime::new(gba).unwrap()
    }
    #[test]
    fn replay_and_reset_restore_machine_and_pixels() {
        let mut rt = runtime();
        let initial = rt.execute(Command::GetState).unwrap();
        rt.execute(Command::SaveState { slot: 0 }).unwrap();
        let action = || Command::Step {
            frames: 3,
            buttons: vec!["A".into()],
        };
        let first = rt.execute(action()).unwrap();
        let state = rt.gba.save_state_bytes().unwrap();
        let pixels = rt.pixels.clone();
        rt.execute(Command::LoadState { slot: 0 }).unwrap();
        assert_eq!(rt.execute(action()).unwrap(), first);
        assert_eq!(rt.gba.save_state_bytes().unwrap(), state);
        assert_eq!(rt.pixels, pixels);
        assert_eq!(rt.gba.input.read_keyinput(), 0x3ff);
        assert_eq!(rt.execute(Command::Reset).unwrap(), initial);
    }
    #[test]
    fn observations_and_rejected_actions() {
        let mut rt = runtime();
        let obs = rt
            .execute(Command::SetObservations {
                fields: vec![MemoryField {
                    name: "player.hp".into(),
                    address: 0x02000001,
                    width: 2,
                }],
            })
            .unwrap();
        assert_eq!(obs["memory"]["player.hp"], 0x1234);
        let before = rt.gba.save_state_bytes().unwrap();
        assert!(rt
            .execute(Command::Step {
                frames: 1,
                buttons: vec!["oops".into()]
            })
            .is_err());
        assert!(rt
            .execute(Command::GetMemory {
                address: u32::MAX,
                length: 2
            })
            .is_err());
        assert!(rt.execute(Command::LoadState { slot: 0 }).is_err());
        assert_eq!(rt.gba.save_state_bytes().unwrap(), before);
    }

    #[test]
    fn workshop_config_matches_cartridge_and_rejects_atomically() {
        let config: ObservationConfig =
            serde_json::from_str(include_str!("../../../tests/fixtures/observations.json"))
                .unwrap();
        let mut rt = runtime();
        let applied = rt
            .execute(Command::ConfigureObservations {
                config: config.clone(),
            })
            .unwrap();
        assert_eq!(applied["memory"]["player.hp"], 0x1234);
        for broken in 0..7 {
            let mut bad = config.clone();
            match broken {
                0 => bad.game.code = "NOPE".into(),
                1 => bad.game.revision = 0,
                2 => bad.schema_version = 99,
                3 => bad.fields[0].name = "player..hp".into(),
                4 => bad.fields.push(bad.fields[0].clone()),
                5 => bad.fields[0].address = u32::MAX,
                _ => bad.fields[0].width = 0,
            }
            assert!(rt
                .execute(Command::ConfigureObservations { config: bad })
                .is_err());
            assert_eq!(rt.execute(Command::GetState).unwrap(), applied);
        }
        rt.execute(Command::SaveState { slot: 0 }).unwrap();
        rt.execute(Command::Step {
            frames: 2,
            buttons: vec![],
        })
        .unwrap();
        assert_eq!(rt.execute(Command::Reset).unwrap(), applied);
        assert_eq!(rt.execute(Command::LoadState { slot: 0 }).unwrap(), applied);
    }
}

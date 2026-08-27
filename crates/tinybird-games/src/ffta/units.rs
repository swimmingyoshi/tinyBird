//! Final Fantasy Tactics Advance live unit data.
//!
//! # How these addresses were found
//!
//! Using `tinybird-probe` against the US ROM (`AFXE`):
//!
//! 1. Booted to the tutorial snowball battle and captured a save state. The
//!    unit panel on screen read `Ritz  HP 16/16  MP 10/10`.
//! 2. `--find-bytes 100010000a000a00` found exactly one match in EWRAM, at
//!    `0x0200360C` — `16, 16, 10, 10` as consecutive little-endian `u16`s.
//! 3. `--stride 264` confirmed the record size: the next two slots held
//!    `10/10` and `10/10`, matching the three player units in that battle, and
//!    every later slot was zero.
//! 4. Re-checked 2500 frames later; both the array and the name buffer were
//!    unchanged, so these are fixed addresses rather than a transient buffer.
//!
//! # What is deliberately not reported
//!
//! Each record has further `u16` fields after the vitals (`19, 25` for one
//! unit, `21, 26` for another) that are probably attack and defence. They are
//! not exposed, because a plausible guess rendered as a labelled stat is worse
//! than no stat at all. `docs/ADDON_DEVELOPMENT.md` describes how to identify
//! them with `--diff`.

use tinybird_addons::MemoryView;

/// Vitals block of the first unit record. Not the record start — the record
/// begins earlier, but the offset to it has not been established, and anchoring
/// on a verified field is safer than anchoring on a guessed one.
pub const UNIT_VITALS_BASE: u32 = 0x0200_360C;

/// Bytes between consecutive unit records.
pub const UNIT_STRIDE: u32 = 0x108;

/// How many slots to examine. FFTA fields at most six player units; the extra
/// slots are scanned so a larger formation is not silently truncated.
pub const UNIT_SLOTS: usize = 8;

/// Player-entered name for the main character, as `0x80`-escaped pairs.
pub const PLAYER_NAME_ADDR: u32 = 0x0200_1F1C;

/// Longest name the entry screen accepts, in characters.
pub const PLAYER_NAME_MAX_CHARS: usize = 16;

/// Largest HP value treated as real. FFTA units stay well under this; anything
/// above it means the array is being read while it holds something else.
const MAX_PLAUSIBLE_HP: u16 = 999;

/// One unit's live vitals.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FftaUnit {
    /// Zero-based slot in the unit array.
    pub slot: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub mp: u16,
    pub max_mp: u16,
}

impl FftaUnit {
    /// Whether this record looks like a real unit rather than stale memory.
    ///
    /// The array keeps its contents between battles, so without these checks
    /// the panel would show a party that is no longer on the field.
    fn is_plausible(&self) -> bool {
        self.max_hp > 0
            && self.max_hp <= MAX_PLAUSIBLE_HP
            && self.hp <= self.max_hp
            && self.max_mp <= MAX_PLAUSIBLE_HP
            && self.mp <= self.max_mp
    }

    /// `"12/16"`, for a table cell.
    pub fn hp_text(&self) -> String {
        format!("{}/{}", self.hp, self.max_hp)
    }

    pub fn mp_text(&self) -> String {
        format!("{}/{}", self.mp, self.max_mp)
    }

    /// Fraction of HP remaining, for a bar.
    pub fn hp_fraction(&self) -> f32 {
        if self.max_hp == 0 {
            return 0.0;
        }
        (self.hp as f32 / self.max_hp as f32).clamp(0.0, 1.0)
    }
}

/// Read one slot without validating it.
fn read_slot(memory: &dyn MemoryView, slot: usize) -> FftaUnit {
    let base = UNIT_VITALS_BASE + UNIT_STRIDE * slot as u32;
    FftaUnit {
        slot: slot as u8,
        hp: memory.read_u16(base),
        max_hp: memory.read_u16(base + 2),
        mp: memory.read_u16(base + 4),
        max_mp: memory.read_u16(base + 6),
    }
}

/// Read the units currently on the field.
///
/// Stops at the first implausible slot rather than skipping it: the array is
/// filled from the front, so a gap means the end of the formation, and
/// continuing past it would report whatever happened to be left in memory.
pub fn read_units(memory: &dyn MemoryView) -> Vec<FftaUnit> {
    let mut units = Vec::new();
    for slot in 0..UNIT_SLOTS {
        let unit = read_slot(memory, slot);
        if !unit.is_plausible() {
            break;
        }
        units.push(unit);
    }
    units
}

/// Read the player's chosen name for the main character.
///
/// Stored as `0x80 <char>` pairs terminated by a zero word, which is the form
/// FFTA uses for text in RAM — unlike the plain single-byte form in the ROM's
/// static tables.
pub fn read_player_name(memory: &dyn MemoryView) -> String {
    let bytes = memory.read_bytes(PLAYER_NAME_ADDR, PLAYER_NAME_MAX_CHARS * 2);
    super::text::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinybird_addons::SparseMemory;

    /// Build a memory image with `vitals` written into consecutive unit slots.
    fn memory_with_units(vitals: &[(u16, u16, u16, u16)]) -> SparseMemory {
        let mut memory = SparseMemory::new();
        for (slot, (hp, max_hp, mp, max_mp)) in vitals.iter().enumerate() {
            let base = UNIT_VITALS_BASE + UNIT_STRIDE * slot as u32;
            let mut bytes = Vec::new();
            bytes.extend(hp.to_le_bytes());
            bytes.extend(max_hp.to_le_bytes());
            bytes.extend(mp.to_le_bytes());
            bytes.extend(max_mp.to_le_bytes());
            memory.write(base, bytes);
        }
        memory
    }

    /// The exact bytes at 0x0200360C in the tutorial-battle save state.
    const TUTORIAL_SLOT_0: [u8; 16] = [
        0x10, 0x00, 0x10, 0x00, 0x0A, 0x00, 0x0A, 0x00, 0x13, 0x00, 0x19, 0x00, 0x00, 0x00, 0x08,
        0x00,
    ];

    #[test]
    fn the_tutorial_battle_bytes_decode_to_the_stats_shown_on_screen() {
        // Ritz's panel read HP 16/16, MP 10/10. If this ever fails, the field
        // layout is wrong and every reported number would be wrong with it.
        let memory = SparseMemory::new().with(UNIT_VITALS_BASE, TUTORIAL_SLOT_0.to_vec());
        let units = read_units(&memory);

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].hp, 16);
        assert_eq!(units[0].max_hp, 16);
        assert_eq!(units[0].mp, 10);
        assert_eq!(units[0].max_mp, 10);
    }

    #[test]
    fn consecutive_slots_are_one_stride_apart() {
        let memory = memory_with_units(&[(16, 16, 10, 10), (10, 10, 10, 10), (7, 10, 4, 10)]);
        let units = read_units(&memory);

        assert_eq!(units.len(), 3, "the tutorial battle fields three units");
        assert_eq!(units[2].hp, 7);
        assert_eq!(units[2].slot, 2);
    }

    #[test]
    fn reading_stops_at_the_first_empty_slot() {
        // Slot 1 empty, slot 2 populated: stale data past the gap must not be
        // reported as part of the formation.
        let mut memory = memory_with_units(&[(16, 16, 10, 10)]);
        let base = UNIT_VITALS_BASE + UNIT_STRIDE * 2;
        memory.write(base, [0x63, 0x00, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00].to_vec());

        let units = read_units(&memory);
        assert_eq!(units.len(), 1);
    }

    #[test]
    fn an_empty_array_reports_no_units() {
        assert!(read_units(&SparseMemory::new()).is_empty());
    }

    #[test]
    fn implausible_records_are_rejected() {
        // Between battles the array keeps whatever was there; these shapes are
        // what stale memory looks like.
        for vitals in [
            (16, 0, 10, 10),      // no max HP
            (99, 16, 10, 10),     // current above max
            (16, 5000, 10, 10),   // max beyond anything in the game
            (16, 16, 99, 10),     // MP above max MP
        ] {
            let memory = memory_with_units(&[vitals]);
            assert!(
                read_units(&memory).is_empty(),
                "{vitals:?} should have been rejected"
            );
        }
    }

    #[test]
    fn a_full_field_is_not_truncated() {
        let full: Vec<_> = (0..UNIT_SLOTS).map(|_| (10u16, 10u16, 5u16, 5u16)).collect();
        assert_eq!(read_units(&memory_with_units(&full)).len(), UNIT_SLOTS);
    }

    #[test]
    fn hp_text_and_fraction_match_the_values() {
        let unit = FftaUnit {
            slot: 0,
            hp: 8,
            max_hp: 16,
            mp: 3,
            max_mp: 10,
        };
        assert_eq!(unit.hp_text(), "8/16");
        assert_eq!(unit.mp_text(), "3/10");
        assert!((unit.hp_fraction() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn hp_fraction_survives_a_zero_max() {
        let unit = FftaUnit {
            slot: 0,
            hp: 0,
            max_hp: 0,
            mp: 0,
            max_mp: 0,
        };
        assert_eq!(unit.hp_fraction(), 0.0);
    }

    #[test]
    fn the_player_name_decodes_from_the_escaped_form_used_in_ram() {
        // Bytes copied from 0x02001F1C after entering "MarcheAAA".
        let bytes = vec![
            0x80, 0xBC, 0x80, 0xCA, 0x80, 0xDB, 0x80, 0xCC, 0x80, 0xD1, 0x80, 0xCE, 0x80, 0xB0,
            0x80, 0xB0, 0x80, 0xB0, 0x00, 0x00,
        ];
        let memory = SparseMemory::new().with(PLAYER_NAME_ADDR, bytes);
        assert_eq!(read_player_name(&memory), "MarcheAAA");
    }

    #[test]
    fn an_unset_player_name_reads_as_empty() {
        assert_eq!(read_player_name(&SparseMemory::new()), "");
    }
}

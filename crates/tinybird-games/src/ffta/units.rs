//! Final Fantasy Tactics Advance live unit data.
//!
//! # The roster is found, not remembered
//!
//! The first version of this module hard-coded `0x0200360C`, found by searching
//! the prologue state for the numbers on screen. Two things were wrong with it,
//! and both showed up the moment a second save state existed:
//!
//! - It is not the start of anything. It is slot 6 of a nine-unit roster that
//!   begins at `0x02002FDC`, so the panel showed the last three of the nine on
//!   the field and called them the party.
//! - It does not stay put. In a real battle the roster is at `0x02015A00` and
//!   that old address holds an unrelated record — the judge — so the panel
//!   showed one unit called `Judge` and nothing else.
//!
//! So [`find_roster`] looks for the roster by its shape instead: records one
//! [`UNIT_STRIDE`] apart, each with plausible vitals and a name pointer that
//! resolves. Both halves are needed — work RAM is full of small numbers, and a
//! stray cartridge address is easy to come by, but the two together are not.
//!
//! # How the fields were found
//!
//! Using `tinybird-probe` against the US ROM (`AFXE`):
//!
//! 1. Booted to the prologue snowball fight and captured a save state. The unit
//!    panel on screen read `Ritz  HP 16/16  MP 10/10`.
//! 2. `--find-bytes 100010000a000a00` found `16, 16, 10, 10` as consecutive
//!    little-endian `u16`s — the vitals block.
//! 3. `--stride 264` confirmed the record size.
//! 4. Unit names are not in RAM in either text form: searching for `Ritz` finds
//!    nothing. The record carries a pointer to the name instead; see
//!    [`UNIT_NAME_POINTER_BACK`].
//! 5. A second state, from a real battle, gave the flag byte: the six units
//!    with [`FLAG_PLAYER`] set were exactly the six the player controls, six
//!    being also the most FFTA will field.
//!
//! # What is deliberately not reported
//!
//! Everything else in the record. The fields that differ between units, as
//! offsets from the vitals block, with what was on screen for comparison:
//!
//! | offset | Ritz | Norma | Leslaie | guess |
//! |---|---|---|---|---|
//! | `-42`, `-41` | `05 02` | `07 03` | `08 02` | a coordinate pair |
//! | `-39`, `-38` | `05 02` | `07 03` | `08 02` | the same pair again |
//! | `-37` | `05` | `06` | `07` | height, or draw order |
//! | `-20`..`-17` | `5c11 5c01` | `604c 6004` | `6b17 6b17` | graphics ids |
//! | `+8`, `+10` | `19 25` | `21 26` | `0 0` | — |
//!
//! An earlier note called `+8` and `+10` attack and defence. That does not
//! survive the comparison: Leslaie has zero for both, and a unit standing on
//! the field with no attack and no defence is not what those would mean.
//!
//! None of it is exposed. `Lv 2`, `Exp 0`, `JP 0` and `WT 1/9` were all on
//! screen for Ritz, and several of these candidates could equally be any of
//! them — a plausible guess rendered as a labelled stat is worse than no stat.
//!
//! # Clan identity fields
//!
//! Public CodeBreaker roster codes confirm that persistent clan records begin
//! at `0x02000080`, making the vitals base `0x02000098`. The same record layout
//! puts race/current job at offsets `+6` and `+7`. The included battle state
//! validates playable combinations including Montblanc / Moogle / Black Mage
//! and Gallahan / Bangaa / White Monk. Level and JP remain unreported until
//! their fields can be verified rather than guessed.

use tinybird_addons::{MemoryView, ROM_BASE};

/// Bytes between consecutive unit records.
pub const UNIT_STRIDE: u32 = 0x108;

/// Longest run of records read from one roster.
///
/// A battle holds up to six of your units plus the other side and the judge;
/// thirteen were counted in the state this was checked against. The cap exists
/// so a corrupt run cannot walk the whole of EWRAM.
pub const UNIT_SLOTS: usize = 32;

/// The persistent clan list in the US release.
///
/// The public CodeBreaker roster codes begin player 1's record at
/// `0x02000080`; the vitals block starts 24 bytes into that record. Unlike the
/// battle array this address stays put, and it is the list wanted between
/// fights (up to twenty clan members rather than everyone on the map).
pub const CLAN_VITALS_BASE: u32 = 0x0200_0098;

/// Opponent-only roster prepared while the player is choosing a team.
///
/// Verified against `tb_bd3d3e5129e94331a95d91466eb69a90_AFXE_1788122234507_`
/// `FinalFantasyTa.state`: the Start To Battle screen holds Belvay, Singfels,
/// Julrat and Ashvin here before any clan deployment flag is committed.
pub const SETUP_ROSTER_VITALS_BASE: u32 = 0x0200_2FDC;

/// Work RAM, which is where every roster seen so far has lived.
const EWRAM_START: u32 = 0x0200_0000;
const EWRAM_END: u32 = 0x0204_0000;

/// The `+16` byte, whose high bit marks a unit as one of yours.
///
/// In the battle state the six units with it set — Udvil, Smyth, Gallahan,
/// Javet, Montblanc and the player — are exactly the six the player controls,
/// and the judge and the other side have it clear. Six is also the most FFTA
/// will field, which is the check that makes this more than a coincidence.
const UNIT_FLAGS: u32 = 16;
/// Bit of [`UNIT_FLAGS`] that marks one of the player's units.
const FLAG_PLAYER: u8 = 0x80;

/// Distance back from the vitals block to the record's name pointer.
///
/// # How this was found
///
/// The record start was unknown, so the bytes before the vitals were dumped
/// from the tutorial-battle state: at `0x020035F4`, twenty-four bytes ahead of
/// slot 0's vitals, sat `ad 12 55 08` — little-endian `0x085512AD`, a cartridge
/// address. `0x5512AD` is one byte before `Ritz` in the ROM's character name
/// table, and Ritz is the unit slot 0's vitals describe. The same offset in
/// slots 1 and 2 gave `Norma` and `Leslaie`, the other two units on the field,
/// and slot 3 held zero, matching the three-unit formation.
const UNIT_NAME_POINTER_BACK: u32 = 0x18;

/// Race and current job sit in the four-byte identity block near the start of
/// each record. The current job is the second job byte: named characters use
/// the first one for their special appearance while generics repeat the job.
const UNIT_RACE_BACK: u32 = 0x12;
const UNIT_JOB_BACK: u32 = 0x11;

/// One past the largest cartridge address a name pointer may hold.
///
/// The GBA cartridge window is 32 MB. A "pointer" outside it is stale memory
/// being read as one, which is what an empty or half-written slot looks like.
const ROM_LIMIT: u32 = ROM_BASE + 0x0200_0000;

/// Byte that introduces each entry in the name table.
///
/// The pointer addresses this rather than the first letter, so it is stepped
/// over before decoding. Tolerated rather than required: a pointer that already
/// lands on a letter decodes just as well.
const NAME_ENTRY_LEAD: u8 = 0x01;

/// Longest unit name read. The table's longest entry is `Adrammelech`.
const UNIT_NAME_MAX_BYTES: usize = 24;

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
    /// The unit's name, when the record's pointer led to a readable one.
    ///
    /// `None` rather than a placeholder, so the caller decides what to show
    /// instead — and so a wrong pointer is visibly absent rather than quietly
    /// rendered as something that looks like a name.
    pub name: Option<String>,
    /// Whether this is one of the player's units rather than the other side's.
    pub player: bool,
    /// FFTA's race id. Kept as an id as well as exposed through [`race_name`]
    /// so an unknown or modded value is not mislabeled.
    pub race_id: u8,
    /// FFTA's current-job id.
    pub job_id: u8,
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

    /// What to call this unit on screen.
    ///
    /// Falls back to the slot number, which is what every unit was called
    /// before the name pointer was found.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Unit {}", self.slot + 1))
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

    pub fn race_name(&self) -> Option<&'static str> {
        match self.race_id {
            1 => Some("Human"),
            2 => Some("Bangaa"),
            3 => Some("Nu Mou"),
            4 => Some("Viera"),
            5 => Some("Moogle"),
            0x06 => Some("Goblin"),
            0x07 => Some("Flan"),
            0x08 => Some("Bomb"),
            0x09 => Some("Dragon"),
            0x0A => Some("Lamia"),
            0x0B => Some("Antlion"),
            0x0C => Some("Blade Biter"),
            0x0D => Some("Tonberry"),
            0x0E => Some("Panther"),
            0x0F => Some("Malboro"),
            0x10 => Some("Ahriman"),
            0x11 => Some("Undead"),
            0x12 => Some("Fairy"),
            0x15 => Some("Totema"),
            0x16 => Some("Li-Grim"),
            0x17 => Some("Judge"),
            _ => None,
        }
    }

    /// Judges share the field roster but are neutral officials, not enemies.
    pub fn is_judge(&self) -> bool {
        self.race_id == 0x17
    }

    pub fn job_name(&self) -> Option<&'static str> {
        const JOBS: [&str; 71] = [
            "",
            "",
            "Soldier",
            "Paladin",
            "Fighter",
            "Thief",
            "Ninja",
            "White Mage",
            "Black Mage",
            "Illusionist",
            "Blue Mage",
            "Archer",
            "Hunter",
            "Warrior",
            "Dragoon",
            "Defender",
            "Gladiator",
            "White Monk",
            "Bishop",
            "Templar",
            "White Mage",
            "Black Mage",
            "Time Mage",
            "Illusionist",
            "Alchemist",
            "Beastmaster",
            "Morpher",
            "Sage",
            "Fencer",
            "Elementalist",
            "Red Mage",
            "White Mage",
            "Summoner",
            "Archer",
            "Assassin",
            "Sniper",
            "Animist",
            "Mog Knight",
            "Gunner",
            "Thief",
            "Juggler",
            "Gadgeteer",
            "Black Mage",
            "Time Mage",
            "Goblin",
            "Red Cap",
            "Jelly",
            "Ice Flan",
            "Cream",
            "Bomb",
            "Grenade",
            "Icedrake",
            "Firewyrm",
            "Thundrake",
            "Lamia",
            "Lilith",
            "Antlion",
            "Jawbreaker",
            "Toughskin",
            "Blade Biter",
            "Tonberry",
            "Masterberry",
            "Red Panther",
            "Coeurl",
            "Malboro",
            "Great Malboro",
            "Float-Eye",
            "Ahriman",
            "Zombie",
            "Vampire",
            "Sprite",
        ];
        JOBS.get(self.job_id as usize)
            .copied()
            .filter(|name| !name.is_empty())
            .or_else(|| match self.job_id {
                0x47 => Some("Titania"),
                0x4D => Some("Official"),
                0x53 => Some("Runeseeker"),
                0x54 => Some("Hermetic"),
                0x56 => Some("Biskmatar"),
                0x58 => Some("Li-Grim"),
                0x5A => Some("New Kid"),
                0x5B => Some("Librarian"),
                0x5C => Some("Class Head"),
                0x5D => Some("PE Head"),
                0x5E => Some("D.J."),
                0x5F => Some("Nurse"),
                0x60 => Some("Custodian"),
                0x61 => Some("Famfrit"),
                0x62 => Some("Ultima"),
                0x63 => Some("Adrammelech"),
                0x64 => Some("Exodus"),
                0x65 => Some("Mateus"),
                0x67 | 0x6A | 0x73 => Some("Judgemaster"),
                0x68 | 0x69 | 0x70..=0x72 => Some("Judge"),
                0x6B => Some("Mr. Leslaie"),
                0x6C => Some("Battle Queen"),
                0x6D => Some("Box"),
                0x6E => Some("Statue"),
                0x6F => Some("Sphere"),
                _ => None,
            })
    }
}

/// Read one slot's vitals without validating them.
fn read_slot(memory: &dyn MemoryView, vitals: u32, slot: usize) -> FftaUnit {
    FftaUnit {
        slot: slot as u8,
        // Filled in only once the vitals prove the slot is real, so a stale
        // record's leftover pointer is never followed.
        name: None,
        player: memory.read_u8(vitals + UNIT_FLAGS) & FLAG_PLAYER != 0,
        race_id: memory.read_u8(vitals - UNIT_RACE_BACK),
        job_id: memory.read_u8(vitals - UNIT_JOB_BACK),
        hp: memory.read_u16(vitals),
        max_hp: memory.read_u16(vitals + 2),
        mp: memory.read_u16(vitals + 4),
        max_mp: memory.read_u16(vitals + 6),
    }
}

/// Follow a unit record's name pointer to wherever the name lives.
///
/// Usually the cartridge, where the fixed character and clan-member names sit.
/// The player's own character is the exception: their name was typed in, so it
/// lives in work RAM and the record points there instead. Both are accepted;
/// anything outside either window is stale memory being read as an address.
///
/// Every step can fail, and each failure answers `None` rather than a guess: a
/// wrong name confidently displayed is worse than no name.
fn read_unit_name(memory: &dyn MemoryView, vitals: u32) -> Option<String> {
    let pointer = memory.read_u32(vitals - UNIT_NAME_POINTER_BACK);
    let addressable =
        (ROM_BASE..ROM_LIMIT).contains(&pointer) || (EWRAM_START..EWRAM_END).contains(&pointer);
    if !addressable {
        return None;
    }

    let mut at = pointer;
    if memory.read_u8(at) == NAME_ENTRY_LEAD {
        at += 1;
    }

    let name = super::text::decode(&memory.read_bytes(at, UNIT_NAME_MAX_BYTES));
    super::text::looks_like_name(&name).then_some(name)
}

/// Whether `vitals` looks like the vitals block of a real unit record.
///
/// Both halves are needed. Plausible vitals alone match any pair of small
/// numbers, of which work RAM has thousands; a readable name alone would follow
/// any stray cartridge address. Together they are specific enough that the scan
/// below finds rosters and nothing else.
fn looks_like_record(memory: &dyn MemoryView, vitals: u32) -> bool {
    if vitals < EWRAM_START + UNIT_NAME_POINTER_BACK || vitals + 8 > EWRAM_END {
        return false;
    }
    read_slot(memory, vitals, 0).is_plausible() && read_unit_name(memory, vitals).is_some()
}

/// How many records run consecutively from `base`.
fn run_length(memory: &dyn MemoryView, base: u32) -> usize {
    (0..UNIT_SLOTS)
        .take_while(|slot| looks_like_record(memory, base + UNIT_STRIDE * *slot as u32))
        .count()
}

/// Find the roster by looking for it, rather than by remembering an address.
///
/// # Why this is a search
///
/// The first version of this module hard-coded `0x0200360C`, found by searching
/// the prologue state for the numbers on screen. That address is not the start
/// of anything: it is slot 6 of a nine-unit roster beginning at `0x02002FDC`,
/// which is why the panel showed three units when nine were on the field. In a
/// real battle the same address holds an unrelated record — the judge — and the
/// panel showed one unit called `Judge` with the whole party missing.
///
/// The roster moves between scenes, so the only thing that can be relied on is
/// its shape: records one [`UNIT_STRIDE`] apart, each with plausible vitals and
/// a name pointer that resolves. The longest such run is the roster; in the
/// battle state that is thirteen units against ten for the clan list and seven
/// for a sub-array of the other side.
pub fn find_roster(memory: &dyn MemoryView) -> Option<u32> {
    find_roster_except(memory, None)
}

/// Find the longest roster other than a known persistent list.
///
/// In FFTA the clan list can be longer than the active field array. Excluding
/// it is therefore necessary when the caller specifically needs combat data.
pub fn find_roster_except(memory: &dyn MemoryView, excluded: Option<u32>) -> Option<u32> {
    let mut best: Option<(usize, u32)> = None;
    let mut vitals = EWRAM_START + UNIT_NAME_POINTER_BACK;

    while vitals + 8 <= EWRAM_END {
        if looks_like_record(memory, vitals) {
            let length = run_length(memory, vitals);
            if length >= 2
                && Some(vitals) != excluded
                && best.is_none_or(|(best_len, _)| length > best_len)
            {
                best = Some((length, vitals));
            }
            // Skip the run rather than every record inside it, so the same
            // roster is not measured once per member.
            vitals += UNIT_STRIDE * length.max(1) as u32;
        } else {
            // Records are word-aligned in every roster seen.
            vitals += 4;
        }
    }

    best.map(|(_, base)| base)
}

/// Whether `base` still looks like the start of a roster.
///
/// Cheap enough to run every frame, unlike [`find_roster`], so a caller can
/// keep an address and re-check it rather than searching again.
pub fn is_roster_at(memory: &dyn MemoryView, base: u32) -> bool {
    run_length(memory, base) >= 2
}

/// Read the roster starting at `base`.
///
/// Stops at the first record that does not validate rather than skipping it:
/// rosters are filled from the front, so a gap is the end of one, and
/// continuing past it would report whatever happened to be left in memory.
pub fn read_units_at(memory: &dyn MemoryView, base: u32) -> Vec<FftaUnit> {
    let mut units = Vec::new();
    for slot in 0..UNIT_SLOTS {
        let vitals = base + UNIT_STRIDE * slot as u32;
        let mut unit = read_slot(memory, vitals, slot);
        // Vitals decide where the roster ends. The name is wanted but not
        // required: a unit whose pointer does not resolve is still standing on
        // the field, and dropping it would be a worse answer than `Unit 4`.
        // Discovery is stricter — see `looks_like_record`.
        if !unit.is_plausible() {
            break;
        }
        unit.name = read_unit_name(memory, vitals);
        units.push(unit);
    }
    units
}

/// Find the roster and read it.
pub fn read_units(memory: &dyn MemoryView) -> Vec<FftaUnit> {
    find_roster(memory)
        .map(|base| read_units_at(memory, base))
        .unwrap_or_default()
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

    /// Somewhere in work RAM to build a roster, standing in for wherever the
    /// game happens to have put one. Nothing depends on the value: finding the
    /// roster without being told is the point.
    const BASE: u32 = 0x0200_3000;
    /// A cartridge address to hang test names off.
    const NAMES: u32 = 0x0855_0000;

    /// The exact bytes at 0x0200360C in the tutorial-battle save state.
    const TUTORIAL_SLOT_0: [u8; 16] = [
        0x10, 0x00, 0x10, 0x00, 0x0A, 0x00, 0x0A, 0x00, 0x13, 0x00, 0x19, 0x00, 0x00, 0x00, 0x08,
        0x00,
    ];
    /// The bytes at `0x085512AD` in the US ROM: the entry lead, then `Ritz`.
    const RITZ_IN_ROM: [u8; 6] = [0x01, 0xC2, 0xD3, 0xDE, 0xE4, 0x00];

    /// Plain-form bytes for a name, as the cartridge stores them.
    fn rom_name(text: &str) -> Vec<u8> {
        let mut bytes = vec![NAME_ENTRY_LEAD];
        bytes.extend(super::super::text::encode_plain(text));
        bytes.push(0x00);
        bytes
    }

    /// One record: vitals, the flag byte, and a name pointer into `NAMES`.
    fn record(
        memory: SparseMemory,
        slot: usize,
        (hp, max_hp, mp, max_mp): Vitals,
        name: &str,
        player: bool,
    ) -> SparseMemory {
        let vitals = BASE + UNIT_STRIDE * slot as u32;
        let at = NAMES + 0x40 * slot as u32;
        let mut bytes = Vec::new();
        bytes.extend(hp.to_le_bytes());
        bytes.extend(max_hp.to_le_bytes());
        bytes.extend(mp.to_le_bytes());
        bytes.extend(max_mp.to_le_bytes());
        memory
            .with(vitals, bytes)
            .with(vitals + UNIT_FLAGS, vec![if player { 0x81 } else { 0x01 }])
            .with(vitals - UNIT_NAME_POINTER_BACK, at.to_le_bytes().to_vec())
            .with(at, rom_name(name))
    }

    /// HP, max HP, MP, max MP — the four numbers a record starts with.
    type Vitals = (u16, u16, u16, u16);

    /// A roster of `(vitals, name)` pairs, all on the player's side.
    fn roster(units: &[(Vitals, &str)]) -> SparseMemory {
        let mut memory = SparseMemory::new();
        for (slot, (vitals, name)) in units.iter().enumerate() {
            memory = record(memory, slot, *vitals, name, true);
        }
        memory
    }

    #[test]
    fn the_tutorial_battle_bytes_decode_to_the_stats_shown_on_screen() {
        // Ritz's panel read HP 16/16, MP 10/10. If this ever fails, the field
        // layout is wrong and every reported number would be wrong with it.
        let memory = SparseMemory::new()
            .with(BASE, TUTORIAL_SLOT_0.to_vec())
            .with(
                BASE - UNIT_NAME_POINTER_BACK,
                0x0855_12ADu32.to_le_bytes().to_vec(),
            )
            .with(0x0855_12AD, RITZ_IN_ROM.to_vec());
        let units = read_units_at(&memory, BASE);

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].hp, 16);
        assert_eq!(units[0].max_hp, 16);
        assert_eq!(units[0].mp, 10);
        assert_eq!(units[0].max_mp, 10);
        assert_eq!(units[0].name.as_deref(), Some("Ritz"));
    }

    #[test]
    fn the_roster_is_found_rather_than_remembered() {
        // The point of the rewrite: nothing tells the reader where to look,
        // and it finds the roster anyway.
        let memory = roster(&[
            ((16, 16, 10, 10), "Ritz"),
            ((10, 10, 10, 10), "Norma"),
            ((10, 10, 10, 10), "Leslaie"),
        ]);
        assert_eq!(find_roster(&memory), Some(BASE));

        let names: Vec<_> = read_units(&memory)
            .iter()
            .map(|unit| unit.display_name())
            .collect();
        assert_eq!(names, ["Ritz", "Norma", "Leslaie"]);
    }

    #[test]
    fn the_longest_roster_wins() {
        // A battle state holds several runs at once: the units on the field,
        // the clan list, and a shorter sub-array of the other side. The one
        // with the most members is the field.
        let mut memory = roster(&[((20, 20, 5, 5), "Gallahan"), ((20, 20, 5, 5), "Javet")]);
        let far = BASE + 0x4000;
        for (slot, name) in ["Sis", "Michaelov", "Kenneth", "David"].iter().enumerate() {
            let vitals = far + UNIT_STRIDE * slot as u32;
            let at = NAMES + 0x2000 + 0x40 * slot as u32;
            memory = memory
                .with(vitals, vec![30, 0, 30, 0, 5, 0, 5, 0])
                .with(vitals - UNIT_NAME_POINTER_BACK, at.to_le_bytes().to_vec())
                .with(at, rom_name(name));
        }

        assert_eq!(find_roster(&memory), Some(far));
        assert_eq!(read_units(&memory).len(), 4);
    }

    #[test]
    fn a_lone_record_is_not_a_roster() {
        // What the old hard-coded address hit in a real battle: one unrelated
        // record, reported as if it were the whole party.
        let memory = roster(&[((10, 10, 10, 10), "Judge")]);
        assert_eq!(find_roster(&memory), None);
        assert!(read_units(&memory).is_empty());
    }

    #[test]
    fn reading_stops_at_the_first_empty_slot() {
        // Slot 1 empty, slot 2 populated: stale data past the gap must not be
        // reported as part of the formation.
        let mut memory = roster(&[((16, 16, 10, 10), "Ritz")]);
        memory = record(memory, 2, (99, 99, 0, 0), "Nobody", false);

        assert_eq!(read_units_at(&memory, BASE).len(), 1);
    }

    #[test]
    fn an_empty_array_reports_no_units() {
        assert!(read_units(&SparseMemory::new()).is_empty());
        assert_eq!(find_roster(&SparseMemory::new()), None);
    }

    #[test]
    fn implausible_records_are_rejected() {
        // Between battles the array keeps whatever was there; these shapes are
        // what stale memory looks like.
        for vitals in [
            (16, 0, 10, 10),    // no max HP
            (99, 16, 10, 10),   // current above max
            (16, 5000, 10, 10), // max beyond anything in the game
            (16, 16, 99, 10),   // MP above max MP
        ] {
            let memory = roster(&[(vitals, "Ghost")]);
            assert!(
                read_units_at(&memory, BASE).is_empty(),
                "{vitals:?} should have been rejected"
            );
        }
    }

    #[test]
    fn the_players_units_are_told_apart_from_the_rest() {
        // The ones with the flag set are the ones you control; the judge and
        // the other side have it clear.
        let mut memory = SparseMemory::new();
        memory = record(memory, 0, (53, 53, 15, 15), "Gallahan", true);
        memory = record(memory, 1, (43, 43, 44, 44), "Montblanc", true);
        memory = record(memory, 2, (10, 10, 10, 10), "Judge", false);
        memory = record(memory, 3, (56, 56, 31, 31), "Zanaharti", false);

        let units = read_units_at(&memory, BASE);
        let mine: Vec<_> = units
            .iter()
            .filter(|unit| unit.player)
            .map(|unit| unit.display_name())
            .collect();
        assert_eq!(mine, ["Gallahan", "Montblanc"]);
        assert_eq!(units.iter().filter(|unit| !unit.player).count(), 2);
    }

    #[test]
    fn a_name_pointer_into_work_ram_is_followed_too() {
        // The player's own character was named at the entry screen, so their
        // record points at the RAM buffer rather than the cartridge.
        let memory = roster(&[((68, 68, 17, 17), "Placeholder")])
            .with(
                BASE - UNIT_NAME_POINTER_BACK,
                PLAYER_NAME_ADDR.to_le_bytes().to_vec(),
            )
            .with(
                PLAYER_NAME_ADDR,
                vec![
                    0x80, 0xBC, 0x80, 0xCA, 0x80, 0xDB, 0x80, 0xCC, 0x80, 0xD1, 0x80, 0xCE, 0x00,
                    0x00,
                ],
            );

        assert_eq!(
            read_units_at(&memory, BASE)[0].name.as_deref(),
            Some("Marche")
        );
    }

    #[test]
    fn a_pointer_that_leads_nowhere_leaves_the_unit_unnamed() {
        // The unit is still on the field, so it is still reported — under the
        // slot number it had before any of this.
        for pointer in [0x0000_0000u32, 0xFFFF_FFFFu32] {
            let memory = roster(&[((16, 16, 10, 10), "Ritz")]).with(
                BASE - UNIT_NAME_POINTER_BACK,
                pointer.to_le_bytes().to_vec(),
            );
            let units = read_units_at(&memory, BASE);
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].name, None, "{pointer:#010x}");
            assert_eq!(units[0].display_name(), "Unit 1");
        }
    }

    #[test]
    fn a_roster_is_not_found_by_its_vitals_alone() {
        // Work RAM is full of small numbers. Without the name pointer the scan
        // would land on any pair of them.
        let mut memory = SparseMemory::new();
        for slot in 0..4u32 {
            memory = memory.with(BASE + UNIT_STRIDE * slot, vec![10, 0, 10, 0, 5, 0, 5, 0]);
        }
        assert_eq!(find_roster(&memory), None);
    }

    #[test]
    fn hp_text_and_fraction_match_the_values() {
        let unit = FftaUnit {
            slot: 0,
            name: None,
            player: true,
            race_id: 1,
            job_id: 1,
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
            name: None,
            player: false,
            race_id: 0,
            job_id: 0,
            hp: 0,
            max_hp: 0,
            mp: 0,
            max_mp: 0,
        };
        assert_eq!(unit.hp_fraction(), 0.0);
    }

    #[test]
    fn an_unnamed_unit_is_still_called_something() {
        let unit = FftaUnit {
            slot: 2,
            name: None,
            player: false,
            race_id: 0,
            job_id: 0,
            hp: 10,
            max_hp: 10,
            mp: 0,
            max_mp: 0,
        };
        assert_eq!(unit.display_name(), "Unit 3");
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

//! Reading name tables out of the cartridge instead of guessing them.
//!
//! Moves, species and items all have their names in the ROM, in fixed-width
//! entries of Generation 3 text. The obvious way to read them is to hardcode
//! the address of each table — and that is wrong, because the addresses move
//! between FireRed, LeafGreen, their revisions and the Japanese builds. A
//! hardcoded offset that is right for one build produces confident nonsense on
//! every other one, which is worse than producing nothing.
//!
//! So the tables are **found** rather than named. Each one has an anchor: a
//! name at a known index that is not going to change. Search the ROM for that
//! name's encoded bytes, work back to where the table must start, then verify
//! by decoding two more entries at indices the anchor did not touch. A base
//! that survives all three is the table.
//!
//! The scan reads the whole cartridge once — about 56ms natively — and the
//! result is cached against the ROM it came from, so switching games re-finds
//! them and playing one does not.
//!
//! When the search fails, callers fall back to the tables compiled into
//! `pokemon_frlg.rs`. Failing is not unusual: a ROM hack may have moved
//! everything, and half a name table is worse than none.

use std::sync::RwLock;

use tinybird_addons::{MemoryView, RomIdentity, ROM_BASE};

/// Largest cartridge the GBA addresses.
const MAX_ROM_BYTES: usize = 32 * 1024 * 1024;
/// Read this much at a time while searching, so one pass is not 16 million
/// separate calls through the bus.
const SCAN_CHUNK: usize = 64 * 1024;

/// One name table: where it starts and how far apart its entries are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Table {
    base: u32,
    stride: u32,
    /// How many bytes of each entry are the name.
    width: usize,
}

impl Table {
    /// The name at `index`, or `None` if it decodes to nothing.
    pub fn name(&self, memory: &dyn MemoryView, index: u16) -> Option<String> {
        let at = self
            .base
            .checked_add(u32::from(index).checked_mul(self.stride)?)?;
        let raw = memory.read_bytes(at, self.width);
        let text = decode(&raw);
        (!text.is_empty()).then_some(text)
    }
}

/// The three tables a FireRed read-out needs.
#[derive(Clone, Copy, Debug, Default)]
struct Tables {
    species: Option<Table>,
    moves: Option<Table>,
    items: Option<Table>,
}

/// Names read out of the cartridge, indexed by the id the game uses.
///
/// The *names* are cached rather than the table addresses, so the lookups stay
/// plain functions of an id. Threading a `MemoryView` down to every place that
/// wants to print a move would have touched a dozen signatures to answer a
/// question that has the same answer every time.
#[derive(Default)]
struct Names {
    species: Vec<String>,
    moves: Vec<String>,
    items: Vec<String>,
}

/// How far each table is read. One past the last real entry is harmless: an
/// entry that does not decode is dropped.
const SPECIES_COUNT: u16 = 412;
const MOVE_COUNT: u16 = 355;
const ITEM_COUNT: u16 = 400;

/// What the names were read from, so a different cartridge re-reads them.
///
/// The fingerprint is what makes this a *cartridge* key rather than a *game*
/// key. A ROM hack keeps the game code and revision of whatever it was built
/// on, so keying on those two alone meant loading vanilla FireRed and then a
/// FireRed hack in one session served the hack the original's name tables —
/// its renamed species reported under the names they used to have, with
/// nothing to indicate the read-out was describing a different cartridge.
#[derive(Clone, PartialEq, Eq)]
struct RomKey {
    game_code: String,
    revision: u8,
    fingerprint: u64,
}

static CACHE: RwLock<Option<(RomKey, Names)>> = RwLock::new(None);

/// Read the name tables for this ROM, once.
///
/// Call before anything asks for a name. Cheap after the first call for a
/// given cartridge, and the first call is one pass over the ROM.
pub fn ensure(memory: &dyn MemoryView, rom: &RomIdentity) {
    ensure_within(memory, rom, MAX_ROM_BYTES)
}

/// [`ensure`], with the scan bounded. Tests use it so a fixture with one table
/// in it does not read the whole address space looking for the other two.
fn ensure_within(memory: &dyn MemoryView, rom: &RomIdentity, limit: usize) {
    let key = RomKey {
        game_code: rom.game_code.clone(),
        revision: rom.revision,
        fingerprint: rom.fingerprint,
    };

    if let Ok(cache) = CACHE.read() {
        if cache.as_ref().is_some_and(|(cached, _)| *cached == key) {
            return;
        }
    }

    let tables = search_within(memory, limit);
    let names = Names {
        species: read_all(memory, tables.species, SPECIES_COUNT),
        moves: read_all(memory, tables.moves, MOVE_COUNT),
        items: read_all(memory, tables.items, ITEM_COUNT),
    };

    if let Ok(mut cache) = CACHE.write() {
        *cache = Some((key, names));
    }
}

fn read_all(memory: &dyn MemoryView, table: Option<Table>, count: u16) -> Vec<String> {
    let Some(table) = table else {
        return Vec::new();
    };
    (0..count)
        .map(|index| table.name(memory, index).unwrap_or_default())
        .collect()
}

fn look_up(pick: fn(&Names) -> &Vec<String>, index: u16) -> Option<String> {
    let cache = CACHE.read().ok()?;
    let (_, names) = cache.as_ref()?;
    let name = pick(names).get(index as usize)?;
    (!name.is_empty()).then(|| title_case(name))
}

/// `"MASTER BALL"` to `"Master Ball"`.
///
/// Generation 3 stores every name in capitals, because that is how the games
/// display them. The read-out is not the game: abilities and natures beside
/// these come from tables written in title case, and a panel that shouts three
/// of its six rows looks broken rather than authentic.
///
/// Word boundaries are spaces, hyphens and full stops, so `X-SCISSOR` and
/// `MR. MIME` come out right rather than as `X-scissor` and `Mr. mime`.
fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut start_of_word = true;

    for ch in name.chars() {
        if start_of_word {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
        start_of_word = matches!(ch, ' ' | '-' | '.');
    }
    out
}

/// A species name from the cartridge, if it had one to give.
pub fn species(index: u16) -> Option<String> {
    look_up(|names| &names.species, index)
}

/// A move name from the cartridge.
pub fn move_name(index: u16) -> Option<String> {
    look_up(|names| &names.moves, index)
}

/// An item name from the cartridge.
pub fn item(index: u16) -> Option<String> {
    look_up(|names| &names.items, index)
}

/// Drop the cache. Only for tests, which run several ROMs in one process.
#[cfg(test)]
pub fn forget() {
    if let Ok(mut cache) = CACHE.write() {
        *cache = None;
    }
}

/// The anchors each table is found by.
///
/// Two are checked beyond the one searched for, at indices far enough apart
/// that a coincidental match cannot satisfy all three. "POUND" appears in the
/// ROM in other places — inside longer strings, in unrelated data — and the
/// verification is what tells the table apart from those.
struct Anchor {
    /// Name to search for, and the index it sits at.
    needle: (&'static str, u16),
    /// Two more (index, name) pairs the candidate base has to satisfy.
    confirm: [(u16, &'static str); 2],
    stride: u32,
    width: usize,
}

const SPECIES: Anchor = Anchor {
    needle: ("BULBASAUR", 1),
    confirm: [(4, "CHARMANDER"), (25, "PIKACHU")],
    stride: 11,
    width: 11,
};

const MOVES: Anchor = Anchor {
    needle: ("POUND", 1),
    confirm: [(10, "SCRATCH"), (33, "TACKLE")],
    stride: 13,
    width: 13,
};

/// Items are a struct, not a bare string: the name is the first 14 bytes of a
/// 44-byte record, so the stride is the record and the width is the name.
const ITEMS: Anchor = Anchor {
    needle: ("MASTER BALL", 1),
    confirm: [(4, "POKé BALL"), (13, "POTION")],
    stride: 44,
    width: 14,
};

/// Search the cartridge for all three tables in one pass.
///
/// `limit` is how far to read. There is deliberately no "stop at the first
/// empty chunk" shortcut: a cartridge can have a run of padding in the middle
/// of it, and a scan that gave up there would silently come back with half the
/// tables — which reads exactly like a game that has no names, and is much
/// harder to notice than being slow.
fn search_within(memory: &dyn MemoryView, limit: usize) -> Tables {
    // Three passes would be three times the reading for the same answer.
    const ANCHORS: [&Anchor; 3] = [&SPECIES, &MOVES, &ITEMS];

    let needles: Vec<Vec<u8>> = ANCHORS.iter().map(|a| encode(a.needle.0)).collect();
    // Chunks overlap by the longest needle, so a name straddling a boundary is
    // still found rather than falling down the gap between two reads.
    let overlap = needles.iter().map(Vec::len).max().unwrap_or(0);

    let mut found: [Option<Table>; 3] = [None; 3];
    let mut offset = 0usize;

    // Stops as soon as all three are found, which on a real cartridge is well
    // before the end.
    while offset < limit && found.iter().any(Option::is_none) {
        let chunk = memory.read_bytes(ROM_BASE.wrapping_add(offset as u32), SCAN_CHUNK + overlap);

        for (slot, needle) in needles.iter().enumerate() {
            if found[slot].is_some() {
                continue;
            }
            for hit in find_all(&chunk, needle) {
                let at = ROM_BASE.wrapping_add((offset + hit) as u32);
                if let Some(table) = base_from_hit(memory, ANCHORS[slot], at) {
                    found[slot] = Some(table);
                    break;
                }
            }
        }

        offset += SCAN_CHUNK;
    }

    Tables {
        species: found[0],
        moves: found[1],
        items: found[2],
    }
}

/// Turn a hit into a table base, if the entries around it agree.
fn base_from_hit(memory: &dyn MemoryView, anchor: &Anchor, hit: u32) -> Option<Table> {
    let base = hit.checked_sub(u32::from(anchor.needle.1) * anchor.stride)?;
    let table = Table {
        base,
        stride: anchor.stride,
        width: anchor.width,
    };

    for (index, expected) in anchor.confirm {
        let name = table.name(memory, index)?;
        if !name.eq_ignore_ascii_case(expected) {
            return None;
        }
    }
    Some(table)
}

fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|start| &haystack[*start..*start + needle.len()] == needle)
        .collect()
}

/// Generation 3 text, encoded. The inverse of [`decode`].
fn encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| match ch {
            ' ' => 0x00,
            '0'..='9' => 0xA1 + (ch as u8 - b'0'),
            '!' => 0xAB,
            '?' => 0xAC,
            '.' => 0xAD,
            '-' => 0xAE,
            '\'' => 0xB4,
            ',' => 0xB8,
            '/' => 0xB9,
            ':' => 0xBA,
            'A'..='Z' => 0xBB + (ch as u8 - b'A'),
            'a'..='z' => 0xD5 + (ch as u8 - b'a'),
            // The accented e in "POKé BALL". Anything else is not searchable.
            'é' => 0x1B,
            _ => 0xFF,
        })
        .collect()
}

/// Generation 3 text, decoded. Kept here rather than shared with
/// `pokemon_frlg.rs` because that one substitutes `UNKNOWN` for an empty
/// result, which is right for a nickname and wrong for a table probe: an empty
/// entry is how this tells a real table from a coincidence.
fn decode(bytes: &[u8]) -> String {
    let mut text = String::new();
    for &byte in bytes {
        match byte {
            0xFF => break,
            0x00 => text.push(' '),
            0x1B => text.push('é'),
            0xA1..=0xAA => text.push(char::from(b'0' + (byte - 0xA1))),
            0xAB => text.push('!'),
            0xAC => text.push('?'),
            0xAD => text.push('.'),
            0xAE => text.push('-'),
            0xB4 => text.push('\''),
            0xB8 => text.push(','),
            0xB9 => text.push('/'),
            0xBA => text.push(':'),
            0xBB..=0xD4 => text.push(char::from(b'A' + (byte - 0xBB))),
            0xD5..=0xEE => text.push(char::from(b'a' + (byte - 0xD5))),
            // Anything else is not text, so this is not a name table.
            _ => return String::new(),
        }
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinybird_addons::SparseMemory;

    fn rom_id(code: &str) -> RomIdentity {
        dump(code, 0)
    }

    /// The same header, from a named cartridge. Every field but the last is
    /// what a ROM hack inherits unchanged from the game it was built on.
    fn dump(code: &str, fingerprint: u64) -> RomIdentity {
        RomIdentity {
            title: "POKEMON FIRE".to_string(),
            game_code: code.to_string(),
            maker_code: "01".to_string(),
            revision: 0,
            fingerprint,
        }
    }

    /// Lay a table out the way a cartridge does, at an address nothing in the
    /// code knows about — which is the whole point of searching for it.
    fn planted(anchor: &Anchor, base: u32, entries: &[(u16, &str)]) -> SparseMemory {
        let mut memory = SparseMemory::new();
        for (index, name) in entries {
            let mut bytes = encode(name);
            bytes.resize(anchor.width, 0xFF);
            memory = memory.with(base + u32::from(*index) * anchor.stride, bytes);
        }
        memory
    }

    #[test]
    fn text_survives_a_round_trip() {
        for original in ["POUND", "MASTER BALL", "POKé BALL", "X-SCISSOR", "DON'T"] {
            assert_eq!(decode(&encode(original)), original, "{original}");
        }
    }

    #[test]
    fn a_table_is_found_by_its_anchor_wherever_it_sits() {
        let base = 0x0824_7094;
        let memory = planted(
            &MOVES,
            base,
            &[(1, "POUND"), (10, "SCRATCH"), (33, "TACKLE"), (52, "EMBER")],
        );

        let table = base_from_hit(&memory, &MOVES, base + MOVES.stride).expect("table");
        assert_eq!(table.base, base);
        assert_eq!(table.name(&memory, 52).as_deref(), Some("EMBER"));
    }

    /// The reason for confirming two more entries. A name appears in a ROM in
    /// plenty of places that are not its table — inside longer strings, in
    /// dialogue, in unrelated data — and the anchor alone cannot tell them
    /// apart.
    #[test]
    fn a_coincidental_match_is_rejected() {
        // "POUND" sitting on its own, with nothing where the table would have
        // SCRATCH and TACKLE.
        let stray = SparseMemory::new().with(0x0810_0000, encode("POUND"));
        assert!(base_from_hit(&stray, &MOVES, 0x0810_0000 + MOVES.stride).is_none());
    }

    /// Items are a 44-byte record with a 14-byte name, not a bare string. Using
    /// the name width as the stride would read every entry from the wrong place.
    #[test]
    fn item_records_are_strided_by_the_record_not_the_name() {
        assert_eq!(ITEMS.stride, 44);
        assert_eq!(ITEMS.width, 14);

        let base = 0x083D_B028;
        let memory = planted(
            &ITEMS,
            base,
            &[
                (1, "MASTER BALL"),
                (4, "POKé BALL"),
                (13, "POTION"),
                (19, "FULL RESTORE"),
            ],
        );

        let table = base_from_hit(&memory, &ITEMS, base + ITEMS.stride).expect("table");
        assert_eq!(table.name(&memory, 19).as_deref(), Some("FULL RESTORE"));
    }

    /// A cartridge with no tables in it yields nothing rather than something
    /// wrong, and the caller falls back to the compiled tables.
    /// A cartridge with no tables in it yields nothing rather than something
    /// wrong, and the caller falls back to the compiled tables.
    #[test]
    fn a_rom_without_the_tables_gives_no_names() {
        forget();
        ensure_within(&SparseMemory::new(), &rom_id("ZZZE"), 256 * 1024);
        assert_eq!(species(1), None);
        assert_eq!(move_name(1), None);
        assert_eq!(item(1), None);
        forget();
    }

    /// The end-to-end path: plant a table where nothing knows to look, and the
    /// lookups find it.
    #[test]
    fn names_come_from_the_cartridge_once_it_has_been_read() {
        forget();
        let memory = planted(
            &MOVES,
            0x0824_7094,
            &[(1, "POUND"), (10, "SCRATCH"), (33, "TACKLE"), (52, "EMBER")],
        );

        ensure_within(&memory, &rom_id("BPRE"), 4 * 1024 * 1024);
        assert_eq!(
            move_name(52).as_deref(),
            Some("Ember"),
            "and title-cased on the way out"
        );
        // Nothing was planted for these, so they have nothing to say.
        assert_eq!(species(1), None);
        assert_eq!(item(1), None);
        forget();
    }

    /// The bug the fingerprint is in this key for.
    ///
    /// A hack keeps the game code and revision of whatever it was built on, so
    /// keyed on those two alone the cache could not tell two cartridges apart:
    /// load the original, load the hack, and the hack was served the
    /// original's names — silently, and under a read-out that looked right.
    #[test]
    fn two_cartridges_with_the_same_header_do_not_share_one_set_of_names() {
        forget();
        let anchors =
            |last: &'static str| vec![(1u16, "POUND"), (10, "SCRATCH"), (33, "TACKLE"), (52, last)];

        let original = planted(&MOVES, 0x0824_7094, &anchors("EMBER"));
        ensure_within(
            &original,
            &dump("BPRE", 0xa370_b3d2_a324_f96e),
            4 * 1024 * 1024,
        );
        assert_eq!(move_name(52).as_deref(), Some("Ember"));

        // Identical in every field a header carries; a different cartridge.
        let hack = planted(&MOVES, 0x0824_7094, &anchors("SCALD"));
        ensure_within(&hack, &dump("BPRE", 0xdfb8_d7b2_56b1_58e2), 4 * 1024 * 1024);
        assert_eq!(
            move_name(52).as_deref(),
            Some("Scald"),
            "the cartridge's own name, not one cached from the cartridge before it",
        );
        forget();
    }

    #[test]
    fn names_are_title_cased_for_a_panel_that_is_not_all_capitals() {
        assert_eq!(title_case("BULBASAUR"), "Bulbasaur");
        assert_eq!(title_case("MASTER BALL"), "Master Ball");
        assert_eq!(title_case("PSYCHO BOOST"), "Psycho Boost");
        // Hyphens and stops start a word too.
        assert_eq!(title_case("X-SCISSOR"), "X-Scissor");
        assert_eq!(title_case("MR. MIME"), "Mr. Mime");
        assert_eq!(title_case("POKé BALL"), "Poké Ball");
    }

    #[test]
    fn a_table_entry_that_is_not_text_is_not_a_name() {
        // 0x50 is not in the character set, so this is data, not a name.
        assert_eq!(decode(&[0xBB, 0x50, 0xBB]), "");
        // A terminator straight away is an empty entry, which is also not one.
        assert_eq!(decode(&[0xFF, 0xBB]), "");
    }
}

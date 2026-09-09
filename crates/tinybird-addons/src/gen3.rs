//! Reading a Generation 3 Pokémon record without knowing which game it is.
//!
//! Two things in these games are stored in a way a plain memory read cannot
//! reach, and both of them are the things a person actually wants to see:
//!
//! - **Names are not ASCII.** The games use a character set of their own, so
//!   `read_ascii` on a nickname produces punctuation soup. [`decode_text`]
//!   is the table.
//! - **Species is encrypted.** Bytes 32..80 of a 100-byte party record are
//!   four 12-byte substructures, XOR'd word by word with `personality ^ ot_id`
//!   and *reordered* by `personality % 24`. Species is the first halfword of
//!   the Growth substructure, wherever the permutation happened to put it.
//!   [`party_species`] undoes both.
//!
//! Everything else a party record holds — level, HP, the six stats — sits in
//! the unencrypted tail from byte 80 on, which is why a manifest could always
//! read those with a plain `u8`/`u16` and never the name of the thing they
//! belong to.
//!
//! This lives here rather than in `tinybird-games` because a JSON manifest is
//! evaluated in `tinybird-addons`, and a reader that can show `HP 53/53`
//! without being able to say *whose* HP it is was the gap worth closing.
//! `pokemon_frlg.rs` keeps its own richer parse; this is the subset a manifest
//! can ask for.

use crate::MemoryView;

/// A party record: personality, OT id, nickname, the encrypted block, stats.
pub const PARTY_RECORD_BYTES: u32 = 100;
/// How much of a record has to be read to decrypt it.
pub const BOXED_BYTES: u32 = 80;
/// A nickname is ten characters, and sits at `record + 8` — outside the
/// encrypted block, but in the game's alphabet rather than in ASCII.
pub const NICKNAME_OFFSET: u32 = 8;

/// Which 12-byte block holds which substructure, for each `personality % 24`.
const SUBSTRUCT_ORDERS: [[usize; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 3, 1, 2],
    [0, 2, 3, 1],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [2, 0, 1, 3],
    [3, 0, 1, 2],
    [2, 0, 3, 1],
    [3, 0, 2, 1],
    [1, 2, 0, 3],
    [1, 3, 0, 2],
    [2, 1, 0, 3],
    [3, 1, 0, 2],
    [2, 3, 0, 1],
    [3, 2, 0, 1],
    [1, 2, 3, 0],
    [1, 3, 2, 0],
    [2, 1, 3, 0],
    [3, 1, 2, 0],
    [2, 3, 1, 0],
    [3, 2, 1, 0],
];

/// Where the Growth substructure sits inside the decrypted 48-byte block.
///
/// Public because the tests that build a record have to write species where
/// the decoder will look for it, and hiding the permutation from them would
/// only mean writing it out twice.
pub fn growth_offset(personality: u32) -> usize {
    SUBSTRUCT_ORDERS[(personality % 24) as usize][0] * 12
}

/// The Generation 3 character set, as far as an English cartridge uses it.
///
/// Unmapped bytes become `?` rather than being dropped, so a read pointed at
/// the wrong address looks wrong instead of looking like a short name.
pub fn decode_text(bytes: &[u8]) -> String {
    let mut text = String::new();
    for &byte in bytes {
        match byte {
            0xFF => break,
            0x00 => text.push(' '),
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
            _ => text.push('?'),
        }
    }
    text.trim().to_string()
}

/// The species index stored in a party or box record, or `None`.
///
/// `None` covers every way this can legitimately not be a Pokémon: an empty
/// slot, a record the game has not finished writing, a checksum that does not
/// match the decrypted block. Returning a wrong species would be worse than
/// returning nothing, because a wrong one still looks like an answer.
pub fn party_species(memory: &dyn MemoryView, at: u32) -> Option<u16> {
    Boxed::read(memory, at).map(|boxed| boxed.species())
}

/// A decrypted record: the plain header, plus the four substructures put back
/// into a fixed order regardless of what the permutation did to them.
///
/// Decrypting is the expensive part — twelve word reads and a checksum — and a
/// card showing four moves, six EVs and six IVs would otherwise pay for it
/// sixteen times over. So a caller decrypts once and then asks for fields.
pub struct Boxed {
    pub personality: u32,
    pub ot_id: u32,
    /// Growth, Attacks, EVs, Misc — always in that order here.
    blocks: [[u8; 12]; 4],
}

impl Boxed {
    /// Read and decrypt the record at `at`, or `None` if it does not hold one.
    ///
    /// The checksum is computed over the decrypted block, so it doubles as
    /// proof that the key was right. That is what makes this safe to point
    /// anywhere: a wrong address gives a wrong key, the checksum fails, and the
    /// answer is "nothing here" rather than plausible-looking nonsense.
    pub fn read(memory: &dyn MemoryView, at: u32) -> Option<Self> {
        let personality = memory.read_u32(at);
        let ot_id = memory.read_u32(at.checked_add(4)?);
        // A slot that never held anything reads as zeroes, and zero is a
        // plausible key — so the pair, not either one, is what says "empty".
        if personality == 0 && ot_id == 0 {
            return None;
        }

        let key = personality ^ ot_id;
        let mut decrypted = [0u8; 48];
        for block in 0..12u32 {
            let word = memory.read_u32(at.checked_add(32 + block * 4)?) ^ key;
            let start = block as usize * 4;
            decrypted[start..start + 4].copy_from_slice(&word.to_le_bytes());
        }

        let stored = memory.read_u16(at.checked_add(28)?);
        let computed = decrypted
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .sum::<u32>() as u16;
        if stored != computed {
            return None;
        }

        let order = SUBSTRUCT_ORDERS[(personality % 24) as usize];
        let mut blocks = [[0u8; 12]; 4];
        for (section, &block) in order.iter().enumerate() {
            blocks[section].copy_from_slice(&decrypted[block * 12..block * 12 + 12]);
        }

        let boxed = Self {
            personality,
            ot_id,
            blocks,
        };
        // Past the last index Generation 3 uses this is not a Pokémon, however
        // well its checksum came out.
        (boxed.species() != 0 && boxed.species() <= 412).then_some(boxed)
    }

    fn byte(&self, section: usize, offset: usize) -> u32 {
        u32::from(self.blocks[section][offset])
    }
    fn half(&self, section: usize, offset: usize) -> u32 {
        u32::from(u16::from_le_bytes([
            self.blocks[section][offset],
            self.blocks[section][offset + 1],
        ]))
    }
    fn word(&self, section: usize, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.blocks[section][offset],
            self.blocks[section][offset + 1],
            self.blocks[section][offset + 2],
            self.blocks[section][offset + 3],
        ])
    }

    pub fn species(&self) -> u16 {
        self.half(0, 0) as u16
    }
    pub fn held_item(&self) -> u16 {
        self.half(0, 2) as u16
    }
    pub fn experience(&self) -> u32 {
        self.word(0, 4)
    }
    pub fn friendship(&self) -> u32 {
        self.byte(0, 9)
    }
    /// The move in slot 0-3, as a move index.
    pub fn move_id(&self, slot: usize) -> u16 {
        self.half(1, slot * 2) as u16
    }
    /// Remaining PP for slot 0-3. The maximum is a property of the move, which
    /// lives in a ROM table this does not read.
    pub fn pp(&self, slot: usize) -> u32 {
        self.byte(1, 8 + slot)
    }
    pub fn ev(&self, stat: Stat) -> u32 {
        self.byte(2, stat as usize)
    }
    pub fn ev_total(&self) -> u32 {
        (0..6).map(|offset| self.byte(2, offset)).sum()
    }
    /// Six five-bit values and two flags, packed into one word.
    fn iv_word(&self) -> u32 {
        self.word(3, 4)
    }
    pub fn iv(&self, stat: Stat) -> u32 {
        (self.iv_word() >> (stat as u32 * 5)) & 0x1F
    }
    pub fn iv_total(&self) -> u32 {
        Stat::ALL.iter().map(|&stat| self.iv(stat)).sum()
    }
    pub fn is_egg(&self) -> bool {
        (self.iv_word() >> 30) & 1 == 1
    }
    /// 0 or 1: which of the species' two abilities this one has. Turning that
    /// into a name needs a species table this crate does not carry.
    pub fn ability_slot(&self) -> u32 {
        (self.iv_word() >> 31) & 1
    }
    pub fn pokerus(&self) -> u32 {
        self.byte(3, 0)
    }
    pub fn met_location(&self) -> u32 {
        self.byte(3, 1)
    }
    /// Nature is not stored anywhere. It *is* the personality, mod 25.
    pub fn nature(&self) -> u32 {
        self.personality % 25
    }
    /// Shiny is the trainer and the Pokémon agreeing by chance.
    pub fn is_shiny(&self) -> bool {
        let fold = |value: u32| (value >> 16) ^ (value & 0xFFFF);
        fold(self.ot_id) ^ fold(self.personality) < 8
    }
}

/// The order Generation 3 stores stats in, which is not the order it shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    Hp = 0,
    Attack = 1,
    Defense = 2,
    Speed = 3,
    SpAttack = 4,
    SpDefense = 5,
}

impl Stat {
    pub const ALL: [Stat; 6] = [
        Stat::Hp,
        Stat::Attack,
        Stat::Defense,
        Stat::Speed,
        Stat::SpAttack,
        Stat::SpDefense,
    ];
}

/// The twenty-five natures, in personality order.
pub const NATURES: [&str; 25] = [
    "Hardy", "Lonely", "Brave", "Adamant", "Naughty", "Bold", "Docile", "Relaxed", "Impish", "Lax",
    "Timid", "Hasty", "Serious", "Jolly", "Naive", "Modest", "Mild", "Quiet", "Bashful", "Rash",
    "Calm", "Gentle", "Sassy", "Careful", "Quirky",
];

/// How far the Hoenn species sit past the National Dex numbering.
///
/// Generation 3 indexes species internally: 1..=251 are Kanto and Johto in Dex
/// order, then twenty-five unused slots, then Hoenn. A sprite named by the
/// internal index would be the wrong Pokémon for every Hoenn species.
const HOENN_INTERNAL_OFFSET: u16 = 25;

/// The National Dex number for an internal species index.
pub fn national_dex_number(species: u16) -> Option<u16> {
    match species {
        1..=251 => Some(species),
        277..=411 => Some(species - HOENN_INTERNAL_OFFSET),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SparseMemory;

    /// Build a record the way the game does, so the decoder is tested against
    /// the encryption rather than against its own inverse written twice.
    fn record(personality: u32, ot_id: u32, species: u16, nickname: &[u8]) -> Vec<u8> {
        let mut raw = vec![0u8; PARTY_RECORD_BYTES as usize];
        raw[0..4].copy_from_slice(&personality.to_le_bytes());
        raw[4..8].copy_from_slice(&ot_id.to_le_bytes());
        raw[8..8 + nickname.len()].copy_from_slice(nickname);

        let mut plain = [0u8; 48];
        let growth = growth_offset(personality);
        plain[growth..growth + 2].copy_from_slice(&species.to_le_bytes());

        let checksum = plain
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .sum::<u32>() as u16;
        raw[28..30].copy_from_slice(&checksum.to_le_bytes());

        let key = personality ^ ot_id;
        for (block, chunk) in plain.chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(chunk.try_into().unwrap()) ^ key;
            raw[32 + block * 4..36 + block * 4].copy_from_slice(&word.to_le_bytes());
        }
        raw
    }

    #[test]
    fn text_decodes_the_games_own_alphabet_rather_than_ascii() {
        // "SPEAROW" in Generation 3 bytes, terminated.
        let bytes = [0xCD, 0xCA, 0xBF, 0xBB, 0xCC, 0xC9, 0xD1, 0xFF, 0x00, 0x00];
        assert_eq!(decode_text(&bytes), "SPEAROW");
        assert_eq!(decode_text(&[0xFF]), "");
        // Read at the wrong address it must look wrong, not look short.
        assert_eq!(decode_text(&[0x10, 0x11]), "??");
    }

    /// Every permutation, because the whole difficulty of this format is that
    /// the same species sits in a different block depending on personality.
    #[test]
    fn species_survives_every_substructure_permutation() {
        for personality in 0..24u32 {
            let memory = SparseMemory::new().with(
                0x0202_0000,
                record(personality, 0x1234_5678, 21, &[0xCD, 0xFF]),
            );
            assert_eq!(
                party_species(&memory, 0x0202_0000),
                Some(21),
                "personality {personality}"
            );
        }
    }

    #[test]
    fn an_empty_slot_and_a_wrong_address_both_report_nothing() {
        let empty = SparseMemory::new().with(0x0202_0000, vec![0u8; 100]);
        assert_eq!(party_species(&empty, 0x0202_0000), None);

        // Plausible-looking noise with no matching checksum: the key was wrong,
        // so there is no species here to report.
        let noise = SparseMemory::new().with(0x0202_0000, vec![0x5Au8; 100]);
        assert_eq!(party_species(&noise, 0x0202_0000), None);
    }

    #[test]
    fn hoenn_species_map_past_the_gap_in_the_internal_numbering() {
        assert_eq!(national_dex_number(21), Some(21));
        assert_eq!(national_dex_number(277), Some(252));
        assert_eq!(national_dex_number(260), None);
    }
}

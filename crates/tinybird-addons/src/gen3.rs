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
    let personality = memory.read_u32(at);
    let ot_id = memory.read_u32(at.checked_add(4)?);
    // A slot that has never held anything reads as zeroes, and zero is a
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

    // The checksum is over the decrypted block, so it doubles as proof that
    // the key was right — which is what stops a misaimed address reporting a
    // confident species from whatever bytes happened to be there.
    let stored = memory.read_u16(at.checked_add(28)?);
    let computed = decrypted
        .chunks_exact(2)
        .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
        .sum::<u32>() as u16;
    if stored != computed {
        return None;
    }

    let growth = growth_offset(personality);
    let species = u16::from_le_bytes([decrypted[growth], decrypted[growth + 1]]);
    // 412 is one past the last index Generation 3 uses; beyond it the record
    // is not a Pokémon however well its checksum came out.
    (species != 0 && species <= 412).then_some(species)
}

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

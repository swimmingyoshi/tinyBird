//! Final Fantasy Tactics Advance text decoding.
//!
//! FFTA does not store text as ASCII. Letters live in one contiguous block of
//! byte values, which is why searching the ROM for `Marche` finds nothing.
//!
//! The mapping was derived from the ROM by relative search — looking for byte
//! runs whose consecutive differences match those of a known word — and then
//! confirmed against the job and character name tables, which decode cleanly
//! into `Soldier`, `Thief`, `Fighter`, `Montblanc`, `Marche`, and so on.
//!
//! Two forms appear:
//!
//! | form | `A` | `a` | used for |
//! |---|---|---|---|
//! | plain, one byte per character | `0xB1` | `0xCB` | names, short labels |
//! | escaped, `0x80` then one byte | `0xB0` | `0xCA` | menu strings with spaces |
//!
//! The escaped form is offset by one from the plain form, and uses the two-byte
//! sequence `0x40 0x73` for a space. Both are handled here because job names mix
//! them: `Beastmaster` is plain while `White Mage` is escaped.

/// Byte value of `A` in the plain, single-byte form.
const PLAIN_UPPER_A: u8 = 0xB1;
/// Byte value of `a` in the plain, single-byte form.
const PLAIN_LOWER_A: u8 = 0xCB;
/// Byte value of `A` in the `0x80`-escaped form.
const WIDE_UPPER_A: u8 = 0xB0;
/// Byte value of `a` in the `0x80`-escaped form.
const WIDE_LOWER_A: u8 = 0xCA;

/// Introduces one escaped character.
const ESCAPE: u8 = 0x80;
/// First byte of the two-byte space sequence in escaped text.
const SPACE_LEAD: u8 = 0x40;
/// Second byte of the two-byte space sequence in escaped text.
const SPACE_TAIL: u8 = 0x73;
/// Ends a string.
const TERMINATOR: u8 = 0x00;

/// Decode one plain-form byte.
fn decode_plain(byte: u8) -> Option<char> {
    match byte {
        b if (PLAIN_UPPER_A..PLAIN_UPPER_A + 26).contains(&b) => {
            Some((b'A' + (b - PLAIN_UPPER_A)) as char)
        }
        b if (PLAIN_LOWER_A..PLAIN_LOWER_A + 26).contains(&b) => {
            Some((b'a' + (b - PLAIN_LOWER_A)) as char)
        }
        _ => None,
    }
}

/// Decode one escaped-form byte (the one after `0x80`).
fn decode_wide(byte: u8) -> Option<char> {
    match byte {
        b if (WIDE_UPPER_A..WIDE_UPPER_A + 26).contains(&b) => {
            Some((b'A' + (b - WIDE_UPPER_A)) as char)
        }
        b if (WIDE_LOWER_A..WIDE_LOWER_A + 26).contains(&b) => {
            Some((b'a' + (b - WIDE_LOWER_A)) as char)
        }
        _ => None,
    }
}

/// Encode a string in the plain, single-byte form.
///
/// Used to search memory for a known name, which is how the unit table was
/// located in the first place. Characters the form cannot represent are
/// skipped rather than substituted, so a search pattern never contains a byte
/// that could not really appear.
///
/// The addon itself only decodes, so nothing in the shipping build calls this.
/// It stays because the round-trip test against real ROM bytes is what proves
/// the derived table is right — if that ever fails, every FFTA memory search
/// would silently find nothing.
#[allow(dead_code)]
pub fn encode_plain(text: &str) -> Vec<u8> {
    text.chars()
        .filter_map(|ch| match ch {
            'A'..='Z' => Some(PLAIN_UPPER_A + (ch as u8 - b'A')),
            'a'..='z' => Some(PLAIN_LOWER_A + (ch as u8 - b'a')),
            _ => None,
        })
        .collect()
}

/// Decode an FFTA string from `bytes`, stopping at the terminator, at the end
/// of the slice, or at the first byte that is not text.
///
/// Stopping on unknown bytes is deliberate: when a pointer is wrong the result
/// is a short or empty string rather than a plausible-looking line of garbage,
/// which makes a bad address obvious instead of subtly wrong.
pub fn decode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];

        if byte == TERMINATOR {
            break;
        }

        if byte == ESCAPE {
            let Some(&next) = bytes.get(index + 1) else {
                break;
            };
            let Some(ch) = decode_wide(next) else {
                break;
            };
            text.push(ch);
            index += 2;
            continue;
        }

        if byte == SPACE_LEAD && bytes.get(index + 1) == Some(&SPACE_TAIL) {
            text.push(' ');
            index += 2;
            continue;
        }

        let Some(ch) = decode_plain(byte) else {
            break;
        };
        text.push(ch);
        index += 1;
    }

    text.trim().to_string()
}

/// Whether a decoded string looks like a real name rather than a coincidence.
///
/// A unit name that came from the right address is short, non-empty, and starts
/// with a letter. Requiring this stops a plausible-looking two-character
/// fragment from being reported as a party member.
pub fn looks_like_name(text: &str) -> bool {
    let length = text.chars().count();
    (2..=16).contains(&length)
        && text.starts_with(|c: char| c.is_ascii_alphabetic())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes taken verbatim from the US ROM's character name table at 0x5512A0.
    const MARCHE_IN_ROM: [u8; 7] = [0xBD, 0xCB, 0xDC, 0xCD, 0xD2, 0xCF, 0x00];
    /// From the same table at 0x5512C8.
    const MONTBLANC_IN_ROM: [u8; 10] = [0xBD, 0xD9, 0xD8, 0xDE, 0xCC, 0xD6, 0xCB, 0xD8, 0xCD, 0x00];
    /// From the job table at 0x5234B4.
    const SOLDIER_IN_ROM: [u8; 8] = [0xC3, 0xD9, 0xD6, 0xCE, 0xD3, 0xCF, 0xDC, 0x00];
    /// From the job table at 0x523486, in the escaped form with a space.
    const WHITE_MAGE_IN_ROM: [u8; 21] = [
        0x80, 0xC6, 0x80, 0xD1, 0x80, 0xD2, 0x80, 0xDD, 0x80, 0xCE, 0x40, 0x73, 0x80, 0xBC, 0x80,
        0xCA, 0x80, 0xD0, 0x80, 0xCE, 0x00,
    ];

    #[test]
    fn plain_names_from_the_rom_decode_correctly() {
        assert_eq!(decode(&MARCHE_IN_ROM), "Marche");
        assert_eq!(decode(&MONTBLANC_IN_ROM), "Montblanc");
        assert_eq!(decode(&SOLDIER_IN_ROM), "Soldier");
    }

    #[test]
    fn escaped_text_with_a_space_decodes_correctly() {
        assert_eq!(decode(&WHITE_MAGE_IN_ROM), "White Mage");
    }

    #[test]
    fn encoding_round_trips_through_decoding() {
        for name in ["Marche", "Montblanc", "Ritz", "Soldier", "Beastmaster"] {
            assert_eq!(decode(&encode_plain(name)), name, "round trip for {name}");
        }
    }

    #[test]
    fn encode_plain_matches_the_bytes_actually_in_the_rom() {
        // If this ever fails, the derived table is wrong and every FFTA memory
        // search built on it would silently find nothing.
        assert_eq!(encode_plain("Marche"), MARCHE_IN_ROM[..6].to_vec());
        assert_eq!(encode_plain("Soldier"), SOLDIER_IN_ROM[..7].to_vec());
    }

    #[test]
    fn decoding_stops_at_the_terminator() {
        let mut bytes = encode_plain("Ritz");
        bytes.push(TERMINATOR);
        bytes.extend(encode_plain("Ignored"));
        assert_eq!(decode(&bytes), "Ritz");
    }

    #[test]
    fn decoding_stops_at_the_first_byte_that_is_not_text() {
        let mut bytes = encode_plain("Ok");
        bytes.push(0x1F);
        bytes.extend(encode_plain("Nope"));
        assert_eq!(decode(&bytes), "Ok");
    }

    #[test]
    fn a_truncated_escape_does_not_panic() {
        assert_eq!(decode(&[ESCAPE]), "");
        assert_eq!(decode(&[0xBD, 0xCB, ESCAPE]), "Ma");
    }

    #[test]
    fn empty_input_decodes_to_an_empty_string() {
        assert_eq!(decode(&[]), "");
        assert_eq!(decode(&[TERMINATOR]), "");
    }

    #[test]
    fn encode_skips_characters_the_table_cannot_represent() {
        // A search pattern must never contain a byte that could not really
        // appear, or it would match nothing for a reason that looks like a bug.
        assert_eq!(encode_plain("Bob-2"), encode_plain("Bob"));
    }

    #[test]
    fn name_plausibility_rejects_fragments_and_garbage() {
        assert!(looks_like_name("Marche"));
        assert!(looks_like_name("White Mage"));

        assert!(!looks_like_name(""), "empty");
        assert!(!looks_like_name("M"), "single character");
        assert!(!looks_like_name("2Fast"), "does not start with a letter");
        assert!(
            !looks_like_name("AVeryLongNameIndeedYes"),
            "longer than any FFTA name field"
        );
    }

    #[test]
    fn the_two_forms_are_offset_by_exactly_one() {
        // Documents the relationship the decoder relies on; if a future region
        // dump disagrees, this is the assumption to revisit.
        assert_eq!(PLAIN_UPPER_A - WIDE_UPPER_A, 1);
        assert_eq!(PLAIN_LOWER_A - WIDE_LOWER_A, 1);
    }
}

//! Text encodings the probe can search with.
//!
//! Searching for a name you can see on screen is the fastest way into an
//! unfamiliar game's memory, but only if you encode it the way the game does.
//! Very few GBA games store plain ASCII.

/// Byte value of `A` in Final Fantasy Tactics Advance's plain text form.
const FFTA_UPPER_A: u8 = 0xB1;
/// Byte value of `a` in the same form.
const FFTA_LOWER_A: u8 = 0xCB;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Final Fantasy Tactics Advance: letters in one contiguous block.
    Ffta,
    /// Plain ASCII, for games that store text unencoded.
    Ascii,
}

impl Codec {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            "ffta" => Ok(Codec::Ffta),
            "ascii" | "raw" => Ok(Codec::Ascii),
            other => Err(format!("unknown codec '{other}' (try ffta, ascii)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::Ffta => "ffta",
            Codec::Ascii => "ascii",
        }
    }

    /// Encode `text` into a search pattern.
    ///
    /// Characters the encoding cannot represent are dropped rather than
    /// substituted: a pattern containing a byte that could never appear would
    /// match nothing, and the empty result would look like a bug in the search.
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Codec::Ascii => text.bytes().collect(),
            Codec::Ffta => text
                .chars()
                .filter_map(|ch| match ch {
                    'A'..='Z' => Some(FFTA_UPPER_A + (ch as u8 - b'A')),
                    'a'..='z' => Some(FFTA_LOWER_A + (ch as u8 - b'a')),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Decode a run of bytes, stopping at the first byte that is not text.
    pub fn decode(self, bytes: &[u8]) -> String {
        let mut text = String::new();
        for &byte in bytes {
            match self.decode_byte(byte) {
                Some(ch) => text.push(ch),
                None => break,
            }
        }
        text
    }

    /// Decode for display, substituting `.` for anything unprintable so a hex
    /// dump keeps its column alignment.
    pub fn decode_lossy(self, bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|&byte| self.decode_byte(byte).unwrap_or('.'))
            .collect()
    }

    fn decode_byte(self, byte: u8) -> Option<char> {
        match self {
            Codec::Ascii => (byte.is_ascii_graphic() || byte == b' ').then_some(byte as char),
            Codec::Ffta => match byte {
                b if (FFTA_UPPER_A..FFTA_UPPER_A + 26).contains(&b) => {
                    Some((b'A' + (b - FFTA_UPPER_A)) as char)
                }
                b if (FFTA_LOWER_A..FFTA_LOWER_A + 26).contains(&b) => {
                    Some((b'a' + (b - FFTA_LOWER_A)) as char)
                }
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Marche" as it appears in the US ROM's name table at 0x5512A0.
    const MARCHE: [u8; 6] = [0xBD, 0xCB, 0xDC, 0xCD, 0xD2, 0xCF];

    #[test]
    fn ffta_encoding_matches_the_bytes_in_the_rom() {
        assert_eq!(Codec::Ffta.encode("Marche"), MARCHE.to_vec());
    }

    #[test]
    fn ffta_round_trips() {
        assert_eq!(Codec::Ffta.decode(&Codec::Ffta.encode("Montblanc")), "Montblanc");
    }

    #[test]
    fn ascii_round_trips() {
        assert_eq!(Codec::Ascii.decode(&Codec::Ascii.encode("PARTY")), "PARTY");
    }

    #[test]
    fn unencodable_characters_are_dropped_not_substituted() {
        assert_eq!(Codec::Ffta.encode("Clan Nutsy!"), Codec::Ffta.encode("ClanNutsy"));
        assert!(Codec::Ffta.encode("123").is_empty());
    }

    #[test]
    fn lossy_decoding_keeps_one_character_per_byte_for_hex_dumps() {
        let bytes = [0xBD, 0x00, 0xCB, 0xFF];
        let text = Codec::Ffta.decode_lossy(&bytes);
        assert_eq!(text.chars().count(), bytes.len());
        assert_eq!(text, "M.a.");
    }

    #[test]
    fn strict_decoding_stops_at_the_first_non_text_byte() {
        assert_eq!(Codec::Ffta.decode(&[0xBD, 0xCB, 0x00, 0xCB]), "Ma");
    }

    #[test]
    fn codec_names_parse_case_insensitively() {
        assert_eq!(Codec::parse("FFTA").unwrap(), Codec::Ffta);
        assert_eq!(Codec::parse("Ascii").unwrap(), Codec::Ascii);
        assert!(Codec::parse("shiftjis").is_err());
    }
}

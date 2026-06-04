//! Game-specific addon hooks for stream overlays, tools, and future web clients.
//!
//! Add new per-game integrations by implementing `GameAddon` and registering
//! the addon in `detect_addon_snapshot`.

use serde::Serialize;
use std::fs;
use std::path::Path;
use tinybird_core::Gba;

pub const STREAM_SNAPSHOT_PATH: &str = "stream-data/current-game.json";

const ROM_HEADER_BASE: u32 = 0x0800_0000;
const PARTY_SLOT_SIZE: usize = 100;
const PARTY_SLOT_COUNT: usize = 6;
const FIRE_RED_PARTY_COUNT_ADDR: u32 = 0x0202_402A;
const FIRE_RED_PARTY_BASE: u32 = 0x0202_4284;
const FIRE_RED_PARTY_SCAN_START: u32 = 0x0202_3000;
const FIRE_RED_PARTY_SCAN_END: u32 = 0x0202_6000;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StreamSnapshot {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rom: Option<RomIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addon: Option<AddonSnapshot>,
}

impl Default for StreamSnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            rom: None,
            addon: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RomIdentity {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub revision: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonSnapshot {
    pub addon_id: &'static str,
    pub display_name: &'static str,
    pub overlay_lines: Vec<String>,
    pub data: AddonData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AddonData {
    FireRed(FireRedSnapshot),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedSnapshot {
    pub source: &'static str,
    pub party_base_address: u32,
    pub party: Vec<FireRedPartyMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedPartyMember {
    pub slot: u8,
    pub nickname: String,
    pub species_id: u16,
    pub personality: u32,
    pub ot_id: u32,
    pub level: u8,
    pub current_hp: u16,
    pub max_hp: u16,
    pub status: u32,
    pub is_egg: bool,
    pub moves: [u16; 4],
}

trait GameAddon {
    fn addon_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn supports(&self, rom: &RomIdentity) -> bool;
    fn snapshot(&self, gba: &Gba, rom: &RomIdentity) -> Option<AddonSnapshot>;
}

pub fn capture_stream_snapshot(gba: Option<&Gba>) -> StreamSnapshot {
    let Some(gba) = gba else {
        return StreamSnapshot::default();
    };

    let Some(rom) = RomIdentity::from_gba(gba) else {
        return StreamSnapshot::default();
    };

    let addon = detect_addon_snapshot(gba, &rom);
    StreamSnapshot {
        schema_version: 1,
        rom: Some(rom),
        addon,
    }
}

pub fn write_stream_snapshot(snapshot: &StreamSnapshot, previous_json: &mut Option<String>) {
    let Ok(json) = serde_json::to_string_pretty(snapshot) else {
        return;
    };
    if previous_json.as_deref() == Some(json.as_str()) {
        return;
    }

    let path = Path::new(STREAM_SNAPSHOT_PATH);
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!(
                "Failed to create addon export directory '{}': {}",
                parent.display(),
                err
            );
            return;
        }
    }

    match fs::write(path, &json) {
        Ok(()) => *previous_json = Some(json),
        Err(err) => eprintln!(
            "Failed to write addon snapshot '{}': {}",
            path.display(),
            err
        ),
    }
}

impl RomIdentity {
    fn from_gba(gba: &Gba) -> Option<Self> {
        let title = read_header_ascii(gba, 0xA0, 12);
        let game_code = read_header_ascii(gba, 0xAC, 4);
        if title.is_empty() && game_code.is_empty() {
            return None;
        }

        Some(Self {
            title,
            game_code,
            maker_code: read_header_ascii(gba, 0xB0, 2),
            revision: gba.read_u8(ROM_HEADER_BASE + 0xBC),
        })
    }
}

fn detect_addon_snapshot(gba: &Gba, rom: &RomIdentity) -> Option<AddonSnapshot> {
    let fire_red = FireRedAddon;
    let addons: [&dyn GameAddon; 1] = [&fire_red];

    for addon in addons {
        if addon.supports(rom) {
            if let Some(snapshot) = addon.snapshot(gba, rom) {
                return Some(snapshot);
            }
        }
    }

    None
}

fn read_header_ascii(gba: &Gba, offset: u32, len: usize) -> String {
    let mut bytes = Vec::with_capacity(len);
    for idx in 0..len {
        let byte = gba.read_u8(ROM_HEADER_BASE + offset + idx as u32);
        if byte == 0 || byte == 0xFF {
            break;
        }
        let normalized = if byte.is_ascii_graphic() || byte == b' ' {
            byte
        } else {
            b'?'
        };
        bytes.push(normalized);
    }

    String::from_utf8_lossy(&bytes).trim().to_string()
}

struct FireRedAddon;

impl GameAddon for FireRedAddon {
    fn addon_id(&self) -> &'static str {
        "pokemon_firered_party"
    }

    fn display_name(&self) -> &'static str {
        "FireRed Party"
    }

    fn supports(&self, rom: &RomIdentity) -> bool {
        rom.game_code.starts_with("BPR") || rom.title.eq_ignore_ascii_case("POKEMON FIRE")
    }

    fn snapshot(&self, gba: &Gba, _rom: &RomIdentity) -> Option<AddonSnapshot> {
        let (party_base_address, party) = locate_fire_red_party(gba)?;

        let mut overlay_lines = Vec::with_capacity(party.len() + 1);
        overlay_lines.push(format!("Party: {} / {}", party.len(), PARTY_SLOT_COUNT));
        overlay_lines.extend(party.iter().map(format_party_line));

        Some(AddonSnapshot {
            addon_id: self.addon_id(),
            display_name: self.display_name(),
            overlay_lines,
            data: AddonData::FireRed(FireRedSnapshot {
                source: "live_memory",
                party_base_address,
                party,
            }),
        })
    }
}

fn locate_fire_red_party(gba: &Gba) -> Option<(u32, Vec<FireRedPartyMember>)> {
    let expected_party_count = read_party_count(gba);
    let mut best_candidate =
        scan_party_candidate_at(gba, FIRE_RED_PARTY_BASE, expected_party_count);

    let last_candidate =
        FIRE_RED_PARTY_SCAN_END.saturating_sub((PARTY_SLOT_SIZE * PARTY_SLOT_COUNT) as u32);
    for base in (FIRE_RED_PARTY_SCAN_START..=last_candidate).step_by(4) {
        let Some(candidate) = scan_party_candidate_at(gba, base, expected_party_count) else {
            continue;
        };
        if best_candidate
            .as_ref()
            .is_none_or(|current| candidate_better_than(&candidate, current))
        {
            best_candidate = Some(candidate);
        }
    }

    best_candidate.map(|candidate| (candidate.base_address, candidate.party))
}

fn read_party_count(gba: &Gba) -> Option<usize> {
    let count = gba.read_u8(FIRE_RED_PARTY_COUNT_ADDR) as usize;
    (1..=PARTY_SLOT_COUNT).contains(&count).then_some(count)
}

fn parse_party_at(gba: &Gba, base: u32, party_count: usize) -> Option<Vec<FireRedPartyMember>> {
    let mut party = Vec::with_capacity(party_count);
    for slot in 0..party_count {
        let raw = read_party_slot(gba, base + (slot * PARTY_SLOT_SIZE) as u32);
        let Some(member) = parse_party_member(&raw, slot as u8 + 1) else {
            return None;
        };
        party.push(member);
    }

    Some(party)
}

fn parse_party_prefix_at(gba: &Gba, base: u32) -> Option<Vec<FireRedPartyMember>> {
    let mut party = Vec::with_capacity(PARTY_SLOT_COUNT);
    for slot in 0..PARTY_SLOT_COUNT {
        let raw = read_party_slot(gba, base + (slot * PARTY_SLOT_SIZE) as u32);
        let Some(member) = parse_party_member(&raw, slot as u8 + 1) else {
            break;
        };
        party.push(member);
    }

    (!party.is_empty()).then_some(party)
}

#[derive(Clone, Debug)]
struct PartyCandidate {
    base_address: u32,
    party: Vec<FireRedPartyMember>,
    matched_expected_count: bool,
}

fn scan_party_candidate_at(
    gba: &Gba,
    base: u32,
    expected_party_count: Option<usize>,
) -> Option<PartyCandidate> {
    let prefix_party = parse_party_prefix_at(gba, base)?;
    let exact_party = expected_party_count.and_then(|count| parse_party_at(gba, base, count));

    let (party, matched_expected_count) = match exact_party {
        Some(party) if party.len() >= prefix_party.len() => (party, true),
        _ => (prefix_party, false),
    };

    Some(PartyCandidate {
        base_address: base,
        party,
        matched_expected_count,
    })
}

fn candidate_better_than(candidate: &PartyCandidate, current: &PartyCandidate) -> bool {
    if candidate.party.len() != current.party.len() {
        return candidate.party.len() > current.party.len();
    }

    if candidate.matched_expected_count != current.matched_expected_count {
        return candidate.matched_expected_count;
    }

    let candidate_distance = candidate.base_address.abs_diff(FIRE_RED_PARTY_BASE);
    let current_distance = current.base_address.abs_diff(FIRE_RED_PARTY_BASE);
    if candidate_distance != current_distance {
        return candidate_distance < current_distance;
    }

    candidate.base_address < current.base_address
}

fn read_party_slot(gba: &Gba, addr: u32) -> [u8; PARTY_SLOT_SIZE] {
    let mut raw = [0u8; PARTY_SLOT_SIZE];
    for (idx, byte) in raw.iter_mut().enumerate() {
        *byte = gba.read_u8(addr + idx as u32);
    }
    raw
}

fn parse_party_member(raw: &[u8; PARTY_SLOT_SIZE], slot: u8) -> Option<FireRedPartyMember> {
    let personality = u32::from_le_bytes(raw[0..4].try_into().ok()?);
    let ot_id = u32::from_le_bytes(raw[4..8].try_into().ok()?);
    if personality == 0 && ot_id == 0 {
        return None;
    }

    let decrypted = decrypt_secure_data(raw, personality ^ ot_id);
    let stored_checksum = u16::from_le_bytes(raw[28..30].try_into().ok()?);
    let computed_checksum = compute_checksum(&decrypted);
    if stored_checksum != computed_checksum {
        return None;
    }

    let sections = unpack_secure_sections(&decrypted, personality);

    let growth = sections[0];
    let attacks = sections[1];
    let species_id = u16::from_le_bytes(growth[0..2].try_into().ok()?);
    if species_id == 0 || species_id > 440 {
        return None;
    }

    let flags = raw[19];
    if flags & 0x2 == 0 {
        return None;
    }
    let is_egg = flags & 0x4 != 0;
    let level = raw[84];
    let current_hp = u16::from_le_bytes(raw[86..88].try_into().ok()?);
    let max_hp = u16::from_le_bytes(raw[88..90].try_into().ok()?);
    if !is_egg && (level == 0 || level > 100 || max_hp == 0) {
        return None;
    }

    Some(FireRedPartyMember {
        slot,
        nickname: decode_gen3_text(&raw[8..18]),
        species_id,
        personality,
        ot_id,
        level,
        current_hp,
        max_hp,
        status: u32::from_le_bytes(raw[80..84].try_into().ok()?),
        is_egg,
        moves: [
            u16::from_le_bytes(attacks[0..2].try_into().ok()?),
            u16::from_le_bytes(attacks[2..4].try_into().ok()?),
            u16::from_le_bytes(attacks[4..6].try_into().ok()?),
            u16::from_le_bytes(attacks[6..8].try_into().ok()?),
        ],
    })
}

fn decrypt_secure_data(raw: &[u8; PARTY_SLOT_SIZE], key: u32) -> [u8; 48] {
    let mut decrypted = [0u8; 48];
    for (block_idx, chunk) in raw[32..80].chunks_exact(4).enumerate() {
        let decrypted_word = u32::from_le_bytes(chunk.try_into().unwrap()) ^ key;
        let start = block_idx * 4;
        decrypted[start..start + 4].copy_from_slice(&decrypted_word.to_le_bytes());
    }
    decrypted
}

fn unpack_secure_sections(decrypted: &[u8; 48], personality: u32) -> [[u8; 12]; 4] {
    let order = SUBSTRUCT_ORDERS[(personality % 24) as usize];
    let mut sections = [[0u8; 12]; 4];
    for (section_id, &block_idx) in order.iter().enumerate() {
        let start = block_idx * 12;
        sections[section_id].copy_from_slice(&decrypted[start..start + 12]);
    }
    sections
}

fn compute_checksum(data: &[u8; 48]) -> u16 {
    let sum: u32 = data
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]) as u32)
        .sum();
    (sum & 0xFFFF) as u16
}

fn decode_gen3_text(bytes: &[u8]) -> String {
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

    let trimmed = text.trim();
    if trimmed.is_empty() {
        "UNKNOWN".to_string()
    } else {
        trimmed.to_string()
    }
}

fn format_party_line(member: &FireRedPartyMember) -> String {
    let nickname = short_name(&member.nickname, 10);
    if member.is_egg {
        format!("{}. {} EGG", member.slot, nickname)
    } else {
        format!(
            "{}. {} Lv{} {}/{}",
            member.slot, nickname, member.level, member.current_hp, member.max_hp
        )
    }
}

fn short_name(name: &str, max_chars: usize) -> String {
    let mut chars = name.chars();
    let mut shortened = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return name.to_string();
        };
        shortened.push(ch);
    }

    if chars.next().is_some() && max_chars >= 3 {
        shortened.truncate(max_chars - 3);
        shortened.push_str("...");
    }

    shortened
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tinybird_core::Gba;

    #[test]
    fn test_decode_gen3_text_basic_ascii() {
        let bytes = encode_gen3_text("AB12");
        assert_eq!(decode_gen3_text(&bytes), "AB12");
    }

    #[test]
    fn test_parse_party_member() {
        let raw = build_party_member_raw(0x0000_0001, 0x0000_0001, "PIKACHU", 25, 25, 58, 58);

        let member = parse_party_member(&raw, 1).expect("valid party member");
        assert_eq!(member.slot, 1);
        assert_eq!(member.nickname, "PIKACHU");
        assert_eq!(member.species_id, 25);
        assert_eq!(member.level, 25);
        assert_eq!(member.current_hp, 58);
        assert_eq!(member.max_hp, 58);
        assert_eq!(member.moves[0], 33);
        assert_eq!(member.moves[1], 45);
    }

    #[test]
    fn test_parse_party_member_with_non_symmetric_substruct_order() {
        let raw = build_party_member_raw(12, 0xA5A5_5A5A, "SQUIRTLE", 7, 10, 29, 29);

        let member = parse_party_member(&raw, 1).expect("valid party member");
        assert_eq!(member.nickname, "SQUIRTLE");
        assert_eq!(member.species_id, 7);
        assert_eq!(member.level, 10);
        assert_eq!(member.current_hp, 29);
        assert_eq!(member.max_hp, 29);
        assert_eq!(member.moves[0], 33);
        assert_eq!(member.moves[1], 45);
    }

    #[test]
    fn test_locate_fire_red_party_without_party_count_address() {
        let mut gba = Gba::new();
        write_party_slot(
            &mut gba,
            FIRE_RED_PARTY_BASE,
            &build_party_member_raw(0x1234_5678, 0x9ABC_DEF0, "BULBASAUR", 1, 5, 20, 20),
        );
        write_party_slot(
            &mut gba,
            FIRE_RED_PARTY_BASE + PARTY_SLOT_SIZE as u32,
            &build_party_member_raw(0x89AB_CDEF, 0x0123_4567, "PIDGEY", 16, 3, 14, 14),
        );

        let (base, party) = locate_fire_red_party(&gba).expect("party should be found");
        assert_eq!(base, FIRE_RED_PARTY_BASE);
        assert_eq!(party.len(), 2);
        assert_eq!(party[0].nickname, "BULBASAUR");
        assert_eq!(party[1].nickname, "PIDGEY");
    }

    #[test]
    fn test_locate_fire_red_party_falls_back_when_count_address_is_wrong() {
        let mut gba = Gba::new();
        gba.write_u8(FIRE_RED_PARTY_COUNT_ADDR, 1);
        write_party_slot(
            &mut gba,
            FIRE_RED_PARTY_BASE,
            &build_party_member_raw(0x1111_1111, 0x2222_2222, "CHARMANDR", 4, 8, 24, 24),
        );
        write_party_slot(
            &mut gba,
            FIRE_RED_PARTY_BASE + PARTY_SLOT_SIZE as u32,
            &build_party_member_raw(0x3333_3333, 0x4444_4444, "RATTATA", 19, 6, 18, 18),
        );

        let (_, party) = locate_fire_red_party(&gba).expect("party should be found");
        assert_eq!(party.len(), 2);
        assert_eq!(party[0].nickname, "CHARMANDR");
        assert_eq!(party[1].nickname, "RATTATA");
    }

    #[test]
    #[ignore = "Manual smoke test for local FireRed savestates when debugging addons"]
    fn inspect_local_firered_state_snapshot() {
        let state_path = Path::new("roms/PokemonFireRed.state");
        if !state_path.is_file() {
            return;
        }

        let bytes = std::fs::read(state_path).expect("read local FireRed savestate");
        let mut gba = Gba::new();
        gba.load_state_bytes(&bytes)
            .expect("deserialize local FireRed savestate");

        let snapshot = capture_stream_snapshot(Some(&gba));
        eprintln!("local FireRed snapshot: {snapshot:#?}");
        assert!(
            snapshot.addon.is_some(),
            "expected local FireRed savestate to produce an addon snapshot"
        );
    }

    fn encode_gen3_text(text: &str) -> [u8; 10] {
        let mut out = [0xFF; 10];
        for (idx, ch) in text.chars().take(10).enumerate() {
            out[idx] = match ch {
                'A'..='Z' => 0xBB + (ch as u8 - b'A'),
                'a'..='z' => 0xD5 + (ch as u8 - b'a'),
                '0'..='9' => 0xA1 + (ch as u8 - b'0'),
                '!' => 0xAB,
                '?' => 0xAC,
                '.' => 0xAD,
                '-' => 0xAE,
                '\'' => 0xB4,
                ',' => 0xB8,
                '/' => 0xB9,
                ':' => 0xBA,
                ' ' => 0x00,
                _ => 0x00,
            };
        }
        out
    }

    fn build_party_member_raw(
        personality: u32,
        ot_id: u32,
        nickname: &str,
        species_id: u16,
        level: u8,
        current_hp: u16,
        max_hp: u16,
    ) -> [u8; PARTY_SLOT_SIZE] {
        let mut raw = [0u8; PARTY_SLOT_SIZE];
        raw[0..4].copy_from_slice(&personality.to_le_bytes());
        raw[4..8].copy_from_slice(&ot_id.to_le_bytes());
        raw[8..18].copy_from_slice(&encode_gen3_text(nickname));
        raw[18] = 2;
        raw[19] = 0x2;

        let mut growth = [0u8; 12];
        growth[0..2].copy_from_slice(&species_id.to_le_bytes());
        growth[4..8].copy_from_slice(&12_345u32.to_le_bytes());
        growth[8] = 0;
        growth[9] = 70;

        let mut attacks = [0u8; 12];
        attacks[0..2].copy_from_slice(&33u16.to_le_bytes());
        attacks[2..4].copy_from_slice(&45u16.to_le_bytes());
        attacks[8] = 35;
        attacks[9] = 30;

        let sections = [growth, attacks, [0u8; 12], [0u8; 12]];
        let secure = pack_secure_sections(&sections, personality);

        let checksum = compute_checksum(&secure);
        raw[28..30].copy_from_slice(&checksum.to_le_bytes());

        let key = personality ^ ot_id;
        for (block_idx, chunk) in secure.chunks_exact(4).enumerate() {
            let encrypted_word = u32::from_le_bytes(chunk.try_into().unwrap()) ^ key;
            let start = 32 + block_idx * 4;
            raw[start..start + 4].copy_from_slice(&encrypted_word.to_le_bytes());
        }

        raw[80..84].copy_from_slice(&0u32.to_le_bytes());
        raw[84] = level;
        raw[86..88].copy_from_slice(&current_hp.to_le_bytes());
        raw[88..90].copy_from_slice(&max_hp.to_le_bytes());
        raw
    }

    fn write_party_slot(gba: &mut Gba, addr: u32, raw: &[u8; PARTY_SLOT_SIZE]) {
        for (idx, byte) in raw.iter().enumerate() {
            gba.write_u8(addr + idx as u32, *byte);
        }
    }

    fn pack_secure_sections(sections: &[[u8; 12]; 4], personality: u32) -> [u8; 48] {
        let order = SUBSTRUCT_ORDERS[(personality % 24) as usize];
        let mut secure = [0u8; 48];
        for (section_id, &block_idx) in order.iter().enumerate() {
            let start = block_idx * 12;
            secure[start..start + 12].copy_from_slice(&sections[section_id]);
        }
        secure
    }
}

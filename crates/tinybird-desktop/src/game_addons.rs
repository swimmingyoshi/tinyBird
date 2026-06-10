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
const FIRE_RED_BATTLE_TYPE_FLAGS_ADDR: u32 = 0x0202_2B4C;
const FIRE_RED_BATTLE_OUTCOME_ADDR: u32 = 0x0202_3E8A;
const FIRE_RED_ENEMY_PARTY_BASE: u32 = 0x0202_402C;
const FIRE_RED_PARTY_COUNT_ADDR: u32 = 0x0202_402A;
const FIRE_RED_PARTY_BASE: u32 = 0x0202_4284;
const FIRE_RED_PARTY_SCAN_START: u32 = 0x0202_3000;
const FIRE_RED_PARTY_SCAN_END: u32 = 0x0202_6000;
const FIRE_RED_SAVE_BLOCK1_PTR_ADDR: u32 = 0x0300_5008;
const SAVE_BLOCK1_LOCATION_OFFSET: u32 = 0x0004;
const WARP_DATA_MAP_GROUP_OFFSET: u32 = 0;
const WARP_DATA_MAP_NUM_OFFSET: u32 = 1;
const BATTLE_TYPE_LINK: u32 = 1 << 1;
const BATTLE_TYPE_TRAINER: u32 = 1 << 3;
const BATTLE_TYPE_SAFARI: u32 = 1 << 7;
const BATTLE_TYPE_OLD_MAN_TUTORIAL: u32 = 1 << 9;
const BATTLE_TYPE_ROAMER: u32 = 1 << 10;
const BATTLE_TYPE_LEGENDARY: u32 = 1 << 13;
const BATTLE_TYPE_GHOST: u32 = 1 << 15;
const BATTLE_TYPE_POKEDUDE: u32 = 1 << 16;
const BATTLE_TYPE_LEGENDARY_FRLG: u32 = 1 << 18;
const BATTLE_OUTCOME_NONE: u8 = 0;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area: Option<FireRedAreaSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battle: Option<FireRedBattleSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedPartyMember {
    pub slot: u8,
    pub nickname: String,
    pub species_id: u16,
    pub species_name: String,
    pub personality: u32,
    pub ot_id: u32,
    pub level: u8,
    pub current_hp: u16,
    pub max_hp: u16,
    pub status: u32,
    pub is_egg: bool,
    pub nature: &'static str,
    pub ability_slot: u8,
    pub ability_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_item: Option<FireRedHeldItem>,
    pub stats: FireRedStatSpread,
    pub evs: FireRedStatSpread,
    pub ivs: FireRedStatSpread,
    pub ev_total: u16,
    pub iv_total: u16,
    pub moves: [u16; 4],
    pub move_slots: Vec<FireRedMoveSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedHeldItem {
    pub item_id: u16,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedMoveSlot {
    pub slot: u8,
    pub move_id: u16,
    pub name: String,
    pub pp: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedStatSpread {
    pub hp: u16,
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub sp_attack: u16,
    pub sp_def: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedAreaSnapshot {
    pub map_group: u8,
    pub map_num: u8,
    pub map_key: String,
    pub name: String,
    pub encounter_groups: Vec<FireRedEncounterGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedBattleSnapshot {
    pub battle_type_flags: u32,
    pub battle_kind: &'static str,
    pub catchable: bool,
    pub opponent: FireRedBattleOpponent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedBattleOpponent {
    pub nickname: String,
    pub species_id: u16,
    pub species_name: String,
    pub level: u8,
    pub current_hp: u16,
    pub max_hp: u16,
    pub status: u32,
    pub nature: &'static str,
    pub ability_slot: u8,
    pub ability_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_item: Option<FireRedHeldItem>,
    pub stats: FireRedStatSpread,
    pub evs: FireRedStatSpread,
    pub ivs: FireRedStatSpread,
    pub ev_total: u16,
    pub iv_total: u16,
    pub moves: [u16; 4],
    pub move_slots: Vec<FireRedMoveSlot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catch_rate: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedEncounterGroup {
    pub method: &'static str,
    pub encounter_rate: u8,
    pub entries: Vec<FireRedEncounterEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FireRedEncounterEntry {
    pub species_id: u16,
    pub species_name: &'static str,
    pub min_level: u8,
    pub max_level: u8,
    pub slot_rate: u8,
    pub catch_rate: u8,
}

trait GameAddon {
    fn addon_id(&self) -> &'static str;
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
        "pokemon_frlg_party"
    }

    fn supports(&self, rom: &RomIdentity) -> bool {
        rom.game_code.starts_with("BPR")
            || rom.game_code.starts_with("BPG")
            || rom.title.eq_ignore_ascii_case("POKEMON FIRE")
            || rom.title.eq_ignore_ascii_case("POKEMON LEAF")
    }

    fn snapshot(&self, gba: &Gba, rom: &RomIdentity) -> Option<AddonSnapshot> {
        let (party_base_address, party) = locate_fire_red_party(gba)?;
        let area = locate_fire_red_area(gba);
        let battle = locate_fire_red_battle(gba, area.as_ref());

        let mut overlay_lines = Vec::with_capacity(party.len() + 4);
        overlay_lines.push(format!("Party: {} / {}", party.len(), PARTY_SLOT_COUNT));
        overlay_lines.extend(party.iter().map(format_party_line));
        if let Some(area) = &area {
            overlay_lines.push(format!("Area: {}", area.name));
            if let Some(group) = area.encounter_groups.first() {
                overlay_lines.push(format!(
                    "{} encounters: {} options",
                    group.method,
                    group.entries.len()
                ));
            }
        }
        if let Some(battle) = &battle {
            overlay_lines.push(format!(
                "Battle: {} {} Lv{}",
                battle.battle_kind, battle.opponent.species_name, battle.opponent.level
            ));
        }

        Some(AddonSnapshot {
            addon_id: self.addon_id(),
            display_name: frlg_display_name(rom),
            overlay_lines,
            data: AddonData::FireRed(FireRedSnapshot {
                source: "live_memory",
                party_base_address,
                party,
                area,
                battle,
            }),
        })
    }
}

fn frlg_display_name(rom: &RomIdentity) -> &'static str {
    if rom.game_code.starts_with("BPG") || rom.title.eq_ignore_ascii_case("POKEMON LEAF") {
        "LeafGreen Party"
    } else if rom.game_code.starts_with("BPR") || rom.title.eq_ignore_ascii_case("POKEMON FIRE") {
        "FireRed Party"
    } else {
        "FR/LG Party"
    }
}

fn locate_fire_red_area(gba: &Gba) -> Option<FireRedAreaSnapshot> {
    let save_block1 = gba.read_u32(FIRE_RED_SAVE_BLOCK1_PTR_ADDR);
    if !(0x0200_0000..=0x0203_F000).contains(&save_block1) {
        return None;
    }

    let location = save_block1 + SAVE_BLOCK1_LOCATION_OFFSET;
    let map_group = gba.read_u8(location + WARP_DATA_MAP_GROUP_OFFSET);
    let map_num = gba.read_u8(location + WARP_DATA_MAP_NUM_OFFSET);
    Some(fire_red_area_for_map(map_group, map_num))
}

fn fire_red_area_for_map(map_group: u8, map_num: u8) -> FireRedAreaSnapshot {
    let (map_key, name, encounter_groups) = match (map_group, map_num) {
        (1, 0) => (
            "MAP_VIRIDIAN_FOREST",
            "Viridian Forest",
            viridian_forest_encounters(),
        ),
        (1, 1) => ("MAP_MT_MOON_1F", "Mt. Moon 1F", mt_moon_1f_encounters()),
        (1, 2) => ("MAP_MT_MOON_B1F", "Mt. Moon B1F", mt_moon_b1f_encounters()),
        (1, 3) => ("MAP_MT_MOON_B2F", "Mt. Moon B2F", mt_moon_b2f_encounters()),
        (3, 0) => ("MAP_PALLET_TOWN", "Pallet Town", Vec::new()),
        (3, 1) => ("MAP_VIRIDIAN_CITY", "Viridian City", Vec::new()),
        (3, 19) => ("MAP_ROUTE1", "Route 1", route1_encounters()),
        (3, 20) => ("MAP_ROUTE2", "Route 2", route2_encounters()),
        (3, 21) => ("MAP_ROUTE3", "Route 3", route3_encounters()),
        (3, 22) => ("MAP_ROUTE4", "Route 4", route4_encounters()),
        (3, 41) => ("MAP_ROUTE22", "Route 22", route22_encounters()),
        _ => ("MAP_UNKNOWN", "Unknown Area", Vec::new()),
    };

    FireRedAreaSnapshot {
        map_group,
        map_num,
        map_key: map_key.to_string(),
        name: name.to_string(),
        encounter_groups,
    }
}

fn route1_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Grass",
        21,
        &[
            encounter(16, "Pidgey", 2, 5, 50, 255),
            encounter(19, "Rattata", 2, 4, 50, 255),
        ],
    )]
}

fn route2_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Grass",
        21,
        &[
            encounter(19, "Rattata", 2, 5, 45, 255),
            encounter(16, "Pidgey", 2, 5, 45, 255),
            encounter(10, "Caterpie", 4, 5, 5, 255),
            encounter(13, "Weedle", 4, 5, 5, 255),
        ],
    )]
}

fn route3_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Grass",
        21,
        &[
            encounter(21, "Spearow", 6, 8, 35, 255),
            encounter(16, "Pidgey", 6, 7, 30, 255),
            encounter(56, "Mankey", 7, 7, 10, 190),
            encounter(32, "Nidoran M", 6, 7, 14, 235),
            encounter(39, "Jigglypuff", 3, 7, 10, 170),
            encounter(29, "Nidoran F", 6, 6, 1, 235),
        ],
    )]
}

fn route4_encounters() -> Vec<FireRedEncounterGroup> {
    vec![
        encounter_group(
            "Grass",
            21,
            &[
                encounter(21, "Spearow", 8, 12, 35, 255),
                encounter(19, "Rattata", 8, 12, 35, 255),
                encounter(23, "Ekans", 6, 12, 25, 255),
                encounter(56, "Mankey", 10, 12, 5, 190),
            ],
        ),
        encounter_group("Surf", 2, &[encounter(72, "Tentacool", 5, 40, 100, 190)]),
        encounter_group(
            "Fishing",
            20,
            &[
                encounter(129, "Magikarp", 5, 15, 40, 255),
                encounter(116, "Horsea", 5, 35, 59, 225),
                encounter(130, "Gyarados", 15, 25, 1, 45),
            ],
        ),
    ]
}

fn route22_encounters() -> Vec<FireRedEncounterGroup> {
    vec![
        encounter_group(
            "Grass",
            21,
            &[
                encounter(19, "Rattata", 2, 5, 45, 255),
                encounter(56, "Mankey", 2, 5, 45, 190),
                encounter(21, "Spearow", 3, 5, 10, 255),
            ],
        ),
        encounter_group("Surf", 2, &[encounter(54, "Psyduck", 20, 40, 100, 190)]),
        encounter_group(
            "Fishing",
            20,
            &[
                encounter(129, "Magikarp", 5, 15, 40, 255),
                encounter(60, "Poliwag", 5, 25, 35, 255),
                encounter(61, "Poliwhirl", 20, 30, 15, 120),
                encounter(130, "Gyarados", 15, 25, 4, 45),
                encounter(54, "Psyduck", 15, 35, 6, 190),
            ],
        ),
    ]
}

fn viridian_forest_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Grass",
        14,
        &[
            encounter(10, "Caterpie", 3, 5, 40, 255),
            encounter(13, "Weedle", 3, 5, 40, 255),
            encounter(11, "Metapod", 5, 5, 5, 120),
            encounter(14, "Kakuna", 4, 6, 10, 120),
            encounter(25, "Pikachu", 3, 5, 5, 190),
        ],
    )]
}

fn mt_moon_1f_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Cave",
        7,
        &[
            encounter(41, "Zubat", 7, 10, 69, 255),
            encounter(74, "Geodude", 7, 9, 25, 255),
            encounter(46, "Paras", 8, 8, 5, 190),
            encounter(35, "Clefairy", 8, 8, 1, 150),
        ],
    )]
}

fn mt_moon_b1f_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Cave",
        5,
        &[encounter(46, "Paras", 5, 10, 100, 190)],
    )]
}

fn mt_moon_b2f_encounters() -> Vec<FireRedEncounterGroup> {
    vec![encounter_group(
        "Cave",
        7,
        &[
            encounter(41, "Zubat", 8, 11, 49, 255),
            encounter(74, "Geodude", 9, 10, 30, 255),
            encounter(46, "Paras", 10, 12, 15, 190),
            encounter(35, "Clefairy", 10, 12, 6, 150),
        ],
    )]
}

fn encounter_group(
    method: &'static str,
    encounter_rate: u8,
    entries: &[FireRedEncounterEntry],
) -> FireRedEncounterGroup {
    FireRedEncounterGroup {
        method,
        encounter_rate,
        entries: entries.to_vec(),
    }
}

fn encounter(
    species_id: u16,
    species_name: &'static str,
    min_level: u8,
    max_level: u8,
    slot_rate: u8,
    catch_rate: u8,
) -> FireRedEncounterEntry {
    FireRedEncounterEntry {
        species_id,
        species_name,
        min_level,
        max_level,
        slot_rate,
        catch_rate,
    }
}

fn locate_fire_red_battle(
    gba: &Gba,
    area: Option<&FireRedAreaSnapshot>,
) -> Option<FireRedBattleSnapshot> {
    let battle_type_flags = gba.read_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR);
    if !fire_red_battle_is_active(gba, battle_type_flags) {
        return None;
    }

    let raw = read_party_slot(gba, FIRE_RED_ENEMY_PARTY_BASE);
    let member = parse_party_member(&raw, 1)?;
    let (species_name, catch_rate) = fire_red_species_info(member.species_id, area);
    let catchable = battle_is_catchable(battle_type_flags);

    Some(FireRedBattleSnapshot {
        battle_type_flags,
        battle_kind: battle_kind_label(battle_type_flags),
        catchable,
        opponent: FireRedBattleOpponent {
            nickname: member.nickname,
            species_id: member.species_id,
            species_name,
            level: member.level,
            current_hp: member.current_hp,
            max_hp: member.max_hp,
            status: member.status,
            nature: member.nature,
            ability_slot: member.ability_slot,
            ability_name: member.ability_name,
            held_item: member.held_item,
            stats: member.stats,
            evs: member.evs,
            ivs: member.ivs,
            ev_total: member.ev_total,
            iv_total: member.iv_total,
            moves: member.moves,
            move_slots: member.move_slots,
            catch_rate: catchable.then_some(catch_rate).flatten(),
        },
    })
}

fn fire_red_battle_is_active(gba: &Gba, battle_type_flags: u32) -> bool {
    battle_type_flags != 0 && gba.read_u8(FIRE_RED_BATTLE_OUTCOME_ADDR) == BATTLE_OUTCOME_NONE
}

fn battle_kind_label(flags: u32) -> &'static str {
    if flags & BATTLE_TYPE_LINK != 0 {
        "Link"
    } else if flags & BATTLE_TYPE_TRAINER != 0 {
        "Trainer"
    } else if flags & BATTLE_TYPE_SAFARI != 0 {
        "Safari"
    } else if flags & BATTLE_TYPE_OLD_MAN_TUTORIAL != 0 {
        "Tutorial"
    } else if flags & BATTLE_TYPE_POKEDUDE != 0 {
        "Pokedude"
    } else if flags & BATTLE_TYPE_GHOST != 0 {
        "Ghost"
    } else if flags & BATTLE_TYPE_ROAMER != 0 {
        "Roamer"
    } else if flags & (BATTLE_TYPE_LEGENDARY | BATTLE_TYPE_LEGENDARY_FRLG) != 0 {
        "Legendary"
    } else {
        "Wild"
    }
}

fn battle_is_catchable(flags: u32) -> bool {
    flags
        & (BATTLE_TYPE_LINK
            | BATTLE_TYPE_TRAINER
            | BATTLE_TYPE_OLD_MAN_TUTORIAL
            | BATTLE_TYPE_GHOST
            | BATTLE_TYPE_POKEDUDE)
        == 0
}

fn fire_red_species_info(
    species_id: u16,
    area: Option<&FireRedAreaSnapshot>,
) -> (String, Option<u8>) {
    if let Some(entry) = area.and_then(|area| find_area_encounter_entry(area, species_id)) {
        return (entry.species_name.to_string(), Some(entry.catch_rate));
    }

    if let Some((species_name, catch_rate)) = fallback_species_info(species_id) {
        return (species_name.to_string(), Some(catch_rate));
    }

    (format!("Species #{species_id:03}"), None)
}

fn find_area_encounter_entry(
    area: &FireRedAreaSnapshot,
    species_id: u16,
) -> Option<&FireRedEncounterEntry> {
    area.encounter_groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .find(|entry| entry.species_id == species_id)
}

fn fallback_species_info(species_id: u16) -> Option<(&'static str, u8)> {
    match species_id {
        1 => Some(("Bulbasaur", 45)),
        2 => Some(("Ivysaur", 45)),
        3 => Some(("Venusaur", 45)),
        4 => Some(("Charmander", 45)),
        5 => Some(("Charmeleon", 45)),
        6 => Some(("Charizard", 45)),
        7 => Some(("Squirtle", 45)),
        8 => Some(("Wartortle", 45)),
        9 => Some(("Blastoise", 45)),
        10 => Some(("Caterpie", 255)),
        11 => Some(("Metapod", 120)),
        13 => Some(("Weedle", 255)),
        14 => Some(("Kakuna", 120)),
        16 => Some(("Pidgey", 255)),
        19 => Some(("Rattata", 255)),
        21 => Some(("Spearow", 255)),
        23 => Some(("Ekans", 255)),
        25 => Some(("Pikachu", 190)),
        29 => Some(("Nidoran F", 235)),
        32 => Some(("Nidoran M", 235)),
        35 => Some(("Clefairy", 150)),
        39 => Some(("Jigglypuff", 170)),
        41 => Some(("Zubat", 255)),
        46 => Some(("Paras", 190)),
        54 => Some(("Psyduck", 190)),
        56 => Some(("Mankey", 190)),
        60 => Some(("Poliwag", 255)),
        61 => Some(("Poliwhirl", 120)),
        72 => Some(("Tentacool", 190)),
        74 => Some(("Geodude", 255)),
        116 => Some(("Horsea", 225)),
        129 => Some(("Magikarp", 255)),
        130 => Some(("Gyarados", 45)),
        _ => None,
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
    let evs_section = sections[2];
    let misc = sections[3];
    let species_id = u16::from_le_bytes(growth[0..2].try_into().ok()?);
    if species_id == 0 || species_id > 440 {
        return None;
    }
    let held_item_id = u16::from_le_bytes(growth[2..4].try_into().ok()?);
    let moves = [
        u16::from_le_bytes(attacks[0..2].try_into().ok()?),
        u16::from_le_bytes(attacks[2..4].try_into().ok()?),
        u16::from_le_bytes(attacks[4..6].try_into().ok()?),
        u16::from_le_bytes(attacks[6..8].try_into().ok()?),
    ];
    let pp = [attacks[8], attacks[9], attacks[10], attacks[11]];
    let iv_word = u32::from_le_bytes(misc[4..8].try_into().ok()?);
    let ability_slot = ((iv_word >> 31) & 1) as u8;
    let evs = FireRedStatSpread {
        hp: evs_section[0] as u16,
        attack: evs_section[1] as u16,
        defense: evs_section[2] as u16,
        speed: evs_section[3] as u16,
        sp_attack: evs_section[4] as u16,
        sp_def: evs_section[5] as u16,
    };
    let ivs = FireRedStatSpread {
        hp: (iv_word & 0x1F) as u16,
        attack: ((iv_word >> 5) & 0x1F) as u16,
        defense: ((iv_word >> 10) & 0x1F) as u16,
        speed: ((iv_word >> 15) & 0x1F) as u16,
        sp_attack: ((iv_word >> 20) & 0x1F) as u16,
        sp_def: ((iv_word >> 25) & 0x1F) as u16,
    };

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
        species_name: species_display_name(species_id),
        personality,
        ot_id,
        level,
        current_hp,
        max_hp,
        status: u32::from_le_bytes(raw[80..84].try_into().ok()?),
        is_egg,
        nature: nature_name(personality),
        ability_slot,
        ability_name: ability_name_for_species(species_id, ability_slot),
        held_item: held_item(held_item_id),
        stats: FireRedStatSpread {
            hp: max_hp,
            attack: u16::from_le_bytes(raw[90..92].try_into().ok()?),
            defense: u16::from_le_bytes(raw[92..94].try_into().ok()?),
            speed: u16::from_le_bytes(raw[94..96].try_into().ok()?),
            sp_attack: u16::from_le_bytes(raw[96..98].try_into().ok()?),
            sp_def: u16::from_le_bytes(raw[98..100].try_into().ok()?),
        },
        evs,
        ivs,
        ev_total: stat_total(&evs),
        iv_total: stat_total(&ivs),
        moves,
        move_slots: move_slots(moves, pp),
    })
}

fn stat_total(spread: &FireRedStatSpread) -> u16 {
    spread.hp + spread.attack + spread.defense + spread.speed + spread.sp_attack + spread.sp_def
}

fn move_slots(moves: [u16; 4], pp: [u8; 4]) -> Vec<FireRedMoveSlot> {
    moves
        .iter()
        .zip(pp.iter())
        .enumerate()
        .filter_map(|(idx, (&move_id, &pp))| {
            (move_id != 0).then(|| FireRedMoveSlot {
                slot: idx as u8 + 1,
                move_id,
                name: move_name(move_id).to_string(),
                pp,
            })
        })
        .collect()
}

fn species_display_name(species_id: u16) -> String {
    fallback_species_info(species_id)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| format!("Species #{species_id:03}"))
}

fn held_item(item_id: u16) -> Option<FireRedHeldItem> {
    (item_id != 0).then(|| FireRedHeldItem {
        item_id,
        name: item_name(item_id).to_string(),
    })
}

fn item_name(item_id: u16) -> &'static str {
    match item_id {
        1 => "Master Ball",
        2 => "Ultra Ball",
        3 => "Great Ball",
        4 => "Poke Ball",
        13 => "Potion",
        14 => "Antidote",
        15 => "Burn Heal",
        16 => "Ice Heal",
        17 => "Awakening",
        18 => "Parlyz Heal",
        19 => "Full Restore",
        20 => "Max Potion",
        21 => "Hyper Potion",
        22 => "Super Potion",
        23 => "Full Heal",
        24 => "Revive",
        25 => "Max Revive",
        75 => "TinyMushroom",
        76 => "Big Mushroom",
        79 => "Pearl",
        80 => "Big Pearl",
        83 => "Stardust",
        84 => "Star Piece",
        143 => "Oran Berry",
        149 => "Leppa Berry",
        161 => "Cheri Berry",
        162 => "Chesto Berry",
        163 => "Pecha Berry",
        164 => "Rawst Berry",
        165 => "Aspear Berry",
        _ => "Unknown Item",
    }
}

fn nature_name(personality: u32) -> &'static str {
    const NATURES: [&str; 25] = [
        "Hardy", "Lonely", "Brave", "Adamant", "Naughty", "Bold", "Docile", "Relaxed", "Impish",
        "Lax", "Timid", "Hasty", "Serious", "Jolly", "Naive", "Modest", "Mild", "Quiet", "Bashful",
        "Rash", "Calm", "Gentle", "Sassy", "Careful", "Quirky",
    ];
    NATURES[(personality % 25) as usize]
}

fn ability_name_for_species(species_id: u16, ability_slot: u8) -> String {
    let (primary, secondary) = species_abilities(species_id);
    if ability_slot != 0 {
        secondary.unwrap_or(primary).to_string()
    } else {
        primary.to_string()
    }
}

fn species_abilities(species_id: u16) -> (&'static str, Option<&'static str>) {
    match species_id {
        1..=3 => ("Overgrow", None),
        4..=6 => ("Blaze", None),
        7..=9 => ("Torrent", None),
        10 | 11 | 13 | 14 => ("Shield Dust", None),
        16..=18 | 21 | 22 => ("Keen Eye", None),
        19 | 20 => ("Run Away", Some("Guts")),
        23 | 24 => ("Intimidate", Some("Shed Skin")),
        25 | 26 => ("Static", None),
        29..=34 => ("Poison Point", None),
        35 | 36 | 39 | 40 => ("Cute Charm", None),
        41 | 42 => ("Inner Focus", None),
        46 | 47 => ("Effect Spore", None),
        54 | 55 => ("Damp", Some("Cloud Nine")),
        56 | 57 => ("Vital Spirit", None),
        60..=62 => ("Water Absorb", Some("Damp")),
        72 | 73 => ("Clear Body", Some("Liquid Ooze")),
        74..=76 => ("Rock Head", Some("Sturdy")),
        116 | 117 | 129 => ("Swift Swim", None),
        130 => ("Intimidate", None),
        _ => ("Unknown", None),
    }
}

fn move_name(move_id: u16) -> &'static str {
    match move_id {
        10 => "Scratch",
        22 => "Vine Whip",
        29 => "Headbutt",
        31 => "Fury Attack",
        33 => "Tackle",
        39 => "Tail Whip",
        40 => "Poison Sting",
        43 => "Leer",
        45 => "Growl",
        52 => "Ember",
        55 => "Water Gun",
        64 => "Peck",
        67 => "Low Kick",
        81 => "String Shot",
        84 => "Thunder Shock",
        98 => "Quick Attack",
        110 => "Withdraw",
        145 => "Bubble",
        158 => "Hyper Fang",
        _ => "Unknown Move",
    }
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
    fn test_fire_red_addon_supports_leaf_green_revision_one() {
        let addon = FireRedAddon;
        let rom = RomIdentity {
            title: "POKEMON LEAF".to_string(),
            game_code: "BPGE".to_string(),
            maker_code: "01".to_string(),
            revision: 1,
        };

        assert!(addon.supports(&rom));
        assert_eq!(frlg_display_name(&rom), "LeafGreen Party");
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
    fn test_parse_party_member_exports_streamer_scout_details() {
        let evs = FireRedStatSpread {
            hp: 1,
            attack: 2,
            defense: 3,
            speed: 4,
            sp_attack: 5,
            sp_def: 6,
        };
        let ivs = FireRedStatSpread {
            hp: 31,
            attack: 30,
            defense: 29,
            speed: 28,
            sp_attack: 27,
            sp_def: 26,
        };
        let raw = build_party_member_raw_with_details(
            0x0000_001A,
            0x1234_5678,
            "RATTATA",
            19,
            8,
            18,
            22,
            13,
            evs,
            ivs,
            1,
        );

        let member = parse_party_member(&raw, 1).expect("valid detailed party member");
        assert_eq!(member.species_name, "Rattata");
        assert_eq!(member.nature, "Lonely");
        assert_eq!(member.ability_slot, 1);
        assert_eq!(member.ability_name, "Guts");
        assert_eq!(
            member.held_item.as_ref().map(|item| item.name.as_str()),
            Some("Potion")
        );
        assert_eq!(member.evs, evs);
        assert_eq!(member.ivs, ivs);
        assert_eq!(member.ev_total, 21);
        assert_eq!(member.iv_total, 171);
        assert_eq!(member.stats.attack, 12);
        assert_eq!(member.move_slots[0].name, "Tackle");
        assert_eq!(member.move_slots[0].pp, 35);
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
    fn test_locate_fire_red_area_from_save_block_pointer() {
        let mut gba = Gba::new();
        let save_block1 = 0x0203_0000;
        gba.write_u32(FIRE_RED_SAVE_BLOCK1_PTR_ADDR, save_block1);
        gba.write_u8(save_block1 + SAVE_BLOCK1_LOCATION_OFFSET, 3);
        gba.write_u8(save_block1 + SAVE_BLOCK1_LOCATION_OFFSET + 1, 19);

        let area = locate_fire_red_area(&gba).expect("area should be found");
        assert_eq!(area.map_group, 3);
        assert_eq!(area.map_num, 19);
        assert_eq!(area.map_key, "MAP_ROUTE1");
        assert_eq!(area.name, "Route 1");
        assert_eq!(area.encounter_groups.len(), 1);
        assert_eq!(area.encounter_groups[0].entries[0].species_name, "Pidgey");
        assert_eq!(area.encounter_groups[0].entries[0].slot_rate, 50);
    }

    #[test]
    fn test_locate_fire_red_battle_is_hidden_without_battle_flags() {
        let mut gba = Gba::new();
        gba.write_u8(FIRE_RED_BATTLE_OUTCOME_ADDR, BATTLE_OUTCOME_NONE);
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE,
            &build_party_member_raw(0x1111_2222, 0x3333_4444, "RATTATA", 19, 3, 12, 12),
        );

        assert_eq!(locate_fire_red_battle(&gba, None), None);
    }

    #[test]
    fn test_locate_fire_red_battle_is_hidden_after_battle_outcome() {
        let mut gba = Gba::new();
        gba.write_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR, 1 << 2);
        gba.write_u8(FIRE_RED_BATTLE_OUTCOME_ADDR, 1);
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE,
            &build_party_member_raw(0x1111_2222, 0x3333_4444, "MANKEY", 56, 2, 8, 14),
        );

        assert_eq!(locate_fire_red_battle(&gba, None), None);
    }

    #[test]
    fn test_locate_fire_red_battle_from_enemy_party_slot() {
        let mut gba = Gba::new();
        gba.write_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR, 1 << 2);
        gba.write_u8(FIRE_RED_BATTLE_OUTCOME_ADDR, BATTLE_OUTCOME_NONE);
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE,
            &build_party_member_raw(0x2222_3333, 0x4444_5555, "RATTATA", 19, 4, 11, 13),
        );

        let area = fire_red_area_for_map(3, 19);
        let battle = locate_fire_red_battle(&gba, Some(&area)).expect("battle should be found");
        assert_eq!(battle.battle_kind, "Wild");
        assert!(battle.catchable);
        assert_eq!(battle.opponent.nickname, "RATTATA");
        assert_eq!(battle.opponent.species_id, 19);
        assert_eq!(battle.opponent.species_name, "Rattata");
        assert_eq!(battle.opponent.level, 4);
        assert_eq!(battle.opponent.current_hp, 11);
        assert_eq!(battle.opponent.max_hp, 13);
        assert_eq!(battle.opponent.catch_rate, Some(255));
    }

    #[test]
    fn test_locate_fire_red_trainer_battle_is_not_catchable() {
        let mut gba = Gba::new();
        gba.write_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR, BATTLE_TYPE_TRAINER);
        gba.write_u8(FIRE_RED_BATTLE_OUTCOME_ADDR, BATTLE_OUTCOME_NONE);
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE,
            &build_party_member_raw(0x3333_4444, 0x5555_6666, "SQUIRTLE", 7, 8, 21, 21),
        );

        let battle = locate_fire_red_battle(&gba, None).expect("battle should be found");
        assert_eq!(battle.battle_kind, "Trainer");
        assert!(!battle.catchable);
        assert_eq!(battle.opponent.species_name, "Squirtle");
        assert_eq!(battle.opponent.catch_rate, None);
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
        build_party_member_raw_with_details(
            personality,
            ot_id,
            nickname,
            species_id,
            level,
            current_hp,
            max_hp,
            0,
            FireRedStatSpread {
                hp: 0,
                attack: 0,
                defense: 0,
                speed: 0,
                sp_attack: 0,
                sp_def: 0,
            },
            FireRedStatSpread {
                hp: 0,
                attack: 0,
                defense: 0,
                speed: 0,
                sp_attack: 0,
                sp_def: 0,
            },
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_party_member_raw_with_details(
        personality: u32,
        ot_id: u32,
        nickname: &str,
        species_id: u16,
        level: u8,
        current_hp: u16,
        max_hp: u16,
        held_item_id: u16,
        evs: FireRedStatSpread,
        ivs: FireRedStatSpread,
        ability_slot: u8,
    ) -> [u8; PARTY_SLOT_SIZE] {
        let mut raw = [0u8; PARTY_SLOT_SIZE];
        raw[0..4].copy_from_slice(&personality.to_le_bytes());
        raw[4..8].copy_from_slice(&ot_id.to_le_bytes());
        raw[8..18].copy_from_slice(&encode_gen3_text(nickname));
        raw[18] = 2;
        raw[19] = 0x2;

        let mut growth = [0u8; 12];
        growth[0..2].copy_from_slice(&species_id.to_le_bytes());
        growth[2..4].copy_from_slice(&held_item_id.to_le_bytes());
        growth[4..8].copy_from_slice(&12_345u32.to_le_bytes());
        growth[8] = 0;
        growth[9] = 70;

        let mut attacks = [0u8; 12];
        attacks[0..2].copy_from_slice(&33u16.to_le_bytes());
        attacks[2..4].copy_from_slice(&45u16.to_le_bytes());
        attacks[8] = 35;
        attacks[9] = 30;

        let mut ev_section = [0u8; 12];
        ev_section[0] = evs.hp as u8;
        ev_section[1] = evs.attack as u8;
        ev_section[2] = evs.defense as u8;
        ev_section[3] = evs.speed as u8;
        ev_section[4] = evs.sp_attack as u8;
        ev_section[5] = evs.sp_def as u8;

        let iv_word = (ivs.hp as u32)
            | ((ivs.attack as u32) << 5)
            | ((ivs.defense as u32) << 10)
            | ((ivs.speed as u32) << 15)
            | ((ivs.sp_attack as u32) << 20)
            | ((ivs.sp_def as u32) << 25)
            | (((ability_slot & 1) as u32) << 31);
        let mut misc = [0u8; 12];
        misc[4..8].copy_from_slice(&iv_word.to_le_bytes());

        let sections = [growth, attacks, ev_section, misc];
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
        raw[90..92].copy_from_slice(&12u16.to_le_bytes());
        raw[92..94].copy_from_slice(&13u16.to_le_bytes());
        raw[94..96].copy_from_slice(&14u16.to_le_bytes());
        raw[96..98].copy_from_slice(&15u16.to_le_bytes());
        raw[98..100].copy_from_slice(&16u16.to_le_bytes());
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

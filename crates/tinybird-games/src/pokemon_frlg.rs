//! Pokemon FireRed / LeafGreen addon.
//!
//! Reads the live party, the current area's wild encounter table, and the
//! active battle out of the running game's work RAM, and exports them both as
//! typed data for the desktop dashboard and as generic sections for anything
//! else that consumes the snapshot schema.
//!
//! Everything here goes through [`MemoryView`], not the emulator, so the
//! parsing is unit-tested against synthetic memory rather than by playing the
//! game to the right screen.

use std::collections::BTreeSet;

use serde::Serialize;
use tinybird_addons::schema::{
    AddonBadge, AddonCard, AddonField, AddonImage, AddonMeter, AddonSection, AddonTone,
};
use tinybird_addons::{AddonInfo, GameAddon, MemoryView, RomIdentity};

use crate::gen3_names;
use crate::{AddonData, AddonSnapshot};

const ADDON_VERSION: &str = "0.2.0";

const PARTY_SLOT_SIZE: usize = 100;
const PARTY_SLOT_COUNT: usize = 6;
/// Six stats at the 31 the game caps an individual value at.
const MAX_IV_TOTAL: u32 = 186;
/// The cap the game puts on the sum of all six effort values.
const MAX_EV_TOTAL: u32 = 510;
/// The easiest catch rate in the game, so a catch-rate bar has a scale.
const MAX_CATCH_RATE: u32 = 255;
/// Encounter slot rates in one method add up to this, so a bar drawn against
/// it ranks a species against the others in the same grass.
const ENCOUNTER_SLOT_TOTAL: u32 = 100;
/// Gen 3 packs sleep as a turn counter in the low three bits of the status
/// word and everything else as a flag above it.
const STATUS_SLEEP_MASK: u32 = 0b111;
const STATUS_POISON: u32 = 1 << 3;
const STATUS_BURN: u32 = 1 << 4;
const STATUS_FREEZE: u32 = 1 << 5;
const STATUS_PARALYSIS: u32 = 1 << 6;
const STATUS_TOXIC: u32 = 1 << 7;
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

/// Reads the live party, area encounters, and active battle for
/// Pokemon FireRed and LeafGreen.
pub struct PokemonFrlgAddon;

impl GameAddon<AddonData> for PokemonFrlgAddon {
    fn info(&self) -> AddonInfo {
        AddonInfo {
            addon_id: "pokemon_frlg_party",
            display_name: "Pokemon FireRed / LeafGreen",
            version: ADDON_VERSION,
            capabilities: &["party", "area", "battle"],
            supported_games: "Pokemon FireRed (BPR*) and LeafGreen (BPG*)",
        }
    }

    fn supports(&self, rom: &RomIdentity) -> bool {
        rom.code_prefix() == "BPR"
            || rom.code_prefix() == "BPG"
            || rom.title.eq_ignore_ascii_case("POKEMON FIRE")
            || rom.title.eq_ignore_ascii_case("POKEMON LEAF")
    }

    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot> {
        let (party_base_address, party) = locate_fire_red_party(memory)?;
        let area = locate_fire_red_area(memory);
        let battle = locate_fire_red_battle(memory, area.as_ref());

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
            if let Some((label, _)) = iv_quality(&battle.opponent.ivs, battle.opponent.iv_total) {
                overlay_lines.push(format!("!! {label}"));
            }
        }

        let fire_red = FireRedSnapshot {
            source: "live_memory",
            party_base_address,
            party,
            area,
            battle,
        };
        let sections = fire_red_sections(&fire_red);

        let info = self.info();
        Some(
            AddonSnapshot::new(
                info.addon_id,
                frlg_display_name(rom),
                overlay_lines,
                AddonData::FireRed(fire_red),
            )
            .with_version(info.version)
            .with_capabilities(info.capabilities.to_vec())
            .with_sections(sections),
        )
    }
}

// The typed payload carries far more than a table of names and levels: natures,
// abilities, held items, the full stat spread, IVs, EVs, PP, catch rates. The
// sections below are what every non-desktop consumer sees, so they exist to get
// that detail out rather than to summarise it away.
//
// Two sections, and no more than two:
//
// - the party, because it is the thing you check constantly;
// - the dex, meaning whatever you are looking at outside your own team.
//
// The dex is one tab that answers one question — "what is that" — and swaps
// what it reports to match. In a battle it is the opponent; out of one it is
// what lives here and how to meet it. These were separate tabs and should not
// have been: they are never both the answer, an area list is no use while a
// Rattata is on screen, and keeping them apart meant the strip carried a tab
// that was idle whenever the other one mattered.
//
// Two earlier tabs are gone for the same reason. "Team" was slots used, total
// HP and average level — arithmetic on the party beside it — and is now the
// line above that party. A section per encounter method turned one question,
// "what is on this route", into four places to look; the methods are badges in
// one list.
//
// The dex is emitted every time, empty or not. A section that comes and goes
// takes its tab with it, and a strip that changes width whenever a wild
// Pokemon appears moves every other tab out from under the cursor.
fn fire_red_sections(snapshot: &FireRedSnapshot) -> Vec<AddonSection> {
    vec![party_section(snapshot), dex_section(snapshot)]
}

/// What you are looking at, whatever that currently is.
fn dex_section(snapshot: &FireRedSnapshot) -> AddonSection {
    match (&snapshot.battle, &snapshot.area) {
        (Some(battle), _) => battle_dex(battle),
        (None, Some(area)) => area_dex(area),
        (None, None) => AddonSection::list(
            "dex",
            "Dex",
            vec!["Nothing to report yet.".to_string()],
        )
        .with_note("What you meet, and what lives where you are"),
    }
}

/// One card per party slot, under a line saying how the team as a whole is.
fn party_section(snapshot: &FireRedSnapshot) -> AddonSection {
    let cards = snapshot.party.iter().map(party_card).collect::<Vec<_>>();
    let section = AddonSection::cards("party", "Party", cards);

    if snapshot.party.is_empty() {
        return section.with_note("No party yet - the save has not been loaded");
    }
    section.with_note(party_summary(snapshot))
}

/// The team, in one line.
///
/// This used to be a section of its own with a row per number. It reads better
/// as a caption over the party: every one of these is an arithmetic fact about
/// the cards directly underneath, and a tab you open to add up the bars in the
/// tab beside it is a tab that is not doing anything.
fn party_summary(snapshot: &FireRedSnapshot) -> String {
    // Eggs occupy a slot but have no HP and cannot faint, so counting them in
    // team health would report a healthy party as half dead.
    let fighters: Vec<_> = snapshot.party.iter().filter(|m| !m.is_egg).collect();
    let mut parts = vec![format!("{}/{}", snapshot.party.len(), PARTY_SLOT_COUNT)];

    if !fighters.is_empty() {
        let hp: u32 = fighters.iter().map(|m| u32::from(m.current_hp)).sum();
        let max_hp: u32 = fighters.iter().map(|m| u32::from(m.max_hp)).sum();
        let levels: u32 = fighters.iter().map(|m| u32::from(m.level)).sum();
        parts.push(format!("{hp}/{max_hp} HP"));
        parts.push(format!("avg Lv{}", levels / fighters.len() as u32));

        // Only what is wrong is worth the words. A healthy team says nothing
        // more, because the six bars underneath already said it.
        let fainted = fighters.iter().filter(|m| m.current_hp == 0).count();
        if fainted > 0 {
            parts.push(format!("{fainted} fainted"));
        }
        let ailing = fighters
            .iter()
            .filter(|m| m.current_hp > 0 && status_label(m.status).is_some())
            .count();
        if ailing > 0 {
            parts.push(format!("{ailing} ailing"));
        }
    }

    let eggs = snapshot.party.iter().filter(|m| m.is_egg).count();
    if eggs > 0 {
        parts.push(format!("{eggs} egg"));
    }

    parts.join(" - ")
}

fn party_card(member: &FireRedPartyMember) -> AddonCard {
    if member.is_egg {
        // Nothing inside an egg is readable, and printing zeroed stats for one
        // reads as a bug rather than as an egg. No picture either: the species
        // behind the shell is exactly what an egg is not supposed to tell you.
        return AddonCard::new(format!("Slot {}", member.slot))
            .with_subtitle("Egg")
            .with_badges(vec![AddonBadge::new("Egg")]);
    }

    let mut badges = Vec::new();
    if member.current_hp == 0 {
        badges.push(AddonBadge::toned("Fainted", AddonTone::Bad));
    } else if let Some((label, tone)) = status_label(member.status) {
        badges.push(AddonBadge::toned(label, tone));
    }
    badges.push(AddonBadge::new(member.nature));

    // No "Level" row: the subtitle carries it, and it is the first thing read
    // about a party member so it belongs in the heading rather than the detail.
    let mut fields = vec![AddonField::new("Ability", member.ability_name.clone())];

    if let Some(item) = &member.held_item {
        fields.push(AddonField::new("Holding", item.name.clone()).with_tone(AddonTone::Good));
    }

    // The totals first, against the theoretical maxima so the bars say
    // something: 31 per stat for IVs, and the 510 the game caps total EVs at.
    fields.push(AddonField::gauge("IVs", u32::from(member.iv_total), MAX_IV_TOTAL));
    fields.push(
        AddonField::gauge("EVs", u32::from(member.ev_total), MAX_EV_TOTAL)
            .with_tone(AddonTone::Neutral),
    );
    fields.extend(stat_rows(&member.stats, &member.ivs, &member.evs, true));

    fields.extend(member.move_slots.iter().map(move_field));

    let title = if member.nickname.trim().is_empty() {
        member.species_name.clone()
    } else {
        member.nickname.clone()
    };
    // Species and level. The slot number was here and told you nothing you
    // could not get by counting down the list.
    let subtitle = if member.nickname.trim().eq_ignore_ascii_case(&member.species_name) {
        format!("Lv {}", member.level)
    } else {
        format!("{} \u{00b7} Lv {}", member.species_name, member.level)
    };

    AddonCard::new(title)
        .with_subtitle(subtitle)
        .with_image(species_sprite(member.species_id, &member.species_name))
        .with_lead(AddonField::gauge(
            "HP",
            u32::from(member.current_hp),
            u32::from(member.max_hp),
        ))
        .with_badges(badges)
        .with_fields(fields)
}

/// Where a consumer can find a picture of this species.
///
/// A path rather than a full URL, so whatever is serving the page serves the
/// sprite too: the host decides where the pictures actually come from and can
/// cache them, and the addon stays a thing that only reads memory.
fn species_sprite(species_id: u16, species_name: &str) -> AddonImage {
    AddonImage::new(format!("/sprites/{species_id}")).with_alt(species_name)
}

/// A move and what is left of it. PP is a bar only for moves whose maximum is
/// known; for the rest the count alone is still worth showing.
fn move_field(slot: &FireRedMoveSlot) -> AddonField {
    let label = format!("Move {}", slot.slot);
    match move_max_pp(slot.move_id) {
        Some(max) => AddonField::new(label, slot.name.clone())
            .with_meter(AddonMeter::new(u32::from(slot.pp), u32::from(max)))
            .with_tone(AddonTone::from_fraction(
                u32::from(slot.pp),
                u32::from(max),
            ))
            .with_hint(format!("{}/{} PP", slot.pp, max)),
        None => {
            AddonField::new(label, slot.name.clone()).with_hint(format!("{} PP left", slot.pp))
        }
    }
}

/// One row per stat: its value, and the IV and EV behind it.
///
/// This was three rows of six bare numbers — stats, then individual values,
/// then effort values — with a legend line under the first saying which column
/// was which. Reading "is my Speed IV any good" meant counting along two rows
/// and hoping you had not slipped a column. Six labelled rows are a few lines
/// longer and cost nothing to read.
///
/// `show_evs` because a wild Pokemon has none, and "EV 0" six times is six
/// pieces of nothing.
fn stat_rows(
    stats: &FireRedStatSpread,
    ivs: &FireRedStatSpread,
    evs: &FireRedStatSpread,
    show_evs: bool,
) -> Vec<AddonField> {
    // The order the game's own summary screen uses, not the order the struct
    // happens to store them in.
    let rows: [(&str, u16, u16, u16); 6] = [
        ("HP", stats.hp, ivs.hp, evs.hp),
        ("Attack", stats.attack, ivs.attack, evs.attack),
        ("Defense", stats.defense, ivs.defense, evs.defense),
        ("Sp. Atk", stats.sp_attack, ivs.sp_attack, evs.sp_attack),
        ("Sp. Def", stats.sp_def, ivs.sp_def, evs.sp_def),
        ("Speed", stats.speed, ivs.speed, evs.speed),
    ];

    rows.iter()
        .map(|(label, value, iv, ev)| {
            let hint = if show_evs {
                format!("IV {iv} \u{00b7} EV {ev}")
            } else {
                format!("IV {iv}")
            };
            AddonField::new(*label, value.to_string()).with_hint(hint)
        })
        .collect()
}

/// Six stats at 31 apiece: a Pokemon that cannot be bettered.
const PERFECT_IV: u16 = 31;
/// Individual values worth stopping for, out of the 186 a perfect one has.
const GREAT_IV_TOTAL: u16 = 155;
const GOOD_IV_TOTAL: u16 = 124;

/// How good this spread is, as something worth saying out loud.
///
/// Read from two angles because breeders and catchers care about different
/// ones: how many stats are maxed, and how good it is overall. A Pokemon with
/// three 31s and three terrible stats is worth catching for breeding and would
/// not clear the total on its own.
fn iv_quality(ivs: &FireRedStatSpread, total: u16) -> Option<(String, AddonTone)> {
    let perfect = [
        ivs.hp,
        ivs.attack,
        ivs.defense,
        ivs.speed,
        ivs.sp_attack,
        ivs.sp_def,
    ]
    .iter()
    .filter(|value| **value >= PERFECT_IV)
    .count();

    if perfect >= 2 || total >= GREAT_IV_TOTAL {
        Some((great_iv_label(perfect), AddonTone::Good))
    } else if perfect == 1 || total >= GOOD_IV_TOTAL {
        Some((
            if perfect == 1 {
                "1 perfect IV".to_string()
            } else {
                "Good IVs".to_string()
            },
            AddonTone::Good,
        ))
    } else {
        None
    }
}

fn great_iv_label(perfect: usize) -> String {
    match perfect {
        0 => "Great IVs".to_string(),
        6 => "Perfect IVs".to_string(),
        n => format!("{n} perfect IVs"),
    }
}

/// The opponent, read the way someone deciding whether to throw a ball reads it.
fn battle_dex(battle: &FireRedBattleSnapshot) -> AddonSection {
    let opponent = &battle.opponent;

    let mut badges = vec![AddonBadge::toned(
        battle.battle_kind,
        if battle.catchable {
            AddonTone::Good
        } else {
            AddonTone::Neutral
        },
    )];
    if let Some((label, tone)) = status_label(opponent.status) {
        badges.push(AddonBadge::toned(label, tone));
    }
    if !battle.catchable {
        badges.push(AddonBadge::toned("No ball", AddonTone::Bad));
    }

    // The one thing worth interrupting someone for. It goes on the card and,
    // below, on the section itself, so it is visible from whichever tab they
    // happen to be looking at when the encounter starts.
    let quality = iv_quality(&opponent.ivs, opponent.iv_total);
    if let Some((label, tone)) = &quality {
        badges.push(AddonBadge::toned(label.clone(), *tone));
    }

    let mut fields = vec![
        AddonField::new("Level", opponent.level.to_string()),
        AddonField::new("Ability", opponent.ability_name.clone()),
        AddonField::new("Nature", opponent.nature),
    ];

    if let Some(item) = &opponent.held_item {
        fields.push(AddonField::new("Holding", item.name.clone()));
    }

    // The whole point of reading catch rate live: a bar against the easiest
    // catch in the game, plus the reading that combines it with current HP.
    if let Some(catch_rate) = opponent.catch_rate {
        let (label, tone) = catch_difficulty(catch_rate, opponent.current_hp, opponent.max_hp);
        fields.push(
            AddonField::new("Catch chance", label)
                .with_tone(tone)
                .with_meter(AddonMeter::new(u32::from(catch_rate), MAX_CATCH_RATE))
                .with_hint(format!("base catch rate {catch_rate}/255")),
        );
    }

    fields.push(AddonField::gauge("IVs", u32::from(opponent.iv_total), MAX_IV_TOTAL));
    // A wild Pokemon has no effort values, and "EV 0" six times is six pieces
    // of nothing. A trainer's has them, and then they are worth the column.
    fields.extend(stat_rows(
        &opponent.stats,
        &opponent.ivs,
        &opponent.evs,
        opponent.ev_total > 0,
    ));
    fields.extend(opponent.move_slots.iter().map(move_field));

    let title = if opponent.nickname.trim().is_empty()
        || opponent
            .nickname
            .trim()
            .eq_ignore_ascii_case(&opponent.species_name)
    {
        opponent.species_name.clone()
    } else {
        format!("{} ({})", opponent.nickname, opponent.species_name)
    };

    let card = AddonCard::new(title)
        .with_subtitle(format!("Lv {}", opponent.level))
        .with_image(species_sprite(opponent.species_id, &opponent.species_name))
        .with_lead(AddonField::gauge(
            "HP",
            u32::from(opponent.current_hp),
            u32::from(opponent.max_hp),
        ))
        .with_badges(badges)
        .with_fields(fields);

    let section = AddonSection::cards("dex", "Dex", vec![card])
        .with_note(format!("{} battle in progress", battle.battle_kind));

    match quality {
        Some((label, tone)) => section.with_badge(AddonBadge::toned(label, tone)),
        None => section,
    }
}

/// How a ball is likely to go, from the base rate and how hurt the target is.
///
/// Not the real Gen 3 formula - that needs the ball, the status, and a random
/// roll. This is the reading a player makes at a glance, and saying so in the
/// hint keeps it from being mistaken for a computed probability.
fn catch_difficulty(catch_rate: u8, current_hp: u16, max_hp: u16) -> (&'static str, AddonTone) {
    let weakened = max_hp > 0 && u32::from(current_hp) * 3 <= u32::from(max_hp);
    match (catch_rate, weakened) {
        (0..=15, false) => ("very hard", AddonTone::Bad),
        (0..=15, true) => ("hard, weakened", AddonTone::Warn),
        (16..=75, false) => ("hard", AddonTone::Warn),
        (16..=75, true) => ("fair, weakened", AddonTone::Good),
        (_, false) => ("fair", AddonTone::Good),
        (_, true) => ("good, weakened", AddonTone::Good),
    }
}

/// Where you are, and everything that lives here.
///
/// One section rather than a header plus a table per method. A player asks
/// "what is on this route" once; answering it across four tabs — Area, Grass,
/// Surf, Fishing — made them ask it four times. The method rides on each
/// species as a badge, which keeps grass and surf apart without keeping them
/// in separate places, and the cards carry sprites because recognising what
/// you are about to walk into is faster from a picture than from a name.
fn area_dex(area: &FireRedAreaSnapshot) -> AddonSection {
    let mut entries: Vec<(&FireRedEncounterGroup, &FireRedEncounterEntry)> = area
        .encounter_groups
        .iter()
        .flat_map(|group| group.entries.iter().map(move |entry| (group, entry)))
        .collect();

    // Grouped by method, and inside a method by how often the slot comes up:
    // the thing you are most likely to meet first, in the way you would meet it.
    entries.sort_by(|(left_group, left), (right_group, right)| {
        left_group
            .method
            .cmp(right_group.method)
            .then_with(|| right.slot_rate.cmp(&left.slot_rate))
            .then_with(|| left.species_name.cmp(right.species_name))
    });

    let species = area
        .encounter_groups
        .iter()
        .flat_map(|group| group.entries.iter().map(|entry| entry.species_name))
        .collect::<BTreeSet<_>>();

    let note = if species.is_empty() {
        format!("{} ({}) - no encounters known", area.name, area.map_key)
    } else {
        format!(
            "{} ({}) - {} species",
            area.name,
            area.map_key,
            species.len()
        )
    };

    if entries.is_empty() {
        // A section that reports nothing at all reads as broken. Saying which
        // map is unmapped is the difference between "no data" and "no data yet".
        return AddonSection::list(
            "dex",
            "Dex",
            vec![format!(
                "No encounter table entered for {} yet.",
                area.map_key
            )],
        )
        .with_note(note);
    }

    let cards = entries
        .into_iter()
        .map(|(group, entry)| {
            AddonCard::new(entry.species_name)
                .with_subtitle(format!(
                    "Lv {}",
                    level_range(entry.min_level, entry.max_level)
                ))
                .with_image(species_sprite(entry.species_id, entry.species_name))
                // The slot rate against the whole encounter table, so the bars
                // rank the species against each other rather than against an
                // abstract hundred.
                .with_lead(
                    AddonField::new("Slot", format!("{}%", entry.slot_rate))
                        .with_meter(AddonMeter::new(
                            u32::from(entry.slot_rate),
                            ENCOUNTER_SLOT_TOTAL,
                        ))
                        .with_tone(AddonTone::Neutral)
                        // Beside the rate rather than as a chip of its own. A
                        // route can run to twenty of these, and a second chip
                        // on every one of them is a second line on every one.
                        .with_hint(format!("catch {}", entry.catch_rate)),
                )
                .with_badges(vec![AddonBadge::toned(group.method, AddonTone::Good)])
        })
        .collect();

    AddonSection::cards("dex", "Dex", cards).with_note(note)
}

fn level_range(min: u8, max: u8) -> String {
    if min == max {
        min.to_string()
    } else {
        format!("{min}-{max}")
    }
}

/// The Gen 3 status word, which the party parser has always read and nothing
/// has ever shown. Sleep is a turn counter in the low three bits; the rest are
/// flags, checked worst-first because one Pokemon can carry more than one.
fn status_label(status: u32) -> Option<(&'static str, AddonTone)> {
    if status & STATUS_TOXIC != 0 {
        Some(("Badly poisoned", AddonTone::Bad))
    } else if status & STATUS_FREEZE != 0 {
        Some(("Frozen", AddonTone::Bad))
    } else if status & STATUS_SLEEP_MASK != 0 {
        Some(("Asleep", AddonTone::Warn))
    } else if status & STATUS_PARALYSIS != 0 {
        Some(("Paralysed", AddonTone::Warn))
    } else if status & STATUS_BURN != 0 {
        Some(("Burned", AddonTone::Warn))
    } else if status & STATUS_POISON != 0 {
        Some(("Poisoned", AddonTone::Warn))
    } else {
        None
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

fn locate_fire_red_area(memory: &dyn MemoryView) -> Option<FireRedAreaSnapshot> {
    let save_block1 = memory.read_u32(FIRE_RED_SAVE_BLOCK1_PTR_ADDR);
    if !(0x0200_0000..=0x0203_F000).contains(&save_block1) {
        return None;
    }

    let location = save_block1 + SAVE_BLOCK1_LOCATION_OFFSET;
    let map_group = memory.read_u8(location + WARP_DATA_MAP_GROUP_OFFSET);
    let map_num = memory.read_u8(location + WARP_DATA_MAP_NUM_OFFSET);
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
    memory: &dyn MemoryView,
    area: Option<&FireRedAreaSnapshot>,
) -> Option<FireRedBattleSnapshot> {
    let battle_type_flags = memory.read_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR);
    if !fire_red_battle_is_active(memory, battle_type_flags) {
        return None;
    }

    let member = locate_active_enemy_party_member(memory)?;
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

fn locate_active_enemy_party_member(memory: &dyn MemoryView) -> Option<FireRedPartyMember> {
    let enemy_party = parse_party_prefix_at(memory, FIRE_RED_ENEMY_PARTY_BASE)?;
    enemy_party
        .iter()
        .find(|member| !member.is_egg && member.current_hp > 0)
        .or_else(|| enemy_party.first())
        .cloned()
}

fn fire_red_battle_is_active(memory: &dyn MemoryView, battle_type_flags: u32) -> bool {
    battle_type_flags != 0 && memory.read_u8(FIRE_RED_BATTLE_OUTCOME_ADDR) == BATTLE_OUTCOME_NONE
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

/// Every species in Generation 3, by the index the game stores.
///
/// **Not National Dex numbers.** Kanto and Johto happen to line up, but the
/// indices 252-276 are unused slots and Hoenn lives at 277-411 — so a traded
/// Treecko is species 277, not 252. Getting that wrong silently renames every
/// Hoenn Pokemon in a FireRed party, which is exactly the kind of confident
/// wrongness a lookup table should not produce.
///
/// The catch rate is the second half of the pair because the battle read-out
/// uses it: it is what turns "Rattata" into "worth a ball".
///
/// Like the move table, this is the Generation 3 data rather than a read out
/// of the cartridge. See `ADDONS.md` for why, and for what reading it from the
/// ROM would fix.
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
        12 => Some(("Butterfree", 45)),
        13 => Some(("Weedle", 255)),
        14 => Some(("Kakuna", 120)),
        15 => Some(("Beedrill", 45)),
        16 => Some(("Pidgey", 255)),
        17 => Some(("Pidgeotto", 120)),
        18 => Some(("Pidgeot", 45)),
        19 => Some(("Rattata", 255)),
        20 => Some(("Raticate", 127)),
        21 => Some(("Spearow", 255)),
        22 => Some(("Fearow", 90)),
        23 => Some(("Ekans", 255)),
        24 => Some(("Arbok", 90)),
        25 => Some(("Pikachu", 190)),
        26 => Some(("Raichu", 75)),
        27 => Some(("Sandshrew", 255)),
        28 => Some(("Sandslash", 90)),
        29 => Some(("Nidoran F", 235)),
        30 => Some(("Nidorina", 120)),
        31 => Some(("Nidoqueen", 45)),
        32 => Some(("Nidoran M", 235)),
        33 => Some(("Nidorino", 120)),
        34 => Some(("Nidoking", 45)),
        35 => Some(("Clefairy", 150)),
        36 => Some(("Clefable", 25)),
        37 => Some(("Vulpix", 190)),
        38 => Some(("Ninetales", 75)),
        39 => Some(("Jigglypuff", 170)),
        40 => Some(("Wigglytuff", 50)),
        41 => Some(("Zubat", 255)),
        42 => Some(("Golbat", 90)),
        43 => Some(("Oddish", 255)),
        44 => Some(("Gloom", 120)),
        45 => Some(("Vileplume", 45)),
        46 => Some(("Paras", 190)),
        47 => Some(("Parasect", 75)),
        48 => Some(("Venonat", 190)),
        49 => Some(("Venomoth", 75)),
        50 => Some(("Diglett", 255)),
        51 => Some(("Dugtrio", 50)),
        52 => Some(("Meowth", 255)),
        53 => Some(("Persian", 90)),
        54 => Some(("Psyduck", 190)),
        55 => Some(("Golduck", 75)),
        56 => Some(("Mankey", 190)),
        57 => Some(("Primeape", 75)),
        58 => Some(("Growlithe", 190)),
        59 => Some(("Arcanine", 75)),
        60 => Some(("Poliwag", 255)),
        61 => Some(("Poliwhirl", 120)),
        62 => Some(("Poliwrath", 45)),
        63 => Some(("Abra", 200)),
        64 => Some(("Kadabra", 100)),
        65 => Some(("Alakazam", 50)),
        66 => Some(("Machop", 180)),
        67 => Some(("Machoke", 90)),
        68 => Some(("Machamp", 45)),
        69 => Some(("Bellsprout", 255)),
        70 => Some(("Weepinbell", 120)),
        71 => Some(("Victreebel", 45)),
        72 => Some(("Tentacool", 190)),
        73 => Some(("Tentacruel", 60)),
        74 => Some(("Geodude", 255)),
        75 => Some(("Graveler", 120)),
        76 => Some(("Golem", 45)),
        77 => Some(("Ponyta", 190)),
        78 => Some(("Rapidash", 60)),
        79 => Some(("Slowpoke", 190)),
        80 => Some(("Slowbro", 75)),
        81 => Some(("Magnemite", 190)),
        82 => Some(("Magneton", 60)),
        83 => Some(("Farfetch'd", 45)),
        84 => Some(("Doduo", 190)),
        85 => Some(("Dodrio", 45)),
        86 => Some(("Seel", 190)),
        87 => Some(("Dewgong", 75)),
        88 => Some(("Grimer", 190)),
        89 => Some(("Muk", 75)),
        90 => Some(("Shellder", 190)),
        91 => Some(("Cloyster", 60)),
        92 => Some(("Gastly", 190)),
        93 => Some(("Haunter", 90)),
        94 => Some(("Gengar", 45)),
        95 => Some(("Onix", 45)),
        96 => Some(("Drowzee", 190)),
        97 => Some(("Hypno", 75)),
        98 => Some(("Krabby", 225)),
        99 => Some(("Kingler", 60)),
        100 => Some(("Voltorb", 190)),
        101 => Some(("Electrode", 60)),
        102 => Some(("Exeggcute", 90)),
        103 => Some(("Exeggutor", 45)),
        104 => Some(("Cubone", 190)),
        105 => Some(("Marowak", 75)),
        106 => Some(("Hitmonlee", 45)),
        107 => Some(("Hitmonchan", 45)),
        108 => Some(("Lickitung", 45)),
        109 => Some(("Koffing", 190)),
        110 => Some(("Weezing", 60)),
        111 => Some(("Rhyhorn", 120)),
        112 => Some(("Rhydon", 60)),
        113 => Some(("Chansey", 30)),
        114 => Some(("Tangela", 45)),
        115 => Some(("Kangaskhan", 45)),
        116 => Some(("Horsea", 225)),
        117 => Some(("Seadra", 75)),
        118 => Some(("Goldeen", 225)),
        119 => Some(("Seaking", 60)),
        120 => Some(("Staryu", 225)),
        121 => Some(("Starmie", 60)),
        122 => Some(("Mr. Mime", 45)),
        123 => Some(("Scyther", 45)),
        124 => Some(("Jynx", 45)),
        125 => Some(("Electabuzz", 45)),
        126 => Some(("Magmar", 45)),
        127 => Some(("Pinsir", 45)),
        128 => Some(("Tauros", 45)),
        129 => Some(("Magikarp", 255)),
        130 => Some(("Gyarados", 45)),
        131 => Some(("Lapras", 45)),
        132 => Some(("Ditto", 35)),
        133 => Some(("Eevee", 45)),
        134 => Some(("Vaporeon", 45)),
        135 => Some(("Jolteon", 45)),
        136 => Some(("Flareon", 45)),
        137 => Some(("Porygon", 45)),
        138 => Some(("Omanyte", 45)),
        139 => Some(("Omastar", 45)),
        140 => Some(("Kabuto", 45)),
        141 => Some(("Kabutops", 45)),
        142 => Some(("Aerodactyl", 45)),
        143 => Some(("Snorlax", 25)),
        144 => Some(("Articuno", 3)),
        145 => Some(("Zapdos", 3)),
        146 => Some(("Moltres", 3)),
        147 => Some(("Dratini", 45)),
        148 => Some(("Dragonair", 45)),
        149 => Some(("Dragonite", 45)),
        150 => Some(("Mewtwo", 3)),
        151 => Some(("Mew", 45)),
        152 => Some(("Chikorita", 45)),
        153 => Some(("Bayleef", 45)),
        154 => Some(("Meganium", 45)),
        155 => Some(("Cyndaquil", 45)),
        156 => Some(("Quilava", 45)),
        157 => Some(("Typhlosion", 45)),
        158 => Some(("Totodile", 45)),
        159 => Some(("Croconaw", 45)),
        160 => Some(("Feraligatr", 45)),
        161 => Some(("Sentret", 255)),
        162 => Some(("Furret", 90)),
        163 => Some(("Hoothoot", 255)),
        164 => Some(("Noctowl", 90)),
        165 => Some(("Ledyba", 255)),
        166 => Some(("Ledian", 90)),
        167 => Some(("Spinarak", 255)),
        168 => Some(("Ariados", 90)),
        169 => Some(("Crobat", 90)),
        170 => Some(("Chinchou", 190)),
        171 => Some(("Lanturn", 75)),
        172 => Some(("Pichu", 190)),
        173 => Some(("Cleffa", 150)),
        174 => Some(("Igglybuff", 170)),
        175 => Some(("Togepi", 190)),
        176 => Some(("Togetic", 75)),
        177 => Some(("Natu", 190)),
        178 => Some(("Xatu", 75)),
        179 => Some(("Mareep", 235)),
        180 => Some(("Flaaffy", 120)),
        181 => Some(("Ampharos", 45)),
        182 => Some(("Bellossom", 45)),
        183 => Some(("Marill", 190)),
        184 => Some(("Azumarill", 75)),
        185 => Some(("Sudowoodo", 65)),
        186 => Some(("Politoed", 45)),
        187 => Some(("Hoppip", 255)),
        188 => Some(("Skiploom", 120)),
        189 => Some(("Jumpluff", 45)),
        190 => Some(("Aipom", 45)),
        191 => Some(("Sunkern", 235)),
        192 => Some(("Sunflora", 120)),
        193 => Some(("Yanma", 75)),
        194 => Some(("Wooper", 255)),
        195 => Some(("Quagsire", 90)),
        196 => Some(("Espeon", 45)),
        197 => Some(("Umbreon", 45)),
        198 => Some(("Murkrow", 30)),
        199 => Some(("Slowking", 70)),
        200 => Some(("Misdreavus", 45)),
        201 => Some(("Unown", 225)),
        202 => Some(("Wobbuffet", 45)),
        203 => Some(("Girafarig", 60)),
        204 => Some(("Pineco", 190)),
        205 => Some(("Forretress", 75)),
        206 => Some(("Dunsparce", 190)),
        207 => Some(("Gligar", 60)),
        208 => Some(("Steelix", 25)),
        209 => Some(("Snubbull", 190)),
        210 => Some(("Granbull", 75)),
        211 => Some(("Qwilfish", 45)),
        212 => Some(("Scizor", 25)),
        213 => Some(("Shuckle", 190)),
        214 => Some(("Heracross", 45)),
        215 => Some(("Sneasel", 60)),
        216 => Some(("Teddiursa", 120)),
        217 => Some(("Ursaring", 60)),
        218 => Some(("Slugma", 190)),
        219 => Some(("Magcargo", 75)),
        220 => Some(("Swinub", 225)),
        221 => Some(("Piloswine", 75)),
        222 => Some(("Corsola", 60)),
        223 => Some(("Remoraid", 190)),
        224 => Some(("Octillery", 75)),
        225 => Some(("Delibird", 45)),
        226 => Some(("Mantine", 25)),
        227 => Some(("Skarmory", 25)),
        228 => Some(("Houndour", 120)),
        229 => Some(("Houndoom", 45)),
        230 => Some(("Kingdra", 45)),
        231 => Some(("Phanpy", 120)),
        232 => Some(("Donphan", 60)),
        233 => Some(("Porygon2", 45)),
        234 => Some(("Stantler", 45)),
        235 => Some(("Smeargle", 45)),
        236 => Some(("Tyrogue", 75)),
        237 => Some(("Hitmontop", 45)),
        238 => Some(("Smoochum", 45)),
        239 => Some(("Elekid", 45)),
        240 => Some(("Magby", 45)),
        241 => Some(("Miltank", 45)),
        242 => Some(("Blissey", 30)),
        243 => Some(("Raikou", 3)),
        244 => Some(("Entei", 3)),
        245 => Some(("Suicune", 3)),
        246 => Some(("Larvitar", 45)),
        247 => Some(("Pupitar", 45)),
        248 => Some(("Tyranitar", 45)),
        249 => Some(("Lugia", 3)),
        250 => Some(("Ho-Oh", 3)),
        251 => Some(("Celebi", 45)),
        277 => Some(("Treecko", 45)),
        278 => Some(("Grovyle", 45)),
        279 => Some(("Sceptile", 45)),
        280 => Some(("Torchic", 45)),
        281 => Some(("Combusken", 45)),
        282 => Some(("Blaziken", 45)),
        283 => Some(("Mudkip", 45)),
        284 => Some(("Marshtomp", 45)),
        285 => Some(("Swampert", 45)),
        286 => Some(("Poochyena", 255)),
        287 => Some(("Mightyena", 127)),
        288 => Some(("Zigzagoon", 255)),
        289 => Some(("Linoone", 90)),
        290 => Some(("Wurmple", 255)),
        291 => Some(("Silcoon", 120)),
        292 => Some(("Beautifly", 45)),
        293 => Some(("Cascoon", 120)),
        294 => Some(("Dustox", 45)),
        295 => Some(("Lotad", 255)),
        296 => Some(("Lombre", 120)),
        297 => Some(("Ludicolo", 45)),
        298 => Some(("Seedot", 255)),
        299 => Some(("Nuzleaf", 120)),
        300 => Some(("Shiftry", 45)),
        301 => Some(("Taillow", 200)),
        302 => Some(("Swellow", 45)),
        303 => Some(("Wingull", 190)),
        304 => Some(("Pelipper", 45)),
        305 => Some(("Ralts", 235)),
        306 => Some(("Kirlia", 120)),
        307 => Some(("Gardevoir", 45)),
        308 => Some(("Surskit", 200)),
        309 => Some(("Masquerain", 75)),
        310 => Some(("Shroomish", 255)),
        311 => Some(("Breloom", 90)),
        312 => Some(("Slakoth", 255)),
        313 => Some(("Vigoroth", 120)),
        314 => Some(("Slaking", 45)),
        315 => Some(("Nincada", 255)),
        316 => Some(("Ninjask", 120)),
        317 => Some(("Shedinja", 45)),
        318 => Some(("Whismur", 190)),
        319 => Some(("Loudred", 120)),
        320 => Some(("Exploud", 45)),
        321 => Some(("Makuhita", 180)),
        322 => Some(("Hariyama", 200)),
        323 => Some(("Azurill", 150)),
        324 => Some(("Nosepass", 255)),
        325 => Some(("Skitty", 255)),
        326 => Some(("Delcatty", 60)),
        327 => Some(("Sableye", 45)),
        328 => Some(("Mawile", 45)),
        329 => Some(("Aron", 180)),
        330 => Some(("Lairon", 90)),
        331 => Some(("Aggron", 45)),
        332 => Some(("Meditite", 180)),
        333 => Some(("Medicham", 90)),
        334 => Some(("Electrike", 120)),
        335 => Some(("Manectric", 45)),
        336 => Some(("Plusle", 200)),
        337 => Some(("Minun", 200)),
        338 => Some(("Volbeat", 150)),
        339 => Some(("Illumise", 150)),
        340 => Some(("Roselia", 150)),
        341 => Some(("Gulpin", 225)),
        342 => Some(("Swalot", 75)),
        343 => Some(("Carvanha", 225)),
        344 => Some(("Sharpedo", 60)),
        345 => Some(("Wailmer", 125)),
        346 => Some(("Wailord", 60)),
        347 => Some(("Numel", 255)),
        348 => Some(("Camerupt", 150)),
        349 => Some(("Torkoal", 90)),
        350 => Some(("Spoink", 255)),
        351 => Some(("Grumpig", 60)),
        352 => Some(("Spinda", 255)),
        353 => Some(("Trapinch", 255)),
        354 => Some(("Vibrava", 120)),
        355 => Some(("Flygon", 45)),
        356 => Some(("Cacnea", 190)),
        357 => Some(("Cacturne", 60)),
        358 => Some(("Swablu", 255)),
        359 => Some(("Altaria", 45)),
        360 => Some(("Zangoose", 90)),
        361 => Some(("Seviper", 90)),
        362 => Some(("Lunatone", 45)),
        363 => Some(("Solrock", 45)),
        364 => Some(("Barboach", 190)),
        365 => Some(("Whiscash", 75)),
        366 => Some(("Corphish", 205)),
        367 => Some(("Crawdaunt", 155)),
        368 => Some(("Baltoy", 255)),
        369 => Some(("Claydol", 90)),
        370 => Some(("Lileep", 45)),
        371 => Some(("Cradily", 45)),
        372 => Some(("Anorith", 45)),
        373 => Some(("Armaldo", 45)),
        374 => Some(("Feebas", 255)),
        375 => Some(("Milotic", 60)),
        376 => Some(("Castform", 45)),
        377 => Some(("Kecleon", 200)),
        378 => Some(("Shuppet", 225)),
        379 => Some(("Banette", 45)),
        380 => Some(("Duskull", 190)),
        381 => Some(("Dusclops", 90)),
        382 => Some(("Tropius", 200)),
        383 => Some(("Chimecho", 45)),
        384 => Some(("Absol", 30)),
        385 => Some(("Wynaut", 125)),
        386 => Some(("Snorunt", 190)),
        387 => Some(("Glalie", 75)),
        388 => Some(("Spheal", 255)),
        389 => Some(("Sealeo", 120)),
        390 => Some(("Walrein", 45)),
        391 => Some(("Clamperl", 255)),
        392 => Some(("Huntail", 60)),
        393 => Some(("Gorebyss", 60)),
        394 => Some(("Relicanth", 25)),
        395 => Some(("Luvdisc", 225)),
        396 => Some(("Bagon", 45)),
        397 => Some(("Shelgon", 45)),
        398 => Some(("Salamence", 45)),
        399 => Some(("Beldum", 3)),
        400 => Some(("Metang", 3)),
        401 => Some(("Metagross", 3)),
        402 => Some(("Regirock", 3)),
        403 => Some(("Regice", 3)),
        404 => Some(("Registeel", 3)),
        405 => Some(("Latias", 3)),
        406 => Some(("Latios", 3)),
        407 => Some(("Kyogre", 5)),
        408 => Some(("Groudon", 5)),
        409 => Some(("Rayquaza", 45)),
        410 => Some(("Jirachi", 3)),
        411 => Some(("Deoxys", 3)),
        _ => None,
    }
}

fn locate_fire_red_party(memory: &dyn MemoryView) -> Option<(u32, Vec<FireRedPartyMember>)> {
    let expected_party_count = read_party_count(memory);
    let mut best_candidate =
        scan_party_candidate_at(memory, FIRE_RED_PARTY_BASE, expected_party_count);

    let last_candidate =
        FIRE_RED_PARTY_SCAN_END.saturating_sub((PARTY_SLOT_SIZE * PARTY_SLOT_COUNT) as u32);
    for base in (FIRE_RED_PARTY_SCAN_START..=last_candidate).step_by(4) {
        let Some(candidate) = scan_party_candidate_at(memory, base, expected_party_count) else {
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

fn read_party_count(memory: &dyn MemoryView) -> Option<usize> {
    let count = memory.read_u8(FIRE_RED_PARTY_COUNT_ADDR) as usize;
    (1..=PARTY_SLOT_COUNT).contains(&count).then_some(count)
}

fn parse_party_at(memory: &dyn MemoryView, base: u32, party_count: usize) -> Option<Vec<FireRedPartyMember>> {
    let mut party = Vec::with_capacity(party_count);
    for slot in 0..party_count {
        let raw = read_party_slot(memory, base + (slot * PARTY_SLOT_SIZE) as u32);
        let Some(member) = parse_party_member(&raw, slot as u8 + 1) else {
            return None;
        };
        party.push(member);
    }

    Some(party)
}

fn parse_party_prefix_at(memory: &dyn MemoryView, base: u32) -> Option<Vec<FireRedPartyMember>> {
    let mut party = Vec::with_capacity(PARTY_SLOT_COUNT);
    for slot in 0..PARTY_SLOT_COUNT {
        let raw = read_party_slot(memory, base + (slot * PARTY_SLOT_SIZE) as u32);
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
    memory: &dyn MemoryView,
    base: u32,
    expected_party_count: Option<usize>,
) -> Option<PartyCandidate> {
    let prefix_party = parse_party_prefix_at(memory, base)?;
    let exact_party = expected_party_count.and_then(|count| parse_party_at(memory, base, count));

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

fn read_party_slot(memory: &dyn MemoryView, addr: u32) -> [u8; PARTY_SLOT_SIZE] {
    let mut raw = [0u8; PARTY_SLOT_SIZE];
    for (idx, byte) in raw.iter_mut().enumerate() {
        *byte = memory.read_u8(addr + idx as u32);
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
                name: move_display_name(move_id),
                pp,
            })
        })
        .collect()
}

/// The species name, preferring what the cartridge says.
///
/// The compiled table is a fallback, not the source of truth: it is right for
/// the retail games and says nothing about a ROM hack, which the cartridge
/// itself always describes correctly.
fn species_display_name(species_id: u16) -> String {
    gen3_names::species(species_id)
        .or_else(|| fallback_species_info(species_id).map(|(name, _)| name.to_string()))
        .unwrap_or_else(|| format!("Species #{species_id:03}"))
}

fn held_item(item_id: u16) -> Option<FireRedHeldItem> {
    (item_id != 0).then(|| FireRedHeldItem {
        item_id,
        name: item_display_name(item_id),
    })
}

/// The item name, preferring what the cartridge says.
///
/// This is the table the compiled fallback covers worst — a few dozen of the
/// several hundred a Generation 3 game has — which is exactly why reading it
/// out of the ROM was worth doing.
fn item_display_name(item_id: u16) -> String {
    gen3_names::item(item_id)
        .unwrap_or_else(|| fallback_item_name(item_id).to_string())
}

fn fallback_item_name(item_id: u16) -> &'static str {
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

/// Every move in Generation 3, by index, with the PP it starts with.
///
/// One table rather than two, so a name and its PP cannot drift apart — the
/// pair used to be separate `match` arms and only nineteen moves deep, which
/// is why anything past the first couple of routes read as "Unknown Move".
///
/// The values are the Generation 3 move data. They are not read out of the
/// cartridge: the name and PP tables live at addresses that move between
/// FireRed, LeafGreen and their revisions, and a wrong address would give
/// confident nonsense where this gives a known answer. Reading them from the
/// ROM is the better long-term fix and is written up in `ADDONS.md`.
fn move_data(move_id: u16) -> Option<(&'static str, u8)> {
    Some(match move_id {
        1 => ("Pound", 35),
        2 => ("Karate Chop", 25),
        3 => ("Double Slap", 10),
        4 => ("Comet Punch", 15),
        5 => ("Mega Punch", 20),
        6 => ("Pay Day", 20),
        7 => ("Fire Punch", 15),
        8 => ("Ice Punch", 15),
        9 => ("Thunder Punch", 15),
        10 => ("Scratch", 35),
        11 => ("Vice Grip", 30),
        12 => ("Guillotine", 5),
        13 => ("Razor Wind", 10),
        14 => ("Swords Dance", 30),
        15 => ("Cut", 30),
        16 => ("Gust", 35),
        17 => ("Wing Attack", 35),
        18 => ("Whirlwind", 20),
        19 => ("Fly", 15),
        20 => ("Bind", 20),
        21 => ("Slam", 20),
        22 => ("Vine Whip", 25),
        23 => ("Stomp", 20),
        24 => ("Double Kick", 30),
        25 => ("Mega Kick", 5),
        26 => ("Jump Kick", 25),
        27 => ("Rolling Kick", 15),
        28 => ("Sand-Attack", 15),
        29 => ("Headbutt", 15),
        30 => ("Horn Attack", 25),
        31 => ("Fury Attack", 20),
        32 => ("Horn Drill", 5),
        33 => ("Tackle", 35),
        34 => ("Body Slam", 15),
        35 => ("Wrap", 20),
        36 => ("Take Down", 20),
        37 => ("Thrash", 20),
        38 => ("Double-Edge", 15),
        39 => ("Tail Whip", 30),
        40 => ("Poison Sting", 35),
        41 => ("Twineedle", 20),
        42 => ("Pin Missile", 20),
        43 => ("Leer", 30),
        44 => ("Bite", 25),
        45 => ("Growl", 40),
        46 => ("Roar", 20),
        47 => ("Sing", 15),
        48 => ("Supersonic", 20),
        49 => ("Sonic Boom", 20),
        50 => ("Disable", 20),
        51 => ("Acid", 30),
        52 => ("Ember", 25),
        53 => ("Flamethrower", 15),
        54 => ("Mist", 30),
        55 => ("Water Gun", 25),
        56 => ("Hydro Pump", 5),
        57 => ("Surf", 15),
        58 => ("Ice Beam", 10),
        59 => ("Blizzard", 5),
        60 => ("Psybeam", 20),
        61 => ("Bubble Beam", 20),
        62 => ("Aurora Beam", 20),
        63 => ("Hyper Beam", 5),
        64 => ("Peck", 35),
        65 => ("Drill Peck", 20),
        66 => ("Submission", 25),
        67 => ("Low Kick", 20),
        68 => ("Counter", 20),
        69 => ("Seismic Toss", 20),
        70 => ("Strength", 15),
        71 => ("Absorb", 25),
        72 => ("Mega Drain", 15),
        73 => ("Leech Seed", 10),
        74 => ("Growth", 40),
        75 => ("Razor Leaf", 25),
        76 => ("Solar Beam", 10),
        77 => ("Poison Powder", 35),
        78 => ("Stun Spore", 30),
        79 => ("Sleep Powder", 15),
        80 => ("Petal Dance", 20),
        81 => ("String Shot", 40),
        82 => ("Dragon Rage", 10),
        83 => ("Fire Spin", 15),
        84 => ("Thunder Shock", 30),
        85 => ("Thunderbolt", 15),
        86 => ("Thunder Wave", 20),
        87 => ("Thunder", 10),
        88 => ("Rock Throw", 15),
        89 => ("Earthquake", 10),
        90 => ("Fissure", 5),
        91 => ("Dig", 10),
        92 => ("Toxic", 10),
        93 => ("Confusion", 25),
        94 => ("Psychic", 10),
        95 => ("Hypnosis", 20),
        96 => ("Meditate", 40),
        97 => ("Agility", 30),
        98 => ("Quick Attack", 30),
        99 => ("Rage", 20),
        100 => ("Teleport", 20),
        101 => ("Night Shade", 15),
        102 => ("Mimic", 10),
        103 => ("Screech", 40),
        104 => ("Double Team", 15),
        105 => ("Recover", 20),
        106 => ("Harden", 30),
        107 => ("Minimize", 20),
        108 => ("Smokescreen", 20),
        109 => ("Confuse Ray", 10),
        110 => ("Withdraw", 40),
        111 => ("Defense Curl", 40),
        112 => ("Barrier", 30),
        113 => ("Light Screen", 30),
        114 => ("Haze", 30),
        115 => ("Reflect", 20),
        116 => ("Focus Energy", 30),
        117 => ("Bide", 10),
        118 => ("Metronome", 10),
        119 => ("Mirror Move", 20),
        120 => ("Self-Destruct", 5),
        121 => ("Egg Bomb", 10),
        122 => ("Lick", 30),
        123 => ("Smog", 20),
        124 => ("Sludge", 20),
        125 => ("Bone Club", 20),
        126 => ("Fire Blast", 5),
        127 => ("Waterfall", 15),
        128 => ("Clamp", 10),
        129 => ("Swift", 20),
        130 => ("Skull Bash", 15),
        131 => ("Spike Cannon", 15),
        132 => ("Constrict", 35),
        133 => ("Amnesia", 20),
        134 => ("Kinesis", 15),
        135 => ("Soft-Boiled", 10),
        136 => ("High Jump Kick", 20),
        137 => ("Glare", 30),
        138 => ("Dream Eater", 15),
        139 => ("Poison Gas", 40),
        140 => ("Barrage", 20),
        141 => ("Leech Life", 15),
        142 => ("Lovely Kiss", 10),
        143 => ("Sky Attack", 5),
        144 => ("Transform", 10),
        145 => ("Bubble", 30),
        146 => ("Dizzy Punch", 10),
        147 => ("Spore", 15),
        148 => ("Flash", 20),
        149 => ("Psywave", 15),
        150 => ("Splash", 40),
        151 => ("Acid Armor", 40),
        152 => ("Crabhammer", 10),
        153 => ("Explosion", 5),
        154 => ("Fury Swipes", 15),
        155 => ("Bonemerang", 10),
        156 => ("Rest", 10),
        157 => ("Rock Slide", 10),
        158 => ("Hyper Fang", 15),
        159 => ("Sharpen", 30),
        160 => ("Conversion", 30),
        161 => ("Tri Attack", 10),
        162 => ("Super Fang", 10),
        163 => ("Slash", 20),
        164 => ("Substitute", 10),
        165 => ("Struggle", 1),
        166 => ("Sketch", 1),
        167 => ("Triple Kick", 10),
        168 => ("Thief", 10),
        169 => ("Spider Web", 10),
        170 => ("Mind Reader", 5),
        171 => ("Nightmare", 15),
        172 => ("Flame Wheel", 25),
        173 => ("Snore", 15),
        174 => ("Curse", 10),
        175 => ("Flail", 15),
        176 => ("Conversion 2", 30),
        177 => ("Aeroblast", 5),
        178 => ("Cotton Spore", 40),
        179 => ("Reversal", 15),
        180 => ("Spite", 10),
        181 => ("Powder Snow", 25),
        182 => ("Protect", 10),
        183 => ("Mach Punch", 30),
        184 => ("Scary Face", 10),
        185 => ("Faint Attack", 20),
        186 => ("Sweet Kiss", 10),
        187 => ("Belly Drum", 10),
        188 => ("Sludge Bomb", 10),
        189 => ("Mud-Slap", 10),
        190 => ("Octazooka", 10),
        191 => ("Spikes", 20),
        192 => ("Zap Cannon", 5),
        193 => ("Foresight", 40),
        194 => ("Destiny Bond", 5),
        195 => ("Perish Song", 5),
        196 => ("Icy Wind", 15),
        197 => ("Detect", 5),
        198 => ("Bone Rush", 10),
        199 => ("Lock-On", 5),
        200 => ("Outrage", 15),
        201 => ("Sandstorm", 10),
        202 => ("Giga Drain", 5),
        203 => ("Endure", 10),
        204 => ("Charm", 20),
        205 => ("Rollout", 20),
        206 => ("False Swipe", 40),
        207 => ("Swagger", 15),
        208 => ("Milk Drink", 10),
        209 => ("Spark", 20),
        210 => ("Fury Cutter", 20),
        211 => ("Steel Wing", 25),
        212 => ("Mean Look", 5),
        213 => ("Attract", 15),
        214 => ("Sleep Talk", 10),
        215 => ("Heal Bell", 5),
        216 => ("Return", 20),
        217 => ("Present", 15),
        218 => ("Frustration", 20),
        219 => ("Safeguard", 25),
        220 => ("Pain Split", 20),
        221 => ("Sacred Fire", 5),
        222 => ("Magnitude", 30),
        223 => ("Dynamic Punch", 5),
        224 => ("Megahorn", 10),
        225 => ("Dragon Breath", 20),
        226 => ("Baton Pass", 40),
        227 => ("Encore", 5),
        228 => ("Pursuit", 20),
        229 => ("Rapid Spin", 40),
        230 => ("Sweet Scent", 20),
        231 => ("Iron Tail", 15),
        232 => ("Metal Claw", 35),
        233 => ("Vital Throw", 10),
        234 => ("Morning Sun", 5),
        235 => ("Synthesis", 5),
        236 => ("Moonlight", 5),
        237 => ("Hidden Power", 15),
        238 => ("Cross Chop", 5),
        239 => ("Twister", 20),
        240 => ("Rain Dance", 5),
        241 => ("Sunny Day", 5),
        242 => ("Crunch", 15),
        243 => ("Mirror Coat", 20),
        244 => ("Psych Up", 10),
        245 => ("Extreme Speed", 5),
        246 => ("Ancient Power", 5),
        247 => ("Shadow Ball", 15),
        248 => ("Future Sight", 15),
        249 => ("Rock Smash", 15),
        250 => ("Whirlpool", 15),
        251 => ("Beat Up", 10),
        252 => ("Fake Out", 10),
        253 => ("Uproar", 10),
        254 => ("Stockpile", 20),
        255 => ("Spit Up", 10),
        256 => ("Swallow", 10),
        257 => ("Heat Wave", 10),
        258 => ("Hail", 10),
        259 => ("Torment", 15),
        260 => ("Flatter", 15),
        261 => ("Will-O-Wisp", 15),
        262 => ("Memento", 10),
        263 => ("Facade", 20),
        264 => ("Focus Punch", 20),
        265 => ("Smelling Salts", 10),
        266 => ("Follow Me", 20),
        267 => ("Nature Power", 20),
        268 => ("Charge", 20),
        269 => ("Taunt", 20),
        270 => ("Helping Hand", 20),
        271 => ("Trick", 10),
        272 => ("Role Play", 10),
        273 => ("Wish", 10),
        274 => ("Assist", 20),
        275 => ("Ingrain", 20),
        276 => ("Superpower", 5),
        277 => ("Magic Coat", 15),
        278 => ("Recycle", 10),
        279 => ("Revenge", 10),
        280 => ("Brick Break", 15),
        281 => ("Yawn", 10),
        282 => ("Knock Off", 20),
        283 => ("Endeavor", 5),
        284 => ("Eruption", 5),
        285 => ("Skill Swap", 10),
        286 => ("Imprison", 10),
        287 => ("Refresh", 20),
        288 => ("Grudge", 5),
        289 => ("Snatch", 10),
        290 => ("Secret Power", 20),
        291 => ("Dive", 10),
        292 => ("Arm Thrust", 20),
        293 => ("Camouflage", 20),
        294 => ("Tail Glow", 20),
        295 => ("Luster Purge", 5),
        296 => ("Mist Ball", 5),
        297 => ("Feather Dance", 15),
        298 => ("Teeter Dance", 20),
        299 => ("Blaze Kick", 10),
        300 => ("Mud Sport", 15),
        301 => ("Ice Ball", 20),
        302 => ("Needle Arm", 15),
        303 => ("Slack Off", 10),
        304 => ("Hyper Voice", 10),
        305 => ("Poison Fang", 15),
        306 => ("Crush Claw", 10),
        307 => ("Blast Burn", 5),
        308 => ("Hydro Cannon", 5),
        309 => ("Meteor Mash", 10),
        310 => ("Astonish", 15),
        311 => ("Weather Ball", 10),
        312 => ("Aromatherapy", 5),
        313 => ("Fake Tears", 20),
        314 => ("Air Cutter", 25),
        315 => ("Overheat", 5),
        316 => ("Odor Sleuth", 40),
        317 => ("Rock Tomb", 10),
        318 => ("Silver Wind", 5),
        319 => ("Metal Sound", 40),
        320 => ("Grass Whistle", 15),
        321 => ("Tickle", 20),
        322 => ("Cosmic Power", 20),
        323 => ("Water Spout", 5),
        324 => ("Signal Beam", 15),
        325 => ("Shadow Punch", 20),
        326 => ("Extrasensory", 30),
        327 => ("Sky Uppercut", 15),
        328 => ("Sand Tomb", 15),
        329 => ("Sheer Cold", 5),
        330 => ("Muddy Water", 10),
        331 => ("Bullet Seed", 30),
        332 => ("Aerial Ace", 20),
        333 => ("Icicle Spear", 30),
        334 => ("Iron Defense", 15),
        335 => ("Block", 5),
        336 => ("Howl", 40),
        337 => ("Dragon Claw", 15),
        338 => ("Frenzy Plant", 5),
        339 => ("Bulk Up", 20),
        340 => ("Bounce", 5),
        341 => ("Mud Shot", 15),
        342 => ("Poison Tail", 25),
        343 => ("Covet", 40),
        344 => ("Volt Tackle", 15),
        345 => ("Magical Leaf", 20),
        346 => ("Water Sport", 15),
        347 => ("Calm Mind", 20),
        348 => ("Leaf Blade", 15),
        349 => ("Dragon Dance", 20),
        350 => ("Rock Blast", 10),
        351 => ("Shock Wave", 20),
        352 => ("Water Pulse", 20),
        353 => ("Doom Desire", 5),
        354 => ("Psycho Boost", 5),
        _ => return None,
    })
}

/// The move name, preferring what the cartridge says.
///
/// PP still comes from the compiled table: it lives in a different structure
/// with its own layout, and a wrong bar is a worse trade than a right name.
fn move_display_name(move_id: u16) -> String {
    gen3_names::move_name(move_id)
        .or_else(|| move_data(move_id).map(|(name, _)| name.to_string()))
        .unwrap_or_else(|| format!("Move #{move_id:03}"))
}

/// Starting PP, so a move's remaining PP has something to be a fraction of.
fn move_max_pp(move_id: u16) -> Option<u8> {
    move_data(move_id).map(|(_, pp)| pp)
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
    use crate::GbaMemory;
    use tinybird_addons::schema::{AddonMeter, AddonSectionContent};
    use std::path::Path;
    use tinybird_core::Gba;

    /// Adapt a real emulator to the read-only view addons are given.
    ///
    /// These tests drive a `Gba` because they exercise the memory *layout*
    /// rather than the parsing helpers; the parsing itself is covered against
    /// `SparseMemory` in the newer addon modules.
    fn view(gba: &Gba) -> GbaMemory<'_> {
        GbaMemory(gba)
    }

    // --- section building ------------------------------------------------
    //
    // The sections are what every consumer outside the desktop app draws, and
    // they are pure functions of the parsed snapshot. Building the snapshot
    // directly tests the read-out without needing a booted ROM behind it.

    fn spread(value: u16) -> FireRedStatSpread {
        FireRedStatSpread {
            hp: value,
            attack: value,
            defense: value,
            speed: value,
            sp_attack: value,
            sp_def: value,
        }
    }

    fn member(slot: u8, current_hp: u16, max_hp: u16) -> FireRedPartyMember {
        FireRedPartyMember {
            slot,
            nickname: format!("MON{slot}"),
            species_id: 16,
            species_name: "Pidgey".to_string(),
            personality: 0,
            ot_id: 0,
            level: 10 + u8::from(slot),
            current_hp,
            max_hp,
            status: 0,
            is_egg: false,
            nature: "Brave",
            ability_slot: 0,
            ability_name: "Keen Eye".to_string(),
            held_item: None,
            stats: spread(20),
            evs: spread(0),
            ivs: spread(31),
            ev_total: 0,
            iv_total: 186,
            moves: [33, 0, 0, 0],
            move_slots: move_slots([33, 0, 0, 0], [20, 0, 0, 0]),
        }
    }

    fn route_with(entries: Vec<FireRedEncounterEntry>) -> FireRedAreaSnapshot {
        FireRedAreaSnapshot {
            map_group: 3,
            map_num: 19,
            map_key: "3:19".to_string(),
            name: "Route 1".to_string(),
            encounter_groups: vec![FireRedEncounterGroup {
                method: "Grass",
                encounter_rate: 25,
                entries,
            }],
        }
    }

    fn battle_with(ivs: FireRedStatSpread, iv_total: u16) -> FireRedBattleSnapshot {
        FireRedBattleSnapshot {
            battle_type_flags: 0,
            battle_kind: "Wild",
            catchable: true,
            opponent: FireRedBattleOpponent {
                nickname: "Pidgey".to_string(),
                species_id: 16,
                species_name: "Pidgey".to_string(),
                level: 3,
                current_hp: 12,
                max_hp: 12,
                status: 0,
                nature: "Timid",
                ability_slot: 0,
                ability_name: "Keen Eye".to_string(),
                held_item: None,
                stats: spread(10),
                evs: spread(0),
                ivs,
                ev_total: 0,
                iv_total,
                moves: [33, 0, 0, 0],
                move_slots: move_slots([33, 0, 0, 0], [20, 0, 0, 0]),
                catch_rate: Some(255),
            },
        }
    }

    fn snapshot_of(party: Vec<FireRedPartyMember>) -> FireRedSnapshot {
        FireRedSnapshot {
            source: "test",
            party_base_address: 0x0202_4284,
            party,
            area: None,
            battle: None,
        }
    }

    fn section<'a>(sections: &'a [AddonSection], id: &str) -> &'a AddonSection {
        sections
            .iter()
            .find(|section| section.section_id == id)
            .unwrap_or_else(|| panic!("no {id} section"))
    }

    fn field<'a>(fields: &'a [AddonField], label: &str) -> &'a AddonField {
        fields
            .iter()
            .find(|field| field.label == label)
            .unwrap_or_else(|| panic!("no {label} field"))
    }

    fn cards(section: &AddonSection) -> &[AddonCard] {
        match &section.content {
            AddonSectionContent::Cards(cards) => cards,
            other => panic!("expected cards, got {other:?}"),
        }
    }

    #[test]
    fn a_party_member_reports_hp_as_a_bar_and_its_detail_behind_it() {
        let sections = fire_red_sections(&snapshot_of(vec![member(1, 9, 36)]));
        let card = &cards(section(&sections, "party"))[0];

        assert_eq!(card.title, "MON1");
        // Species and level, not a slot number you could get by counting.
        assert_eq!(card.subtitle.as_deref(), Some("Pidgey \u{00b7} Lv 11"));

        // The picture is addressed by species, not by slot: two Pidgey in the
        // party are the same sprite and should hit the same cache entry.
        let image = card.image.as_ref().expect("species sprite");
        assert_eq!(image.src, "/sprites/16");
        assert_eq!(image.alt.as_deref(), Some("Pidgey"));

        let hp = card.lead.as_ref().expect("HP lead");
        assert_eq!(hp.value, "9/36");
        assert_eq!(hp.meter, Some(AddonMeter::new(9, 36)));
        assert_eq!(hp.tone, AddonTone::Warn);

        // Everything the slot parser recovered, not just name and level.
        for label in ["Ability", "IVs", "EVs", "Move 1"] {
            field(&card.fields, label);
        }
        assert_eq!(field(&card.fields, "IVs").meter, Some(AddonMeter::new(186, 186)));

        // Every stat says which stat it is, and carries its own IV and EV.
        // These used to be three rows of six bare numbers under one legend,
        // which meant counting columns to answer "is my Speed IV any good".
        for label in ["HP", "Attack", "Defense", "Sp. Atk", "Sp. Def", "Speed"] {
            let row = field(&card.fields, label);
            assert_eq!(row.value, "20", "{label} should carry its own value");
            assert_eq!(row.hint.as_deref(), Some("IV 31 \u{00b7} EV 0"));
        }
    }

    /// The status word has always been parsed and was never shown anywhere.
    #[test]
    fn a_status_condition_becomes_a_badge_rather_than_staying_in_the_payload() {
        let mut poisoned = member(1, 20, 36);
        poisoned.status = STATUS_TOXIC;

        let sections = fire_red_sections(&snapshot_of(vec![poisoned]));
        let card = &cards(section(&sections, "party"))[0];

        assert_eq!(card.badges[0].text, "Badly poisoned");
        assert_eq!(card.badges[0].tone, AddonTone::Bad);
        // The nature still rides along behind it.
        assert_eq!(card.badges[1].text, "Brave");
    }

    #[test]
    fn a_fainted_member_is_flagged_as_fainted_not_as_its_status() {
        let mut fainted = member(1, 0, 36);
        fainted.status = STATUS_POISON;

        let sections = fire_red_sections(&snapshot_of(vec![fainted]));
        let card = &cards(section(&sections, "party"))[0];

        assert_eq!(card.badges[0].text, "Fainted");
        assert_eq!(card.lead.as_ref().unwrap().tone, AddonTone::Bad);
    }

    /// An egg has no readable stats, so printing zeroes for one would read as
    /// a parser failure rather than as an egg.
    #[test]
    fn an_egg_reports_as_an_egg_and_is_left_out_of_team_health() {
        let mut egg = member(2, 0, 0);
        egg.is_egg = true;

        let sections = fire_red_sections(&snapshot_of(vec![member(1, 36, 36), egg]));

        let card = &cards(section(&sections, "party"))[1];
        assert_eq!(card.subtitle.as_deref(), Some("Egg"));
        assert!(card.fields.is_empty());
        assert!(
            card.image.is_none(),
            "showing the species would give away what is in the egg"
        );

        // The healthy member alone, not averaged against an egg with no HP.
        let note = section(&sections, "party").note.as_deref().unwrap_or("");
        assert!(note.contains("36/36 HP"), "{note}");
        assert!(note.contains("1 egg"), "{note}");
        assert!(!note.contains("fainted"), "an egg cannot faint: {note}");
    }

    /// The team used to be a section of its own. It is a caption over the party
    /// now, because every number in it is arithmetic on the cards underneath.
    #[test]
    fn the_team_summary_is_a_line_over_the_party_rather_than_a_tab_of_its_own() {
        let mut asleep = member(2, 20, 36);
        asleep.status = 2; // two turns of sleep left

        let sections = fire_red_sections(&snapshot_of(vec![member(1, 0, 30), asleep]));

        assert!(
            !sections.iter().any(|section| section.section_id == "summary"),
            "the team should not be a section any more"
        );

        let note = section(&sections, "party").note.as_deref().unwrap_or("");
        assert!(note.starts_with("2/6"), "{note}");
        assert!(note.contains("1 fainted"), "{note}");
        assert!(note.contains("1 ailing"), "{note}");
    }

    /// A healthy team says nothing beyond the numbers: the bars under it have
    /// already said it, and words that only ever mean "fine" are words to skip.
    #[test]
    fn a_healthy_team_is_not_described_as_healthy() {
        let sections = fire_red_sections(&snapshot_of(vec![member(1, 30, 30)]));
        let note = section(&sections, "party").note.as_deref().unwrap_or("");

        assert_eq!(note, "1/6 - 30/30 HP - avg Lv11");
    }

    /// A battle is what is on screen right now, so it outranks the standing
    /// read-out of the team.
    /// The tab is a fixture. One that came and went took its tab with it, and a
    /// strip that changes width whenever a wild Pokemon appears moves every
    /// other tab out from under the cursor.
    /// The dex tab is a fixture. One that came and went took its tab with it,
    /// and a strip that changes width whenever a wild Pokemon appears moves
    /// every other tab out from under the cursor.
    #[test]
    fn the_dex_is_there_with_no_battle_and_no_area() {
        let sections = fire_red_sections(&snapshot_of(vec![member(1, 30, 30)]));

        let ids: Vec<_> = sections.iter().map(|s| s.section_id).collect();
        assert_eq!(ids, vec!["party", "dex"]);

        let dex = section(&sections, "dex");
        assert!(dex.badge.is_none(), "nothing to flag when idle");
        match &dex.content {
            AddonSectionContent::List(lines) => assert_eq!(lines[0], "Nothing to report yet."),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    /// One tab, two answers. They are never both the answer at once: an area
    /// list is no use while a Rattata is on screen.
    #[test]
    fn the_dex_shows_the_battle_when_there_is_one_and_the_area_otherwise() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.area = Some(route_with(vec![encounter(16, "Pidgey", 2, 5, 45, 255)]));

        // Out of battle, it is the route.
        let sections = fire_red_sections(&snapshot);
        assert_eq!(
            section(&sections, "dex").note.as_deref(),
            Some("Route 1 (3:19) - 1 species")
        );
        assert_eq!(cards(section(&sections, "dex"))[0].title, "Pidgey");

        // In one, the same tab is the opponent.
        snapshot.battle = Some(battle_with(spread(10), 60));
        let sections = fire_red_sections(&snapshot);
        assert_eq!(
            section(&sections, "dex").note.as_deref(),
            Some("Wild battle in progress")
        );
        // One card, which is what has a consumer draw it as the featured thing
        // rather than as a row in a list.
        assert_eq!(cards(section(&sections, "dex")).len(), 1);
        assert_eq!(cards(section(&sections, "dex"))[0].title, "Pidgey");
    }

    /// Two 31s, or an exceptional total, is worth leaving another tab for.
    #[test]
    fn notable_ivs_are_flagged_on_the_section_as_well_as_the_card() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.battle = Some(battle_with(spread(31), 186));

        let sections = fire_red_sections(&snapshot);
        let battle = section(&sections, "dex");

        // On the tab, so it is visible from whichever section is open...
        let badge = battle.badge.as_ref().expect("a flag on the section");
        assert_eq!(badge.text, "Perfect IVs");
        assert_eq!(badge.tone, AddonTone::Good);

        // ...and on the card, for once the section is open.
        assert!(cards(battle)[0]
            .badges
            .iter()
            .any(|chip| chip.text == "Perfect IVs"));
    }

    #[test]
    fn iv_quality_reads_maxed_stats_and_the_total_separately() {
        // Three 31s and nothing else is a breeding catch, and misses on total.
        let mut mixed = spread(0);
        mixed.hp = 31;
        mixed.attack = 31;
        mixed.speed = 31;
        assert_eq!(
            iv_quality(&mixed, 93),
            Some(("3 perfect IVs".to_string(), AddonTone::Good))
        );

        // A high total with nothing maxed still counts.
        assert_eq!(
            iv_quality(&spread(27), 162),
            Some(("Great IVs".to_string(), AddonTone::Good))
        );

        // One maxed stat is worth a mention, and no more than that.
        let mut single = spread(4);
        single.speed = 31;
        assert_eq!(
            iv_quality(&single, 51),
            Some(("1 perfect IV".to_string(), AddonTone::Good))
        );

        // An ordinary wild Pokemon says nothing: a flag that is always there
        // is a flag nobody sees.
        assert_eq!(iv_quality(&spread(12), 72), None);
    }

    #[test]
    fn an_active_battle_is_reported_before_anything_else() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.battle = Some(FireRedBattleSnapshot {
            battle_type_flags: 0,
            battle_kind: "Wild",
            catchable: true,
            opponent: FireRedBattleOpponent {
                nickname: "Pidgey".to_string(),
                species_id: 16,
                species_name: "Pidgey".to_string(),
                level: 3,
                current_hp: 2,
                max_hp: 12,
                status: 0,
                nature: "Timid",
                ability_slot: 0,
                ability_name: "Keen Eye".to_string(),
                held_item: None,
                stats: spread(10),
                evs: spread(0),
                ivs: spread(15),
                ev_total: 0,
                iv_total: 90,
                moves: [33, 0, 0, 0],
                move_slots: move_slots([33, 0, 0, 0], [20, 0, 0, 0]),
                catch_rate: Some(255),
            },
        });

        let sections = fire_red_sections(&snapshot);
        let battle = section(&sections, "dex");

        let card = &cards(battle)[0];
        // A nickname equal to the species is not worth printing twice.
        assert_eq!(card.title, "Pidgey");
        assert_eq!(card.badges[0].text, "Wild");

        // The reason catch rate is read live at all.
        let catch = field(&card.fields, "Catch chance");
        assert_eq!(catch.value, "good, weakened");
        assert_eq!(catch.tone, AddonTone::Good);
        assert_eq!(catch.meter, Some(AddonMeter::new(255, 255)));

        // A wild Pokemon has no effort values, so its rows do not pretend to.
        assert_eq!(field(&card.fields, "Speed").hint.as_deref(), Some("IV 15"));
    }

    #[test]
    fn catch_difficulty_reads_the_rate_and_the_damage_together() {
        // A hard catch at full health, the same catch nearly fainted.
        assert_eq!(catch_difficulty(10, 40, 40).0, "very hard");
        assert_eq!(catch_difficulty(10, 5, 40).0, "hard, weakened");
        assert_eq!(catch_difficulty(255, 40, 40).0, "fair");
    }

    /// The old read-out flattened every method into one list of strings, which
    /// lost the per-method encounter rate and buried the common slots.
    #[test]
    fn encounters_split_by_method_and_lead_with_the_likeliest_slot() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.area = Some(FireRedAreaSnapshot {
            map_group: 3,
            map_num: 19,
            map_key: "3:19".to_string(),
            name: "Route 1".to_string(),
            encounter_groups: vec![FireRedEncounterGroup {
                method: "Grass",
                encounter_rate: 25,
                entries: vec![
                    encounter(19, "Rattata", 2, 4, 15, 255),
                    encounter(16, "Pidgey", 2, 5, 45, 255),
                ],
            }],
        });

        let sections = fire_red_sections(&snapshot);

        // One Area section, not Area plus a section per method.
        assert_eq!(
            sections
                .iter()
                .filter(|section| section.section_id == "dex")
                .count(),
            1,
        );

        let area = section(&sections, "dex");
        assert_eq!(area.note.as_deref(), Some("Route 1 (3:19) - 2 species"));

        match &area.content {
            AddonSectionContent::Cards(cards) => {
                // Sorted by slot rate: the one actually walked into comes first.
                assert_eq!(cards[0].title, "Pidgey");
                assert_eq!(cards[0].subtitle.as_deref(), Some("Lv 2-5"));
                assert_eq!(cards[1].title, "Rattata");

                // The picture is the point of a card here: recognising what is
                // in the grass is faster than reading a column of names.
                assert_eq!(cards[0].image.as_ref().unwrap().src, "/sprites/16");

                // Slot rates are shown against the whole table, so the bars
                // rank the species against each other.
                let slot = cards[0].lead.as_ref().expect("slot rate");
                assert_eq!(slot.value, "45%");
                assert_eq!(slot.meter, Some(AddonMeter::new(45, 100)));

                assert_eq!(slot.hint.as_deref(), Some("catch 255"));

                // The method rides on the species, which is what lets grass and
                // surf share one list without being confused for each other.
                assert_eq!(cards[0].badges.len(), 1);
                assert_eq!(cards[0].badges[0].text, "Grass");
            }
            other => panic!("expected cards, got {other:?}"),
        }
    }

    /// Several methods on one route stay apart without being kept apart: they
    /// sort together and each species says which one it belongs to.
    #[test]
    fn every_method_on_a_route_lands_in_the_one_area_section() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.area = Some(FireRedAreaSnapshot {
            map_group: 3,
            map_num: 19,
            map_key: "3:19".to_string(),
            name: "Route 1".to_string(),
            encounter_groups: vec![
                FireRedEncounterGroup {
                    method: "Surf",
                    encounter_rate: 10,
                    entries: vec![encounter(72, "Tentacool", 5, 40, 60, 190)],
                },
                FireRedEncounterGroup {
                    method: "Grass",
                    encounter_rate: 25,
                    entries: vec![encounter(16, "Pidgey", 2, 5, 45, 255)],
                },
            ],
        });

        let sections = fire_red_sections(&snapshot);
        let area = section(&sections, "dex");
        assert_eq!(area.note.as_deref(), Some("Route 1 (3:19) - 2 species"));

        match &area.content {
            AddonSectionContent::Cards(cards) => {
                assert_eq!(cards.len(), 2);
                // Grouped by method, so a list never interleaves the two.
                assert_eq!(cards[0].badges[0].text, "Grass");
                assert_eq!(cards[1].badges[0].text, "Surf");
            }
            other => panic!("expected cards, got {other:?}"),
        }
    }

    /// An area nobody has entered a table for says so, rather than reporting an
    /// empty list that reads as a broken addon.
    #[test]
    fn an_unmapped_area_says_which_map_is_missing() {
        let mut snapshot = snapshot_of(vec![member(1, 30, 30)]);
        snapshot.area = Some(FireRedAreaSnapshot {
            map_group: 16,
            map_num: 1,
            map_key: "16:1".to_string(),
            name: "Unknown Area".to_string(),
            encounter_groups: Vec::new(),
        });

        let sections = fire_red_sections(&snapshot);
        let area = section(&sections, "dex");
        assert_eq!(
            area.note.as_deref(),
            Some("Unknown Area (16:1) - no encounters known")
        );

        match &area.content {
            AddonSectionContent::List(lines) => {
                assert!(lines[0].contains("16:1"), "{:?}", lines[0]);
            }
            other => panic!("expected a list, got {other:?}"),
        }
    }

    /// The species table is indexed by what the game stores, which is not the
    /// National Dex. Kanto and Johto line up; Hoenn does not, and a traded
    /// Treecko is species 277. Getting this wrong renames every Hoenn Pokemon
    /// in a FireRed party without any other symptom.
    #[test]
    fn species_are_keyed_by_internal_index_not_dex_number() {
        assert_eq!(species_display_name(1), "Bulbasaur");
        assert_eq!(species_display_name(251), "Celebi");

        // The gap between Johto and Hoenn is unused.
        for unused in [252, 260, 276] {
            assert!(
                fallback_species_info(unused).is_none(),
                "{unused} is one of the unused slots"
            );
        }

        // Hoenn sits 25 slots past its dex number.
        assert_eq!(species_display_name(277), "Treecko");
        assert_eq!(species_display_name(411), "Deoxys");
    }

    #[test]
    fn every_species_the_table_knows_has_a_usable_catch_rate() {
        let mut named = 0;
        for id in 1..=411u16 {
            if let Some((name, catch)) = fallback_species_info(id) {
                named += 1;
                assert!(!name.is_empty(), "species {id} has no name");
                assert!(catch > 0, "species {id} would be impossible to catch");
            }
        }
        assert_eq!(named, 386, "Generation 3 has 386 species");
    }

    /// An index outside the table still reads as something rather than
    /// crashing or claiming to be a Pokemon it is not.
    #[test]
    fn an_unknown_species_says_so() {
        assert_eq!(species_display_name(9999), "Species #9999");
    }

    #[test]
    fn a_known_move_gets_a_pp_bar_and_an_unknown_one_gets_the_count() {
        let known = move_field(&FireRedMoveSlot {
            slot: 1,
            move_id: 33, // Tackle, 35 PP
            name: "Tackle".to_string(),
            pp: 35,
        });
        assert_eq!(known.meter, Some(AddonMeter::new(35, 35)));
        assert_eq!(known.hint.as_deref(), Some("35/35 PP"));

        let unknown = move_field(&FireRedMoveSlot {
            slot: 2,
            move_id: 9999,
            name: "Unknown Move".to_string(),
            pp: 7,
        });
        assert_eq!(unknown.meter, None);
        assert_eq!(unknown.hint.as_deref(), Some("7 PP left"));
    }

    #[test]
    fn test_decode_gen3_text_basic_ascii() {
        let bytes = encode_gen3_text("AB12");
        assert_eq!(decode_gen3_text(&bytes), "AB12");
    }

    #[test]
    fn test_fire_red_addon_supports_leaf_green_revision_one() {
        let addon = PokemonFrlgAddon;
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

        let (base, party) = locate_fire_red_party(&view(&gba)).expect("party should be found");
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

        let (_, party) = locate_fire_red_party(&view(&gba)).expect("party should be found");
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

        let area = locate_fire_red_area(&view(&gba)).expect("area should be found");
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

        assert_eq!(locate_fire_red_battle(&view(&gba), None), None);
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

        assert_eq!(locate_fire_red_battle(&view(&gba), None), None);
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
        let battle = locate_fire_red_battle(&view(&gba), Some(&area)).expect("battle should be found");
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

        let battle = locate_fire_red_battle(&view(&gba), None).expect("battle should be found");
        assert_eq!(battle.battle_kind, "Trainer");
        assert!(!battle.catchable);
        assert_eq!(battle.opponent.species_name, "Squirtle");
        assert_eq!(battle.opponent.catch_rate, None);
    }

    #[test]
    fn test_locate_fire_red_trainer_battle_advances_after_first_faints() {
        let mut gba = Gba::new();
        gba.write_u32(FIRE_RED_BATTLE_TYPE_FLAGS_ADDR, BATTLE_TYPE_TRAINER);
        gba.write_u8(FIRE_RED_BATTLE_OUTCOME_ADDR, BATTLE_OUTCOME_NONE);
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE,
            &build_party_member_raw(0x1111_2222, 0x3333_4444, "RATTATA", 19, 4, 0, 13),
        );
        write_party_slot(
            &mut gba,
            FIRE_RED_ENEMY_PARTY_BASE + PARTY_SLOT_SIZE as u32,
            &build_party_member_raw(0x2222_3333, 0x4444_5555, "PIDGEY", 16, 5, 17, 17),
        );

        let battle = locate_fire_red_battle(&view(&gba), None).expect("battle should be found");
        assert_eq!(battle.battle_kind, "Trainer");
        assert_eq!(battle.opponent.nickname, "PIDGEY");
        assert_eq!(battle.opponent.species_id, 16);
        assert_eq!(battle.opponent.current_hp, 17);
    }

    /// The name tables, found in a real cartridge rather than assumed.
    ///
    /// This is the test that would have caught a wrong stride or a bad anchor:
    /// the unit tests plant their own tables, so they prove the search works on
    /// a table shaped the way this code expects. Only a real ROM proves the
    /// shape is right.
    #[test]
    #[ignore = "Needs a commercial ROM"]
    fn name_tables_are_found_in_a_real_cartridge() {
        let rom_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("roms/pokemon_fire_red.gba");
        if !rom_path.is_file() {
            eprintln!("no ROM at {}; nothing to check", rom_path.display());
            return;
        }

        let mut gba = Gba::new();
        gba.load_rom(std::fs::read(&rom_path).expect("read ROM"));
        let memory = view(&gba);
        let rom = tinybird_addons::read_rom_identity(&memory).expect("header");

        gen3_names::ensure(&memory, &rom);

        // Species, including the Hoenn block that sits past the unused slots.
        assert_eq!(gen3_names::species(1).as_deref(), Some("Bulbasaur"));
        assert_eq!(gen3_names::species(151).as_deref(), Some("Mew"));
        assert_eq!(gen3_names::species(277).as_deref(), Some("Treecko"));

        // Moves, including one well past where the old table stopped.
        assert_eq!(gen3_names::move_name(1).as_deref(), Some("Pound"));
        assert_eq!(gen3_names::move_name(33).as_deref(), Some("Tackle"));
        assert_eq!(gen3_names::move_name(354).as_deref(), Some("Psycho Boost"));

        // Items, which the compiled fallback barely covers.
        assert_eq!(gen3_names::item(1).as_deref(), Some("Master Ball"));
        assert_eq!(gen3_names::item(13).as_deref(), Some("Potion"));

        eprintln!("item 179 = {:?}", gen3_names::item(179));
    }

    #[test]
    #[ignore = "Manual smoke test for local FireRed savestates when debugging addons"]
    fn inspect_local_firered_state_snapshot() {
        // Relative to the workspace, not to this crate: `cargo test -p` runs
        // with the crate directory as the working directory, so a bare
        // "roms/..." silently resolved to a path that never exists and the
        // test passed by doing nothing.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let state_path = workspace.join("roms/PokemonFireRed.state");
        let rom_path = workspace.join("roms/pokemon_fire_red.gba");
        if !state_path.is_file() || !rom_path.is_file() {
            eprintln!(
                "need both {} and {}; nothing to inspect",
                rom_path.display(),
                state_path.display()
            );
            return;
        }

        // A savestate restores the machine around a cartridge; it does not
        // carry the cartridge. Restoring into an empty `Gba` fails, which is
        // why this test could not have worked before the path was fixed.
        let mut gba = Gba::new();
        gba.load_rom(std::fs::read(&rom_path).expect("read FireRed ROM"));
        let bytes = std::fs::read(&state_path).expect("read local FireRed savestate");
        gba.load_state_bytes(&bytes)
            .expect("deserialize local FireRed savestate");

        let snapshot = crate::capture_stream_snapshot(Some(&gba));

        // The JSON rather than the Debug: this is what the web read-out, the
        // stream overlay, and every external tool actually receive, so it is
        // the thing worth looking at when a panel comes out wrong.
        eprintln!(
            "local FireRed snapshot:\n{}",
            crate::snapshot_to_json(&snapshot).expect("serialize snapshot")
        );

        let addon = snapshot
            .addon
            .as_ref()
            .expect("expected local FireRed savestate to produce an addon snapshot");
        assert!(
            !addon.sections.is_empty(),
            "an addon that reports no sections is invisible outside the desktop app"
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

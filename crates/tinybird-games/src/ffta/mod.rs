//! Final Fantasy Tactics Advance addon.
//!
//! Reports the player's chosen name and full clan between fights, then switches
//! to the deployed party with live HP/MP once a battle formation is set.
//!
//! The addresses come from `tinybird-probe` against the US release (`AFXE`) and
//! are documented in [`units`]. Other regions (`AFXJ`, `AFXP`) are claimed by
//! this addon so `Tools > Addon Status` reports them as recognised, but their
//! RAM layout has not been verified, so no data is read for them — the
//! cartridge fallback covers those instead of showing numbers that could be
//! wrong.

pub mod text;
pub mod units;

use std::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;
use tinybird_addons::schema::{
    AddonBadge, AddonCard, AddonField, AddonImage, AddonSection, AddonTone,
};
use tinybird_addons::{AddonInfo, GameAddon, MemoryView, RomIdentity};

use crate::{AddonData, AddonSnapshot};
use units::FftaUnit;

const ADDON_VERSION: &str = "0.1.0";

/// Game code prefix shared by every regional release.
const GAME_CODE_PREFIX: &str = "AFX";
/// The only region whose RAM layout has been verified.
const VERIFIED_REGION: char = 'E';

/// A snapshot of FFTA's live state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FftaSnapshot {
    pub source: &'static str,
    /// The player's chosen name for the main character, if one is set.
    pub player_name: Option<String>,
    /// Units currently on the field.
    pub units: Vec<FftaUnit>,
    /// Opposing field units, excluding neutral Judges.
    pub enemies: Vec<FftaUnit>,
    /// Whether `units` is the deployed party rather than the whole clan.
    pub in_battle: bool,
    /// Address the unit array was read from, for diagnostics.
    pub unit_base_address: u32,
}

impl FftaSnapshot {
    /// The units the player controls.
    ///
    /// The roster holds everyone on the field — the other side and the judge
    /// as well — so anything about *your* position has to be counted over this
    /// rather than over all of it. Falls back to the whole roster when nothing
    /// is flagged, which is what the prologue looks like: a scripted fight
    /// where the game never sets the bit.
    pub fn party(&self) -> impl Iterator<Item = &FftaUnit> {
        self.units.iter()
    }

    /// How many of the player's units are below half HP.
    pub fn wounded_count(&self) -> usize {
        self.party().filter(|unit| unit.hp_fraction() < 0.5).count()
    }

    /// How many units on the field are the player's.
    pub fn party_count(&self) -> usize {
        self.party().count()
    }
}

/// How many snapshots may reuse a cached roster address before it is searched
/// for again.
///
/// The check on a cached address is cheap; the search is about a millisecond,
/// which is a tenth of the browser's frame. But a roster that is still valid is
/// not necessarily still the right one — leaving a battle can leave the old one
/// readable — so the address is refreshed a couple of times a second rather
/// than trusted until it breaks.
const ROSTER_RECHECK_FRAMES: u32 = 30;

#[derive(Default)]
pub struct FftaAddon {
    /// Where the roster was last found, or zero for nothing yet.
    ///
    /// Atomic because `snapshot` takes `&self` and the addon is shared.
    known_roster: AtomicU32,
    /// Snapshots taken since the address was last searched for.
    since_search: AtomicU32,
}

impl FftaAddon {
    /// The roster address, searched for only when the cached one will not do.
    fn roster_base(&self, memory: &dyn MemoryView) -> Option<u32> {
        let cached = self.known_roster.load(Ordering::Relaxed);
        let waited = self.since_search.fetch_add(1, Ordering::Relaxed);
        if cached != 0 && waited < ROSTER_RECHECK_FRAMES && units::is_roster_at(memory, cached) {
            return Some(cached);
        }

        let found = units::find_roster_except(memory, Some(units::CLAN_VITALS_BASE));
        self.known_roster
            .store(found.unwrap_or(0), Ordering::Relaxed);
        self.since_search.store(0, Ordering::Relaxed);
        found
    }
}

impl GameAddon<AddonData> for FftaAddon {
    fn info(&self) -> AddonInfo {
        AddonInfo {
            addon_id: "ffta_clan",
            display_name: "Final Fantasy Tactics Advance",
            version: ADDON_VERSION,
            capabilities: &["units", "player"],
            supported_games: "Final Fantasy Tactics Advance (AFXE; USA only for live data)",
        }
    }

    fn supports(&self, rom: &RomIdentity) -> bool {
        rom.code_prefix() == GAME_CODE_PREFIX || rom.title.starts_with("FFTA")
    }

    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot> {
        // Reading US addresses out of a Japanese or European build would report
        // numbers that look real and are not. Leave those to the fallback.
        if rom.region_code() != Some(VERIFIED_REGION) {
            return None;
        }

        // Found rather than remembered: the roster moves between scenes, and an
        // address that was right in one battle reads an unrelated record in the
        // next. See `units::find_roster`.
        let clan = units::read_units_at(memory, units::CLAN_VITALS_BASE);
        // The field roster is populated during team setup, before the fixed
        // clan list marks anyone as deployed. Read it in both modes so the
        // opponent preview does not have to wait for "Start Battle".
        let field_base = self.roster_base(memory);
        let field = field_base
            .map(|at| units::read_units_at(memory, at))
            .unwrap_or_default();
        // This is the formation bit, so the view changes while units are being
        // set for battle rather than waiting for the map transition.
        let in_battle = clan.iter().any(|unit| unit.player);
        let belongs_to_clan = |unit: &FftaUnit| {
            unit.name
                .as_ref()
                .is_some_and(|name| clan.iter().any(|member| member.name.as_ref() == Some(name)))
        };
        // A matching clan member proves this is a field roster for the current
        // formation rather than a coincidental run of enemy-looking records.
        let setup_enemy_roster = !in_battle
            && field_base == Some(units::SETUP_ROSTER_VITALS_BASE)
            && !clan.is_empty()
            && !field.is_empty();
        let field_is_current = setup_enemy_roster || field.iter().any(&belongs_to_clan);
        let field_enemies = field_is_current
            .then(|| {
                field
                    .iter()
                    .filter(|unit| !unit.player && !unit.is_judge() && !belongs_to_clan(unit))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let (units, enemies, base) = if in_battle {
            // Selection belongs to the fixed clan list. Current HP/MP belongs
            // to the moving field copy, once it exists. Joining by name gives
            // the dashboard both without ever admitting enemies or the judge.
            let party = clan
                .iter()
                .filter(|unit| unit.player)
                .map(|selected| {
                    field
                        .iter()
                        .find(|live| live.name == selected.name)
                        .cloned()
                        .unwrap_or_else(|| selected.clone())
                })
                .collect::<Vec<_>>();
            (
                party,
                field_enemies,
                field_base.unwrap_or(units::CLAN_VITALS_BASE),
            )
        } else if !clan.is_empty() {
            (clan, field_enemies, units::CLAN_VITALS_BASE)
        } else {
            // The scripted tutorial predates the clan list. It only has the
            // moving roster, so retain the old safe fallback for that scene.
            let any_flagged = field.iter().any(|unit| unit.player);
            let visible = field
                .into_iter()
                .filter(|unit| unit.player || !any_flagged)
                .collect();
            (visible, Vec::new(), field_base.unwrap_or(0))
        };
        let player_name =
            Some(units::read_player_name(memory)).filter(|name| text::looks_like_name(name));

        // With no units and no name there is nothing to show, and returning
        // None lets the cartridge addon describe the ROM instead.
        if units.is_empty() && player_name.is_none() {
            return None;
        }

        let snapshot = FftaSnapshot {
            source: "live_memory",
            player_name,
            units,
            enemies,
            in_battle,
            unit_base_address: base,
        };

        let info = self.info();
        Some(
            AddonSnapshot::new(
                info.addon_id,
                info.display_name,
                overlay_lines(&snapshot),
                AddonData::Ffta(snapshot.clone()),
            )
            .with_version(info.version)
            .with_capabilities(info.capabilities.to_vec())
            .with_sections(sections(&snapshot)),
        )
    }
}

/// Compact lines for the stream overlay.
fn overlay_lines(snapshot: &FftaSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(name) = &snapshot.player_name {
        lines.push(format!("Player: {name}"));
    }
    if snapshot.units.is_empty() {
        lines.push("No clan members found".to_string());
        return lines;
    }

    if snapshot.in_battle {
        lines.push(format!("Party: {}", snapshot.units.len()));
        if !snapshot.enemies.is_empty() {
            lines.push(format!("Enemies: {}", snapshot.enemies.len()));
        }
    } else {
        lines.push(format!("Clan: {}", snapshot.units.len()));
    }
    lines.extend(snapshot.units.iter().map(|unit| {
        format!(
            "{}  HP {}  MP {}",
            unit.display_name(),
            unit.hp_text(),
            unit.mp_text()
        )
    }));
    let wounded = snapshot.wounded_count();
    if wounded > 0 {
        lines.push(format!("Wounded: {wounded}"));
    }
    lines
}

/// Generic sections, so consumers that know nothing about FFTA still render it.
fn sections(snapshot: &FftaSnapshot) -> Vec<AddonSection> {
    let mut sections = Vec::new();

    let roster_note = if snapshot.in_battle {
        format!("{} deployed · live fight values", snapshot.units.len())
    } else if let Some(name) = &snapshot.player_name {
        format!("{} members · Player {name}", snapshot.units.len())
    } else {
        format!("{} members · full clan roster", snapshot.units.len())
    };
    sections.push(
        AddonSection::cards(
            "units",
            if snapshot.in_battle {
                "Fight Party"
            } else {
                "Clan Members"
            },
            unit_cards(&snapshot.units),
        )
        .with_note(roster_note),
    );

    if !snapshot.enemies.is_empty() {
        let active = snapshot.enemies.iter().filter(|unit| unit.hp > 0).count();
        let note = if snapshot.in_battle {
            format!("{active} of {} standing", snapshot.enemies.len())
        } else {
            format!("Setup preview · {active} opponents")
        };
        sections.push(
            AddonSection::cards("enemies", "Enemy Team", unit_cards(&snapshot.enemies))
                .with_note(note),
        );
    } else {
        sections.push(
            AddonSection::key_value(
                "enemies",
                "Enemy Team",
                vec![AddonField::new(
                    "Status",
                    if snapshot.in_battle {
                        "Waiting for field roster"
                    } else {
                        "Waiting for battle"
                    },
                )],
            )
            .with_note("Opponent details appear when the battle map loads"),
        );
    }

    sections
}

fn unit_cards(units: &[FftaUnit]) -> Vec<AddonCard> {
    units
        .iter()
        .map(|unit| {
            let hp = AddonField::gauge("HP", u32::from(unit.hp), u32::from(unit.max_hp));
            let identity = match (unit.race_name(), unit.job_name()) {
                (Some(race), Some(job)) => Some(format!("{race} · {job}")),
                (Some(race), None) => Some(race.to_string()),
                (None, Some(job)) => Some(job.to_string()),
                (None, None) => None,
            };
            let mut card = AddonCard::new(unit.display_name())
                .with_lead(hp)
                .with_optional_image(job_image(unit).or_else(|| race_image(unit)))
                .with_fields(vec![AddonField::gauge(
                    "MP",
                    u32::from(unit.mp),
                    u32::from(unit.max_mp),
                )]);
            if let Some(identity) = identity {
                card = card.with_subtitle(identity);
            }

            let mut badges = Vec::new();
            if unit.hp == 0 {
                badges.push(AddonBadge::toned("Down", AddonTone::Bad));
            } else if unit.hp_fraction() < 0.5 {
                badges.push(AddonBadge::toned("Wounded", AddonTone::Warn));
            }
            if !badges.is_empty() {
                card = card.with_badges(badges);
            }
            card
        })
        .collect()
}

fn job_image(unit: &FftaUnit) -> Option<AddonImage> {
    // The supplied sheet covers all 42 playable jobs in the same order as the
    // game's ids. Monsters and story-only jobs fall back to race art or text.
    (0x02..=0x2B).contains(&unit.job_id).then(|| {
        AddonImage::new(format!("/ffta/jobs/{:02x}?v=2", unit.job_id))
            .with_alt(unit.job_name().unwrap_or("Unknown class"))
    })
}

fn race_image(unit: &FftaUnit) -> Option<AddonImage> {
    let slug = match unit.race_id {
        1 => "human",
        2 => "bangaa",
        3 => "nu-mou",
        4 => "viera",
        5 => "moogle",
        _ => return None,
    };
    Some(
        // Race art is served immutable for performance. Version the URL when
        // the bundled art changes so an old browser cache cannot win forever.
        AddonImage::new(format!("/ffta/races/{slug}?v=2"))
            .with_alt(unit.race_name().unwrap_or("Unknown race")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinybird_addons::schema::{AddonMeter, AddonSectionContent};
    use tinybird_addons::SparseMemory;

    fn ffta_rom(region: &str) -> RomIdentity {
        RomIdentity {
            title: "FFTA_USVER.".to_string(),
            game_code: format!("AFX{region}"),
            maker_code: "01".to_string(),
            revision: 0,
            // Not a real dump; nothing here reads the fingerprint.
            fingerprint: 0,
        }
    }

    /// Somewhere to build a roster. The reader finds it; nothing depends on
    /// the value.
    const TEST_ROSTER_BASE: u32 = 0x0200_3000;
    /// A cartridge address to hang test names off.
    const TEST_NAMES: u32 = 0x0855_0000;

    /// Memory matching the tutorial-battle save state: three units and a name.
    fn tutorial_memory() -> SparseMemory {
        let mut memory = SparseMemory::new();
        for (slot, (hp, max_hp, mp, max_mp)) in [
            (16u16, 16u16, 10u16, 10u16),
            (10, 10, 10, 10),
            (5, 10, 10, 10),
        ]
        .iter()
        .enumerate()
        {
            let base = TEST_ROSTER_BASE + units::UNIT_STRIDE * slot as u32;
            let mut bytes = Vec::new();
            bytes.extend(hp.to_le_bytes());
            bytes.extend(max_hp.to_le_bytes());
            bytes.extend(mp.to_le_bytes());
            bytes.extend(max_mp.to_le_bytes());
            memory.write(base, bytes);
            // A roster is only found by its name pointers, so the fixture has
            // to carry them the way the game does.
            let at = TEST_NAMES + 0x40 * slot as u32;
            memory.write(base - 0x18, at.to_le_bytes().to_vec());
            let mut name = vec![0x01u8];
            name.extend(text::encode_plain(["Ritz", "Norma", "Leslaie"][slot]));
            name.push(0x00);
            memory.write(at, name);
        }
        memory.write(
            units::PLAYER_NAME_ADDR,
            vec![
                0x80, 0xBC, 0x80, 0xCA, 0x80, 0xDB, 0x80, 0xCC, 0x80, 0xD1, 0x80, 0xCE, 0x00, 0x00,
            ],
        );
        memory
    }

    fn put_unit(
        memory: &mut SparseMemory,
        base: u32,
        slot: usize,
        name_at: u32,
        name: &str,
        hp: u16,
        selected: bool,
        race: u8,
        job: u8,
    ) {
        let vitals = base + units::UNIT_STRIDE * slot as u32;
        let mut bytes = Vec::new();
        bytes.extend(hp.to_le_bytes());
        bytes.extend(40u16.to_le_bytes());
        bytes.extend(12u16.to_le_bytes());
        bytes.extend(20u16.to_le_bytes());
        memory.write(vitals, bytes);
        memory.write(vitals + 16, vec![if selected { 0x80 } else { 0 }]);
        memory.write(vitals - 0x12, vec![race, job]);
        memory.write(vitals - 0x18, name_at.to_le_bytes().to_vec());
        let mut encoded = vec![0x01];
        encoded.extend(text::encode_plain(name));
        encoded.push(0);
        memory.write(name_at, encoded);
    }

    #[test]
    fn it_claims_every_regional_release() {
        for region in ["E", "J", "P"] {
            assert!(
                FftaAddon::default().supports(&ffta_rom(region)),
                "region {region}"
            );
        }
    }

    #[test]
    fn it_does_not_claim_an_unrelated_rom() {
        let mut rom = ffta_rom("E");
        rom.game_code = "BPRE".to_string();
        rom.title = "POKEMON FIRE".to_string();
        assert!(!FftaAddon::default().supports(&rom));
    }

    #[test]
    fn it_reads_the_tutorial_battle() {
        let snapshot = FftaAddon::default()
            .snapshot(&tutorial_memory(), &ffta_rom("E"))
            .expect("should produce a snapshot");

        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert_eq!(data.player_name.as_deref(), Some("Marche"));
        assert_eq!(data.units.len(), 3);
        assert_eq!(data.units[0].hp_text(), "16/16");
    }

    #[test]
    fn unverified_regions_report_nothing_rather_than_wrong_numbers() {
        // The addresses are US-only. Reading them from a JP build would produce
        // numbers that look plausible and are meaningless.
        for region in ["J", "P"] {
            assert!(
                FftaAddon::default()
                    .snapshot(&tutorial_memory(), &ffta_rom(region))
                    .is_none(),
                "region {region} must not report US addresses"
            );
        }
    }

    #[test]
    fn an_empty_game_state_produces_no_snapshot_so_the_fallback_runs() {
        assert!(FftaAddon::default()
            .snapshot(&SparseMemory::new(), &ffta_rom("E"))
            .is_none());
    }

    #[test]
    fn a_name_with_no_units_still_produces_a_snapshot() {
        // Between battles the unit array empties but the name persists.
        let mut memory = SparseMemory::new();
        memory.write(
            units::PLAYER_NAME_ADDR,
            vec![0x80, 0xBC, 0x80, 0xCA, 0x80, 0xDB, 0x00, 0x00],
        );
        let snapshot = FftaAddon::default()
            .snapshot(&memory, &ffta_rom("E"))
            .expect("a known player name is worth reporting on its own");

        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert!(data.units.is_empty());
        assert_eq!(data.player_name.as_deref(), Some("Mar"));
    }

    #[test]
    fn the_clan_view_switches_to_the_selected_live_party() {
        let mut memory = SparseMemory::new();
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            0,
            TEST_NAMES,
            "Marche",
            40,
            true,
            1,
            2,
        );
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            1,
            TEST_NAMES + 0x40,
            "Montblanc",
            40,
            false,
            5,
            0x2A,
        );
        // The moving copy has battle damage. An enemy makes this a real field
        // run, but must never appear in the party view.
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            0,
            TEST_NAMES + 0x80,
            "Marche",
            17,
            true,
            1,
            2,
        );
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            1,
            TEST_NAMES + 0xC0,
            "Judge",
            40,
            false,
            0x17,
            0x68,
        );
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            2,
            TEST_NAMES + 0x100,
            "Goblin",
            23,
            false,
            0x06,
            0x2C,
        );

        let snapshot = FftaAddon::default()
            .snapshot(&memory, &ffta_rom("E"))
            .expect("party snapshot");
        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert!(data.in_battle);
        assert_eq!(data.units.len(), 1);
        assert_eq!(data.units[0].display_name(), "Marche");
        assert_eq!(data.units[0].hp, 17, "fight HP should beat the clan copy");
        assert_eq!(data.units[0].race_name(), Some("Human"));
        assert_eq!(data.units[0].job_name(), Some("Soldier"));
        assert_eq!(data.enemies.len(), 1);
        assert_eq!(data.enemies[0].display_name(), "Goblin");
        assert_eq!(snapshot.sections[0].title, "Fight Party");
        assert_eq!(snapshot.sections[1].title, "Enemy Team");
    }

    #[test]
    fn no_formation_flags_means_the_whole_clan() {
        let mut memory = SparseMemory::new();
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            0,
            TEST_NAMES,
            "Marche",
            40,
            false,
            1,
            2,
        );
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            1,
            TEST_NAMES + 0x40,
            "Montblanc",
            40,
            false,
            5,
            0x2A,
        );

        let snapshot = FftaAddon::default()
            .snapshot(&memory, &ffta_rom("E"))
            .expect("clan snapshot");
        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert!(!data.in_battle);
        assert_eq!(data.units.len(), 2);
        assert_eq!(snapshot.sections[0].title, "Clan Members");
        assert_eq!(snapshot.sections[1].title, "Enemy Team");
    }

    #[test]
    fn team_setup_previews_enemies_before_deployment_is_committed() {
        let mut memory = SparseMemory::new();
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            0,
            TEST_NAMES,
            "Marche",
            40,
            false,
            1,
            2,
        );
        put_unit(
            &mut memory,
            units::CLAN_VITALS_BASE,
            1,
            TEST_NAMES + 0x40,
            "Montblanc",
            40,
            false,
            5,
            0x2A,
        );
        // The moving roster is already prepared while the fixed clan list has
        // not yet committed its deployment flags.
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            0,
            TEST_NAMES + 0x80,
            "Marche",
            40,
            true,
            1,
            2,
        );
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            1,
            TEST_NAMES + 0xC0,
            "Judge",
            10,
            false,
            0x17,
            0x68,
        );
        put_unit(
            &mut memory,
            TEST_ROSTER_BASE,
            2,
            TEST_NAMES + 0x100,
            "Goblin",
            23,
            false,
            0x06,
            0x2C,
        );

        let snapshot = FftaAddon::default()
            .snapshot(&memory, &ffta_rom("E"))
            .expect("setup snapshot");
        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert!(
            !data.in_battle,
            "setup still shows the full clan on the left"
        );
        assert_eq!(data.units.len(), 2);
        assert_eq!(data.enemies.len(), 1);
        assert_eq!(data.enemies[0].display_name(), "Goblin");
        assert_eq!(
            snapshot.sections[1].note.as_deref(),
            Some("Setup preview · 1 opponents")
        );
    }

    #[test]
    fn wounded_units_are_counted_at_below_half_hp() {
        let snapshot = FftaSnapshot {
            source: "test",
            player_name: None,
            in_battle: true,
            unit_base_address: 0,
            enemies: Vec::new(),
            units: vec![
                FftaUnit {
                    slot: 0,
                    name: None,
                    player: true,
                    race_id: 1,
                    job_id: 1,
                    hp: 16,
                    max_hp: 16,
                    mp: 0,
                    max_mp: 0,
                },
                FftaUnit {
                    slot: 1,
                    name: None,
                    player: true,
                    race_id: 1,
                    job_id: 1,
                    hp: 8,
                    max_hp: 16,
                    mp: 0,
                    max_mp: 0,
                },
                FftaUnit {
                    slot: 2,
                    name: None,
                    player: true,
                    race_id: 1,
                    job_id: 1,
                    hp: 7,
                    max_hp: 16,
                    mp: 0,
                    max_mp: 0,
                },
            ],
        };
        assert_eq!(snapshot.wounded_count(), 1, "exactly half is not wounded");
    }

    #[test]
    fn generic_sections_describe_the_state_for_consumers_that_know_nothing_about_ffta() {
        let snapshot = FftaAddon::default()
            .snapshot(&tutorial_memory(), &ffta_rom("E"))
            .expect("snapshot");

        let ids: Vec<_> = snapshot
            .sections
            .iter()
            .map(|section| section.section_id)
            .collect();
        assert_eq!(ids, vec!["units", "enemies"]);

        let units = snapshot
            .sections
            .iter()
            .find(|section| section.section_id == "units")
            .expect("units section");
        match &units.content {
            AddonSectionContent::Cards(cards) => {
                assert_eq!(cards.len(), 3);
                let first = &cards[0];
                // Named from the cartridge, not numbered by slot.
                assert_eq!(first.title, "Ritz");

                // HP is the headline, and it carries the bar as well as the
                // text so a graphical consumer never has to parse "16/16".
                let hp = first.lead.as_ref().expect("HP lead");
                assert_eq!(hp.value, "16/16");
                assert_eq!(hp.meter, Some(AddonMeter::new(16, 16)));
                assert_eq!(hp.tone, AddonTone::Good);

                assert_eq!(first.fields[0].value, "10/10");
                // A unit at full health has nothing to flag.
                assert!(first.badges.is_empty());
            }
            other => panic!("expected cards, got {other:?}"),
        }
    }

    /// The reason units became cards: a hurt unit has to be visible as hurt
    /// without the consumer knowing what an FFTA unit is.
    #[test]
    fn a_wounded_unit_is_flagged_rather_than_left_to_the_reader() {
        let unit = FftaUnit {
            slot: 0,
            name: None,
            player: true,
            race_id: 1,
            job_id: 1,
            hp: 3,
            max_hp: 20,
            mp: 0,
            max_mp: 10,
        };
        let snapshot = FftaSnapshot {
            source: "live_memory",
            player_name: None,
            units: vec![unit],
            enemies: Vec::new(),
            in_battle: true,
            unit_base_address: 0,
        };

        let sections = sections(&snapshot);
        let units = sections
            .iter()
            .find(|section| section.section_id == "units")
            .expect("units section");

        match &units.content {
            AddonSectionContent::Cards(cards) => {
                assert_eq!(cards[0].badges[0].text, "Wounded");
                assert_eq!(cards[0].badges[0].tone, AddonTone::Warn);
                assert_eq!(cards[0].lead.as_ref().unwrap().tone, AddonTone::Warn);
            }
            other => panic!("expected cards, got {other:?}"),
        }
    }

    #[test]
    fn overlay_lines_lead_with_the_player_and_unit_count() {
        let snapshot = FftaAddon::default()
            .snapshot(&tutorial_memory(), &ffta_rom("E"))
            .expect("snapshot");

        assert_eq!(snapshot.overlay_lines[0], "Player: Marche");
        assert_eq!(snapshot.overlay_lines[1], "Clan: 3");
        assert!(snapshot
            .overlay_lines
            .iter()
            .any(|line| line.contains("16/16")));
    }
}

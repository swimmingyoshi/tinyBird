//! Final Fantasy Tactics Advance addon.
//!
//! Reports the player's chosen name and the live HP/MP of the units currently
//! on the field, as typed data for the dashboard and as generic sections for
//! the web overlay and any other consumer.
//!
//! The addresses come from `tinybird-probe` against the US release (`AFXE`) and
//! are documented in [`units`]. Other regions (`AFXJ`, `AFXP`) are claimed by
//! this addon so `Tools > Addon Status` reports them as recognised, but their
//! RAM layout has not been verified, so no data is read for them — the
//! cartridge fallback covers those instead of showing numbers that could be
//! wrong.

pub mod text;
pub mod units;

use serde::Serialize;
use tinybird_addons::schema::{AddonBadge, AddonCard, AddonField, AddonSection, AddonTone};
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
    /// Address the unit array was read from, for diagnostics.
    pub unit_base_address: u32,
}

impl FftaSnapshot {
    /// How many units are below half HP, for an at-a-glance overlay line.
    pub fn wounded_count(&self) -> usize {
        self.units
            .iter()
            .filter(|unit| unit.hp_fraction() < 0.5)
            .count()
    }
}

pub struct FftaAddon;

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

        let units = units::read_units(memory);
        let player_name = Some(units::read_player_name(memory))
            .filter(|name| text::looks_like_name(name));

        // With no units and no name there is nothing to show, and returning
        // None lets the cartridge addon describe the ROM instead.
        if units.is_empty() && player_name.is_none() {
            return None;
        }

        let snapshot = FftaSnapshot {
            source: "live_memory",
            player_name,
            units,
            unit_base_address: units::UNIT_VITALS_BASE,
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
        lines.push("No units on the field".to_string());
        return lines;
    }

    lines.push(format!("Units: {}", snapshot.units.len()));
    lines.extend(
        snapshot
            .units
            .iter()
            .map(|unit| format!("Unit {}  HP {}  MP {}", unit.slot + 1, unit.hp_text(), unit.mp_text())),
    );
    let wounded = snapshot.wounded_count();
    if wounded > 0 {
        lines.push(format!("Wounded: {wounded}"));
    }
    lines
}

/// Generic sections, so consumers that know nothing about FFTA still render it.
fn sections(snapshot: &FftaSnapshot) -> Vec<AddonSection> {
    let mut sections = Vec::new();

    let mut summary = Vec::new();
    if let Some(name) = &snapshot.player_name {
        summary.push(AddonField::new("Player", name.clone()));
    }
    summary.push(AddonField::new("Units", snapshot.units.len().to_string()));
    if !snapshot.units.is_empty() {
        // Wounded is the number that decides whether to press on or heal, so
        // it is toned: none is good news, any at all is worth seeing.
        let wounded = snapshot.wounded_count();
        summary.push(
            AddonField::new("Wounded", wounded.to_string())
                .with_tone(if wounded == 0 {
                    AddonTone::Good
                } else {
                    AddonTone::Warn
                })
                .with_hint("below half HP"),
        );

        let hp: u32 = snapshot.units.iter().map(|unit| u32::from(unit.hp)).sum();
        let max_hp: u32 = snapshot.units.iter().map(|unit| u32::from(unit.max_hp)).sum();
        summary.push(AddonField::gauge("Clan HP", hp, max_hp));
    }

    sections.push(
        AddonSection::key_value("summary", "Clan", summary)
            .with_note("Live unit array, USA build"),
    );

    if !snapshot.units.is_empty() {
        // One card per unit rather than one table row: HP and MP are both
        // bars, and a row of "12/16" strings hides which unit is in trouble.
        let cards = snapshot
            .units
            .iter()
            .map(|unit| {
                let hp = AddonField::gauge("HP", u32::from(unit.hp), u32::from(unit.max_hp));
                let mut card = AddonCard::new(format!("Unit {}", unit.slot + 1))
                    .with_lead(hp)
                    .with_fields(vec![AddonField::gauge(
                        "MP",
                        u32::from(unit.mp),
                        u32::from(unit.max_mp),
                    )]);
                if unit.hp == 0 {
                    card = card.with_badges(vec![AddonBadge::toned("Down", AddonTone::Bad)]);
                } else if unit.hp_fraction() < 0.5 {
                    card = card.with_badges(vec![AddonBadge::toned("Wounded", AddonTone::Warn)]);
                }
                card
            })
            .collect();

        sections.push(AddonSection::cards("units", "Units", cards));
    }

    sections
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
        }
    }

    /// Memory matching the tutorial-battle save state: three units and a name.
    fn tutorial_memory() -> SparseMemory {
        let mut memory = SparseMemory::new();
        for (slot, (hp, max_hp, mp, max_mp)) in
            [(16u16, 16u16, 10u16, 10u16), (10, 10, 10, 10), (5, 10, 10, 10)]
                .iter()
                .enumerate()
        {
            let base = units::UNIT_VITALS_BASE + units::UNIT_STRIDE * slot as u32;
            let mut bytes = Vec::new();
            bytes.extend(hp.to_le_bytes());
            bytes.extend(max_hp.to_le_bytes());
            bytes.extend(mp.to_le_bytes());
            bytes.extend(max_mp.to_le_bytes());
            memory.write(base, bytes);
        }
        memory.write(
            units::PLAYER_NAME_ADDR,
            vec![
                0x80, 0xBC, 0x80, 0xCA, 0x80, 0xDB, 0x80, 0xCC, 0x80, 0xD1, 0x80, 0xCE, 0x00, 0x00,
            ],
        );
        memory
    }

    #[test]
    fn it_claims_every_regional_release() {
        for region in ["E", "J", "P"] {
            assert!(FftaAddon.supports(&ffta_rom(region)), "region {region}");
        }
    }

    #[test]
    fn it_does_not_claim_an_unrelated_rom() {
        let mut rom = ffta_rom("E");
        rom.game_code = "BPRE".to_string();
        rom.title = "POKEMON FIRE".to_string();
        assert!(!FftaAddon.supports(&rom));
    }

    #[test]
    fn it_reads_the_tutorial_battle() {
        let snapshot = FftaAddon
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
                FftaAddon.snapshot(&tutorial_memory(), &ffta_rom(region)).is_none(),
                "region {region} must not report US addresses"
            );
        }
    }

    #[test]
    fn an_empty_game_state_produces_no_snapshot_so_the_fallback_runs() {
        assert!(FftaAddon
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
        let snapshot = FftaAddon
            .snapshot(&memory, &ffta_rom("E"))
            .expect("a known player name is worth reporting on its own");

        let AddonData::Ffta(data) = &snapshot.data else {
            panic!("expected FFTA payload");
        };
        assert!(data.units.is_empty());
        assert_eq!(data.player_name.as_deref(), Some("Mar"));
    }

    #[test]
    fn wounded_units_are_counted_at_below_half_hp() {
        let snapshot = FftaSnapshot {
            source: "test",
            player_name: None,
            unit_base_address: 0,
            units: vec![
                FftaUnit { slot: 0, hp: 16, max_hp: 16, mp: 0, max_mp: 0 },
                FftaUnit { slot: 1, hp: 8, max_hp: 16, mp: 0, max_mp: 0 },
                FftaUnit { slot: 2, hp: 7, max_hp: 16, mp: 0, max_mp: 0 },
            ],
        };
        assert_eq!(snapshot.wounded_count(), 1, "exactly half is not wounded");
    }

    #[test]
    fn generic_sections_describe_the_state_for_consumers_that_know_nothing_about_ffta() {
        let snapshot = FftaAddon
            .snapshot(&tutorial_memory(), &ffta_rom("E"))
            .expect("snapshot");

        let ids: Vec<_> = snapshot
            .sections
            .iter()
            .map(|section| section.section_id)
            .collect();
        assert_eq!(ids, vec!["summary", "units"]);

        let units = snapshot
            .sections
            .iter()
            .find(|section| section.section_id == "units")
            .expect("units section");
        match &units.content {
            AddonSectionContent::Cards(cards) => {
                assert_eq!(cards.len(), 3);
                let first = &cards[0];
                assert_eq!(first.title, "Unit 1");

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
            hp: 3,
            max_hp: 20,
            mp: 0,
            max_mp: 10,
        };
        let snapshot = FftaSnapshot {
            source: "live_memory",
            player_name: None,
            units: vec![unit],
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
        let snapshot = FftaAddon
            .snapshot(&tutorial_memory(), &ffta_rom("E"))
            .expect("snapshot");

        assert_eq!(snapshot.overlay_lines[0], "Player: Marche");
        assert_eq!(snapshot.overlay_lines[1], "Units: 3");
        assert!(snapshot.overlay_lines.iter().any(|line| line.contains("16/16")));
    }
}

//! Game addons: which one claims the running ROM, and what it reports.
//!
//! This crate is deliberately free of any frontend or filesystem dependency so
//! that the desktop app and the WebAssembly build report identical game data.
//! Adding a game means writing one module here and adding one line to
//! [`build_registry`].
//!
//! Two things make this work for games nobody has written a rich renderer for:
//!
//! - every addon also describes itself through the schema's generic
//!   `sections`, which the dashboard renders when it has no typed renderer for
//!   the payload; and
//! - [`cartridge::CartridgeAddon`] is registered last and claims *every* ROM,
//!   so an unrecognised game shows its header, detected save type, and live
//!   RAM signature instead of an empty panel.

pub mod cartridge;
pub mod ffta;
pub mod pokemon_frlg;

use serde::Serialize;
use tinybird_addons::{
    read_rom_identity, AddonInfo, AddonRegistry, AddonSnapshot as ContractAddonSnapshot, MemoryView,
    StreamSnapshot as ContractStreamSnapshot, SNAPSHOT_SCHEMA_VERSION,
};
use tinybird_core::Gba;

pub use ffta::FftaSnapshot;
pub use pokemon_frlg::{
    FireRedAreaSnapshot, FireRedBattleSnapshot, FireRedEncounterEntry, FireRedEncounterGroup,
    FireRedMoveSlot, FireRedPartyMember, FireRedSnapshot, FireRedStatSpread,
};

pub type StreamSnapshot = ContractStreamSnapshot<AddonData>;
pub type AddonSnapshot = ContractAddonSnapshot<AddonData>;

/// Typed payloads the desktop dashboard knows how to render richly.
///
/// [`AddonData::Generic`] is not a fallback for failure — it is the normal case
/// for an addon that describes itself entirely through schema sections, which
/// the dashboard renders with its generic table/list/key-value renderer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AddonData {
    FireRed(FireRedSnapshot),
    Ffta(FftaSnapshot),
    Generic,
}

/// Adapts the emulator to the narrow read-only interface addons are given.
///
/// The newtype exists for the orphan rule — `MemoryView` and `Gba` are both
/// foreign to this crate — but it also documents the boundary: an addon gets
/// memory reads and nothing else, so it can never stall or perturb emulation.
pub struct GbaMemory<'a>(pub &'a Gba);

impl MemoryView for GbaMemory<'_> {
    fn read_u8(&self, addr: u32) -> u8 {
        self.0.read_u8(addr)
    }

    fn read_u16(&self, addr: u32) -> u16 {
        self.0.read_u16(addr)
    }

    fn read_u32(&self, addr: u32) -> u32 {
        self.0.read_u32(addr)
    }
}

/// Every addon the desktop frontend ships with.
///
/// Order matters: the first addon that claims the ROM *and* produces data wins,
/// so specific addons come first and the catch-all cartridge addon comes last.
pub fn build_registry() -> AddonRegistry<AddonData> {
    AddonRegistry::new()
        .with(Box::new(pokemon_frlg::PokemonFrlgAddon))
        .with(Box::new(ffta::FftaAddon))
        .with(Box::new(cartridge::CartridgeAddon))
}

/// A detection result plus the metadata needed to explain it in the UI.
pub struct AddonStatus {
    pub detection_line: String,
    pub matching: Vec<AddonInfo>,
    pub registered: Vec<AddonInfo>,
}

/// Read the current game state through the registry.
pub fn capture_stream_snapshot(gba: Option<&Gba>) -> StreamSnapshot {
    let Some(gba) = gba else {
        return StreamSnapshot::default();
    };
    let memory = GbaMemory(gba);

    let Some(rom) = read_rom_identity(&memory) else {
        return StreamSnapshot::default();
    };

    let detection = build_registry().detect(&memory, &rom);
    StreamSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        rom: Some(rom),
        addon: detection.snapshot(),
    }
}

/// Explain what the registry did with the current ROM, for `Tools > Addon Status`.
pub fn describe_addon_status(gba: Option<&Gba>) -> AddonStatus {
    let registry = build_registry();
    let registered = registry.infos();

    let Some(gba) = gba else {
        return AddonStatus {
            detection_line: "No ROM loaded".to_string(),
            matching: Vec::new(),
            registered,
        };
    };
    let memory = GbaMemory(gba);

    let Some(rom) = read_rom_identity(&memory) else {
        return AddonStatus {
            detection_line: "ROM header is unreadable".to_string(),
            matching: Vec::new(),
            registered,
        };
    };

    let detection = registry.detect(&memory, &rom);
    AddonStatus {
        detection_line: detection.status_line(),
        matching: registry.matching(&rom),
        registered,
    }
}

/// Serialize a snapshot to the JSON every consumer of the export expects.
pub fn snapshot_to_json(snapshot: &StreamSnapshot) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(snapshot)
}

/// Display label for the currently detected game, used by panel headers.
pub fn game_display_label(snapshot: &StreamSnapshot) -> String {
    if let Some(addon) = &snapshot.addon {
        return addon.display_name.to_string();
    }
    match &snapshot.rom {
        Some(rom) if !rom.title.is_empty() => rom.title.clone(),
        Some(rom) if !rom.game_code.is_empty() => rom.game_code.clone(),
        _ => "No game loaded".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinybird_addons::{Detection, RomIdentity, SparseMemory};

    /// Run detection the way `capture_stream_snapshot` does.
    fn detect_for(memory: &dyn MemoryView, rom: &RomIdentity) -> Detection<AddonData> {
        build_registry().detect(memory, rom)
    }

    #[test]
    fn the_registry_ships_every_addon_module() {
        let ids: Vec<_> = build_registry()
            .infos()
            .iter()
            .map(|info| info.addon_id)
            .collect();
        assert!(ids.contains(&"pokemon_frlg_party"));
        assert!(ids.contains(&"ffta_clan"));
        assert!(ids.contains(&"cartridge"));
    }

    #[test]
    fn the_cartridge_addon_is_registered_last_so_it_never_shadows_a_real_one() {
        let infos = build_registry().infos();
        assert_eq!(
            infos.last().map(|info| info.addon_id),
            Some("cartridge"),
            "a catch-all registered early would hide every game-specific addon"
        );
    }

    #[test]
    fn an_unknown_rom_still_gets_an_addon() {
        // The whole point of the cartridge fallback: no game shows an empty panel.
        let memory = SparseMemory::new().with_rom_header("SOME GAME", "ZZZE", "01", 0);
        let rom = read_rom_identity(&memory).expect("header");
        let detection = detect_for(&memory, &rom);

        match detection {
            Detection::Active { info, snapshot } => {
                assert_eq!(info.addon_id, "cartridge");
                assert!(
                    !snapshot.sections.is_empty(),
                    "the fallback must describe the cartridge through sections"
                );
            }
            other => panic!("expected the cartridge addon to claim it, got {other:?}"),
        }
    }

    #[test]
    fn a_known_rom_prefers_its_specific_addon() {
        let memory = SparseMemory::new().with_rom_header("FFTA_USVER.", "AFXE", "01", 0);
        let rom = read_rom_identity(&memory).expect("header");

        let matching: Vec<_> = build_registry()
            .matching(&rom)
            .iter()
            .map(|info| info.addon_id)
            .collect();
        assert_eq!(
            matching,
            vec!["ffta_clan", "cartridge"],
            "FFTA should be tried before the catch-all"
        );
    }

    #[test]
    fn a_snapshot_with_no_rom_serializes_to_the_schema_version_alone() {
        let snapshot = capture_stream_snapshot(None);
        let json = serde_json::to_value(&snapshot).expect("serialize");
        assert_eq!(json["schema_version"], SNAPSHOT_SCHEMA_VERSION);
        assert!(json.get("rom").is_none());
    }

    #[test]
    fn game_display_label_falls_back_through_addon_then_title_then_code() {
        let mut snapshot = StreamSnapshot::default();
        assert_eq!(game_display_label(&snapshot), "No game loaded");

        snapshot.rom = Some(RomIdentity {
            title: String::new(),
            game_code: "AFXE".to_string(),
            maker_code: "01".to_string(),
            revision: 0,
        });
        assert_eq!(game_display_label(&snapshot), "AFXE");

        snapshot.rom.as_mut().unwrap().title = "FFTA_USVER.".to_string();
        assert_eq!(game_display_label(&snapshot), "FFTA_USVER.");
    }

    #[test]
    fn addon_status_explains_an_absent_rom_rather_than_reporting_nothing() {
        let status = describe_addon_status(None);
        assert_eq!(status.detection_line, "No ROM loaded");
        assert!(
            !status.registered.is_empty(),
            "the status view should still list what is available"
        );
    }
}

//! The catch-all addon: claims every ROM, reports what the cartridge header
//! says about it.
//!
//! Registered last, so it only ever runs when no game-specific addon produced
//! data. Its job is to make sure bringing up a new game never shows an empty
//! panel — before this existed, loading anything other than FireRed rendered
//! "No addon data yet" and told you nothing about why.
//!
//! Everything reported here is derived from the 192-byte GBA cartridge header,
//! which every commercial game fills in identically, so it needs no per-game
//! knowledge at all.

use tinybird_addons::schema::{AddonField, AddonSection, AddonTone};
use tinybird_addons::{AddonInfo, GameAddon, MemoryView, RomIdentity, ROM_BASE};

use crate::{AddonData, AddonSnapshot};

const ADDON_VERSION: &str = "0.1.0";

/// Offset of the Nintendo logo block that the BIOS checks at boot.
const HEADER_LOGO: u32 = 0x04;
/// First bytes of the compressed Nintendo logo, identical on every retail cart.
const LOGO_SIGNATURE: [u8; 4] = [0x24, 0xFF, 0xAE, 0x51];
/// Offset of the header checksum byte.
const HEADER_CHECKSUM: u32 = 0xBD;
/// Range the header checksum is computed over.
const CHECKSUM_START: u32 = 0xA0;
const CHECKSUM_END: u32 = 0xBD;

pub struct CartridgeAddon;

impl GameAddon<AddonData> for CartridgeAddon {
    fn info(&self) -> AddonInfo {
        AddonInfo {
            addon_id: "cartridge",
            display_name: "Cartridge",
            version: ADDON_VERSION,
            capabilities: &["rom"],
            supported_games: "any ROM (fallback)",
        }
    }

    fn supports(&self, _rom: &RomIdentity) -> bool {
        true
    }

    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot> {
        let title = if rom.title.is_empty() {
            "(untitled)".to_string()
        } else {
            rom.title.clone()
        };

        let mut fields = vec![
            field("Title", &title),
            field("Game Code", &rom.game_code),
            field("Region", rom.region_name()),
            field("Maker", maker_label(&rom.maker_code)),
            field("Revision", &format!("{}", rom.revision)),
        ];

        // Two integrity checks the real hardware also makes. Toning them means
        // a bad dump reads as a problem at a glance rather than as one more
        // grey row among the header facts.
        let checksum = read_header_checksum(memory);
        let expected = compute_header_checksum(memory);
        fields.push(if checksum == expected {
            field("Header", "valid").with_tone(AddonTone::Good)
        } else {
            field("Header", "checksum mismatch")
                .with_tone(AddonTone::Bad)
                .with_hint(format!("read {checksum:#04x}, computed {expected:#04x}"))
        });
        fields.push(if has_nintendo_logo(memory) {
            field("Boot Logo", "present").with_tone(AddonTone::Good)
        } else {
            field("Boot Logo", "missing")
                .with_tone(AddonTone::Warn)
                .with_hint("real hardware refuses to boot this")
        });

        let overlay_lines = vec![
            format!("{title} [{}]", rom.game_code),
            format!("{} - rev {}", rom.region_name(), rom.revision),
        ];

        let sections = vec![
            AddonSection::key_value("cartridge", "Cartridge", fields)
                .with_note("Read from the 192-byte ROM header"),
            // This addon cannot tell "no addon exists" from "the addon has
            // nothing yet", so the wording covers both and points at the view
            // that does know.
            AddonSection::list(
                "addon_status",
                "Addon",
                vec![
                    "No game data available yet.".to_string(),
                    "See Tools > Addon Status.".to_string(),
                ],
            )
            .with_note("No game-specific addon claimed this ROM"),
        ];

        let info = self.info();
        Some(
            AddonSnapshot::new(
                info.addon_id,
                info.display_name,
                overlay_lines,
                AddonData::Generic,
            )
            .with_version(info.version)
            .with_capabilities(info.capabilities.to_vec())
            .with_sections(sections),
        )
    }
}

fn field(label: &str, value: &str) -> AddonField {
    AddonField::new(label, value)
}

/// Expand the two-character maker code for the handful worth naming.
fn maker_label(code: &str) -> &str {
    match code {
        "01" => "01 (Nintendo)",
        "08" => "08 (Capcom)",
        "13" => "13 (Electronic Arts)",
        "18" => "18 (Hudson Soft)",
        "41" => "41 (Ubisoft)",
        "51" => "51 (Acclaim)",
        "52" => "52 (Activision)",
        "5D" => "5D (Midway)",
        "69" => "69 (Electronic Arts)",
        "6S" => "6S (TDK)",
        "70" => "70 (Infogrames)",
        "78" => "78 (THQ)",
        "7D" => "7D (Vivendi)",
        "8P" => "8P (Sega)",
        "A4" => "A4 (Konami)",
        "AF" => "AF (Namco)",
        "B2" => "B2 (Bandai)",
        "C0" => "C0 (Taito)",
        "EB" => "EB (Atlus)",
        "" => "(none)",
        other => other,
    }
}

fn read_header_checksum(memory: &dyn MemoryView) -> u8 {
    memory.read_u8(ROM_BASE + HEADER_CHECKSUM)
}

/// The header checksum the GBA BIOS verifies: `-(0x19 + sum(0xA0..0xBD))`.
fn compute_header_checksum(memory: &dyn MemoryView) -> u8 {
    let mut sum: u8 = 0;
    for offset in CHECKSUM_START..CHECKSUM_END {
        sum = sum.wrapping_sub(memory.read_u8(ROM_BASE + offset));
    }
    sum.wrapping_sub(0x19)
}

/// Whether the header carries the Nintendo boot logo every retail cart needs.
///
/// Checking the first four bytes is enough to tell a real dump from a homebrew
/// or corrupt file without reading all 156 bytes on every refresh.
fn has_nintendo_logo(memory: &dyn MemoryView) -> bool {
    let mut logo = [0u8; 4];
    memory.read_into(ROM_BASE + HEADER_LOGO, &mut logo);
    logo == LOGO_SIGNATURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinybird_addons::schema::AddonSectionContent;
    use tinybird_addons::{read_rom_identity, SparseMemory};

    fn rom(title: &str, code: &str) -> RomIdentity {
        RomIdentity {
            title: title.to_string(),
            game_code: code.to_string(),
            maker_code: "01".to_string(),
            revision: 0,
        }
    }

    fn sections_of(snapshot: &AddonSnapshot, id: &str) -> AddonSectionContent {
        snapshot
            .sections
            .iter()
            .find(|section| section.section_id == id)
            .map(|section| section.content.clone())
            .expect("section should be present")
    }

    fn value_of(content: &AddonSectionContent, label: &str) -> String {
        match content {
            AddonSectionContent::KeyValue(fields) => fields
                .iter()
                .find(|f| f.label == label)
                .map(|f| f.value.clone())
                .unwrap_or_else(|| panic!("no field named {label}")),
            other => panic!("expected key/value content, got {other:?}"),
        }
    }

    #[test]
    fn it_claims_every_rom() {
        assert!(CartridgeAddon.supports(&rom("ANYTHING", "ZZZZ")));
        assert!(CartridgeAddon.supports(&rom("", "")));
    }

    #[test]
    fn it_always_produces_a_snapshot_so_no_game_shows_an_empty_panel() {
        let memory = SparseMemory::new().with_rom_header("FFTA_USVER.", "AFXE", "01", 0);
        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon
            .snapshot(&memory, &identity)
            .expect("the fallback must never return None");

        assert_eq!(snapshot.sections.len(), 2);
        assert!(!snapshot.overlay_lines.is_empty());
    }

    #[test]
    fn it_reports_the_header_fields() {
        let memory = SparseMemory::new().with_rom_header("FFTA_USVER.", "AFXE", "01", 2);
        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");
        let cartridge = sections_of(&snapshot, "cartridge");

        assert_eq!(value_of(&cartridge, "Title"), "FFTA_USVER.");
        assert_eq!(value_of(&cartridge, "Game Code"), "AFXE");
        assert_eq!(value_of(&cartridge, "Region"), "USA");
        assert_eq!(value_of(&cartridge, "Maker"), "01 (Nintendo)");
        assert_eq!(value_of(&cartridge, "Revision"), "2");
    }

    #[test]
    fn an_untitled_rom_is_labelled_rather_than_left_blank() {
        let memory = SparseMemory::new().with_rom_header("", "ZZZE", "", 0);
        let identity = read_rom_identity(&memory).expect("game code alone is enough");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");

        assert_eq!(value_of(&sections_of(&snapshot, "cartridge"), "Title"), "(untitled)");
        assert_eq!(value_of(&sections_of(&snapshot, "cartridge"), "Maker"), "(none)");
    }

    #[test]
    fn a_missing_boot_logo_is_reported() {
        let memory = SparseMemory::new().with_rom_header("HOMEBREW", "ZZZE", "01", 0);
        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");

        assert_eq!(
            value_of(&sections_of(&snapshot, "cartridge"), "Boot Logo"),
            "missing"
        );
    }

    #[test]
    fn a_present_boot_logo_is_recognised() {
        let memory = SparseMemory::new()
            .with_rom_header("REAL GAME", "ZZZE", "01", 0)
            .with(ROM_BASE + HEADER_LOGO, LOGO_SIGNATURE.to_vec());
        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");

        assert_eq!(
            value_of(&sections_of(&snapshot, "cartridge"), "Boot Logo"),
            "present"
        );
    }

    #[test]
    fn the_header_checksum_matches_the_bios_formula() {
        // Build a header, compute what the checksum byte should be, store it,
        // and confirm the addon then reports the header as valid.
        let mut memory = SparseMemory::new().with_rom_header("CHECKSUM", "ZZZE", "01", 0);
        let expected = compute_header_checksum(&memory);
        memory.write(ROM_BASE + HEADER_CHECKSUM, vec![expected]);

        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");
        assert_eq!(
            value_of(&sections_of(&snapshot, "cartridge"), "Header"),
            "valid"
        );
    }

    #[test]
    fn a_wrong_header_checksum_is_reported_as_a_mismatch() {
        let mut memory = SparseMemory::new().with_rom_header("CHECKSUM", "ZZZE", "01", 0);
        let wrong = compute_header_checksum(&memory).wrapping_add(1);
        memory.write(ROM_BASE + HEADER_CHECKSUM, vec![wrong]);

        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");
        assert_eq!(
            value_of(&sections_of(&snapshot, "cartridge"), "Header"),
            "checksum mismatch"
        );
    }

    #[test]
    fn the_payload_is_generic_so_the_dashboard_uses_the_section_renderer() {
        let memory = SparseMemory::new().with_rom_header("ANY", "ZZZE", "01", 0);
        let identity = read_rom_identity(&memory).expect("header");
        let snapshot = CartridgeAddon.snapshot(&memory, &identity).expect("snapshot");
        assert_eq!(snapshot.data, AddonData::Generic);
    }
}

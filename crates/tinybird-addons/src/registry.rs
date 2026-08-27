//! The addon extension point: identify a ROM, find the addon that claims it,
//! and ask that addon for a snapshot.
//!
//! Before this module the "registry" was a hardcoded array of length one inside
//! a 1700-line FireRed file:
//!
//! ```ignore
//! let addons: [&dyn GameAddon; 1] = [&fire_red];
//! ```
//!
//! and the trait was private, so there was no way to add a game without editing
//! that file. Here [`GameAddon`] is public, addons are registered into an
//! [`AddonRegistry`], and detection reports *why* it produced nothing — which is
//! the question you actually have when a new ROM shows an empty panel.

use crate::memory::{read_ascii, MemoryView, ROM_BASE};
use crate::schema::AddonSnapshot;

/// Offsets of the fields we read out of the GBA cartridge header.
const HEADER_TITLE: u32 = 0xA0;
const HEADER_TITLE_LEN: usize = 12;
const HEADER_GAME_CODE: u32 = 0xAC;
const HEADER_GAME_CODE_LEN: usize = 4;
const HEADER_MAKER_CODE: u32 = 0xB0;
const HEADER_MAKER_CODE_LEN: usize = 2;
const HEADER_REVISION: u32 = 0xBC;

/// Identifying fields from a GBA cartridge header.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RomIdentity {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub revision: u8,
}

impl RomIdentity {
    /// The first three characters of the game code, which identify the game
    /// independently of its release region.
    ///
    /// `AFXE` (US), `AFXJ` (Japan), and `AFXP` (Europe) are all Final Fantasy
    /// Tactics Advance, so this is what an addon should match on.
    pub fn code_prefix(&self) -> &str {
        let end = self.game_code.len().min(3);
        &self.game_code[..end]
    }

    /// The region letter of the game code, if present.
    pub fn region_code(&self) -> Option<char> {
        self.game_code.chars().nth(3)
    }

    /// A human-readable region name for the game code's region letter.
    pub fn region_name(&self) -> &'static str {
        match self.region_code() {
            Some('E') => "USA",
            Some('P') => "Europe",
            Some('J') => "Japan",
            Some('D') => "Germany",
            Some('F') => "France",
            Some('I') => "Italy",
            Some('S') => "Spain",
            Some('K') => "Korea",
            Some('C') => "China",
            _ => "Unknown",
        }
    }
}

/// Read the cartridge header out of memory.
///
/// Returns `None` when neither the title nor the game code is readable, which
/// is the case before a ROM is loaded.
pub fn read_rom_identity(memory: &dyn MemoryView) -> Option<RomIdentity> {
    let title = read_ascii(memory, ROM_BASE + HEADER_TITLE, HEADER_TITLE_LEN);
    let game_code = read_ascii(memory, ROM_BASE + HEADER_GAME_CODE, HEADER_GAME_CODE_LEN);
    if title.is_empty() && game_code.is_empty() {
        return None;
    }

    Some(RomIdentity {
        title,
        game_code,
        maker_code: read_ascii(memory, ROM_BASE + HEADER_MAKER_CODE, HEADER_MAKER_CODE_LEN),
        revision: memory.read_u8(ROM_BASE + HEADER_REVISION),
    })
}

/// Static metadata describing an addon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AddonInfo {
    /// Stable identifier, e.g. `pokemon_frlg_party`. Appears in exported JSON.
    pub addon_id: &'static str,
    /// Name shown in the UI.
    pub display_name: &'static str,
    pub version: &'static str,
    /// What this addon can report: `party`, `area`, `battle`, ...
    pub capabilities: &'static [&'static str],
    /// Which ROMs this addon claims, in words. Shown by `Tools > Addon Status`
    /// so a user can tell "no addon exists" from "the addon did not match".
    pub supported_games: &'static str,
}

/// A per-game data provider.
///
/// `T` is the host's payload type. The desktop frontend uses its own enum so
/// rich renderers can pattern-match on it; a host that only wants the generic
/// sections can use `serde_json::Value` or `()`.
///
/// Implementations must be cheap to call: `snapshot` runs on a timer while the
/// game is running (every 250 ms today).
pub trait GameAddon<T>: Send + Sync {
    fn info(&self) -> AddonInfo;

    /// Whether this addon claims `rom`. Match on
    /// [`RomIdentity::code_prefix`] rather than the full game code unless the
    /// addon genuinely only works for one region.
    fn supports(&self, rom: &RomIdentity) -> bool;

    /// Read the current game state.
    ///
    /// Return `None` when the data is not available *yet* — during the title
    /// screen, before a save is loaded, mid-transition. That is reported as
    /// [`Detection::Idle`] and is not an error.
    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot<T>>;
}

/// What detection found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection<T> {
    /// An addon claimed the ROM and produced data.
    Active {
        info: AddonInfo,
        snapshot: AddonSnapshot<T>,
    },
    /// An addon claimed the ROM but has nothing to report yet.
    Idle { info: AddonInfo },
    /// No registered addon claims this ROM.
    Unsupported,
}

impl<T> Detection<T> {
    pub fn snapshot(self) -> Option<AddonSnapshot<T>> {
        match self {
            Detection::Active { snapshot, .. } => Some(snapshot),
            _ => None,
        }
    }

    pub fn info(&self) -> Option<AddonInfo> {
        match self {
            Detection::Active { info, .. } | Detection::Idle { info } => Some(*info),
            Detection::Unsupported => None,
        }
    }

    /// A one-line explanation for `Tools > Addon Status`.
    pub fn status_line(&self) -> String {
        match self {
            Detection::Active { info, .. } => {
                if info.capabilities.is_empty() {
                    format!("{} is active", info.display_name)
                } else {
                    format!(
                        "{} is active ({})",
                        info.display_name,
                        info.capabilities.join(", ")
                    )
                }
            }
            Detection::Idle { info } => format!(
                "{} matched but has no data yet",
                info.display_name
            ),
            Detection::Unsupported => "No addon claims this ROM".to_string(),
        }
    }
}

/// The set of addons a host knows about.
pub struct AddonRegistry<T> {
    addons: Vec<Box<dyn GameAddon<T>>>,
}

impl<T> Default for AddonRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AddonRegistry<T> {
    pub fn new() -> Self {
        Self { addons: Vec::new() }
    }

    pub fn register(&mut self, addon: Box<dyn GameAddon<T>>) {
        self.addons.push(addon);
    }

    /// Builder form, for assembling a registry in one expression.
    pub fn with(mut self, addon: Box<dyn GameAddon<T>>) -> Self {
        self.register(addon);
        self
    }

    pub fn len(&self) -> usize {
        self.addons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.addons.is_empty()
    }

    /// Metadata for every registered addon, in registration order.
    pub fn infos(&self) -> Vec<AddonInfo> {
        self.addons.iter().map(|addon| addon.info()).collect()
    }

    /// Addons that claim `rom`, whether or not they can currently read data.
    pub fn matching(&self, rom: &RomIdentity) -> Vec<AddonInfo> {
        self.addons
            .iter()
            .filter(|addon| addon.supports(rom))
            .map(|addon| addon.info())
            .collect()
    }

    /// Find the addon for `rom` and ask it for a snapshot.
    ///
    /// The first addon that both claims the ROM *and* produces data wins. An
    /// addon that claims the ROM but returns `None` does not block a later one,
    /// so a broad fallback addon can be registered behind a specific one.
    pub fn detect(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Detection<T> {
        let mut first_claim = None;

        for addon in &self.addons {
            if !addon.supports(rom) {
                continue;
            }
            let info = addon.info();
            if first_claim.is_none() {
                first_claim = Some(info);
            }
            if let Some(snapshot) = addon.snapshot(memory, rom) {
                return Detection::Active { info, snapshot };
            }
        }

        match first_claim {
            Some(info) => Detection::Idle { info },
            None => Detection::Unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SparseMemory;
    use crate::schema::AddonSnapshot;

    fn ffta_rom() -> RomIdentity {
        RomIdentity {
            title: "FFTA_USVER.".to_string(),
            game_code: "AFXE".to_string(),
            maker_code: "01".to_string(),
            revision: 0,
        }
    }

    /// An addon that claims ROMs by code prefix and optionally yields data.
    struct StubAddon {
        id: &'static str,
        prefix: &'static str,
        yields_data: bool,
    }

    impl GameAddon<u32> for StubAddon {
        fn info(&self) -> AddonInfo {
            AddonInfo {
                addon_id: self.id,
                display_name: self.id,
                version: "1.0.0",
                capabilities: &["test"],
                supported_games: "test ROMs",
            }
        }

        fn supports(&self, rom: &RomIdentity) -> bool {
            rom.code_prefix() == self.prefix
        }

        fn snapshot(&self, _memory: &dyn MemoryView, _rom: &RomIdentity) -> Option<AddonSnapshot<u32>> {
            self.yields_data
                .then(|| AddonSnapshot::new(self.id, self.id, Vec::new(), 7))
        }
    }

    fn stub(id: &'static str, prefix: &'static str, yields_data: bool) -> Box<dyn GameAddon<u32>> {
        Box::new(StubAddon {
            id,
            prefix,
            yields_data,
        })
    }

    #[test]
    fn rom_identity_reads_the_cartridge_header() {
        let memory = SparseMemory::new().with_rom_header("FFTA_USVER.", "AFXE", "01", 2);
        let rom = read_rom_identity(&memory).expect("header should parse");

        assert_eq!(rom.title, "FFTA_USVER.");
        assert_eq!(rom.game_code, "AFXE");
        assert_eq!(rom.maker_code, "01");
        assert_eq!(rom.revision, 2);
    }

    #[test]
    fn rom_identity_is_none_before_a_rom_is_loaded() {
        assert!(read_rom_identity(&SparseMemory::new()).is_none());
    }

    #[test]
    fn code_prefix_ignores_the_region_letter() {
        let mut rom = ffta_rom();
        assert_eq!(rom.code_prefix(), "AFX");
        rom.game_code = "AFXJ".to_string();
        assert_eq!(rom.code_prefix(), "AFX");
        assert_eq!(rom.region_name(), "Japan");
    }

    #[test]
    fn code_prefix_survives_a_short_or_empty_game_code() {
        let mut rom = ffta_rom();
        rom.game_code = "AF".to_string();
        assert_eq!(rom.code_prefix(), "AF");
        rom.game_code = String::new();
        assert_eq!(rom.code_prefix(), "");
        assert_eq!(rom.region_name(), "Unknown");
    }

    #[test]
    fn detection_returns_the_snapshot_of_a_matching_addon() {
        let registry = AddonRegistry::new().with(stub("ffta", "AFX", true));
        let detection = registry.detect(&SparseMemory::new(), &ffta_rom());

        assert!(matches!(detection, Detection::Active { .. }));
        assert_eq!(detection.snapshot().map(|s| s.data), Some(7));
    }

    #[test]
    fn an_addon_with_no_data_yet_reports_idle_not_unsupported() {
        let registry = AddonRegistry::new().with(stub("ffta", "AFX", false));
        let detection = registry.detect(&SparseMemory::new(), &ffta_rom());

        match &detection {
            Detection::Idle { info } => assert_eq!(info.addon_id, "ffta"),
            other => panic!("expected Idle, got {other:?}"),
        }
        assert!(detection.status_line().contains("no data yet"));
    }

    #[test]
    fn an_unclaimed_rom_reports_unsupported() {
        let registry = AddonRegistry::new().with(stub("ffta", "AFX", true));
        let mut rom = ffta_rom();
        rom.game_code = "BPRE".to_string();

        let detection = registry.detect(&SparseMemory::new(), &rom);
        assert_eq!(detection, Detection::Unsupported);
        assert_eq!(detection.status_line(), "No addon claims this ROM");
    }

    #[test]
    fn a_claiming_addon_with_no_data_does_not_block_a_later_one() {
        // This is what lets a broad fallback addon sit behind a specific one.
        let registry = AddonRegistry::new()
            .with(stub("specific", "AFX", false))
            .with(stub("fallback", "AFX", true));

        match registry.detect(&SparseMemory::new(), &ffta_rom()) {
            Detection::Active { info, .. } => assert_eq!(info.addon_id, "fallback"),
            other => panic!("expected the fallback to win, got {other:?}"),
        }
    }

    #[test]
    fn registration_order_decides_which_addon_wins() {
        let registry = AddonRegistry::new()
            .with(stub("first", "AFX", true))
            .with(stub("second", "AFX", true));

        match registry.detect(&SparseMemory::new(), &ffta_rom()) {
            Detection::Active { info, .. } => assert_eq!(info.addon_id, "first"),
            other => panic!("expected the first addon, got {other:?}"),
        }
    }

    #[test]
    fn matching_lists_every_claimant_for_diagnostics() {
        let registry = AddonRegistry::new()
            .with(stub("a", "AFX", false))
            .with(stub("b", "AFX", true))
            .with(stub("c", "BPR", true));

        let ids: Vec<_> = registry
            .matching(&ffta_rom())
            .iter()
            .map(|info| info.addon_id)
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn an_empty_registry_claims_nothing() {
        let registry: AddonRegistry<u32> = AddonRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(
            registry.detect(&SparseMemory::new(), &ffta_rom()),
            Detection::Unsupported
        );
    }
}

//! Shared addon contracts for tinyBird hosts, overlays, and external tools.
//!
//! This crate is the addon extension point. It has three parts:
//!
//! - [`schema`] — the stable export envelope (`StreamSnapshot`,
//!   `AddonSnapshot`, `AddonSection`) that overlays and tools consume.
//! - [`memory`] — [`MemoryView`], the narrow read-only window on emulated
//!   memory that an addon is given. Deliberately small, so this crate needs no
//!   dependency on `tinybird-core` and addons can be tested against a handful
//!   of bytes via [`SparseMemory`].
//! - [`registry`] — [`GameAddon`] and [`AddonRegistry`]: how a host discovers
//!   which addon claims a ROM, and why it produced nothing when it did not.
//!
//! Adding support for a new game means implementing [`GameAddon`] and
//! registering it. Nothing else in the workspace needs to change.
//!
//! See `ADDONS.md` for a worked example.

pub mod manifest;
pub mod memory;
pub mod registry;
pub mod schema;

pub use manifest::{Manifest, ManifestAddon};
pub use memory::{read_ascii, MemoryView, SparseMemory, EWRAM_BASE, IWRAM_BASE, ROM_BASE};
pub use registry::{
    read_rom_identity, AddonInfo, AddonRegistry, Detection, GameAddon, RomIdentity,
};
pub use schema::{
    AddonBadge, AddonCard, AddonField, AddonImage, AddonMeter, AddonSection, AddonSectionContent,
    AddonSnapshot, AddonTable, AddonTone, StreamSnapshot, SNAPSHOT_SCHEMA_VERSION,
};

//! GBA Memory Map Constants
//!
//! The GBA has a 32-bit address space (4GB), but only specific regions are mapped.
//! Unmapped regions return open bus values on read.

/// Total addressable memory space (32-bit)
pub const MEMORY_SIZE: u32 = 0x1000_0000;

// Memory Regions
/// BIOS region (32KB, mirrored)
#[allow(missing_docs)]
pub const REGION_BIOS_START: u32 = 0x0000_0000;
#[allow(missing_docs)]
pub const REGION_BIOS_END: u32 = 0x0000_3FFF;
#[allow(missing_docs)]
pub const REGION_BIOS_SIZE: usize = 0x4000;

/// External Work RAM (256KB)
#[allow(missing_docs)]
pub const REGION_EWRAM_START: u32 = 0x0200_0000;
#[allow(missing_docs)]
pub const REGION_EWRAM_END: u32 = 0x02FF_FFFF;
#[allow(missing_docs)]
pub const REGION_EWRAM_SIZE: usize = 0x40000;

/// Internal Work RAM (32KB)
#[allow(missing_docs)]
pub const REGION_IWRAM_START: u32 = 0x0300_0000;
#[allow(missing_docs)]
pub const REGION_IWRAM_END: u32 = 0x03FF_FFFF;
#[allow(missing_docs)]
pub const REGION_IWRAM_SIZE: usize = 0x8000;

/// I/O Registers
#[allow(missing_docs)]
pub const REGION_IO_START: u32 = 0x0400_0000;
#[allow(missing_docs)]
pub const REGION_IO_END: u32 = 0x04FF_FFFF;
#[allow(missing_docs)]
pub const REGION_IO_SIZE: usize = 0x400;

/// Palette RAM (1KB)
#[allow(missing_docs)]
pub const REGION_PALETTE_START: u32 = 0x0500_0000;
#[allow(missing_docs)]
pub const REGION_PALETTE_END: u32 = 0x05FF_FFFF;
#[allow(missing_docs)]
pub const REGION_PALETTE_SIZE: usize = 0x400;

/// VRAM (96KB)
#[allow(missing_docs)]
pub const REGION_VRAM_START: u32 = 0x0600_0000;
#[allow(missing_docs)]
pub const REGION_VRAM_END: u32 = 0x06FF_FFFF;
#[allow(missing_docs)]
pub const REGION_VRAM_SIZE: usize = 0x18000;

/// OAM - Object Attribute Memory (1KB)
#[allow(missing_docs)]
pub const REGION_OAM_START: u32 = 0x0700_0000;
#[allow(missing_docs)]
pub const REGION_OAM_END: u32 = 0x07FF_FFFF;
#[allow(missing_docs)]
pub const REGION_OAM_SIZE: usize = 0x400;

/// Game Pak ROM (mirrored across 0x08000000-0x0DFFFFFF, max cart size 32MB)
#[allow(missing_docs)]
pub const REGION_ROM_START: u32 = 0x0800_0000;
#[allow(missing_docs)]
pub const REGION_ROM_END: u32 = 0x0DFF_FFFF;
#[allow(missing_docs)]
pub const REGION_ROM_MAX_SIZE: usize = 0x200_0000;

/// Game Pak SRAM/Flash window (64KB)
#[allow(missing_docs)]
pub const REGION_SRAM_START: u32 = 0x0E00_0000;
#[allow(missing_docs)]
pub const REGION_SRAM_END: u32 = 0x0E00_FFFF;
#[allow(missing_docs)]
pub const REGION_SRAM_SIZE: usize = 0x10000;

// Wait states for different memory regions (in cycles)
/// BIOS wait states
#[allow(missing_docs)]
pub const WAITSTATE_BIOS: u32 = 0;

/// EWRAM wait states (configurable)
#[allow(missing_docs)]
pub const WAITSTATE_EWRAM: u32 = 4;

/// IWRAM wait states
#[allow(missing_docs)]
pub const WAITSTATE_IWRAM: u32 = 0;

/// I/O registers wait states
#[allow(missing_docs)]
pub const WAITSTATE_IO: u32 = 0;

/// VRAM wait states
#[allow(missing_docs)]
pub const WAITSTATE_VRAM: u32 = 0;

/// OAM wait states
#[allow(missing_docs)]
pub const WAITSTATE_OAM: u32 = 0;

/// ROM wait states (varies by region)
#[allow(missing_docs)]
pub const WAITSTATE_ROM_0: u32 = 4; // 0x08000000-0x09FFFFFF
#[allow(missing_docs)]
pub const WAITSTATE_ROM_1: u32 = 4; // 0x0A000000-0x0BFFFFFF
#[allow(missing_docs)]
pub const WAITSTATE_ROM_2: u32 = 4; // 0x0C000000-0x0DFFFFFF
#[allow(missing_docs)]
pub const WAITSTATE_ROM_3: u32 = 8; // 0x0E000000-0x0FFFFFFF

/// Get the memory region name for debugging
pub fn get_region_name(addr: u32) -> &'static str {
    match addr {
        REGION_BIOS_START..=REGION_BIOS_END => "BIOS",
        REGION_EWRAM_START..=REGION_EWRAM_END => "EWRAM",
        REGION_IWRAM_START..=REGION_IWRAM_END => "IWRAM",
        REGION_IO_START..=REGION_IO_END => "I/O",
        REGION_PALETTE_START..=REGION_PALETTE_END => "Palette",
        REGION_VRAM_START..=REGION_VRAM_END => "VRAM",
        REGION_OAM_START..=REGION_OAM_END => "OAM",
        REGION_ROM_START..=REGION_ROM_END => "ROM",
        REGION_SRAM_START..=REGION_SRAM_END => "SRAM",
        _ => "Unmapped",
    }
}

/// Check if an address is in a valid memory region
pub fn is_valid_address(addr: u32) -> bool {
    matches!(addr,
        REGION_BIOS_START..=REGION_BIOS_END |
        REGION_EWRAM_START..=REGION_EWRAM_END |
        REGION_IWRAM_START..=REGION_IWRAM_END |
        REGION_IO_START..=REGION_IO_END |
        REGION_PALETTE_START..=REGION_PALETTE_END |
        REGION_VRAM_START..=REGION_VRAM_END |
        REGION_OAM_START..=REGION_OAM_END |
        REGION_ROM_START..=REGION_ROM_END |
        REGION_SRAM_START..=REGION_SRAM_END
    )
}

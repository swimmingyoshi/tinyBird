//! A narrow read-only view of emulated memory, so addons never need the
//! emulator itself.
//!
//! This is deliberately the *only* thing an addon is given. Keeping the
//! interface this small means:
//!
//! - `tinybird-addons` stays free of a `tinybird-core` dependency, so
//!   `tinybird-web` does not pull the whole emulator in just to read a schema;
//! - an addon can be unit-tested against a handful of bytes with
//!   [`SparseMemory`] instead of booting a ROM and playing to the right screen.
//!
//! Reads never fail. Unmapped addresses read as zero, matching how a GBA game
//! sees open bus closely enough for structure parsing, and letting addons use
//! plain expressions instead of threading `Result` through every field.

use std::collections::BTreeMap;

/// Base address of the cartridge ROM in the GBA memory map.
pub const ROM_BASE: u32 = 0x0800_0000;
/// Base address of on-board work RAM (256 KB).
pub const EWRAM_BASE: u32 = 0x0200_0000;
/// Size of on-board work RAM.
pub const EWRAM_SIZE: u32 = 0x0004_0000;
/// Base address of on-chip work RAM (32 KB).
pub const IWRAM_BASE: u32 = 0x0300_0000;
/// Size of on-chip work RAM.
pub const IWRAM_SIZE: u32 = 0x0000_8000;

/// Read-only access to emulated memory.
///
/// Implementors only have to provide [`MemoryView::read_u8`]; the wider reads
/// default to little-endian composition, which is what the ARM7TDMI does.
pub trait MemoryView {
    fn read_u8(&self, addr: u32) -> u8;

    fn read_u16(&self, addr: u32) -> u16 {
        u16::from_le_bytes([self.read_u8(addr), self.read_u8(addr.wrapping_add(1))])
    }

    fn read_u32(&self, addr: u32) -> u32 {
        u32::from_le_bytes([
            self.read_u8(addr),
            self.read_u8(addr.wrapping_add(1)),
            self.read_u8(addr.wrapping_add(2)),
            self.read_u8(addr.wrapping_add(3)),
        ])
    }

    /// Fill `out` with consecutive bytes starting at `addr`.
    fn read_into(&self, addr: u32, out: &mut [u8]) {
        for (offset, slot) in out.iter_mut().enumerate() {
            *slot = self.read_u8(addr.wrapping_add(offset as u32));
        }
    }

    fn read_bytes(&self, addr: u32, len: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; len];
        self.read_into(addr, &mut buffer);
        buffer
    }
}

impl<T: MemoryView + ?Sized> MemoryView for &T {
    fn read_u8(&self, addr: u32) -> u8 {
        (**self).read_u8(addr)
    }
}

/// Read a NUL-terminated ASCII string of at most `max_len` bytes.
///
/// Bytes that are not printable ASCII terminate the string rather than being
/// substituted, because a run of garbage is a much stronger signal that a
/// pointer is wrong than a run of `?` characters would be.
pub fn read_ascii(memory: &dyn MemoryView, addr: u32, max_len: usize) -> String {
    let mut text = String::with_capacity(max_len);
    for offset in 0..max_len {
        let byte = memory.read_u8(addr.wrapping_add(offset as u32));
        if byte == 0 || byte == 0xFF {
            break;
        }
        if !byte.is_ascii_graphic() && byte != b' ' {
            break;
        }
        text.push(byte as char);
    }
    text.trim().to_string()
}

/// Whether `addr` points into a region a game could plausibly keep data in.
///
/// Useful for sanity-checking a pointer read out of RAM before dereferencing it,
/// which is the single most common way a game-specific addon goes wrong after a
/// game update or a region change.
pub fn is_plausible_data_pointer(addr: u32) -> bool {
    (EWRAM_BASE..EWRAM_BASE + EWRAM_SIZE).contains(&addr)
        || (IWRAM_BASE..IWRAM_BASE + IWRAM_SIZE).contains(&addr)
        || (ROM_BASE..ROM_BASE + 0x0200_0000).contains(&addr)
}

/// An in-memory [`MemoryView`] built from sparse regions.
///
/// Its reason to exist is testing: an addon test can place the twelve bytes it
/// cares about at the right address and assert on the parse, with no ROM, no
/// BIOS, and no need to play the game to the screen where the data appears.
#[derive(Clone, Debug, Default)]
pub struct SparseMemory {
    regions: BTreeMap<u32, Vec<u8>>,
}

impl SparseMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Place `bytes` at `addr`. Later writes to the same address replace it.
    pub fn write(&mut self, addr: u32, bytes: impl Into<Vec<u8>>) -> &mut Self {
        self.regions.insert(addr, bytes.into());
        self
    }

    pub fn with(mut self, addr: u32, bytes: impl Into<Vec<u8>>) -> Self {
        self.write(addr, bytes);
        self
    }

    pub fn write_u16(&mut self, addr: u32, value: u16) -> &mut Self {
        self.write(addr, value.to_le_bytes().to_vec())
    }

    pub fn write_u32(&mut self, addr: u32, value: u32) -> &mut Self {
        self.write(addr, value.to_le_bytes().to_vec())
    }

    /// Write a standard GBA cartridge header so ROM identification works.
    pub fn with_rom_header(
        mut self,
        title: &str,
        game_code: &str,
        maker: &str,
        revision: u8,
    ) -> Self {
        let mut title_bytes = [0u8; 12];
        for (slot, byte) in title_bytes.iter_mut().zip(title.bytes()) {
            *slot = byte;
        }
        let mut code_bytes = [0u8; 4];
        for (slot, byte) in code_bytes.iter_mut().zip(game_code.bytes()) {
            *slot = byte;
        }
        let mut maker_bytes = [0u8; 2];
        for (slot, byte) in maker_bytes.iter_mut().zip(maker.bytes()) {
            *slot = byte;
        }
        self.write(ROM_BASE + 0xA0, title_bytes.to_vec());
        self.write(ROM_BASE + 0xAC, code_bytes.to_vec());
        self.write(ROM_BASE + 0xB0, maker_bytes.to_vec());
        self.write(ROM_BASE + 0xBC, vec![revision]);
        self
    }
}

impl MemoryView for SparseMemory {
    fn read_u8(&self, addr: u32) -> u8 {
        // The last region starting at or before `addr` is the only one that can
        // contain it, because regions are keyed by their base address.
        let Some((base, bytes)) = self.regions.range(..=addr).next_back() else {
            return 0;
        };
        let offset = (addr - base) as usize;
        bytes.get(offset).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_reads_are_little_endian() {
        let memory = SparseMemory::new().with(0x0200_0000, vec![0x78, 0x56, 0x34, 0x12]);
        assert_eq!(memory.read_u8(0x0200_0000), 0x78);
        assert_eq!(memory.read_u16(0x0200_0000), 0x5678);
        assert_eq!(memory.read_u32(0x0200_0000), 0x1234_5678);
    }

    #[test]
    fn unmapped_addresses_read_as_zero() {
        let memory = SparseMemory::new().with(0x0200_0000, vec![0xFF; 4]);
        assert_eq!(memory.read_u32(0x0300_0000), 0);
        assert_eq!(memory.read_u8(0x0200_0004), 0, "just past the region");
        assert_eq!(memory.read_u8(0x01FF_FFFF), 0, "just before the region");
    }

    #[test]
    fn regions_do_not_bleed_into_each_other() {
        let memory = SparseMemory::new()
            .with(0x0200_0000, vec![0x11, 0x22])
            .with(0x0200_0100, vec![0x33, 0x44]);
        assert_eq!(memory.read_u8(0x0200_0001), 0x22);
        assert_eq!(memory.read_u8(0x0200_0002), 0, "gap between regions");
        assert_eq!(memory.read_u8(0x0200_0100), 0x33);
    }

    #[test]
    fn read_ascii_stops_at_the_terminator() {
        let memory = SparseMemory::new().with(0x0200_0000, b"Marche\0garbage".to_vec());
        assert_eq!(read_ascii(&memory, 0x0200_0000, 16), "Marche");
    }

    #[test]
    fn read_ascii_stops_at_the_first_non_printable_byte() {
        // A wrong pointer usually lands in binary data; stopping early makes
        // that obvious instead of rendering a line of substitution characters.
        let memory = SparseMemory::new().with(0x0200_0000, vec![b'O', b'K', 0x01, b'X']);
        assert_eq!(read_ascii(&memory, 0x0200_0000, 8), "OK");
    }

    #[test]
    fn read_ascii_respects_the_length_cap() {
        let memory = SparseMemory::new().with(0x0200_0000, b"ABCDEFGHIJ".to_vec());
        assert_eq!(read_ascii(&memory, 0x0200_0000, 4), "ABCD");
    }

    #[test]
    fn read_bytes_zero_fills_past_the_end_of_a_region() {
        let memory = SparseMemory::new().with(0x0200_0000, vec![1, 2]);
        assert_eq!(memory.read_bytes(0x0200_0000, 4), vec![1, 2, 0, 0]);
    }

    #[test]
    fn pointer_plausibility_covers_ewram_iwram_and_rom() {
        assert!(is_plausible_data_pointer(0x0200_4284));
        assert!(is_plausible_data_pointer(0x0300_5008));
        assert!(is_plausible_data_pointer(0x0800_0000));

        assert!(!is_plausible_data_pointer(0));
        assert!(!is_plausible_data_pointer(0x0400_0000), "I/O registers");
        assert!(!is_plausible_data_pointer(0x0204_0000), "past end of EWRAM");
        assert!(!is_plausible_data_pointer(0xFFFF_FFFF));
    }

    #[test]
    fn rom_header_helper_places_fields_where_the_gba_spec_says() {
        let memory = SparseMemory::new().with_rom_header("FFTA_USVER.", "AFXE", "01", 0);
        assert_eq!(read_ascii(&memory, ROM_BASE + 0xA0, 12), "FFTA_USVER.");
        assert_eq!(read_ascii(&memory, ROM_BASE + 0xAC, 4), "AFXE");
        assert_eq!(read_ascii(&memory, ROM_BASE + 0xB0, 2), "01");
        assert_eq!(memory.read_u8(ROM_BASE + 0xBC), 0);
    }
}

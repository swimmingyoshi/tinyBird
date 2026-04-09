//! Memory Bus Implementation
//!
//! The GBA bus connects the CPU to all memory regions and peripherals.
//! It handles address decoding, wait states, and open bus behavior.

use crate::memory_map::*;
use crate::debug::config as debug_config;
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use std::cell::Cell;

thread_local! {
    /// Current emulator cycle (set by Gba::step for debug watchpoints)
    pub static DEBUG_CYCLE: Cell<u64> = Cell::new(0);
    /// Current PC (set by Gba::step for debug watchpoints)
    pub static DEBUG_PC: Cell<u32> = Cell::new(0);
}

/// Bus error types
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum BusError {
    #[error("Unmapped memory access at address 0x{0:08X}")]
    UnmappedAccess(u32),
    #[error("BIOS write attempt at address 0x{0:08X}")]
    BiosWrite(u32),
}

/// Bus result type
pub type BusResult<T> = Result<T, BusError>;

const AUDIO_IO_START: usize = 0x60;
const AUDIO_IO_END: usize = 0xA8;
const AUDIO_IO_LEN: usize = AUDIO_IO_END - AUDIO_IO_START;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct DirtyRange {
    start: usize,
    end: usize,
    dirty: bool,
}

impl DirtyRange {
    fn mark(&mut self, start: usize, len: usize, max_len: usize) {
        if start >= max_len || len == 0 {
            return;
        }

        let end = start.saturating_add(len).min(max_len);
        if !self.dirty {
            self.start = start;
            self.end = end;
            self.dirty = true;
        } else {
            self.start = self.start.min(start);
            self.end = self.end.max(end);
        }
    }

    fn take(&mut self) -> Option<(usize, usize)> {
        if !self.dirty {
            return None;
        }

        let range = (self.start, self.end);
        *self = Self::default();
        Some(range)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessWidth {
    Byte,
    Half,
    Word,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessKind {
    Data,
    Opcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessRegion {
    Bios,
    Ewram,
    Iwram,
    Io,
    Palette,
    Vram,
    Oam,
    Rom(usize),
    Sram,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AccessStamp {
    region: AccessRegion,
    next_addr: u32,
}

/// Trait for bus access
pub trait Bus {
    /// Read a byte from the bus
    fn read_u8(&self, addr: u32) -> u8;

    /// Read a halfword (16-bit) from the bus (does not update open bus value)
    fn read_u16(&self, addr: u32) -> u16;

    /// Read a word (32-bit) from the bus (does not update open bus value)
    fn read_u32(&self, addr: u32) -> u32;

    /// Read a halfword opcode from the bus.
    fn read_opcode_u16(&self, addr: u32) -> u16 {
        self.read_u16(addr)
    }

    /// Read a word opcode from the bus.
    fn read_opcode_u32(&self, addr: u32) -> u32 {
        self.read_u32(addr)
    }

    /// Write a byte to the bus
    fn write_u8(&mut self, addr: u32, value: u8);

    /// Write a halfword (16-bit) to the bus
    fn write_u16(&mut self, addr: u32, value: u16);

    /// Write a word (32-bit) to the bus
    fn write_u32(&mut self, addr: u32, value: u32);

    /// Check if an address is readable
    fn is_readable(&self, addr: u32) -> bool;

    /// Check if an address is writable
    fn is_writable(&self, addr: u32) -> bool;

    /// Start collecting timing for one CPU instruction.
    fn begin_instruction_timing(&mut self) {}

    /// Finish collecting timing for one CPU instruction.
    fn finish_instruction_timing(&mut self) -> u32 {
        1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SaveType {
    None,
    Sram,
    Flash64K,
    Flash128K,
    Eeprom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum FlashCommandState {
    Ready,
    Unlock1,
    Unlock2,
    Program,
    BankSwitch,
    EraseUnlock1,
    EraseUnlock2,
    EraseCommand,
}

/// Simple bus implementation with ROM and RAM arrays
#[derive(Clone)]
pub struct SimpleBus {
    /// BIOS memory (32KB, read-only after boot)
    bios: Box<[u8; REGION_BIOS_SIZE]>,
    /// External Work RAM (256KB)
    ewram: Box<[u8; REGION_EWRAM_SIZE]>,
    /// Internal Work RAM (32KB)
    iwram: Box<[u8; REGION_IWRAM_SIZE]>,
    /// I/O registers (simplified - just an array for now)
    io: Box<[u8; REGION_IO_SIZE]>,
    /// Palette RAM (1KB)
    palette: Box<[u8; REGION_PALETTE_SIZE]>,
    /// VRAM (96KB)
    vram: Box<[u8; REGION_VRAM_SIZE]>,
    /// OAM (1KB)
    oam: Box<[u8; REGION_OAM_SIZE]>,
    /// Dirty byte span for palette RAM since the last PPU sync.
    palette_dirty: DirtyRange,
    /// Dirty byte span for VRAM since the last PPU sync.
    vram_dirty: DirtyRange,
    /// Dirty byte span for OAM since the last PPU sync.
    oam_dirty: DirtyRange,
    /// Dirty byte span for sound I/O registers since the last APU sync.
    audio_io_dirty: DirtyRange,
    /// Set when any DMA register (I/O 0xB0..=0xDF) was written; cleared by take_dma_dirty.
    dma_dirty: bool,
    /// Game Pak ROM (variable size, using max for simplicity)
    rom: Vec<u8>,
    /// Cartridge-backed save memory.
    save_memory: Vec<u8>,
    /// Detected save hardware type for the current ROM.
    save_type: SaveType,
    /// Flash identify-mode flag used by cartridge probing.
    flash_id_mode: bool,
    /// Current flash command state.
    flash_cmd_state: FlashCommandState,
    /// Active 64KB bank for 128KB flash cartridges.
    flash_bank: usize,
    /// Whether cartridge save memory needs flushing to disk.
    save_dirty: bool,
    /// BIOS is protected (read-only) after boot
    bios_protected: bool,
    /// Last read value for open bus behavior
    open_bus_value: u32,
    /// VCOUNT - current scanline (0-227)
    vcount: u16,
    /// Total cycles for VCOUNT tracking
    vcount_cycles: u64,
    /// Whether CPU instruction timing collection is active.
    cpu_timing_active: Cell<bool>,
    /// Cycles accumulated for the current CPU instruction.
    cpu_timing_cycles: Cell<u32>,
    /// Last CPU-side memory access within the current instruction.
    cpu_last_access: Cell<Option<AccessStamp>>,
}

#[derive(Serialize, Deserialize)]
struct SimpleBusSerde {
    bios: Vec<u8>,
    ewram: Vec<u8>,
    iwram: Vec<u8>,
    io: Vec<u8>,
    palette: Vec<u8>,
    vram: Vec<u8>,
    oam: Vec<u8>,
    palette_dirty: DirtyRange,
    vram_dirty: DirtyRange,
    oam_dirty: DirtyRange,
    audio_io_dirty: DirtyRange,
    dma_dirty: bool,
    rom: Vec<u8>,
    save_memory: Vec<u8>,
    save_type: SaveType,
    flash_id_mode: bool,
    flash_cmd_state: FlashCommandState,
    flash_bank: usize,
    save_dirty: bool,
    bios_protected: bool,
    open_bus_value: u32,
    vcount: u16,
    vcount_cycles: u64,
}

fn vec_to_boxed_array<const N: usize, E: DeError>(vec: Vec<u8>, name: &str) -> Result<Box<[u8; N]>, E> {
    let arr: [u8; N] = vec
        .try_into()
        .map_err(|v: Vec<u8>| E::custom(format!("{} had length {}, expected {}", name, v.len(), N)))?;
    Ok(Box::new(arr))
}

impl Serialize for SimpleBus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let helper = SimpleBusSerde {
            bios: self.bios.as_ref().to_vec(),
            ewram: self.ewram.as_ref().to_vec(),
            iwram: self.iwram.as_ref().to_vec(),
            io: self.io.as_ref().to_vec(),
            palette: self.palette.as_ref().to_vec(),
            vram: self.vram.as_ref().to_vec(),
            oam: self.oam.as_ref().to_vec(),
            palette_dirty: self.palette_dirty,
            vram_dirty: self.vram_dirty,
            oam_dirty: self.oam_dirty,
            audio_io_dirty: self.audio_io_dirty,
            dma_dirty: self.dma_dirty,
            rom: self.rom.clone(),
            save_memory: self.save_memory.clone(),
            save_type: self.save_type,
            flash_id_mode: self.flash_id_mode,
            flash_cmd_state: self.flash_cmd_state,
            flash_bank: self.flash_bank,
            save_dirty: self.save_dirty,
            bios_protected: self.bios_protected,
            open_bus_value: self.open_bus_value,
            vcount: self.vcount,
            vcount_cycles: self.vcount_cycles,
        };
        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SimpleBus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = SimpleBusSerde::deserialize(deserializer)?;
        Ok(Self {
            bios: vec_to_boxed_array::<REGION_BIOS_SIZE, D::Error>(helper.bios, "bios")?,
            ewram: vec_to_boxed_array::<REGION_EWRAM_SIZE, D::Error>(helper.ewram, "ewram")?,
            iwram: vec_to_boxed_array::<REGION_IWRAM_SIZE, D::Error>(helper.iwram, "iwram")?,
            io: vec_to_boxed_array::<REGION_IO_SIZE, D::Error>(helper.io, "io")?,
            palette: vec_to_boxed_array::<REGION_PALETTE_SIZE, D::Error>(helper.palette, "palette")?,
            vram: vec_to_boxed_array::<REGION_VRAM_SIZE, D::Error>(helper.vram, "vram")?,
            oam: vec_to_boxed_array::<REGION_OAM_SIZE, D::Error>(helper.oam, "oam")?,
            palette_dirty: helper.palette_dirty,
            vram_dirty: helper.vram_dirty,
            oam_dirty: helper.oam_dirty,
            audio_io_dirty: helper.audio_io_dirty,
            dma_dirty: helper.dma_dirty,
            rom: helper.rom,
            save_memory: helper.save_memory,
            save_type: helper.save_type,
            flash_id_mode: helper.flash_id_mode,
            flash_cmd_state: helper.flash_cmd_state,
            flash_bank: helper.flash_bank,
            save_dirty: helper.save_dirty,
            bios_protected: helper.bios_protected,
            open_bus_value: helper.open_bus_value,
            vcount: helper.vcount,
            vcount_cycles: helper.vcount_cycles,
            cpu_timing_active: Cell::new(false),
            cpu_timing_cycles: Cell::new(0),
            cpu_last_access: Cell::new(None),
        })
    }
}

impl SimpleBus {
    /// Map a VRAM bus address to the underlying 96KB physical VRAM layout.
    ///
    /// The 128KB CPU-visible VRAM window mirrors `0x06018000..=0x0601FFFF`
    /// onto the upper 32KB physical block at `0x06010000..=0x06017FFF`.
    fn vram_index(addr: u32) -> u32 {
        let mut offset = addr & 0x1_FFFF;
        if offset >= 0x18_000 {
            offset -= 0x8_000;
        }
        offset
    }

    /// Create a new simple bus with optional ROM data
    pub fn new(rom: Option<Vec<u8>>) -> Self {
        let mut bus = Self {
            bios: Box::new([0; REGION_BIOS_SIZE]),
            ewram: Box::new([0; REGION_EWRAM_SIZE]),
            iwram: Box::new([0; REGION_IWRAM_SIZE]),
            io: Box::new([0; REGION_IO_SIZE]),
            palette: Box::new([0; REGION_PALETTE_SIZE]),
            vram: Box::new([0; REGION_VRAM_SIZE]),
            oam: Box::new([0; REGION_OAM_SIZE]),
            palette_dirty: DirtyRange::default(),
            vram_dirty: DirtyRange::default(),
            oam_dirty: DirtyRange::default(),
            audio_io_dirty: DirtyRange::default(),
            dma_dirty: false,
            rom: rom.unwrap_or_default(),
            save_memory: vec![0xFF; REGION_SRAM_SIZE],
            save_type: SaveType::None,
            flash_id_mode: false,
            flash_cmd_state: FlashCommandState::Ready,
            flash_bank: 0,
            save_dirty: false,
            bios_protected: false,
            open_bus_value: 0,
            vcount: 0,
            vcount_cycles: 0,
            cpu_timing_active: Cell::new(false),
            cpu_timing_cycles: Cell::new(0),
            cpu_last_access: Cell::new(None),
        };
        bus.detect_save_type();
        bus.init_bios_stub();
        bus
    }

    /// Initialize a minimal BIOS stub for HLE.
    ///
    /// Exception vector table (0x00-0x1C) + IRQ trampoline (0x18-0x3C).
    ///
    /// Vectors 0x00-0x14 use BX LR so that accidental jumps to address 0
    /// (e.g. null function-pointer calls) return harmlessly to the caller.
    ///
    /// IRQ trampoline at 0x18 calls the game's handler stored at [0x03007FFC]
    /// (GBA standard convention) and performs an exception return via SUBS PC, LR, #4.
    ///
    /// Assembled ARM instructions (little-endian):
    ///   0x00: BX LR                          ; reset/null-fn stub — return to caller
    ///   0x04: BX LR                          ; undefined instruction stub
    ///   0x08: BX LR                          ; SWI stub (handled by HLE, never reached)
    ///   0x0C: BX LR                          ; prefetch abort stub
    ///   0x10: BX LR                          ; data abort stub
    ///   0x14: BX LR                          ; reserved stub
    ///   0x18: STMFD SP!, {R0-R3, R12, LR}   ; save regs on IRQ stack
    ///   0x1C: MOV   R0, #0x03000000          ;
    ///   0x20: ADD   R0, R0, #0x7F00          ; R0 = 0x03007F00
    ///   0x24: LDR   R0, [R0, #0xFC]          ; R0 = [0x03007FFC]
    ///   0x28: CMP   R0, #0                   ; null-handler check
    ///   0x2C: BEQ   0x38                     ; skip call if no handler
    ///   0x30: MOV   LR, PC                   ; LR = 0x38 (return address)
    ///   0x34: BX    R0                        ; call handler (ARM or Thumb)
    ///   0x38: LDMFD SP!, {R0-R3, R12, LR}    ; restore saved registers
    ///   0x3C: SUBS  PC, LR, #4               ; exception return
    fn init_bios_stub(&mut self) {
        // Each instruction stored little-endian
        let stub: &[(usize, u32)] = &[
            (0x00, 0xE12FFF1E), // BX LR  (reset/null-fn: return to caller)
            (0x04, 0xE12FFF1E), // BX LR  (undefined instruction stub)
            (0x08, 0xE12FFF1E), // BX LR  (SWI stub — handled by HLE, never reached)
            (0x0C, 0xE12FFF1E), // BX LR  (prefetch abort stub)
            (0x10, 0xE12FFF1E), // BX LR  (data abort stub)
            (0x14, 0xE12FFF1E), // BX LR  (reserved stub)
            (0x18, 0xE92D500F), // STMFD SP!, {R0-R3, R12, LR}
            (0x1C, 0xE3A00403), // MOV R0, #0x03000000
            (0x20, 0xE2800C7F), // ADD R0, R0, #0x7F00
            (0x24, 0xE59000FC), // LDR R0, [R0, #0xFC]
            (0x28, 0xE3500000), // CMP R0, #0
            (0x2C, 0x0A000001), // BEQ 0x38
            (0x30, 0xE1A0E00F), // MOV LR, PC  (LR = 0x38)
            (0x34, 0xE12FFF10), // BX R0
            (0x38, 0xE8BD500F), // LDMFD SP!, {R0-R3, R12, LR}
            (0x3C, 0xE25EF004), // SUBS PC, LR, #4
        ];
        for &(addr, word) in stub {
            let bytes = word.to_le_bytes();
            self.bios[addr..addr + 4].copy_from_slice(&bytes);
        }
    }

    /// Load ROM data into the bus
    pub fn load_rom(&mut self, rom: Vec<u8>) {
        self.rom = rom;
        self.detect_save_type();
        self.reset_save_state();
    }

    fn detect_save_type(&mut self) {
        let save_type = if self.rom.windows(b"FLASH1M_V".len()).any(|w| w == b"FLASH1M_V") {
            SaveType::Flash128K
        } else if self.rom.windows(b"FLASH_V".len()).any(|w| w == b"FLASH_V")
            || self
                .rom
                .windows(b"FLASH512_V".len())
                .any(|w| w == b"FLASH512_V")
        {
            SaveType::Flash64K
        } else if self.rom.windows(b"SRAM_V".len()).any(|w| w == b"SRAM_V") {
            SaveType::Sram
        } else if self.rom.windows(b"EEPROM_V".len()).any(|w| w == b"EEPROM_V") {
            SaveType::Eeprom
        } else {
            SaveType::None
        };

        self.save_type = save_type;
        let size = match save_type {
            SaveType::None => REGION_SRAM_SIZE,
            SaveType::Sram | SaveType::Flash64K | SaveType::Eeprom => REGION_SRAM_SIZE,
            SaveType::Flash128K => REGION_SRAM_SIZE * 2,
        };
        self.save_memory = vec![0xFF; size];
    }

    fn reset_save_state(&mut self) {
        self.flash_id_mode = false;
        self.flash_cmd_state = FlashCommandState::Ready;
        self.flash_bank = 0;
        self.save_dirty = false;
    }

    fn flash_bank_offset(&self, masked_addr: u32) -> usize {
        let bank = match self.save_type {
            SaveType::Flash128K => self.flash_bank,
            _ => 0,
        };
        bank * REGION_SRAM_SIZE + masked_addr as usize
    }

    fn erase_flash_sector(&mut self, masked_addr: u32) {
        let sector_base = (masked_addr as usize) & !0x0FFF;
        let bank_base = match self.save_type {
            SaveType::Flash128K => self.flash_bank * REGION_SRAM_SIZE,
            _ => 0,
        };
        let start = bank_base + sector_base;
        let end = (start + 0x1000).min(self.save_memory.len());
        self.save_memory[start..end].fill(0xFF);
        self.save_dirty = true;
    }

    /// Return whether the current cartridge has persistent save storage.
    pub fn has_persistent_save(&self) -> bool {
        !matches!(self.save_type, SaveType::None | SaveType::Eeprom)
    }

    /// Return a snapshot of the cartridge save memory for persistence.
    pub fn save_data(&self) -> Option<&[u8]> {
        self.has_persistent_save().then_some(self.save_memory.as_slice())
    }

    /// Replace the cartridge save memory from persisted data.
    pub fn load_save_data(&mut self, data: &[u8]) {
        if !self.has_persistent_save() {
            return;
        }

        self.save_memory.fill(0xFF);
        let len = data.len().min(self.save_memory.len());
        self.save_memory[..len].copy_from_slice(&data[..len]);
        self.save_dirty = false;
        self.flash_id_mode = false;
        self.flash_cmd_state = FlashCommandState::Ready;
        self.flash_bank = 0;
    }

    /// Return whether the cartridge save memory changed since the last flush.
    pub fn take_save_dirty(&mut self) -> bool {
        let dirty = self.save_dirty;
        self.save_dirty = false;
        dirty
    }

    /// Load BIOS data (for testing/development)
    pub fn load_bios(&mut self, bios: &[u8]) {
        let len = bios.len().min(REGION_BIOS_SIZE);
        self.bios[..len].copy_from_slice(&bios[..len]);
    }

    /// Enable BIOS protection (called after boot)
    pub fn protect_bios(&mut self) {
        self.bios_protected = true;
    }

    /// Get the framebuffer as RGBA pixels (240x160x4 bytes)
    /// Reads from VRAM as if Mode 3 (16-bit bitmap at 0x06000000)
    pub fn get_framebuffer_rgba(&self) -> Vec<u8> {
        let width = 240usize;
        let height = 160usize;
        let mut rgba = vec![0u8; width * height * 4];

        for y in 0..height {
            for x in 0..width {
                let vram_offset = (y * width + x) * 2;
                if vram_offset + 1 < self.vram.len() {
                    let color = (self.vram[vram_offset] as u16)
                        | ((self.vram[vram_offset + 1] as u16) << 8);
                    let r = (color & 0x1F) as u8;
                    let g = ((color >> 5) & 0x1F) as u8;
                    let b = ((color >> 10) & 0x1F) as u8;
                    let pixel_offset = (y * width + x) * 4;
                    rgba[pixel_offset] = (r as u16 * 255 / 31) as u8;
                    rgba[pixel_offset + 1] = (g as u16 * 255 / 31) as u8;
                    rgba[pixel_offset + 2] = (b as u16 * 255 / 31) as u8;
                    rgba[pixel_offset + 3] = 0xFF;
                }
            }
        }
        rgba
    }

    /// Get the open bus value (last read value)
    pub fn get_open_bus_value(&self) -> u32 {
        self.open_bus_value
    }

    /// Get VRAM as slice (for PPU synchronization)
    pub fn get_vram_slice(&self) -> &[u8] {
        self.vram.as_ref()
    }

    /// Get palette as slice (for PPU synchronization)
    pub fn get_palette_slice(&self) -> &[u8] {
        self.palette.as_ref()
    }

    /// Get OAM as slice (for PPU synchronization)
    pub fn get_oam_slice(&self) -> &[u8] {
        self.oam.as_ref()
    }

    /// Return and clear the dirty byte ranges for video memory mirrors.
    pub fn take_video_dirty_ranges(
        &mut self,
    ) -> (
        Option<(usize, usize)>,
        Option<(usize, usize)>,
        Option<(usize, usize)>,
    ) {
        (
            self.palette_dirty.take(),
            self.vram_dirty.take(),
            self.oam_dirty.take(),
        )
    }

    /// Return and clear the dirty byte range for sound I/O mirrors.
    pub fn take_audio_io_dirty_range(&mut self) -> Option<(usize, usize)> {
        self.audio_io_dirty.take()
    }

    /// Take and clear the DMA-dirty flag. Returns true if any DMA register was written.
    pub fn take_dma_dirty(&mut self) -> bool {
        let dirty = self.dma_dirty;
        self.dma_dirty = false;
        dirty
    }

    #[inline(always)]
    fn mark_dma_dirty(&mut self, offset: usize) {
        // DMA registers occupy I/O offsets 0xB0..=0xDF (all four channels)
        if offset >= 0xB0 && offset <= 0xDF {
            self.dma_dirty = true;
        }
    }

    fn mark_audio_io_dirty(&mut self, offset: usize, len: usize) {
        let start = offset.max(AUDIO_IO_START);
        let end = offset.saturating_add(len).min(AUDIO_IO_END);
        if start < end {
            self.audio_io_dirty
                .mark(start - AUDIO_IO_START, end - start, AUDIO_IO_LEN);
        }
    }

    /// Write directly to I/O register array (for internal sync, bypasses side effects)
    pub fn write_io_direct(&mut self, offset: usize, value: u8) {
        if offset < self.io.len() {
            self.io[offset] = value;
            self.mark_audio_io_dirty(offset, 1);
            self.mark_dma_dirty(offset);
        }
    }

    /// Read I/O register directly as u8 (for internal checks)
    pub fn read_io_direct(&self, offset: usize) -> u8 {
        if offset < self.io.len() {
            self.io[offset]
        } else {
            0
        }
    }

    /// Read I/O register directly as u16 (for internal sync)
    pub fn read_io_direct_u16(&self, offset: usize) -> u16 {
        if offset + 1 < self.io.len() {
            u16::from_le_bytes([self.io[offset], self.io[offset + 1]])
        } else {
            0
        }
    }

    /// Write I/O register directly as u16 (for internal sync)
    pub fn write_io_direct_u16(&mut self, offset: usize, value: u16) {
        let bytes = value.to_le_bytes();
        if offset + 1 < self.io.len() {
            self.io[offset] = bytes[0];
            self.io[offset + 1] = bytes[1];
            self.mark_audio_io_dirty(offset, 2);
            self.mark_dma_dirty(offset);
        }
    }

    /// Update VCOUNT based on cycles
    pub fn update_vcount(&mut self, cycles: u64) {
        self.vcount_cycles = cycles;
        // GBA has 228 scanlines per frame (160 visible + 68 VBlank)
        // Each scanline is 1232 cycles
        let scanline = ((cycles / 1232) % 228) as u16;
        self.vcount = scanline;

        // Update DISPSTAT mirror if needed
        self.update_dispstat();
    }

    /// Update DISPSTAT register with VCOUNT match
    fn update_dispstat(&mut self) {
        // DISPSTAT at 0x04000004
        // VCOUNT at 0x04000006
        let vcount_trigger = self.read_u16(0x04000004) >> 8;
        if self.vcount == vcount_trigger {
            // Set V-Counter flag in DISPSTAT
            let dispstat = self.read_u16(0x04000004);
            self.write_u16(0x04000004, dispstat | 0x0004);
        }
    }

    /// Get current VCOUNT
    pub fn get_vcount(&self) -> u16 {
        self.vcount
    }

    /// Mask address for mirroring regions
    fn mask_address(&self, addr: u32) -> u32 {
        match addr {
            // BIOS mirrors every 32KB in first 4MB (0x00000000-0x00003FFF)
            0x0000_0000..=0x0000_3FFF => addr & 0x3FFF,
            // EWRAM mirrors every 256KB in 0x02000000-0x02FFFFFF
            0x0200_0000..=0x02FF_FFFF => addr & 0x3FFFF,
            // IWRAM mirrors every 32KB in 0x03000000-0x03FFFFFF
            0x0300_0000..=0x03FF_FFFF => addr & 0x7FFF,
            // I/O mirrors every 1KB in 0x04000000-0x04FFFFFF
            0x0400_0000..=0x04FF_FFFF => addr & 0x3FF,
            // Palette mirrors every 1KB in 0x05000000-0x05FF_FFFF
            0x0500_0000..=0x05FF_FFFF => addr & 0x3FF,
            // VRAM repeats every 128KB, with the top 32KB aliasing 0x10000-0x17FFF.
            0x0600_0000..=0x06FF_FFFF => Self::vram_index(addr),
            // OAM mirrors every 1KB in 0x07000000-0x07FF_FFFF
            0x0700_0000..=0x07FF_FFFF => addr & 0x3FF,
            // ROM mirrors in 0x08000000-0x0DFFFFFF
            0x0800_0000..=0x0DFF_FFFF => addr & 0x1FFFFFF,
            // Game Pak SRAM/Flash window mirrors every 64KB.
            0x0E00_0000..=0x0E00_FFFF => addr & 0xFFFF,
            _ => addr,
        }
    }

    /// Get ROM index with mirroring
    fn rom_index(&self, addr: u32) -> Option<usize> {
        if self.rom.is_empty() {
            return None;
        }

        let masked = addr & 0x1FFFFFF;
        let rom_size = self.rom.len() as u32;

        // Mirror ROM if it's smaller than the maximum
        if rom_size > 0 {
            Some((masked % rom_size) as usize)
        } else {
            None
        }
    }

    fn flash_read_u8(&self, masked_addr: u32) -> u8 {
        if self.flash_id_mode {
            return match (self.save_type, masked_addr) {
                // Match common GBA save-flash identities used by existing emulators.
                (SaveType::Flash128K, 0x0000) => 0x62, // Sanyo manufacturer ID
                (SaveType::Flash128K, 0x0001) => 0x13, // 1M flash device ID
                (SaveType::Flash64K, 0x0000) => 0x32,  // Panasonic manufacturer ID
                (SaveType::Flash64K, 0x0001) => 0x1B,  // 512K flash device ID
                (_, _) => 0xFF,
            };
        }

        self.save_memory[self.flash_bank_offset(masked_addr)]
    }

    fn flash_write_u8(&mut self, masked_addr: u32, value: u8) {
        match self.save_type {
            SaveType::Sram => {
                self.save_memory[masked_addr as usize] = value;
                self.save_dirty = true;
            }
            SaveType::Flash64K | SaveType::Flash128K => {
                // 0xF0 is a soft-reset command, but only when the chip is NOT actively
                // waiting for a data byte to program.  If we honour it unconditionally,
                // any save-data byte that happens to equal 0xF0 cancels the program
                // sequence instead of being written — the game then fails its verify
                // pass and reports a save error.
                if value == 0xF0
                    && !matches!(self.flash_cmd_state, FlashCommandState::Program)
                {
                    self.flash_id_mode = false;
                    self.flash_cmd_state = FlashCommandState::Ready;
                    return;
                }

                match self.flash_cmd_state {
                    FlashCommandState::Ready => {
                        if masked_addr == 0x5555 && value == 0xAA {
                            self.flash_cmd_state = FlashCommandState::Unlock1;
                        }
                    }
                    FlashCommandState::Unlock1 => {
                        if masked_addr == 0x2AAA && value == 0x55 {
                            self.flash_cmd_state = FlashCommandState::Unlock2;
                        } else {
                            self.flash_cmd_state = FlashCommandState::Ready;
                        }
                    }
                    FlashCommandState::Unlock2 => {
                        self.flash_cmd_state = FlashCommandState::Ready;
                        if masked_addr != 0x5555 {
                            return;
                        }
                        match value {
                            0x90 => self.flash_id_mode = true,
                            0xA0 => self.flash_cmd_state = FlashCommandState::Program,
                            0x80 => self.flash_cmd_state = FlashCommandState::EraseUnlock1,
                            0xB0 if matches!(self.save_type, SaveType::Flash128K) => {
                                self.flash_cmd_state = FlashCommandState::BankSwitch;
                            }
                            _ => {}
                        }
                    }
                    FlashCommandState::Program => {
                        let idx = self.flash_bank_offset(masked_addr);
                        self.save_memory[idx] = value;
                        self.save_dirty = true;
                        self.flash_cmd_state = FlashCommandState::Ready;
                    }
                    FlashCommandState::BankSwitch => {
                        if masked_addr == 0x0000 {
                            self.flash_bank = (value as usize) & 1;
                        }
                        self.flash_cmd_state = FlashCommandState::Ready;
                    }
                    FlashCommandState::EraseUnlock1 => {
                        if masked_addr == 0x5555 && value == 0xAA {
                            self.flash_cmd_state = FlashCommandState::EraseUnlock2;
                        } else {
                            self.flash_cmd_state = FlashCommandState::Ready;
                        }
                    }
                    FlashCommandState::EraseUnlock2 => {
                        if masked_addr == 0x2AAA && value == 0x55 {
                            self.flash_cmd_state = FlashCommandState::EraseCommand;
                        } else {
                            self.flash_cmd_state = FlashCommandState::Ready;
                        }
                    }
                    FlashCommandState::EraseCommand => {
                        if masked_addr == 0x5555 && value == 0x10 {
                            self.save_memory.fill(0xFF);
                            self.save_dirty = true;
                        } else if value == 0x30 {
                            self.erase_flash_sector(masked_addr);
                        }
                        self.flash_cmd_state = FlashCommandState::Ready;
                    }
                }
            }
            SaveType::None | SaveType::Eeprom => {}
        }
    }

    fn log_watch_if_hit(&self, addr: u32, len: u32, value: u32, kind: &str) {
        let debug = debug_config();
        let Some(watch) = debug.watch_addr else {
            return;
        };
        if !(addr <= watch && watch < addr.saturating_add(len)) {
            return;
        }

        let cy = DEBUG_CYCLE.with(|c| c.get());
        if cy < debug.watch_after {
            return;
        }
        let pc = DEBUG_PC.with(|c| c.get());
        eprintln!(
            "WATCH {} write: [{:08x}] = {:0width$x} at cy={} pc={:08x}",
            kind,
            addr,
            value,
            cy,
            pc,
            width = (len as usize) * 2
        );
    }

    #[inline(always)]
    fn waitcnt(&self) -> u16 {
        self.read_io_direct_u16(0x204)
    }

    #[inline(always)]
    fn access_region(addr: u32) -> AccessRegion {
        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => AccessRegion::Bios,
            REGION_EWRAM_START..=REGION_EWRAM_END => AccessRegion::Ewram,
            REGION_IWRAM_START..=REGION_IWRAM_END => AccessRegion::Iwram,
            REGION_IO_START..=REGION_IO_END => AccessRegion::Io,
            REGION_PALETTE_START..=REGION_PALETTE_END => AccessRegion::Palette,
            REGION_VRAM_START..=REGION_VRAM_END => AccessRegion::Vram,
            REGION_OAM_START..=REGION_OAM_END => AccessRegion::Oam,
            0x0800_0000..=0x09FF_FFFF => AccessRegion::Rom(0),
            0x0A00_0000..=0x0BFF_FFFF => AccessRegion::Rom(1),
            0x0C00_0000..=0x0DFF_FFFF => AccessRegion::Rom(2),
            REGION_SRAM_START..=REGION_SRAM_END => AccessRegion::Sram,
            _ => AccessRegion::Other,
        }
    }

    #[inline(always)]
    fn next_sequential_addr(addr: u32, width: AccessWidth, region: AccessRegion) -> u32 {
        match region {
            AccessRegion::Rom(_) => match width {
                AccessWidth::Word => (addr & !3).wrapping_add(4),
                _ => (addr & !1).wrapping_add(2),
            },
            AccessRegion::Sram => addr.wrapping_add(1),
            _ => match width {
                AccessWidth::Word => (addr & !3).wrapping_add(4),
                AccessWidth::Half => (addr & !1).wrapping_add(2),
                AccessWidth::Byte => addr.wrapping_add(1),
            },
        }
    }

    #[inline(always)]
    fn gamepak_wait_cycles(&self, area: usize, sequential: bool) -> u32 {
        const FIRST_ACCESS_WAITS: [u32; 4] = [4, 3, 2, 8];
        const SECOND_ACCESS_WAITS: [[u32; 2]; 3] = [[2, 1], [4, 1], [8, 1]];

        let waitcnt = self.waitcnt();
        if sequential {
            let idx = match area {
                0 => ((waitcnt >> 4) & 0x1) as usize,
                1 => ((waitcnt >> 7) & 0x1) as usize,
                _ => ((waitcnt >> 10) & 0x1) as usize,
            };
            1 + SECOND_ACCESS_WAITS[area][idx]
        } else {
            let idx = match area {
                0 => ((waitcnt >> 2) & 0x3) as usize,
                1 => ((waitcnt >> 5) & 0x3) as usize,
                _ => ((waitcnt >> 8) & 0x3) as usize,
            };
            1 + FIRST_ACCESS_WAITS[idx]
        }
    }

    #[inline(always)]
    fn sram_wait_cycles(&self) -> u32 {
        const SRAM_WAITS: [u32; 4] = [4, 3, 2, 8];
        1 + SRAM_WAITS[(self.waitcnt() & 0x3) as usize]
    }

    #[inline(always)]
    fn prefetch_enabled(&self) -> bool {
        (self.waitcnt() & (1 << 14)) != 0
    }

    #[inline(always)]
    fn record_cpu_access(&self, addr: u32, width: AccessWidth, kind: AccessKind) {
        if !self.cpu_timing_active.get() {
            return;
        }

        let region = Self::access_region(addr);
        let sequential = if let Some(last) = self.cpu_last_access.get() {
            last.region == region
                && last.next_addr == addr
                && match region {
                    AccessRegion::Rom(_) => (addr & 0x1_FFFF) != 0,
                    _ => true,
                }
        } else {
            false
        };

        let cycles = match region {
            AccessRegion::Bios | AccessRegion::Iwram | AccessRegion::Io | AccessRegion::Other => 1,
            AccessRegion::Ewram => match width {
                AccessWidth::Word => 6,
                _ => 3,
            },
            AccessRegion::Palette | AccessRegion::Vram | AccessRegion::Oam => match width {
                AccessWidth::Word => 2,
                _ => 1,
            },
            AccessRegion::Rom(area) => {
                let prefetch_hit = kind == AccessKind::Opcode && self.prefetch_enabled() && sequential;
                match width {
                    AccessWidth::Word => {
                        let first = if prefetch_hit { 1 } else { self.gamepak_wait_cycles(area, sequential) };
                        first + if prefetch_hit { 1 } else { self.gamepak_wait_cycles(area, true) }
                    }
                    AccessWidth::Half | AccessWidth::Byte => {
                        if prefetch_hit {
                            1
                        } else {
                            self.gamepak_wait_cycles(area, sequential)
                        }
                    }
                }
            }
            AccessRegion::Sram => match width {
                AccessWidth::Byte => self.sram_wait_cycles(),
                AccessWidth::Half => self.sram_wait_cycles() * 2,
                AccessWidth::Word => self.sram_wait_cycles() * 4,
            },
        };

        self.cpu_timing_cycles
            .set(self.cpu_timing_cycles.get().saturating_add(cycles));
        self.cpu_last_access
            .set(Some(AccessStamp {
                region,
                next_addr: Self::next_sequential_addr(addr, width, region),
            }));
    }
}

impl Bus for SimpleBus {
    fn begin_instruction_timing(&mut self) {
        self.cpu_timing_active.set(true);
        self.cpu_timing_cycles.set(0);
        self.cpu_last_access.set(None);
    }

    fn finish_instruction_timing(&mut self) -> u32 {
        self.cpu_timing_active.set(false);
        self.cpu_last_access.set(None);
        self.cpu_timing_cycles.get().max(1)
    }

    fn read_u8(&self, addr: u32) -> u8 {
        self.record_cpu_access(addr, AccessWidth::Byte, AccessKind::Data);
        let masked_addr = self.mask_address(addr);

        let value = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => self.bios[masked_addr as usize],
            REGION_EWRAM_START..=REGION_EWRAM_END => self.ewram[masked_addr as usize],
            REGION_IWRAM_START..=REGION_IWRAM_END => self.iwram[masked_addr as usize],
            REGION_IO_START..=REGION_IO_END => self.io[masked_addr as usize],
            REGION_PALETTE_START..=REGION_PALETTE_END => self.palette[masked_addr as usize],
            REGION_VRAM_START..=REGION_VRAM_END => self.vram[masked_addr as usize],
            REGION_OAM_START..=REGION_OAM_END => self.oam[masked_addr as usize],
            REGION_ROM_START..=REGION_ROM_END => {
                if let Some(idx) = self.rom_index(addr) {
                    self.rom[idx]
                } else {
                    // Open bus - return last read value (upper byte)
                    ((self.open_bus_value >> 8) & 0xFF) as u8
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => self.flash_read_u8(masked_addr),
            _ => {
                // Unmapped - open bus behavior
                ((self.open_bus_value >> 8) & 0xFF) as u8
            }
        };

        value
    }

    fn read_u16(&self, addr: u32) -> u16 {
        self.record_cpu_access(addr & !1, AccessWidth::Half, AccessKind::Data);
        let masked_addr = self.mask_address(addr);

        let value = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.bios[idx], self.bios[idx + 1]])
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.ewram[idx], self.ewram[idx + 1]])
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.iwram[idx], self.iwram[idx + 1]])
            }
            REGION_IO_START..=REGION_IO_END => {
                // Handle special I/O registers
                match masked_addr {
                    0x0004 => {
                        // DISPSTAT (bits 0-2 are runtime status flags)
                        // Keep this sourced from the live I/O mirror, which gba.rs updates.
                        u16::from_le_bytes([self.io[4], self.io[5]])
                    }
                    0x0006 => {
                        // VCOUNT (live I/O mirror written by gba.rs)
                        u16::from_le_bytes([self.io[6], self.io[7]])
                    }
                    _ => {
                        let idx = masked_addr as usize & !1;
                        u16::from_le_bytes([self.io[idx], self.io[idx + 1]])
                    }
                }
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.palette[idx], self.palette[idx + 1]])
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.vram[idx], self.vram[idx + 1]])
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.oam[idx], self.oam[idx + 1]])
            }
            REGION_ROM_START..=REGION_ROM_END => {
                if let Some(idx) = self.rom_index(addr) {
                    if idx + 1 < self.rom.len() {
                        u16::from_le_bytes([self.rom[idx], self.rom[idx + 1]])
                    } else {
                        self.rom[idx] as u16
                    }
                } else {
                    // Open bus
                    (self.open_bus_value & 0xFFFF) as u16
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                let idx = masked_addr & !1;
                u16::from_le_bytes([
                    self.flash_read_u8(idx),
                    self.flash_read_u8((idx + 1) & 0xFFFF),
                ])
            }
            _ => {
                // Unmapped - open bus behavior
                (self.open_bus_value & 0xFFFF) as u16
            }
        };

        value
    }

    fn read_u32(&self, addr: u32) -> u32 {
        self.record_cpu_access(addr & !3, AccessWidth::Word, AccessKind::Data);
        let masked_addr = self.mask_address(addr);

        let value = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.bios[idx],
                    self.bios[idx + 1],
                    self.bios[idx + 2],
                    self.bios[idx + 3],
                ])
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.ewram[idx],
                    self.ewram[idx + 1],
                    self.ewram[idx + 2],
                    self.ewram[idx + 3],
                ])
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.iwram[idx],
                    self.iwram[idx + 1],
                    self.iwram[idx + 2],
                    self.iwram[idx + 3],
                ])
            }
            REGION_IO_START..=REGION_IO_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.io[idx],
                    self.io[idx + 1],
                    self.io[idx + 2],
                    self.io[idx + 3],
                ])
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.palette[idx],
                    self.palette[idx + 1],
                    self.palette[idx + 2],
                    self.palette[idx + 3],
                ])
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.vram[idx],
                    self.vram[idx + 1],
                    self.vram[idx + 2],
                    self.vram[idx + 3],
                ])
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.oam[idx],
                    self.oam[idx + 1],
                    self.oam[idx + 2],
                    self.oam[idx + 3],
                ])
            }
            REGION_ROM_START..=REGION_ROM_END => {
                if let Some(idx) = self.rom_index(addr) {
                    if idx + 3 < self.rom.len() {
                        u32::from_le_bytes([
                            self.rom[idx],
                            self.rom[idx + 1],
                            self.rom[idx + 2],
                            self.rom[idx + 3],
                        ])
                    } else {
                        // Partial read from end of ROM
                        let mut bytes = [0u8; 4];
                        for i in 0..4 {
                            if idx + i < self.rom.len() {
                                bytes[i] = self.rom[idx + i];
                            }
                        }
                        u32::from_le_bytes(bytes)
                    }
                } else {
                    // Open bus
                    self.open_bus_value
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                let idx = masked_addr & !3;
                u32::from_le_bytes([
                    self.flash_read_u8(idx),
                    self.flash_read_u8((idx + 1) & 0xFFFF),
                    self.flash_read_u8((idx + 2) & 0xFFFF),
                    self.flash_read_u8((idx + 3) & 0xFFFF),
                ])
            }
            _ => {
                // Unmapped - open bus behavior
                self.open_bus_value
            }
        };

        value
    }

    fn read_opcode_u16(&self, addr: u32) -> u16 {
        self.record_cpu_access(addr & !1, AccessWidth::Half, AccessKind::Opcode);
        let masked_addr = self.mask_address(addr);

        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.bios[idx], self.bios[idx + 1]])
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.ewram[idx], self.ewram[idx + 1]])
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.iwram[idx], self.iwram[idx + 1]])
            }
            REGION_IO_START..=REGION_IO_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.io[idx], self.io[idx + 1]])
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.palette[idx], self.palette[idx + 1]])
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.vram[idx], self.vram[idx + 1]])
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize & !1;
                u16::from_le_bytes([self.oam[idx], self.oam[idx + 1]])
            }
            REGION_ROM_START..=REGION_ROM_END => {
                if let Some(idx) = self.rom_index(addr) {
                    if idx + 1 < self.rom.len() {
                        u16::from_le_bytes([self.rom[idx], self.rom[idx + 1]])
                    } else {
                        self.rom[idx] as u16
                    }
                } else {
                    (self.open_bus_value & 0xFFFF) as u16
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                let idx = masked_addr & !1;
                u16::from_le_bytes([
                    self.flash_read_u8(idx),
                    self.flash_read_u8((idx + 1) & 0xFFFF),
                ])
            }
            _ => (self.open_bus_value & 0xFFFF) as u16,
        }
    }

    fn read_opcode_u32(&self, addr: u32) -> u32 {
        self.record_cpu_access(addr & !3, AccessWidth::Word, AccessKind::Opcode);
        let masked_addr = self.mask_address(addr);

        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.bios[idx],
                    self.bios[idx + 1],
                    self.bios[idx + 2],
                    self.bios[idx + 3],
                ])
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.ewram[idx],
                    self.ewram[idx + 1],
                    self.ewram[idx + 2],
                    self.ewram[idx + 3],
                ])
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.iwram[idx],
                    self.iwram[idx + 1],
                    self.iwram[idx + 2],
                    self.iwram[idx + 3],
                ])
            }
            REGION_IO_START..=REGION_IO_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.io[idx],
                    self.io[idx + 1],
                    self.io[idx + 2],
                    self.io[idx + 3],
                ])
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.palette[idx],
                    self.palette[idx + 1],
                    self.palette[idx + 2],
                    self.palette[idx + 3],
                ])
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.vram[idx],
                    self.vram[idx + 1],
                    self.vram[idx + 2],
                    self.vram[idx + 3],
                ])
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize & !3;
                u32::from_le_bytes([
                    self.oam[idx],
                    self.oam[idx + 1],
                    self.oam[idx + 2],
                    self.oam[idx + 3],
                ])
            }
            REGION_ROM_START..=REGION_ROM_END => {
                if let Some(idx) = self.rom_index(addr) {
                    if idx + 3 < self.rom.len() {
                        u32::from_le_bytes([
                            self.rom[idx],
                            self.rom[idx + 1],
                            self.rom[idx + 2],
                            self.rom[idx + 3],
                        ])
                    } else {
                        let mut bytes = [0u8; 4];
                        for i in 0..4 {
                            if idx + i < self.rom.len() {
                                bytes[i] = self.rom[idx + i];
                            }
                        }
                        u32::from_le_bytes(bytes)
                    }
                } else {
                    self.open_bus_value
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                let idx = masked_addr & !3;
                u32::from_le_bytes([
                    self.flash_read_u8(idx),
                    self.flash_read_u8((idx + 1) & 0xFFFF),
                    self.flash_read_u8((idx + 2) & 0xFFFF),
                    self.flash_read_u8((idx + 3) & 0xFFFF),
                ])
            }
            _ => self.open_bus_value,
        }
    }

    fn write_u8(&mut self, addr: u32, value: u8) {
        self.record_cpu_access(addr, AccessWidth::Byte, AccessKind::Data);
        let masked_addr = self.mask_address(addr);

        // Debug logging for PPU register writes
        if debug_config().ppu_debug && addr >= 0x04000000 && addr < 0x04000060 {
            println!("PPU write: {:08x} = {:02x}", addr, value);
        }

        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                // BIOS is read-only (ignore writes)
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                self.ewram[masked_addr as usize] = value;
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                self.iwram[masked_addr as usize] = value;
            }
            REGION_IO_START..=REGION_IO_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                match masked_addr {
                    // DISPSTAT low byte: bits 0-2 are read-only status flags
                    0x0004 => {
                        let current = self.io[0x0004] & 0x07;
                        self.io[0x0004] = current | (value & 0xF8);
                    }
                    // IF register bytes (0x0202 low, 0x0203 high) use write-to-clear
                    0x0202 | 0x0203 => {
                        let current = self.io[masked_addr as usize];
                        self.io[masked_addr as usize] = current & !value;
                    }
                    _ => {
                        self.io[masked_addr as usize] = value;
                    }
                }
                self.mark_audio_io_dirty(masked_addr as usize, 1);
                self.mark_dma_dirty(masked_addr as usize);
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                self.palette[masked_addr as usize] = value;
                self.palette_dirty
                    .mark(masked_addr as usize, 1, REGION_PALETTE_SIZE);
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                self.vram[masked_addr as usize] = value;
                self.vram_dirty
                    .mark(masked_addr as usize, 1, REGION_VRAM_SIZE);
            }
            REGION_OAM_START..=REGION_OAM_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                self.oam[masked_addr as usize] = value;
                self.oam_dirty
                    .mark(masked_addr as usize, 1, REGION_OAM_SIZE);
            }
            REGION_ROM_START..=REGION_ROM_END => {
                // ROM writes are ignored (could be flash/SRAM save)
            }
            REGION_SRAM_START..=REGION_SRAM_END => self.flash_write_u8(masked_addr, value),
            _ => {
                // Unmapped - ignore
            }
        }
    }

    fn write_u16(&mut self, addr: u32, value: u16) {
        self.record_cpu_access(addr & !1, AccessWidth::Half, AccessKind::Data);
        let masked_addr = self.mask_address(addr);
        let bytes = value.to_le_bytes();

        // Trace DMA register writes when TINYBIRD_DMA_DEBUG is set
        if debug_config().dma_debug
            && addr >= 0x040000B0
            && addr <= 0x040000DE
        {
            let names = ["SAD_L","SAD_H","DAD_L","DAD_H","CNT_L","CNT_H"];
            let ch = ((addr - 0x040000B0) / 12) as usize;
            let reg = (((addr - 0x040000B0) % 12) / 2) as usize;
            let name = if ch < 4 && reg < 6 { names[reg] } else { "???" };
            eprintln!("DMA{} {} write: addr={:08x} val={:04x}", ch, name, addr, value);
        }

        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                // BIOS is read-only
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                self.ewram[idx] = bytes[0];
                self.ewram[idx + 1] = bytes[1];
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                self.iwram[idx] = bytes[0];
                self.iwram[idx + 1] = bytes[1];
            }
            REGION_IO_START..=REGION_IO_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                // IF register (0x04000202) uses write-to-clear: writing 1 clears that bit
                if masked_addr == 0x0202 {
                    let written = u16::from_le_bytes([bytes[0], bytes[1]]);
                    let current = u16::from_le_bytes([self.io[0x202], self.io[0x203]]);
                    let new_val = current & !written;
                    self.io[0x202] = (new_val & 0xFF) as u8;
                    self.io[0x203] = ((new_val >> 8) & 0xFF) as u8;
                } else if masked_addr == 0x0004 {
                    // DISPSTAT: bits 0-2 are read-only status flags.
                    let current = u16::from_le_bytes([self.io[0x0004], self.io[0x0005]]);
                    let preserved_status = current & 0x0007;
                    let new_dispstat = preserved_status | (value & 0xFFF8);
                    let new_bytes = new_dispstat.to_le_bytes();
                    self.io[0x0004] = new_bytes[0];
                    self.io[0x0005] = new_bytes[1];
                } else {
                    self.io[idx] = bytes[0];
                    self.io[idx + 1] = bytes[1];
                }
                self.mark_audio_io_dirty(idx, 2);
                self.mark_dma_dirty(idx);
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                self.palette[idx] = bytes[0];
                self.palette[idx + 1] = bytes[1];
                self.palette_dirty.mark(idx, 2, REGION_PALETTE_SIZE);
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                self.vram[idx] = bytes[0];
                self.vram[idx + 1] = bytes[1];
                self.vram_dirty.mark(idx, 2, REGION_VRAM_SIZE);
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 2, value as u32, "u16");
                self.oam[idx] = bytes[0];
                self.oam[idx + 1] = bytes[1];
                self.oam_dirty.mark(idx, 2, REGION_OAM_SIZE);
            }
            REGION_ROM_START..=REGION_ROM_END => {
                // ROM writes are ignored
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                self.flash_write_u8(masked_addr, bytes[0]);
                self.flash_write_u8((masked_addr + 1) & 0xFFFF, bytes[1]);
            }
            _ => {
                // Unmapped - ignore
            }
        }
    }

    fn write_u32(&mut self, addr: u32, value: u32) {
        self.record_cpu_access(addr & !3, AccessWidth::Word, AccessKind::Data);
        let masked_addr = self.mask_address(addr);
        let bytes = value.to_le_bytes();

        // Trace DMA register writes when TINYBIRD_DMA_DEBUG is set
        if debug_config().dma_debug
            && addr >= 0x040000B0
            && addr <= 0x040000DE
        {
            eprintln!("DMA u32 write: addr={:08x} val={:08x}", addr, value);
        }

        match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                // BIOS is read-only
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                self.ewram[idx..idx + 4].copy_from_slice(&bytes);
            }
            REGION_IWRAM_START..=REGION_IWRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                self.iwram[idx..idx + 4].copy_from_slice(&bytes);
            }
            REGION_IO_START..=REGION_IO_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                // IF register (offset 0x202) is write-to-clear; handle it specially
                // whether it appears as the low 2 bytes (write at 0x202) or
                // high 2 bytes (write at 0x200 covering IE+IF together).
                if idx == 0x202 {
                    // Write at IF directly
                    let written = u32::from_le_bytes(bytes);
                    let current = u32::from_le_bytes([
                        self.io[0x202], self.io[0x203], self.io[0x204], self.io[0x205],
                    ]);
                    let if_cleared = (current & 0x0000_FFFF) & !(written & 0x0000_FFFF);
                    let rest = (current & 0xFFFF_0000) & !(written & 0xFFFF_0000);
                    let new_val = if_cleared | rest;
                    let new_bytes = new_val.to_le_bytes();
                    self.io[0x202..0x206].copy_from_slice(&new_bytes);
                } else if idx == 0x200 {
                    // Write at IE (low 16) + IF (high 16)
                    self.io[0x200] = bytes[0];
                    self.io[0x201] = bytes[1];
                    let if_written = u16::from_le_bytes([bytes[2], bytes[3]]);
                    let current_if = u16::from_le_bytes([self.io[0x202], self.io[0x203]]);
                    let new_if = current_if & !if_written;
                    self.io[0x202] = (new_if & 0xFF) as u8;
                    self.io[0x203] = ((new_if >> 8) & 0xFF) as u8;
                } else if idx == 0x004 {
                    // DISPSTAT (bytes 0x004-0x005): preserve status bits 0-2.
                    let current = u16::from_le_bytes([self.io[0x0004], self.io[0x0005]]);
                    let written = u16::from_le_bytes([bytes[0], bytes[1]]);
                    let new_dispstat = (current & 0x0007) | (written & 0xFFF8);
                    let ds_bytes = new_dispstat.to_le_bytes();
                    self.io[0x0004] = ds_bytes[0];
                    self.io[0x0005] = ds_bytes[1];
                    self.io[0x0006] = bytes[2];
                    self.io[0x0007] = bytes[3];
                } else {
                    self.io[idx..idx + 4].copy_from_slice(&bytes);
                }
                self.mark_audio_io_dirty(idx, 4);
                self.mark_dma_dirty(idx);
            }
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                self.palette[idx..idx + 4].copy_from_slice(&bytes);
                self.palette_dirty.mark(idx, 4, REGION_PALETTE_SIZE);
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                self.vram[idx..idx + 4].copy_from_slice(&bytes);
                self.vram_dirty.mark(idx, 4, REGION_VRAM_SIZE);
            }
            REGION_OAM_START..=REGION_OAM_END => {
                let idx = masked_addr as usize;
                self.log_watch_if_hit(addr, 4, value, "u32");
                self.oam[idx..idx + 4].copy_from_slice(&bytes);
                self.oam_dirty.mark(idx, 4, REGION_OAM_SIZE);
            }
            REGION_ROM_START..=REGION_ROM_END => {
                // ROM writes are ignored
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                self.flash_write_u8(masked_addr, bytes[0]);
                self.flash_write_u8((masked_addr + 1) & 0xFFFF, bytes[1]);
                self.flash_write_u8((masked_addr + 2) & 0xFFFF, bytes[2]);
                self.flash_write_u8((masked_addr + 3) & 0xFFFF, bytes[3]);
            }
            _ => {
                // Unmapped - ignore
            }
        }
    }

    fn is_readable(&self, addr: u32) -> bool {
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

    fn is_writable(&self, addr: u32) -> bool {
        matches!(addr,
            REGION_EWRAM_START..=REGION_EWRAM_END |
            REGION_IWRAM_START..=REGION_IWRAM_END |
            REGION_IO_START..=REGION_IO_END |
            REGION_PALETTE_START..=REGION_PALETTE_END |
            REGION_VRAM_START..=REGION_VRAM_END |
            REGION_OAM_START..=REGION_OAM_END |
            REGION_SRAM_START..=REGION_SRAM_END
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewram_read_write() {
        let mut bus = SimpleBus::new(None);

        bus.write_u32(REGION_EWRAM_START, 0x12345678);
        assert_eq!(bus.read_u32(REGION_EWRAM_START), 0x12345678);
    }

    #[test]
    fn test_iwram_read_write() {
        let mut bus = SimpleBus::new(None);

        bus.write_u32(REGION_IWRAM_START, 0xDEADBEEF);
        assert_eq!(bus.read_u32(REGION_IWRAM_START), 0xDEADBEEF);
    }

    #[test]
    fn test_rom_read() {
        let rom = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let bus = SimpleBus::new(Some(rom));

        assert_eq!(bus.read_u32(REGION_ROM_START), 0x03020100);
        assert_eq!(bus.read_u32(REGION_ROM_START + 4), 0x07060504);
    }

    #[test]
    fn test_rom_mirroring() {
        let rom = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let bus = SimpleBus::new(Some(rom));

        // ROM should mirror
        assert_eq!(bus.read_u32(REGION_ROM_START), 0xDDCCBBAA);
        assert_eq!(bus.read_u32(REGION_ROM_START + 4), 0xDDCCBBAA);
    }

    #[test]
    fn test_vram_upper_window_mirrors_to_upper_physical_block() {
        let mut bus = SimpleBus::new(None);

        bus.write_u16(0x0601_0000, 0x1234);
        assert_eq!(bus.read_u16(0x0601_8000), 0x1234);

        bus.write_u16(0x0601_8000, 0xABCD);
        assert_eq!(bus.read_u16(0x0601_0000), 0xABCD);
    }

    #[test]
    fn test_take_video_dirty_ranges_reports_written_spans() {
        let mut bus = SimpleBus::new(None);

        bus.write_u8(0x0500_0001, 0x12);
        bus.write_u16(0x0600_0010, 0x3456);
        bus.write_u32(0x0700_0008, 0xDEADBEEF);

        let (palette, vram, oam) = bus.take_video_dirty_ranges();
        assert_eq!(palette, Some((1, 2)));
        assert_eq!(vram, Some((0x10, 0x12)));
        assert_eq!(oam, Some((0x08, 0x0C)));

        let (palette, vram, oam) = bus.take_video_dirty_ranges();
        assert_eq!(palette, None);
        assert_eq!(vram, None);
        assert_eq!(oam, None);
    }

    #[test]
    fn test_take_video_dirty_ranges_merges_overlapping_writes() {
        let mut bus = SimpleBus::new(None);

        bus.write_u8(0x0600_0020, 0xAA);
        bus.write_u32(0x0600_0022, 0x11223344);
        bus.write_u8(0x0600_0027, 0x55);

        let (_, vram, _) = bus.take_video_dirty_ranges();
        assert_eq!(vram, Some((0x20, 0x28)));
    }

    #[test]
    fn test_take_audio_io_dirty_range_reports_sound_register_writes() {
        let mut bus = SimpleBus::new(None);

        bus.write_u8(0x0400_0060, 0x12);
        bus.write_u16(0x0400_0082, 0x3456);

        assert_eq!(bus.take_audio_io_dirty_range(), Some((0, 0x24)));
        assert_eq!(bus.take_audio_io_dirty_range(), None);
    }

    #[test]
    fn test_flash1m_bank_switch_and_program() {
        let mut rom = vec![0; 0x200];
        rom[0x40..0x40 + b"FLASH1M_V103".len()].copy_from_slice(b"FLASH1M_V103");
        let mut bus = SimpleBus::new(Some(rom));

        // Program bank 0, address 0x1234 with 0x5A.
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xAA);
        bus.write_u8(REGION_SRAM_START + 0x2AAA, 0x55);
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xA0);
        bus.write_u8(REGION_SRAM_START + 0x1234, 0x5A);
        assert_eq!(bus.read_u8(REGION_SRAM_START + 0x1234), 0x5A);

        // Switch to bank 1 and program a different value at the same address.
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xAA);
        bus.write_u8(REGION_SRAM_START + 0x2AAA, 0x55);
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xB0);
        bus.write_u8(REGION_SRAM_START, 0x01);
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xAA);
        bus.write_u8(REGION_SRAM_START + 0x2AAA, 0x55);
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xA0);
        bus.write_u8(REGION_SRAM_START + 0x1234, 0xA5);
        assert_eq!(bus.read_u8(REGION_SRAM_START + 0x1234), 0xA5);

        // Switching back to bank 0 should reveal the original value.
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xAA);
        bus.write_u8(REGION_SRAM_START + 0x2AAA, 0x55);
        bus.write_u8(REGION_SRAM_START + 0x5555, 0xB0);
        bus.write_u8(REGION_SRAM_START, 0x00);
        assert_eq!(bus.read_u8(REGION_SRAM_START + 0x1234), 0x5A);
        assert!(bus.take_save_dirty());
    }
}

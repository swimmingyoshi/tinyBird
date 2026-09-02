//! Memory Bus Implementation
//!
//! The GBA bus connects the CPU to all memory regions and peripherals.
//! It handles address decoding, wait states, and open bus behavior.

use crate::debug::config as debug_config;
use crate::eeprom::{Eeprom, LARGE_SIZE as EEPROM_LARGE_SIZE};
use crate::memory_map::*;
use crate::rtc::{Rtc, GPIO_CONTROL, GPIO_DATA, GPIO_DIRECTION};
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use std::cell::{Cell, RefCell};

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
const PPU_IO_START: usize = 0x00;
const PPU_IO_END: usize = 0x56;
const PPU_IO_LEN: usize = PPU_IO_END - PPU_IO_START;

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

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the whole region dirty, for when it was replaced wholesale.
    fn mark_all(&mut self, max_len: usize) {
        self.start = 0;
        self.end = max_len;
        self.dirty = max_len > 0;
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
    /// Whether a real BIOS image is loaded rather than the built-in stubs.
    ///
    /// Answered directly rather than by reading address zero: the BIOS refuses
    /// ordinary reads from outside itself and hands back its last fetched
    /// opcode, so probing it through the bus reports whatever is latched.
    fn has_real_bios(&self) -> bool;

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
/// `BX LR`, the instruction every high-level BIOS stub is made of. Finding it
/// at address zero is how a stubbed BIOS is told from a real image.
const HLE_STUB_OPCODE: u32 = 0xE12F_FF1E;

/// A plain SRAM cartridge carries 32 KB, half the window it appears in.
const SRAM_CHIP_SIZE: usize = 0x8000;

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
    /// Dirty byte span for PPU control registers since the last render-side sync.
    ppu_io_dirty: DirtyRange,
    /// Set when any timer control register (TMxCNT_H) was written by the CPU.
    timer_control_dirty: bool,
    /// Set when any DMA register (I/O 0xB0..=0xDF) was written; cleared by take_dma_dirty.
    dma_dirty: bool,
    /// Set when a serial register was written; cleared by `take_sio_dirty`.
    sio_dirty: bool,
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
    /// The last opcode fetched out of the BIOS, and whether the core is
    /// currently executing there.
    ///
    /// The BIOS refuses to be read from outside itself: instead of its own
    /// contents it hands back whatever it last fetched. A few games read the
    /// region deliberately, as a cheap source of a value an emulator is
    /// unlikely to reproduce.
    ///
    /// `Cell` because instruction fetches come through `&self`, and neither
    /// field belongs in a savestate: both are re-established by the next fetch.
    bios_opcode: Cell<u32>,
    executing_in_bios: Cell<bool>,
    /// The cartridge clock, when the cartridge has one.
    ///
    /// Left out of the savestate deliberately. A real cartridge's clock keeps
    /// its own time on its own battery whether or not the console is running,
    /// so restoring a machine should not wind it back — and the host pushes the
    /// time in anyway, which puts it right on the very next frame.
    rtc: Rtc,
    /// Whether this cartridge has a clock chip on it at all.
    ///
    /// Only the three games that carry one get the GPIO registers, so no other
    /// cartridge can have its ROM shadowed by them.
    has_rtc: bool,
    /// Serial EEPROM, when the cartridge has one.
    ///
    /// Behind a `RefCell` because clocking a bit out changes the chip's state,
    /// and reads come through `&self`. Its contents ride in `save_memory` when
    /// serialized, so adding it did not change the savestate format.
    eeprom: RefCell<Eeprom>,
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

fn vec_to_boxed_array<const N: usize, E: DeError>(
    vec: Vec<u8>,
    name: &str,
) -> Result<Box<[u8; N]>, E> {
    let arr: [u8; N] = vec.try_into().map_err(|v: Vec<u8>| {
        E::custom(format!("{} had length {}, expected {}", name, v.len(), N))
    })?;
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
            // EEPROM keeps its data inside the chip; hand it over through the
            // same field the other backup types use so the format is unchanged.
            save_memory: if matches!(self.save_type, SaveType::Eeprom) {
                self.eeprom.borrow().data().to_vec()
            } else {
                self.save_memory.clone()
            },
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

/// Whether a cartridge image is one of the three with a clock chip on it.
///
/// Named by game code rather than probed, because there is nothing to probe:
/// the GPIO pins on a board without the chip read as ROM, which is exactly
/// what they read as here. Ruby, Sapphire and Emerald are the whole list for
/// the Game Boy Advance's Pokemon line; Boktai's cartridges also carry one,
/// along with a light sensor this does not emulate, so they are left out.
fn cart_has_clock(rom: &[u8]) -> bool {
    const GAME_CODE: usize = 0xAC;
    let Some(code) = rom.get(GAME_CODE..GAME_CODE + 3) else {
        return false;
    };
    matches!(code, b"AXV" | b"AXP" | b"BPE")
}

impl<'de> Deserialize<'de> for SimpleBus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = SimpleBusSerde::deserialize(deserializer)?;
        // Read before the fields are moved out of the helper.
        let rom_for_clock = helper.rom.clone();
        let mut eeprom = Eeprom::new();
        if matches!(helper.save_type, SaveType::Eeprom) {
            eeprom.load(&helper.save_memory);
        }
        Ok(Self {
            bios: vec_to_boxed_array::<REGION_BIOS_SIZE, D::Error>(helper.bios, "bios")?,
            ewram: vec_to_boxed_array::<REGION_EWRAM_SIZE, D::Error>(helper.ewram, "ewram")?,
            iwram: vec_to_boxed_array::<REGION_IWRAM_SIZE, D::Error>(helper.iwram, "iwram")?,
            io: vec_to_boxed_array::<REGION_IO_SIZE, D::Error>(helper.io, "io")?,
            palette: vec_to_boxed_array::<REGION_PALETTE_SIZE, D::Error>(
                helper.palette,
                "palette",
            )?,
            vram: vec_to_boxed_array::<REGION_VRAM_SIZE, D::Error>(helper.vram, "vram")?,
            oam: vec_to_boxed_array::<REGION_OAM_SIZE, D::Error>(helper.oam, "oam")?,
            palette_dirty: helper.palette_dirty,
            vram_dirty: helper.vram_dirty,
            oam_dirty: helper.oam_dirty,
            audio_io_dirty: helper.audio_io_dirty,
            ppu_io_dirty: DirtyRange::default(),
            timer_control_dirty: false,
            dma_dirty: helper.dma_dirty,
            // Not carried in the savestate: a restored console has just been
            // unplugged, and whatever the game was about to write it writes again.
            sio_dirty: false,
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
            bios_opcode: Cell::new(0),
            executing_in_bios: Cell::new(true),
            rtc: Rtc::new(),
            has_rtc: cart_has_clock(&rom_for_clock),
            eeprom: RefCell::new(eeprom),
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
            ppu_io_dirty: DirtyRange::default(),
            timer_control_dirty: false,
            dma_dirty: false,
            sio_dirty: false,
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
            bios_opcode: Cell::new(0),
            executing_in_bios: Cell::new(true),
            rtc: Rtc::new(),
            // Set from the cartridge just below, once it is in the struct.
            has_rtc: false,
            eeprom: RefCell::new(Eeprom::new()),
        };
        bus.has_rtc = cart_has_clock(&bus.rom);
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
            (0x00, HLE_STUB_OPCODE), // BX LR  (reset/null-fn: return to caller)
            (0x04, 0xE12FFF1E),      // BX LR  (undefined instruction stub)
            (0x08, 0xE12FFF1E),      // BX LR  (SWI stub — handled by HLE, never reached)
            (0x0C, 0xE12FFF1E),      // BX LR  (prefetch abort stub)
            (0x10, 0xE12FFF1E),      // BX LR  (data abort stub)
            (0x14, 0xE12FFF1E),      // BX LR  (reserved stub)
            (0x18, 0xE92D500F),      // STMFD SP!, {R0-R3, R12, LR}
            (0x1C, 0xE3A00403),      // MOV R0, #0x03000000
            (0x20, 0xE2800C7F),      // ADD R0, R0, #0x7F00
            (0x24, 0xE59000FC),      // LDR R0, [R0, #0xFC]
            (0x28, 0xE3500000),      // CMP R0, #0
            (0x2C, 0x0A000001),      // BEQ 0x38
            (0x30, 0xE1A0E00F),      // MOV LR, PC  (LR = 0x38)
            (0x34, 0xE12FFF10),      // BX R0
            (0x38, 0xE8BD500F),      // LDMFD SP!, {R0-R3, R12, LR}
            (0x3C, 0xE25EF004),      // SUBS PC, LR, #4
        ];
        for &(addr, word) in stub {
            let bytes = word.to_le_bytes();
            self.bios[addr..addr + 4].copy_from_slice(&bytes);
        }
    }

    /// Load ROM data into the bus
    /// Insert a cartridge.
    ///
    /// A different cartridge brings its own battery, so the backup memory is
    /// wiped rather than carried over: leaving the previous game's save behind
    /// hands the new one somebody else's data under its own save type.
    pub fn load_rom(&mut self, rom: Vec<u8>) {
        self.rom = rom;
        self.has_rtc = cart_has_clock(&self.rom);
        self.rtc = Rtc::new();
        self.detect_save_type();
        self.reset_save_state();
        self.save_memory = vec![0xFF; self.save_memory.len().max(REGION_SRAM_SIZE)];
    }

    /// Take the cartridge image out of the bus.
    ///
    /// Used when writing a savestate. The ROM is the largest thing in the
    /// machine by an order of magnitude — up to 32MB against roughly 400KB of
    /// everything else put together — and it is also the one part the player
    /// already holds a copy of, since a state is only ever restored with the
    /// same cartridge inserted.
    pub fn take_rom(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rom)
    }

    /// Put a cartridge image back, and nothing else.
    ///
    /// Unlike [`SimpleBus::load_rom`] this leaves the save memory and the
    /// detected save type alone: restoring a savestate puts back the very
    /// cartridge the state was written against, battery and all, so wiping the
    /// backup memory would throw away the progress being restored.
    pub fn restore_rom(&mut self, rom: Vec<u8>) {
        self.rom = rom;
        self.has_rtc = cart_has_clock(&self.rom);
    }

    /// Tell the cartridge clock what the time is. See [`crate::rtc`].
    pub fn set_wall_clock(&mut self, unix_seconds: i64) {
        self.rtc.set_wall_clock(unix_seconds);
    }

    /// Carry the cartridge clock forward by cycles the machine has run.
    pub fn step_rtc(&mut self, cycles: u64) {
        if self.has_rtc {
            self.rtc.step(cycles);
        }
    }

    /// Whether this cartridge has a clock chip on it.
    pub fn has_clock(&self) -> bool {
        self.has_rtc
    }

    /// The clock register at this address, if the game has switched the
    /// registers on.
    ///
    /// Reads are gated on the control register as well as on the cartridge:
    /// until a game sets it, these three halfwords are the ROM underneath, and
    /// a game that never asks never sees anything else. The `has_rtc` check
    /// comes first and is one bool, so every other ROM read pays a branch that
    /// is always false.
    fn live_gpio_addr(&self, addr: u32) -> Option<u32> {
        if !self.has_rtc || !self.rtc.registers_are_live() {
            return None;
        }
        self.is_gpio_addr(addr & !1)
    }

    /// Whether an address is one of the clock's three registers.
    ///
    /// Checked against the cartridge offset, so it catches the mirrors too.
    fn is_gpio_addr(&self, addr: u32) -> Option<u32> {
        // The region check is not redundant. The cartridge offset of these
        // registers is 0xC4..0xC8, and every region mirrors down to the same
        // low offsets — EWRAM's 0x020000C4 masks to 0xC4 as readily as ROM's
        // 0x080000C4 does. Only one of them is the clock.
        if !self.has_rtc || !matches!(addr, REGION_ROM_START..=REGION_ROM_END) {
            return None;
        }
        match addr & 0x01FF_FFFF {
            GPIO_DATA => Some(GPIO_DATA),
            GPIO_DIRECTION => Some(GPIO_DIRECTION),
            GPIO_CONTROL => Some(GPIO_CONTROL),
            _ => None,
        }
    }

    /// Take the cartridge out.
    ///
    /// Not the same as loading an empty one: the save memory, the backup type
    /// and the clock all belong to the cartridge that just left, and leaving
    /// any of them behind would hand the next game somebody else's battery.
    pub fn eject_rom(&mut self) {
        self.rom = Vec::new();
        self.has_rtc = false;
        self.rtc = Rtc::new();
        self.save_type = SaveType::None;
        self.save_memory = vec![0xFF; REGION_SRAM_SIZE];
        self.eeprom = RefCell::new(Eeprom::new());
        self.reset_save_state();
    }

    /// Whether a cartridge is present.
    pub fn has_rom(&self) -> bool {
        !self.rom.is_empty()
    }

    /// Return the machine to its power-on state.
    ///
    /// Clears everything volatile: work RAM, video memory, and the I/O
    /// registers. The cartridge, the BIOS image, and battery-backed save data
    /// all survive, exactly as they do when you press reset on the hardware.
    ///
    /// Without this a reset only rolled back the CPU and PPU while every byte
    /// of RAM and every I/O register kept the previous game's values, so
    /// swapping cartridges left the old game on screen and the new one wedged
    /// on hardware it never configured.
    pub fn reset(&mut self) {
        self.ewram.fill(0);
        self.iwram.fill(0);
        self.io.fill(0);
        self.palette.fill(0);
        self.vram.fill(0);
        self.oam.fill(0);

        // Everything the PPU and APU mirror has just changed under them, so
        // hand back the full span rather than a stale window.
        self.palette_dirty.mark_all(REGION_PALETTE_SIZE);
        self.vram_dirty.mark_all(REGION_VRAM_SIZE);
        self.oam_dirty.mark_all(REGION_OAM_SIZE);
        self.audio_io_dirty.mark_all(REGION_IO_SIZE);
        self.ppu_io_dirty.mark_all(REGION_IO_SIZE);
        // The DMA and timer units are reset by the caller rather than re-synced
        // from these registers, so there is nothing pending to hand them.
        self.timer_control_dirty = false;
        self.dma_dirty = false;
        self.sio_dirty = false;

        self.open_bus_value = 0;
        self.vcount = 0;
        self.vcount_cycles = 0;
        self.bios_protected = false;
        // The BIOS latch is part of the machine's volatile state; leaving it
        // set makes a reset machine differ from a cold one, which is exactly
        // what `a_switched_rom_runs_like_a_cold_boot` exists to catch.
        self.bios_opcode.set(0);
        self.executing_in_bios.set(true);
        self.cpu_timing_active.set(false);
        self.cpu_timing_cycles.set(0);
        self.cpu_last_access.set(None);

        // The cartridge stays in the slot, so its backup memory stays with it;
        // only the volatile command state of the flash chip is cleared.
        self.reset_save_state();
    }

    /// Human-readable name of the cartridge backup this ROM advertises.
    ///
    /// Detected from the tag string every commercial GBA cart embeds. Exposed
    /// so the frontend can show it when bringing up a new game: a wrong or
    /// missing backup type is the usual reason a game boots but cannot save.
    pub fn save_type_label(&self) -> &'static str {
        match self.save_type {
            SaveType::None => "None detected",
            SaveType::Sram => "SRAM 32K",
            SaveType::Flash64K => "Flash 64K",
            SaveType::Flash128K => "Flash 128K",
            SaveType::Eeprom => "EEPROM",
        }
    }

    /// Whether the cartridge advertises any battery-backed save at all.
    pub fn has_battery_save(&self) -> bool {
        !matches!(self.save_type, SaveType::None)
    }

    fn detect_save_type(&mut self) {
        let save_type = if self
            .rom
            .windows(b"FLASH1M_V".len())
            .any(|w| w == b"FLASH1M_V")
        {
            SaveType::Flash128K
        } else if self.rom.windows(b"FLASH_V".len()).any(|w| w == b"FLASH_V")
            || self
                .rom
                .windows(b"FLASH512_V".len())
                .any(|w| w == b"FLASH512_V")
        {
            SaveType::Flash64K
        } else if self.rom.windows(b"SRAM_V".len()).any(|w| w == b"SRAM_V")
            || self
                .rom
                .windows(b"SRAM_F_V".len())
                .any(|w| w == b"SRAM_F_V")
        {
            SaveType::Sram
        } else if self
            .rom
            .windows(b"EEPROM_V".len())
            .any(|w| w == b"EEPROM_V")
        {
            SaveType::Eeprom
        } else {
            SaveType::None
        };

        self.save_type = save_type;
        let size = match save_type {
            SaveType::None => REGION_SRAM_SIZE,
            SaveType::Sram => SRAM_CHIP_SIZE,
            SaveType::Flash64K => REGION_SRAM_SIZE,
            SaveType::Eeprom => EEPROM_LARGE_SIZE,
            SaveType::Flash128K => REGION_SRAM_SIZE * 2,
        };
        self.save_memory = vec![0xFF; size];
        if matches!(save_type, SaveType::Eeprom) {
            *self.eeprom.get_mut() = Eeprom::new();
        }
    }

    fn reset_save_state(&mut self) {
        self.flash_id_mode = false;
        self.flash_cmd_state = FlashCommandState::Ready;
        self.flash_bank = 0;
        self.save_dirty = false;
    }

    /// Where an address inside the 64 KB window lands in the save chip.
    ///
    /// A plain SRAM cartridge carries 32 KB, so it mirrors halfway up the
    /// window; flash fills it. Missing that mirror leaves the upper half of the
    /// window reading as a separate, empty 32 KB that a game expects to be the
    /// same memory it just wrote.
    fn flash_bank_offset(&self, masked_addr: u32) -> usize {
        let (bank, chip_mask) = match self.save_type {
            SaveType::Flash128K => (self.flash_bank, 0xFFFF),
            SaveType::Sram => (0, SRAM_CHIP_SIZE as u32 - 1),
            _ => (0, 0xFFFF),
        };
        let offset = bank * REGION_SRAM_SIZE + (masked_addr & chip_mask) as usize;
        // Never index past the chip, whatever the save type claims.
        offset.min(self.save_memory.len().saturating_sub(1))
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
        !matches!(self.save_type, SaveType::None)
    }

    /// Return a snapshot of the cartridge save memory for persistence.
    ///
    /// EEPROM is not memory-mapped, so its contents live in the chip rather
    /// than in `save_memory`; the caller should not have to care which.
    pub fn save_data(&self) -> Vec<u8> {
        match self.save_type {
            SaveType::None => Vec::new(),
            SaveType::Eeprom => self.eeprom.borrow().data().to_vec(),
            _ => self.save_memory.clone(),
        }
    }

    /// Replace the cartridge save memory from persisted data.
    pub fn load_save_data(&mut self, data: &[u8]) {
        if !self.has_persistent_save() {
            return;
        }

        if matches!(self.save_type, SaveType::Eeprom) {
            self.eeprom.get_mut().load(data);
            self.save_dirty = false;
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
    /// Whether an audio register has been written since the last sync.
    ///
    /// Unlike `take_audio_io_dirty_range`, this leaves the range in place. The
    /// APU batches its ticking, and a pending batch has to be flushed *before*
    /// a register write reaches the chip — asking without consuming is what
    /// lets the caller decide to flush first.
    pub fn audio_io_dirty(&self) -> bool {
        self.audio_io_dirty.is_dirty()
    }

    pub fn take_audio_io_dirty_range(&mut self) -> Option<(usize, usize)> {
        self.audio_io_dirty.take()
    }

    /// Return and clear the dirty byte range for display-control I/O mirrors.
    pub fn take_ppu_io_dirty_range(&mut self) -> Option<(usize, usize)> {
        self.ppu_io_dirty.take()
    }

    /// Take and clear the timer-control dirty flag.
    pub fn take_timer_control_dirty(&mut self) -> bool {
        let dirty = self.timer_control_dirty;
        self.timer_control_dirty = false;
        dirty
    }

    /// Take and clear the DMA-dirty flag. Returns true if any DMA register was written.
    pub fn take_dma_dirty(&mut self) -> bool {
        let dirty = self.dma_dirty;
        self.dma_dirty = false;
        dirty
    }

    /// Take and clear the serial-dirty flag.
    ///
    /// True when the game touched a link-cable register, which is the only
    /// time the serial port needs looking at. The alternative is re-reading a
    /// register that almost never changes on every single instruction.
    #[inline(always)]
    pub fn take_sio_dirty(&mut self) -> bool {
        // Checked once per instruction and true perhaps a few times in a whole
        // game, so the early return matters: clearing unconditionally means a
        // store into this struct on every instruction ever executed.
        if !self.sio_dirty {
            return false;
        }
        self.sio_dirty = false;
        true
    }

    #[inline(always)]
    fn mark_sio_dirty(&mut self, offset: usize, len: usize) {
        let end = offset.saturating_add(len);
        // The data and control registers sit together at 0x120..=0x12F. RCNT,
        // which decides whether the port is a link cable at all, is off on its
        // own at 0x134.
        if (offset < 0x130 && 0x120 < end) || (offset < 0x136 && 0x134 < end) {
            self.sio_dirty = true;
        }
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

    fn mark_ppu_io_dirty(&mut self, offset: usize, len: usize) {
        let start = offset.max(PPU_IO_START);
        let end = offset.saturating_add(len).min(PPU_IO_END);
        if start < end {
            self.ppu_io_dirty
                .mark(start - PPU_IO_START, end - start, PPU_IO_LEN);
        }
    }

    fn mark_timer_control_dirty(&mut self, offset: usize, len: usize) {
        let end = offset.saturating_add(len);
        for base in [0x102usize, 0x106, 0x10A, 0x10E] {
            if offset < base + 2 && base < end {
                self.timer_control_dirty = true;
                break;
            }
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
            // Game Pak SRAM/Flash window mirrors every 64KB, all the way up.
            0x0E00_0000..=0x0FFF_FFFF => addr & 0xFFFF,
            _ => addr,
        }
    }

    /// Get ROM index with mirroring
    /// Whether `addr` lands on the cartridge's serial EEPROM.
    ///
    /// EEPROM sits at the top of the Game Pak window. On a cartridge of 16 MB
    /// or less the whole of `0x0D000000..=0x0DFFFFFF` answers, because there is
    /// no ROM up there to collide with; a larger cartridge leaves only the last
    /// 256 bytes, which is why big games address it at `0x0DFFFF00`.
    fn is_eeprom_addr(&self, addr: u32) -> bool {
        if !matches!(self.save_type, SaveType::Eeprom) {
            return false;
        }
        if self.rom.len() > 16 * 1024 * 1024 {
            (0x0DFF_FF00..=0x0DFF_FFFF).contains(&addr)
        } else {
            (0x0D00_0000..=0x0DFF_FFFF).contains(&addr)
        }
    }

    /// Clock one bit out of the EEPROM. The game sees it in bit 0.
    fn eeprom_read(&self) -> u8 {
        u8::from(self.eeprom.borrow_mut().read_bit())
    }

    /// Clock one bit in. Only bit 0 of the written value carries data.
    fn eeprom_write(&mut self, value: u32) {
        let chip = self.eeprom.get_mut();
        chip.write_bit(value & 1 == 1);
        if chip.is_dirty() {
            chip.clear_dirty();
            self.save_dirty = true;
        }
    }

    /// Where `addr` lands in the cartridge, or `None` if it is past the end.
    ///
    /// Cartridge ROM does not mirror. An earlier version wrapped with `%`, so a
    /// read beyond the cartridge answered with its own opening bytes instead of
    /// what the hardware returns.
    fn rom_index(&self, addr: u32) -> Option<usize> {
        let offset = (addr & 0x1FF_FFFF) as usize;
        (offset < self.rom.len()).then_some(offset)
    }

    /// Where sprite VRAM starts, which depends on the video mode.
    ///
    /// Tiled modes give backgrounds the first 64 KB; the bitmap modes need
    /// 80 KB for the frame buffer and push sprites up to match.
    fn obj_vram_base(&self) -> usize {
        if self.io[0] & 0x7 >= 3 {
            0x1_4000
        } else {
            0x1_0000
        }
    }

    /// Note where an instruction was fetched from, for BIOS protection.
    fn note_opcode_fetch(&self, addr: u32, opcode: u32) {
        let in_bios = addr <= REGION_BIOS_END;
        self.executing_in_bios.set(in_bios);
        if in_bios {
            self.bios_opcode.set(opcode);
        }
    }

    /// One aligned word of the BIOS, wrapping inside the image.
    fn bios_word(&self, addr: u32) -> u32 {
        let idx = (addr as usize & !3) % REGION_BIOS_SIZE;
        u32::from_le_bytes([
            self.bios[idx],
            self.bios[idx + 1],
            self.bios[idx + 2],
            self.bios[idx + 3],
        ])
    }

    /// A read of the BIOS region, honouring its read protection.
    ///
    /// Code running inside the BIOS sees the real thing; anything else gets the
    /// last opcode the BIOS fetched.
    fn bios_read_u32(&self, masked_addr: u32) -> u32 {
        if self.executing_in_bios.get() {
            let idx = (masked_addr & !3) as usize;
            u32::from_le_bytes([
                self.bios[idx],
                self.bios[idx + 1],
                self.bios[idx + 2],
                self.bios[idx + 3],
            ])
        } else {
            self.bios_opcode.get()
        }
    }

    /// What the Game Pak bus reports for an address with no ROM behind it.
    ///
    /// Nothing drives the data lines, so what comes back is the address the CPU
    /// put on the bus: the halfword at `addr` reads as `addr >> 1`.
    fn gamepak_open_bus_u16(addr: u32) -> u16 {
        (addr >> 1) as u16
    }

    fn gamepak_open_bus_u8(addr: u32) -> u8 {
        let half = Self::gamepak_open_bus_u16(addr);
        if addr & 1 == 0 {
            half as u8
        } else {
            (half >> 8) as u8
        }
    }

    /// Read a halfword of cartridge, falling back to the bus past the end.
    fn rom_read_u16(&self, addr: u32) -> u16 {
        let aligned = addr & !1;
        match self.rom_index(aligned) {
            Some(idx) if idx + 1 < self.rom.len() => {
                u16::from_le_bytes([self.rom[idx], self.rom[idx + 1]])
            }
            _ => Self::gamepak_open_bus_u16(aligned),
        }
    }

    /// Read a word of cartridge, falling back to the bus past the end.
    fn rom_read_u32(&self, addr: u32) -> u32 {
        let aligned = addr & !3;
        match self.rom_index(aligned) {
            Some(idx) if idx + 3 < self.rom.len() => u32::from_le_bytes([
                self.rom[idx],
                self.rom[idx + 1],
                self.rom[idx + 2],
                self.rom[idx + 3],
            ]),
            // A word straddling the end is two halfwords, each answered by
            // whichever side of the boundary it falls on.
            _ => {
                let low = self.rom_read_u16(aligned) as u32;
                let high = self.rom_read_u16(aligned.wrapping_add(2)) as u32;
                low | (high << 16)
            }
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
                if value == 0xF0 && !matches!(self.flash_cmd_state, FlashCommandState::Program) {
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
                let prefetch_hit =
                    kind == AccessKind::Opcode && self.prefetch_enabled() && sequential;
                match width {
                    AccessWidth::Word => {
                        let first = if prefetch_hit {
                            1
                        } else {
                            self.gamepak_wait_cycles(area, sequential)
                        };
                        first
                            + if prefetch_hit {
                                1
                            } else {
                                self.gamepak_wait_cycles(area, true)
                            }
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
        self.cpu_last_access.set(Some(AccessStamp {
            region,
            next_addr: Self::next_sequential_addr(addr, width, region),
        }));
    }
}

impl Bus for SimpleBus {
    fn has_real_bios(&self) -> bool {
        u32::from_le_bytes([self.bios[0], self.bios[1], self.bios[2], self.bios[3]])
            != HLE_STUB_OPCODE
    }

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
            REGION_BIOS_START..=REGION_BIOS_END => {
                let word = self.bios_read_u32(masked_addr);
                (word >> ((masked_addr & 3) * 8)) as u8
            }
            REGION_EWRAM_START..=REGION_EWRAM_END => self.ewram[masked_addr as usize],
            REGION_IWRAM_START..=REGION_IWRAM_END => self.iwram[masked_addr as usize],
            REGION_IO_START..=REGION_IO_END => self.io[masked_addr as usize],
            REGION_PALETTE_START..=REGION_PALETTE_END => self.palette[masked_addr as usize],
            REGION_VRAM_START..=REGION_VRAM_END => self.vram[masked_addr as usize],
            REGION_OAM_START..=REGION_OAM_END => self.oam[masked_addr as usize],
            REGION_ROM_START..=REGION_ROM_END => {
                if self.is_eeprom_addr(addr) {
                    self.eeprom_read()
                } else if let Some(register) = self.live_gpio_addr(addr) {
                    // The low byte of the halfword the chip presents.
                    (self.rtc.read(register) >> ((addr & 1) * 8)) as u8
                } else if let Some(idx) = self.rom_index(addr) {
                    self.rom[idx]
                } else {
                    Self::gamepak_open_bus_u8(addr)
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
        let masked_addr = self.mask_address(addr) & !1;

        let value = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let word = self.bios_read_u32(masked_addr);
                (word >> ((masked_addr & 2) * 8)) as u16
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
                // One halfword access carries one EEPROM bit; this is the path
                // the DMA that drives the chip actually takes.
                if self.is_eeprom_addr(addr) {
                    self.eeprom_read() as u16
                } else if let Some(register) = self.live_gpio_addr(addr) {
                    self.rtc.read(register)
                } else {
                    self.rom_read_u16(addr)
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                // The cartridge save bus is 8 bits wide, so a halfword access
                // returns the single byte at that address repeated, not the
                // two bytes either side of it. The address is deliberately the
                // unaligned one: an 8-bit chip picks its byte from the low bits
                // the wider buses ignore.
                self.flash_read_u8(self.mask_address(addr)) as u16 * 0x0101
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
        let masked_addr = self.mask_address(addr) & !3;

        let value = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => self.bios_read_u32(masked_addr),
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
                if self.is_eeprom_addr(addr) {
                    self.eeprom_read() as u32
                } else {
                    self.rom_read_u32(addr)
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                // Same 8-bit bus: one byte, repeated across the word.
                self.flash_read_u8(self.mask_address(addr)) as u32 * 0x0101_0101
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
        let masked_addr = self.mask_address(addr) & !1;
        self.executing_in_bios.set(addr <= REGION_BIOS_END);

        let fetched = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => {
                let word = self.bios_read_u32(masked_addr);
                (word >> ((masked_addr & 2) * 8)) as u16
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
            REGION_ROM_START..=REGION_ROM_END => self.rom_read_u16(addr),
            REGION_SRAM_START..=REGION_SRAM_END => {
                // The cartridge save bus is 8 bits wide, so a halfword access
                // returns the single byte at that address repeated, not the
                // two bytes either side of it. The address is deliberately the
                // unaligned one: an 8-bit chip picks its byte from the low bits
                // the wider buses ignore.
                self.flash_read_u8(self.mask_address(addr)) as u16 * 0x0101
            }
            _ => (self.open_bus_value & 0xFFFF) as u16,
        };

        if addr <= REGION_BIOS_END {
            // Thumb prefetches two halfwords ahead rather than two words.
            self.bios_opcode
                .set(self.bios_word(masked_addr.wrapping_add(4)));
        }
        fetched
    }

    fn read_opcode_u32(&self, addr: u32) -> u32 {
        self.record_cpu_access(addr & !3, AccessWidth::Word, AccessKind::Opcode);
        let masked_addr = self.mask_address(addr) & !3;
        self.executing_in_bios.set(addr <= REGION_BIOS_END);

        let fetched = match addr {
            REGION_BIOS_START..=REGION_BIOS_END => self.bios_read_u32(masked_addr),
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
            REGION_ROM_START..=REGION_ROM_END => self.rom_read_u32(addr),
            REGION_SRAM_START..=REGION_SRAM_END => {
                // Same 8-bit bus: one byte, repeated across the word.
                self.flash_read_u8(self.mask_address(addr)) as u32 * 0x0101_0101
            }
            _ => self.open_bus_value,
        };

        if addr <= REGION_BIOS_END {
            // What the BIOS hands out is the opcode its *prefetch* holds, two
            // instructions ahead of the one executing. This core fetches and
            // executes in one step, so the value is read from where the
            // prefetch would have been rather than from where it is.
            self.bios_opcode
                .set(self.bios_word(masked_addr.wrapping_add(8)));
        }
        fetched
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
                self.mark_ppu_io_dirty(masked_addr as usize, 1);
                self.mark_timer_control_dirty(masked_addr as usize, 1);
                self.mark_dma_dirty(masked_addr as usize);
                self.mark_sio_dirty(masked_addr as usize, 1);
            }
            // Video memory sits on a 16-bit bus, so a byte write cannot address
            // half of a halfword. Palette and background VRAM answer by storing
            // the byte in *both* halves; sprite VRAM and OAM ignore the write
            // altogether. Storing the single byte instead corrupts the
            // neighbouring one, which shows up as stray colours and misplaced
            // tiles rather than as an obvious failure.
            REGION_PALETTE_START..=REGION_PALETTE_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                let idx = (masked_addr & !1) as usize;
                self.palette[idx] = value;
                self.palette[idx + 1] = value;
                self.palette_dirty.mark(idx, 2, REGION_PALETTE_SIZE);
            }
            REGION_VRAM_START..=REGION_VRAM_END => {
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
                let idx = masked_addr as usize;
                if idx >= self.obj_vram_base() {
                    // Sprite VRAM ignores byte writes.
                } else {
                    let idx = idx & !1;
                    self.vram[idx] = value;
                    self.vram[idx + 1] = value;
                    self.vram_dirty.mark(idx, 2, REGION_VRAM_SIZE);
                }
            }
            REGION_OAM_START..=REGION_OAM_END => {
                // Ignored outright.
                self.log_watch_if_hit(addr, 1, value as u32, "u8");
            }
            REGION_ROM_START..=REGION_ROM_END => {
                if self.is_eeprom_addr(addr) {
                    self.eeprom_write(value as u32);
                }
                // Otherwise ROM writes are ignored.
            }
            REGION_SRAM_START..=REGION_SRAM_END => self.flash_write_u8(masked_addr, value),
            _ => {
                // Unmapped - ignore
            }
        }
    }

    fn write_u16(&mut self, addr: u32, value: u16) {
        self.record_cpu_access(addr & !1, AccessWidth::Half, AccessKind::Data);
        let masked_addr = self.mask_address(addr) & !1;
        let bytes = value.to_le_bytes();

        // Trace DMA register writes when TINYBIRD_DMA_DEBUG is set
        if debug_config().dma_debug && addr >= 0x040000B0 && addr <= 0x040000DE {
            let names = ["SAD_L", "SAD_H", "DAD_L", "DAD_H", "CNT_L", "CNT_H"];
            let ch = ((addr - 0x040000B0) / 12) as usize;
            let reg = (((addr - 0x040000B0) % 12) / 2) as usize;
            let name = if ch < 4 && reg < 6 { names[reg] } else { "???" };
            eprintln!(
                "DMA{} {} write: addr={:08x} val={:04x}",
                ch, name, addr, value
            );
        }

        // Before the region match: the clock's registers sit inside the ROM,
        // which is otherwise read-only, and the control register is how a game
        // switches the other two on — so this cannot be gated on them being on.
        if let Some(register) = self.is_gpio_addr(addr & !1) {
            self.rtc.write(register, value);
            return;
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
                self.mark_ppu_io_dirty(idx, 2);
                self.mark_timer_control_dirty(idx, 2);
                self.mark_dma_dirty(idx);
                self.mark_sio_dirty(idx, 2);
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
                // A halfword write clocks one bit into the EEPROM; this is how
                // the DMA that drives the chip delivers a command.
                if self.is_eeprom_addr(addr) {
                    self.eeprom_write(value as u32);
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                // Only one byte reaches an 8-bit bus: the one in the lane the
                // real address selects, which the alignment above discards.
                let raw = self.mask_address(addr);
                self.flash_write_u8(raw, bytes[(raw & 1) as usize]);
            }
            _ => {
                // Unmapped - ignore
            }
        }
    }

    fn write_u32(&mut self, addr: u32, value: u32) {
        self.record_cpu_access(addr & !3, AccessWidth::Word, AccessKind::Data);
        let masked_addr = self.mask_address(addr) & !3;
        let bytes = value.to_le_bytes();

        // Trace DMA register writes when TINYBIRD_DMA_DEBUG is set
        if debug_config().dma_debug && addr >= 0x040000B0 && addr <= 0x040000DE {
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
                        self.io[0x202],
                        self.io[0x203],
                        self.io[0x204],
                        self.io[0x205],
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
                self.mark_ppu_io_dirty(idx, 4);
                self.mark_timer_control_dirty(idx, 4);
                self.mark_dma_dirty(idx);
                self.mark_sio_dirty(idx, 4);
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
                if self.is_eeprom_addr(addr) {
                    self.eeprom_write(value);
                }
            }
            REGION_SRAM_START..=REGION_SRAM_END => {
                let raw = self.mask_address(addr);
                self.flash_write_u8(raw, bytes[(raw & 3) as usize]);
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
    fn reset_clears_volatile_memory() {
        let mut bus = SimpleBus::new(Some(vec![0xAA; 64]));
        bus.write_u16(0x0200_0000, 0x1234); // EWRAM
        bus.write_u16(0x0300_0000, 0x5678); // IWRAM
        bus.write_u16(0x0500_0000, 0x7FFF); // palette
        bus.write_u16(0x0600_0000, 0x4321); // VRAM
        bus.write_u16(0x0700_0000, 0x8765); // OAM
        bus.write_io_direct(0x00, 0x40); // DISPCNT

        bus.reset();

        assert_eq!(bus.read_u16(0x0200_0000), 0, "EWRAM");
        assert_eq!(bus.read_u16(0x0300_0000), 0, "IWRAM");
        assert_eq!(bus.read_u16(0x0500_0000), 0, "palette");
        assert_eq!(bus.read_u16(0x0600_0000), 0, "VRAM");
        assert_eq!(bus.read_u16(0x0700_0000), 0, "OAM");
        assert_eq!(bus.read_u16(0x0400_0000), 0, "DISPCNT");
    }

    /// A cartridge image with a game code at the header offset, filled with a
    /// recognisable byte so ROM showing through is obvious.
    fn cart(code: &[u8; 4]) -> Vec<u8> {
        let mut rom = vec![0xAA; 0x200];
        rom[0xAC..0xB0].copy_from_slice(code);
        rom
    }

    /// Ejecting takes the cartridge's battery and clock with it.
    ///
    /// A cartridge left half out is worse than one left in: the next game
    /// would boot onto somebody else's save memory and read it as its own.
    #[test]
    fn ejecting_leaves_nothing_of_the_cartridge_behind() {
        let mut bus = SimpleBus::new(Some(cart(b"BPEE")));
        bus.save_type = SaveType::Sram;
        bus.write_u8(0x0E00_0000, 0x42);
        assert!(bus.has_rom());
        assert!(bus.has_clock());

        bus.eject_rom();

        assert!(!bus.has_rom());
        assert!(!bus.has_clock(), "the clock belonged to the cartridge");
        assert_eq!(bus.save_type, SaveType::None);
        assert_eq!(
            bus.read_u8(0x0E00_0000),
            0xFF,
            "and so did the battery, so the next game does not read it"
        );
    }

    /// FireRed has no clock chip, so 0x080000C4 is cartridge and stays it.
    #[test]
    fn a_cartridge_without_a_clock_never_shadows_its_own_rom() {
        let mut bus = SimpleBus::new(Some(cart(b"BPRE")));
        assert!(!bus.has_clock());

        // Even a game that wrote the control register would get nothing: there
        // is no chip on the board to answer.
        bus.write_u16(REGION_ROM_START + 0xC8, 1);
        assert_eq!(bus.read_u16(REGION_ROM_START + 0xC4), 0xAAAA);
    }

    /// Emerald has one, but the registers stay invisible until it asks.
    #[test]
    fn a_clock_appears_only_once_the_game_switches_it_on() {
        let mut bus = SimpleBus::new(Some(cart(b"BPEE")));
        assert!(bus.has_clock());

        assert_eq!(
            bus.read_u16(REGION_ROM_START + 0xC4),
            0xAAAA,
            "before the control register is set these are still the cartridge"
        );

        bus.write_u16(REGION_ROM_START + 0xC8, 1);
        assert_eq!(bus.read_u16(REGION_ROM_START + 0xC8), 1);
        assert_eq!(
            bus.read_u16(REGION_ROM_START + 0xC4),
            0,
            "and now they are the chip, whose pins are all low"
        );
    }

    /// The registers are at cartridge offset 0xC4, and every region mirrors
    /// down to the same low offsets. Only the cartridge's is the clock.
    #[test]
    fn the_clock_does_not_capture_the_same_offset_in_other_regions() {
        let mut bus = SimpleBus::new(Some(cart(b"BPEE")));
        bus.write_u16(REGION_ROM_START + 0xC8, 1);

        bus.write_u16(REGION_EWRAM_START + 0xC4, 0x1234);
        assert_eq!(
            bus.read_u16(REGION_EWRAM_START + 0xC4),
            0x1234,
            "EWRAM at the mirrored offset is memory, not a clock register"
        );
    }

    /// A reset is not the same as pulling the cartridge out.
    #[test]
    fn reset_keeps_the_cartridge_and_its_battery() {
        let mut bus = SimpleBus::new(Some(vec![0xAA; 64]));
        bus.save_type = SaveType::Sram;
        bus.write_u8(0x0E00_0000, 0x42);
        assert_eq!(bus.read_u8(0x0E00_0000), 0x42);

        bus.reset();

        assert_eq!(bus.read_u8(REGION_ROM_START), 0xAA, "ROM survives a reset");
        assert_eq!(
            bus.read_u8(0x0E00_0000),
            0x42,
            "battery save survives a reset"
        );
    }

    /// A different cartridge brings its own battery.
    #[test]
    fn a_new_rom_wipes_the_previous_save() {
        let mut bus = SimpleBus::new(Some(vec![0xAA; 64]));
        bus.save_type = SaveType::Sram;
        bus.write_u8(0x0E00_0000, 0x42);

        bus.load_rom(vec![0xBB; 64]);
        bus.save_type = SaveType::Sram;

        assert_eq!(bus.read_u8(REGION_ROM_START), 0xBB);
        assert_eq!(
            bus.read_u8(0x0E00_0000),
            0xFF,
            "the new cartridge must not see the old one's save"
        );
    }

    #[test]
    fn test_rom_read() {
        let rom = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let bus = SimpleBus::new(Some(rom));

        assert_eq!(bus.read_u32(REGION_ROM_START), 0x03020100);
        assert_eq!(bus.read_u32(REGION_ROM_START + 4), 0x07060504);
    }

    /// Cartridge ROM does not mirror; past the end the bus answers with the
    /// address itself.
    ///
    /// This test used to assert the opposite, which is what an earlier
    /// `masked % rom_size` produced: a read beyond the cartridge came back with
    /// the cartridge's own opening bytes. Hardware leaves the data lines
    /// undriven, so what returns is the address the CPU put on the bus — the
    /// halfword at `addr` reads as `addr >> 1`. jsmolka's unsafe.gba checks
    /// exactly this.
    #[test]
    fn rom_reads_past_the_cartridge_return_the_address_not_a_mirror() {
        let rom = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let bus = SimpleBus::new(Some(rom));

        assert_eq!(bus.read_u32(REGION_ROM_START), 0xDDCCBBAA, "in range");

        // 0x08000004 is past the end: halfwords read as 0x0002 and 0x0003.
        assert_eq!(bus.read_u32(REGION_ROM_START + 4), 0x0003_0002);
        assert_eq!(bus.read_u16(REGION_ROM_START + 4), 0x0002);

        // Bytes take the low or high half of that halfword.
        assert_eq!(bus.read_u8(REGION_ROM_START + 4), 0x02);
        assert_eq!(bus.read_u8(REGION_ROM_START + 5), 0x00);
    }

    /// The eight-bit cartridge save bus repeats one byte across a wider access.
    #[test]
    fn a_wide_read_of_save_memory_repeats_one_byte() {
        let mut bus = SimpleBus::new(Some(vec![0; 4]));
        bus.save_type = SaveType::Sram;
        bus.write_u8(0x0E00_0000, 0x5A);

        assert_eq!(bus.read_u8(0x0E00_0000), 0x5A);
        assert_eq!(bus.read_u16(0x0E00_0000), 0x5A5A);
        assert_eq!(bus.read_u32(0x0E00_0000), 0x5A5A_5A5A);
    }

    /// A 32 KB SRAM chip mirrors halfway up its 64 KB window, and the window
    /// itself repeats to the top of the address space.
    #[test]
    fn save_memory_mirrors_the_way_the_chip_does() {
        let mut bus = SimpleBus::new(Some(vec![0; 4]));
        bus.save_type = SaveType::Sram;
        bus.save_memory = vec![0xFF; 0x8000];

        bus.write_u8(0x0E00_0000, 0x42);
        assert_eq!(bus.read_u8(0x0E00_8000), 0x42, "32 KB chip mirror");
        assert_eq!(bus.read_u8(0x0F00_0000), 0x42, "window mirror");
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
        // A byte write to palette lands on the whole halfword, because the
        // video bus is 16 bits wide and stores the byte in both halves.
        assert_eq!(palette, Some((0, 2)));
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
    fn test_take_ppu_io_dirty_range_tracks_cpu_mmio_but_not_internal_mirrors() {
        let mut bus = SimpleBus::new(None);

        bus.write_u16(0x0400_0008, 0x1234);
        bus.write_u8(0x0400_0045, 0x56);

        assert_eq!(bus.take_ppu_io_dirty_range(), Some((0x08, 0x46)));
        assert_eq!(bus.take_ppu_io_dirty_range(), None);

        bus.write_io_direct_u16(0x004, 0x1234);
        assert_eq!(bus.take_ppu_io_dirty_range(), None);
    }

    #[test]
    fn test_take_timer_control_dirty_tracks_tmcnt_h_cpu_writes_only() {
        let mut bus = SimpleBus::new(None);

        bus.write_u16(0x0400_0102, 0x0080);
        assert!(bus.take_timer_control_dirty());
        assert!(!bus.take_timer_control_dirty());

        bus.write_u16(0x0400_0100, 0x1234);
        assert!(!bus.take_timer_control_dirty());

        bus.write_u32(0x0400_0100, 0x0080_1234);
        assert!(bus.take_timer_control_dirty());

        bus.write_io_direct_u16(0x0102, 0x0080);
        assert!(!bus.take_timer_control_dirty());
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

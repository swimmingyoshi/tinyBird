//! Main GBA Emulator Struct
//!
//! This module provides the top-level GBA emulator structure that ties together
//! all the subsystems: CPU, memory, display, audio, input, etc.

use crate::apu::{Apu, LegacyApuV2};
use crate::bios::Bios;
use crate::bus::{Bus, SimpleBus, DEBUG_CYCLE, DEBUG_PC};
use crate::cpu::{Cpu, CpuMode};
use crate::debug::config as debug_config;
use crate::dma::DmaController;
use crate::input::Input;
use crate::ppu::Ppu;
use crate::scheduler::Scheduler;
use crate::timers::TimerController;
use serde::{Deserialize, Serialize};

/// GBA screen dimensions
#[allow(missing_docs)]
pub const SCREEN_WIDTH: u32 = 240;
#[allow(missing_docs)]
pub const SCREEN_HEIGHT: u32 = 160;

/// GBA clock speed (in Hz)
pub const CLOCK_SPEED: u32 = 16_777_216; // 16.78 MHz

/// Cycles per video frame (228 scanlines * 1232 cycles)
pub const CYCLES_PER_FRAME: u32 = crate::ppu::TOTAL_SCANLINES * crate::ppu::CYCLES_PER_SCANLINE;

/// Emulation speed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmulationSpeed {
    /// Full speed (no throttling)
    FullSpeed,
    /// Limit to specific FPS
    Limited(u32),
    /// Turbo mode (run as fast as possible)
    Turbo,
}

/// GBA state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GbaState {
    /// Running normally
    Running,
    /// Paused
    Paused,
    /// Stopped
    Stopped,
}

const SAVESTATE_MAGIC: &[u8; 4] = b"TBSV";
const SAVESTATE_VERSION: u32 = 4;

#[derive(Clone, Serialize, Deserialize)]
struct Savestate {
    cpu: Cpu,
    bus: SimpleBus,
    ppu: Ppu,
    apu: Apu,
    last_dispstat_status_bits: u16,
    last_vcount: u16,
    last_dispcnt: u16,
    last_dispstat_control: u16,
    bios_intr_wait_mask: Option<u16>,
    dma: DmaController,
    timers: TimerController,
    input: Input,
    scheduler: Scheduler,
    state: GbaState,
    speed: EmulationSpeed,
    audio_enabled: bool,
    total_cycles: u64,
    frame_count: u64,
    use_bios: bool,
}

#[derive(Serialize, Deserialize)]
struct LegacySavestateV2 {
    cpu: Cpu,
    bus: SimpleBus,
    ppu: Ppu,
    apu: LegacyApuV2,
    last_dispstat_status_bits: u16,
    last_vcount: u16,
    last_dispcnt: u16,
    last_dispstat_control: u16,
    bios_intr_wait_mask: Option<u16>,
    dma: DmaController,
    timers: TimerController,
    input: Input,
    scheduler: Scheduler,
    state: GbaState,
    speed: EmulationSpeed,
    audio_enabled: bool,
    total_cycles: u64,
    frame_count: u64,
    use_bios: bool,
}

#[derive(Serialize, Deserialize)]
struct LegacySavestateV1 {
    cpu: Cpu,
    bus: SimpleBus,
    ppu: Ppu,
    apu: LegacyApuV2,
    last_dispstat_status_bits: u16,
    last_vcount: u16,
    last_dispcnt: u16,
    last_dispstat_control: u16,
    dma: DmaController,
    timers: TimerController,
    input: Input,
    scheduler: Scheduler,
    state: GbaState,
    speed: EmulationSpeed,
    audio_enabled: bool,
    total_cycles: u64,
    frame_count: u64,
    use_bios: bool,
}

/// Main GBA emulator struct
/// How many cycles may build up before the APU is handed them.
///
/// The chip emits a sample every ~380 cycles and clocks its frame sequencer
/// every ~32768, so anything well under a sample period is inaudible. 64 keeps
/// the batch far finer than the chip's own resolution while cutting the number
/// of calls by roughly the average instruction length.
pub(crate) const APU_TICK_BATCH_CYCLES: u32 = 64;

#[derive(Clone, Serialize, Deserialize)]
pub struct Gba {
    /// CPU core
    pub cpu: Cpu,
    /// Memory bus
    pub bus: SimpleBus,
    /// Pixel processing unit
    pub ppu: Ppu,
    /// Audio processing unit
    pub apu: Apu,
    /// Cycles owed to the APU but not yet handed to it.
    ///
    /// See `flush_apu`: ticking the sound chip once per instruction was about
    /// a third of a frame, almost all of it fixed per-call overhead.
    apu_pending_cycles: u32,
    /// Last status bits mirrored into DISPSTAT.
    last_dispstat_status_bits: u16,
    /// Last scanline mirrored into VCOUNT.
    last_vcount: u16,
    /// Last software-controlled DISPCNT value synced into the PPU.
    last_dispcnt: u16,
    /// Last software-controlled DISPSTAT bits synced into the PPU.
    last_dispstat_control: u16,
    /// Active BIOS IntrWait/VBlankIntrWait mask while the HLE wait is pending.
    bios_intr_wait_mask: Option<u16>,
    /// DMA controller
    pub dma: DmaController,
    /// Timer controller
    pub timers: TimerController,
    /// Input handler
    pub input: Input,
    /// Event scheduler
    pub scheduler: Scheduler,
    /// Emulation state
    pub state: GbaState,
    /// Emulation speed setting
    pub speed: EmulationSpeed,
    /// Whether audio emulation is active.
    pub audio_enabled: bool,
    /// Total cycles executed
    pub total_cycles: u64,
    /// Frame counter
    pub frame_count: u64,
    /// Whether BIOS is being used
    pub use_bios: bool,
    /// The link cable.
    ///
    /// Not part of a savestate: the registers travel in the I/O array with
    /// every other register, and what is left here — which console this is and
    /// what is on the wire — describes a cable, not a machine. Restoring a
    /// state gives you a console that has just been unplugged.
    #[serde(skip)]
    pub sio: crate::sio::Sio,
}

impl From<&Gba> for Savestate {
    fn from(gba: &Gba) -> Self {
        Self {
            cpu: gba.cpu.clone(),
            bus: gba.bus.clone(),
            ppu: gba.ppu.clone(),
            apu: gba.apu.clone(),
            last_dispstat_status_bits: gba.last_dispstat_status_bits,
            last_vcount: gba.last_vcount,
            last_dispcnt: gba.last_dispcnt,
            last_dispstat_control: gba.last_dispstat_control,
            bios_intr_wait_mask: gba.bios_intr_wait_mask,
            dma: gba.dma.clone(),
            timers: gba.timers.clone(),
            input: gba.input.clone(),
            scheduler: gba.scheduler.clone(),
            state: gba.state,
            speed: gba.speed,
            audio_enabled: gba.audio_enabled,
            total_cycles: gba.total_cycles,
            frame_count: gba.frame_count,
            use_bios: gba.use_bios,
        }
    }
}

impl From<Savestate> for Gba {
    fn from(mut state: Savestate) -> Self {
        // A version 4 state left these out; an older one has them already.
        state.ppu.framebuffer.ensure_sized();
        Self {
            cpu: state.cpu,
            bus: state.bus,
            ppu: state.ppu,
            // Transient bookkeeping, not part of the saved machine: a restored
            // console owes the sound chip nothing.
            apu_pending_cycles: 0,
            apu: state.apu,
            last_dispstat_status_bits: state.last_dispstat_status_bits,
            last_vcount: state.last_vcount,
            last_dispcnt: state.last_dispcnt,
            last_dispstat_control: state.last_dispstat_control,
            bios_intr_wait_mask: state.bios_intr_wait_mask,
            dma: state.dma,
            timers: state.timers,
            input: state.input,
            scheduler: state.scheduler,
            sio: crate::sio::Sio::new(),
            state: state.state,
            speed: state.speed,
            audio_enabled: state.audio_enabled,
            total_cycles: state.total_cycles,
            frame_count: state.frame_count,
            use_bios: state.use_bios,
        }
    }
}

impl From<LegacySavestateV2> for Gba {
    fn from(state: LegacySavestateV2) -> Self {
        Self {
            cpu: state.cpu,
            bus: state.bus,
            ppu: state.ppu,
            // Transient bookkeeping, not part of the saved machine: a restored
            // console owes the sound chip nothing.
            apu_pending_cycles: 0,
            apu: state.apu.into(),
            last_dispstat_status_bits: state.last_dispstat_status_bits,
            last_vcount: state.last_vcount,
            last_dispcnt: state.last_dispcnt,
            last_dispstat_control: state.last_dispstat_control,
            bios_intr_wait_mask: state.bios_intr_wait_mask,
            dma: state.dma,
            timers: state.timers,
            input: state.input,
            scheduler: state.scheduler,
            sio: crate::sio::Sio::new(),
            state: state.state,
            speed: state.speed,
            audio_enabled: state.audio_enabled,
            total_cycles: state.total_cycles,
            frame_count: state.frame_count,
            use_bios: state.use_bios,
        }
    }
}

impl From<&Gba> for LegacySavestateV2 {
    fn from(gba: &Gba) -> Self {
        Self {
            cpu: gba.cpu.clone(),
            bus: gba.bus.clone(),
            ppu: gba.ppu.clone(),
            apu: LegacyApuV2::from(&gba.apu),
            last_dispstat_status_bits: gba.last_dispstat_status_bits,
            last_vcount: gba.last_vcount,
            last_dispcnt: gba.last_dispcnt,
            last_dispstat_control: gba.last_dispstat_control,
            bios_intr_wait_mask: gba.bios_intr_wait_mask,
            dma: gba.dma.clone(),
            timers: gba.timers.clone(),
            input: gba.input.clone(),
            scheduler: gba.scheduler.clone(),
            state: gba.state,
            speed: gba.speed,
            audio_enabled: gba.audio_enabled,
            total_cycles: gba.total_cycles,
            frame_count: gba.frame_count,
            use_bios: gba.use_bios,
        }
    }
}

impl From<&Gba> for LegacySavestateV1 {
    fn from(gba: &Gba) -> Self {
        Self {
            cpu: gba.cpu.clone(),
            bus: gba.bus.clone(),
            ppu: gba.ppu.clone(),
            apu: LegacyApuV2::from(&gba.apu),
            last_dispstat_status_bits: gba.last_dispstat_status_bits,
            last_vcount: gba.last_vcount,
            last_dispcnt: gba.last_dispcnt,
            last_dispstat_control: gba.last_dispstat_control,
            dma: gba.dma.clone(),
            timers: gba.timers.clone(),
            input: gba.input.clone(),
            scheduler: gba.scheduler.clone(),
            state: gba.state,
            speed: gba.speed,
            audio_enabled: gba.audio_enabled,
            total_cycles: gba.total_cycles,
            frame_count: gba.frame_count,
            use_bios: gba.use_bios,
        }
    }
}

impl From<LegacySavestateV1> for Gba {
    fn from(state: LegacySavestateV1) -> Self {
        Self {
            cpu: state.cpu,
            bus: state.bus,
            ppu: state.ppu,
            // Transient bookkeeping, not part of the saved machine: a restored
            // console owes the sound chip nothing.
            apu_pending_cycles: 0,
            apu: state.apu.into(),
            last_dispstat_status_bits: state.last_dispstat_status_bits,
            last_vcount: state.last_vcount,
            last_dispcnt: state.last_dispcnt,
            last_dispstat_control: state.last_dispstat_control,
            bios_intr_wait_mask: None,
            dma: state.dma,
            timers: state.timers,
            input: state.input,
            scheduler: state.scheduler,
            sio: crate::sio::Sio::new(),
            state: state.state,
            speed: state.speed,
            audio_enabled: state.audio_enabled,
            total_cycles: state.total_cycles,
            frame_count: state.frame_count,
            use_bios: state.use_bios,
        }
    }
}

impl Default for Gba {
    fn default() -> Self {
        Self::new()
    }
}

impl Gba {
    /// Create a new GBA emulator
    pub fn new() -> Self {
        Self {
            cpu: Cpu::new(),
            bus: SimpleBus::new(None),
            ppu: Ppu::new(),
            apu: Apu::new(),
            last_dispstat_status_bits: u16::MAX,
            last_vcount: u16::MAX,
            last_dispcnt: u16::MAX,
            last_dispstat_control: u16::MAX,
            bios_intr_wait_mask: None,
            dma: DmaController::new(),
            timers: TimerController::new(),
            input: Input::new(),
            scheduler: Scheduler::new(),
            sio: crate::sio::Sio::new(),
            state: GbaState::Stopped,
            speed: EmulationSpeed::FullSpeed,
            audio_enabled: true,
            apu_pending_cycles: 0,
            total_cycles: 0,
            frame_count: 0,
            use_bios: false,
        }
    }

    /// Create a new GBA with ROM data
    pub fn with_rom(rom: Vec<u8>) -> Self {
        let mut gba = Self::new();
        gba.load_rom(rom);
        gba
    }

    /// Load a ROM file
    pub fn load_rom(&mut self, rom: Vec<u8>) {
        self.bus.load_rom(rom);
        self.reset();
    }

    /// Load a BIOS file
    pub fn load_bios(&mut self, bios: Vec<u8>) {
        self.bus.load_bios(&bios);
        self.use_bios = true;
    }

    /// Reset the emulator
    ///
    /// A power-on reset: the bus clears work RAM, video memory, and the I/O
    /// registers, so a game always starts on a machine in a known state rather
    /// than on whatever the previous cartridge left behind.
    pub fn reset(&mut self) {
        self.bus.reset();
        self.cpu.reset();
        self.ppu.reset();
        self.scheduler.clear();
        self.total_cycles = 0;
        self.frame_count = 0;
        self.state = GbaState::Running;
        self.apu.reset();
        // Peripherals hold their own copies of the control registers, so
        // clearing I/O is not enough to stop a transfer the last game armed.
        self.dma = DmaController::new();
        self.timers = TimerController::new();
        self.last_dispstat_status_bits = u16::MAX;
        self.last_vcount = u16::MAX;
        self.last_dispcnt = u16::MAX;
        self.last_dispstat_control = u16::MAX;
        self.bios_intr_wait_mask = None;

        // Start the PPU one cycle ahead to avoid pathological CPU/PPU lockstep
        // in tight polling loops that sample IRQ flags at a fixed phase.
        self.ppu.cycle = 1;

        // Initialize HALTCNT (I/O offset 0x301) to 0xFF so the HALT detector
        // doesn't fire spuriously from zero-initialized I/O memory.
        self.bus.write_io_direct(0x301, 0xFF);

        // Set initial PC based on BIOS usage
        if self.use_bios {
            self.cpu.pipeline.set_fetch_addr(0x00000000);
        } else {
            // Skip BIOS - start at ROM entry point and initialize registers
            self.cpu.pipeline.set_fetch_addr(0x08000000);

            // Initialize stack pointers for all modes (GBA default values)
            self.cpu.registers.switch_mode(CpuMode::Supervisor);
            self.cpu.registers.set_sp(0x03007FE0);
            self.cpu.registers.switch_mode(CpuMode::IRQ);
            self.cpu.registers.set_sp(0x03007FA0);
            self.cpu.registers.switch_mode(CpuMode::User);
            self.cpu.registers.set_sp(0x03007F00);

            // Set PC to ROM start
            self.cpu.registers.set_pc(0x08000000);

            // Simulate post-BIOS state: BIOS clears I and F flags before jumping to ROM,
            // enabling IRQ and FIQ.  Without this the game never receives interrupts.
            let cpsr = self.cpu.registers.cpsr();
            self.cpu.registers.set_cpsr(cpsr & !(0x80 | 0x40), true); // clear I (bit7) and F (bit6)
        }
    }

    /// Start emulation
    pub fn start(&mut self) {
        if self.state == GbaState::Stopped {
            self.reset();
        }
        self.state = GbaState::Running;
    }

    /// Pause emulation
    pub fn pause(&mut self) {
        self.state = GbaState::Paused;
    }

    /// Stop emulation
    pub fn stop(&mut self) {
        self.state = GbaState::Stopped;
    }

    /// Run a single instruction
    pub fn step(&mut self) {
        if self.state != GbaState::Running {
            return;
        }

        self.apply_bios_intr_wait_gate();

        // Sync PPU runtime status to I/O (must be current for polling games).
        // Preserve writable DISPSTAT bits set by software (IRQ enables + VCOUNT compare).
        let ppu_status_bits = self.ppu.read_dispstat_low() & 0x0007;
        if self.last_dispstat_status_bits != ppu_status_bits {
            let current_dispstat = self.bus.read_io_direct_u16(0x004);
            let preserved_writable = current_dispstat & !0x0007;
            self.bus
                .write_io_direct_u16(0x004, preserved_writable | ppu_status_bits);
            self.last_dispstat_status_bits = ppu_status_bits;
        }

        let vcount = self.ppu.scanline as u16;
        if self.last_vcount != vcount {
            self.bus.write_io_direct_u16(0x006, vcount);
            self.last_vcount = vcount;
        }

        // Sync input (only needs to be once per frame, but do it often enough)
        if (self.total_cycles & 63) == 0 {
            let keyinput = self.input.read_keyinput();
            let ki_bytes = keyinput.to_le_bytes();
            self.bus.write_io_direct(0x130, ki_bytes[0]);
            self.bus.write_io_direct(0x131, ki_bytes[1]);
        }

        let dbg = debug_config();
        let pc_before = self.cpu.fetch_addr();
        let pending_intr_wait = self
            .current_hle_swi_comment(pc_before)
            .filter(|comment| matches!(comment, 0x04 | 0x05));
        // Only pay TLS write cost when at least one debug trace mode is active
        if dbg.trace_range.is_some() || dbg.watch_addr.is_some() || dbg.pc_trace {
            DEBUG_CYCLE.with(|c| c.set(self.total_cycles));
            DEBUG_PC.with(|c| c.set(pc_before));
        }
        // Detailed CPU instruction trace for a specific cycle range
        if let Some((start, end)) = dbg.trace_range {
            if self.total_cycles >= start && self.total_cycles <= end {
                let thumb = self.cpu.is_thumb_mode();
                let pc = pc_before;
                let opcode = if thumb {
                    self.bus.read_u16(pc) as u32
                } else {
                    self.bus.read_u32(pc)
                };
                eprintln!("cy={} pc={:08x} {} op={:08x} r0={:08x} r1={:08x} r2={:08x} r3={:08x} r13={:08x} r14={:08x}",
                    self.total_cycles, pc, if thumb { "T" } else { "A" }, opcode,
                    self.cpu.registers.get_reg(0), self.cpu.registers.get_reg(1),
                    self.cpu.registers.get_reg(2), self.cpu.registers.get_reg(3),
                    self.cpu.registers.get_reg(13), self.cpu.registers.get_reg(14));
            }
        }
        let cpu_cycles = self.cpu.step(&mut self.bus) as u64;
        self.total_cycles += cpu_cycles;
        if self.audio_enabled {
            self.sync_io_to_apu();
        }

        // Check if HALT was requested via HALTCNT (I/O offset 0x301).
        // The BIOS SWI 0x02 handler writes 0x00 there; we detect it here and
        // freeze the CPU. A value of 0xFF means no halt is pending.
        if !self.cpu.halted && self.bus.read_io_direct(0x301) == 0x00 {
            self.cpu.halted = true;
            self.bus.write_io_direct(0x301, 0xFF); // clear the request
            self.bios_intr_wait_mask = pending_intr_wait
                .map(|_| (self.cpu.registers.get_reg(1) & 0x3FFF) as u16)
                .filter(|mask| *mask != 0);
        }

        // Wake from HALT: any pending interrupt (IE & IF) resumes the CPU,
        // regardless of IME or the I flag in CPSR.
        if self.cpu.halted {
            let pending = self.pending_enabled_irq_bits();
            if pending != 0 {
                if let Some(mask) = self.bios_intr_wait_mask {
                    if self.cpu.registers.mode() == CpuMode::IRQ {
                        self.cpu.halted = false;
                    } else if (pending & mask) != 0 {
                        self.consume_bios_intr_wait(mask);
                        self.cpu.halted = false;
                    }
                } else {
                    self.cpu.halted = false;
                }
            }
        }
        // Track PC region changes for debugging
        if dbg.pc_trace {
            let pc_after = self.cpu.fetch_addr();
            fn is_valid_exec(pc: u32) -> bool {
                // BIOS (0x0-0x3FFF), EWRAM (0x02000000-0x0203FFFF),
                // IWRAM code area (0x03000000-0x03006FFF, below stacks),
                // ROM (0x08000000-0x0DFFFFFF)
                matches!(pc, 0x0000_0000..=0x0000_3FFF
                    | 0x0200_0000..=0x0203_FFFF
                    | 0x0300_0000..=0x0300_7EFF
                    | 0x0800_0000..=0x0DFF_FFFF)
            }
            let was_valid = is_valid_exec(pc_before);
            let is_valid = is_valid_exec(pc_after);
            if was_valid && !is_valid {
                eprintln!(
                    "PC WENT INVALID: {:08x} -> {:08x} (cycle {})",
                    pc_before, pc_after, self.total_cycles
                );
                for r in 0..16 {
                    eprint!("  r{}={:08x}", r, self.cpu.registers.get_reg(r));
                }
                eprintln!("  cpsr={:08x}", self.cpu.registers.cpsr());
            }
        }

        // Sync timer controls only when the CPU actually wrote TMxCNT_H.
        if self.bus.take_timer_control_dirty() {
            self.sync_timer_controls();
        }

        // The link cable, and only while one is plugged in.
        //
        // This is the interpreter's inner loop, so the guard is not tidiness:
        // reaching into the bus for a flag on every instruction cost about 9%
        // of emulation speed even though the flag is almost never set, and a
        // single-player game should not pay for a cable it does not have. The
        // field checked here is on `Gba`, which the loop is already holding.
        if self.sio.connected() {
            // The game touching a serial register is the only thing that can
            // start a transfer or change what the hardware bits should read.
            if self.bus.take_sio_dirty() {
                self.sync_sio();
            }

            // Run the cable for this instruction. A transfer that lands writes
            // the other consoles into SIOMULTI0-3 and lets the game go on.
            if let Some(values) = self.sio.tick(cpu_cycles as u32) {
                self.finish_link_transfer(values);
            }
        }

        // Tick timers for the number of cycles the instruction consumed.
        self.tick_timers(cpu_cycles as u32);

        // Audio is ticked in batches rather than once per instruction.
        //
        // `Apu::tick` walks four channels, the frame sequencer and the sample
        // generator on every call. Called per instruction — a hundred thousand
        // times a frame, usually with one to five cycles — almost all of that
        // is fixed overhead: the chip only emits a sample every ~380 cycles and
        // clocks its sequencer every ~32768, so the fine granularity bought
        // nothing. Batching was worth about a third of a frame.
        //
        // The batch is flushed before any register write reaches the APU, so a
        // game that changes a frequency still hears it change at the right
        // point in the waveform.
        if self.audio_enabled {
            self.apu_pending_cycles += cpu_cycles as u32;
            if self.apu_pending_cycles >= APU_TICK_BATCH_CYCLES || self.bus.audio_io_dirty() {
                self.flush_apu();
            }
        }

        // Check for DMA transfers only when a DMA register was written
        if self.bus.take_dma_dirty() {
            self.check_dma();
        }

        // Step PPU (runs at same clock speed as CPU).
        // Sync display/register state after the CPU/DMA work for this instruction
        // so mid-frame MMIO writes (like FireRed's WIN0 updates in the options
        // menu) take effect on the very next pixels we render.
        let prev_scanline = self.ppu.scanline;
        let prev_cycle = self.ppu.cycle;
        self.sync_dirty_ppu_control_regs_to_ppu();
        let crosses_hblank_start = self.ppu.scanline < crate::ppu::VISIBLE_SCANLINES
            && prev_cycle < crate::ppu::HBLANK_START
            && prev_cycle + cpu_cycles as u32 >= crate::ppu::HBLANK_START;
        if crosses_hblank_start {
            self.sync_video_memory();
        }
        let crossed_scanline_boundary =
            prev_cycle + cpu_cycles as u32 >= crate::ppu::CYCLES_PER_SCANLINE;
        let ppu_events = self.ppu.step_cycles(cpu_cycles as u32);
        if !ppu_events.is_empty() {
            let mut lcd_irq_mask = 0u16;

            if ppu_events.contains(crate::ppu::PpuEvent::VBlank) {
                self.dma.on_vblank();
                self.run_pending_dma_channels();

                // Set IF bit 0 (VBlank request) when DISPSTAT bit 3 enables it.
                let dispstat = self.bus.read_io_direct_u16(0x004);
                if (dispstat & (1 << 3)) != 0 {
                    lcd_irq_mask |= 0x0001;
                }
            }

            if ppu_events.contains(crate::ppu::PpuEvent::HBlank) {
                if prev_scanline < crate::ppu::VISIBLE_SCANLINES {
                    self.dma.on_hblank();
                    self.run_pending_dma_channels();
                }

                // Set IF bit 1 (HBlank request) when DISPSTAT bit 4 enables it.
                let dispstat = self.bus.read_io_direct_u16(0x004);
                if (dispstat & (1 << 4)) != 0 {
                    lcd_irq_mask |= 0x0002;
                }
            }

            if ppu_events.contains(crate::ppu::PpuEvent::VCounter) {
                // Set IF bit 2 (VCount match request) when DISPSTAT bit 5 enables it.
                let dispstat = self.bus.read_io_direct_u16(0x004);
                if (dispstat & (1 << 5)) != 0 {
                    lcd_irq_mask |= 0x0004;
                }
            }

            if ppu_events.contains(crate::ppu::PpuEvent::FrameComplete) {
                // Sync everything at frame boundary
                self.sync_io_to_ppu();
                self.frame_count += 1;
            }

            if lcd_irq_mask != 0 {
                self.request_irq(lcd_irq_mask);
                let ime = self.bus.read_io_direct_u16(0x208);
                let ie = self.bus.read_io_direct_u16(0x200);
                if dbg.irq_debug {
                    eprintln!(
                        "[LCDIRQ] pc={:08x} mask={:04x} ime={:04x} ie={:04x} -> fire={}",
                        self.cpu.fetch_addr(),
                        lcd_irq_mask,
                        ime,
                        ie,
                        ime != 0 && (ie & lcd_irq_mask) != 0
                    );
                }
                if ime != 0 && (ie & lcd_irq_mask) != 0 {
                    self.cpu.irq();
                }
            }
        }
        // Sync the rest of the PPU register set at the start of each scanline.
        if crossed_scanline_boundary {
            self.sync_video_memory();
        }

        self.scheduler.advance(cpu_cycles);

        // General IRQ check: fire if any pending interrupt is enabled
        // (handles cases where IF/IE/IME changed outside of hardware event handlers)
        if !self.cpu.halted {
            let if_reg = self.bus.read_io_direct_u16(0x202);
            let ie = self.bus.read_io_direct_u16(0x200);
            let ime = self.bus.read_io_direct_u16(0x208);
            if ime != 0 && (ie & if_reg) != 0 {
                self.cpu.irq();
            }
        }
    }

    /// Run for one frame
    pub fn run_frame(&mut self) -> u64 {
        if self.state != GbaState::Running {
            return 0;
        }

        let start_cycles = self.total_cycles;
        let target_frame = self.frame_count + 1;

        while self.frame_count < target_frame {
            self.step();
            // A link transfer that has begun and is waiting on the other
            // consoles ends the frame here, part-way through.
            //
            // Without this the game sets START and the rest of the frame runs
            // anyway, so by the time the host notices, up to a frame of
            // emulated time has passed inside a transfer that hardware
            // finishes in about a third of a scanline. Stopping on the spot is
            // what lets the host freeze the console for exactly as long as the
            // network takes and no longer. The frame is finished on the next
            // call, once the data is in.
            if self.sio.transfer_pending() {
                break;
            }
        }

        // Hand the APU what it is owed before the frame ends. Every caller
        // drains audio after running a frame, so settling here means none of
        // them has to know the chip is ticked in batches — a buffer short by
        // up to a batch, with the gap landing somewhere different each time,
        // is not a bug anyone would enjoy finding.
        self.settle_audio();

        let ran = self.total_cycles - start_cycles;
        // The cartridge clock keeps its own time. A host that pushes the wall
        // clock in every frame overrides this immediately; one that pushes
        // once gets a clock that runs with the emulation, which is what a
        // cartridge being fast forwarded should experience.
        self.bus.step_rtc(ran);
        ran
    }

    /// Take the cartridge out and stop the machine.
    ///
    /// A reset restarts the game that is in; this is the other thing — there is
    /// no game in any more. Everything the cartridge brought with it goes with
    /// it: its save memory, its backup type, its clock. The state is `Stopped`
    /// rather than `Paused`, because paused is a game you can resume.
    pub fn eject(&mut self) {
        self.reset();
        self.bus.eject_rom();
        self.state = GbaState::Stopped;
        self.ppu.framebuffer.clear();
    }

    /// Tell the cartridge clock what the time is, in seconds since the epoch.
    ///
    /// Only Ruby, Sapphire and Emerald have a clock to tell; on anything else
    /// this does nothing. Call it as often as the host likes — every frame
    /// keeps the cartridge on true wall-clock time, which is what berry growth
    /// and the tides in Shoal Cave are counting.
    ///
    /// The core cannot read the clock itself: it runs on
    /// `wasm32-unknown-unknown`, where `SystemTime::now` panics.
    pub fn set_wall_clock(&mut self, unix_seconds: i64) {
        self.bus.set_wall_clock(unix_seconds);
    }

    /// Whether the cartridge in the machine has a clock chip on it.
    pub fn has_cartridge_clock(&self) -> bool {
        self.bus.has_clock()
    }

    /// Run for one frame, stopping if the cartridge fails to reach a frame boundary.
    pub fn run_frame_with_budget(&mut self, max_steps: u64) -> Option<u64> {
        if self.state != GbaState::Running {
            return Some(0);
        }

        let start_cycles = self.total_cycles;
        let target_frame = self.frame_count + 1;
        let mut steps = 0;

        while self.frame_count < target_frame {
            if steps >= max_steps {
                return None;
            }
            self.step();
            steps += 1;
            // Same as `run_frame`: a transfer waiting on the other consoles
            // ends the slice here, so the host can act on it at once.
            if self.sio.transfer_pending() {
                return None;
            }
        }

        let ran = self.total_cycles - start_cycles;
        // As in `run_frame`: the cartridge clock keeps its own time, and a
        // host that never pushes the wall clock still gets a clock that runs.
        self.bus.step_rtc(ran);
        Some(ran)
    }

    /// Run for multiple frames
    pub fn run_frames(&mut self, frames: u32) -> u64 {
        let mut total = 0;
        for _ in 0..frames {
            total += self.run_frame();
        }
        total
    }

    /// Get the current PC
    pub fn pc(&self) -> u32 {
        self.cpu.pc()
    }

    /// Serialize the full emulator state into a savestate blob.
    pub fn save_state_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        let mut state = Savestate::from(self);
        // The cartridge is left out from version 4 on. It dwarfs the rest of
        // the machine — 16MB of Fire Red against about 400KB of RAM, video
        // memory and registers — so including it made every state twenty times
        // larger than it needed to be, slow to upload and slow to store, to
        // carry bytes the player already has. Restoring puts back the ROM
        // that is in the machine at the time.
        state.bus.take_rom();
        // Same reasoning for the rendered picture: it is drawn again from
        // video memory the moment the state starts running.
        state.ppu.framebuffer.forget_pixels();
        let payload = bincode::serialize(&state)?;
        let mut bytes = Vec::with_capacity(SAVESTATE_MAGIC.len() + 4 + payload.len());
        bytes.extend_from_slice(SAVESTATE_MAGIC);
        bytes.extend_from_slice(&SAVESTATE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// Restore a full emulator state from a savestate blob.
    pub fn load_state_bytes(&mut self, bytes: &[u8]) -> Result<(), bincode::Error> {
        if bytes.len() >= SAVESTATE_MAGIC.len() + 4
            && &bytes[..SAVESTATE_MAGIC.len()] == SAVESTATE_MAGIC
        {
            let version_offset = SAVESTATE_MAGIC.len();
            let version = u32::from_le_bytes(
                bytes[version_offset..version_offset + 4]
                    .try_into()
                    .expect("savestate header slice has fixed length"),
            );
            return match version {
                4 => {
                    let mut state: Savestate = bincode::deserialize(&bytes[version_offset + 4..])?;
                    // The state carries no cartridge, so it comes from the
                    // machine. Loading one without a game in is the one case
                    // this format cannot serve, and it is worth saying so
                    // rather than restoring a console with an empty slot.
                    let rom = self.bus.take_rom();
                    if rom.is_empty() {
                        return Err(Box::new(bincode::ErrorKind::Custom(
                            "load the cartridge before restoring one of its savestates".to_string(),
                        )));
                    }
                    state.bus.restore_rom(rom);
                    *self = state.into();
                    Ok(())
                }
                3 => {
                    let state: Savestate = bincode::deserialize(&bytes[version_offset + 4..])?;
                    *self = state.into();
                    Ok(())
                }
                2 => {
                    let state: LegacySavestateV2 =
                        bincode::deserialize(&bytes[version_offset + 4..])?;
                    *self = state.into();
                    Ok(())
                }
                _ => Err(Box::new(bincode::ErrorKind::Custom(format!(
                    "unsupported savestate version {}",
                    version
                )))),
            };
        }

        if let Ok(state) = bincode::deserialize::<Savestate>(bytes) {
            *self = state.into();
            return Ok(());
        }

        if let Ok(state) = bincode::deserialize::<LegacySavestateV2>(bytes) {
            *self = state.into();
            return Ok(());
        }

        let legacy: LegacySavestateV1 = bincode::deserialize(bytes)?;
        *self = legacy.into();
        Ok(())
    }

    /// Return whether the current cartridge exposes persistent save memory.
    pub fn has_persistent_save(&self) -> bool {
        self.bus.has_persistent_save()
    }

    /// Return a snapshot of cartridge-backed save memory.
    ///
    /// Empty when the cartridge has no backup chip. EEPROM is included: it is
    /// battery-backed like the rest, it just is not memory-mapped.
    pub fn save_data(&self) -> Vec<u8> {
        self.bus.save_data()
    }

    /// Load cartridge-backed save memory from external storage.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.bus.load_save_data(data);
    }

    /// Return whether cartridge-backed save memory changed since the last flush.
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.take_save_dirty()
    }

    /// Get the current CPSR
    pub fn cpsr(&self) -> u32 {
        self.cpu.registers.cpsr()
    }

    /// Get a register value
    pub fn get_register(&self, index: usize) -> u32 {
        self.cpu.registers.get_reg(index)
    }

    /// Set a register value
    pub fn set_register(&mut self, index: usize, value: u32) {
        self.cpu.registers.set_reg(index, value);
    }

    /// Read memory at address
    pub fn read_u8(&self, addr: u32) -> u8 {
        self.bus.read_u8(addr)
    }

    /// Read memory as u16
    pub fn read_u16(&self, addr: u32) -> u16 {
        self.bus.read_u16(addr)
    }

    /// Read memory as u32
    pub fn read_u32(&self, addr: u32) -> u32 {
        self.bus.read_u32(addr)
    }

    /// Write memory
    pub fn write_u8(&mut self, addr: u32, value: u8) {
        self.bus.write_u8(addr, value);
    }

    /// Write memory as u16
    pub fn write_u16(&mut self, addr: u32, value: u16) {
        self.bus.write_u16(addr, value);
    }

    /// Write memory as u32
    pub fn write_u32(&mut self, addr: u32, value: u32) {
        self.bus.write_u32(addr, value);
    }

    /// Get emulation status
    pub fn status(&self) -> GbaStatus {
        GbaStatus {
            state: self.state,
            pc: self.pc(),
            cpsr: self.cpsr(),
            total_cycles: self.total_cycles,
            frame_count: self.frame_count,
            cycles_per_second: if self.total_cycles > 0 {
                // Would need timing info for accurate calculation
                0
            } else {
                0
            },
        }
    }

    /// Schedule a VBlank interrupt
    pub fn schedule_vblank(&mut self, cycles: u64) {
        self.scheduler
            .schedule(crate::scheduler::EventType::VBlank, cycles);
    }

    /// Schedule an HBlank interrupt
    pub fn schedule_hblank(&mut self, cycles: u64) {
        self.scheduler
            .schedule(crate::scheduler::EventType::HBlank, cycles);
    }

    /// Act on a write to a link-cable register.
    ///
    /// Two things happen here. The parent setting `START` is what begins a
    /// transfer, and every write has the hardware-owned bits put back, because
    /// a game may write anything to them and must read back the truth.
    fn sync_sio(&mut self) {
        use crate::sio::{PortMode, SerialMode, MAX_PLAYERS, NO_DATA, RCNT, SIOCNT, SIOMULTI0};

        let rcnt = self.bus.read_io_direct_u16(RCNT);
        let siocnt = self.bus.read_io_direct_u16(SIOCNT);

        // Transfers are only carried in multi-player mode. The cable's own
        // bits are imposed either way, at the end.
        if PortMode::from_rcnt(rcnt) == PortMode::Serial
            && SerialMode::from_control(siocnt) == SerialMode::MultiPlayer
        {
            // Bit 7 is the parent asking for a transfer.
            if siocnt & 0x0080 != 0 && self.sio.start_transfer() {
                // Every slot blanks when a transfer starts, so a console that
                // is not there reads as absent rather than as having sent
                // zero, which a game would take for real data.
                for slot in 0..MAX_PLAYERS {
                    self.bus.write_io_direct_u16(SIOMULTI0 + slot * 2, NO_DATA);
                }
            }
        }

        self.refresh_link_control();
    }

    /// Land a transfer: the other consoles arrive and the game is let go.
    fn finish_link_transfer(&mut self, values: [u16; crate::sio::MAX_PLAYERS]) {
        use crate::sio::{IRQ_BIT, SIOCNT, SIOMULTI0};

        for (slot, value) in values.iter().enumerate() {
            self.bus.write_io_direct_u16(SIOMULTI0 + slot * 2, *value);
        }

        // Clearing START is what the waiting game is polling for.
        let siocnt = self.bus.read_io_direct_u16(SIOCNT);
        self.refresh_link_control();

        // Bit 14 asks for an interrupt rather than a poll.
        if siocnt & 0x4000 != 0 {
            self.request_irq(IRQ_BIT);
        }
    }

    /// Plug this console into a cable as `id`, one of `players` consoles.
    ///
    /// Console 0 is the parent: the only one that may start a transfer.
    pub fn link_connect(&mut self, id: u8, players: u8) {
        self.sio.connect(id, players);
        self.refresh_link_control();
    }

    /// Unplug the cable.
    ///
    /// A transfer in flight is abandoned. The game sees `START` clear with the
    /// data slots still blank, which is what it would see if somebody pulled
    /// the cable out, and every game already copes with that.
    pub fn link_disconnect(&mut self) {
        self.sio.disconnect();
        self.refresh_link_control();
    }

    /// Put the bits the cable owns back into `SIOCNT`.
    ///
    /// The terminals — which end of the cable this is, whether anybody is on
    /// the other end, this console's player number — describe the wire, not
    /// the protocol, and read the same whichever serial mode is selected.
    ///
    /// Imposing them only in multi-player mode was a real bug with a strange
    /// shape: a game asking "is a cable plugged in?" during its setup, which
    /// Fire Red does while briefly in normal mode, was told there was nothing
    /// there. Switching the cable off and on fixed it, because that path wrote
    /// the bits regardless of mode — which is exactly the workaround players
    /// found.
    fn refresh_link_control(&mut self) {
        use crate::sio::{PortMode, SerialMode, RCNT, SIOCNT};

        // The port being used for something else entirely — general-purpose
        // pins, or the JOY bus — is not a link cable and owns none of this.
        if PortMode::from_rcnt(self.bus.read_io_direct_u16(RCNT)) != PortMode::Serial {
            return;
        }

        let siocnt = self.bus.read_io_direct_u16(SIOCNT);
        let mut corrected = self.sio.apply_terminal_bits(siocnt);
        if SerialMode::from_control(siocnt) == SerialMode::MultiPlayer {
            corrected = self.sio.apply_busy_bit(corrected);
        }

        if corrected != siocnt {
            self.bus.write_io_direct_u16(SIOCNT, corrected);
        }
    }

    /// Whether this console has a transfer waiting on the others.
    pub fn link_transfer_pending(&self) -> bool {
        self.sio.transfer_pending()
    }

    /// Give up on a transfer nobody is going to answer.
    ///
    /// The data slots keep whatever they were blanked to, so the game reads an
    /// absent partner and takes its own view of that — which is the same thing
    /// it would see if the cable had been pulled out mid-transfer.
    pub fn link_abandon(&mut self) -> bool {
        let abandoned = self.sio.abandon();
        if abandoned {
            self.refresh_link_control();
        }
        abandoned
    }

    /// Take part in a transfer the parent has begun.
    ///
    /// Returns whether this console joined, which is false for the parent and
    /// for anyone already busy.
    pub fn link_join(&mut self) -> bool {
        // The previous transfer may still be clocking when the parent asks for
        // the next one — it has its own clock and does not wait for ours.
        // Finish it rather than let the new one overwrite it, which would drop
        // a halfword the game is counting on.
        if let Some(values) = self.sio.take_clocking() {
            self.finish_link_transfer(values);
        }

        let joined = self.sio.join_transfer();
        if joined {
            self.refresh_link_control();
        }
        joined
    }

    /// What this console is putting on the wire.
    pub fn link_send_value(&self) -> u16 {
        self.bus.read_io_direct_u16(crate::sio::SIOMLT_SEND)
    }

    /// How long a transfer on this console's cable takes, in cycles.
    ///
    /// Only meaningful on the parent, which is the console that clocks the
    /// cable. The host reads it there and passes it to everyone.
    pub fn link_transfer_cycles(&self) -> u32 {
        let siocnt = self.bus.read_io_direct_u16(crate::sio::SIOCNT);
        crate::sio::Sio::transfer_cycles(siocnt, self.sio.players())
    }

    /// Hand back what every console sent.
    ///
    /// `cycles` is the parent's transfer time; see [`crate::sio::Sio::deliver`].
    /// Returns whether the console took it. A console that was not waiting for
    /// this transfer says so rather than silently swallowing it.
    pub fn link_deliver(&mut self, values: [u16; crate::sio::MAX_PLAYERS], cycles: u32) -> bool {
        self.sio.deliver(values, cycles)
    }

    /// Sync timer control registers from I/O (call when timer control regs may have changed)
    fn sync_timer_controls(&mut self) {
        for i in 0..4 {
            let cnt_h_offset = 0x102 + i * 4;
            let io_control = self.bus.read_io_direct_u16(cnt_h_offset);
            if self.timers.timers[i].control != io_control {
                let was_enabled = self.timers.timers[i].is_enabled();
                if !was_enabled && (io_control & 0x80) != 0 {
                    let io_reload = self.bus.read_io_direct_u16(0x100 + i * 4);
                    self.timers.write_reload(i, io_reload);
                }
                self.timers.write_control(i, io_control);
            }
        }
    }

    /// Tick timers and update I/O counters (called every CPU cycle)
    #[inline(always)]
    fn tick_timers(&mut self, cycles: u32) {
        let overflowed = self.timers.tick(cycles);

        // Write enabled timer counters back to I/O
        for i in 0..4 {
            if self.timers.timers[i].is_enabled() {
                self.bus
                    .write_io_direct_u16(0x100 + i * 4, self.timers.timers[i].counter);
            }
        }

        // Handle timer overflow IRQs (rare path)
        if overflowed.iter().any(|&count| count != 0) {
            let ime = self.bus.read_io_direct_u16(0x208);
            let ie = self.bus.read_io_direct_u16(0x200);
            for i in 0..4 {
                if self.audio_enabled && overflowed[i] != 0 && i < 2 {
                    for _ in 0..overflowed[i] {
                        let (need_a, need_b) = self.apu.on_timer_overflow(i as u8);
                        if need_a {
                            self.service_sound_fifo_dma(0);
                        }
                        if need_b {
                            self.service_sound_fifo_dma(1);
                        }
                    }
                }
                if overflowed[i] != 0 && self.timers.timers[i].irq_enabled() {
                    let bit = 1u16 << (3 + i);
                    self.request_irq(bit);
                    if ime != 0 && (ie & bit) != 0 {
                        self.cpu.irq();
                    }
                }
            }
        }
    }

    /// Sync sound I/O writes from the bus into the APU model.
    /// Hand the APU the cycles it is owed, then any register writes.
    ///
    /// Order matters: the chip has to advance to the moment of the write
    /// before the write lands, or a frequency change is heard slightly early.
    fn flush_apu(&mut self) {
        if self.apu_pending_cycles > 0 {
            self.apu.tick(self.apu_pending_cycles);
            self.apu_pending_cycles = 0;
        }
        self.sync_io_to_apu();
    }

    /// Flush any batched audio cycles, for callers about to read samples out.
    ///
    /// Draining without this would return a buffer missing up to a batch of
    /// sound, and the gap would land differently every time.
    pub fn settle_audio(&mut self) {
        if self.audio_enabled {
            self.flush_apu();
        }
    }

    fn sync_io_to_apu(&mut self) {
        let Some((start, end)) = self.bus.take_audio_io_dirty_range() else {
            return;
        };

        for idx in start..end {
            let offset = 0x60 + idx;
            let value = self.bus.read_io_direct(offset);
            self.apu.write_register(0x0400_0000 + offset as u32, value);
        }
    }

    fn sync_all_audio_io_to_apu(&mut self) {
        for offset in 0x60..=0xA7 {
            let value = self.bus.read_io_direct(offset);
            self.apu.write_register(0x0400_0000 + offset as u32, value);
        }
    }

    /// Enable or disable audio emulation work.
    pub fn set_audio_enabled(&mut self, enabled: bool) {
        if self.audio_enabled == enabled {
            return;
        }

        self.audio_enabled = enabled;
        self.apu.reset();
        self.bus.take_audio_io_dirty_range();

        if enabled {
            self.sync_all_audio_io_to_apu();
        }
    }

    /// Service sound FIFO DMA requests generated by timer overflow.
    fn service_sound_fifo_dma(&mut self, fifo: usize) {
        for channel in 1..=2 {
            if self.dma.sound_fifo_target(channel) != Some(fifo) {
                continue;
            }
            if let Some((_, bytes, len, irq)) = self.dma.run_sound_fifo(channel, &mut self.bus) {
                self.apu.fifo_write(fifo, &bytes[..len]);
                if irq {
                    let bit = 1u16 << (8 + channel);
                    self.request_irq(bit);
                }
            }
        }
    }

    /// Execute any pending DMA channels in priority order.
    fn run_pending_dma_channels(&mut self) {
        const DMA_CNT_H_OFFSETS: [usize; 4] = [0xBA, 0xC6, 0xD2, 0xDE];

        while let Some(channel) = self.dma.check_pending() {
            let irq = self.dma.run_channel(channel, &mut self.bus);

            // Keep the I/O mirror in sync when the DMA controller updates enable/repeat bits.
            self.bus.write_io_direct_u16(
                DMA_CNT_H_OFFSETS[channel],
                self.dma.channels[channel].control,
            );

            if irq {
                let bit = 1u16 << (8 + channel);
                self.request_irq(bit);

                let ime = self.bus.read_io_direct_u16(0x208);
                let ie = self.bus.read_io_direct_u16(0x200);
                if ime != 0 && (ie & bit) != 0 {
                    self.cpu.irq();
                }
            }
        }
    }

    /// Check for and execute DMA transfers by syncing I/O registers to DMA controller
    fn check_dma(&mut self) {
        let dma_debug = debug_config().dma_debug;
        // DMA register base offsets (from 0x04000000):
        // DMA0: 0xB0 (SAD), 0xB4 (DAD), 0xB8 (CNT_L), 0xBA (CNT_H)
        // DMA1: 0xBC, 0xC0, 0xC4, 0xC6
        // DMA2: 0xC8, 0xCC, 0xD0, 0xD2
        // DMA3: 0xD4, 0xD8, 0xDC, 0xDE
        const DMA_OFFSETS: [(usize, usize, usize, usize); 4] = [
            (0xB0, 0xB4, 0xB8, 0xBA),
            (0xBC, 0xC0, 0xC4, 0xC6),
            (0xC8, 0xCC, 0xD0, 0xD2),
            (0xD4, 0xD8, 0xDC, 0xDE),
        ];

        for (ch, &(sad, dad, cnt_l, cnt_h)) in DMA_OFFSETS.iter().enumerate() {
            let io_control = self.bus.read_io_direct_u16(cnt_h);
            let prev_control = self.dma.read_control(ch);
            let was_enabled = (prev_control & 0x8000) != 0;
            let io_enabled = (io_control & 0x8000) != 0;

            if !io_enabled {
                if was_enabled || prev_control != io_control || self.dma.channels[ch].pending {
                    self.dma.write_control(ch, io_control);
                }
                continue;
            }

            // Detect rising edge: I/O has enable set but DMA controller doesn't yet
            if !was_enabled {
                let source = self.bus.read_io_direct_u16(sad) as u32
                    | ((self.bus.read_io_direct_u16(sad + 2) as u32) << 16);
                let dest = self.bus.read_io_direct_u16(dad) as u32
                    | ((self.bus.read_io_direct_u16(dad + 2) as u32) << 16);
                let count = self.bus.read_io_direct_u16(cnt_l);

                if dma_debug {
                    let bits = if io_control & 0x0400 != 0 { 32 } else { 16 };
                    let timing = (io_control >> 12) & 3;
                    eprintln!(
                        "DMA{} enable: src={:08x} dst={:08x} count={} {}bit timing={} ctrl={:04x}",
                        ch, source, dest, count, bits, timing, io_control
                    );
                }

                self.dma.write_source(ch, source);
                self.dma.write_dest(ch, dest);
                self.dma.write_count(ch, count);
                self.dma.write_control(ch, io_control);

                // Execute pending immediate DMA
                if self.dma.channels[ch].pending {
                    self.run_pending_dma_channels();
                }
            } else if prev_control != io_control {
                self.dma.write_control(ch, io_control);
            }
        }
    }

    /// Public wrapper around sync_io_to_ppu for testing.
    #[cfg(test)]
    pub fn sync_io_to_ppu_pub(&mut self) {
        self.sync_io_to_ppu();
    }

    /// Sync the LCD control registers that affect timing/interrupt behavior.
    fn sync_lcd_state_to_ppu(&mut self) {
        // DISPCNT (0x04000000)
        let dispcnt = self.bus.read_io_direct_u16(0x000);
        if self.last_dispcnt != dispcnt {
            self.ppu.write_dispcnt(dispcnt);
            self.last_dispcnt = dispcnt;
        }

        // DISPSTAT (0x04000004) - writable bits only (IRQ enables, VCounter compare)
        let dispstat = self.bus.read_io_direct_u16(0x004) & !0x0007;
        if self.last_dispstat_control != dispstat {
            self.ppu.display_status.vblank_irq = (dispstat & (1 << 3)) != 0;
            self.ppu.display_status.hblank_irq = (dispstat & (1 << 4)) != 0;
            self.ppu.display_status.vcounter_irq = (dispstat & (1 << 5)) != 0;
            self.ppu.display_status.vcounter_compare = (dispstat >> 8) as u8;
            self.last_dispstat_control = dispstat;
        }
    }

    #[inline(always)]
    fn io_range_dirty(start: usize, end: usize, dirty_start: usize, dirty_end: usize) -> bool {
        dirty_start < end && start < dirty_end
    }

    /// Sync all PPU control registers from bus to the render-side mirror.
    fn sync_all_ppu_control_regs_to_ppu(&mut self) {
        self.sync_lcd_state_to_ppu();

        // BG0CNT-BG3CNT (0x04000008-0x0400000E)
        for i in 0..4 {
            let bgcnt = self.bus.read_io_direct_u16(0x008 + i * 2);
            self.ppu.write_bgcnt(i as usize, bgcnt);
        }

        // BG scroll registers (0x04000010-0x0400001E)
        for i in 0..4 {
            let hofs = self.bus.read_io_direct_u16(0x010 + i * 4);
            let vofs = self.bus.read_io_direct_u16(0x012 + i * 4);
            self.ppu.write_bghofs(i as usize, hofs);
            self.ppu.write_bgvofs(i as usize, vofs);
        }

        // Affine BG2 matrix and reference point (0x04000020-0x0400002F)
        self.ppu.backgrounds[2].pa = self.bus.read_io_direct_u16(0x020) as i16;
        self.ppu.backgrounds[2].pb = self.bus.read_io_direct_u16(0x022) as i16;
        self.ppu.backgrounds[2].pc = self.bus.read_io_direct_u16(0x024) as i16;
        self.ppu.backgrounds[2].pd = self.bus.read_io_direct_u16(0x026) as i16;
        {
            let lo = self.bus.read_io_direct_u16(0x028) as u32;
            let hi = self.bus.read_io_direct_u16(0x02A) as u32;
            self.ppu.backgrounds[2].ref_x = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }
        {
            let lo = self.bus.read_io_direct_u16(0x02C) as u32;
            let hi = self.bus.read_io_direct_u16(0x02E) as u32;
            self.ppu.backgrounds[2].ref_y = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }

        // Affine BG3 matrix and reference point (0x04000030-0x0400003F)
        self.ppu.backgrounds[3].pa = self.bus.read_io_direct_u16(0x030) as i16;
        self.ppu.backgrounds[3].pb = self.bus.read_io_direct_u16(0x032) as i16;
        self.ppu.backgrounds[3].pc = self.bus.read_io_direct_u16(0x034) as i16;
        self.ppu.backgrounds[3].pd = self.bus.read_io_direct_u16(0x036) as i16;
        {
            let lo = self.bus.read_io_direct_u16(0x038) as u32;
            let hi = self.bus.read_io_direct_u16(0x03A) as u32;
            self.ppu.backgrounds[3].ref_x = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }
        {
            let lo = self.bus.read_io_direct_u16(0x03C) as u32;
            let hi = self.bus.read_io_direct_u16(0x03E) as u32;
            self.ppu.backgrounds[3].ref_y = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }

        // Window registers (0x04000040-0x04000048)
        self.ppu.write_win0h(self.bus.read_io_direct_u16(0x040));
        self.ppu.write_win1h(self.bus.read_io_direct_u16(0x042));
        self.ppu.write_win0v(self.bus.read_io_direct_u16(0x044));
        self.ppu.write_win1v(self.bus.read_io_direct_u16(0x046));
        self.ppu.write_winin(self.bus.read_io_direct_u16(0x048));
        self.ppu.write_winout(self.bus.read_io_direct_u16(0x04A));

        // Color effect registers (0x04000050-0x04000054)
        self.ppu.write_bldcnt(self.bus.read_io_direct_u16(0x050));
        self.ppu.write_bldalpha(self.bus.read_io_direct_u16(0x052));
        self.ppu.write_bldy(self.bus.read_io_direct_u16(0x054));
    }

    /// Sync only the PPU control register groups that changed since the last mirror update.
    fn sync_dirty_ppu_control_regs_to_ppu(&mut self) {
        let Some((dirty_start, dirty_end)) = self.bus.take_ppu_io_dirty_range() else {
            return;
        };

        if Self::io_range_dirty(0x000, 0x002, dirty_start, dirty_end) {
            let dispcnt = self.bus.read_io_direct_u16(0x000);
            if self.last_dispcnt != dispcnt {
                self.ppu.write_dispcnt(dispcnt);
                self.last_dispcnt = dispcnt;
            }
        }

        if Self::io_range_dirty(0x004, 0x006, dirty_start, dirty_end) {
            let dispstat = self.bus.read_io_direct_u16(0x004) & !0x0007;
            if self.last_dispstat_control != dispstat {
                self.ppu.display_status.vblank_irq = (dispstat & (1 << 3)) != 0;
                self.ppu.display_status.hblank_irq = (dispstat & (1 << 4)) != 0;
                self.ppu.display_status.vcounter_irq = (dispstat & (1 << 5)) != 0;
                self.ppu.display_status.vcounter_compare = (dispstat >> 8) as u8;
                self.last_dispstat_control = dispstat;
            }
        }

        if Self::io_range_dirty(0x008, 0x010, dirty_start, dirty_end) {
            for i in 0..4 {
                let bgcnt = self.bus.read_io_direct_u16(0x008 + i * 2);
                self.ppu.write_bgcnt(i as usize, bgcnt);
            }
        }

        if Self::io_range_dirty(0x010, 0x020, dirty_start, dirty_end) {
            for i in 0..4 {
                let hofs = self.bus.read_io_direct_u16(0x010 + i * 4);
                let vofs = self.bus.read_io_direct_u16(0x012 + i * 4);
                self.ppu.write_bghofs(i as usize, hofs);
                self.ppu.write_bgvofs(i as usize, vofs);
            }
        }

        if Self::io_range_dirty(0x020, 0x030, dirty_start, dirty_end) {
            self.ppu.backgrounds[2].pa = self.bus.read_io_direct_u16(0x020) as i16;
            self.ppu.backgrounds[2].pb = self.bus.read_io_direct_u16(0x022) as i16;
            self.ppu.backgrounds[2].pc = self.bus.read_io_direct_u16(0x024) as i16;
            self.ppu.backgrounds[2].pd = self.bus.read_io_direct_u16(0x026) as i16;
            let lo = self.bus.read_io_direct_u16(0x028) as u32;
            let hi = self.bus.read_io_direct_u16(0x02A) as u32;
            self.ppu.backgrounds[2].ref_x = ((lo | (hi << 16)) as i32) << 4 >> 4;
            let lo = self.bus.read_io_direct_u16(0x02C) as u32;
            let hi = self.bus.read_io_direct_u16(0x02E) as u32;
            self.ppu.backgrounds[2].ref_y = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }

        if Self::io_range_dirty(0x030, 0x040, dirty_start, dirty_end) {
            self.ppu.backgrounds[3].pa = self.bus.read_io_direct_u16(0x030) as i16;
            self.ppu.backgrounds[3].pb = self.bus.read_io_direct_u16(0x032) as i16;
            self.ppu.backgrounds[3].pc = self.bus.read_io_direct_u16(0x034) as i16;
            self.ppu.backgrounds[3].pd = self.bus.read_io_direct_u16(0x036) as i16;
            let lo = self.bus.read_io_direct_u16(0x038) as u32;
            let hi = self.bus.read_io_direct_u16(0x03A) as u32;
            self.ppu.backgrounds[3].ref_x = ((lo | (hi << 16)) as i32) << 4 >> 4;
            let lo = self.bus.read_io_direct_u16(0x03C) as u32;
            let hi = self.bus.read_io_direct_u16(0x03E) as u32;
            self.ppu.backgrounds[3].ref_y = ((lo | (hi << 16)) as i32) << 4 >> 4;
        }

        if Self::io_range_dirty(0x040, 0x04C, dirty_start, dirty_end) {
            self.ppu.write_win0h(self.bus.read_io_direct_u16(0x040));
            self.ppu.write_win1h(self.bus.read_io_direct_u16(0x042));
            self.ppu.write_win0v(self.bus.read_io_direct_u16(0x044));
            self.ppu.write_win1v(self.bus.read_io_direct_u16(0x046));
            self.ppu.write_winin(self.bus.read_io_direct_u16(0x048));
            self.ppu.write_winout(self.bus.read_io_direct_u16(0x04A));
        }

        if Self::io_range_dirty(0x050, 0x056, dirty_start, dirty_end) {
            self.ppu.write_bldcnt(self.bus.read_io_direct_u16(0x050));
            self.ppu.write_bldalpha(self.bus.read_io_direct_u16(0x052));
            self.ppu.write_bldy(self.bus.read_io_direct_u16(0x054));
        }
    }

    /// Sync I/O registers from bus to PPU.
    fn sync_io_to_ppu(&mut self) {
        self.sync_all_ppu_control_regs_to_ppu();
        // Sync VRAM, palette, and OAM from bus to PPU
        self.sync_video_memory();
    }

    fn pending_enabled_irq_bits(&self) -> u16 {
        self.bus.read_io_direct_u16(0x200) & self.bus.read_io_direct_u16(0x202)
    }

    fn request_irq(&mut self, mask: u16) {
        if mask == 0 {
            return;
        }

        let if_reg = self.bus.read_io_direct_u16(0x202);
        self.bus.write_io_direct_u16(0x202, if_reg | mask);

        // BIOS IRQ code mirrors acknowledged requests here; keeping it current
        // helps HLE waits and software that inspects the mirror.
        let irq_flags = self.bus.read_u16(0x03007FF8);
        self.bus.write_u16(0x03007FF8, irq_flags | mask);
    }

    fn clear_irq_bits(&mut self, mask: u16) {
        if mask == 0 {
            return;
        }

        let if_reg = self.bus.read_io_direct_u16(0x202);
        self.bus.write_io_direct_u16(0x202, if_reg & !mask);

        let irq_flags = self.bus.read_u16(0x03007FF8);
        self.bus.write_u16(0x03007FF8, irq_flags & !mask);
    }

    fn consume_bios_intr_wait(&mut self, mask: u16) {
        self.clear_irq_bits(mask);
        self.bios_intr_wait_mask = None;
    }

    fn apply_bios_intr_wait_gate(&mut self) {
        let Some(mask) = self.bios_intr_wait_mask else {
            return;
        };

        // Let the user IRQ handler run to completion before deciding whether the
        // BIOS wait should resume or re-halt.
        if self.cpu.registers.mode() == CpuMode::IRQ {
            self.cpu.halted = false;
            return;
        }

        let pending = self.pending_enabled_irq_bits();

        if (pending & mask) != 0 {
            self.consume_bios_intr_wait(mask);
            self.cpu.halted = false;
            return;
        }

        // Another interrupt is pending, and it is not the one being waited on.
        //
        // Hardware still runs its handler: `IntrWait` halts, *any* enabled
        // interrupt wakes the CPU, the BIOS dispatcher calls the game's own
        // handler, and only then does the wait look again at the flag it cares
        // about. Re-halting here instead meant a game sitting in
        // `VBlankIntrWait` could never service anything else.
        //
        // For a linked game that is fatal rather than slow: the serial
        // interrupt is how a console reads what came off the cable, so a child
        // that opened a menu which waits on VBlank stopped answering the cable
        // entirely, and the game reported a communication error at once.
        //
        // The interrupt is *taken* here, not merely woken from. Clearing
        // `halted` on its own would let the waiting thread run its next
        // instruction — the one after `IntrWait` — so the wait would have
        // returned without its flag ever being set. Entering the handler puts
        // the program counter on the IRQ vector instead, which is where
        // hardware puts it.
        //
        // Nothing spins. The handler acknowledges its interrupt, `IF` clears,
        // and once it returns the gate finds nothing pending and halts again,
        // which is what the hardware does between interrupts.
        if pending != 0 {
            let ime = self.bus.read_io_direct_u16(0x208);
            if ime != 0 {
                // `Cpu::irq` checks the I flag itself and does nothing when
                // interrupts are masked, so a game inside a critical section
                // still gets to finish it.
                self.cpu.irq();
                if self.cpu.registers.mode() == CpuMode::IRQ {
                    self.cpu.halted = false;
                    return;
                }
            }
        }

        self.cpu.halted = true;
    }

    fn current_hle_swi_comment(&self, pc: u32) -> Option<u8> {
        if self.cpu.halted {
            return None;
        }

        // Not by reading address 0: the BIOS refuses to be read from outside
        // itself and answers with its last fetched opcode, so probing it that
        // way reports whatever happened to be latched.
        let has_real_bios = self.use_bios;

        if self.cpu.is_thumb_mode() {
            let opcode = self.bus.read_u16(pc);
            if (opcode & 0xFF00) != 0xDF00 {
                return None;
            }

            let comment = (opcode & 0x00FF) as u8;
            if has_real_bios && !Bios::should_hle_with_real_bios(comment) {
                None
            } else {
                Some(comment)
            }
        } else {
            let opcode = self.bus.read_u32(pc);
            if (opcode >> 24) != 0xEF {
                return None;
            }

            let high = ((opcode >> 16) & 0xFF) as u8;
            let low = (opcode & 0xFF) as u8;
            let comment = if high != 0 { high } else { low };
            if has_real_bios && !Bios::should_hle_with_real_bios(comment) {
                None
            } else {
                Some(comment)
            }
        }
    }

    /// Sync VRAM, palette, and OAM from bus to PPU
    fn sync_video_memory(&mut self) {
        let (palette_dirty, vram_dirty, oam_dirty) = self.bus.take_video_dirty_ranges();

        if let Some((start, end)) = vram_dirty {
            let bus_vram = self.bus.get_vram_slice();
            let end = end.min(bus_vram.len()).min(self.ppu.vram.len());
            if start < end {
                self.ppu.vram[start..end].copy_from_slice(&bus_vram[start..end]);
            }
        }

        if let Some((start, end)) = palette_dirty {
            let bus_palette = self.bus.get_palette_slice();
            let start_idx = (start / 2).min(self.ppu.palette.len());
            let end_idx = ((end + 1) / 2).min(self.ppu.palette.len());
            for i in start_idx..end_idx {
                let lo = i * 2;
                if lo + 1 < bus_palette.len() {
                    self.ppu.palette[i] =
                        u16::from_le_bytes([bus_palette[lo], bus_palette[lo + 1]]);
                }
            }
        }

        if let Some((start, end)) = oam_dirty {
            let bus_oam = self.bus.get_oam_slice();
            let end = end.min(bus_oam.len()).min(self.ppu.oam.len());
            if start < end {
                self.ppu.oam[start..end].copy_from_slice(&bus_oam[start..end]);
                self.ppu.sprite_renderer.invalidate_cache();
            }
        }
    }
}

/// Emulation status information
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct GbaStatus {
    pub state: GbaState,
    pub pc: u32,
    pub cpsr: u32,
    pub total_cycles: u64,
    pub frame_count: u64,
    pub cycles_per_second: u64,
}

/// Run the cable between consoles that share one.
///
/// This is the entire host side of a link for the case where every console is
/// in the same process: watch the parent, collect everybody's halfword, hand
/// the set back. A networked host does the same three things with a round trip
/// in the middle, which is why the core exposes them separately.
///
/// Returns whether a transfer was carried.
pub fn link_step(consoles: &mut [Gba]) -> bool {
    use crate::sio::{MAX_PLAYERS, NO_DATA};

    if consoles.len() < 2 || !consoles[0].link_transfer_pending() {
        return false;
    }

    // Everyone else is pulled into the transfer the parent started. They do
    // not get a say: on real hardware the parent drives the clock line and the
    // children answer whether they were ready or not.
    for child in consoles.iter_mut().skip(1) {
        child.link_join();
    }

    // A console that is not on the cable stays blank rather than sending zero,
    // which a game would read as real data.
    let mut values = [NO_DATA; MAX_PLAYERS];
    for (slot, console) in consoles.iter().take(MAX_PLAYERS).enumerate() {
        values[slot] = console.link_send_value();
    }

    // The parent clocks the cable, so its baud rate is what everyone runs at.
    let cycles = consoles[0].link_transfer_cycles();
    for console in consoles.iter_mut() {
        console.link_deliver(values, cycles);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gba_new() {
        let gba = Gba::new();
        assert_eq!(gba.state, GbaState::Stopped);
        assert_eq!(gba.total_cycles, 0);
        assert_eq!(gba.frame_count, 0);
    }

    #[test]
    fn test_gba_with_rom() {
        let rom = vec![0u8; 1024];
        let gba = Gba::with_rom(rom);
        assert_eq!(gba.state, GbaState::Running);
    }

    #[test]
    fn test_gba_reset() {
        let mut gba = Gba::new();
        gba.start();
        gba.stop();
        gba.reset();
        assert_eq!(gba.state, GbaState::Running);
        assert_eq!(gba.total_cycles, 0);
    }

    #[test]
    fn test_gba_read_write() {
        let mut gba = Gba::new();

        gba.write_u32(0x03000000, 0x12345678);
        assert_eq!(gba.read_u32(0x03000000), 0x12345678);

        gba.write_u16(0x03000004, 0xABCD);
        assert_eq!(gba.read_u16(0x03000004), 0xABCD);

        gba.write_u8(0x03000006, 0xEF);
        assert_eq!(gba.read_u8(0x03000006), 0xEF);
    }

    #[test]
    fn test_effect_registers_synced() {
        let mut gba = Gba::new();
        gba.start();

        // Write BLDCNT (alpha blend, BG0 first, BG1 second) to I/O bus
        gba.bus.write_io_direct_u16(0x050, 0x0241); // Alpha mode, BG0 first, BG1 second
        gba.bus.write_io_direct_u16(0x052, 0x0808); // EVA=8, EVB=8

        // Force a sync
        gba.sync_io_to_ppu_pub();

        use crate::ppu::effects::BlendMode;
        assert_eq!(gba.ppu.effects.blend_control.blend_mode, BlendMode::Alpha);
        assert_eq!(gba.ppu.effects.blend_alpha.alpha_a, 8);
        assert_eq!(gba.ppu.effects.blend_alpha.alpha_b, 8);
    }

    #[test]
    fn test_window_registers_synced() {
        let mut gba = Gba::new();
        gba.start();

        // WIN0H: left=10, right=200 → 0x0AC8
        gba.bus.write_io_direct_u16(0x040, 0x0AC8);
        gba.sync_io_to_ppu_pub();

        assert_eq!(gba.ppu.windows[0].h_range.left, 10);
        assert_eq!(gba.ppu.windows[0].h_range.right, 200);
    }

    #[test]
    fn test_window_register_updates_sync_during_visible_scanline() {
        let mut gba = Gba::new();
        gba.start();
        gba.cpu.halted = true;
        gba.ppu.scanline = 10;
        gba.ppu.cycle = 100;

        gba.bus.write_u16(0x0400_0044, 0x3A4A);

        gba.step();

        assert_eq!(gba.ppu.windows[0].v_range.top, 0x3A);
        assert_eq!(gba.ppu.windows[0].v_range.bottom, 0x4A);
    }

    #[test]
    fn test_visible_scanline_syncs_bg_state_before_hblank_render() {
        let mut gba = Gba::new();
        gba.start();

        gba.bus.write_u16(0x0400_0000, 0x0100); // mode 0, BG0 on
        gba.bus.write_u16(0x0400_0008, 0x0100); // BG0 char base 0, screen base 1, priority 0

        // BG palette entry 1 = bright red, backdrop entry 0 = black.
        gba.bus.write_u16(0x0500_0000, 0x0000);
        gba.bus.write_u16(0x0500_0002, 0x001F);

        // Tile 0 filled with palette index 1 in 4bpp.
        for offset in 0..32u32 {
            gba.bus.write_u8(0x0600_0000 + offset, 0x11);
        }
        // Screen entry 0 points at tile 0.
        gba.bus.write_u16(0x0600_0800, 0x0000);

        gba.ppu.scanline = 0;
        gba.ppu.cycle = crate::ppu::HBLANK_START - 1;

        gba.step();

        let pixel = gba.ppu.framebuffer.back_buffer_slice()[0].color;
        assert_eq!(pixel, crate::ppu::Color::from_rgb555(0x001F));
    }

    /// Loading a second cartridge must not leave the first one's machine behind.
    ///
    /// Switching ROMs used to reset only the CPU and PPU while every byte of
    /// work RAM, video memory, and I/O kept the previous game's values — so the
    /// old game stayed on screen and the new one booted onto hardware it had
    /// never configured. Every ROM pair was affected, in both directions.
    #[test]
    fn loading_a_second_rom_starts_from_a_clean_machine() {
        let mut gba = Gba::new();
        gba.load_rom(vec![0u8; 1024]);

        // Stand in for a game that has been running for a while.
        gba.bus.write_u16(0x0200_0000, 0xBEEF); // EWRAM
        gba.bus.write_u16(0x0300_0000, 0xCAFE); // IWRAM
        gba.bus.write_u16(0x0600_0000, 0x7FFF); // VRAM: what is on screen
        gba.bus.write_u16(0x0500_0000, 0x03E0); // palette
        gba.bus.write_u16(0x0400_0000, 0x0403); // DISPCNT: mode 3, BG2 on

        gba.load_rom(vec![0u8; 2048]);

        assert_eq!(gba.read_u16(0x0200_0000), 0, "EWRAM carried over");
        assert_eq!(gba.read_u16(0x0300_0000), 0, "IWRAM carried over");
        assert_eq!(
            gba.read_u16(0x0600_0000),
            0,
            "the old game is still on screen"
        );
        assert_eq!(gba.read_u16(0x0500_0000), 0, "palette carried over");
        assert_eq!(
            gba.read_u16(0x0400_0000) & 0x0407,
            0,
            "DISPCNT carried over"
        );
    }

    /// The same run of frames must produce the same machine, cold or switched.
    #[test]
    fn a_switched_rom_runs_like_a_cold_boot() {
        // A ROM that is all zeroes decodes as ANDEQ r0,r0,r0 — harmless, and it
        // exercises the boot path without needing a real game.
        let first = vec![0u8; 4096];
        let second = {
            let mut rom = vec![0u8; 4096];
            rom[0x1000 - 4..].copy_from_slice(&[0, 0, 0, 0]);
            rom
        };

        let mut cold = Gba::new();
        cold.load_rom(second.clone());
        for _ in 0..4 {
            cold.run_frame();
        }

        let mut switched = Gba::new();
        switched.load_rom(first);
        for _ in 0..4 {
            switched.run_frame();
        }
        switched.load_rom(second);
        for _ in 0..4 {
            switched.run_frame();
        }

        assert_eq!(switched.pc(), cold.pc(), "CPU diverged after a ROM switch");
        for addr in [
            0x0200_0000u32,
            0x0300_0000,
            0x0500_0000,
            0x0600_0000,
            0x0700_0000,
        ] {
            assert_eq!(
                switched.read_u16(addr),
                cold.read_u16(addr),
                "memory at {addr:08X} differs from a cold boot"
            );
        }
    }

    #[test]
    fn test_hblank_dma_does_not_run_during_vblank() {
        let mut gba = Gba::new();
        gba.start();
        gba.cpu.halted = true;

        gba.bus.write_u16(0x0200_0000, 0x2468);
        gba.dma.write_source(0, 0x0200_0000);
        gba.dma.write_dest(0, 0x0300_0000);
        gba.dma.write_count(0, 1);
        gba.dma.write_control(0, 0x8000 | 0x0200 | (2 << 12));

        gba.ppu.scanline = crate::ppu::VISIBLE_SCANLINES;
        gba.ppu.cycle = crate::ppu::HBLANK_START - 1;

        gba.step();

        assert_eq!(gba.read_u16(0x0300_0000), 0);

        gba.ppu.scanline = 0;
        gba.ppu.cycle = crate::ppu::HBLANK_START - 1;
        gba.step();
        assert_eq!(gba.read_u16(0x0300_0000), 0x2468);
    }

    #[test]
    fn test_dma_disable_write_rearms_hblank_channel_with_new_source() {
        let mut gba = Gba::new();

        gba.bus.write_u16(0x0200_0000, 0x1111);
        gba.bus.write_u16(0x0200_0002, 0x9ABC);
        gba.bus.write_u16(0x0200_0100, 0x2222);

        gba.bus.write_io_direct_u16(0x0B0, 0x0000);
        gba.bus.write_io_direct_u16(0x0B2, 0x0200);
        gba.bus.write_io_direct_u16(0x0B4, 0x0000);
        gba.bus.write_io_direct_u16(0x0B6, 0x0300);
        gba.bus.write_io_direct_u16(0x0B8, 0x0001);
        gba.bus.write_io_direct_u16(0x0BA, 0x8200 | (2 << 12));
        gba.check_dma();

        gba.dma.on_hblank();
        gba.run_pending_dma_channels();
        assert_eq!(gba.read_u16(0x0300_0000), 0x1111);

        gba.bus.write_io_direct_u16(0x0BA, 0x0000);
        gba.check_dma();
        assert!(!gba.dma.channels[0].is_enabled());
        assert!(!gba.dma.channels[0].pending);

        gba.bus.write_io_direct_u16(0x0B0, 0x0100);
        gba.bus.write_io_direct_u16(0x0B2, 0x0200);
        gba.bus.write_io_direct_u16(0x0B4, 0x0000);
        gba.bus.write_io_direct_u16(0x0B6, 0x0300);
        gba.bus.write_io_direct_u16(0x0B8, 0x0001);
        gba.bus.write_io_direct_u16(0x0BA, 0x8200 | (2 << 12));
        gba.check_dma();

        gba.dma.on_hblank();
        gba.run_pending_dma_channels();
        assert_eq!(gba.read_u16(0x0300_0000), 0x2222);
    }

    #[test]
    fn test_vblank_intr_wait_consumes_irq_when_woken_later() {
        let mut gba = Gba::new();
        gba.start();

        gba.cpu.set_thumb_mode(false);
        gba.cpu.pipeline.set_fetch_addr(0x0300_0000);
        gba.cpu.registers.set_pc(0x0300_0000);

        // SWI 0x05 (VBlankIntrWait), then ARM NOP.
        gba.bus.write_u32(0x0300_0000, 0xEF00_0005);
        gba.bus.write_u32(0x0300_0004, 0xE1A0_0000);

        gba.bus.write_io_direct_u16(0x200, 0x0001); // IE: VBlank

        gba.step();

        assert!(gba.cpu.halted);
        assert_eq!(gba.bios_intr_wait_mask, Some(0x0001));

        gba.request_irq(0x0001);
        assert_eq!(gba.bus.read_io_direct_u16(0x202) & 0x0001, 0x0001);
        assert_eq!(gba.bus.read_u16(0x0300_7FF8) & 0x0001, 0x0001);

        gba.step();

        assert!(!gba.cpu.halted);
        assert_eq!(gba.bios_intr_wait_mask, None);
        assert_eq!(gba.bus.read_io_direct_u16(0x202) & 0x0001, 0);
        assert_eq!(gba.bus.read_u16(0x0300_7FF8) & 0x0001, 0);
        assert_eq!(gba.cpu.fetch_addr(), 0x0300_0008);
    }

    /// A console with interrupts actually enabled: `IME` on and the CPSR I
    /// flag clear. `IntrWait` does nothing useful without both, so a test that
    /// leaves them out is testing a console that could never take an interrupt.
    fn enable_interrupts(gba: &mut Gba) {
        gba.bus.write_io_direct_u16(0x208, 1);
        let cpsr = gba.cpu.registers.cpsr() & !(1 << 7);
        gba.cpu.registers.set_cpsr(cpsr, false);
    }

    #[test]
    fn test_intr_wait_ignores_unrelated_irq_bits() {
        let mut gba = Gba::new();
        gba.start();

        gba.cpu.set_thumb_mode(false);
        gba.cpu.pipeline.set_fetch_addr(0x0300_0000);
        gba.cpu.registers.set_pc(0x0300_0000);
        gba.cpu.registers.set_reg(0, 0);
        gba.cpu.registers.set_reg(1, 0x0001); // Wait for VBlank only

        // SWI 0x04 (IntrWait), then ARM NOP.
        gba.bus.write_u32(0x0300_0000, 0xEF00_0004);
        gba.bus.write_u32(0x0300_0004, 0xE1A0_0000);

        gba.bus.write_io_direct_u16(0x200, 0x0009); // IE: VBlank + Timer0
        enable_interrupts(&mut gba);

        gba.step();

        assert!(gba.cpu.halted);
        assert_eq!(gba.bios_intr_wait_mask, Some(0x0001));

        gba.request_irq(0x0008); // Timer0 only
        gba.step();

        // The wait is not satisfied: it is still outstanding, still waiting on
        // VBlank, and it has not consumed Timer0's flag.
        assert_eq!(gba.bios_intr_wait_mask, Some(0x0001));
        assert_eq!(gba.bus.read_io_direct_u16(0x202) & 0x0008, 0x0008);

        // But the console is awake, because Timer0's own handler has to run.
        //
        // This assertion used to be the opposite, and that was the bug. On
        // hardware `IntrWait` halts and *any* enabled interrupt wakes the CPU:
        // the BIOS dispatcher calls the game's handler, and only then does the
        // wait look again at the flag it wants. Staying halted through an
        // unrelated interrupt means its handler never runs at all.
        //
        // For a linked game that is fatal. The serial interrupt is how a
        // console reads what came off the cable, so a game sitting in
        // `VBlankIntrWait` — which is most menus — stopped answering the cable
        // the moment it got there.
        assert!(
            !gba.cpu.halted,
            "an unrelated interrupt still has a handler to run"
        );
    }

    /// The link-cable case, which is what made the bug visible.
    ///
    /// A console waiting on VBlank has to keep servicing the cable, or the
    /// game on the other end sees a partner that has stopped answering.
    #[test]
    fn a_serial_interrupt_is_serviced_while_waiting_for_vblank() {
        const VBLANK: u16 = 0x0001;
        const SERIAL: u16 = 0x0080;

        let mut gba = Gba::new();
        gba.start();

        gba.cpu.set_thumb_mode(false);
        gba.cpu.pipeline.set_fetch_addr(0x0300_0000);
        gba.cpu.registers.set_pc(0x0300_0000);
        gba.cpu.registers.set_reg(0, 0);
        gba.cpu.registers.set_reg(1, u32::from(VBLANK));

        // SWI 0x04 (IntrWait), then ARM NOP.
        gba.bus.write_u32(0x0300_0000, 0xEF00_0004);
        gba.bus.write_u32(0x0300_0004, 0xE1A0_0000);
        gba.bus.write_io_direct_u16(0x200, VBLANK | SERIAL);
        enable_interrupts(&mut gba);

        gba.step();
        assert!(gba.cpu.halted, "the wait starts by halting");

        // The cable delivers, and the console has to wake up for it.
        gba.request_irq(SERIAL);
        gba.step();

        assert!(
            !gba.cpu.halted,
            "a console that sleeps through the cable is a console the other end gives up on"
        );

        // In the handler, not back in the waiting thread. Merely waking would
        // have run the instruction after `IntrWait`, which is the wait
        // returning without its flag ever being set.
        assert_eq!(gba.cpu.registers.mode(), CpuMode::IRQ);
        // At the vector, having already run its first instruction — the step
        // that took the interrupt also executed one. What matters is that it
        // is here and not at 0x03000004, which is where the waiting thread
        // would have carried on.
        assert!(
            (0x18..0x20).contains(&gba.cpu.fetch_addr()),
            "expected the IRQ vector, got {:#x}",
            gba.cpu.fetch_addr()
        );

        // And the wait itself is untouched: still outstanding, still on VBlank.
        assert_eq!(gba.bios_intr_wait_mask, Some(VBLANK));
        assert_eq!(gba.bus.read_io_direct_u16(0x202) & SERIAL, SERIAL);
    }

    /// With nothing pending at all, the wait still halts. Otherwise the fix
    /// above would turn every `IntrWait` into a spin.
    #[test]
    fn an_intr_wait_with_nothing_pending_stays_halted() {
        let mut gba = Gba::new();
        gba.start();

        gba.cpu.set_thumb_mode(false);
        gba.cpu.pipeline.set_fetch_addr(0x0300_0000);
        gba.cpu.registers.set_pc(0x0300_0000);
        gba.cpu.registers.set_reg(0, 0);
        gba.cpu.registers.set_reg(1, 0x0001);

        gba.bus.write_u32(0x0300_0000, 0xEF00_0004);
        gba.bus.write_u32(0x0300_0004, 0xE1A0_0000);
        gba.bus.write_io_direct_u16(0x200, 0x0001);
        enable_interrupts(&mut gba);

        gba.step();
        for _ in 0..8 {
            gba.step();
            assert!(
                gba.cpu.halted,
                "nothing is pending, so there is nothing to wake for"
            );
        }
        assert_eq!(gba.bios_intr_wait_mask, Some(0x0001));
    }

    #[test]
    fn test_sync_video_memory_invalidates_sprite_cache_when_oam_changes() {
        let mut gba = Gba::new();
        gba.start();

        // Give OBJ tile 0 a solid visible pattern and palette entry 1 a visible
        // color. Halfword writes, because sprite VRAM ignores byte stores.
        for offset in (0..32u32).step_by(2) {
            gba.bus.write_u16(0x0601_0000 + offset, 0x1111);
        }
        gba.bus.write_u16(0x0500_0202, 0x001F);

        // Hide every OAM entry first; otherwise zeroed entries render as visible sprites.
        for i in 0..128u32 {
            let base = 0x0700_0000 + i * 8;
            gba.bus.write_u16(base, 0x0200);
            gba.bus.write_u16(base + 2, 0x0000);
            gba.bus.write_u16(base + 4, 0x0000);
        }

        // First sync with sprite 0 disabled so the cache learns a hidden sprite.
        gba.sync_io_to_ppu_pub();

        gba.ppu.framebuffer.clear_scanline(0);
        gba.ppu.sprite_renderer.render_sprites(
            &mut gba.ppu.framebuffer,
            &gba.ppu.vram,
            &gba.ppu.oam,
            &gba.ppu.palette,
            0,
            0,
            true,
        );
        let hidden_non_black = gba
            .ppu
            .framebuffer
            .back_buffer_slice()
            .iter()
            .filter(|p| p.color != crate::ppu::Color::BLACK)
            .count();
        assert_eq!(hidden_non_black, 0);

        // Now make sprite 0 visible at (0, 0). The next sync must invalidate the cache.
        gba.bus.write_u16(0x0700_0000, 0x0000);
        gba.bus.write_u16(0x0700_0002, 0x0000);
        gba.bus.write_u16(0x0700_0004, 0x0000);
        gba.sync_io_to_ppu_pub();

        gba.ppu.framebuffer.clear_scanline(0);
        gba.ppu.sprite_renderer.render_sprites(
            &mut gba.ppu.framebuffer,
            &gba.ppu.vram,
            &gba.ppu.oam,
            &gba.ppu.palette,
            0,
            0,
            true,
        );
        let visible_non_black = gba
            .ppu
            .framebuffer
            .back_buffer_slice()
            .iter()
            .filter(|p| p.color != crate::ppu::Color::BLACK)
            .count();
        assert!(visible_non_black > 0);
    }

    #[test]
    fn test_sync_io_to_apu_preserves_same_value_fifo_writes() {
        let mut gba = Gba::new();

        gba.bus.write_u8(0x0400_0084, 0x80);
        gba.sync_io_to_apu();
        gba.bus.write_u8(0x0400_00A0, 0x7F);
        gba.sync_io_to_apu();
        gba.bus.write_u8(0x0400_00A0, 0x7F);
        gba.sync_io_to_apu();

        assert_eq!(gba.apu.fifo_a.len(), 2);
    }

    #[test]
    fn test_savestate_round_trip() {
        let mut gba = Gba::new();
        gba.load_rom(cartridge(1024 * 1024));
        gba.write_u32(0x0300_0000, 0x1234_5678);
        gba.speed = EmulationSpeed::Turbo;
        gba.audio_enabled = false;
        gba.apu.wave.two_banks = true;
        gba.apu.wave.bank_select = 1;
        gba.apu.wave.wave_ram[16] = 0xAB;

        let bytes = gba.save_state_bytes().expect("serialize savestate");

        let mut restored = Gba::new();
        restored.load_rom(cartridge(1024 * 1024));
        restored
            .load_state_bytes(&bytes)
            .expect("deserialize savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0x1234_5678);
        assert_eq!(restored.speed, EmulationSpeed::Turbo);
        assert!(!restored.audio_enabled);
        assert!(restored.apu.wave.two_banks);
        assert_eq!(restored.apu.wave.bank_select, 1);
        assert_eq!(restored.apu.wave.wave_ram[16], 0xAB);
    }

    #[test]
    fn test_load_legacy_savestate_without_intr_wait_mask() {
        let mut gba = Gba::new();
        gba.write_u32(0x0300_0000, 0x89AB_CDEF);
        gba.speed = EmulationSpeed::Limited(60);
        gba.audio_enabled = false;

        let bytes =
            bincode::serialize(&LegacySavestateV1::from(&gba)).expect("serialize legacy savestate");

        let mut restored = Gba::new();
        restored
            .load_state_bytes(&bytes)
            .expect("deserialize legacy savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0x89AB_CDEF);
        assert_eq!(restored.speed, EmulationSpeed::Limited(60));
        assert!(!restored.audio_enabled);
        assert_eq!(restored.bios_intr_wait_mask, None);
    }

    #[test]
    fn test_load_v2_savestate_with_legacy_apu_layout() {
        let mut gba = Gba::new();
        gba.write_u32(0x0300_0000, 0x1357_9BDF);
        gba.bios_intr_wait_mask = Some(0x0001);
        gba.apu.wave.wave_ram[0] = 0xCD;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(SAVESTATE_MAGIC);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend(
            bincode::serialize(&LegacySavestateV2::from(&gba))
                .expect("serialize legacy v2 savestate"),
        );

        let mut restored = Gba::new();
        restored
            .load_state_bytes(&bytes)
            .expect("deserialize legacy v2 savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0x1357_9BDF);
        assert_eq!(restored.bios_intr_wait_mask, Some(0x0001));
        assert_eq!(restored.apu.wave.wave_ram[0], 0xCD);
        assert_eq!(restored.apu.wave.wave_ram[16], 0xCD);
        assert!(!restored.apu.wave.two_banks);
    }

    /// A 16MB cartridge and a fabricated header, enough for the bus to accept
    /// it as a game and for the size to be worth measuring.
    fn cartridge(size: usize) -> Vec<u8> {
        let mut rom = vec![0u8; size];
        rom[0xAC..0xB0].copy_from_slice(b"BPRE");
        rom
    }

    #[test]
    fn a_savestate_does_not_carry_the_cartridge() {
        let mut gba = Gba::new();
        gba.load_rom(cartridge(16 * 1024 * 1024));
        gba.write_u32(0x0300_0000, 0xFEED_FACE);

        let bytes = gba.save_state_bytes().expect("serialize savestate");

        // What is left is the console's memory: 256KB of EWRAM, 32KB of
        // IWRAM, 96KB of VRAM and the small stuff. Against the 16MB cartridge
        // and the 1MB of rendered pixels that used to travel with it, this is
        // the difference between a save that uploads in a second and one that
        // times out.
        assert!(
            bytes.len() < 768 * 1024,
            "a savestate should be a few hundred kilobytes, this one is {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn a_savestate_restores_against_the_cartridge_in_the_machine() {
        let mut gba = Gba::new();
        gba.load_rom(cartridge(4 * 1024 * 1024));
        gba.write_u32(0x0300_0000, 0xFEED_FACE);
        gba.speed = EmulationSpeed::Turbo;
        let bytes = gba.save_state_bytes().expect("serialize savestate");

        // A second machine with the same cartridge in, as when someone reopens
        // the page and picks the game before restoring.
        let mut restored = Gba::new();
        restored.load_rom(cartridge(4 * 1024 * 1024));
        restored
            .load_state_bytes(&bytes)
            .expect("deserialize savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0xFEED_FACE);
        assert_eq!(restored.speed, EmulationSpeed::Turbo);
        // And the cartridge is still readable, rather than an empty slot.
        assert_eq!(restored.read_u32(0x0800_00AC), u32::from_le_bytes(*b"BPRE"));
    }

    #[test]
    fn restoring_without_a_cartridge_says_so() {
        let mut gba = Gba::new();
        gba.load_rom(cartridge(1024 * 1024));
        let bytes = gba.save_state_bytes().expect("serialize savestate");

        let err = Gba::new()
            .load_state_bytes(&bytes)
            .expect_err("a state with no cartridge to restore against should fail");
        assert!(
            err.to_string().contains("load the cartridge"),
            "unhelpful message: {err}"
        );
    }

    /// Registers, as a game sets them up for a link.
    const RCNT_SERIAL: u32 = 0x0400_0134;
    const SIOCNT_ADDR: u32 = 0x0400_0128;
    const SIOMLT_SEND_ADDR: u32 = 0x0400_012A;
    /// Multi-player mode at 115200 bps.
    const MULTI_115200: u16 = 0x2003;
    const START: u16 = 0x0080;

    /// A console sat at the link menu: halted, so it accrues cycles without
    /// executing whatever happens to be in an empty cartridge slot.
    fn linked_console(id: u8, players: u8, sends: u16) -> Gba {
        let mut gba = Gba::new();
        gba.start();
        gba.cpu.halted = true;
        gba.link_connect(id, players);
        gba.bus.write_u16(RCNT_SERIAL, 0x0000);
        gba.bus.write_u16(SIOCNT_ADDR, MULTI_115200);
        gba.bus.write_u16(SIOMLT_SEND_ADDR, sends);
        gba.step();
        gba
    }

    /// What a console is holding in its four data slots.
    fn slots(console: &Gba) -> Vec<u16> {
        (0..4)
            .map(|slot| console.bus.read_io_direct_u16(0x120 + slot * 2))
            .collect()
    }

    /// Run every console until the cable goes quiet.
    fn run_until_idle(consoles: &mut [Gba], limit: u32) -> u32 {
        for elapsed in 0..limit {
            link_step(consoles);
            for console in consoles.iter_mut() {
                console.step();
            }
            if consoles.iter().all(|c| !c.sio.busy()) {
                return elapsed;
            }
        }
        panic!("the cable never went idle");
    }

    #[test]
    fn two_consoles_exchange_a_halfword() {
        let mut consoles = vec![linked_console(0, 2, 0x1234), linked_console(1, 2, 0xABCD)];

        // Both know which end of the cable they are on before anything moves.
        assert_eq!(
            consoles[0].bus.read_io_direct_u16(0x128) & 0x0004,
            0,
            "parent"
        );
        assert_eq!(
            consoles[1].bus.read_io_direct_u16(0x128) & 0x0004,
            4,
            "child"
        );
        assert_eq!(
            (consoles[1].bus.read_io_direct_u16(0x128) >> 4) & 0b11,
            1,
            "the child is player one"
        );

        // The parent starts a transfer, which is all the game does.
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        assert!(consoles[0].link_transfer_pending());

        run_until_idle(&mut consoles, 20_000);

        // Both consoles end up holding both halfwords, in player order.
        for (which, console) in consoles.iter().enumerate() {
            assert_eq!(
                slots(console),
                vec![0x1234, 0xABCD, 0xFFFF, 0xFFFF],
                "console {which} saw the wrong cable"
            );
        }

        // And START has cleared, which is what the game is waiting on.
        assert_eq!(consoles[0].bus.read_io_direct_u16(0x128) & START, 0);
        assert_eq!(consoles[1].bus.read_io_direct_u16(0x128) & START, 0);
    }

    #[test]
    fn all_four_consoles_hear_each_other() {
        let sent = [0x1111u16, 0x2222, 0x3333, 0x4444];
        let mut consoles: Vec<Gba> = sent
            .iter()
            .enumerate()
            .map(|(id, value)| linked_console(id as u8, 4, *value))
            .collect();

        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        run_until_idle(&mut consoles, 40_000);

        for (which, console) in consoles.iter().enumerate() {
            assert_eq!(slots(console), sent.to_vec(), "console {which}");
        }
    }

    #[test]
    fn a_transfer_takes_the_time_the_cable_needs() {
        let mut consoles = vec![linked_console(0, 2, 0x0001), linked_console(1, 2, 0x0002)];
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();

        let before = consoles[0].total_cycles;
        run_until_idle(&mut consoles, 20_000);
        let spent = consoles[0].total_cycles - before;

        // Two consoles at 115200 is 36 bits on the wire. A transfer that took
        // no time would let a game spin through them faster than the hardware
        // ever could, which is how link code loses sync.
        let expected = u64::from(crate::sio::Sio::transfer_cycles(MULTI_115200, 2));
        assert!(
            spent >= expected,
            "the transfer took {spent} cycles, less than the {expected} the cable needs"
        );
    }

    /// The parent drives the clock line, so the cable runs at the parent's
    /// baud rate and a child's own setting does not come into it.
    ///
    /// This is not hypothetical. Pokémon raises the parent to 115200 for a
    /// trade and leaves the child on the 9600 it started at. Timing the child
    /// from its own register made it hold the wire for 62,914 cycles against
    /// the parent's 5,242 — so it was still busy when the next transfer came,
    /// refused to join it, and silently missed every transfer after the first.
    /// The game calls that a communication error.
    #[test]
    fn a_child_keeps_the_parents_pace_whatever_its_own_baud_says() {
        let parent = linked_console(0, 2, 0xAAAA);
        let mut child = linked_console(1, 2, 0xBBBB);
        // The child, left behind at 9600.
        child.bus.write_u16(SIOCNT_ADDR, 0x2000);
        child.step();

        let mut consoles = vec![parent, child];
        let parent_cycles = crate::sio::Sio::transfer_cycles(MULTI_115200, 2);

        // Several transfers back to back at the parent's cadence. One is not
        // enough to show the fault: the first always lands, and it is the
        // second that a still-busy child refuses.
        for round in 0..4 {
            consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
            consoles[0].step();
            assert!(
                link_step(&mut consoles),
                "round {round}: the parent could not start a transfer"
            );

            // Exactly the time the parent's cable needs, and no more.
            for _ in 0..parent_cycles {
                for console in consoles.iter_mut() {
                    console.step();
                }
            }

            assert!(
                !consoles[1].sio.busy(),
                "round {round}: the child was still clocking after the parent had finished"
            );
            assert_eq!(
                slots(&consoles[1]),
                vec![0xAAAA, 0xBBBB, 0xFFFF, 0xFFFF],
                "round {round}: the child missed the transfer"
            );
        }
    }

    /// Every halfword has to reach the game, even when the next transfer is
    /// asked for before this console finished clocking the last one.
    ///
    /// Two browsers do not share a frame clock. The parent can run out its
    /// wire, have its game ask for another transfer, and announce it while the
    /// child has not yet had an animation frame in which to finish the
    /// previous one. The child used to take the new data straight over the
    /// old, so the halfword mid-clock never reached the game at all — and a
    /// trade does not survive a gap in its stream. That is what the game means
    /// by "Sorry, we have a link error".
    #[test]
    fn a_transfer_asked_for_early_does_not_swallow_the_one_before_it() {
        let parent = linked_console(0, 2, 0x1111);
        let child = linked_console(1, 2, 0x2222);
        let mut consoles = vec![parent, child];
        let cycles = crate::sio::Sio::transfer_cycles(MULTI_115200, 2);

        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        assert!(link_step(&mut consoles));

        // The parent runs its wire out; the child, on its own clock, has only
        // got half way. Both are mid-transfer as far as the cable knows.
        for _ in 0..cycles {
            consoles[0].step();
        }
        for _ in 0..(cycles / 2) {
            consoles[1].step();
        }
        assert!(!consoles[0].sio.busy(), "the parent should be free");
        assert!(consoles[1].sio.busy(), "the child should still be clocking");

        // Now the child is pulled into the next transfer. Joining has to hand
        // the game what was on the wire before taking anything new.
        assert!(consoles[1].link_join());
        assert_eq!(
            slots(&consoles[1]),
            vec![0x1111, 0x2222, 0xFFFF, 0xFFFF],
            "the halfword that was still clocking never reached the game"
        );
    }

    /// Data may only land on a console that asked for it.
    #[test]
    fn a_console_that_is_not_waiting_refuses_delivery() {
        let mut idle = linked_console(1, 2, 0x0001);
        assert!(
            !idle.link_deliver([1, 2, 0xFFFF, 0xFFFF], 100),
            "a console with no transfer in flight has nothing to accept"
        );
        assert_eq!(slots(&idle), vec![0, 0, 0, 0], "nothing should have landed");
    }

    /// A game asking whether a cable is plugged in gets a truthful answer
    /// whichever serial mode it happens to be in.
    ///
    /// The terminals describe the wire, not the protocol. Fire Red passes
    /// through normal mode while setting a link up, and imposing these bits
    /// only in multi-player mode meant it was told there was nothing plugged
    /// in — so it gave up before it ever got to multi-player mode. Switching
    /// the cable off and on fixed it, because that path wrote the bits
    /// regardless of mode, which is the workaround players found for
    /// themselves.
    #[test]
    fn the_cable_is_visible_whatever_serial_mode_the_game_is_in() {
        for (what, siocnt) in [
            ("normal 8-bit", 0x0000u16),
            ("normal 32-bit", 0x1000),
            ("multi-player", 0x2000),
            ("UART", 0x3000),
        ] {
            let mut console = Gba::new();
            console.start();
            console.cpu.halted = true;
            console.link_connect(1, 2);
            console.bus.write_u16(RCNT_SERIAL, 0x0000);
            console.bus.write_u16(SIOCNT_ADDR, siocnt);
            console.step();

            let seen = console.bus.read_io_direct_u16(0x128);
            assert_eq!(
                seen & 0x0008,
                0x0008,
                "{what}: the game was told nothing was plugged in"
            );
            assert_eq!(seen & 0x0004, 0x0004, "{what}: this console is a child");
            assert_eq!((seen >> 4) & 0b11, 1, "{what}: it is player one");
        }
    }

    /// In normal mode the transfer bit belongs to the game, not to us.
    #[test]
    fn a_normal_mode_transfer_bit_is_left_alone() {
        let mut console = Gba::new();
        console.start();
        console.cpu.halted = true;
        console.link_connect(0, 2);
        console.bus.write_u16(RCNT_SERIAL, 0x0000);
        // Normal 32-bit with the game's own start bit set. Multi-player
        // transfers are the only ones carried here, so clearing this would be
        // answering a question that was never asked.
        console.bus.write_u16(SIOCNT_ADDR, 0x1000 | START);
        console.step();

        assert_eq!(
            console.bus.read_io_direct_u16(0x128) & START,
            START,
            "the game's own start bit was taken away from it"
        );
    }

    /// And the port being used for something else is not a cable at all.
    #[test]
    fn general_purpose_wiring_is_left_entirely_alone() {
        let mut console = Gba::new();
        console.start();
        console.cpu.halted = true;
        console.link_connect(1, 2);
        // A cartridge driving the pins directly, which is how a real-time
        // clock is wired. Every bit here is the cartridge's.
        console.bus.write_u16(RCNT_SERIAL, 0x8000);
        console.bus.write_u16(SIOCNT_ADDR, 0x0000);
        console.step();

        assert_eq!(
            console.bus.read_io_direct_u16(0x128),
            0x0000,
            "the port is not a link cable right now"
        );
    }

    /// A console waiting for data that never comes has to be able to give up.
    ///
    /// Waiting is what keeps consoles in step, and it works by not running the
    /// game at all. That makes a lost message unrecoverable unless there is a
    /// way out: a child that joined a transfer and was never sent the result
    /// sat frozen on "Awaiting linkup" for good, because nothing on its side
    /// had a deadline. The parent had one; the child had none.
    #[test]
    fn a_console_can_give_up_on_a_transfer_that_never_arrives() {
        let mut child = linked_console(1, 2, 0x1234);
        assert!(child.link_join());
        assert!(child.link_transfer_pending(), "joined and waiting");
        assert_eq!(
            child.bus.read_io_direct_u16(0x128) & START,
            START,
            "the game should see the cable as busy"
        );

        assert!(child.link_abandon());
        assert!(!child.link_transfer_pending(), "the wait is over");
        assert!(!child.sio.busy());
        assert_eq!(
            child.bus.read_io_direct_u16(0x128) & START,
            0,
            "the game has to be let go of, or it never runs again"
        );

        // Nothing to give up on the second time.
        assert!(!child.link_abandon());
    }

    /// Giving up must not throw away a transfer that is merely still clocking:
    /// that data has arrived and is on its way to the game.
    #[test]
    fn giving_up_leaves_a_transfer_that_is_already_landing() {
        let parent = linked_console(0, 2, 0xAAAA);
        let child = linked_console(1, 2, 0xBBBB);
        let mut consoles = vec![parent, child];

        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        assert!(link_step(&mut consoles));

        assert!(
            !consoles[1].link_abandon(),
            "the data is in and clocking; there is nothing to abandon"
        );

        let cycles = crate::sio::Sio::transfer_cycles(MULTI_115200, 2);
        for _ in 0..cycles {
            for console in consoles.iter_mut() {
                console.step();
            }
        }
        assert_eq!(slots(&consoles[1]), vec![0xAAAA, 0xBBBB, 0xFFFF, 0xFFFF]);
    }

    #[test]
    fn a_child_cannot_start_a_transfer() {
        let mut consoles = vec![linked_console(0, 2, 0xAAAA), linked_console(1, 2, 0xBBBB)];

        // The child sets START. That bit is read-only for a child, so nothing
        // at all should happen.
        consoles[1].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[1].step();
        assert!(!consoles[1].link_transfer_pending());
        assert!(!consoles[1].sio.busy());
        assert_eq!(consoles[1].bus.read_io_direct_u16(0x128) & START, 0);

        // And the cable stays idle: no data moves.
        for _ in 0..100 {
            assert!(!link_step(&mut consoles));
            for console in consoles.iter_mut() {
                console.step();
            }
        }
        assert_eq!(consoles[0].bus.read_io_direct_u16(0x120), 0x0000);
    }

    #[test]
    fn a_transfer_raises_an_interrupt_when_the_game_asks_for_one() {
        let mut consoles = vec![linked_console(0, 2, 0x0042), linked_console(1, 2, 0x0043)];

        // Bit 14 asks for an interrupt instead of a poll.
        for console in consoles.iter_mut() {
            console.bus.write_u16(SIOCNT_ADDR, MULTI_115200 | 0x4000);
            console.step();
            // Clear IF so the assertion below cannot pass on a stale bit.
            console.bus.write_io_direct_u16(0x202, 0);
        }
        consoles[0]
            .bus
            .write_u16(SIOCNT_ADDR, MULTI_115200 | 0x4000 | START);
        consoles[0].step();
        run_until_idle(&mut consoles, 20_000);

        for (which, console) in consoles.iter().enumerate() {
            assert_eq!(
                console.bus.read_io_direct_u16(0x202) & crate::sio::IRQ_BIT,
                crate::sio::IRQ_BIT,
                "console {which} was never told the transfer landed"
            );
        }
    }

    #[test]
    fn no_interrupt_when_the_game_did_not_ask() {
        let mut consoles = vec![linked_console(0, 2, 0x0042), linked_console(1, 2, 0x0043)];
        for console in consoles.iter_mut() {
            console.bus.write_io_direct_u16(0x202, 0);
        }
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        run_until_idle(&mut consoles, 20_000);

        for console in consoles.iter() {
            assert_eq!(
                console.bus.read_io_direct_u16(0x202) & crate::sio::IRQ_BIT,
                0
            );
        }
    }

    #[test]
    fn a_departed_console_reads_as_absent_rather_than_as_its_last_message() {
        let mut consoles = vec![linked_console(0, 2, 0x1234), linked_console(1, 2, 0x5678)];
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        run_until_idle(&mut consoles, 20_000);
        assert_eq!(slots(&consoles[0])[1], 0x5678);

        // Now the child leaves and the parent transfers alone. The old
        // halfword must not still be sitting in slot 1 pretending to be live.
        consoles.truncate(1);
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        assert_eq!(
            slots(&consoles[0])[1],
            0xFFFF,
            "a departed console must read as absent, not as its last message"
        );
    }

    #[test]
    fn the_port_does_nothing_when_it_is_not_a_link_cable() {
        let mut consoles = vec![linked_console(0, 2, 0x1234), linked_console(1, 2, 0x5678)];

        // A cartridge switching the port to general-purpose pins, which is how
        // a real-time clock is wired. Setting START must not move any data.
        consoles[0].bus.write_u16(RCNT_SERIAL, 0x8000);
        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        assert!(!consoles[0].link_transfer_pending());
        assert!(!link_step(&mut consoles));
    }

    #[test]
    fn a_game_already_waiting_at_the_link_menu_picks_up_a_late_cable() {
        // The order a lobby actually produces: you reach the link screen and
        // sit there, and somebody joins a while later. Everything the game did
        // to the serial registers happened before there was a cable at all.
        let mut parent = Gba::new();
        parent.start();
        parent.cpu.halted = true;
        parent.bus.write_u16(RCNT_SERIAL, 0x0000);
        parent.bus.write_u16(SIOCNT_ADDR, MULTI_115200);
        parent.bus.write_u16(SIOMLT_SEND_ADDR, 0x0BED);
        for _ in 0..50 {
            parent.step();
        }
        assert_eq!(
            parent.bus.read_io_direct_u16(0x128) & 0x0008,
            0,
            "nobody is on the other end yet"
        );

        // Now the cable goes in.
        parent.link_connect(0, 2);
        assert_eq!(
            parent.bus.read_io_direct_u16(0x128) & 0x0008,
            0x0008,
            "the game must be able to see that somebody arrived"
        );

        let child = linked_console(1, 2, 0x0FAB);
        let mut consoles = vec![parent, child];

        consoles[0].bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        consoles[0].step();
        run_until_idle(&mut consoles, 20_000);

        assert_eq!(slots(&consoles[0]), vec![0x0BED, 0x0FAB, 0xFFFF, 0xFFFF]);
        assert_eq!(slots(&consoles[1]), vec![0x0BED, 0x0FAB, 0xFFFF, 0xFFFF]);
    }

    #[test]
    fn unplugging_the_cable_shows_a_bad_connection() {
        let mut console = linked_console(0, 2, 0x1234);
        assert_eq!(
            console.bus.read_io_direct_u16(0x128) & 0x0008,
            0x0008,
            "SD is high while somebody is on the other end"
        );

        console.link_disconnect();
        assert_eq!(
            console.bus.read_io_direct_u16(0x128) & 0x0008,
            0,
            "SD drops when the cable comes out"
        );

        console.bus.write_u16(SIOCNT_ADDR, MULTI_115200 | START);
        console.step();
        assert!(
            !console.sio.busy(),
            "an unplugged console transfers nothing"
        );
    }

    /// States written before version 4 still have the cartridge inside them,
    /// and have to keep loading: they are somebody's progress.
    #[test]
    fn a_version_3_savestate_still_loads_with_its_own_cartridge() {
        let mut gba = Gba::new();
        gba.load_rom(cartridge(1024 * 1024));
        gba.write_u32(0x0300_0000, 0x0BAD_F00D);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(SAVESTATE_MAGIC);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend(bincode::serialize(&Savestate::from(&gba)).expect("serialize v3"));

        // No cartridge in this machine: a version 3 state brought its own.
        let mut restored = Gba::new();
        restored
            .load_state_bytes(&bytes)
            .expect("deserialize v3 savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0x0BAD_F00D);
        assert_eq!(restored.read_u32(0x0800_00AC), u32::from_le_bytes(*b"BPRE"));
    }
}

#[cfg(test)]
mod headless_tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_run_frames_headless() {
        let rom = match std::fs::read("../../roms/PokemonFireRed.gba") {
            Ok(r) => r,
            Err(_) => {
                println!("ROM not found, skipping");
                return;
            }
        };
        let mut gba = Gba::new();
        gba.load_rom(rom);

        // Run until we enter the stuck loop, then step-trace it
        for _ in 0..9 {
            gba.run_frame();
        }
        // Run into stuck state, then step-trace at loop
        for _ in 0..13 {
            gba.run_frame();
        }
        // Now step and capture state BEFORE executing 0x080008bc
        let mut irq_count = 0;
        let mut loop_count = 0;
        for _ in 0..5_000_000 {
            let pre_pc = gba.cpu.fetch_addr();
            if pre_pc == 0x080008bc && loop_count < 3 {
                let r4 = gba.cpu.registers.get_reg(4);
                let val = gba.bus.read_u16(r4.wrapping_add(28));
                eprintln!(
                    "LOOP iter {}: R4={:08x} [R4+28]={:04x}",
                    loop_count, r4, val
                );
                loop_count += 1;
            }
            // Count VBlank IRQs by checking when we jump to 0x18
            if pre_pc == 0x00000018 {
                irq_count += 1;
                if irq_count <= 3 {
                    let r4 = gba.cpu.registers.get_reg(4);
                    eprintln!("IRQ #{} pc_was=0x18 R4={:08x}", irq_count, r4);
                }
            }
            gba.step();
        }
        eprintln!("Total IRQs entering 0x18: {}", irq_count);

        let start = std::time::Instant::now();
        for i in 0..60 {
            gba.run_frame();
            let dispcnt = gba.bus.read_io_direct_u16(0x000);
            let dispstat = gba.bus.read_io_direct_u16(0x004);
            let ie = gba.bus.read_io_direct_u16(0x200);
            let if_reg = gba.bus.read_io_direct_u16(0x202);
            let ime = gba.bus.read_io_direct_u16(0x208);
            let handler = {
                let lo = gba.bus.read_u8(0x03007FFC) as u32;
                let hi = gba.bus.read_u8(0x03007FFD) as u32;
                let h2 = gba.bus.read_u8(0x03007FFE) as u32;
                let h3 = gba.bus.read_u8(0x03007FFF) as u32;
                lo | (hi << 8) | (h2 << 16) | (h3 << 24)
            };
            if i < 6 || i % 10 == 0 {
                eprintln!("Frame {}: pc={:08x} DISPCNT={:04x} DISPSTAT={:04x} IE={:04x} IF={:04x} IME={:04x} h={:08x}",
                    i, gba.pc(), dispcnt, dispstat, ie, if_reg, ime, handler);
            }
        }
        let elapsed = start.elapsed();
        eprintln!(
            "60 frames in {:.2}ms ({:.1} fps)",
            elapsed.as_millis(),
            60000.0 / elapsed.as_millis() as f64
        );
    }
}

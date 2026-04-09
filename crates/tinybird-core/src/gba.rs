//! Main GBA Emulator Struct
//!
//! This module provides the top-level GBA emulator structure that ties together
//! all the subsystems: CPU, memory, display, audio, input, etc.

use crate::apu::Apu;
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
pub const CYCLES_PER_FRAME: u32 =
    crate::ppu::TOTAL_SCANLINES * crate::ppu::CYCLES_PER_SCANLINE;

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

/// Main GBA emulator struct
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
    /// Last status bits mirrored into DISPSTAT.
    last_dispstat_status_bits: u16,
    /// Last scanline mirrored into VCOUNT.
    last_vcount: u16,
    /// Last software-controlled DISPCNT value synced into the PPU.
    last_dispcnt: u16,
    /// Last software-controlled DISPSTAT bits synced into the PPU.
    last_dispstat_control: u16,
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
            dma: DmaController::new(),
            timers: TimerController::new(),
            input: Input::new(),
            scheduler: Scheduler::new(),
            state: GbaState::Stopped,
            speed: EmulationSpeed::FullSpeed,
            audio_enabled: true,
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
    pub fn reset(&mut self) {
        self.cpu.reset();
        self.ppu.reset();
        self.scheduler.clear();
        self.total_cycles = 0;
        self.frame_count = 0;
        self.state = GbaState::Running;
        self.apu.reset();
        self.last_dispstat_status_bits = u16::MAX;
        self.last_vcount = u16::MAX;
        self.last_dispcnt = u16::MAX;
        self.last_dispstat_control = u16::MAX;

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
        self.cpu.step(&mut self.bus);
        self.total_cycles += 1;
        if self.audio_enabled {
            self.sync_io_to_apu();
        }

        // Check if HALT was requested via HALTCNT (I/O offset 0x301).
        // The BIOS SWI 0x02 handler writes 0x00 there; we detect it here and
        // freeze the CPU. A value of 0xFF means no halt is pending.
        if !self.cpu.halted && self.bus.read_io_direct(0x301) == 0x00 {
            self.cpu.halted = true;
            self.bus.write_io_direct(0x301, 0xFF); // clear the request
        }

        // Wake from HALT: any pending interrupt (IE & IF) resumes the CPU,
        // regardless of IME or the I flag in CPSR.
        if self.cpu.halted {
            let if_reg = self.bus.read_io_direct_u16(0x202);
            let ie = self.bus.read_io_direct_u16(0x200);
            if (ie & if_reg) != 0 {
                self.cpu.halted = false;
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
                eprintln!("PC WENT INVALID: {:08x} -> {:08x} (cycle {})", pc_before, pc_after, self.total_cycles);
                for r in 0..16 {
                    eprint!("  r{}={:08x}", r, self.cpu.registers.get_reg(r));
                }
                eprintln!("  cpsr={:08x}", self.cpu.registers.cpsr());
            }
        }

        // Sync timer controls from I/O (check frequently to catch enable transitions)
        if (self.total_cycles & 15) == 0 {
            self.sync_timer_controls();
        }

        // Tick timers every cycle
        self.tick_timers();

        // Tick audio every cycle so timer-driven FIFO playback and frame
        // sequencer state can advance while the BIOS sound driver is active.
        if self.audio_enabled {
            self.apu.tick(1);
        }

        // Check for DMA transfers only when a DMA register was written
        if self.bus.take_dma_dirty() {
            self.check_dma();
        }

        // Step PPU (runs at same clock speed as CPU)
        // Keep timing-critical LCD control state current every cycle so
        // DISPSTAT/DISPCNT writes from IRQ callbacks are visible immediately,
        // while leaving the heavier BG/window sync on scanline boundaries.
        let prev_cycle = self.ppu.cycle;
        self.sync_lcd_state_to_ppu();
        let ppu_events = self.ppu.step();
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
                self.dma.on_hblank();
                self.run_pending_dma_channels();

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
            }

            if lcd_irq_mask != 0 {
                let if_reg = self.bus.read_io_direct_u16(0x202);
                self.bus.write_io_direct_u16(0x202, if_reg | lcd_irq_mask);
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
        if self.ppu.cycle == 1 && prev_cycle != 1 {
            self.sync_io_to_ppu();
        }

        self.scheduler.advance(1);

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

        loop {
            self.step();
            if self.ppu.scanline == 0 && self.ppu.cycle == 0 {
                break;
            }
        }

        self.frame_count += 1;
        self.total_cycles - start_cycles
    }

    /// Run for multiple frames
    pub fn run_frames(&mut self, frames: u32) -> u64 {
        let mut total = 0;
        for _ in 0..frames {
            total += self.run_frame();
        }
        total
    }

    /// Handle a scheduled event
    fn handle_event(&mut self, event_type: crate::scheduler::EventType) {
        match event_type {
            crate::scheduler::EventType::VBlank => {
                // Trigger VBlank interrupt
                self.cpu.irq();
            }
            crate::scheduler::EventType::HBlank => {
                // HBlank handling (optional interrupt)
            }
            crate::scheduler::EventType::TimerOverflow(_) => {
                // Timer interrupt
                self.cpu.irq();
            }
            crate::scheduler::EventType::Dma(_) => {
                // DMA complete
            }
            crate::scheduler::EventType::Serial => {
                // Serial communication complete
            }
            crate::scheduler::EventType::Custom(_) => {
                // Custom event
            }
        }
    }

    /// Get the current PC
    pub fn pc(&self) -> u32 {
        self.cpu.pc()
    }

    /// Serialize the full emulator state into a savestate blob.
    pub fn save_state_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Restore a full emulator state from a savestate blob.
    pub fn load_state_bytes(&mut self, bytes: &[u8]) -> Result<(), bincode::Error> {
        *self = bincode::deserialize(bytes)?;
        Ok(())
    }

    /// Return whether the current cartridge exposes persistent save memory.
    pub fn has_persistent_save(&self) -> bool {
        self.bus.has_persistent_save()
    }

    /// Return a snapshot of cartridge-backed save memory.
    pub fn save_data(&self) -> Option<&[u8]> {
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
    fn tick_timers(&mut self) {
        let overflowed = self.timers.tick(1);

        // Write enabled timer counters back to I/O
        for i in 0..4 {
            if self.timers.timers[i].is_enabled() {
                self.bus.write_io_direct_u16(0x100 + i * 4, self.timers.timers[i].counter);
            }
        }

        // Handle timer overflow IRQs (rare path)
        if overflowed.iter().any(|&x| x) {
            let ime = self.bus.read_io_direct_u16(0x208);
            let ie = self.bus.read_io_direct_u16(0x200);
            for i in 0..4 {
                if self.audio_enabled && overflowed[i] && i < 2 {
                    let (need_a, need_b) = self.apu.on_timer_overflow(i as u8);
                    if need_a {
                        self.service_sound_fifo_dma(0);
                    }
                    if need_b {
                        self.service_sound_fifo_dma(1);
                    }
                }
                if overflowed[i] && self.timers.timers[i].irq_enabled() {
                    let bit = 1u16 << (3 + i);
                    let if_reg = self.bus.read_io_direct_u16(0x202);
                    self.bus.write_io_direct_u16(0x202, if_reg | bit);
                    if ime != 0 && (ie & bit) != 0 {
                        self.cpu.irq();
                    }
                }
            }
        }
    }

    /// Sync sound I/O writes from the bus into the APU model.
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
            if let Some((_, bytes, len, irq)) = self.dma.run_sound_fifo(channel, &mut self.bus)
            {
                self.apu.fifo_write(fifo, &bytes[..len]);
                if irq {
                    let bit = 1u16 << (8 + channel);
                    let if_reg = self.bus.read_io_direct_u16(0x202);
                    self.bus.write_io_direct_u16(0x202, if_reg | bit);
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
                let if_reg = self.bus.read_io_direct_u16(0x202);
                self.bus.write_io_direct_u16(0x202, if_reg | bit);

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
            let was_enabled = self.dma.channels[ch].is_enabled();

            // Detect rising edge: I/O has enable set but DMA controller doesn't yet
            if (io_control & 0x8000 != 0) && !was_enabled {
                let source = self.bus.read_io_direct_u16(sad) as u32
                    | ((self.bus.read_io_direct_u16(sad + 2) as u32) << 16);
                let dest = self.bus.read_io_direct_u16(dad) as u32
                    | ((self.bus.read_io_direct_u16(dad + 2) as u32) << 16);
                let count = self.bus.read_io_direct_u16(cnt_l);

                if dma_debug {
                    let bits = if io_control & 0x0400 != 0 { 32 } else { 16 };
                    let timing = (io_control >> 12) & 3;
                    eprintln!("DMA{} enable: src={:08x} dst={:08x} count={} {}bit timing={} ctrl={:04x}",
                        ch, source, dest, count, bits, timing, io_control);
                }

                self.dma.write_source(ch, source);
                self.dma.write_dest(ch, dest);
                self.dma.write_count(ch, count);
                self.dma.write_control(ch, io_control);

                // Execute pending immediate DMA
                if self.dma.channels[ch].pending {
                    self.run_pending_dma_channels();
                }
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

    /// Sync I/O registers from bus to PPU (lightweight, called per scanline)
    fn sync_io_to_ppu(&mut self) {
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

        // Sync VRAM, palette, and OAM from bus to PPU
        self.sync_video_memory();
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
    fn test_sync_video_memory_invalidates_sprite_cache_when_oam_changes() {
        let mut gba = Gba::new();
        gba.start();

        // Give OBJ tile 0 a solid visible pattern and palette entry 1 a visible color.
        for offset in 0..32u32 {
            gba.bus.write_u8(0x0601_0000 + offset, 0x11);
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
        gba.write_u32(0x0300_0000, 0x1234_5678);
        gba.speed = EmulationSpeed::Turbo;
        gba.audio_enabled = false;

        let bytes = gba.save_state_bytes().expect("serialize savestate");

        let mut restored = Gba::new();
        restored.load_state_bytes(&bytes).expect("deserialize savestate");

        assert_eq!(restored.read_u32(0x0300_0000), 0x1234_5678);
        assert_eq!(restored.speed, EmulationSpeed::Turbo);
        assert!(!restored.audio_enabled);
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
            Err(_) => { println!("ROM not found, skipping"); return; }
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
                eprintln!("LOOP iter {}: R4={:08x} [R4+28]={:04x}", loop_count, r4, val);
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
        eprintln!("60 frames in {:.2}ms ({:.1} fps)", elapsed.as_millis(), 60000.0 / elapsed.as_millis() as f64);
    }
}

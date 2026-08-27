//! ARM7TDMI CPU Core Module
//!
//! This module implements the ARM7TDMI CPU core, including:
//! - Register file with mode banking
//! - 3-stage pipeline (Fetch, Decode, Execute)
//! - ARM instruction set (32-bit)
//! - Thumb instruction set (16-bit)

pub mod arm;
pub mod pipeline;
pub mod registers;
pub mod thumb;

pub use arm::{decode_arm, execute_arm};
pub use pipeline::{Instruction, InstructionCategory, Pipeline};
pub use registers::{CpuMode, Registers};
pub use thumb::{decode_thumb, execute_thumb};

use crate::bus::Bus;
use crate::debug::config as debug_config;
use serde::{Deserialize, Serialize};

#[inline]
pub(crate) fn armv4_load_word<B: Bus>(bus: &B, addr: u32) -> u32 {
    let aligned = bus.read_u32(addr & !3);
    aligned.rotate_right((addr & 3) * 8)
}

/// `LDRH`, including the rotation a misaligned address causes.
///
/// The rotate is of the 32-bit register value, not of the halfword: loading
/// `0x0020` from an odd address gives `0x20000000`, not `0x00002000`. Rotating
/// the narrow value instead puts the bytes in the wrong half of the register.
#[inline]
pub(crate) fn armv4_load_halfword<B: Bus>(bus: &B, addr: u32) -> u32 {
    let aligned = bus.read_u16(addr & !1) as u32;
    if (addr & 1) != 0 {
        aligned.rotate_right(8)
    } else {
        aligned
    }
}

#[inline]
pub(crate) fn armv4_load_signed_halfword<B: Bus>(bus: &B, addr: u32) -> u32 {
    if (addr & 1) != 0 {
        bus.read_u8(addr) as i8 as i32 as u32
    } else {
        bus.read_u16(addr) as i16 as i32 as u32
    }
}

#[inline]
pub(crate) fn align_loaded_pc(value: u32, thumb: bool) -> u32 {
    if thumb {
        value & !1
    } else {
        value & !3
    }
}

/// CPU core state
#[derive(Clone, Serialize, Deserialize)]
pub struct Cpu {
    /// Register file
    pub registers: Registers,
    /// Instruction pipeline
    pub pipeline: Pipeline,
    /// Number of cycles executed
    pub cycles: u64,
    /// Whether the CPU is halted
    pub halted: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// Create a new CPU core
    pub fn new() -> Self {
        Self {
            registers: Registers::new(),
            pipeline: Pipeline::new(),
            cycles: 0,
            halted: false,
        }
    }

    /// Create a new CPU with a specific start address
    pub fn with_start_addr(addr: u32, thumb: bool) -> Self {
        Self {
            registers: Registers::new(),
            pipeline: Pipeline::with_start_addr(addr, thumb),
            cycles: 0,
            halted: false,
        }
    }

    /// Reset the CPU to its initial state
    pub fn reset(&mut self) {
        self.registers = Registers::new();
        self.pipeline = Pipeline::new();
        self.cycles = 0;
        self.halted = false;
    }

    /// Execute a single instruction
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if self.halted {
            return 1;
        }

        // no trace

        // Use direct fetch-decode-execute (no pipeline simulation)
        bus.begin_instruction_timing();
        self.execute(bus);
        bus.finish_instruction_timing()
    }

    /// Execute a single instruction (without pipeline simulation)
    pub fn execute<B: Bus>(&mut self, bus: &mut B) {
        if self.halted {
            return;
        }

        let instr_addr = self.pipeline.fetch_addr;
        // Set execute_addr so pc() returns instr_addr + 8/4
        self.pipeline.execute_addr = instr_addr;

        // Update R15 to reflect the PC as seen by the instruction
        // ARM: PC = instruction_addr + 8, Thumb: PC = instruction_addr + 4
        let pc_val = if self.registers.is_thumb_mode() {
            instr_addr.wrapping_add(4)
        } else {
            instr_addr.wrapping_add(8)
        };
        self.registers.set_pc(pc_val);

        // Fetch
        let mut instruction = self.pipeline.fetch(bus);

        // Decode
        if instruction.is_thumb {
            decode_thumb(&mut instruction);
        } else {
            decode_arm(&mut instruction);
        }

        // Execute
        self.execute_instruction(bus, &instruction);

        // Advance PC only if no branch was taken
        if self.pipeline.fetch_addr == instr_addr {
            if instruction.is_thumb {
                self.pipeline.fetch_addr = self.pipeline.fetch_addr.wrapping_add(2);
            } else {
                self.pipeline.fetch_addr = self.pipeline.fetch_addr.wrapping_add(4);
            }
        }

        self.cycles += 1;
    }

    /// Execute an instruction
    fn execute_instruction<B: Bus>(&mut self, bus: &mut B, instruction: &Instruction) {
        // Update thumb mode from CPSR
        self.pipeline.set_thumb_mode(self.registers.is_thumb_mode());

        // Debug logging
        if debug_config().cpu_debug {
            let mode = if instruction.is_thumb { "THUMB" } else { "ARM" };
            let pc = self.pipeline.pc();
            println!("CPU[{:08x}] {}: {:?}", pc, mode, instruction);
        }

        if instruction.is_thumb {
            execute_thumb(instruction, bus, &mut self.registers, &mut self.pipeline);
        } else {
            execute_arm(instruction, bus, &mut self.registers, &mut self.pipeline);
        }
    }

    /// Get the current PC value
    pub fn pc(&self) -> u32 {
        self.pipeline.pc()
    }

    /// Get the current instruction address (fetch address)
    pub fn fetch_addr(&self) -> u32 {
        self.pipeline.fetch_addr
    }

    /// Check if in Thumb mode
    pub fn is_thumb_mode(&self) -> bool {
        self.registers.is_thumb_mode()
    }

    /// Set Thumb mode
    pub fn set_thumb_mode(&mut self, thumb: bool) {
        self.registers.set_thumb_mode(thumb);
        self.pipeline.set_thumb_mode(thumb);
    }

    /// Trigger an IRQ exception
    pub fn irq(&mut self) {
        // IRQ is enabled when CPSR bit 7 (I flag) is 0
        let cpsr = self.registers.cpsr();
        if debug_config().irq_debug {
            eprintln!(
                "[cpu.irq] cpsr={:08x} I={} pc={:08x} -> fires={}",
                cpsr,
                (cpsr >> 7) & 1,
                self.pipeline.fetch_addr,
                (cpsr & (1 << 7)) == 0
            );
        }
        if (cpsr & (1 << 7)) == 0 {
            let return_addr = if self.registers.is_thumb_mode() {
                self.pipeline.fetch_addr.wrapping_add(4)
            } else {
                self.pipeline.fetch_addr.wrapping_add(4)
            };
            self.registers
                .enter_exception_with_return_addr(CpuMode::IRQ, return_addr);
            self.pipeline.branch_with_mode(0x00000018, false); // IRQ vector
            self.pipeline.flush();
        }
    }

    /// Trigger an FIQ exception
    pub fn fiq(&mut self) {
        // FIQ is enabled when CPSR bit 6 (F flag) is 0
        if (self.registers.cpsr() & (1 << 6)) == 0 {
            let return_addr = self.pipeline.fetch_addr.wrapping_add(4);
            self.registers
                .enter_exception_with_return_addr(CpuMode::FIQ, return_addr);
            self.pipeline.branch_with_mode(0x0000001C, false); // FIQ vector
            self.pipeline.flush();
        }
    }

    /// Trigger a software interrupt
    pub fn swi(&mut self, _num: u32) {
        self.registers.enter_exception(CpuMode::Supervisor, 4);
        self.pipeline.branch_with_mode(0x00000008, false); // SWI vector
        self.pipeline.flush();
    }

    /// Trigger an undefined instruction exception
    pub fn undefined(&mut self) {
        self.registers.enter_exception(CpuMode::Undefined, 4);
        self.pipeline.branch_with_mode(0x00000004, false); // Undefined vector
        self.pipeline.flush();
    }

    /// Trigger a prefetch abort exception
    pub fn prefetch_abort(&mut self) {
        self.registers.enter_exception(CpuMode::Abort, 4);
        self.pipeline.branch_with_mode(0x0000000C, false); // Prefetch abort vector
        self.pipeline.flush();
    }

    /// Trigger a data abort exception
    pub fn data_abort(&mut self) {
        self.registers.enter_exception(CpuMode::Abort, 8);
        self.pipeline.branch_with_mode(0x00000010, false); // Data abort vector
        self.pipeline.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SimpleBus;

    #[test]
    fn test_cpu_new() {
        let cpu = Cpu::new();
        assert!(!cpu.halted);
        assert_eq!(cpu.cycles, 0);
    }

    #[test]
    fn test_cpu_reset() {
        let mut cpu = Cpu::new();
        cpu.cycles = 100;
        cpu.halted = true;
        cpu.reset();
        assert_eq!(cpu.cycles, 0);
        assert!(!cpu.halted);
    }

    #[test]
    fn test_bx_arm_to_thumb() {
        let mut cpu = Cpu::new();
        let mut bus = SimpleBus::new(None);

        // ARM mode BX instruction: E12FFF11 = BX r1
        // Set r1 to 0x08000001 (bit 0 set for Thumb mode)
        cpu.registers.set_reg(1, 0x08000001);
        cpu.registers.set_pc(0x00000000);

        // Execute BX in ARM mode
        cpu.set_thumb_mode(false);
        let mut instr = Instruction::arm(0xE12FFF11);
        decode_arm(&mut instr);

        // Debug: Check if decoded as BX
        assert!(instr.decoded.is_some());
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Bx);
        assert_eq!(decoded.rn, Some(1)); // Rm is in rn field

        execute_arm(&instr, &mut bus, &mut cpu.registers, &mut cpu.pipeline);

        // Should switch to Thumb mode
        assert!(cpu.is_thumb_mode());
    }

    #[test]
    fn test_swi_hle() {
        let mut cpu = Cpu::new();
        let mut bus = SimpleBus::new(None);

        // ARM mode SWI 0x05 (VBlankIntrWait)
        cpu.registers.set_pc(0x00000000);

        // Execute SWI in ARM mode
        cpu.set_thumb_mode(false);
        let mut instr = Instruction::arm(0xEF000005); // SWI 0x05
        decode_arm(&mut instr);

        // Debug: Check if decoded
        assert!(instr.decoded.is_some());
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Swi);

        execute_arm(&instr, &mut bus, &mut cpu.registers, &mut cpu.pipeline);

        // BIOS HLE should handle this
        // After SWI, R0 should be set to 1 (discard flags), R1 = 1 (VBlank flag)
        assert_eq!(cpu.registers.get_reg(0), 1);
        assert_eq!(cpu.registers.get_reg(1), 1);
    }

    #[test]
    fn test_halt_sets_haltcnt() {
        // SWI 0x02 (HALT) should write 0x00 to HALTCNT (I/O offset 0x301)
        let mut cpu = Cpu::new();
        let mut bus = SimpleBus::new(None);

        cpu.set_thumb_mode(false);
        // SWI 0x02 in ARM: EF000002
        let mut instr = Instruction::arm(0xEF000002);
        decode_arm(&mut instr);
        execute_arm(&instr, &mut bus, &mut cpu.registers, &mut cpu.pipeline);

        assert_eq!(
            bus.read_io_direct(0x301),
            0x00,
            "HALT SWI should write 0x00 to HALTCNT"
        );
    }

    #[test]
    fn test_irq_clears_halt() {
        use crate::gba::Gba;

        let mut gba = Gba::new();
        gba.start();

        // Manually halt the CPU
        gba.cpu.halted = true;

        // Set IE bit 0 (VBlank) and IF bit 0 (pending)
        gba.bus.write_io_direct_u16(0x200, 0x0001); // IE
        gba.bus.write_io_direct_u16(0x202, 0x0001); // IF

        // One step should wake the CPU (PPU/timer ticks, halt check runs)
        gba.step();

        assert!(
            !gba.cpu.halted,
            "CPU should wake from HALT when IE & IF != 0"
        );
    }

    #[test]
    fn test_irq_after_thumb_branch_returns_to_branch_target() {
        let mut cpu = Cpu::with_start_addr(0x0200_0000, true);
        let mut bus = SimpleBus::new(None);

        cpu.set_thumb_mode(true);
        cpu.registers
            .set_cpsr(cpu.registers.cpsr() & !(1 << 7), true);
        cpu.registers.set_reg(0, 0x0200_0101); // BX target in Thumb mode
        bus.write_u16(0x0200_0000, 0x4700); // BX R0

        cpu.step(&mut bus);
        assert_eq!(cpu.fetch_addr(), 0x0200_0100);

        cpu.irq();

        assert_eq!(cpu.fetch_addr(), 0x0000_0018);
        assert_eq!(cpu.registers.get_reg(14), 0x0200_0104);
        assert_eq!(cpu.registers.mode(), CpuMode::IRQ);
    }
}

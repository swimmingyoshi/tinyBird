//! ARM7TDMI Register File with Mode Banking
//!
//! The ARM7TDMI has 37 registers: 31 general-purpose registers and 6 status registers.
//! Registers are organized into 7 processor modes, with banked registers for fast context
//! switching during exceptions.

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

/// Processor modes (CPSR[4:0])
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum CpuMode {
    User = 0x10,
    FIQ = 0x11,
    IRQ = 0x12,
    Supervisor = 0x13,
    Abort = 0x17,
    Undefined = 0x1B,
    System = 0x1F,
}

/// Convert CpuMode to bank index (0-6)
fn mode_to_bank_index(mode: CpuMode) -> usize {
    match mode {
        CpuMode::User => 0,
        CpuMode::FIQ => 1,
        CpuMode::IRQ => 2,
        CpuMode::Supervisor => 3,
        CpuMode::Abort => 4,
        CpuMode::Undefined => 5,
        CpuMode::System => 6,
    }
}

impl CpuMode {
    /// Create a CpuMode from raw CPSR mode bits
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x1F {
            0x10 => CpuMode::User,
            0x11 => CpuMode::FIQ,
            0x12 => CpuMode::IRQ,
            0x13 => CpuMode::Supervisor,
            0x17 => CpuMode::Abort,
            0x1B => CpuMode::Undefined,
            0x1F => CpuMode::System,
            _ => CpuMode::User, // Default to User mode for invalid values
        }
    }

    /// Check if this mode banks R8-R12
    pub fn banks_r8_r12(self) -> bool {
        matches!(self, CpuMode::FIQ)
    }

    /// Check if this mode banks R13-R14
    pub fn banks_r13_r14(self) -> bool {
        matches!(
            self,
            CpuMode::FIQ | CpuMode::IRQ | CpuMode::Supervisor | CpuMode::Abort | CpuMode::Undefined
        )
    }

    /// Check if this mode has SPSR (all except User and System)
    pub fn has_spsr(self) -> bool {
        !matches!(self, CpuMode::User | CpuMode::System)
    }
}

// CPSR flags - bitflags macro generates its own internal structure
// Note: missing_docs warning is suppressed for bitflags-generated code
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[allow(missing_docs)]
    pub struct CpsrFlags: u32 {
        /// Negative condition flag
        const N = 1 << 31;
        /// Zero condition flag
        const Z = 1 << 30;
        /// Carry condition flag
        const C = 1 << 29;
        /// Overflow condition flag
        const V = 1 << 28;
        /// Q sticky overflow flag (ARM5TE)
        const Q = 1 << 27;
        /// Interrupt disable - IRQ
        const I = 1 << 7;
        /// Interrupt disable - FIQ
        const F = 1 << 6;
        /// Thumb state bit
        const T = 1 << 5;
    }
}

impl Default for CpsrFlags {
    fn default() -> Self {
        // Default: IRQ and FIQ enabled (bits set means disabled)
        CpsrFlags::I | CpsrFlags::F
    }
}

/// Register indices
#[allow(missing_docs)]
pub const R0: usize = 0;
#[allow(missing_docs)]
pub const R1: usize = 1;
#[allow(missing_docs)]
pub const R2: usize = 2;
#[allow(missing_docs)]
pub const R3: usize = 3;
#[allow(missing_docs)]
pub const R4: usize = 4;
#[allow(missing_docs)]
pub const R5: usize = 5;
#[allow(missing_docs)]
pub const R6: usize = 6;
#[allow(missing_docs)]
pub const R7: usize = 7;
#[allow(missing_docs)]
pub const R8: usize = 8;
#[allow(missing_docs)]
pub const R9: usize = 9;
#[allow(missing_docs)]
pub const R10: usize = 10;
#[allow(missing_docs)]
pub const R11: usize = 11;
#[allow(missing_docs)]
pub const R12: usize = 12;
#[allow(missing_docs)]
pub const R13: usize = 13;
#[allow(missing_docs)]
pub const R14: usize = 14;
#[allow(missing_docs)]
pub const R15: usize = 15;
#[allow(missing_docs)]
pub const PC: usize = R15;

/// Number of general-purpose registers per mode
const REGS_PER_MODE: usize = 16;

/// Total number of processor modes
const NUM_MODES: usize = 7;

/// Register file with mode banking
///
/// Memory layout:
/// - Mode 0 (User): R0-R15, CPSR
/// - Mode 1 (FIQ): R8_fiq-R14_fiq, SPSR_fiq (banks R8-R14)
/// - Mode 2 (IRQ): R13_irq, R14_irq, SPSR_irq (banks R13-R14)
/// - Mode 3 (Supervisor): R13_svc, R14_svc, SPSR_svc (banks R13-R14)
/// - Mode 4 (Abort): R13_abt, R14_abt, SPSR_abt (banks R13-R14)
/// - Mode 5 (Undefined): R13_und, R14_und, SPSR_und (banks R13-R14)
/// - Mode 6 (System): R0-R15, CPSR (same as User but privileged)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registers {
    /// All registers laid out as [mode][register]
    /// Mode order: User, FIQ, IRQ, Supervisor, Abort, Undefined, System
    /// Note: System mode shares registers with User mode
    banks: [[u32; REGS_PER_MODE]; NUM_MODES],
    /// Current Program Status Register
    cpsr: u32,
    /// Saved Program Status Registers for each exception mode
    /// Order: FIQ, IRQ, Supervisor, Abort, Undefined
    spsr: [u32; 5],
    /// Current processor mode
    mode: CpuMode,
    /// Thumb mode flag (cached from CPSR for performance)
    thumb_mode: bool,
}

impl Default for Registers {
    fn default() -> Self {
        Self::new()
    }
}

impl Registers {
    /// Create a new register file initialized to reset state
    pub fn new() -> Self {
        let mut regs = Self {
            banks: [[0; REGS_PER_MODE]; NUM_MODES],
            cpsr: CpsrFlags::default().bits(),
            spsr: [0; 5],
            mode: CpuMode::Supervisor, // Start in Supervisor mode after reset
            thumb_mode: false,
        };

        // Set initial mode flags in CPSR
        regs.cpsr = (regs.cpsr & !0x1F) | (regs.mode as u32);

        regs
    }

    /// Get the current processor mode
    pub fn mode(&self) -> CpuMode {
        self.mode
    }

    /// Get the current mode index
    fn mode_index(&self) -> usize {
        mode_to_bank_index(self.mode)
    }

    /// Get mode index for SPSR array
    fn spsr_index(mode: CpuMode) -> Option<usize> {
        match mode {
            CpuMode::FIQ => Some(0),
            CpuMode::IRQ => Some(1),
            CpuMode::Supervisor => Some(2),
            CpuMode::Abort => Some(3),
            CpuMode::Undefined => Some(4),
            _ => None,
        }
    }

    /// Get bank index for a mode
    fn bank_index(mode: CpuMode) -> usize {
        mode_to_bank_index(mode)
    }

    /// Check if currently in Thumb mode
    pub fn is_thumb_mode(&self) -> bool {
        self.thumb_mode
    }

    /// Set Thumb mode
    pub fn set_thumb_mode(&mut self, thumb: bool) {
        self.thumb_mode = thumb;
        if thumb {
            self.cpsr |= CpsrFlags::T.bits();
        } else {
            self.cpsr &= !CpsrFlags::T.bits();
        }
    }

    /// Get PC value (with pipeline offset already applied)
    pub fn pc(&self) -> u32 {
        self.get_reg(R15)
    }

    /// Set PC value (should be the actual target, pipeline offset added during fetch)
    pub fn set_pc(&mut self, value: u32) {
        self.set_reg(R15, value);
    }

    /// Get stack pointer (R13) for current mode
    pub fn sp(&self) -> u32 {
        self.get_reg(R13)
    }

    /// Set stack pointer
    pub fn set_sp(&mut self, value: u32) {
        self.set_reg(R13, value);
    }

    /// Get link register (R14) for current mode
    pub fn lr(&self) -> u32 {
        self.get_reg(R14)
    }

    /// Set link register
    pub fn set_lr(&mut self, value: u32) {
        self.set_reg(R14, value);
    }

    /// Get a general-purpose register for the current mode
    pub fn get_reg(&self, index: usize) -> u32 {
        let mode_idx = self.mode_index();

        // R0-R7 are NEVER banked — all modes share User mode's R0-R7
        if index < 8 {
            return self.banks[Self::bank_index(CpuMode::User)][index];
        }

        // R8-R12: FIQ mode has banked versions
        if index < 13 {
            if self.mode == CpuMode::FIQ {
                return self.banks[Self::bank_index(CpuMode::FIQ)][index];
            } else {
                // Use User mode registers for R8-R12
                return self.banks[Self::bank_index(CpuMode::User)][index];
            }
        }

        // R13-R14: Most exception modes have banked versions
        if index < 15 {
            if self.mode.banks_r13_r14() {
                return self.banks[mode_idx][index];
            } else {
                // Use User mode registers
                return self.banks[Self::bank_index(CpuMode::User)][index];
            }
        }

        // R15 (PC)
        self.banks[mode_idx][R15]
    }

    /// Set a general-purpose register for the current mode
    pub fn set_reg(&mut self, index: usize, value: u32) {
        let mode_idx = self.mode_index();

        // R0-R7 are NEVER banked — all modes share User mode's R0-R7
        if index < 8 {
            self.banks[Self::bank_index(CpuMode::User)][index] = value;
            return;
        }

        if index < 13 {
            if self.mode == CpuMode::FIQ {
                self.banks[Self::bank_index(CpuMode::FIQ)][index] = value;
            } else {
                self.banks[Self::bank_index(CpuMode::User)][index] = value;
            }
            return;
        }

        if index < 15 {
            if self.mode.banks_r13_r14() {
                self.banks[mode_idx][index] = value;
            } else {
                self.banks[Self::bank_index(CpuMode::User)][index] = value;
            }
            return;
        }

        // R15 (PC) - update thumb mode from CPSR
        self.banks[mode_idx][R15] = value;
    }

    /// Get CPSR
    pub fn cpsr(&self) -> u32 {
        self.cpsr
    }

    /// Set CPSR (with optional flag update control)
    pub fn set_cpsr(&mut self, value: u32, update_flags: bool) {
        // Preserve mode bits if not explicitly changing mode
        let mode_bits = value & 0x1F;
        let flags = if update_flags {
            // N, Z, C, V, Q, I, F, and T. The T bit is bit 5, so the low byte
            // has to be 0xFF: masking it out silently pinned the core in
            // whatever state it was already in, so an `MSR` that switched to
            // Thumb did nothing and execution carried on decoding ARM.
            value & 0xF000_00FF
        } else {
            (self.cpsr & 0xF000_0000) | (value & 0x0000_00DF)
        };

        self.cpsr = flags | mode_bits;
        self.mode = CpuMode::from_bits(self.cpsr as u8);
        self.thumb_mode = (self.cpsr & CpsrFlags::T.bits()) != 0;
    }

    /// Update only the condition flags in CPSR
    pub fn set_flags(&mut self, n: bool, z: bool, c: bool, v: bool) {
        self.cpsr &= !(CpsrFlags::N.bits()
            | CpsrFlags::Z.bits()
            | CpsrFlags::C.bits()
            | CpsrFlags::V.bits());

        if n {
            self.cpsr |= CpsrFlags::N.bits();
        }
        if z {
            self.cpsr |= CpsrFlags::Z.bits();
        }
        if c {
            self.cpsr |= CpsrFlags::C.bits();
        }
        if v {
            self.cpsr |= CpsrFlags::V.bits();
        }
    }

    /// Get N flag
    pub fn flag_n(&self) -> bool {
        (self.cpsr & CpsrFlags::N.bits()) != 0
    }

    /// Get Z flag
    pub fn flag_z(&self) -> bool {
        (self.cpsr & CpsrFlags::Z.bits()) != 0
    }

    /// Get C flag
    pub fn flag_c(&self) -> bool {
        (self.cpsr & CpsrFlags::C.bits()) != 0
    }

    /// Get V flag
    pub fn flag_v(&self) -> bool {
        (self.cpsr & CpsrFlags::V.bits()) != 0
    }

    /// Get SPSR for current mode (if available)
    pub fn spsr(&self) -> Option<u32> {
        if self.mode.has_spsr() {
            Self::spsr_index(self.mode).map(|idx| self.spsr[idx])
        } else {
            None
        }
    }

    /// Set SPSR for current mode (if available)
    pub fn set_spsr(&mut self, value: u32) {
        if let Some(idx) = Self::spsr_index(self.mode) {
            self.spsr[idx] = value;
        }
    }

    /// Get condition code from instruction
    pub fn get_cond(&self, cond: u8) -> bool {
        match cond {
            0b0000 => self.flag_z(),                                    // EQ - Equal
            0b0001 => !self.flag_z(),                                   // NE - Not equal
            0b0010 => self.flag_c(),  // CS/HS - Carry set / Higher or same
            0b0011 => !self.flag_c(), // CC/LO - Carry clear / Lower
            0b0100 => self.flag_n(),  // MI - Minus / Negative
            0b0101 => !self.flag_n(), // PL - Plus / Positive or zero
            0b0110 => self.flag_v(),  // VS - Overflow
            0b0111 => !self.flag_v(), // VC - No overflow
            0b1000 => self.flag_c() && !self.flag_z(), // HI - Higher
            0b1001 => !self.flag_c() || self.flag_z(), // LS - Lower or same
            0b1010 => self.flag_n() == self.flag_v(), // GE - Greater or equal
            0b1011 => self.flag_n() != self.flag_v(), // LT - Less than
            0b1100 => !self.flag_z() && self.flag_n() == self.flag_v(), // GT - Greater than
            0b1101 => self.flag_z() || self.flag_n() != self.flag_v(), // LE - Less or equal
            0b1110 => true,           // AL - Always
            0b1111 => true,           // NV - Never (deprecated, treat as AL)
            _ => unreachable!(),
        }
    }

    /// Switch to a new processor mode
    pub fn switch_mode(&mut self, new_mode: CpuMode) {
        if new_mode != self.mode {
            self.mode = new_mode;
            self.cpsr = (self.cpsr & !0x1F) | (new_mode as u32);
            self.thumb_mode = (self.cpsr & CpsrFlags::T.bits()) != 0;
        }
    }

    /// Initialize registers for an exception entry
    pub fn enter_exception(&mut self, new_mode: CpuMode, lr_offset: u32) {
        let pc = self.pc();
        let return_addr = pc.wrapping_sub(lr_offset);
        self.enter_exception_with_return_addr(new_mode, return_addr);
    }

    /// Initialize registers for an exception entry when the exact return
    /// address is already known (for example, an IRQ taken between
    /// instructions after a branch updated the fetch address).
    pub fn enter_exception_with_return_addr(&mut self, new_mode: CpuMode, return_addr: u32) {
        let cpsr_to_save = self.cpsr;

        // Save CPSR in the new mode's SPSR BEFORE switching
        if let Some(idx) = Self::spsr_index(new_mode) {
            self.spsr[idx] = cpsr_to_save;
        }

        // Switch to exception mode FIRST so the banked R14 is the exception mode's LR
        self.switch_mode(new_mode);
        self.cpsr |= CpsrFlags::I.bits(); // Disable IRQ
        if new_mode == CpuMode::FIQ {
            self.cpsr |= CpsrFlags::F.bits(); // Disable FIQ too
        }
        self.set_thumb_mode(false); // Exception handlers always start in ARM mode

        // Now write return address into the exception mode's banked R14
        self.set_reg(R14, return_addr);
    }

    /// Return from exception (restores full CPSR including T bit)
    pub fn return_from_exception(&mut self) {
        if let Some(spsr) = self.spsr() {
            // Restore every CPSR bit from SPSR, the T bit included. Assigning
            // directly rather than going through set_cpsr keeps the mode bits
            // exactly as the exception left them.
            self.cpsr = spsr;
            self.mode = CpuMode::from_bits(spsr as u8);
            self.thumb_mode = (spsr & CpsrFlags::T.bits()) != 0;
        }
    }

    /// Update Z and N flags based on result
    pub fn set_zn_flags(&mut self, result: u32) {
        self.set_flags(
            (result as i32) < 0, // N
            result == 0,         // Z
            self.flag_c(),       // C unchanged
            self.flag_v(),       // V unchanged
        );
    }

    /// Set N and Z flags from a 64-bit result (used by long multiply instructions)
    pub fn set_zn_flags64(&mut self, result: u64) {
        self.set_flags(
            (result as i64) < 0, // N
            result == 0,         // Z
            self.flag_c(),       // C unchanged
            self.flag_v(),       // V unchanged
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writing the T bit through the CPSR must actually take.
    ///
    /// The flag mask used to be `0xF000_00DF`, whose low byte clears bit 5 —
    /// the T bit. The core could therefore never be switched into Thumb by
    /// anything that went through `set_cpsr`, which is how `MSR` and every
    /// exception return get there. Games that enter Thumb that way ran on
    /// decoding ARM instead.
    #[test]
    fn the_thumb_bit_can_be_set_through_cpsr() {
        let mut regs = Registers::new();
        assert!(!regs.is_thumb_mode());

        let cpsr = regs.cpsr() | CpsrFlags::T.bits();
        regs.set_cpsr(cpsr, true);

        assert!(regs.is_thumb_mode(), "T should have taken");
        assert_ne!(
            regs.cpsr() & CpsrFlags::T.bits(),
            0,
            "T should be in the CPSR"
        );
    }

    #[test]
    fn the_thumb_bit_can_be_cleared_through_cpsr() {
        let mut regs = Registers::new();
        regs.set_cpsr(regs.cpsr() | CpsrFlags::T.bits(), true);
        assert!(regs.is_thumb_mode());

        regs.set_cpsr(regs.cpsr() & !CpsrFlags::T.bits(), true);
        assert!(!regs.is_thumb_mode());
    }

    /// The interrupt masks share the low byte with T and must still work.
    #[test]
    fn the_interrupt_masks_still_survive_a_cpsr_write() {
        let mut regs = Registers::new();
        regs.set_cpsr(
            regs.cpsr() | CpsrFlags::I.bits() | CpsrFlags::F.bits(),
            true,
        );
        assert_ne!(regs.cpsr() & CpsrFlags::I.bits(), 0);
        assert_ne!(regs.cpsr() & CpsrFlags::F.bits(), 0);
    }

    use super::*;

    #[test]
    fn test_register_basic() {
        let mut regs = Registers::new();

        regs.set_reg(R0, 0x12345678);
        assert_eq!(regs.get_reg(R0), 0x12345678);
    }

    #[test]
    fn test_mode_banking_r13_r14() {
        let mut regs = Registers::new();

        // Set R13/R14 in User mode
        regs.switch_mode(CpuMode::User);
        regs.set_reg(R13, 0x11111111);
        regs.set_reg(R14, 0x22222222);

        // Switch to IRQ mode and verify different registers
        regs.switch_mode(CpuMode::IRQ);
        assert_eq!(regs.get_reg(R13), 0); // Should be 0 (banked)
        assert_eq!(regs.get_reg(R14), 0);

        // Set IRQ mode registers
        regs.set_reg(R13, 0x33333333);
        regs.set_reg(R14, 0x44444444);

        // Switch back to User and verify original values
        regs.switch_mode(CpuMode::User);
        assert_eq!(regs.get_reg(R13), 0x11111111);
        assert_eq!(regs.get_reg(R14), 0x22222222);
    }

    #[test]
    fn test_fiq_banking_r8_r12() {
        let mut regs = Registers::new();

        // Set R8-R12 in User mode (use wrapping to avoid overflow in debug mode)
        for i in 8..=12 {
            regs.set_reg(i, (i as u32).wrapping_mul(0x10000000));
        }

        // Switch to FIQ mode
        regs.switch_mode(CpuMode::FIQ);

        // Should see zeros (banked registers)
        for i in 8..=12 {
            assert_eq!(regs.get_reg(i), 0);
        }

        // Set FIQ registers
        for i in 8..=12 {
            regs.set_reg(i, (i as u32).wrapping_mul(0x20000000));
        }

        // Switch back to User
        regs.switch_mode(CpuMode::User);

        // Verify original User values
        for i in 8..=12 {
            assert_eq!(regs.get_reg(i), (i as u32).wrapping_mul(0x10000000));
        }
    }

    #[test]
    fn test_condition_flags() {
        let mut regs = Registers::new();

        // Test EQ
        regs.set_flags(false, true, false, false); // Z set
        assert!(regs.get_cond(0b0000)); // EQ
        assert!(!regs.get_cond(0b0001)); // NE

        // Test CS/CC
        regs.set_flags(false, false, true, false); // C set
        assert!(regs.get_cond(0b0010)); // CS
        assert!(!regs.get_cond(0b0011)); // CC

        // Test MI/PL
        regs.set_flags(true, false, false, false); // N set
        assert!(regs.get_cond(0b0100)); // MI
        assert!(!regs.get_cond(0b0101)); // PL

        // Test VS/VC
        regs.set_flags(false, false, false, true); // V set
        assert!(regs.get_cond(0b0110)); // VS
        assert!(!regs.get_cond(0b0111)); // VC
    }

    #[test]
    fn test_spsr() {
        let mut regs = Registers::new();

        // User mode has no SPSR
        regs.switch_mode(CpuMode::User);
        assert!(regs.spsr().is_none());

        // IRQ mode has SPSR
        regs.switch_mode(CpuMode::IRQ);
        regs.set_spsr(0xDEADBEEF);
        assert_eq!(regs.spsr(), Some(0xDEADBEEF));
    }
}

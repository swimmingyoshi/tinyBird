//! ARM7TDMI 3-Stage Pipeline
//!
//! The ARM7TDMI uses a 3-stage pipeline:
//! 1. Fetch - Fetch instruction from memory at PC
//! 2. Decode - Decode the fetched instruction
//! 3. Execute - Execute the instruction
//!
//! Due to the pipeline, the PC value seen by instructions is:
//! - ARM mode: PC = fetch_addr + 8
//! - Thumb mode: PC = fetch_addr + 4

use super::registers::Registers;
use crate::bus::Bus;
use serde::{Deserialize, Serialize};

/// Size of an ARM instruction in bytes
pub const ARM_INSTRUCTION_SIZE: u32 = 4;

/// Size of a Thumb instruction in bytes
pub const THUMB_INSTRUCTION_SIZE: u32 = 2;

/// Pipeline state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    /// Address of the instruction currently being fetched
    pub fetch_addr: u32,
    /// Address of the instruction currently being decoded
    pub decode_addr: u32,
    /// Address of the instruction currently being executed
    pub execute_addr: u32,
    /// Instruction currently being decoded (with type info)
    pub decode_instruction: Option<Instruction>,
    /// Instruction currently being executed
    pub execute_instruction: Option<Instruction>,
    /// Whether we're in Thumb mode (affects PC calculation)
    pub thumb_mode: bool,
    /// Next fetch address (for branch prediction/simple branching)
    pub next_fetch_addr: Option<u32>,
}

/// Instruction representation
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Instruction {
    /// Raw instruction bits
    pub opcode: u32,
    /// Whether this is a Thumb instruction
    pub is_thumb: bool,
    /// Pre-decoded instruction info (optional, for performance)
    pub decoded: Option<DecodedInstruction>,
}

impl Instruction {
    /// Create a new ARM instruction
    pub fn arm(opcode: u32) -> Self {
        Self {
            opcode,
            is_thumb: false,
            decoded: None,
        }
    }

    /// Create a new Thumb instruction
    pub fn thumb(opcode: u16) -> Self {
        Self {
            opcode: opcode as u32,
            is_thumb: true,
            decoded: None,
        }
    }

    /// Get the size of this instruction
    pub fn size(&self) -> u32 {
        if self.is_thumb {
            THUMB_INSTRUCTION_SIZE
        } else {
            ARM_INSTRUCTION_SIZE
        }
    }
}

/// Pre-decoded instruction information
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DecodedInstruction {
    /// Instruction category
    pub category: InstructionCategory,
    /// Condition code (for ARM instructions, 0xE for always)
    pub condition: u8,
    /// Destination register
    pub rd: Option<u8>,
    /// First operand register
    pub rn: Option<u8>,
    /// Second operand register
    pub rm: Option<u8>,
    /// Shift amount/type if applicable
    pub shift: Option<ShiftInfo>,
    /// Immediate value if applicable
    pub immediate: Option<u32>,
    /// Branch target if applicable
    pub branch_target: Option<u32>,
    /// Whether this instruction writes back
    pub writes_back: bool,
}

/// Instruction category for quick dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum InstructionCategory {
    // Data processing
    And,
    Eor,
    Sub,
    Rsb,
    Add,
    Adc,
    Sbc,
    Rsc,
    Tst,
    Teq,
    Cmp,
    Cmn,
    Orr,
    Mov,
    Bic,
    Mvn,

    // Multiply
    Mul,
    Mla,
    Umull,
    Umlal,
    Smull,
    Smlal,

    // Load/Store
    Ldr,
    Str,
    Ldrb,
    Strb,
    Ldrh,
    Strh,
    // Atomic swap
    Swp,
    Swpb,
    Ldrsb,
    Ldrsh,
    Ldm,
    Stm,

    // Branch
    B,
    Bl,
    Bx,
    Blx,

    // Status register
    Msr,
    Mrs,

    // Exception
    Swi,
    Undefined,
    Nop,

    // Thumb specific
    ThumbShift,
    ThumbAddSub,
    ThumbCompare,
    ThumbMove,
    ThumbBranch,
    ThumbLoadStore,
    ThumbMultiple,
    ThumbPushPop,
    ThumbMisc,
}

/// Shift information
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct ShiftInfo {
    pub shift_type: ShiftType,
    pub amount: u8,
    pub shift_reg: Option<u8>, // If shift by register
}

/// Shift types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum ShiftType {
    Lsl, // Logical shift left
    Lsr, // Logical shift right
    Asr, // Arithmetic shift right
    Ror, // Rotate right
    Rrx, // Rotate right with extend
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    /// Create a new pipeline starting at address 0
    pub fn new() -> Self {
        Self {
            fetch_addr: 0,
            decode_addr: 0,
            execute_addr: 0,
            decode_instruction: None,
            execute_instruction: None,
            thumb_mode: false,
            next_fetch_addr: None,
        }
    }

    /// Create a new pipeline starting at a specific address
    pub fn with_start_addr(addr: u32, thumb: bool) -> Self {
        Self {
            fetch_addr: addr,
            decode_addr: addr,
            execute_addr: addr,
            decode_instruction: None,
            execute_instruction: None,
            thumb_mode: thumb,
            next_fetch_addr: None,
        }
    }

    /// Get the current PC value as seen by the executing instruction
    /// PC = execute_addr + 8 (ARM) or + 4 (Thumb)
    pub fn pc(&self) -> u32 {
        if self.thumb_mode {
            self.execute_addr.wrapping_add(4)
        } else {
            self.execute_addr.wrapping_add(8)
        }
    }

    /// Get the next sequential PC (for non-branch instructions)
    pub fn next_pc(&self) -> u32 {
        if self.thumb_mode {
            self.execute_addr.wrapping_add(THUMB_INSTRUCTION_SIZE + 4)
        } else {
            self.execute_addr.wrapping_add(ARM_INSTRUCTION_SIZE + 8)
        }
    }

    /// Set Thumb mode
    pub fn set_thumb_mode(&mut self, thumb: bool) {
        self.thumb_mode = thumb;
    }

    /// Check if in Thumb mode
    pub fn is_thumb_mode(&self) -> bool {
        self.thumb_mode
    }

    /// Fetch the next instruction from memory
    pub fn fetch<B: Bus>(&mut self, bus: &B) -> Instruction {
        let addr = self.fetch_addr;

        let instruction = if self.thumb_mode {
            // Thumb instructions are 16-bit aligned
            let opcode = bus.read_opcode_u16(addr & !1);
            Instruction::thumb(opcode)
        } else {
            // ARM instructions are 32-bit aligned
            let opcode = bus.read_opcode_u32(addr & !3);
            Instruction::arm(opcode)
        };

        instruction
    }

    /// Advance the pipeline by one stage
    /// Returns the instruction that completed execution (or None if pipeline was empty)
    pub fn tick<B: Bus>(&mut self, bus: &B, _regs: &Registers) -> Option<Instruction> {
        use crate::cpu::{decode_arm, decode_thumb};

        // Determine next fetch address
        let new_fetch_addr = if let Some(next) = self.next_fetch_addr.take() {
            // Branch was taken
            next
        } else {
            // Sequential execution
            if self.thumb_mode {
                self.fetch_addr.wrapping_add(THUMB_INSTRUCTION_SIZE)
            } else {
                self.fetch_addr.wrapping_add(ARM_INSTRUCTION_SIZE)
            }
        };

        // Move execute -> done (save its address for pc() during execution)
        let executed = self.execute_instruction.take();
        // execute_addr stays as-is: it was set when we moved from decode to execute

        // Move decode -> execute (and decode it if needed)
        if let Some(mut instr) = self.decode_instruction.take() {
            // Decode the instruction before moving to execute stage
            if instr.is_thumb {
                decode_thumb(&mut instr);
            } else {
                decode_arm(&mut instr);
            }
            self.execute_instruction = Some(instr);
            self.execute_addr = self.decode_addr;
        }

        // Fetch -> decode
        self.decode_addr = self.fetch_addr;
        self.decode_instruction = Some(self.fetch(bus));

        // Update fetch address
        self.fetch_addr = new_fetch_addr;

        executed
    }

    /// Force a branch to a new address
    pub fn branch(&mut self, target: u32, link: bool, regs: &mut Registers) {
        if link {
            // Save return address (current PC)
            regs.set_lr(self.pc());
        }

        // Set next fetch address
        self.next_fetch_addr = Some(target);

        // Update Thumb mode if this is BX/BLX
        if self.next_fetch_addr.is_some() {
            // Thumb mode will be updated by the instruction executor
        }
    }

    /// Force a branch with Thumb mode change
    pub fn branch_with_mode(&mut self, target: u32, thumb: bool) {
        self.fetch_addr = target;
        self.thumb_mode = thumb;
        // Flush pipeline - discard instructions that were fetched sequentially
        self.decode_instruction = None;
        self.execute_instruction = None;
        self.next_fetch_addr = None;
    }

    /// Flush the pipeline (on exception or context switch)
    pub fn flush(&mut self) {
        self.decode_instruction = None;
        self.execute_instruction = None;
        self.next_fetch_addr = None;
    }

    /// Set the fetch address directly (for reset/initialization)
    pub fn set_fetch_addr(&mut self, addr: u32) {
        self.fetch_addr = addr;
        self.decode_addr = addr;
        self.execute_addr = addr;
        self.flush();
    }
}

/// Utility functions for instruction decoding
pub mod decode_utils {
    use super::*;

    /// Extract bits [hi:lo] from a value (inclusive)
    #[inline]
    pub fn bits(value: u32, hi: u32, lo: u32) -> u32 {
        let mask = (1 << (hi - lo + 1)) - 1;
        (value >> lo) & mask
    }

    /// Extract a single bit
    #[inline]
    pub fn bit(value: u32, pos: u32) -> bool {
        (value & (1 << pos)) != 0
    }

    /// Sign-extend a value from n bits to 32 bits
    #[inline]
    pub fn sign_extend(value: u32, bits: u32) -> i32 {
        let shift = 32 - bits;
        ((value as i32) << shift) >> shift
    }

    /// Zero-extend a value from n bits to 32 bits
    #[inline]
    pub fn zero_extend(value: u32, _bits: u32) -> u32 {
        value // Already zero-extended in u32
    }

    /// Rotate right a 32-bit value
    #[inline]
    pub fn ror(value: u32, amount: u32) -> u32 {
        value.rotate_right(amount)
    }

    /// Apply shift operation and return (result, carry_out)
    pub fn apply_shift(value: u32, shift: &ShiftInfo, carry_in: bool) -> (u32, bool) {
        match shift.shift_type {
            ShiftType::Lsl => {
                let amount = shift.amount as u32;
                if amount == 0 {
                    (value, carry_in)
                } else if amount < 32 {
                    let result = value << amount;
                    let carry = (value >> (32 - amount)) & 1;
                    (result, carry != 0)
                } else if amount == 32 {
                    (0, (value & 1) != 0)
                } else {
                    (0, false)
                }
            }
            ShiftType::Lsr => {
                let amount = shift.amount as u32;
                if amount == 0 {
                    (0, (value >> 31) != 0)
                } else if amount < 32 {
                    let result = value >> amount;
                    let carry = (value >> (amount - 1)) & 1;
                    (result, carry != 0)
                } else if amount == 32 {
                    (0, (value >> 31) != 0)
                } else {
                    (0, false)
                }
            }
            ShiftType::Asr => {
                let amount = shift.amount as u32;
                if amount == 0 {
                    let sign_bit = (value >> 31) != 0;
                    (if sign_bit { 0xFFFF_FFFF } else { 0 }, sign_bit)
                } else if amount < 32 {
                    let result = ((value as i32) >> amount) as u32;
                    let carry = (value >> (amount - 1)) & 1;
                    (result, carry != 0)
                } else {
                    let sign_bit = (value >> 31) != 0;
                    (if sign_bit { 0xFFFF_FFFF } else { 0 }, sign_bit)
                }
            }
            ShiftType::Ror => {
                let amount = shift.amount as u32;
                if amount == 0 {
                    // RRX - rotate right with extend
                    let result = (if carry_in { 1 << 31 } else { 0 }) | (value >> 1);
                    (result, (value & 1) != 0)
                } else {
                    let rot = amount & 31;
                    if rot == 0 {
                        (value, (value >> 31) != 0)
                    } else {
                        let result = value.rotate_right(rot);
                        let carry = (value >> (rot - 1)) & 1;
                        (result, carry != 0)
                    }
                }
            }
            ShiftType::Rrx => {
                let result = (if carry_in { 1 << 31 } else { 0 }) | (value >> 1);
                (result, (value & 1) != 0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_pc_arm() {
        let pipeline = Pipeline::with_start_addr(0x08000000, false);

        // PC should be fetch_addr + 8 in ARM mode
        assert_eq!(pipeline.pc(), 0x08000008);
    }

    #[test]
    fn test_pipeline_pc_thumb() {
        let pipeline = Pipeline::with_start_addr(0x08000000, true);

        // PC should be fetch_addr + 4 in Thumb mode
        assert_eq!(pipeline.pc(), 0x08000004);
    }

    #[test]
    fn test_bits_extraction() {
        use decode_utils::bits;

        let value = 0x12345678;
        assert_eq!(bits(value, 3, 0), 0x8);
        assert_eq!(bits(value, 7, 4), 0x7);
        assert_eq!(bits(value, 31, 28), 0x1);
    }

    #[test]
    fn test_sign_extend() {
        use decode_utils::sign_extend;

        // Sign extend 8-bit value
        assert_eq!(sign_extend(0x7F, 8), 127);
        assert_eq!(sign_extend(0x80, 8), -128);
        assert_eq!(sign_extend(0xFF, 8), -1);

        // Sign extend 16-bit value
        assert_eq!(sign_extend(0x7FFF, 16), 32767);
        assert_eq!(sign_extend(0x8000, 16), -32768);
    }

    #[test]
    fn test_apply_shift_lsl() {
        use decode_utils::apply_shift;

        let shift = ShiftInfo {
            shift_type: ShiftType::Lsl,
            amount: 4,
            shift_reg: None,
        };

        let (result, carry) = apply_shift(0x12345678, &shift, false);
        assert_eq!(result, 0x23456780);
        assert!(carry); // Bit 28 of 0x12345678 is 1

        let (result, carry) = apply_shift(0x0FFF_FFFF, &shift, false);
        assert_eq!(result, 0xFFF_FFFF0);
        assert!(!carry); // Bit 28 of 0x0FFFFFFF is 0
    }

    #[test]
    fn test_apply_shift_large_amounts() {
        use decode_utils::apply_shift;

        let lsl_32 = ShiftInfo {
            shift_type: ShiftType::Lsl,
            amount: 32,
            shift_reg: None,
        };
        let (result, carry) = apply_shift(0x8000_0001, &lsl_32, false);
        assert_eq!(result, 0);
        assert!(carry);

        let lsr_40 = ShiftInfo {
            shift_type: ShiftType::Lsr,
            amount: 40,
            shift_reg: None,
        };
        let (result, carry) = apply_shift(0x8000_0001, &lsr_40, false);
        assert_eq!(result, 0);
        assert!(!carry);

        let asr_40 = ShiftInfo {
            shift_type: ShiftType::Asr,
            amount: 40,
            shift_reg: None,
        };
        let (result, carry) = apply_shift(0xF000_0000, &asr_40, false);
        assert_eq!(result, 0xFFFF_FFFF);
        assert!(carry);
    }
}

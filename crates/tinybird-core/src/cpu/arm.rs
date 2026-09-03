//! ARM Instruction Set Decoder and Executor
//!
//! This module implements the 32-bit ARM instruction set for the ARM7TDMI core.
//! Uses direct decoding based on instruction bit patterns.

use crate::bios::Bios;
use crate::bus::Bus;
use crate::cpu::pipeline::ARM_INSTRUCTION_SIZE;
use crate::cpu::pipeline::{
    decode_utils::{apply_shift, bit, bits, sign_extend},
    DecodedInstruction, Instruction, InstructionCategory, Pipeline, ShiftInfo, ShiftType,
};
use crate::cpu::registers::{CpuMode, Registers};
use crate::cpu::{
    align_loaded_pc, armv4_load_halfword, armv4_load_signed_halfword, armv4_load_word,
};
use crate::debug::config as debug_config;

/// Decode an ARM instruction directly from the opcode bits
fn decode_arm_instruction(opcode: u32) -> Option<DecodedInstruction> {
    let top3 = bits(opcode, 27, 25);
    let i_flag = bit(opcode, 25);
    let _op = bits(opcode, 24, 21);

    // SWI: bits 27:24 = 0b1111
    if bits(opcode, 27, 24) == 0b1111 {
        return decode_swi(opcode);
    }

    // Branch: bits [27:25] = 0b101 (B/BL)
    if top3 == 0b101 {
        return decode_branch(opcode);
    }

    // Block data transfer (LDM/STM): bits [27:25] = 0b100
    if top3 == 0b100 {
        return decode_ldm_stm(opcode);
    }

    // Single data transfer (LDR/STR): bits [27:26] = 0b01
    if bits(opcode, 27, 26) == 0b01 {
        return decode_ldr_str(opcode);
    }

    // Bits [27:26] = 0b00: data processing, multiply, halfword transfer, MSR/MRS, BX
    if bits(opcode, 27, 26) == 0b00 {
        // SWP / SWPB, which shares the 1001 pattern with multiply but sits in
        // bits[27:23] = 00010. Without this it falls through to data
        // processing and is decoded as something else entirely.
        if !i_flag
            && bits(opcode, 27, 23) == 0b00010
            && !bit(opcode, 21)
            && !bit(opcode, 20)
            && bits(opcode, 11, 4) == 0b0000_1001
        {
            return decode_swap(opcode);
        }

        // Check for multiply: bits[7:4] = 1001 and bits[27:24] = 0000 (covers MUL/MLA and long)
        if bits(opcode, 7, 4) == 0b1001 && bits(opcode, 27, 24) == 0 && !i_flag {
            return decode_multiply(opcode);
        }

        // Check for halfword/signed transfer: bits[7:4] = 1xx1 and bit[25]=0
        // Pattern: bits[7]=1, bits[4]=1, bits[6:5]!=00
        if !i_flag && bit(opcode, 7) && bit(opcode, 4) && bits(opcode, 6, 5) != 0 {
            return decode_halfword_transfer(opcode);
        }

        // MRS (register transfer PSR -> GPR), register form:
        // bits[27:23]=00010, bit21=0, bit20=0, bits[19:16]=1111, bits[11:0]=0
        if !i_flag
            && bits(opcode, 27, 23) == 0b00010
            && !bit(opcode, 21)
            && !bit(opcode, 20)
            && bits(opcode, 19, 16) == 0b1111
            && bits(opcode, 11, 0) == 0
        {
            return decode_mrs(opcode);
        }

        // MSR (register/immediate transfer GPR/imm -> PSR)
        // Register form: bits[27:23]=00010, bit21=1, bit20=0, bits[15:12]=1111, bits[11:4]=0
        if !i_flag
            && bits(opcode, 27, 23) == 0b00010
            && bit(opcode, 21)
            && !bit(opcode, 20)
            && bits(opcode, 15, 12) == 0b1111
            && bits(opcode, 11, 4) == 0
        {
            return decode_msr(opcode);
        }

        // MSR with immediate:
        // bits[27:23]=00110, bit21=1, bit20=0, bits[15:12]=1111
        if i_flag
            && bits(opcode, 27, 23) == 0b00110
            && bit(opcode, 21)
            && !bit(opcode, 20)
            && bits(opcode, 15, 12) == 0b1111
        {
            return decode_msr(opcode);
        }

        // Default: data processing
        return decode_data_processing(opcode);
    }

    // Undefined
    Some(DecodedInstruction {
        category: InstructionCategory::Undefined,
        condition: bits(opcode, 31, 28) as u8,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: None,
        branch_target: None,
        writes_back: false,
    })
}

fn decode_data_processing(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let op = bits(opcode, 24, 21) as u8;
    let i_flag = bit(opcode, 25);
    let s_flag = bit(opcode, 20);

    let rd = bits(opcode, 15, 12) as u8;
    let rn = bits(opcode, 19, 16) as u8;

    // Check for BX instruction:
    // cond=xxxx, bits 27:4=000100101111111111110001, bits 3:0=Rm
    // Bits 27:4 = 0x12FFF1
    if bits(opcode, 27, 4) == 0x12FFF1 {
        // BX instruction
        let rm = bits(opcode, 3, 0) as u8;
        return Some(DecodedInstruction {
            category: InstructionCategory::Bx,
            condition: cond,
            rd: None,
            rn: Some(rm),
            rm: None,
            shift: None,
            immediate: None,
            branch_target: None,
            writes_back: false,
        });
    }

    let category = match op {
        0b0000 => InstructionCategory::And,
        0b0001 => InstructionCategory::Eor,
        0b0010 => InstructionCategory::Sub,
        0b0011 => InstructionCategory::Rsb,
        0b0100 => InstructionCategory::Add,
        0b0101 => InstructionCategory::Adc,
        0b0110 => InstructionCategory::Sbc,
        0b0111 => InstructionCategory::Rsc,
        0b1000 => InstructionCategory::Tst,
        0b1001 => InstructionCategory::Teq,
        0b1010 => InstructionCategory::Cmp,
        0b1011 => InstructionCategory::Cmn,
        0b1100 => InstructionCategory::Orr,
        0b1101 => InstructionCategory::Mov,
        0b1110 => InstructionCategory::Bic,
        0b1111 => InstructionCategory::Mvn,
        _ => unreachable!(),
    };

    let (rm, shift, immediate) = if i_flag {
        let imm8 = bits(opcode, 7, 0);
        let rotate = bits(opcode, 11, 8) * 2;
        let imm32 = imm8.rotate_right(rotate);
        (None, None, Some(imm32))
    } else {
        let rm_val = bits(opcode, 3, 0) as u8;
        let shift_type_bits = bits(opcode, 6, 5);
        let shift_imm = bits(opcode, 11, 7);

        let shift_type = match shift_type_bits {
            0b00 => ShiftType::Lsl,
            0b01 => ShiftType::Lsr,
            0b10 => ShiftType::Asr,
            0b11 => {
                // RRX is what an *immediate* rotate of zero encodes. Bit 4
                // selects a register-specified amount, which is always a plain
                // rotate — reading it the other way round turned every
                // `ROR Rd, Rs` into a rotate-right-through-carry by one.
                if !bit(opcode, 4) && shift_imm == 0 {
                    ShiftType::Rrx
                } else {
                    ShiftType::Ror
                }
            }
            _ => unreachable!(),
        };

        let shift_info = if bit(opcode, 4) {
            // Register-shifted operand: Rs = bits[11:8]
            let rs = bits(opcode, 11, 8) as u8;
            Some(ShiftInfo {
                shift_type,
                amount: 0,
                shift_reg: Some(rs),
            })
        } else {
            // Immediate-shifted operand: shift amount = bits[11:7]
            Some(ShiftInfo {
                shift_type,
                amount: shift_imm as u8,
                shift_reg: None,
            })
        };

        (Some(rm_val), shift_info, None)
    };

    // A compare has no destination, with one exception: `Rd = 15` with the S
    // bit is the old ARMv2 "P" form, which returns from an exception instead of
    // comparing. That has to survive decoding, so the field is kept when it
    // names PC and dropped otherwise.
    let rd_opt = if matches!(
        category,
        InstructionCategory::Tst
            | InstructionCategory::Teq
            | InstructionCategory::Cmp
            | InstructionCategory::Cmn
    ) {
        (rd == 15).then_some(rd)
    } else {
        Some(rd)
    };

    Some(DecodedInstruction {
        category,
        condition: cond,
        rd: rd_opt,
        rn: if matches!(
            category,
            InstructionCategory::Mov | InstructionCategory::Mvn
        ) {
            None
        } else {
            Some(rn)
        },
        rm,
        shift,
        immediate,
        branch_target: None,
        writes_back: s_flag,
    })
}

fn decode_ldr_str(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let is_load = bit(opcode, 20);
    let is_byte = bit(opcode, 22);
    let is_reg_offset = bit(opcode, 25); // bit25: 0=immediate, 1=register
    let is_up = bit(opcode, 23); // U bit: 1=add offset, 0=subtract offset
    let is_pre = bit(opcode, 24); // P bit: 1=pre-index, 0=post-index
    let writeback = bit(opcode, 21); // W bit: writeback to base

    let rd = bits(opcode, 15, 12) as u8;
    let rn = bits(opcode, 19, 16) as u8;

    let category = if is_byte {
        if is_load {
            InstructionCategory::Ldrb
        } else {
            InstructionCategory::Strb
        }
    } else {
        if is_load {
            InstructionCategory::Ldr
        } else {
            InstructionCategory::Str
        }
    };

    let (rm, shift, immediate) = if !is_reg_offset {
        // Immediate offset
        let mut imm12 = bits(opcode, 11, 0);
        if !is_up {
            imm12 = (-(imm12 as i32)) as u32;
        }
        (None, None, Some(imm12))
    } else {
        // Register offset with shift
        let rm = bits(opcode, 3, 0) as u8;
        let shift_type_bits = bits(opcode, 6, 5);
        let shift_imm = bits(opcode, 11, 7) as u8;
        let shift_type = match shift_type_bits {
            0b00 => ShiftType::Lsl,
            0b01 => ShiftType::Lsr,
            0b10 => ShiftType::Asr,
            0b11 => {
                if shift_imm == 0 {
                    ShiftType::Rrx
                } else {
                    ShiftType::Ror
                }
            }
            _ => ShiftType::Lsl,
        };
        (
            Some(rm),
            Some(ShiftInfo {
                shift_type,
                amount: shift_imm,
                shift_reg: None,
            }),
            None,
        )
    };

    // Encode pre/post/up/writeback flags in branch_target
    // bit 0 = pre, bit 1 = up, bit 2 = writeback
    let flags = (is_pre as u32) | ((is_up as u32) << 1) | ((writeback as u32) << 2);

    Some(DecodedInstruction {
        category,
        condition: cond,
        rd: Some(rd),
        rn: Some(rn),
        rm,
        shift,
        immediate,
        branch_target: Some(flags),
        writes_back: writeback || !is_pre, // post-index always writes back
    })
}

fn decode_ldm_stm(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let is_load = bit(opcode, 20);
    let writeback = bit(opcode, 21);
    let s_bit = bit(opcode, 22); // S-bit: SPSR->CPSR on LDM^+PC, or user bank
    let is_up = bit(opcode, 23);
    let is_pre = bit(opcode, 24);
    let rn = bits(opcode, 19, 16) as u8;
    let rlist = bits(opcode, 15, 0);

    let category = if is_load {
        InstructionCategory::Ldm
    } else {
        InstructionCategory::Stm
    };

    // Encode P/U/S flags in branch_target: bit0=pre, bit1=up, bit2=s
    let flags = (is_pre as u32) | ((is_up as u32) << 1) | ((s_bit as u32) << 2);

    Some(DecodedInstruction {
        category,
        condition: cond,
        rd: Some(rn), // Base register in rd (consistent with Thumb)
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(rlist),
        branch_target: Some(flags),
        writes_back: writeback,
    })
}

/// `SWP` and `SWPB`: read a word or byte and write one back in its place.
fn decode_swap(opcode: u32) -> Option<DecodedInstruction> {
    let is_byte = bit(opcode, 22);
    Some(DecodedInstruction {
        category: if is_byte {
            InstructionCategory::Swpb
        } else {
            InstructionCategory::Swp
        },
        condition: bits(opcode, 31, 28) as u8,
        rd: Some(bits(opcode, 15, 12) as u8),
        rn: Some(bits(opcode, 19, 16) as u8),
        rm: Some(bits(opcode, 3, 0) as u8),
        immediate: None,
        shift: None,
        branch_target: None,
        writes_back: false,
    })
}

fn decode_halfword_transfer(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let is_pre = bit(opcode, 24);
    let is_up = bit(opcode, 23);
    let is_imm = bit(opcode, 22);
    let writeback = bit(opcode, 21);
    let is_load = bit(opcode, 20);
    let rn = bits(opcode, 19, 16) as u8;
    let rd = bits(opcode, 15, 12) as u8;
    let sh = bits(opcode, 6, 5);

    let category = match (is_load, sh) {
        (false, 0b01) => InstructionCategory::Strh,
        (true, 0b01) => InstructionCategory::Ldrh,
        (true, 0b10) => InstructionCategory::Ldrsb,
        (true, 0b11) => InstructionCategory::Ldrsh,
        _ => InstructionCategory::Undefined,
    };

    let (rm, immediate) = if is_imm {
        let hi = bits(opcode, 11, 8);
        let lo = bits(opcode, 3, 0);
        let mut offset = (hi << 4) | lo;
        if !is_up {
            offset = (-(offset as i32)) as u32;
        }
        (None, Some(offset))
    } else {
        let rm = bits(opcode, 3, 0) as u8;
        (Some(rm), None)
    };

    let flags = (is_pre as u32) | ((is_up as u32) << 1) | ((writeback as u32) << 2);

    Some(DecodedInstruction {
        category,
        condition: cond,
        rd: Some(rd),
        rn: Some(rn),
        rm,
        shift: None,
        immediate,
        branch_target: Some(flags),
        writes_back: writeback || !is_pre,
    })
}

fn decode_msr(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let use_spsr = bit(opcode, 22);
    let i_flag = bit(opcode, 25);
    let field_mask = bits(opcode, 19, 16); // which fields to write

    let (rm, immediate) = if i_flag {
        let imm8 = bits(opcode, 7, 0);
        let rotate = bits(opcode, 11, 8) * 2;
        (None, Some(imm8.rotate_right(rotate)))
    } else {
        let rm = bits(opcode, 3, 0) as u8;
        (Some(rm), None)
    };

    // Encode: branch_target = field_mask | (spsr << 4)
    let flags = field_mask | ((use_spsr as u32) << 4);

    Some(DecodedInstruction {
        category: InstructionCategory::Msr,
        condition: cond,
        rd: None,
        rn: None,
        rm,
        shift: None,
        immediate,
        branch_target: Some(flags),
        writes_back: false,
    })
}

fn decode_mrs(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let use_spsr = bit(opcode, 22);
    let rd = bits(opcode, 15, 12) as u8;

    Some(DecodedInstruction {
        category: InstructionCategory::Mrs,
        condition: cond,
        rd: Some(rd),
        rn: None,
        rm: None,
        shift: None,
        immediate: None,
        branch_target: Some(use_spsr as u32),
        writes_back: false,
    })
}

fn decode_branch(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let is_link = bit(opcode, 24);
    let offset = sign_extend(bits(opcode, 23, 0), 24) as u32;

    Some(DecodedInstruction {
        category: if is_link {
            InstructionCategory::Bl
        } else {
            InstructionCategory::B
        },
        condition: cond,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(offset << 2),
        branch_target: None,
        writes_back: is_link,
    })
}

fn decode_multiply(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    // For short multiply: rd=bits[19:16], rn=bits[15:12], rs=bits[11:8], rm=bits[3:0]
    // For long multiply: rdhi=bits[19:16], rdlo=bits[15:12], rs=bits[11:8], rm=bits[3:0]
    let rd = bits(opcode, 19, 16) as u8;
    let rn = bits(opcode, 15, 12) as u8;
    let rs = bits(opcode, 11, 8) as u8;
    let rm = bits(opcode, 3, 0) as u8;
    let is_long = bit(opcode, 23);
    let is_signed = bit(opcode, 22);
    let accumulate = bit(opcode, 21);
    let set_flags = bit(opcode, 20);

    let category = if is_long {
        match (is_signed, accumulate) {
            (false, false) => InstructionCategory::Umull,
            (false, true) => InstructionCategory::Umlal,
            (true, false) => InstructionCategory::Smull,
            (true, true) => InstructionCategory::Smlal,
        }
    } else if accumulate {
        InstructionCategory::Mla
    } else {
        InstructionCategory::Mul
    };

    Some(DecodedInstruction {
        category,
        condition: cond,
        rd: Some(rd),
        rn: Some(rn),
        rm: Some(rm),
        shift: None,
        immediate: Some(rs as u32), // Rs register number
        branch_target: None,
        writes_back: set_flags,
    })
}

fn decode_swi(opcode: u32) -> Option<DecodedInstruction> {
    let cond = bits(opcode, 31, 28) as u8;
    let swi_num = bits(opcode, 23, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::Swi,
        condition: cond,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(swi_num),
        branch_target: None,
        writes_back: false,
    })
}

/// Decode an ARM instruction
#[allow(missing_docs)]
pub fn decode_arm(instruction: &mut Instruction) {
    let opcode = instruction.opcode;
    instruction.decoded = decode_arm_instruction(opcode);
}

/// Execute an ARM instruction
#[allow(missing_docs)]
pub fn execute_arm<B: Bus>(
    instruction: &Instruction,
    bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
) {
    let decoded = match &instruction.decoded {
        Some(d) => d,
        None => return,
    };

    if !regs.get_cond(decoded.condition) {
        return;
    }

    let opcode = instruction.opcode;

    match decoded.category {
        InstructionCategory::And => {
            exec_data_proc(bus, regs, pipeline, decoded, opcode, |a, b| a & b)
        }
        InstructionCategory::Eor => {
            exec_data_proc(bus, regs, pipeline, decoded, opcode, |a, b| a ^ b)
        }
        InstructionCategory::Sub => {
            exec_data_proc_with_flags(bus, regs, pipeline, decoded, sub_with_flags)
        }
        InstructionCategory::Rsb => {
            exec_data_proc_with_flags(bus, regs, pipeline, decoded, |a, b| sub_with_flags(b, a))
        }
        InstructionCategory::Add => {
            exec_data_proc_with_flags(bus, regs, pipeline, decoded, add_with_flags)
        }
        InstructionCategory::Adc => exec_adc(bus, regs, pipeline, decoded),
        InstructionCategory::Sbc => exec_sbc(bus, regs, pipeline, decoded),
        InstructionCategory::Rsc => exec_rsc(bus, regs, pipeline, decoded),
        InstructionCategory::Tst => exec_test(bus, regs, pipeline, decoded, opcode, |a, b| a & b),
        InstructionCategory::Teq => exec_test(bus, regs, pipeline, decoded, opcode, |a, b| a ^ b),
        InstructionCategory::Cmp => exec_compare(bus, regs, pipeline, decoded, sub_with_flags),
        InstructionCategory::Cmn => exec_compare(bus, regs, pipeline, decoded, add_with_flags),
        InstructionCategory::Orr => {
            exec_data_proc(bus, regs, pipeline, decoded, opcode, |a, b| a | b)
        }
        InstructionCategory::Mov => exec_data_proc(bus, regs, pipeline, decoded, opcode, |_a, b| b),
        InstructionCategory::Bic => {
            exec_data_proc(bus, regs, pipeline, decoded, opcode, |a, b| a & !b)
        }
        InstructionCategory::Mvn => {
            exec_data_proc(bus, regs, pipeline, decoded, opcode, |_a, b| !b)
        }
        InstructionCategory::Ldr => exec_ldr(bus, regs, pipeline, decoded, false),
        InstructionCategory::Str => exec_str(bus, regs, decoded, false),
        InstructionCategory::Ldrb => exec_ldr(bus, regs, pipeline, decoded, true),
        InstructionCategory::Strb => exec_str(bus, regs, decoded, true),
        InstructionCategory::Swp => exec_swap(bus, regs, decoded, false),
        InstructionCategory::Swpb => exec_swap(bus, regs, decoded, true),
        InstructionCategory::Ldrh => exec_ldr_half(bus, regs, pipeline, decoded, false, false),
        InstructionCategory::Strh => exec_str_half(bus, regs, decoded),
        InstructionCategory::Ldrsb => exec_ldr_half(bus, regs, pipeline, decoded, true, true),
        InstructionCategory::Ldrsh => exec_ldr_half(bus, regs, pipeline, decoded, false, true),
        InstructionCategory::B => exec_branch(regs, pipeline, decoded, false),
        InstructionCategory::Bl => exec_branch(regs, pipeline, decoded, true),
        InstructionCategory::Bx => exec_bx(regs, pipeline, decoded),
        InstructionCategory::Blx => exec_blx(regs, pipeline, decoded),
        InstructionCategory::Swi => exec_swi(bus, regs, pipeline, decoded.immediate.unwrap_or(0)),
        InstructionCategory::Mul => exec_mul(regs, decoded),
        InstructionCategory::Mla => exec_mla(regs, decoded),
        InstructionCategory::Umull => exec_umull(regs, decoded),
        InstructionCategory::Umlal => exec_umlal(regs, decoded),
        InstructionCategory::Smull => exec_smull(regs, decoded),
        InstructionCategory::Smlal => exec_smlal(regs, decoded),
        InstructionCategory::Ldm => exec_ldm(bus, regs, pipeline, decoded),
        InstructionCategory::Stm => exec_stm(bus, regs, decoded),
        InstructionCategory::Msr => exec_msr(regs, pipeline, decoded),
        InstructionCategory::Mrs => exec_mrs(regs, decoded),
        _ => {}
    }
}

#[inline]
fn add_with_flags(a: u32, b: u32) -> (u32, bool, bool) {
    let (result, carry) = a.overflowing_add(b);
    let overflow = (((a ^ result) & (b ^ result)) & 0x8000_0000) != 0;
    (result, carry, overflow)
}

#[inline]
fn sub_with_flags(a: u32, b: u32) -> (u32, bool, bool) {
    let (result, borrow) = a.overflowing_sub(b);
    let carry = !borrow;
    let overflow = (((a ^ b) & (a ^ result)) & 0x8000_0000) != 0;
    (result, carry, overflow)
}

fn exec_data_proc<B: Bus, F>(
    _bus: &mut B,
    regs: &mut Registers,
    _pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    opcode: u32,
    op: F,
) where
    F: Fn(u32, u32) -> u32,
{
    let register_shift = uses_register_shift(decoded);
    let rn_val = decoded
        .rn
        .map(|r| read_operand_reg(regs, r, register_shift))
        .unwrap_or(0);
    let (op2_val, shifter_carry) = get_operand2_and_carry(regs, decoded, opcode);
    let result = op(rn_val, op2_val);

    if let Some(rd) = decoded.rd {
        if rd != 15 {
            regs.set_reg(rd as usize, result);
            if decoded.writes_back {
                // Logical data-processing ops write C from shifter carry.
                regs.set_flags(
                    (result as i32) < 0,
                    result == 0,
                    shifter_carry,
                    regs.flag_v(),
                );
            }
        } else {
            if decoded.writes_back {
                // S=1 with Rd=PC: exception return — restore CPSR from SPSR
                regs.return_from_exception();
            }
            let thumb = regs.is_thumb_mode();
            _pipeline.branch_with_mode(align_loaded_pc(result, thumb), thumb);
        }
    } else if decoded.writes_back {
        regs.set_zn_flags(result);
    }
}

fn exec_data_proc_with_flags<B: Bus, F>(
    _bus: &mut B,
    regs: &mut Registers,
    _pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    op: F,
) where
    F: Fn(u32, u32) -> (u32, bool, bool),
{
    let rn_val = decoded
        .rn
        .map(|r| read_operand_reg(regs, r, uses_register_shift(decoded)))
        .unwrap_or(0);
    let op2_val = get_operand2(regs, decoded);
    let (result, carry, overflow) = op(rn_val, op2_val);
    let n = (result as i32) < 0;
    let z = result == 0;

    if let Some(rd) = decoded.rd {
        if rd != 15 {
            regs.set_reg(rd as usize, result);
            if decoded.writes_back {
                regs.set_flags(n, z, carry, overflow);
            }
        } else {
            if decoded.writes_back {
                // S=1 with Rd=PC: exception return (SUBS PC, LR, #4 etc.)
                regs.return_from_exception();
            }
            let thumb = regs.is_thumb_mode();
            _pipeline.branch_with_mode(align_loaded_pc(result, thumb), thumb);
        }
    } else if decoded.writes_back {
        regs.set_flags(n, z, carry, overflow);
    }
}

fn exec_adc<B: Bus>(
    _bus: &mut B,
    regs: &mut Registers,
    _pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
) {
    let rn_val = decoded
        .rn
        .map(|r| read_operand_reg(regs, r, uses_register_shift(decoded)))
        .unwrap_or(0);
    let op2_val = get_operand2(regs, decoded);
    let carry_in = if regs.flag_c() { 1 } else { 0 };

    let (result, carry1) = rn_val.overflowing_add(op2_val);
    let (result, carry2) = result.overflowing_add(carry_in);
    let overflow = ((rn_val as i32) >= 0 && (op2_val as i32) >= 0 && (result as i32) < 0)
        || ((rn_val as i32) < 0 && (op2_val as i32) < 0 && (result as i32) >= 0);

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_flags((result as i32) < 0, result == 0, carry1 || carry2, overflow);
    }
}

fn exec_sbc<B: Bus>(
    _bus: &mut B,
    regs: &mut Registers,
    _pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
) {
    let rn_val = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let op2_val = get_operand2(regs, decoded);
    let carry_in = if regs.flag_c() { 0 } else { 1 };

    let (result, borrow1) = rn_val.overflowing_sub(op2_val);
    let (result, borrow2) = result.overflowing_sub(carry_in);
    let overflow = (((rn_val as i32) < 0) && ((op2_val as i32) >= 0) && ((result as i32) >= 0))
        || (((rn_val as i32) >= 0) && ((op2_val as i32) < 0) && ((result as i32) < 0));

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_flags(
            (result as i32) < 0,
            result == 0,
            !(borrow1 || borrow2),
            overflow,
        );
    }
}

fn exec_rsc<B: Bus>(
    _bus: &mut B,
    regs: &mut Registers,
    _pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
) {
    let rn_val = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let op2_val = get_operand2(regs, decoded);
    let carry_in = if regs.flag_c() { 0 } else { 1 };

    let (result, borrow1) = op2_val.overflowing_sub(rn_val);
    let (result, borrow2) = result.overflowing_sub(carry_in);

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_flags(
            (result as i32) < 0,
            result == 0,
            !(borrow1 || borrow2),
            false,
        );
    }
}

/// `TST` and `TEQ`, which share `CMP`'s `Rd = 15` exception-return form.
fn exec_test<B: Bus, F>(
    _bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    opcode: u32,
    op: F,
) where
    F: Fn(u32, u32) -> u32,
{
    if decoded.rd == Some(15) {
        let was_thumb = pipeline.is_thumb_mode();
        regs.return_from_exception();
        if regs.is_thumb_mode() != was_thumb {
            let next = pipeline.pc().wrapping_sub(ARM_INSTRUCTION_SIZE);
            pipeline.branch_with_mode(next, regs.is_thumb_mode());
        }
        return;
    }

    let rn_val = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let (op2_val, shifter_carry) = get_operand2_and_carry(regs, decoded, opcode);
    let result = op(rn_val, op2_val);
    regs.set_flags(
        (result as i32) < 0,
        result == 0,
        shifter_carry,
        regs.flag_v(),
    );
}

/// `CMP`, `CMN`, `TST`, `TEQ`.
///
/// With `Rd = 15` these are not comparisons at all but the ARMv2 exception
/// return: CPSR is restored from SPSR, which can change processor mode and so
/// swap out the banked registers. The flags come from SPSR rather than from the
/// operation, so nothing is compared.
fn exec_compare<B: Bus, F>(
    _bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    op: F,
) where
    F: Fn(u32, u32) -> (u32, bool, bool),
{
    if decoded.rd == Some(15) {
        let was_thumb = pipeline.is_thumb_mode();
        regs.return_from_exception();
        if regs.is_thumb_mode() != was_thumb {
            // The restored CPSR can change instruction set, and the pipeline
            // fetches in its own copy of that state.
            let next = pipeline.pc().wrapping_sub(ARM_INSTRUCTION_SIZE);
            pipeline.branch_with_mode(next, regs.is_thumb_mode());
        }
        return;
    }

    let rn_val = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let op2_val = get_operand2(regs, decoded);
    let (result, carry, overflow) = op(rn_val, op2_val);
    if debug_config().cmp_debug {
        eprintln!(
            "[cmp] rn={:08x} op2={:08x} -> res={:08x} c={} v={} cpsr_before={:08x}",
            rn_val,
            op2_val,
            result,
            carry,
            overflow,
            regs.cpsr()
        );
    }
    regs.set_flags((result as i32) < 0, result == 0, carry, overflow);
}

fn arm_ldr_str_addr(regs: &mut Registers, decoded: &DecodedInstruction) -> (u32, Option<u32>) {
    let flags = decoded.branch_target.unwrap_or(0b011); // default: pre, up, no writeback
    let is_pre = (flags & 1) != 0;
    let is_up = (flags & 2) != 0;
    let _writeback = (flags & 4) != 0;

    let base = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);

    // Get offset: either immediate (already sign-adjusted for !up) or register+shift
    let offset = if decoded.immediate.is_some() {
        decoded.immediate.unwrap()
    } else {
        let op2 = get_operand2(regs, decoded);
        if is_up {
            op2
        } else {
            (-(op2 as i32)) as u32
        }
    };

    let addr = if is_pre {
        base.wrapping_add(offset)
    } else {
        base
    };

    // Writeback: post-index always writes back, pre-index writes back if W bit set
    let wb_addr = if decoded.writes_back {
        Some(if is_pre {
            addr
        } else {
            base.wrapping_add(offset)
        })
    } else {
        None
    };

    (addr, wb_addr)
}

fn exec_ldr<B: Bus>(
    bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    is_byte: bool,
) {
    let (addr, wb_addr) = arm_ldr_str_addr(regs, decoded);

    let value = if is_byte {
        bus.read_u8(addr) as u32
    } else {
        armv4_load_word(bus, addr)
    };
    // Write the base back first, so that when the base and the destination are
    // the same register the loaded value is what survives. On ARM7TDMI the load
    // wins; doing it the other way round leaves the address in the register
    // instead of the data that was just fetched.
    if let (Some(rn), Some(wb)) = (decoded.rn, wb_addr) {
        regs.set_reg(rn as usize, wb);
    }
    if let Some(rd) = decoded.rd {
        if rd == 15 {
            pipeline.branch_with_mode(value & !3, false);
        } else {
            regs.set_reg(rd as usize, value);
        }
    }
}

fn exec_str<B: Bus>(
    bus: &mut B,
    regs: &mut Registers,
    decoded: &DecodedInstruction,
    is_byte: bool,
) {
    let (addr, wb_addr) = arm_ldr_str_addr(regs, decoded);

    let value = decoded
        .rd
        .map(|r| {
            let value = regs.get_reg(r as usize);
            // Storing PC stores the instruction's address plus twelve, four
            // beyond what reading PC gives, because the store happens a cycle
            // later than a data-processing read would.
            if r == 15 {
                value.wrapping_add(4)
            } else {
                value
            }
        })
        .unwrap_or(0);
    if is_byte {
        bus.write_u8(addr, value as u8);
    } else {
        bus.write_u32(addr, value);
    }
    // Writeback to base register
    if let (Some(rn), Some(wb)) = (decoded.rn, wb_addr) {
        regs.set_reg(rn as usize, wb);
    }
}

fn exec_branch(
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    link: bool,
) {
    let pc = pipeline.pc();
    let offset = decoded.immediate.unwrap_or(0);
    let target = pc.wrapping_add(offset);
    if link {
        // ARM BL: LR = return address = instruction_addr + 4 = PC - 4
        regs.set_lr(pc.wrapping_sub(4));
    }
    pipeline.branch_with_mode(target, false);
}

fn exec_swi<B: Bus>(bus: &mut B, regs: &mut Registers, pipeline: &mut Pipeline, swi_num: u32) {
    // ARM SWI comments are seen both as 0xXX0000 and 0x0000XX in the wild.
    // Prefer bits 16-23 when present, otherwise fall back to low byte.
    let high = ((swi_num >> 16) & 0xFF) as u8;
    let low = (swi_num & 0xFF) as u8;
    let swi_comment = if high != 0 { high } else { low };
    if debug_config().real_swi_debug {
        eprintln!(
            "[SWI-REAL] {:02x} r0={:08x} r1={:08x} r2={:08x} r3={:08x} sp={:08x} lr={:08x}",
            swi_comment,
            regs.get_reg(0),
            regs.get_reg(1),
            regs.get_reg(2),
            regs.get_reg(3),
            regs.get_reg(13),
            regs.get_reg(14),
        );
    }

    // If a real BIOS is present, dispatch SWI via exception vector.
    // The built-in HLE stub starts with BX LR at 0x00000000.
    let has_real_bios = bus.has_real_bios();
    if has_real_bios && !Bios::should_hle_with_real_bios(swi_comment) {
        let lr_offset = if regs.is_thumb_mode() { 2 } else { 4 };
        regs.enter_exception(CpuMode::Supervisor, lr_offset);
        pipeline.branch_with_mode(0x0000_0008, false);
        pipeline.flush();
        return;
    }
    Bios::handle_swi(swi_comment, regs, bus);
}

fn exec_mul(regs: &mut Registers, decoded: &DecodedInstruction) {
    // MUL: Rd = Rm * Rs
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0);
    let result = rm_val.wrapping_mul(rs_val);
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_zn_flags(result);
    }
}

fn exec_mla(regs: &mut Registers, decoded: &DecodedInstruction) {
    // MLA: Rd = Rm * Rs + Rn
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0);
    let rn_val = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let result = rm_val.wrapping_mul(rs_val).wrapping_add(rn_val);
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_zn_flags(result);
    }
}

fn exec_umull(regs: &mut Registers, decoded: &DecodedInstruction) {
    // UMULL: RdHi:RdLo = Rm * Rs (unsigned 64-bit)
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0) as u64;
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0) as u64;
    let result = rm_val * rs_val;
    let rdhi = decoded.rd.unwrap_or(0) as usize;
    let rdlo = decoded.rn.unwrap_or(0) as usize;
    regs.set_reg(rdhi, (result >> 32) as u32);
    regs.set_reg(rdlo, result as u32);
    if decoded.writes_back {
        regs.set_zn_flags64(result);
    }
}

fn exec_umlal(regs: &mut Registers, decoded: &DecodedInstruction) {
    // UMLAL: RdHi:RdLo += Rm * Rs (unsigned 64-bit)
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0) as u64;
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0) as u64;
    let rdhi = decoded.rd.unwrap_or(0) as usize;
    let rdlo = decoded.rn.unwrap_or(0) as usize;
    let acc = ((regs.get_reg(rdhi) as u64) << 32) | (regs.get_reg(rdlo) as u64);
    let result = (rm_val * rs_val).wrapping_add(acc);
    regs.set_reg(rdhi, (result >> 32) as u32);
    regs.set_reg(rdlo, result as u32);
    if decoded.writes_back {
        regs.set_zn_flags64(result);
    }
}

fn exec_smull(regs: &mut Registers, decoded: &DecodedInstruction) {
    // SMULL: RdHi:RdLo = Rm * Rs (signed 64-bit)
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0) as i32 as i64;
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0) as i32 as i64;
    let result = (rm_val * rs_val) as u64;
    let rdhi = decoded.rd.unwrap_or(0) as usize;
    let rdlo = decoded.rn.unwrap_or(0) as usize;
    regs.set_reg(rdhi, (result >> 32) as u32);
    regs.set_reg(rdlo, result as u32);
    if decoded.writes_back {
        regs.set_zn_flags64(result);
    }
}

fn exec_smlal(regs: &mut Registers, decoded: &DecodedInstruction) {
    // SMLAL: RdHi:RdLo += Rm * Rs (signed 64-bit)
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0) as i32 as i64;
    let rs_val = decoded
        .immediate
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(0) as i32 as i64;
    let rdhi = decoded.rd.unwrap_or(0) as usize;
    let rdlo = decoded.rn.unwrap_or(0) as usize;
    let acc = (((regs.get_reg(rdhi) as u64) << 32) | (regs.get_reg(rdlo) as u64)) as i64;
    let result = ((rm_val * rs_val).wrapping_add(acc)) as u64;
    regs.set_reg(rdhi, (result >> 32) as u32);
    regs.set_reg(rdlo, result as u32);
    if decoded.writes_back {
        regs.set_zn_flags64(result);
    }
}

fn exec_bx(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let rm = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let thumb = (rm & 1) != 0;
    pipeline.branch_with_mode(rm & !1, thumb);
    regs.set_thumb_mode(thumb);
}

fn exec_blx(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let rm = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let pc = pipeline.pc();
    regs.set_lr(pc);
    let thumb = (rm & 1) != 0;
    pipeline.branch_with_mode(rm & !1, thumb);
    regs.set_thumb_mode(thumb);
}

fn exec_ldm<B: Bus>(
    bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
) {
    let rn = decoded.rd.unwrap_or(0); // Base register is in rd for our decode
    let base = regs.get_reg(rn as usize);
    let rlist = decoded.immediate.unwrap_or(0);
    let flags = decoded.branch_target.unwrap_or(0b10); // default: post-increment (IA)
    let is_pre = (flags & 1) != 0;
    let is_up = (flags & 2) != 0;
    let s_bit = (flags & 4) != 0;
    // Count registers
    let reg_count = (0..16).filter(|i| (rlist & (1 << i)) != 0).count() as u32;

    let mut addr = if is_up {
        if is_pre {
            base.wrapping_add(4)
        } else {
            base
        }
    } else {
        // Decrement: start from base - count*4
        let start = base.wrapping_sub(reg_count * 4);
        if is_pre {
            start
        } else {
            start.wrapping_add(4)
        }
    };

    for i in 0..16u32 {
        if (rlist & (1 << i)) != 0 {
            let value = bus.read_u32(addr);
            if i == 15 {
                if s_bit {
                    // LDM with S-bit and R15: restore SPSR to CPSR (exception return)
                    regs.return_from_exception();
                    let thumb = regs.is_thumb_mode();
                    pipeline.branch_with_mode(align_loaded_pc(value, thumb), thumb);
                } else {
                    // Regular ARM LDM loading PC stays in ARM state.
                    pipeline.branch_with_mode(value & !3, false);
                }
            } else {
                regs.set_reg(i as usize, value);
            }
            addr = addr.wrapping_add(4);
        }
    }

    let base_in_list = (rlist & (1 << rn)) != 0;
    if decoded.writes_back && !base_in_list {
        let final_addr = if is_up {
            base.wrapping_add(reg_count * 4)
        } else {
            base.wrapping_sub(reg_count * 4)
        };
        regs.set_reg(rn as usize, final_addr);
    }
}

fn exec_stm<B: Bus>(bus: &mut B, regs: &mut Registers, decoded: &DecodedInstruction) {
    let rn = decoded.rd.unwrap_or(0); // Base register is in rd
    let base = regs.get_reg(rn as usize);
    let rlist = decoded.immediate.unwrap_or(0);
    let flags = decoded.branch_target.unwrap_or(0b10);
    let is_pre = (flags & 1) != 0;
    let is_up = (flags & 2) != 0;

    let reg_count = (0..16).filter(|i| (rlist & (1 << i)) != 0).count() as u32;

    let mut addr = if is_up {
        if is_pre {
            base.wrapping_add(4)
        } else {
            base
        }
    } else {
        let start = base.wrapping_sub(reg_count * 4);
        if is_pre {
            start
        } else {
            start.wrapping_add(4)
        }
    };

    for i in 0..16u32 {
        if (rlist & (1 << i)) != 0 {
            let value = regs.get_reg(i as usize);
            bus.write_u32(addr, value);
            addr = addr.wrapping_add(4);
        }
    }

    if decoded.writes_back {
        let final_addr = if is_up {
            base.wrapping_add(reg_count * 4)
        } else {
            base.wrapping_sub(reg_count * 4)
        };
        regs.set_reg(rn as usize, final_addr);
    }
}

/// `SWP` / `SWPB`: exchange a register with memory in one go.
///
/// The read happens before the write, which is what makes `SWP Rd, Rd, [Rn]`
/// meaningful — the destination and the source can be the same register.
fn exec_swap<B: Bus>(
    bus: &mut B,
    regs: &mut Registers,
    decoded: &DecodedInstruction,
    is_byte: bool,
) {
    let addr = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let source = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);

    let loaded = if is_byte {
        bus.read_u8(addr) as u32
    } else {
        armv4_load_word(bus, addr)
    };

    if is_byte {
        bus.write_u8(addr, source as u8);
    } else {
        // The stored word goes to the aligned address, like any other store.
        bus.write_u32(addr, source);
    }

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, loaded);
    }
}

fn exec_ldr_half<B: Bus>(
    bus: &mut B,
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    is_byte: bool,
    is_signed: bool,
) {
    let (addr, wb_addr) = arm_ldr_str_addr(regs, decoded);

    let value = if is_byte && is_signed {
        // LDRSB
        bus.read_u8(addr) as i8 as i32 as u32
    } else if is_signed {
        // LDRSH
        armv4_load_signed_halfword(bus, addr)
    } else {
        // LDRH
        armv4_load_halfword(bus, addr)
    };

    // Base first, so a load into the base register keeps the loaded value
    // rather than the address, exactly as in `exec_ldr`.
    if let (Some(rn), Some(wb)) = (decoded.rn, wb_addr) {
        regs.set_reg(rn as usize, wb);
    }
    if let Some(rd) = decoded.rd {
        if rd == 15 {
            pipeline.branch_with_mode(value & !3, false);
        } else {
            regs.set_reg(rd as usize, value);
        }
    }
}

fn exec_str_half<B: Bus>(bus: &mut B, regs: &mut Registers, decoded: &DecodedInstruction) {
    let (addr, wb_addr) = arm_ldr_str_addr(regs, decoded);
    let value = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    bus.write_u16(addr, value as u16);
    if let (Some(rn), Some(wb)) = (decoded.rn, wb_addr) {
        regs.set_reg(rn as usize, wb);
    }
}

/// `MSR` — write the program status register.
///
/// Writing the T bit through `MSR` is called UNPREDICTABLE by the architecture
/// reference, but ARM7TDMI silicon honours it, and self-extracting packers use
/// it to enter the Thumb code they have just written: set T, pad with two
/// halfwords, then `BX`. Updating only the register file leaves the pipeline
/// fetching ARM, so the padding decodes as ARM and execution runs on past the
/// `BX` into whatever follows. The pipeline has to be resteered as well.
fn exec_msr(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let flags = decoded.branch_target.unwrap_or(0);
    let field_mask = flags & 0xF;
    let use_spsr = (flags & 0x10) != 0;

    let value = if let Some(imm) = decoded.immediate {
        imm
    } else if let Some(rm) = decoded.rm {
        regs.get_reg(rm as usize)
    } else {
        return;
    };

    // Build a mask from field_mask bits
    let mut mask = 0u32;
    if (field_mask & 1) != 0 {
        mask |= 0x000000FF;
    } // control
    if (field_mask & 2) != 0 {
        mask |= 0x0000FF00;
    } // extension
    if (field_mask & 4) != 0 {
        mask |= 0x00FF0000;
    } // status
    if (field_mask & 8) != 0 {
        mask |= 0xFF000000;
    } // flags

    if use_spsr {
        let current = regs.spsr().unwrap_or(0);
        regs.set_spsr((current & !mask) | (value & mask));
    } else {
        let current = regs.cpsr();
        let new_cpsr = (current & !mask) | (value & mask);
        // update_flags = true since we're writing the full value
        regs.set_cpsr(new_cpsr, true);

        if regs.is_thumb_mode() != pipeline.is_thumb_mode() {
            // Continue from the instruction after this one, in the new state.
            // The two instructions already in the pipeline were fetched as
            // ARM and must be dropped rather than executed.
            let next = pipeline.pc().wrapping_sub(ARM_INSTRUCTION_SIZE);
            pipeline.branch_with_mode(next, regs.is_thumb_mode());
        }
    }
}

fn exec_mrs(regs: &mut Registers, decoded: &DecodedInstruction) {
    let use_spsr = decoded.branch_target.unwrap_or(0) != 0;
    let value = if use_spsr {
        regs.spsr().unwrap_or(0)
    } else {
        regs.cpsr()
    };
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, value);
    }
}

fn get_operand2(regs: &Registers, decoded: &DecodedInstruction) -> u32 {
    if let Some(imm) = decoded.immediate {
        return imm;
    }
    if let Some(rm) = decoded.rm {
        let register_shift = uses_register_shift(decoded);
        let mut value = read_operand_reg(regs, rm, register_shift);
        if let Some(shift) = decoded.shift {
            let is_reg_shift = shift.shift_reg.is_some();
            let shift_amount = if let Some(rs) = shift.shift_reg {
                read_operand_reg(regs, rs, register_shift) & 0xFF
            } else {
                shift.amount as u32
            };
            // For register-shifted operands with Rs=0: result is Rm unchanged (ARM spec)
            if !is_reg_shift || shift_amount != 0 {
                let (shifted, _) = apply_shift(
                    value,
                    &ShiftInfo {
                        shift_type: shift.shift_type,
                        amount: shift_amount as u8,
                        shift_reg: None,
                    },
                    regs.flag_c(),
                );
                value = shifted;
            }
        }
        return value;
    }
    0
}

/// Whether this instruction takes its shift amount from a register.
fn uses_register_shift(decoded: &DecodedInstruction) -> bool {
    decoded
        .shift
        .as_ref()
        .map(|shift| shift.shift_reg.is_some())
        .unwrap_or(false)
}

/// Read a register as a data-processing instruction sees it.
///
/// A register-specified shift costs the core an extra cycle, and the prefetch
/// runs on during it, so PC reads as the instruction's address plus twelve
/// rather than the usual plus eight. Every register read in such an instruction
/// sees the later value, not only the shifted one.
fn read_operand_reg(regs: &Registers, index: u8, register_shift: bool) -> u32 {
    let value = regs.get_reg(index as usize);
    if index == 15 && register_shift {
        value.wrapping_add(4)
    } else {
        value
    }
}

fn get_operand2_and_carry(
    regs: &Registers,
    decoded: &DecodedInstruction,
    opcode: u32,
) -> (u32, bool) {
    if bit(opcode, 25) {
        let imm8 = bits(opcode, 7, 0);
        let rotate = bits(opcode, 11, 8) * 2;
        let value = imm8.rotate_right(rotate);
        let carry = if rotate == 0 {
            regs.flag_c()
        } else {
            (value >> 31) != 0
        };
        return (value, carry);
    }

    if let Some(rm) = decoded.rm {
        let register_shift = uses_register_shift(decoded);
        let mut value = read_operand_reg(regs, rm, register_shift);
        if let Some(shift) = decoded.shift {
            let is_reg_shift = shift.shift_reg.is_some();
            let shift_amount = if let Some(rs) = shift.shift_reg {
                read_operand_reg(regs, rs, register_shift) & 0xFF
            } else {
                shift.amount as u32
            };

            // For register-shifted operands with Rs=0, shifter carry and value are unchanged.
            if !is_reg_shift || shift_amount != 0 {
                let (shifted, carry) = apply_shift(
                    value,
                    &ShiftInfo {
                        shift_type: shift.shift_type,
                        amount: shift_amount as u8,
                        shift_reg: None,
                    },
                    regs.flag_c(),
                );
                value = shifted;
                return (value, carry);
            }
            return (value, regs.flag_c());
        }
        return (value, regs.flag_c());
    }

    (0, regs.flag_c())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SimpleBus;

    fn run_arm(opcode: u32, regs: &mut Registers) {
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        let mut instr = Instruction::arm(opcode);
        decode_arm(&mut instr);
        execute_arm(&instr, &mut bus, regs, &mut pipeline);
    }

    fn run_arm_with_bus(
        opcode: u32,
        regs: &mut Registers,
        bus: &mut SimpleBus,
        pipeline: &mut Pipeline,
    ) {
        let mut instr = Instruction::arm(opcode);
        decode_arm(&mut instr);
        execute_arm(&instr, bus, regs, pipeline);
    }

    #[test]
    fn test_mul_basic() {
        // MUL R2, R0, R1  (R2 = R0 * R1)
        // opcode: E0020190
        let mut regs = Registers::new();
        regs.set_reg(0, 6);
        regs.set_reg(1, 7);
        run_arm(0xE0020190, &mut regs);
        assert_eq!(regs.get_reg(2), 42);
    }

    #[test]
    fn test_mla_basic() {
        // MLA R4, R0, R1, R2  (R4 = R0 * R1 + R2)
        // opcode: E0242190
        let mut regs = Registers::new();
        regs.set_reg(0, 6);
        regs.set_reg(1, 7);
        regs.set_reg(2, 10);
        run_arm(0xE0242190, &mut regs);
        assert_eq!(regs.get_reg(4), 52);
    }

    #[test]
    fn test_umull_basic() {
        // UMULL R3, R2, R0, R1  (R3:R2 = R0 * R1, unsigned)
        // opcode: E0832190
        let mut regs = Registers::new();
        regs.set_reg(0, 0xFFFF_FFFF);
        regs.set_reg(1, 2);
        run_arm(0xE0832190, &mut regs);
        // 0xFFFFFFFF * 2 = 0x1_FFFF_FFFE
        assert_eq!(regs.get_reg(2), 0xFFFF_FFFE); // RdLo
        assert_eq!(regs.get_reg(3), 0x0000_0001); // RdHi
    }

    #[test]
    fn test_smull_negative() {
        // SMULL R3, R2, R0, R1  (R3:R2 = R0 * R1, signed)
        // opcode: E0C32190
        let mut regs = Registers::new();
        regs.set_reg(0, (-3i32) as u32);
        regs.set_reg(1, 4);
        run_arm(0xE0C32190, &mut regs);
        // -3 * 4 = -12 = 0xFFFFFFFF_FFFFFFF4
        assert_eq!(regs.get_reg(2), 0xFFFF_FFF4); // RdLo
        assert_eq!(regs.get_reg(3), 0xFFFF_FFFF); // RdHi
    }

    #[test]
    fn test_umlal_accumulate() {
        // UMLAL R3, R2, R0, R1  (R3:R2 += R0 * R1)
        // opcode: E0A32190
        let mut regs = Registers::new();
        regs.set_reg(0, 5);
        regs.set_reg(1, 5);
        regs.set_reg(2, 100); // RdLo = 100
        regs.set_reg(3, 0); // RdHi = 0
        run_arm(0xE0A32190, &mut regs);
        // 5 * 5 + 100 = 125
        assert_eq!(regs.get_reg(2), 125);
        assert_eq!(regs.get_reg(3), 0);
    }

    #[test]
    fn test_smlal_accumulate() {
        // SMLAL R3, R2, R0, R1  (R3:R2 += R0 * R1, signed)
        // opcode: E0E32190
        let mut regs = Registers::new();
        regs.set_reg(0, (-2i32) as u32);
        regs.set_reg(1, 3);
        regs.set_reg(2, 50); // RdLo
        regs.set_reg(3, 0); // RdHi
        run_arm(0xE0E32190, &mut regs);
        // -2 * 3 + 50 = 44
        assert_eq!(regs.get_reg(2), 44);
        assert_eq!(regs.get_reg(3), 0);
    }

    #[test]
    fn test_decode_mov() {
        let mut instr = Instruction::arm(0xE3A0002A);
        decode_arm(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Mov);
        assert_eq!(decoded.rd, Some(0));
        assert_eq!(decoded.immediate, Some(42));
    }

    #[test]
    fn test_decode_add() {
        let mut instr = Instruction::arm(0xE0810002);
        decode_arm(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Add);
        assert_eq!(decoded.rd, Some(0));
        assert_eq!(decoded.rn, Some(1));
        assert_eq!(decoded.rm, Some(2));
    }

    #[test]
    fn test_decode_cmp_not_mrs() {
        // CMP LR, R11
        let mut instr = Instruction::arm(0xE15E000B);
        decode_arm(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Cmp);
        assert_eq!(decoded.rn, Some(14));
        assert_eq!(decoded.rm, Some(11));
        assert_eq!(decoded.rd, None);
    }

    #[test]
    fn test_cmp_sets_zero_flag_when_equal() {
        // CMP LR, R11 with equal values should set Z=1.
        let mut regs = Registers::new();
        regs.set_reg(0, 0x1234_5678); // Guard against accidental MRS decode clobbering r0.
        regs.set_reg(14, 8);
        regs.set_reg(11, 8);
        run_arm(0xE15E000B, &mut regs);
        assert!(regs.flag_z());
        assert!(regs.flag_c());
        assert_eq!(regs.get_reg(0), 0x1234_5678);
    }

    #[test]
    fn test_cmp_shifted_operand_keeps_carry_set_when_equal() {
        // CMP R2, R0, LSR #1  with both operands resolving to 0.
        let mut instr = Instruction::arm(0xE15200A0);
        decode_arm(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Cmp);

        let mut regs = Registers::new();
        regs.set_reg(2, 0);
        regs.set_reg(0, 0);
        run_arm(0xE15200A0, &mut regs);
        assert!(regs.flag_z());
        assert!(regs.flag_c());
    }

    /// `MSR` writing the T bit must move the pipeline into Thumb as well.
    ///
    /// Self-extracting packers use this to enter code they have just written:
    /// set T through `MSR`, pad, then `BX`. Updating only the register file
    /// left the pipeline fetching ARM, so the padding decoded as ARM and
    /// execution ran straight past the `BX` into whatever followed. Pokemon
    /// Pinball: Ruby & Sapphire unpacks itself exactly this way and hung on a
    /// white screen because of it.
    #[test]
    fn msr_switching_to_thumb_resteers_the_pipeline() {
        // MSR CPSR_fc, r2 with r2 = mode bits plus T.
        let mut regs = Registers::new();
        regs.set_reg(2, 0x0000_003F);
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_fetch_addr(0x0800_0100);
        assert!(!pipeline.is_thumb_mode());

        run_arm_with_bus(0xE129_F002, &mut regs, &mut bus, &mut pipeline);

        assert!(regs.is_thumb_mode(), "the register file should be in Thumb");
        assert!(pipeline.is_thumb_mode(), "so should the pipeline");
    }

    /// The mirror case: leaving Thumb has to resteer too.
    #[test]
    fn msr_leaving_thumb_resteers_the_pipeline() {
        let mut regs = Registers::new();
        regs.set_cpsr(regs.cpsr() | (1 << 5), true); // set T
        regs.set_reg(2, 0x0000_001F);
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_fetch_addr(0x0800_0100);
        pipeline.set_thumb_mode(true);

        run_arm_with_bus(0xE129_F002, &mut regs, &mut bus, &mut pipeline);

        assert!(!regs.is_thumb_mode());
        assert!(!pipeline.is_thumb_mode());
    }

    /// An `MSR` that does not touch T must leave the pipeline alone.
    #[test]
    fn msr_that_only_writes_flags_does_not_resteer() {
        let mut regs = Registers::new();
        regs.set_reg(2, 0xF000_0000);
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_fetch_addr(0x0800_0100);

        // MSR CPSR_f, r2 -- flags field only.
        run_arm_with_bus(0xE128_F002, &mut regs, &mut bus, &mut pipeline);

        assert!(!pipeline.is_thumb_mode());
    }

    /// `ROR` by exactly 32, from a register.
    ///
    /// ARM treats a register-specified rotate of 32 as "leave the value alone,
    /// but set carry from bit 31" — distinct both from a rotate of 0, which
    /// leaves carry alone, and from RRX, which is what an *immediate* rotate of
    /// 0 encodes. jsmolka's arm.gba checks this as test 164.
    #[test]
    fn ror_by_thirty_two_from_a_register_keeps_the_value_and_sets_carry() {
        let mut regs = Registers::new();
        regs.set_reg(0, 0x8000_0000);
        regs.set_reg(1, 32);
        regs.set_flags(false, false, false, false);

        // MOVS r0, r0, ROR r1
        run_arm(0xE1B0_0170, &mut regs);

        assert_eq!(regs.get_reg(0), 0x8000_0000, "the value must be unchanged");
        assert!(regs.flag_c(), "carry must come from bit 31");
    }

    /// PC reads as instruction+12 when the shift amount comes from a register.
    ///
    /// The register-specified shift costs an extra cycle and the prefetch runs
    /// on during it. Reading the usual +8 puts every such instruction four
    /// bytes out. jsmolka's arm.gba checks this as test 224.
    #[test]
    fn a_register_shift_reads_pc_as_instruction_plus_twelve() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_fetch_addr(0x0800_0100);
        // The core stages R15 as instruction + 8 before executing.
        regs.set_pc(0x0800_0108);
        regs.set_reg(0, 0);

        // MOV r0, pc, LSL r0 -- a register-specified shift of zero, so the
        // only thing under test is which PC the instruction sees.
        run_arm_with_bus(0xE1A0_001F, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(
            regs.get_reg(0),
            0x0800_010C,
            "a register-shifted operand must see PC as instruction + 12"
        );
    }

    /// An immediate shift keeps the ordinary +8.
    #[test]
    fn an_immediate_shift_reads_pc_as_instruction_plus_eight() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_fetch_addr(0x0800_0100);
        regs.set_pc(0x0800_0108);

        // MOV r0, pc  -- no shift register involved.
        run_arm_with_bus(0xE1A0_000F, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0x0800_0108);
    }

    /// The neighbouring cases, so a fix for one cannot break the others.
    #[test]
    fn ror_by_a_register_covers_zero_and_more_than_thirty_two() {
        // ROR by 0 leaves both the value and the carry alone.
        let mut regs = Registers::new();
        regs.set_reg(0, 0x8000_0000);
        regs.set_reg(1, 0);
        regs.set_flags(false, false, false, false);
        run_arm(0xE1B0_0170, &mut regs);
        assert_eq!(regs.get_reg(0), 0x8000_0000);
        assert!(!regs.flag_c(), "a rotate of zero must not touch carry");

        // ROR by 33 is ROR by 1.
        let mut regs = Registers::new();
        regs.set_reg(0, 2);
        regs.set_reg(1, 33);
        run_arm(0xE1B0_0170, &mut regs);
        assert_eq!(regs.get_reg(0), 1, "33 should rotate by one");
    }

    #[test]
    fn test_ands_immediate_sets_c_from_shifter_carry() {
        // ANDS R3, R1, #0x80000000 (imm8=0x02, rotate=2)
        let mut regs = Registers::new();
        regs.set_reg(1, 0x0000_0002);
        regs.set_flags(false, false, false, true); // Set V=1 to verify it is preserved.

        run_arm(0xE211_3102, &mut regs);

        assert_eq!(regs.get_reg(3), 0);
        assert!(regs.flag_z());
        assert!(regs.flag_c());
        assert!(regs.flag_v());
    }

    #[test]
    fn test_tst_immediate_sets_c_from_shifter_carry() {
        // TST R1, #0x80000000 (imm8=0x02, rotate=2)
        let mut regs = Registers::new();
        regs.set_reg(1, 0);
        regs.set_flags(false, false, false, true); // Keep V set.

        run_arm(0xE311_3102, &mut regs);

        assert!(regs.flag_z());
        assert!(regs.flag_c());
        assert!(regs.flag_v());
    }

    #[test]
    fn test_decode_branch() {
        let mut instr = Instruction::arm(0xEA000002);
        decode_arm(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::B);
        assert_eq!(decoded.immediate, Some(8));
    }

    #[test]
    fn test_arm_ldr_rotates_misaligned_word() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u32(0x0200_0000, 0x0062_A4C3);
        regs.set_reg(1, 0x0200_0001);

        run_arm_with_bus(0xE591_0000, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0xC300_62A4);
    }

    #[test]
    fn test_arm_ldr_pc_stays_arm_and_word_aligned() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u32(0x0200_0000, 0x0800_0003);
        regs.set_reg(0, 0x0200_0000);

        run_arm_with_bus(0xE590_F000, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(pipeline.fetch_addr, 0x0800_0000);
        assert!(!pipeline.is_thumb_mode());
        assert!(!regs.is_thumb_mode());
    }

    #[test]
    fn test_arm_ldm_pc_does_not_interwork() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        let decoded = DecodedInstruction {
            category: InstructionCategory::Ldm,
            condition: 0xE,
            rd: Some(0),
            rn: None,
            rm: None,
            shift: None,
            immediate: Some(1 << 15),
            branch_target: Some(0b10),
            writes_back: false,
        };

        regs.set_reg(0, 0x0200_0000);
        bus.write_u32(0x0200_0000, 0x0800_0001);

        exec_ldm(&mut bus, &mut regs, &mut pipeline, &decoded);

        assert_eq!(pipeline.fetch_addr, 0x0800_0000);
        assert!(!pipeline.is_thumb_mode());
        assert!(!regs.is_thumb_mode());
    }

    #[test]
    fn test_arm_ldm_with_pc_in_list_still_writes_back_base() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        let decoded = DecodedInstruction {
            category: InstructionCategory::Ldm,
            condition: 0xE,
            rd: Some(13),
            rn: None,
            rm: None,
            shift: None,
            immediate: Some((1 << 0) | (1 << 15)),
            branch_target: Some(0b10),
            writes_back: true,
        };

        regs.set_reg(13, 0x0200_0000);
        bus.write_u32(0x0200_0000, 0xDEAD_BEEF);
        bus.write_u32(0x0200_0004, 0x0800_0001);

        exec_ldm(&mut bus, &mut regs, &mut pipeline, &decoded);

        assert_eq!(regs.get_reg(0), 0xDEAD_BEEF);
        assert_eq!(regs.get_reg(13), 0x0200_0008);
        assert_eq!(pipeline.fetch_addr, 0x0800_0000);
        assert!(!pipeline.is_thumb_mode());
    }

    #[test]
    fn test_arm_ldrh_rotates_on_odd_address() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u16(0x0200_0000, 0x1234);
        regs.set_reg(1, 0x0200_0001);

        run_arm_with_bus(0xE1D1_00B0, &mut regs, &mut bus, &mut pipeline);

        // The rotate is of the 32-bit register value, not of the halfword.
        // This asserted 0x3412 while the emulator rotated the narrow value,
        // which left the bytes in the wrong half of the register.
        assert_eq!(regs.get_reg(0), 0x3400_0012);
    }

    #[test]
    fn test_arm_ldrsh_odd_address_acts_like_ldrsb() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u8(0x0200_0001, 0x80);
        regs.set_reg(1, 0x0200_0001);

        run_arm_with_bus(0xE1D1_00F0, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0xFFFF_FF80);
    }
}

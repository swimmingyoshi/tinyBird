//! Thumb Instruction Set Decoder and Executor
//!
//! This module implements the 16-bit Thumb instruction set for the ARM7TDMI core.
//! Thumb instructions provide better code density than ARM instructions.
//! Uses direct decoding based on instruction bit patterns.

use crate::bios::Bios;
use crate::bus::Bus;
use crate::cpu::pipeline::{
    decode_utils::{apply_shift, bit, bits},
    DecodedInstruction, Instruction, InstructionCategory, Pipeline, ShiftInfo, ShiftType,
};
use crate::cpu::registers::{CpuMode, Registers};
use crate::cpu::{armv4_load_halfword, armv4_load_signed_halfword, armv4_load_word};
use crate::debug::config as debug_config;

/// Decode a Thumb instruction directly from the opcode bits
fn decode_thumb_instruction(opcode: u16) -> Option<DecodedInstruction> {
    // Thumb instruction decoding based on bit patterns
    // The top bits determine the instruction format

    let top_bits = (opcode >> 10) & 0x3F;

    // Format 1: Shift by immediate (000xxxx) - bits [15:13] = 0b000, op in [12:11]
    // LSL: top_bits 0-1, LSR: 2-3, ASR: 4-5
    if top_bits < 0b000110 {
        return decode_shift_imm(opcode);
    }

    // Format 2: Add/Subtract (00011x) - bits [15:10] = 0b00011x
    if top_bits >= 0b000110 && top_bits < 0b001000 {
        return decode_add_sub(opcode);
    }

    // Format 3: Move/Compare/Add immediate (001xx) - bits [15:11] = 0b001xx
    if top_bits >= 0b001000 && top_bits < 0b010000 {
        return decode_move_compare(opcode);
    }

    // Format 4: ALU operations (010000) - bits [15:10] = 0b010000
    if top_bits == 0b010000 {
        return decode_alu(opcode);
    }

    // Format 5: Hi register operations (010001) - bits [15:10] = 0b010001
    if top_bits == 0b010001 {
        return decode_hi_reg(opcode);
    }

    // Format 6: PC-relative load (01001) - bits [15:11] = 0b01001
    if top_bits >= 0b010010 && top_bits < 0b010100 {
        return decode_ldr_pc(opcode);
    }

    // Format 7: Load/Store with register offset (0101) - bits [15:12] = 0b0101
    if top_bits >= 0b010100 && top_bits < 0b011000 {
        return decode_ldr_str_reg(opcode);
    }

    // Format 9: Load/Store word/byte with immediate offset (011x) - bits [15:13] = 0b011
    if top_bits >= 0b011000 && top_bits < 0b100000 {
        // Bit 12 (B flag): 0 = word transfer (offset*4), 1 = byte transfer (offset)
        return decode_ldr_str_imm_offset(opcode);
    }

    // Format 10: Load/Store halfword (1000) - bits [15:12] = 0b1000
    if top_bits >= 0b100000 && top_bits < 0b100100 {
        return decode_ldr_str_half_imm(opcode);
    }

    // Format 10: Stack load/store (1001) - bits [15:11] = 0b1001x
    if top_bits >= 0b100100 && top_bits < 0b101000 {
        return decode_stack(opcode);
    }

    // Format 12: Load address (1010x) - bits [15:12] = 0b1010
    if top_bits >= 0b101000 && top_bits < 0b101100 {
        if (opcode & 0x0800) == 0 {
            return decode_add_pc(opcode); // ADD Rd, PC, #imm
        } else {
            return decode_add_sp_imm(opcode); // ADD Rd, SP, #imm
        }
    }

    // Format 13: Adjust stack pointer (10110000_xxxxxxxx)
    if (opcode & 0xFF00) == 0xB000 {
        return decode_adjust_sp(opcode);
    }

    // Format 13: Push/Pop (1011x10) - bits [15:9] = 0b1011x10
    if (opcode & 0xFE00) == 0xB400 || (opcode & 0xFE00) == 0xBC00 {
        return decode_push_pop(opcode);
    }

    // Format 15: Multiple load/store (1100) - bits [15:12] = 0b1100
    if top_bits >= 0b110000 && top_bits < 0b110100 {
        return decode_ldm_stm(opcode);
    }

    // Format 16: Conditional branch (1101) - bits [15:12] = 0b1101
    if top_bits >= 0b110100 && top_bits < 0b111000 {
        // Check for SWI (11011111)
        if (opcode & 0xFF00) == 0xDF00 {
            return decode_swi(opcode);
        }
        return decode_cond_branch(opcode);
    }

    // Format 17: Unconditional branch (11100) - bits [15:11] = 0b11100
    if top_bits >= 0b111000 && top_bits < 0b111010 {
        return decode_uncond_branch(opcode);
    }

    // Format 18/19: Long branch with link - two-part instruction
    // First half: 11110_xxxxxxxxxxx (bits[15:11] = 11110) - set LR = PC + (offset << 12)
    // Second half: 11111_xxxxxxxxxxx (bits[15:11] = 11111) - BL: branch to LR + (offset << 1)
    //              11101_xxxxxxxxxxx (bits[15:11] = 11101) - BLX: same but switch to ARM
    let top5 = (opcode >> 11) & 0x1F;
    if top5 == 0b11110 {
        return decode_bl_prefix(opcode);
    }
    if top5 == 0b11111 {
        return decode_bl_suffix(opcode);
    }
    if top5 == 0b11101 {
        return decode_blx(opcode);
    }

    // Undefined
    Some(DecodedInstruction {
        category: InstructionCategory::Undefined,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: None,
        branch_target: None,
        writes_back: false,
    })
}

fn decode_shift_imm(opcode: u16) -> Option<DecodedInstruction> {
    let op = bits(opcode as u32, 12, 11) as u8;
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rs = bits(opcode as u32, 5, 3) as u8;
    let imm5 = bits(opcode as u32, 10, 6) as u8;

    let category = match op {
        0b00 => InstructionCategory::ThumbShift,
        0b01 => InstructionCategory::ThumbShift,
        0b10 => InstructionCategory::ThumbShift,
        _ => InstructionCategory::Undefined,
    };

    let shift_type = match op {
        0b00 => ShiftType::Lsl,
        0b01 => ShiftType::Lsr,
        0b10 => ShiftType::Asr,
        _ => ShiftType::Lsl,
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rs),
        rm: None,
        shift: Some(ShiftInfo {
            shift_type,
            amount: imm5,
            shift_reg: None,
        }),
        immediate: None,
        branch_target: None,
        writes_back: true,
    })
}

fn decode_add_sub(opcode: u16) -> Option<DecodedInstruction> {
    let i_flag = bit(opcode as u32, 10);
    let is_sub = bit(opcode as u32, 9);
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rs = bits(opcode as u32, 5, 3) as u8;
    let rn_or_imm3 = bits(opcode as u32, 8, 6);

    let category = if is_sub {
        InstructionCategory::Sub
    } else {
        InstructionCategory::Add
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rs),
        rm: if !i_flag {
            Some(rn_or_imm3 as u8)
        } else {
            None
        },
        shift: None,
        immediate: if i_flag { Some(rn_or_imm3) } else { None },
        branch_target: None,
        writes_back: true,
    })
}

fn decode_move_compare(opcode: u16) -> Option<DecodedInstruction> {
    let op = bits(opcode as u32, 12, 11) as u8;
    let rd = bits(opcode as u32, 10, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);

    let category = match op {
        0b00 => InstructionCategory::Mov,
        0b01 => InstructionCategory::Cmp,
        0b10 => InstructionCategory::Add,
        0b11 => InstructionCategory::Sub,
        _ => InstructionCategory::Undefined,
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm8),
        branch_target: None,
        writes_back: true, // All Format 3 ops set flags
    })
}

fn decode_alu(opcode: u16) -> Option<DecodedInstruction> {
    let op = bits(opcode as u32, 9, 6) as u8;
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rm = bits(opcode as u32, 5, 3) as u8;

    // All Thumb ALU ops use Rd as both source and destination
    // op encodes the operation (16 opcodes)
    let category = match op {
        0x0 => InstructionCategory::And,        // AND Rd, Rs
        0x1 => InstructionCategory::Eor,        // EOR Rd, Rs
        0x2 => InstructionCategory::ThumbShift, // LSL Rd, Rs (shift by register)
        0x3 => InstructionCategory::ThumbShift, // LSR Rd, Rs
        0x4 => InstructionCategory::ThumbShift, // ASR Rd, Rs
        0x5 => InstructionCategory::Adc,        // ADC Rd, Rs
        0x6 => InstructionCategory::Sbc,        // SBC Rd, Rs
        0x7 => InstructionCategory::ThumbShift, // ROR Rd, Rs
        0x8 => InstructionCategory::Tst,        // TST Rd, Rs
        0x9 => InstructionCategory::Sub,        // NEG Rd, Rs (Rd = 0 - Rs)
        0xA => InstructionCategory::Cmp,        // CMP Rd, Rs
        0xB => InstructionCategory::Cmn,        // CMN Rd, Rs
        0xC => InstructionCategory::Orr,        // ORR Rd, Rs
        0xD => InstructionCategory::Mul,        // MUL Rd, Rs
        0xE => InstructionCategory::Bic,        // BIC Rd, Rs
        0xF => InstructionCategory::Mvn,        // MVN Rd, Rs
        _ => InstructionCategory::Undefined,
    };

    // For shift by register, encode the shift type
    let shift = match op {
        0x2 => Some(ShiftInfo {
            shift_type: ShiftType::Lsl,
            amount: 0,
            shift_reg: Some(rm),
        }),
        0x3 => Some(ShiftInfo {
            shift_type: ShiftType::Lsr,
            amount: 0,
            shift_reg: Some(rm),
        }),
        0x4 => Some(ShiftInfo {
            shift_type: ShiftType::Asr,
            amount: 0,
            shift_reg: Some(rm),
        }),
        0x7 => Some(ShiftInfo {
            shift_type: ShiftType::Ror,
            amount: 0,
            shift_reg: Some(rm),
        }),
        _ => None,
    };

    // For NEG (0x9): Rd = 0 - Rs. Set rn to None so exec_sub uses 0 as source.
    let rn_field = if op == 0x9 { None } else { Some(rd) };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: rn_field,
        rm: Some(rm),
        shift,
        immediate: Some(op as u32), // Pass ALU op code
        branch_target: None,
        writes_back: true,
    })
}

fn decode_hi_reg(opcode: u16) -> Option<DecodedInstruction> {
    let op = bits(opcode as u32, 9, 8) as u8;
    let rd = bits(opcode as u32, 2, 0) as u8 | ((bits(opcode as u32, 7, 7) as u8) << 3);
    let rm = bits(opcode as u32, 5, 3) as u8 | ((bits(opcode as u32, 6, 6) as u8) << 3);

    let category = match op {
        0b00 => InstructionCategory::Add,
        0b01 => InstructionCategory::Cmp,
        0b10 => InstructionCategory::Mov,
        0b11 => InstructionCategory::Bx,
        _ => InstructionCategory::Undefined,
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rm),
        rm: None,
        shift: None,
        immediate: None,
        branch_target: None,
        writes_back: matches!(category, InstructionCategory::Cmp),
    })
}

fn decode_ldr_pc(opcode: u16) -> Option<DecodedInstruction> {
    let rd = bits(opcode as u32, 10, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::Ldr,
        condition: 0xE,
        rd: Some(rd),
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm8 << 2),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_ldr_str_reg(opcode: u16) -> Option<DecodedInstruction> {
    let op = bits(opcode as u32, 11, 9) as u8;
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rn = bits(opcode as u32, 5, 3) as u8;
    let rm = bits(opcode as u32, 8, 6) as u8;

    let category = match op {
        0b000 => InstructionCategory::Str,
        0b001 => InstructionCategory::Strh,
        0b010 => InstructionCategory::Strb,
        0b011 => InstructionCategory::Ldrsb,
        0b100 => InstructionCategory::Ldr,
        0b101 => InstructionCategory::Ldrh,
        0b110 => InstructionCategory::Ldrb,
        0b111 => InstructionCategory::Ldrsh,
        _ => InstructionCategory::Undefined,
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rn),
        rm: Some(rm),
        shift: None,
        immediate: None,
        branch_target: None,
        writes_back: false,
    })
}

/// Format 9: Load/Store word/byte with 5-bit immediate offset
/// Encoding: 011BL_iiiii_nnn_ddd
/// B=bit12 (0=word, 1=byte), L=bit11 (0=store, 1=load)
fn decode_ldr_str_imm_offset(opcode: u16) -> Option<DecodedInstruction> {
    let is_byte = (opcode & 0x1000) != 0;
    let is_load = (opcode & 0x0800) != 0;
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rn = bits(opcode as u32, 5, 3) as u8;
    let imm5 = bits(opcode as u32, 10, 6);

    let category = match (is_load, is_byte) {
        (true, false) => InstructionCategory::Ldr,
        (true, true) => InstructionCategory::Ldrb,
        (false, false) => InstructionCategory::Str,
        (false, true) => InstructionCategory::Strb,
    };

    // Word offset is imm5 * 4, byte offset is imm5
    let offset = if is_byte { imm5 } else { imm5 << 2 };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rn),
        rm: None,
        shift: None,
        immediate: Some(offset),
        branch_target: None,
        writes_back: false,
    })
}

/// Format 10: Load/Store halfword with 5-bit immediate offset
/// Encoding: 1000L_iiiii_nnn_ddd
/// L=bit11 (0=store, 1=load), offset = imm5 * 2
fn decode_ldr_str_half_imm(opcode: u16) -> Option<DecodedInstruction> {
    let is_load = (opcode & 0x0800) != 0;
    let rd = bits(opcode as u32, 2, 0) as u8;
    let rn = bits(opcode as u32, 5, 3) as u8;
    let imm5 = bits(opcode as u32, 10, 6);

    let category = if is_load {
        InstructionCategory::Ldrh
    } else {
        InstructionCategory::Strh
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(rn),
        rm: None,
        shift: None,
        immediate: Some(imm5 << 1),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_stack(opcode: u16) -> Option<DecodedInstruction> {
    let is_load = (opcode & 0x0800) != 0;
    let rd = bits(opcode as u32, 10, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);

    let category = if is_load {
        InstructionCategory::Ldr
    } else {
        InstructionCategory::Str
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rd),
        rn: Some(13), // SP-relative
        rm: None,
        shift: None,
        immediate: Some(imm8 as u32 * 4),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_add_pc(opcode: u16) -> Option<DecodedInstruction> {
    let rd = bits(opcode as u32, 10, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::ThumbMove,
        condition: 0xE,
        rd: Some(rd),
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm8 as u32 * 4),
        branch_target: None,
        writes_back: false,
    })
}

/// Format 12: ADD Rd, SP, #imm8*4
/// Called when bits[15:11] = 10101
fn decode_add_sp_imm(opcode: u16) -> Option<DecodedInstruction> {
    let rd = bits(opcode as u32, 10, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);
    Some(DecodedInstruction {
        category: InstructionCategory::ThumbAddSub,
        condition: 0xE,
        rd: Some(rd),
        rn: None, // rn=None signals SP-relative in exec_add_sub
        rm: None,
        shift: None,
        immediate: Some(imm8 as u32 * 4),
        branch_target: None,
        writes_back: false,
    })
}

/// Format 13: ADD/SUB SP, #imm7*4
/// Called when bits[15:8] = 10110000
fn decode_adjust_sp(opcode: u16) -> Option<DecodedInstruction> {
    let is_sub = (opcode & 0x0080) != 0;
    let imm7 = bits(opcode as u32, 6, 0);
    let offset = imm7 as u32 * 4;
    Some(DecodedInstruction {
        category: InstructionCategory::ThumbAddSub,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(if is_sub {
            (-(offset as i32)) as u32
        } else {
            offset
        }),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_push_pop(opcode: u16) -> Option<DecodedInstruction> {
    let is_pop = (opcode & 0x0800) != 0;
    let rlist = bits(opcode as u32, 7, 0); // Only bits 0-7 are register list
    let r_bit = (opcode & 0x0100) != 0; // bit 8: include LR (push) or PC (pop)

    // Encode: branch_target bit 0 = is_pop, bit 1 = r_bit
    let flags = (if is_pop { 1u32 } else { 0 }) | (if r_bit { 2 } else { 0 });

    Some(DecodedInstruction {
        category: InstructionCategory::ThumbPushPop,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(rlist as u32),
        branch_target: Some(flags),
        writes_back: true,
    })
}

fn decode_ldm_stm(opcode: u16) -> Option<DecodedInstruction> {
    let is_load = (opcode & 0x0800) != 0;
    let rn = bits(opcode as u32, 10, 8) as u8;
    let rlist = bits(opcode as u32, 7, 0);

    let category = if is_load {
        InstructionCategory::Ldm
    } else {
        InstructionCategory::Stm
    };

    Some(DecodedInstruction {
        category,
        condition: 0xE,
        rd: Some(rn),
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(rlist as u32),
        branch_target: None,
        writes_back: true,
    })
}

fn decode_cond_branch(opcode: u16) -> Option<DecodedInstruction> {
    let cond = bits(opcode as u32, 11, 8) as u8;
    let imm8 = bits(opcode as u32, 7, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::ThumbBranch,
        condition: cond,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some((imm8 as i8 as i32 * 2) as u32),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_swi(opcode: u16) -> Option<DecodedInstruction> {
    let imm8 = bits(opcode as u32, 7, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::Swi,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm8 as u32),
        branch_target: None,
        writes_back: false,
    })
}

fn decode_uncond_branch(opcode: u16) -> Option<DecodedInstruction> {
    let imm11 = bits(opcode as u32, 10, 0);
    // Sign-extend 11-bit offset and multiply by 2
    let offset = ((imm11 as i32) << 21) >> 20; // sign-extend and *2

    Some(DecodedInstruction {
        category: InstructionCategory::ThumbBranch,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(offset as u32),
        branch_target: None,
        writes_back: false,
    })
}

/// BL prefix: stores PC + (sign_extended_offset << 12) into LR
fn decode_bl_prefix(opcode: u16) -> Option<DecodedInstruction> {
    let imm11 = bits(opcode as u32, 10, 0);
    // Sign-extend 11 bits, shift left 12
    let offset = (((imm11 as i32) << 21) >> 9) as u32; // sign-extend then << 12

    Some(DecodedInstruction {
        category: InstructionCategory::ThumbMisc, // Use ThumbMisc for BL prefix
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(offset),
        branch_target: Some(1), // Flag: this is BL prefix
        writes_back: false,
    })
}

/// BL suffix: branches to LR + (offset << 1), stores return in LR
fn decode_bl_suffix(opcode: u16) -> Option<DecodedInstruction> {
    let imm11 = bits(opcode as u32, 10, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::Bl,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm11 << 1),
        branch_target: Some(2), // Flag: this is BL suffix (stay Thumb)
        writes_back: true,
    })
}

/// BLX suffix: branches to LR + (offset << 1), switches to ARM
fn decode_blx(opcode: u16) -> Option<DecodedInstruction> {
    let imm11 = bits(opcode as u32, 10, 0);

    Some(DecodedInstruction {
        category: InstructionCategory::Blx,
        condition: 0xE,
        rd: None,
        rn: None,
        rm: None,
        shift: None,
        immediate: Some(imm11 << 1),
        branch_target: Some(3), // Flag: BLX suffix (switch to ARM)
        writes_back: true,
    })
}

/// Decode a Thumb instruction
#[allow(missing_docs)]
pub fn decode_thumb(instruction: &mut Instruction) {
    let opcode = instruction.opcode as u16;
    instruction.decoded = decode_thumb_instruction(opcode);
}

/// Execute a Thumb instruction
#[allow(missing_docs)]
pub fn execute_thumb<B: Bus>(
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

    match decoded.category {
        InstructionCategory::ThumbShift => exec_shift(regs, decoded),
        InstructionCategory::ThumbAddSub => exec_add_sub(regs, decoded),
        InstructionCategory::Mov => {
            exec_mov(regs, decoded);
            // Format 5 hi-reg MOV to PC is a branch (stays in Thumb mode)
            // R15 can only appear here via Format 5 (hi-reg), not Format 3 (3-bit Rd)
            if decoded.rd == Some(15) {
                let target = regs.get_reg(15) & !1;
                pipeline.branch_with_mode(target, true);
            }
        }
        InstructionCategory::Cmp => exec_cmp(regs, decoded),
        InstructionCategory::Add => {
            exec_add(regs, decoded);
            // Format 5 hi-reg ADD to PC is a branch (stays in Thumb mode)
            if decoded.immediate.is_none() && decoded.rm.is_none() && decoded.rd == Some(15) {
                let target = regs.get_reg(15) & !1;
                pipeline.branch_with_mode(target, true);
            }
        }
        InstructionCategory::Sub => exec_sub(regs, decoded),
        InstructionCategory::And => exec_alu(regs, decoded, |a, b| a & b),
        InstructionCategory::Eor => exec_alu(regs, decoded, |a, b| a ^ b),
        InstructionCategory::Orr => exec_alu(regs, decoded, |a, b| a | b),
        InstructionCategory::Bic => exec_alu(regs, decoded, |a, b| a & !b),
        InstructionCategory::Mvn => {
            let rm = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let result = !rm;
            if let Some(rd) = decoded.rd {
                regs.set_reg(rd as usize, result);
            }
            regs.set_zn_flags(result);
        }
        InstructionCategory::Tst => {
            let rd_val = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let result = rd_val & rm_val;
            regs.set_zn_flags(result);
        }
        InstructionCategory::Cmn => {
            let rd_val = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let (result, carry) = rd_val.overflowing_add(rm_val);
            let overflow =
                (((rd_val as i32) >= 0) && ((rm_val as i32) >= 0) && ((result as i32) < 0))
                    || (((rd_val as i32) < 0) && ((rm_val as i32) < 0) && ((result as i32) >= 0));
            regs.set_flags((result as i32) < 0, result == 0, carry, overflow);
        }
        InstructionCategory::Adc => exec_adc(regs, decoded),
        InstructionCategory::Sbc => exec_sbc(regs, decoded),
        InstructionCategory::Mul => {
            let rd_val = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
            let result = rd_val.wrapping_mul(rm_val);
            if let Some(rd) = decoded.rd {
                regs.set_reg(rd as usize, result);
            }
            regs.set_zn_flags(result);
        }
        InstructionCategory::Ldr => exec_ldr(regs, pipeline, decoded, bus, false, false),
        InstructionCategory::Str => exec_str(regs, decoded, bus, false, false),
        InstructionCategory::Ldrb => exec_ldr(regs, pipeline, decoded, bus, true, false),
        InstructionCategory::Strb => exec_str(regs, decoded, bus, true, false),
        InstructionCategory::Ldrh => exec_ldr(regs, pipeline, decoded, bus, false, true),
        InstructionCategory::Strh => exec_str(regs, decoded, bus, false, true),
        InstructionCategory::ThumbBranch => exec_branch(regs, pipeline, decoded, false),
        InstructionCategory::Bl => exec_bl_suffix(regs, pipeline, decoded),
        InstructionCategory::Blx => exec_blx_suffix(regs, pipeline, decoded),
        InstructionCategory::ThumbMisc => {
            // BL prefix: LR = PC + offset
            if decoded.branch_target == Some(1) {
                let pc = pipeline.pc();
                let offset = decoded.immediate.unwrap_or(0);
                regs.set_lr(pc.wrapping_add(offset));
            }
        }
        InstructionCategory::ThumbMove => {
            // ADD Rd, PC, #imm8*4 (load address from PC)
            let pc = pipeline.pc() & !2;
            let result = pc.wrapping_add(decoded.immediate.unwrap_or(0));
            if let Some(rd) = decoded.rd {
                regs.set_reg(rd as usize, result);
            }
        }
        InstructionCategory::Bx => exec_bx(regs, pipeline, decoded),
        InstructionCategory::Swi => exec_swi(bus, regs, pipeline, decoded.immediate.unwrap_or(0)),
        InstructionCategory::ThumbPushPop => exec_push_pop(regs, pipeline, decoded, bus),
        InstructionCategory::Ldm => exec_ldm_thumb(regs, decoded, bus),
        InstructionCategory::Stm => exec_stm_thumb(regs, decoded, bus),
        InstructionCategory::Ldrsb => exec_ldr_signed(regs, pipeline, decoded, bus, true),
        InstructionCategory::Ldrsh => exec_ldr_signed(regs, pipeline, decoded, bus, false),
        _ => {}
    }
}

fn exec_shift(regs: &mut Registers, decoded: &DecodedInstruction) {
    let mut shift = decoded.shift.unwrap();

    // For register shifts (ALU Format 4): get shift amount from register
    // The value being shifted is Rd (rn field = rd for ALU ops)
    let (value, register_shift_amount) = if shift.shift_reg.is_some() {
        // ALU register shift: value = Rd, amount from Rs (low byte)
        let shift_amount = regs.get_reg(shift.shift_reg.unwrap() as usize) & 0xFF;
        shift.amount = shift_amount as u8;
        (
            decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0),
            Some(shift_amount),
        )
    } else {
        // Immediate shift (Format 1): value from Rs (rn field)
        (
            decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0),
            None,
        )
    };

    // Register-based shifts with amount 0 leave both value and C unchanged.
    let (result, carry) = if register_shift_amount == Some(0) {
        (value, regs.flag_c())
    } else {
        apply_shift(value, &shift, regs.flag_c())
    };

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_flags((result as i32) < 0, result == 0, carry, false);
    }
}

fn exec_add_sub(regs: &mut Registers, decoded: &DecodedInstruction) {
    // SP adjust (ADD/SUB SP, #imm) - rd and rn are both None
    if decoded.rd.is_none() && decoded.rn.is_none() {
        let sp = regs.get_reg(13);
        let imm = decoded.immediate.unwrap_or(0);
        let result = sp.wrapping_add(imm);
        regs.set_reg(13, result);
        return;
    }

    // ADD Rd, SP, #imm (Format 12) - rn is None but rd is set
    if decoded.rn.is_none() {
        let sp = regs.get_reg(13);
        let imm = decoded.immediate.unwrap_or(0);
        let result = sp.wrapping_add(imm);
        if let Some(rd) = decoded.rd {
            regs.set_reg(rd as usize, result);
        }
        return;
    }

    // Should not reach here - Format 2 add/sub now uses Add/Sub categories
    // Keep as fallback for decode_add_sp
}

fn exec_mov(regs: &mut Registers, decoded: &DecodedInstruction) {
    // MOV can be Format 3 (Rd, #imm8) or Format 5 hi-reg (Rd, Rm)
    let value = if let Some(imm) = decoded.immediate {
        imm
    } else if let Some(rn) = decoded.rn {
        regs.get_reg(rn as usize)
    } else {
        0
    };
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, value);
    }
    // Format 3 MOV always sets flags; Format 5 hi-reg MOV does not
    if decoded.immediate.is_some() {
        regs.set_zn_flags(value);
    }
}

fn exec_cmp(regs: &mut Registers, decoded: &DecodedInstruction) {
    // Determine source (always Rd)
    let src = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    // Determine operand2:
    // - ALU CMP (Format 4): rm is set → use Rm
    // - Hi-reg CMP (Format 5): rn has the other register (rm is None)
    // - Format 3 CMP Rd, #imm8: rn is None, rm is None → use immediate
    let operand2 = if let Some(rm) = decoded.rm {
        regs.get_reg(rm as usize)
    } else if let Some(rn) = decoded.rn {
        regs.get_reg(rn as usize)
    } else {
        decoded.immediate.unwrap_or(0)
    };
    let (result, borrow) = src.overflowing_sub(operand2);
    let overflow = (((src as i32) < 0) && ((operand2 as i32) >= 0) && ((result as i32) >= 0))
        || (((src as i32) >= 0) && ((operand2 as i32) < 0) && ((result as i32) < 0));
    regs.set_flags((result as i32) < 0, result == 0, !borrow, overflow);
}

fn exec_add(regs: &mut Registers, decoded: &DecodedInstruction) {
    // Determine source and operand based on format:
    // Format 2 (rn set): ADD Rd, Rs, Rn/Imm3
    // Format 3 (rn None, imm set): ADD Rd, #imm8 (src = Rd)
    // Format 5 hi-reg (rn set, no imm): ADD Rd, Rm
    let (src, operand2) = if let Some(rn) = decoded.rn {
        let s = regs.get_reg(rn as usize);
        let op2 = decoded
            .rm
            .map(|r| regs.get_reg(r as usize))
            .unwrap_or_else(|| {
                decoded.immediate.unwrap_or_else(|| {
                    // Format 5 hi-reg ADD Rd, Rm: op2 = current Rd value (s=Rm above)
                    decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0)
                })
            });
        (s, op2)
    } else {
        // Format 3: src = Rd value, operand = immediate
        let s = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
        (s, decoded.immediate.unwrap_or(0))
    };

    let (result, carry) = src.overflowing_add(operand2);
    let overflow = (((src as i32) >= 0) && ((operand2 as i32) >= 0) && ((result as i32) < 0))
        || (((src as i32) < 0) && ((operand2 as i32) < 0) && ((result as i32) >= 0));
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    // Format 5 hi-reg ADD doesn't set flags (writes_back = false from hi_reg decode for CMP only)
    if decoded.writes_back {
        regs.set_flags((result as i32) < 0, result == 0, carry, overflow);
    }
}

fn exec_sub(regs: &mut Registers, decoded: &DecodedInstruction) {
    let (src, operand2) = if let Some(rn) = decoded.rn {
        let s = regs.get_reg(rn as usize);
        let op2 = decoded
            .rm
            .map(|r| regs.get_reg(r as usize))
            .unwrap_or_else(|| decoded.immediate.unwrap_or(0));
        (s, op2)
    } else if let Some(rm) = decoded.rm {
        // NEG: Rd = 0 - Rs (rn is None, rm has source register)
        (0u32, regs.get_reg(rm as usize))
    } else {
        // Format 3 SUB Rd, #imm8
        let s = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
        (s, decoded.immediate.unwrap_or(0))
    };

    let (result, borrow) = src.overflowing_sub(operand2);
    let overflow = (((src as i32) < 0) && ((operand2 as i32) >= 0) && ((result as i32) >= 0))
        || (((src as i32) >= 0) && ((operand2 as i32) < 0) && ((result as i32) < 0));
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_flags((result as i32) < 0, result == 0, !borrow, overflow);
    }
}

fn exec_alu(regs: &mut Registers, decoded: &DecodedInstruction, op: fn(u32, u32) -> u32) {
    let rn = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let rm = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let result = op(rn, rm);
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    if decoded.writes_back {
        regs.set_zn_flags(result);
    }
}

fn exec_adc(regs: &mut Registers, decoded: &DecodedInstruction) {
    let rd_val = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let carry_in = if regs.flag_c() { 1u32 } else { 0 };
    let (tmp, c1) = rd_val.overflowing_add(rm_val);
    let (result, c2) = tmp.overflowing_add(carry_in);
    let carry = c1 || c2;
    let overflow = (((rd_val as i32) >= 0) && ((rm_val as i32) >= 0) && ((result as i32) < 0))
        || (((rd_val as i32) < 0) && ((rm_val as i32) < 0) && ((result as i32) >= 0));
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    regs.set_flags((result as i32) < 0, result == 0, carry, overflow);
}

fn exec_sbc(regs: &mut Registers, decoded: &DecodedInstruction) {
    let rd_val = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let rm_val = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let borrow_in = if regs.flag_c() { 0u32 } else { 1 };
    let (tmp, b1) = rd_val.overflowing_sub(rm_val);
    let (result, b2) = tmp.overflowing_sub(borrow_in);
    let borrow = b1 || b2;
    let overflow = (((rd_val as i32) < 0) && ((rm_val as i32) >= 0) && ((result as i32) >= 0))
        || (((rd_val as i32) >= 0) && ((rm_val as i32) < 0) && ((result as i32) < 0));
    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, result);
    }
    regs.set_flags((result as i32) < 0, result == 0, !borrow, overflow);
}

fn exec_ldr<B: Bus>(
    regs: &mut Registers,
    pipeline: &Pipeline,
    decoded: &DecodedInstruction,
    bus: &mut B,
    is_byte: bool,
    is_half: bool,
) {
    let addr = if decoded.rn.is_none() {
        // PC-relative LDR: addr = (PC & ~2) + imm
        let pc = pipeline.pc() & !2;
        pc.wrapping_add(decoded.immediate.unwrap_or(0))
    } else {
        let base = regs.get_reg(decoded.rn.unwrap() as usize);
        let offset = decoded
            .rm
            .map(|r| regs.get_reg(r as usize))
            .unwrap_or(decoded.immediate.unwrap_or(0));
        base.wrapping_add(offset)
    };

    let value = if is_byte {
        bus.read_u8(addr) as u32
    } else if is_half {
        armv4_load_halfword(bus, addr) as u32
    } else {
        armv4_load_word(bus, addr)
    };

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, value);
    }
}

fn exec_str<B: Bus>(
    regs: &mut Registers,
    decoded: &DecodedInstruction,
    bus: &mut B,
    is_byte: bool,
    is_half: bool,
) {
    let base = regs.get_reg(decoded.rn.unwrap() as usize);
    let offset = decoded
        .rm
        .map(|r| regs.get_reg(r as usize))
        .unwrap_or(decoded.immediate.unwrap_or(0));
    let addr = base.wrapping_add(offset);
    let value = decoded.rd.map(|r| regs.get_reg(r as usize)).unwrap_or(0);

    if is_byte {
        bus.write_u8(addr, value as u8);
    } else if is_half {
        bus.write_u16(addr, value as u16);
    } else {
        bus.write_u32(addr, value);
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
        regs.set_lr(pc & !1);
    }

    // Stay in Thumb mode for Thumb branches (conditional/unconditional)
    pipeline.branch_with_mode(target & !1, true);
}

fn exec_bx(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let rm = decoded.rn.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let thumb = (rm & 1) != 0;
    pipeline.branch_with_mode(rm & !1, thumb);
    regs.set_thumb_mode(thumb);
}

/// BL suffix: target = LR + offset, LR = return_addr | 1
fn exec_bl_suffix(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let lr = regs.get_reg(14);
    let offset = decoded.immediate.unwrap_or(0);
    let target = lr.wrapping_add(offset);
    let return_addr = pipeline.fetch_addr.wrapping_add(2); // Next instruction after this one
    regs.set_lr(return_addr | 1); // Set bit 0 to indicate Thumb return
    pipeline.branch_with_mode(target & !1, true); // Stay in Thumb
}

/// BLX suffix: target = LR + offset, switch to ARM
fn exec_blx_suffix(regs: &mut Registers, pipeline: &mut Pipeline, decoded: &DecodedInstruction) {
    let lr = regs.get_reg(14);
    let offset = decoded.immediate.unwrap_or(0);
    let target = lr.wrapping_add(offset);
    let return_addr = pipeline.fetch_addr.wrapping_add(2);
    regs.set_lr(return_addr | 1);
    pipeline.branch_with_mode(target & !3, false); // Switch to ARM (align to 4)
    regs.set_thumb_mode(false);
}

fn exec_swi<B: Bus>(bus: &mut B, regs: &mut Registers, pipeline: &mut Pipeline, swi_num: u32) {
    // In Thumb mode, the SWI number is in bits 0-7
    let swi_comment = (swi_num & 0xFF) as u8;
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
    let has_real_bios = bus.read_u32(0x0000_0000) != 0xE12F_FF1E;
    if has_real_bios && !Bios::should_hle_with_real_bios(swi_comment) {
        let lr_offset = if regs.is_thumb_mode() { 2 } else { 4 };
        regs.enter_exception(CpuMode::Supervisor, lr_offset);
        pipeline.branch_with_mode(0x0000_0008, false);
        pipeline.flush();
    } else {
        // Handle SWI via HLE (High-Level Emulation)
        Bios::handle_swi(swi_comment, regs, bus);
    }
}

fn exec_push_pop<B: Bus>(
    regs: &mut Registers,
    pipeline: &mut Pipeline,
    decoded: &DecodedInstruction,
    bus: &mut B,
) {
    let rlist = decoded.immediate.unwrap_or(0) as u8;
    let flags = decoded.branch_target.unwrap_or(0);
    let is_pop = (flags & 1) != 0;
    let r_bit = (flags & 2) != 0;

    if is_pop {
        // POP {Rlist} or POP {Rlist, PC}
        let mut addr = regs.get_reg(13);

        for i in 0..8u8 {
            if (rlist & (1 << i)) != 0 {
                let value = bus.read_u32(addr);
                regs.set_reg(i as usize, value);
                addr = addr.wrapping_add(4);
            }
        }

        if r_bit {
            // Pop PC
            let pc_val = bus.read_u32(addr);
            addr = addr.wrapping_add(4);
            let thumb = (pc_val & 1) != 0;
            pipeline.branch_with_mode(pc_val & !1, thumb);
            regs.set_thumb_mode(thumb);
        }

        regs.set_reg(13, addr);
    } else {
        // PUSH {Rlist} or PUSH {Rlist, LR}
        // Count registers to push
        let mut count = 0u32;
        for i in 0..8u8 {
            if (rlist & (1 << i)) != 0 {
                count += 1;
            }
        }
        if r_bit {
            count += 1;
        }

        let mut sp = regs.get_reg(13);
        sp = sp.wrapping_sub(count * 4);
        let base = sp;
        let mut addr = base;

        // Push R0-R7 in order
        for i in 0..8u8 {
            if (rlist & (1 << i)) != 0 {
                let value = regs.get_reg(i as usize);
                bus.write_u32(addr, value);
                addr = addr.wrapping_add(4);
            }
        }

        if r_bit {
            // Push LR
            let lr = regs.get_reg(14);
            bus.write_u32(addr, lr);
        }

        regs.set_reg(13, base);
    }
}

fn exec_ldm_thumb<B: Bus>(regs: &mut Registers, decoded: &DecodedInstruction, bus: &mut B) {
    let rn = decoded.rd.unwrap_or(0);
    let base = regs.get_reg(rn as usize);
    let rlist = decoded.immediate.unwrap_or(0) as u16;
    let mut addr = base;

    for i in 0..8 {
        if (rlist & (1 << i)) != 0 {
            let value = bus.read_u32(addr);
            regs.set_reg(i as usize, value);
            addr = addr.wrapping_add(4);
        }
    }

    if decoded.writes_back {
        regs.set_reg(rn as usize, addr);
    }
}

fn exec_stm_thumb<B: Bus>(regs: &mut Registers, decoded: &DecodedInstruction, bus: &mut B) {
    let rn = decoded.rd.unwrap_or(0);
    let base = regs.get_reg(rn as usize);
    let rlist = decoded.immediate.unwrap_or(0) as u16;
    let mut addr = base;

    for i in 0..8 {
        if (rlist & (1 << i)) != 0 {
            let value = regs.get_reg(i as usize);
            bus.write_u32(addr, value);
            addr = addr.wrapping_add(4);
        }
    }

    if decoded.writes_back {
        regs.set_reg(rn as usize, addr);
    }
}

fn exec_ldr_signed<B: Bus>(
    regs: &mut Registers,
    _pipeline: &Pipeline,
    decoded: &DecodedInstruction,
    bus: &mut B,
    is_byte: bool,
) {
    let base = regs.get_reg(decoded.rn.unwrap() as usize);
    let offset = decoded.rm.map(|r| regs.get_reg(r as usize)).unwrap_or(0);
    let addr = base.wrapping_add(offset);

    let value = if is_byte {
        let byte = bus.read_u8(addr) as i8;
        byte as i32 as u32
    } else {
        armv4_load_signed_halfword(bus, addr)
    };

    if let Some(rd) = decoded.rd {
        regs.set_reg(rd as usize, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SimpleBus;

    fn run_thumb(opcode: u16, regs: &mut Registers) {
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();
        pipeline.set_thumb_mode(true);
        regs.set_thumb_mode(true);
        let mut instr = Instruction::thumb(opcode);
        decode_thumb(&mut instr);
        execute_thumb(&instr, &mut bus, regs, &mut pipeline);
    }

    fn run_thumb_with_bus(
        opcode: u16,
        regs: &mut Registers,
        bus: &mut SimpleBus,
        pipeline: &mut Pipeline,
    ) {
        pipeline.set_thumb_mode(true);
        regs.set_thumb_mode(true);
        let mut instr = Instruction::thumb(opcode);
        decode_thumb(&mut instr);
        execute_thumb(&instr, bus, regs, pipeline);
    }

    #[test]
    fn test_decode_mov() {
        let mut instr = Instruction::thumb(0x2000);
        decode_thumb(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Mov);
        assert_eq!(decoded.rd, Some(0));
        assert_eq!(decoded.immediate, Some(0));
    }

    #[test]
    fn test_decode_add() {
        let mut instr = Instruction::thumb(0x1C00);
        decode_thumb(&mut instr);
        let decoded = instr.decoded.unwrap();
        assert_eq!(decoded.category, InstructionCategory::Add);
    }

    #[test]
    fn test_thumb_register_shift_zero_keeps_value_and_carry() {
        // LSR R0, R1 (ALU format, shift by register)
        let mut regs = Registers::new();
        regs.set_reg(0, 0x1234_5678);
        regs.set_reg(1, 0); // shift amount 0 => no shift, C unchanged
        regs.set_flags(false, false, true, false);

        run_thumb(0x40C8, &mut regs);

        assert_eq!(regs.get_reg(0), 0x1234_5678);
        assert!(regs.flag_c());
    }

    #[test]
    fn test_thumb_ldr_rotates_misaligned_word() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u32(0x0200_0000, 0x0062_A4C3);
        regs.set_reg(1, 0x0200_0001);

        // LDR R0, [R1, #0]
        run_thumb_with_bus(0x6808, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0xC300_62A4);
    }

    #[test]
    fn test_thumb_ldrh_rotates_on_odd_address() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u16(0x0200_0000, 0x1234);
        regs.set_reg(1, 0x0200_0001);

        // LDRH R0, [R1, #0]
        run_thumb_with_bus(0x8808, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0x3412);
    }

    #[test]
    fn test_thumb_ldrsh_odd_address_acts_like_ldrsb() {
        let mut regs = Registers::new();
        let mut bus = SimpleBus::new(None);
        let mut pipeline = Pipeline::new();

        bus.write_u8(0x0200_0001, 0x80);
        regs.set_reg(1, 0x0200_0001);
        regs.set_reg(2, 0);

        // LDRSH R0, [R1, R2]
        run_thumb_with_bus(0x5E88, &mut regs, &mut bus, &mut pipeline);

        assert_eq!(regs.get_reg(0), 0xFFFF_FF80);
    }
}

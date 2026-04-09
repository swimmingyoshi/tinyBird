//! BIOS High-Level Emulation (HLE)
//!
//! Implements the most critical GBA BIOS SWI (software interrupt) functions via
//! high-level emulation rather than executing the actual BIOS ROM. This allows
//! the emulator to run games without requiring a real BIOS dump.
//!
//! SWI functions are invoked by the SWI instruction. The function number comes
//! from bits 0-7 of the comment field in Thumb mode, or bits 16-23 in ARM mode.

use crate::bus::Bus;
use crate::bus::DEBUG_CYCLE;
use crate::cpu::registers::{R0, R1, R13, R14, R15, R2, R3};
use crate::cpu::Registers;
use crate::debug::config as debug_config;

/// GBA BIOS HLE implementation.
///
/// Provides static methods that emulate individual SWI calls by directly
/// manipulating CPU registers and bus memory.
pub struct Bios;

impl Bios {
    /// Some BIOS calls are pure data-manipulation helpers and are safer to
    /// satisfy via HLE even when a real BIOS is present.
    pub fn should_hle_with_real_bios(comment: u8) -> bool {
        matches!(comment, 0x11 | 0x12 | 0x14 | 0x15)
    }

    /// Dispatch a SWI call based on the comment byte.
    ///
    /// The `comment` parameter is the SWI function number (0x00-0xFF).
    /// Unimplemented SWIs are silently ignored.
    pub fn handle_swi<B: Bus>(comment: u8, regs: &mut Registers, bus: &mut B) {
        if debug_config().swi_debug {
            let cycle = DEBUG_CYCLE.with(|c| c.get());
            eprintln!(
                "[SWI] cy={} {:02x} r0={:08x} r1={:08x} r2={:08x} r3={:08x} sp={:08x} lr={:08x}",
                cycle,
                comment,
                regs.get_reg(R0),
                regs.get_reg(R1),
                regs.get_reg(R2),
                regs.get_reg(R3),
                regs.get_reg(R13),
                regs.get_reg(R14)
            );
        }
        match comment {
            0x00 => Self::soft_reset(regs, bus),
            0x01 => Self::register_ram_reset(regs, bus),
            0x02 => Self::halt(regs, bus),
            0x04 => Self::intr_wait(regs, bus),
            0x05 => Self::vblank_intr_wait(regs, bus),
            0x06 => Self::div(regs),
            0x07 => Self::div_arm(regs),
            0x08 => Self::sqrt(regs),
            0x09 => Self::arctan(regs),
            0x0A => Self::arctan2(regs),
            0x0B => Self::cpu_set(regs, bus),
            0x0C => Self::cpu_fast_set(regs, bus),
            0x0D => Self::get_bios_checksum(regs),
            0x0E => Self::bg_affine_set(regs, bus),
            0x0F => Self::obj_affine_set(regs, bus),
            0x11 => Self::lz77_uncomp_wram(regs, bus),
            0x12 => Self::lz77_uncomp_vram(regs, bus),
            0x14 => Self::rl_uncomp_wram(regs, bus),
            0x15 => Self::rl_uncomp_vram(regs, bus),
            _ => {
                if debug_config().swi_debug {
                    eprintln!("[SWI] unimplemented {:02x}", comment);
                }
            } // Unimplemented SWI - silently ignore
        }
    }

    /// SWI 0x00 - SoftReset
    ///
    /// Resets the CPU to the start of the ROM. Clears registers and sets the
    /// stack pointers to their default values. Jumps to 0x08000000 (ROM start)
    /// or 0x02000000 depending on the reset flag at 0x03007FFA.
    fn soft_reset<B: Bus>(regs: &mut Registers, bus: &B) {
        let flag = bus.read_u8(0x03007FFA);

        // Clear registers R0-R12
        for i in R0..=12 {
            regs.set_reg(i, 0);
        }

        // Set stack pointers to default values
        regs.set_reg(R13, 0x03007F00); // SP_svc
        regs.set_reg(R14, 0);

        // Jump to ROM start or EWRAM based on flag
        let dest = if flag != 0 { 0x02000000 } else { 0x08000000 };
        regs.set_reg(R15, dest);
    }

    /// SWI 0x01 - RegisterRamReset
    ///
    /// Resets various memory regions and I/O registers based on flags in R0.
    /// This is a simplified stub that clears EWRAM and IWRAM.
    fn register_ram_reset<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let flags = regs.get_reg(R0);

        // Bit 0: Clear 256K EWRAM (0x02000000-0x0203FFFF)
        if flags & (1 << 0) != 0 {
            for addr in (0x0200_0000..0x0204_0000).step_by(4) {
                bus.write_u32(addr, 0);
            }
        }

        // Bit 1: Clear 32K IWRAM (except last 0x200 bytes for stack)
        if flags & (1 << 1) != 0 {
            for addr in (0x0300_0000..0x0300_7E00).step_by(4) {
                bus.write_u32(addr, 0);
            }
        }

        // Bits 2-7 control other regions (palette, VRAM, OAM, IO, etc.)
        // Simplified: clear those regions if requested
        if flags & (1 << 2) != 0 {
            for addr in (0x0500_0000..0x0500_0400).step_by(4) {
                bus.write_u32(addr, 0);
            }
        }

        if flags & (1 << 3) != 0 {
            for addr in (0x0600_0000..0x0601_8000).step_by(4) {
                bus.write_u32(addr, 0);
            }
        }

        if flags & (1 << 4) != 0 {
            for addr in (0x0700_0000..0x0700_0400).step_by(4) {
                bus.write_u32(addr, 0);
            }
        }
    }

    /// SWI 0x02 - Halt
    ///
    /// Puts the CPU into low-power halt state until an interrupt occurs.
    /// Signals halt by writing to HALTCNT (I/O offset 0x301). The main
    /// emulation loop checks this register and sets cpu.halted accordingly.
    fn halt<B: Bus>(regs: &mut Registers, bus: &mut B) {
        // Writing 0x00 to HALTCNT signals HALT mode.
        // The main loop detects this and freezes the CPU until (IE & IF) != 0.
        bus.write_u8(0x04000301, 0x00);
        let _ = regs;
    }

    /// SWI 0x04 - IntrWait
    ///
    /// Waits until one of the interrupt flags in R1 becomes pending.
    /// - R0: discard old flags (non-zero clears existing matching flags first)
    /// - R1: interrupt flag mask to wait for
    ///
    /// HLE behavior:
    /// - Optionally clear matching IF bits first
    /// - If not pending, request HALT so CPU sleeps until an interrupt occurs
    fn intr_wait<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let discard_old = regs.get_reg(R0) != 0;
        let mask = (regs.get_reg(R1) & 0x3FFF) as u16;

        if mask == 0 {
            return;
        }

        if discard_old {
            // IF is write-one-to-clear.
            bus.write_u16(0x04000202, mask);
            let irq_flags = bus.read_u16(0x03007FF8);
            bus.write_u16(0x03007FF8, irq_flags & !mask);
        }

        let if_reg = bus.read_u16(0x04000202);
        if (if_reg & mask) == 0 {
            // Sleep until an enabled interrupt wakes the CPU.
            bus.write_u8(0x04000301, 0x00);
        } else {
            // Matching interrupt is already pending; consume it.
            bus.write_u16(0x04000202, mask);
            let irq_flags = bus.read_u16(0x03007FF8);
            bus.write_u16(0x03007FF8, irq_flags & !mask);
        }
    }

    /// SWI 0x05 - VBlankIntrWait
    ///
    /// Waits for the next VBlank interrupt. Equivalent to setting R0=1, R1=1
    /// then calling IntrWait (SWI 0x04).
    fn vblank_intr_wait<B: Bus>(regs: &mut Registers, bus: &mut B) {
        // Set R0=1 (discard old flags), R1=1 (VBlank flag), then IntrWait.
        regs.set_reg(R0, 1);
        regs.set_reg(R1, 1);
        Self::intr_wait(regs, bus);
    }

    /// SWI 0x06 - Div
    ///
    /// Signed integer division.
    /// - Input: R0 = numerator, R1 = denominator
    /// - Output: R0 = quotient, R1 = remainder, R3 = abs(quotient)
    ///
    /// If the denominator is zero, the result is undefined (we return 0).
    fn div(regs: &mut Registers) {
        let numerator = regs.get_reg(R0) as i32;
        let denominator = regs.get_reg(R1) as i32;

        if denominator == 0 {
            // Division by zero: GBA BIOS behavior is undefined.
            // We match common emulator behavior and return 0.
            regs.set_reg(R0, 0);
            regs.set_reg(R1, 0);
            regs.set_reg(R3, 0);
            return;
        }

        let quotient = numerator.wrapping_div(denominator);
        let remainder = numerator.wrapping_rem(denominator);

        regs.set_reg(R0, quotient as u32);
        regs.set_reg(R1, remainder as u32);
        regs.set_reg(R3, quotient.unsigned_abs());
    }

    /// SWI 0x07 - DivArm
    ///
    /// Same as Div but with swapped arguments.
    /// - Input: R0 = denominator, R1 = numerator
    /// - Output: R0 = quotient, R1 = remainder, R3 = abs(quotient)
    fn div_arm(regs: &mut Registers) {
        // Swap R0 and R1, then call Div
        let r0 = regs.get_reg(R0);
        let r1 = regs.get_reg(R1);
        regs.set_reg(R0, r1);
        regs.set_reg(R1, r0);
        Self::div(regs);
    }

    /// SWI 0x08 - Sqrt
    ///
    /// Integer square root.
    /// - Input: R0 = 32-bit unsigned value
    /// - Output: R0 = floor(sqrt(R0))
    fn sqrt(regs: &mut Registers) {
        let value = regs.get_reg(R0);
        let result = (value as f64).sqrt() as u32;
        regs.set_reg(R0, result);
    }

    /// SWI 0x09 - ArcTan
    ///
    /// Calculates the arctangent. Stub implementation.
    fn arctan(regs: &mut Registers) {
        #[cfg(debug_assertions)]
        eprintln!("[BIOS] ArcTan (SWI 0x09) not fully implemented");

        // Basic stub: input R0 = tan value (fixed point), output R0 = angle
        let _ = regs;
    }

    /// SWI 0x0A - ArcTan2
    ///
    /// Calculates the full-circle arctangent. Stub implementation.
    fn arctan2(regs: &mut Registers) {
        #[cfg(debug_assertions)]
        eprintln!("[BIOS] ArcTan2 (SWI 0x0A) not fully implemented");

        let _ = regs;
    }

    /// SWI 0x0B - CpuSet
    ///
    /// Memory copy/fill in 16-bit or 32-bit units.
    /// - R0 = source address
    /// - R1 = destination address
    /// - R2 = length and mode:
    ///   - Bits 0-20: transfer count (number of halfwords or words)
    ///   - Bit 24: size (0 = 16-bit, 1 = 32-bit)
    ///   - Bit 26: mode (0 = copy, 1 = fill)
    fn cpu_set<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let control = regs.get_reg(R2);

        let count = control & 0x001F_FFFF;
        let is_32bit = control & (1 << 24) != 0;
        let is_fill = control & (1 << 26) != 0;

        if is_32bit {
            if is_fill {
                let fill_value = bus.read_u32(src);
                for i in 0..count {
                    bus.write_u32(dst.wrapping_add(i * 4), fill_value);
                }
            } else {
                for i in 0..count {
                    let value = bus.read_u32(src.wrapping_add(i * 4));
                    bus.write_u32(dst.wrapping_add(i * 4), value);
                }
            }
        } else {
            if is_fill {
                let fill_value = bus.read_u16(src);
                for i in 0..count {
                    bus.write_u16(dst.wrapping_add(i * 2), fill_value);
                }
            } else {
                for i in 0..count {
                    let value = bus.read_u16(src.wrapping_add(i * 2));
                    bus.write_u16(dst.wrapping_add(i * 2), value);
                }
            }
        }
    }

    /// SWI 0x0C - CpuFastSet
    ///
    /// Fast memory copy/fill in 32-bit units, count rounded up to 8-word blocks.
    /// - R0 = source address
    /// - R1 = destination address
    /// - R2 = length and mode:
    ///   - Bits 0-20: word count (rounded up to multiple of 8)
    ///   - Bit 26: mode (0 = copy, 1 = fill)
    fn cpu_fast_set<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let control = regs.get_reg(R2);

        let count = control & 0x001F_FFFF;
        // Round up to multiple of 8
        let count = (count + 7) & !7;
        let is_fill = control & (1 << 26) != 0;

        if is_fill {
            let fill_value = bus.read_u32(src);
            for i in 0..count {
                bus.write_u32(dst.wrapping_add(i * 4), fill_value);
            }
        } else {
            for i in 0..count {
                let value = bus.read_u32(src.wrapping_add(i * 4));
                bus.write_u32(dst.wrapping_add(i * 4), value);
            }
        }
    }

    /// SWI 0x0D - GetBiosChecksum
    ///
    /// Returns the hardcoded BIOS checksum value in R0.
    /// Always returns 0xBAAE187F for a valid GBA BIOS.
    fn get_bios_checksum(regs: &mut Registers) {
        regs.set_reg(R0, 0xBAAE187F);
    }

    /// SWI 0x0E - BgAffineSet
    ///
    /// Calculates background affine transformation parameters. Stub implementation.
    fn bg_affine_set<B: Bus>(regs: &mut Registers, bus: &mut B) {
        #[cfg(debug_assertions)]
        eprintln!("[BIOS] BgAffineSet (SWI 0x0E) not fully implemented");

        let _ = (regs, bus);
    }

    /// SWI 0x0F - ObjAffineSet
    ///
    /// Calculates sprite affine transformation parameters. Stub implementation.
    fn obj_affine_set<B: Bus>(regs: &mut Registers, bus: &mut B) {
        #[cfg(debug_assertions)]
        eprintln!("[BIOS] ObjAffineSet (SWI 0x0F) not fully implemented");

        let _ = (regs, bus);
    }

    /// SWI 0x11 - LZ77UnCompWram (write to WRAM)
    ///
    /// Decompresses LZ77-compressed data.
    /// - R0 = source address (compressed data with header)
    /// - R1 = destination address
    ///
    /// Header format (first 4 bytes at source):
    /// - Bits 4-31: decompressed size
    /// - Bits 0-3: reserved (should be 0x01 for LZ77)
    fn lz77_uncomp_wram<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let data = Self::lz77_expand(bus, src);
        Self::write_bytes(bus, dst, &data);
    }

    /// SWI 0x12 - LZ77UnCompVram (write to VRAM, 16-bit aligned)
    ///
    /// Same as LZ77UnCompWram but output is written as 16-bit values for
    /// VRAM compatibility.
    fn lz77_uncomp_vram<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let data = Self::lz77_expand(bus, src);
        Self::write_halfword_packed(bus, dst, &data);
    }

    /// Expand LZ77-compressed data into a byte buffer.
    fn lz77_expand<B: Bus>(bus: &mut B, src: u32) -> Vec<u8> {
        // Read header
        let header = bus.read_u32(src);
        let decompressed_size = header >> 8;
        let mut output = Vec::with_capacity(decompressed_size as usize);

        let mut src_pos = src + 4;
        while output.len() < decompressed_size as usize {
            let flags = bus.read_u8(src_pos);
            src_pos += 1;

            for bit in (0..8).rev() {
                if output.len() >= decompressed_size as usize {
                    break;
                }

                if flags & (1 << bit) != 0 {
                    let b1 = bus.read_u8(src_pos) as usize;
                    let b2 = bus.read_u8(src_pos + 1) as usize;
                    src_pos += 2;

                    let length = (b1 >> 4) + 3;
                    let offset = (((b1 & 0x0F) << 8) | b2) + 1;
                    let ref_start = output.len().saturating_sub(offset);

                    for j in 0..length {
                        if output.len() >= decompressed_size as usize {
                            break;
                        }
                        let byte = output[ref_start + j];
                        output.push(byte);
                    }
                } else {
                    let byte = bus.read_u8(src_pos);
                    src_pos += 1;
                    output.push(byte);
                }
            }
        }

        output
    }

    /// SWI 0x14 - RLUnCompWram (write to WRAM)
    ///
    /// Decompresses run-length encoded data.
    /// - R0 = source address (RLE data with header)
    /// - R1 = destination address
    ///
    /// Header format (first 4 bytes at source):
    /// - Bits 4-31: decompressed size
    /// - Bits 0-3: reserved (should be 0x03 for RLE)
    fn rl_uncomp_wram<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let data = Self::rl_expand(bus, src);
        Self::write_bytes(bus, dst, &data);
    }

    /// SWI 0x15 - RLUnCompVram (write to VRAM, 16-bit aligned)
    ///
    /// Same as RLUnCompWram but writes are 16-bit aligned for VRAM compatibility.
    fn rl_uncomp_vram<B: Bus>(regs: &mut Registers, bus: &mut B) {
        let src = regs.get_reg(R0);
        let dst = regs.get_reg(R1);
        let data = Self::rl_expand(bus, src);
        Self::write_halfword_packed(bus, dst, &data);
    }

    /// Expand run-length encoded data into a byte buffer.
    fn rl_expand<B: Bus>(bus: &mut B, src: u32) -> Vec<u8> {
        // Read header
        let header = bus.read_u32(src);
        let decompressed_size = header >> 8;
        let mut output = Vec::with_capacity(decompressed_size as usize);

        let mut src_pos = src + 4;

        while output.len() < decompressed_size as usize {
            let flag_byte = bus.read_u8(src_pos);
            src_pos += 1;

            if flag_byte & 0x80 != 0 {
                let run_length = ((flag_byte & 0x7F) + 3) as usize;
                let data = bus.read_u8(src_pos);
                src_pos += 1;

                for _ in 0..run_length {
                    if output.len() >= decompressed_size as usize {
                        break;
                    }
                    output.push(data);
                }
            } else {
                let run_length = ((flag_byte & 0x7F) + 1) as usize;

                for _ in 0..run_length {
                    if output.len() >= decompressed_size as usize {
                        break;
                    }
                    let data = bus.read_u8(src_pos);
                    src_pos += 1;
                    output.push(data);
                }
            }
        }

        output
    }

    fn write_bytes<B: Bus>(bus: &mut B, dst: u32, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            bus.write_u8(dst.wrapping_add(i as u32), byte);
        }
    }

    fn write_halfword_packed<B: Bus>(bus: &mut B, dst: u32, data: &[u8]) {
        let mut addr = dst;
        let mut chunks = data.chunks_exact(2);
        for pair in &mut chunks {
            bus.write_u16(addr, u16::from_le_bytes([pair[0], pair[1]]));
            addr = addr.wrapping_add(2);
        }
        let rem = chunks.remainder();
        if let [last] = rem {
            bus.write_u16(addr, u16::from_le_bytes([*last, 0]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SimpleBus;

    /// Helper to create registers and bus for testing.
    fn setup() -> (Registers, SimpleBus) {
        (Registers::new(), SimpleBus::new(None))
    }

    #[test]
    fn test_div_positive() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 10);
        regs.set_reg(R1, 3);
        Bios::handle_swi(0x06, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0) as i32, 3); // quotient
        assert_eq!(regs.get_reg(R1) as i32, 1); // remainder
        assert_eq!(regs.get_reg(R3), 3); // abs(quotient)
    }

    #[test]
    fn test_div_negative() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, (-10i32) as u32);
        regs.set_reg(R1, 3);
        Bios::handle_swi(0x06, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0) as i32, -3); // quotient
        assert_eq!(regs.get_reg(R1) as i32, -1); // remainder
        assert_eq!(regs.get_reg(R3), 3); // abs(quotient)
    }

    #[test]
    fn test_div_by_zero() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 42);
        regs.set_reg(R1, 0);
        Bios::handle_swi(0x06, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 0);
        assert_eq!(regs.get_reg(R1), 0);
        assert_eq!(regs.get_reg(R3), 0);
    }

    #[test]
    fn test_div_arm_swapped_args() {
        let (mut regs, mut bus) = setup();
        // DivArm: R0=denominator, R1=numerator
        regs.set_reg(R0, 3);
        regs.set_reg(R1, 10);
        Bios::handle_swi(0x07, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0) as i32, 3); // 10/3
        assert_eq!(regs.get_reg(R1) as i32, 1); // 10%3
        assert_eq!(regs.get_reg(R3), 3);
    }

    #[test]
    fn test_sqrt() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 144);
        Bios::handle_swi(0x08, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 12);
    }

    #[test]
    fn test_sqrt_non_perfect() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 10);
        Bios::handle_swi(0x08, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 3); // floor(sqrt(10))
    }

    #[test]
    fn test_sqrt_zero() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 0);
        Bios::handle_swi(0x08, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 0);
    }

    #[test]
    fn test_get_bios_checksum() {
        let (mut regs, mut bus) = setup();
        Bios::handle_swi(0x0D, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 0xBAAE187F);
    }

    #[test]
    fn test_cpu_set_copy_32bit() {
        let (mut regs, mut bus) = setup();

        // Write source data to EWRAM
        let src = 0x0200_0000;
        let dst = 0x0200_1000;
        bus.write_u32(src, 0xDEADBEEF);
        bus.write_u32(src + 4, 0xCAFEBABE);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);
        // Count=2, 32-bit mode, copy mode
        regs.set_reg(R2, 2 | (1 << 24));

        Bios::handle_swi(0x0B, &mut regs, &mut bus);

        assert_eq!(bus.read_u32(dst), 0xDEADBEEF);
        assert_eq!(bus.read_u32(dst + 4), 0xCAFEBABE);
    }

    #[test]
    fn test_cpu_set_fill_32bit() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;
        bus.write_u32(src, 0x42424242);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);
        // Count=4, 32-bit, fill mode
        regs.set_reg(R2, 4 | (1 << 24) | (1 << 26));

        Bios::handle_swi(0x0B, &mut regs, &mut bus);

        for i in 0..4 {
            assert_eq!(bus.read_u32(dst + i * 4), 0x42424242);
        }
    }

    #[test]
    fn test_cpu_set_copy_16bit() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;
        bus.write_u16(src, 0x1234);
        bus.write_u16(src + 2, 0x5678);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);
        // Count=2, 16-bit (bit 24 = 0), copy mode
        regs.set_reg(R2, 2);

        Bios::handle_swi(0x0B, &mut regs, &mut bus);

        assert_eq!(bus.read_u16(dst), 0x1234);
        assert_eq!(bus.read_u16(dst + 2), 0x5678);
    }

    #[test]
    fn test_cpu_fast_set_copy() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;
        for i in 0..8u32 {
            bus.write_u32(src + i * 4, i + 1);
        }

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);
        // Count=5 (should round up to 8)
        regs.set_reg(R2, 5);

        Bios::handle_swi(0x0C, &mut regs, &mut bus);

        // All 8 words should be copied (rounded up)
        for i in 0..8u32 {
            assert_eq!(bus.read_u32(dst + i * 4), i + 1);
        }
    }

    #[test]
    fn test_cpu_fast_set_fill() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;
        bus.write_u32(src, 0xAAAAAAAA);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);
        // Count=3 (rounds to 8), fill mode
        regs.set_reg(R2, 3 | (1 << 26));

        Bios::handle_swi(0x0C, &mut regs, &mut bus);

        for i in 0..8u32 {
            assert_eq!(bus.read_u32(dst + i * 4), 0xAAAAAAAA);
        }
    }

    #[test]
    fn test_lz77_decompress() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;

        // Build a simple LZ77 stream:
        // Header: type 0x10 (LZ77), decompressed size = 5
        // Encoding: 5 << 8 | 0x10
        let header: u32 = (5 << 8) | 0x10;
        bus.write_u32(src, header);

        // Flag byte: all 5 bits uncompressed (0b00000000)
        bus.write_u8(src + 4, 0x00);
        // 5 literal bytes (only first 5 of 8 bits used, but we have the size check)
        bus.write_u8(src + 5, 0x41); // 'A'
        bus.write_u8(src + 6, 0x42); // 'B'
        bus.write_u8(src + 7, 0x43); // 'C'
        bus.write_u8(src + 8, 0x44); // 'D'
        bus.write_u8(src + 9, 0x45); // 'E'

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x11, &mut regs, &mut bus);

        assert_eq!(bus.read_u8(dst), 0x41);
        assert_eq!(bus.read_u8(dst + 1), 0x42);
        assert_eq!(bus.read_u8(dst + 2), 0x43);
        assert_eq!(bus.read_u8(dst + 3), 0x44);
        assert_eq!(bus.read_u8(dst + 4), 0x45);
    }

    #[test]
    fn test_lz77_decompress_vram_packs_halfwords() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0600_0000;

        let header: u32 = (5 << 8) | 0x10;
        bus.write_u32(src, header);
        bus.write_u8(src + 4, 0x00);
        bus.write_u8(src + 5, 0x41);
        bus.write_u8(src + 6, 0x42);
        bus.write_u8(src + 7, 0x43);
        bus.write_u8(src + 8, 0x44);
        bus.write_u8(src + 9, 0x45);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x12, &mut regs, &mut bus);

        assert_eq!(bus.read_u16(dst), 0x4241);
        assert_eq!(bus.read_u16(dst + 2), 0x4443);
        assert_eq!(bus.read_u16(dst + 4), 0x0045);
    }

    #[test]
    fn test_lz77_compressed_reference() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;

        // Decompressed size = 6
        let header: u32 = (6 << 8) | 0x10;
        bus.write_u32(src, header);

        // Flag byte: bit 7 = 0 (uncompressed), bits 6..4 = 0 (uncompressed),
        // bit 2 = 1 (compressed reference for the 6th group position)
        // We want: 3 uncompressed bytes, then 1 compressed reference
        // flag = 0b00010000 => bit 4 set means 4th position (from MSB: positions 7,6,5,4,3,2,1,0)
        // Actually flags are checked from MSB to LSB:
        // bit 7 = first block, bit 6 = second, etc.
        // We want: 3 uncompressed (bits 7,6,5 = 0) then 1 compressed (bit 4 = 1)
        bus.write_u8(src + 4, 0b0001_0000);
        // 3 literal bytes
        bus.write_u8(src + 5, 0xAA);
        bus.write_u8(src + 6, 0xBB);
        bus.write_u8(src + 7, 0xCC);
        // Compressed block: length=3 (encoded as 0), offset=3 (encoded as 2)
        // b1 = (0 << 4) | ((2 >> 8) & 0x0F) = 0x00
        // b2 = 2 & 0xFF = 0x02
        bus.write_u8(src + 8, 0x00);
        bus.write_u8(src + 9, 0x02);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x11, &mut regs, &mut bus);

        assert_eq!(bus.read_u8(dst), 0xAA);
        assert_eq!(bus.read_u8(dst + 1), 0xBB);
        assert_eq!(bus.read_u8(dst + 2), 0xCC);
        // Reference: offset=3 back from current pos (3), length=3
        // dst+3 reads from dst+0 = 0xAA
        // dst+4 reads from dst+1 = 0xBB
        // dst+5 reads from dst+2 = 0xCC
        assert_eq!(bus.read_u8(dst + 3), 0xAA);
        assert_eq!(bus.read_u8(dst + 4), 0xBB);
        assert_eq!(bus.read_u8(dst + 5), 0xCC);
    }

    #[test]
    fn test_lz77_firered_bg_tile_stream() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;

        // FireRed stream at ROM 0x084026c0. It expands to:
        // - 32 bytes of 0x00
        // - 32 bytes of 0x11
        // - 32 bytes of 0x22
        let stream: [u8; 24] = [
            0x10, 0x60, 0x00, 0x00, 0x33, 0x00, 0x00, 0xF0, 0x01, 0x90, 0x01, 0x11, 0x11, 0xF0,
            0x01, 0x90, 0x01, 0x30, 0x22, 0x22, 0xF0, 0x01, 0x90, 0x01,
        ];
        for (i, byte) in stream.into_iter().enumerate() {
            bus.write_u8(src + i as u32, byte);
        }

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x11, &mut regs, &mut bus);

        for i in 0..32 {
            assert_eq!(bus.read_u8(dst + i), 0x00);
        }
        for i in 32..64 {
            assert_eq!(bus.read_u8(dst + i), 0x11);
        }
        for i in 64..96 {
            assert_eq!(bus.read_u8(dst + i), 0x22);
        }
    }

    #[test]
    fn test_rl_decompress_compressed_run() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;

        // Header: RLE type (0x30), decompressed size = 5
        let header: u32 = (5 << 8) | 0x30;
        bus.write_u32(src, header);

        // Compressed run: flag bit 7 set, length = 5 (encoded as 5-3 = 2)
        bus.write_u8(src + 4, 0x80 | 2);
        bus.write_u8(src + 5, 0xFF);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x14, &mut regs, &mut bus);

        for i in 0..5 {
            assert_eq!(bus.read_u8(dst + i), 0xFF);
        }
    }

    #[test]
    fn test_rl_decompress_uncompressed_run() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0200_1000;

        // Header: decompressed size = 3
        let header: u32 = (3 << 8) | 0x30;
        bus.write_u32(src, header);

        // Uncompressed run: flag bit 7 clear, length = 3 (encoded as 3-1 = 2)
        bus.write_u8(src + 4, 2);
        bus.write_u8(src + 5, 0x11);
        bus.write_u8(src + 6, 0x22);
        bus.write_u8(src + 7, 0x33);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x14, &mut regs, &mut bus);

        assert_eq!(bus.read_u8(dst), 0x11);
        assert_eq!(bus.read_u8(dst + 1), 0x22);
        assert_eq!(bus.read_u8(dst + 2), 0x33);
    }

    #[test]
    fn test_rl_decompress_vram_packs_halfwords() {
        let (mut regs, mut bus) = setup();

        let src = 0x0200_0000;
        let dst = 0x0600_0000;

        let header: u32 = (3 << 8) | 0x30;
        bus.write_u32(src, header);
        bus.write_u8(src + 4, 2);
        bus.write_u8(src + 5, 0x11);
        bus.write_u8(src + 6, 0x22);
        bus.write_u8(src + 7, 0x33);

        regs.set_reg(R0, src);
        regs.set_reg(R1, dst);

        Bios::handle_swi(0x15, &mut regs, &mut bus);

        assert_eq!(bus.read_u16(dst), 0x2211);
        assert_eq!(bus.read_u16(dst + 2), 0x0033);
    }

    #[test]
    fn test_soft_reset_jumps_to_rom() {
        let (mut regs, mut bus) = setup();
        // Flag at 0x03007FFA = 0 -> jump to 0x08000000
        bus.write_u8(0x03007FFA, 0);
        Bios::handle_swi(0x00, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R15), 0x08000000);
    }

    #[test]
    fn test_soft_reset_jumps_to_ewram() {
        let (mut regs, mut bus) = setup();
        // Flag at 0x03007FFA = 1 -> jump to 0x02000000
        bus.write_u8(0x03007FFA, 1);
        Bios::handle_swi(0x00, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R15), 0x02000000);
    }

    #[test]
    fn test_vblank_intr_wait() {
        let (mut regs, mut bus) = setup();
        bus.write_u16(0x03007FF8, 0xFFFF);
        Bios::handle_swi(0x05, &mut regs, &mut bus);
        assert_eq!(regs.get_reg(R0), 1);
        assert_eq!(regs.get_reg(R1), 1);
        // VBlank flag (bit 0) should be cleared
        assert_eq!(bus.read_u16(0x03007FF8) & 1, 0);
    }

    #[test]
    fn test_unknown_swi_ignored() {
        let (mut regs, mut bus) = setup();
        regs.set_reg(R0, 0x12345678);
        Bios::handle_swi(0xFF, &mut regs, &mut bus);
        // R0 should be unchanged
        assert_eq!(regs.get_reg(R0), 0x12345678);
    }
}

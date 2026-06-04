//! Fast-forward to a late frame, then single-step with tracing.

mod support;

use tinybird_core::Bus;

fn is_valid_exec(pc: u32) -> bool {
    matches!(
        pc,
        0x0000_0000..=0x0000_3FFF
            | 0x0200_0000..=0x0203_FFFF
            | 0x0300_0000..=0x0300_7EFF
            | 0x0800_0000..=0x0DFF_FFFF
    )
}

fn main() {
    let mut gba = support::load_gba_from_args("late_trace");

    let target_frames: u32 = std::env::var("TARGET_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1240);
    let extra_steps: u64 = std::env::var("EXTRA_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000);
    let trace_start: u64 = std::env::var("TRACE_START")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let trace_end: u64 = std::env::var("TRACE_END")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX);
    let dump_lo: u32 = std::env::var("TINYBIRD_DUMP_RANGE_LO")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let dump_hi: u32 = std::env::var("TINYBIRD_DUMP_RANGE_HI")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);

    let frame_cycles = gba.run_frames(target_frames);
    eprintln!(
        "Fast-forwarded {} frames in {} cycles; total_cycles={} pc={:08x}",
        target_frames,
        frame_cycles,
        gba.total_cycles,
        gba.cpu.fetch_addr()
    );

    for step_idx in 0..extra_steps {
        let pc_before = gba.cpu.fetch_addr();
        let thumb = gba.cpu.is_thumb_mode();
        if gba.total_cycles >= trace_start && gba.total_cycles <= trace_end {
            let opcode = if thumb {
                gba.bus.read_u16(pc_before) as u32
            } else {
                gba.bus.read_u32(pc_before)
            };
            eprintln!(
                "step={} cy={} pc={:08x} {} op={:08x} r0={:08x} r1={:08x} r2={:08x} r3={:08x} r4={:08x} r5={:08x} sp={:08x} lr={:08x} cpsr={:08x}",
                step_idx,
                gba.total_cycles,
                pc_before,
                if thumb { "T" } else { "A" },
                opcode,
                gba.cpu.registers.get_reg(0),
                gba.cpu.registers.get_reg(1),
                gba.cpu.registers.get_reg(2),
                gba.cpu.registers.get_reg(3),
                gba.cpu.registers.get_reg(4),
                gba.cpu.registers.get_reg(5),
                gba.cpu.registers.get_reg(13),
                gba.cpu.registers.get_reg(14),
                gba.cpu.registers.cpsr(),
            );
        }

        gba.step();

        let pc_after = gba.cpu.fetch_addr();
        if is_valid_exec(pc_before) && !is_valid_exec(pc_after) {
            eprintln!(
                "PC WENT INVALID: {:08x} -> {:08x} (cycle {})",
                pc_before, pc_after, gba.total_cycles
            );
            for r in 0..16 {
                eprint!("  r{}={:08x}", r, gba.cpu.registers.get_reg(r));
            }
            eprintln!(
                "\n  cpsr={:08x} mode={:?}",
                gba.cpu.registers.cpsr(),
                gba.cpu.registers.mode()
            );
            break;
        }
    }

    if dump_hi > dump_lo {
        for addr in (dump_lo..dump_hi).step_by(16) {
            let mut line = format!("MEM {:08x}:", addr);
            for byte_addr in addr..(addr + 16).min(dump_hi) {
                line.push_str(&format!(" {:02x}", gba.bus.read_u8(byte_addr)));
            }
            eprintln!("{line}");
        }
    }

    eprintln!(
        "Done. total_cycles={} pc={:08x} dispcnt={:04x}",
        gba.total_cycles,
        gba.cpu.fetch_addr(),
        gba.bus.read_u16(0x0400_0000)
    );
}

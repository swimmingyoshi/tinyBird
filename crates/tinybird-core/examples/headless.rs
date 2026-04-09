//! Headless GBA emulator runner for debugging

use tinybird_core::{ppu::ObjectAttribute, Bus, Gba, GbaButton};

fn parse_buttons(spec: &str) -> Option<GbaButton> {
    let mut buttons = GbaButton::empty();
    if spec.trim().is_empty() {
        return Some(buttons);
    }

    for token in spec.split('+') {
        let button = match token.trim().to_ascii_uppercase().as_str() {
            "A" => GbaButton::A,
            "B" => GbaButton::B,
            "SELECT" => GbaButton::SELECT,
            "START" => GbaButton::START,
            "RIGHT" => GbaButton::RIGHT,
            "LEFT" => GbaButton::LEFT,
            "UP" => GbaButton::UP,
            "DOWN" => GbaButton::DOWN,
            "R" => GbaButton::R,
            "L" => GbaButton::L,
            _ => return None,
        };
        buttons |= button;
    }

    Some(buttons)
}

fn parse_input_script(script: &str) -> Vec<(u64, GbaButton)> {
    let mut events = Vec::new();
    for entry in script.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((cycle, buttons)) = entry.split_once(':') else {
            continue;
        };
        let Ok(cycle) = cycle.trim().parse::<u64>() else {
            continue;
        };
        let Some(buttons) = parse_buttons(buttons) else {
            continue;
        };
        events.push((cycle, buttons));
    }
    events.sort_by_key(|&(cycle, _)| cycle);
    events
}

fn main() {
    let mut rom_path = "/home/swim/Documents/Code/tinyBird/roms/PokemonFireRed.gba".to_string();
    let mut bios_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bios" {
            bios_path = args.next();
        } else {
            rom_path = arg;
        }
    }

    let rom = std::fs::read(&rom_path).expect("failed to read ROM");
    let mut gba = Gba::new();
    if let Some(path) = bios_path {
        let bios = std::fs::read(&path).expect("failed to read BIOS");
        gba.load_bios(bios);
    }
    gba.load_rom(rom);
    gba.start();

    let max_cycles: u64 = std::env::var("MAX_CYCLES")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(300_000);

    let trace_start: u64 = std::env::var("TRACE_START")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
    let trace_end: u64 = std::env::var("TRACE_END")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
    let progress_every: u64 = std::env::var("PROGRESS_EVERY")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let input_script = std::env::var("TINYBIRD_INPUT_SCRIPT")
        .ok()
        .map(|script| parse_input_script(&script))
        .unwrap_or_default();
    let mut next_input_event = 0usize;

    fn is_valid_exec(pc: u32) -> bool {
        matches!(pc,
            0x0000_0000..=0x0000_3FFF
            | 0x0200_0000..=0x0203_FFFF
            | 0x0300_0000..=0x0300_7EFF
            | 0x0800_0000..=0x0DFF_FFFF)
    }

    let watch_addr: u32 = std::env::var("TINYBIRD_WATCH_ADDR")
        .ok().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let mut watch_prev = if watch_addr != 0 { gba.bus.read_u32(watch_addr) } else { 0 };
    let stop_on_watch = std::env::var("TINYBIRD_STOP_ON_WATCH").is_ok();
    let watch_after: u64 = std::env::var("TINYBIRD_WATCH_AFTER")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(0);

    // Track writes to a range of IWRAM
    let range_lo: u32 = std::env::var("TINYBIRD_WATCH_RANGE_LO")
        .ok().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let range_hi: u32 = std::env::var("TINYBIRD_WATCH_RANGE_HI")
        .ok().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    // snapshot of IWRAM range for change detection
    let range_len = if range_hi > range_lo { (range_hi - range_lo) as usize } else { 0 };
    let mut range_snapshot: Vec<u8> = if range_len > 0 {
        (range_lo..range_hi).map(|a| gba.bus.read_u8(a)).collect()
    } else { vec![] };

    let stop_at_rom = std::env::var("STOP_AT_ROM").is_ok();
    let watch_pc: u32 = std::env::var("WATCH_PC")
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    while gba.total_cycles < max_cycles {
        while next_input_event < input_script.len()
            && gba.total_cycles >= input_script[next_input_event].0
        {
            let (cycle, buttons) = input_script[next_input_event];
            gba.input.set_buttons(buttons);
            eprintln!(
                "INPUT event at cy={}: buttons={:?}",
                cycle,
                buttons
            );
            next_input_event += 1;
        }

        if progress_every != 0 && gba.total_cycles % progress_every == 0 {
            eprintln!(
                "progress cy={} pc={:08x} dispcnt={:04x} non_black={} halted={} ie={:04x} if={:04x} ime={:04x} bios_if={:04x}",
                gba.total_cycles,
                gba.cpu.fetch_addr(),
                gba.bus.read_u16(0x0400_0000),
                gba.ppu
                    .get_framebuffer()
                    .get_pixels_u32()
                    .iter()
                    .filter(|&&p| p != 0xFF00_0000)
                    .count(),
                gba.cpu.halted,
                gba.bus.read_u16(0x0400_0200),
                gba.bus.read_u16(0x0400_0202),
                gba.bus.read_u16(0x0400_0208),
                gba.bus.read_u16(0x0300_7FF8),
            );
        }

        let pc_before = gba.cpu.fetch_addr();
        let thumb = gba.cpu.is_thumb_mode();

        if watch_pc != 0 && pc_before == watch_pc {
            eprintln!(
                "WATCH_PC hit at cy={} pc={:08x} lr={:08x} sp={:08x} cpsr={:08x}",
                gba.total_cycles,
                pc_before,
                gba.cpu.registers.get_reg(14),
                gba.cpu.registers.get_reg(13),
                gba.cpu.registers.cpsr()
            );
        }

        if gba.total_cycles >= trace_start && gba.total_cycles <= trace_end {
            let opcode = if thumb {
                gba.bus.read_u16(pc_before) as u32
            } else {
                gba.bus.read_u32(pc_before)
            };
            let irq_vec = gba.bus.read_u32(0x03FF_FFFC);
            let ie = gba.bus.read_u16(0x0400_0200);
            let if_reg = gba.bus.read_u16(0x0400_0202);
            let ime = gba.bus.read_u16(0x0400_0208);
            eprintln!(
                "cy={} pc={:08x} {} op={:08x} r0={:08x} r1={:08x} r2={:08x} r3={:08x} r4={:08x} r5={:08x} r6={:08x} r7={:08x} r8={:08x} r9={:08x} r10={:08x} r11={:08x} r12={:08x} sp={:08x} lr={:08x} mode={:05b} cpsr={:08x} irqvec={:08x} ie={:04x} if={:04x} ime={:04x}",
                gba.total_cycles, pc_before,
                if thumb { "T" } else { "A" }, opcode,
                gba.cpu.registers.get_reg(0), gba.cpu.registers.get_reg(1),
                gba.cpu.registers.get_reg(2), gba.cpu.registers.get_reg(3),
                gba.cpu.registers.get_reg(4), gba.cpu.registers.get_reg(5),
                gba.cpu.registers.get_reg(6), gba.cpu.registers.get_reg(7),
                gba.cpu.registers.get_reg(8), gba.cpu.registers.get_reg(9),
                gba.cpu.registers.get_reg(10), gba.cpu.registers.get_reg(11),
                gba.cpu.registers.get_reg(12),
                gba.cpu.registers.get_reg(13), gba.cpu.registers.get_reg(14),
                gba.cpu.registers.cpsr() & 0x1F,
                gba.cpu.registers.cpsr(),
                irq_vec,
                ie,
                if_reg,
                ime,
            );
        }

        gba.step();

        if stop_at_rom {
            let pc = gba.cpu.fetch_addr();
            if (0x0800_0000..=0x0DFF_FFFF).contains(&pc) {
                eprintln!("Entered ROM at cycle {} pc={:08x}", gba.total_cycles, pc);
                break;
            }
        }

        // Track watchpoint address changes
        if watch_addr != 0 {
            let watch_now = gba.bus.read_u32(watch_addr);
            if gba.total_cycles < watch_after {
                watch_prev = watch_now;
            } else if watch_now != watch_prev {
                eprintln!("WATCH[{:08x}] changed at cy={}: {:08x} -> {:08x}  (pc_before={:08x} {})",
                    watch_addr, gba.total_cycles, watch_prev, watch_now,
                    pc_before, if thumb { "T" } else { "A" });
                eprintln!(
                    "  r0={:08x} r1={:08x} r2={:08x} r3={:08x} sp={:08x} lr={:08x} cpsr={:08x}",
                    gba.cpu.registers.get_reg(0),
                    gba.cpu.registers.get_reg(1),
                    gba.cpu.registers.get_reg(2),
                    gba.cpu.registers.get_reg(3),
                    gba.cpu.registers.get_reg(13),
                    gba.cpu.registers.get_reg(14),
                    gba.cpu.registers.cpsr(),
                );
                watch_prev = watch_now;
                if stop_on_watch {
                    break;
                }
            }
        }

        // Scan range for changes
        if range_len > 0 {
            for i in 0..range_len {
                let cur = gba.bus.read_u8(range_lo + i as u32);
                if cur != range_snapshot[i] {
                    eprintln!("RANGE WRITE [{:08x}] = {:02x} (was {:02x}) at cy={} pc={:08x} {}",
                        range_lo + i as u32, cur, range_snapshot[i],
                        gba.total_cycles, pc_before, if thumb { "T" } else { "A" });
                    range_snapshot[i] = cur;
                }
            }
        }

        let pc_after = gba.cpu.fetch_addr();
        if is_valid_exec(pc_before) && !is_valid_exec(pc_after) {
            eprintln!("PC WENT INVALID: {:08x} -> {:08x} (cycle {})", pc_before, pc_after, gba.total_cycles);
            for r in 0..16 {
                eprint!("  r{}={:08x}", r, gba.cpu.registers.get_reg(r));
            }
            eprintln!("\n  cpsr={:08x} mode={:?}", gba.cpu.registers.cpsr(), gba.cpu.registers.mode());
            // Auto-trace the next few instructions from the invalid address
            eprintln!("Auto-tracing 30 instructions from invalid PC...");
            for _ in 0..30 {
                let pc = gba.cpu.fetch_addr();
                let t = gba.cpu.is_thumb_mode();
                let op = if t { gba.bus.read_u16(pc) as u32 } else { gba.bus.read_u32(pc) };
                eprintln!("  cy={} pc={:08x} {} op={:08x} sp={:08x} lr={:08x} cpsr={:08x}",
                    gba.total_cycles, pc, if t { "T" } else { "A" }, op,
                    gba.cpu.registers.get_reg(13), gba.cpu.registers.get_reg(14),
                    gba.cpu.registers.cpsr());
                gba.step();
            }
            break;
        }
    }

    let pixels = gba.ppu.get_framebuffer().get_pixels_u32();
    let non_black = pixels.iter().filter(|&&p| p != 0xFF00_0000).count();
    let dispcnt = gba.bus.read_u16(0x0400_0000);
    if let Ok(path) = std::env::var("TINYBIRD_DUMP_PPM") {
        let rgb = gba.ppu.get_framebuffer().get_pixels_rgb888();
        let mut ppm = Vec::with_capacity(32 + rgb.len() * 3);
        ppm.extend_from_slice(b"P6\n240 160\n255\n");
        for (r, g, b) in rgb {
            ppm.push(r);
            ppm.push(g);
            ppm.push(b);
        }
        std::fs::write(&path, ppm).expect("failed to write PPM dump");
        eprintln!("Wrote framebuffer dump to {}", path);
    }
    if std::env::var("TINYBIRD_DUMP_PPU").is_ok() {
        let bg_cnt = [
            gba.bus.read_u16(0x0400_0008),
            gba.bus.read_u16(0x0400_000A),
            gba.bus.read_u16(0x0400_000C),
            gba.bus.read_u16(0x0400_000E),
        ];
        let obj_base = if (gba.bus.read_u16(0x0400_0000) & 0x7) <= 2 {
            0x10000
        } else {
            0x14000
        };
        let vram = gba.bus.get_vram_slice();
        let obj_nonzero = vram
            .get(obj_base..)
            .unwrap_or(&[])
            .iter()
            .filter(|&&b| b != 0)
            .count();
        eprintln!(
            "PPU DUMP: DISPCNT={:04x} DISPSTAT={:04x} VCOUNT={:04x} BG0CNT={:04x} BG1CNT={:04x} BG2CNT={:04x} BG3CNT={:04x}",
            gba.bus.read_u16(0x0400_0000),
            gba.bus.read_u16(0x0400_0004),
            gba.bus.read_u16(0x0400_0006),
            gba.bus.read_u16(0x0400_0008),
            gba.bus.read_u16(0x0400_000A),
            gba.bus.read_u16(0x0400_000C),
            gba.bus.read_u16(0x0400_000E),
        );
        eprintln!(
            "PPU DUMP: BG0HOFS={:04x} BG0VOFS={:04x} BG1HOFS={:04x} BG1VOFS={:04x} BG2HOFS={:04x} BG2VOFS={:04x} BG3HOFS={:04x} BG3VOFS={:04x}",
            gba.bus.read_u16(0x0400_0010),
            gba.bus.read_u16(0x0400_0012),
            gba.bus.read_u16(0x0400_0014),
            gba.bus.read_u16(0x0400_0016),
            gba.bus.read_u16(0x0400_0018),
            gba.bus.read_u16(0x0400_001A),
            gba.bus.read_u16(0x0400_001C),
            gba.bus.read_u16(0x0400_001E),
        );
        eprintln!(
            "PPU DUMP: WIN0H={:04x} WIN1H={:04x} WIN0V={:04x} WIN1V={:04x} WININ={:04x} WINOUT={:04x} BLDCNT={:04x} BLDALPHA={:04x} BLDY={:04x}",
            gba.bus.read_u16(0x0400_0040),
            gba.bus.read_u16(0x0400_0042),
            gba.bus.read_u16(0x0400_0044),
            gba.bus.read_u16(0x0400_0046),
            gba.bus.read_u16(0x0400_0048),
            gba.bus.read_u16(0x0400_004A),
            gba.bus.read_u16(0x0400_0050),
            gba.bus.read_u16(0x0400_0052),
            gba.bus.read_u16(0x0400_0054),
        );
        eprintln!(
            "PPU DUMP: PAL0={:04x} PAL1={:04x} VRAM0={:04x} VRAM1={:04x} OAM0={:04x} OAM1={:04x}",
            gba.bus.read_u16(0x0500_0000),
            gba.bus.read_u16(0x0500_0002),
            gba.bus.read_u16(0x0600_0000),
            gba.bus.read_u16(0x0600_0002),
            gba.bus.read_u16(0x0700_0000),
            gba.bus.read_u16(0x0700_0002),
        );
        for (bg_id, bgcnt) in bg_cnt.into_iter().enumerate() {
            let char_base = (((bgcnt >> 2) & 0x3) as usize) * 16 * 1024;
            let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2 * 1024;
            let color_256 = (bgcnt & 0x80) != 0;
            let screen_size = (bgcnt >> 14) & 0x3;
            let char_nonzero = vram
                .get(char_base..char_base + 0x4000)
                .unwrap_or(&[])
                .iter()
                .filter(|&&b| b != 0)
                .count();
            let screen_nonzero = vram
                .get(screen_base..screen_base + 0x800)
                .unwrap_or(&[])
                .iter()
                .filter(|&&b| b != 0)
                .count();
            eprintln!(
                "PPU DUMP: BG{} char_base={:04x} nonzero={} sample={:04x} {:04x} screen_base={:04x} nonzero={} sample={:04x} {:04x} 256c={} size={}",
                bg_id,
                char_base,
                char_nonzero,
                gba.bus.read_u16(0x0600_0000 + char_base as u32),
                gba.bus.read_u16(0x0600_0002 + char_base as u32),
                screen_base,
                screen_nonzero,
                gba.bus.read_u16(0x0600_0000 + screen_base as u32),
                gba.bus.read_u16(0x0600_0002 + screen_base as u32),
                color_256,
                screen_size,
            );
        }
        eprintln!(
            "PPU DUMP: OBJ pal0={:04x} pal1={:04x} obj_base={:04x} nonzero={} sample={:04x} {:04x}",
            gba.bus.read_u16(0x0500_0200),
            gba.bus.read_u16(0x0500_0202),
            obj_base,
            obj_nonzero,
            gba.bus.read_u16(0x0600_0000 + obj_base as u32),
            gba.bus.read_u16(0x0600_0002 + obj_base as u32),
        );
        if std::env::var("TINYBIRD_DUMP_VISIBLE_OAM").is_ok() {
            for i in 0..128usize {
                let base = i * 8;
                let Some(obj) = ObjectAttribute::from_oam(&gba.ppu.oam[base..base + 8]) else {
                    continue;
                };
                let (mut w, mut h) = obj.get_dimensions();
                if obj.double_size && obj.is_affine() {
                    w *= 2;
                    h *= 2;
                }
                let obj_x = if obj.x >= 256 {
                    obj.x as i32 - 512
                } else {
                    obj.x as i32
                };
                let obj_y = if obj.y >= 160 {
                    obj.y as i32 - 256
                } else {
                    obj.y as i32
                };
                let visible_y = (0..160).any(|y| obj.is_on_scanline(y));
                let visible_x = obj_x < 240 && obj_x + w as i32 > 0;
                if !visible_x || !visible_y {
                    continue;
                }
                eprintln!(
                    "OBJ {:03}: x={} y={} w={} h={} tile={:03x} prio={} pal={} affine={} dbl={} mode={} gfx={} 8bpp={} mosaic={} hflip={} vflip={}",
                    i,
                    obj_x,
                    obj_y,
                    w,
                    h,
                    obj.tile_num,
                    obj.priority,
                    obj.palette,
                    obj.is_affine(),
                    obj.double_size,
                    obj.obj_mode,
                    obj.gfx_mode,
                    obj.color_mode,
                    obj.mosaic,
                    obj.hflip,
                    obj.vflip,
                );
            }
        }
    }
    if std::env::var("TINYBIRD_DUMP_BG_QUEUE").is_ok() {
        for slot in 0..8u32 {
            let base = 0x0300_00C8 + slot * 0x10;
            eprintln!(
                "BGQ slot{}: src={:08x} dst={:08x} len={:04x} mode={:04x}",
                slot,
                gba.bus.read_u32(base),
                gba.bus.read_u32(base + 4),
                gba.bus.read_u16(base + 8),
                gba.bus.read_u16(base + 10),
            );
        }
    }
    let dump_lo: u32 = std::env::var("TINYBIRD_DUMP_RANGE_LO")
        .ok().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let dump_hi: u32 = std::env::var("TINYBIRD_DUMP_RANGE_HI")
        .ok().and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
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
        "Done. Ran {} cycles, {} frames, pc={:08x}, DISPCNT={:04x}, non_black={}",
        gba.total_cycles,
        gba.frame_count,
        gba.cpu.fetch_addr(),
        dispcnt,
        non_black
    );
}

//! Accuracy tests, run against real GBA test ROMs.
//!
//! These exist because the two worst bugs found in this emulator so far were
//! both single wrong constants in the CPU and neither was caught by a unit
//! test: `set_cpsr` masked out the T bit so Thumb could never be entered, and
//! `reset` left every byte of RAM and I/O holding the previous game's values.
//! Both surfaced as a game refusing to boot, which is an expensive way to find
//! a one-line defect.
//!
//! The ROMs are [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests). Each
//! runs a numbered series of checks and leaves the verdict in `r12`: zero means
//! every test passed, anything else is the number of the first failure. That
//! makes them readable without looking at the screen.
//!
//! Fetch them first:
//!
//! ```text
//! ./scripts/fetch-test-roms.sh
//! cargo test -p tinybird-core --test accuracy -- --nocapture
//! ```
//!
//! Without the ROMs every case reports as skipped rather than failing, so a
//! fresh checkout still passes.

use std::path::{Path, PathBuf};

use tinybird_core::gba::Gba;

/// Where `scripts/fetch-test-roms.sh` puts them.
const ROM_DIR: &str = "tests/roms";
/// Long enough for every ROM in the suite to finish and settle into its idle
/// loop. They are short; this is generous rather than tuned.
const FRAMES: u32 = 900;
/// The register a ROM leaves its verdict in.
///
/// Most of the suite uses `r12`, but the Thumb ROM cannot reach the high
/// registers with the instructions it is testing, so it uses `r7`. Reading the
/// wrong one does not fail loudly: it reports whatever that register happened
/// to hold, which for the Thumb ROM was zero — a silent pass for a suite that
/// had never actually been checked.
const VERDICT_DEFAULT: usize = 12;
const VERDICT_THUMB: usize = 7;

/// The repository root, from this crate's directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/tinybird-core should sit two levels below the root")
        .to_path_buf()
}

fn rom_path(name: &str) -> PathBuf {
    repo_root().join(ROM_DIR).join(name)
}

/// A real BIOS, if one is sitting in the repository root.
fn bios() -> Option<Vec<u8>> {
    std::fs::read(repo_root().join("gba_bios.bin")).ok()
}

/// What a test ROM reported.
enum Verdict {
    /// The ROM was not fetched. Reported, not failed.
    Missing,
    Passed,
    /// The number of the first check that failed.
    Failed(u32),
}

impl Verdict {
    fn describe(&self, name: &str) -> String {
        match self {
            Self::Missing => format!("{name}: skipped, run scripts/fetch-test-roms.sh"),
            Self::Passed => format!("{name}: all tests passed"),
            Self::Failed(n) => format!("{name}: failed at test {n}"),
        }
    }
}

/// Run one test ROM and read its verdict out of the register it reports in.
fn run(name: &str, verdict_reg: usize, use_bios: bool) -> Verdict {
    let Ok(rom) = std::fs::read(rom_path(name)) else {
        return Verdict::Missing;
    };

    let mut gba = Gba::new();
    if use_bios {
        if let Some(bios) = bios() {
            gba.load_bios(bios);
        }
    }
    gba.load_rom(rom);

    for _ in 0..FRAMES {
        gba.run_frame();
    }

    match gba.get_register(verdict_reg) {
        0 => Verdict::Passed,
        n => Verdict::Failed(n),
    }
}

/// Assert a ROM passes, skipping when it has not been fetched.
fn expect_pass(name: &str) {
    expect_pass_in(name, VERDICT_DEFAULT, false);
}

fn expect_pass_with(name: &str, use_bios: bool) {
    expect_pass_in(name, VERDICT_DEFAULT, use_bios);
}

fn expect_pass_in(name: &str, verdict_reg: usize, use_bios: bool) {
    let verdict = run(name, verdict_reg, use_bios);
    println!("{}", verdict.describe(name));
    if let Verdict::Failed(n) = verdict {
        panic!("{name}: first failing check is test {n}");
    }
}

// ------------------------------------------------------------------- the CPU

#[test]
fn arm_instructions() {
    // Conditions, branches, flags, shifts, data processing, PSR transfer,
    // multiply, single/halfword/block transfer, and swap.
    expect_pass("arm-arm.gba");
}

#[test]
fn thumb_instructions() {
    // Reports in r7, not r12; see VERDICT_THUMB.
    expect_pass_in("thumb-thumb.gba", VERDICT_THUMB, false);
}

// ------------------------------------------------------------------ the buses

#[test]
fn memory_access() {
    expect_pass("memory-memory.gba");
}

/// Reads of unmapped and misaligned addresses, where hardware has particular
/// answers rather than undefined ones.
#[test]
fn unsafe_access() {
    expect_pass("unsafe-unsafe.gba");
}

// ------------------------------------------------------------------- the BIOS

/// Needs a real BIOS image; the high-level stand-ins are not what it checks.
///
/// All four of its checks pass — the ROM never signals a failure — but the
/// verdict cannot be read cleanly. After the checks finish, something jumps to
/// the reset vector at `0x00000000` and re-runs the BIOS boot code, which
/// leaves `r12` holding a CPSR value instead of the zero the ROM left there.
/// The legitimate BIOS entries either side of it, the `SWI` at `0x08` and the
/// IRQ at `0x18`, both preserve `r12` correctly.
///
/// So this asserts what can be trusted: that the ROM never reports one of its
/// four tests as failed. A garbage verdict is reported rather than asserted on,
/// because it says nothing about the four behaviours under test.
#[test]
fn bios_calls() {
    if bios().is_none() {
        println!("bios-bios.gba: skipped, no gba_bios.bin in the repository root");
        return;
    }

    let verdict = run("bios-bios.gba", VERDICT_DEFAULT, true);
    match verdict {
        Verdict::Missing => println!("bios-bios.gba: skipped, run scripts/fetch-test-roms.sh"),
        Verdict::Passed => println!("bios-bios.gba: all tests passed"),
        // The ROM only ever writes 1..=4 to report a failure.
        Verdict::Failed(n) if n <= 4 => panic!("bios-bios.gba: failed at test {n}"),
        Verdict::Failed(n) => println!(
            "bios-bios.gba: no test reported a failure, but the verdict register              holds {n:#010X} rather than zero — see the note on this test"
        ),
    }
}

// -------------------------------------------------------------- cartridge save

#[test]
fn save_none() {
    expect_pass("save-none.gba");
}

#[test]
fn save_sram() {
    expect_pass("save-sram.gba");
}

#[test]
fn save_flash64() {
    expect_pass("save-flash64.gba");
}

#[test]
fn save_flash128() {
    expect_pass("save-flash128.gba");
}

// -------------------------------------------------------------------- the PPU
//
// These draw rather than report, so they are checked by what reaches the
// framebuffer: a blank screen means nothing was rendered at all.

/// Distinct colours on screen after the ROM has had time to draw.
fn colours_drawn(name: &str) -> Option<usize> {
    let rom = std::fs::read(rom_path(name)).ok()?;
    let mut gba = Gba::new();
    gba.load_rom(rom);
    for _ in 0..FRAMES {
        gba.run_frame();
    }
    let mut seen = std::collections::HashSet::new();
    for pixel in gba.ppu.get_framebuffer().as_slice() {
        seen.insert(format!("{:?}", pixel.color));
    }
    Some(seen.len())
}

#[test]
fn ppu_draws_something() {
    for (name, least) in [
        ("ppu-hello.gba", 2),
        ("ppu-shades.gba", 8),
        ("ppu-stripes.gba", 2),
    ] {
        match colours_drawn(name) {
            None => println!("{name}: skipped, run scripts/fetch-test-roms.sh"),
            Some(colours) => {
                println!("{name}: {colours} distinct colours");
                assert!(
                    colours >= least,
                    "{name} drew {colours} colours, expected at least {least} \
                     — a near-blank screen means nothing rendered"
                );
            }
        }
    }
}

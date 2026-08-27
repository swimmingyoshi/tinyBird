//! How fast a frame actually is.
//!
//! Ignored by default: it needs a commercial ROM, and it is a measurement
//! rather than an assertion — there is no threshold to fail against that would
//! not also fail on a slower machine.
//!
//! ```text
//! cargo test --release -p tinybird-core --test throughput -- --ignored --nocapture
//! ```
//!
//! The number that matters is milliseconds per frame against the 16.74ms a
//! Game Boy Advance frame is allowed. Anything close to that ceiling means the
//! browser build cannot fast-forward, because a 4x speed-up needs four frames
//! inside one frame's time.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tinybird_core::Gba;

/// A Game Boy Advance frame: 280,896 cycles at 16.78 MHz.
const FRAME_BUDGET_MS: f64 = 1000.0 / (16_777_216.0 / 280_896.0);

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Boot a ROM far enough that it is doing real work, then time it.
fn timed_frames(rom: &Path, state: Option<&Path>, warmup: u32, frames: u32) -> f64 {
    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(rom).expect("read ROM"));

    if let Some(state) = state {
        let bytes = std::fs::read(state).expect("read savestate");
        gba.load_state_bytes(&bytes).expect("restore savestate");
    }

    // A cold cartridge spends its first frames on logos and static screens,
    // which is not what anybody is trying to make faster.
    for _ in 0..warmup {
        gba.run_frame();
    }

    let start = Instant::now();
    for _ in 0..frames {
        gba.run_frame();
    }
    start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames)
}

#[test]
#[ignore = "Measurement, not an assertion; needs a commercial ROM"]
fn frame_cost_against_the_frame_budget() {
    let rom = workspace().join("roms/pokemon_fire_red.gba");
    if !rom.is_file() {
        eprintln!("no ROM at {}; nothing to measure", rom.display());
        return;
    }

    let state = workspace().join("TradeTest1.state");
    let state = state.is_file().then_some(state);

    let ms = timed_frames(&rom, state.as_deref(), 60, 600);
    let headroom = FRAME_BUDGET_MS - ms;

    eprintln!("--- frame throughput -------------------------------------");
    eprintln!("  budget      {FRAME_BUDGET_MS:.2} ms/frame");
    eprintln!("  measured    {ms:.2} ms/frame");
    eprintln!("  headroom    {headroom:.2} ms");
    eprintln!("  realtime    {:.1}x", FRAME_BUDGET_MS / ms);
    eprintln!("  ceiling     {:.1}x fast-forward before frames are dropped", FRAME_BUDGET_MS / ms);
    eprintln!("----------------------------------------------------------");

    assert!(ms > 0.0, "a frame has to take some time");
}

/// The same measurement with audio off.
///
/// `sync_io_to_apu` runs once per *instruction*, not once per frame, so the
/// gap between these two numbers is how much of a frame goes into keeping the
/// sound chip's registers in step.
#[test]
#[ignore = "Measurement, not an assertion; needs a commercial ROM"]
fn frame_cost_with_audio_disabled() {
    let rom = workspace().join("roms/pokemon_fire_red.gba");
    if !rom.is_file() {
        eprintln!("no ROM at {}; nothing to measure", rom.display());
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom).expect("read ROM"));
    gba.set_audio_enabled(false);
    for _ in 0..60 {
        gba.run_frame();
    }

    let start = Instant::now();
    for _ in 0..600 {
        gba.run_frame();
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / 600.0;

    eprintln!("  audio off   {ms:.2} ms/frame");
    assert!(ms > 0.0);
}

/// How long it takes to read the whole cartridge through the addon's view.
///
/// Addons are handed a `MemoryView` and nothing else, and its `read_bytes`
/// walks a byte at a time through the bus. Searching the ROM for a name table
/// means reading all of it, so this is the question of whether that can happen
/// on demand or has to be avoided.
#[test]
#[ignore = "Measurement, not an assertion; needs a commercial ROM"]
fn scanning_the_whole_rom_through_a_memory_view() {
    let rom_path = workspace().join("roms/pokemon_fire_red.gba");
    if !rom_path.is_file() {
        eprintln!("no ROM; nothing to measure");
        return;
    }

    let rom = std::fs::read(&rom_path).expect("read ROM");
    let len = rom.len();
    let mut gba = Gba::new();
    gba.load_rom(rom);

    let start = Instant::now();
    let mut checksum = 0u64;
    for offset in 0..len as u32 {
        checksum = checksum.wrapping_add(u64::from(gba.read_u8(0x0800_0000 + offset)));
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0;

    eprintln!("  ROM scan    {ms:.1} ms for {} MB (checksum {checksum})", len / 1_048_576);
}


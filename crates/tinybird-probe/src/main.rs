//! `tinybird-probe` — the memory research tool for writing game addons.
//!
//! Writing an addon for a new game means answering "where does this game keep
//! its party / gil / current map, and in what layout?". Doing that by hand
//! means a debugger, a lot of guessing, and replaying to the same screen over
//! and over.
//!
//! This tool makes the loop short:
//!
//! 1. Play in the desktop app until the data you want is on screen, then `F5`
//!    to write a save state.
//! 2. Point the probe at that state and search for a value you can see:
//!    `--find-text Marche`, `--find-u32 1500`, `--find-bytes 0a00`.
//! 3. `--dump` the hit to read the struct around it, and `--stride` to confirm
//!    the record size by checking that the pattern repeats.
//! 4. `--diff` two states to find what a single in-game action changed.
//!
//! Everything it prints is an address you can paste straight into an addon.
//!
//! ```text
//! tinybird-probe <rom> [--bios PATH] [--state PATH] [--frames N]
//!                      [--find-text S] [--find-bytes HEX] [--find-u32 N] [--find-u16 N]
//!                      [--codec ffta|ascii] [--region ewram|iwram|all]
//!                      [--dump ADDR[:LEN]] [--stride N] [--diff OTHER_STATE]
//! ```

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tinybird_core::{Gba, GbaButton};

mod codec;
mod scan;

use codec::Codec;
use scan::{MemoryRegion, Regions};

/// How many bytes of context a `--dump` shows when no length is given.
const DEFAULT_DUMP_LEN: usize = 96;
/// Cap on reported hits, so a pattern like `0000` cannot flood the terminal.
const MAX_HITS: usize = 64;

struct Options {
    rom: PathBuf,
    bios: Option<PathBuf>,
    state: Option<PathBuf>,
    diff_state: Option<PathBuf>,
    frames: u32,
    mash: GbaButton,
    mash_period: u32,
    codec: Codec,
    regions: Regions,
    find_text: Vec<String>,
    find_relative: Vec<String>,
    find_bytes: Vec<Vec<u8>>,
    find_u32: Vec<u32>,
    find_u16: Vec<u16>,
    dump: Vec<(u32, usize)>,
    stride: Option<u32>,
    save_state: Option<PathBuf>,
    strings: Option<usize>,
    screenshot: Option<PathBuf>,
}

impl Options {
    fn wants_search(&self) -> bool {
        !self.find_text.is_empty()
            || !self.find_relative.is_empty()
            || !self.find_bytes.is_empty()
            || !self.find_u32.is_empty()
            || !self.find_u16.is_empty()
    }
}

fn usage() -> &'static str {
    "\
tinybird-probe — memory research tool for tinyBird game addons

USAGE:
  tinybird-probe <rom.gba> [options]

LOADING
  --bios PATH         BIOS image (defaults to ./gba_bios.bin when present)
  --state PATH        load a .state savestate written by the desktop app
  --save-state PATH   write a savestate after running, to iterate without replaying
  --screenshot PATH   write a PNG of the current frame, to see where the game is
  --frames N          run N frames after loading (default 0)
  --mash BUTTONS      tap these every --mash-period frames while running,
                      e.g. --mash a,start to advance through an intro
  --mash-period N     frames between taps (default 8)

SEARCHING
  --find-text STR     find STR encoded with --codec
  --find-relative STR find STR under ANY encoding offset (use one case, e.g. arche)
  --find-bytes HEX    find a raw byte pattern, e.g. 4a00ff
  --find-u32 N        find a little-endian 32-bit value (decimal or 0x..)
  --find-u16 N        find a little-endian 16-bit value
  --codec NAME        ffta (default) or ascii
  --region NAME       ewram (default), iwram, or all

READING
  --strings N         list every decodable text run of at least N characters
  --dump ADDR[:LEN]   hex + text dump at ADDR (default LEN 96)
  --stride N          after a search, test whether hits repeat every N bytes
  --diff OTHER.state  report addresses that differ from another savestate

EXAMPLES
  tinybird-probe game.gba --state slot1.state --find-text Marche
  tinybird-probe game.gba --state slot1.state --dump 0x02000C40:128
  tinybird-probe game.gba --state a.state --diff b.state
"
}

fn parse_number(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let parsed = if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        trimmed.parse::<u64>()
    };
    parsed.map_err(|_| format!("'{text}' is not a number"))
}

fn parse_hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != ',')
        .collect();
    if cleaned.is_empty() {
        return Err("--find-bytes needs at least one byte".to_string());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(format!("'{text}' has an odd number of hex digits"));
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|_| format!("'{}' is not a hex byte", &cleaned[i..i + 2]))
        })
        .collect()
}

fn parse_dump(text: &str) -> Result<(u32, usize), String> {
    match text.split_once(':') {
        Some((addr, len)) => Ok((
            parse_number(addr)? as u32,
            parse_number(len)?.max(1) as usize,
        )),
        None => Ok((parse_number(text)? as u32, DEFAULT_DUMP_LEN)),
    }
}

fn parse_args() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut options = Options {
        rom: PathBuf::new(),
        bios: None,
        state: None,
        diff_state: None,
        frames: 0,
        mash: GbaButton::empty(),
        mash_period: 8,
        codec: Codec::Ffta,
        regions: Regions::Ewram,
        find_text: Vec::new(),
        find_relative: Vec::new(),
        find_bytes: Vec::new(),
        find_u32: Vec::new(),
        find_u16: Vec::new(),
        dump: Vec::new(),
        stride: None,
        save_state: None,
        strings: None,
        screenshot: None,
    };
    let mut rom_seen = false;

    while let Some(arg) = args.next() {
        let mut next = |flag: &str| -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("{flag} needs a value"))
        };

        match arg.as_str() {
            "--help" | "-h" => return Err(String::new()),
            "--bios" => options.bios = Some(PathBuf::from(next("--bios")?)),
            "--state" => options.state = Some(PathBuf::from(next("--state")?)),
            "--diff" => options.diff_state = Some(PathBuf::from(next("--diff")?)),
            "--frames" => options.frames = parse_number(&next("--frames")?)? as u32,
            "--mash" => options.mash = parse_buttons(&next("--mash")?)?,
            "--mash-period" => {
                options.mash_period = (parse_number(&next("--mash-period")?)? as u32).max(1)
            }
            "--codec" => options.codec = Codec::parse(&next("--codec")?)?,
            "--region" => options.regions = Regions::parse(&next("--region")?)?,
            "--find-text" => options.find_text.push(next("--find-text")?),
            "--find-relative" => options.find_relative.push(next("--find-relative")?),
            "--find-bytes" => options.find_bytes.push(parse_hex_bytes(&next("--find-bytes")?)?),
            "--find-u32" => options.find_u32.push(parse_number(&next("--find-u32")?)? as u32),
            "--find-u16" => options.find_u16.push(parse_number(&next("--find-u16")?)? as u16),
            "--dump" => options.dump.push(parse_dump(&next("--dump")?)?),
            "--stride" => options.stride = Some(parse_number(&next("--stride")?)? as u32),
            "--save-state" => options.save_state = Some(PathBuf::from(next("--save-state")?)),
            "--screenshot" => options.screenshot = Some(PathBuf::from(next("--screenshot")?)),
            "--strings" => options.strings = Some(parse_number(&next("--strings")?)?.max(2) as usize),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other if !rom_seen => {
                options.rom = PathBuf::from(other);
                rom_seen = true;
            }
            other => return Err(format!("unexpected argument {other}")),
        }
    }

    if !rom_seen {
        return Err("a ROM path is required".to_string());
    }
    Ok(options)
}

fn default_bios() -> Option<PathBuf> {
    let path = PathBuf::from("gba_bios.bin");
    path.is_file().then_some(path)
}

fn load_gba(options: &Options) -> Result<Gba, String> {
    let rom = fs::read(&options.rom)
        .map_err(|err| format!("cannot read ROM '{}': {err}", options.rom.display()))?;

    let mut gba = Gba::new();
    if let Some(path) = options.bios.clone().or_else(default_bios) {
        match fs::read(&path) {
            Ok(bios) => {
                eprintln!("bios   {} ({} bytes)", path.display(), bios.len());
                gba.load_bios(bios);
            }
            Err(err) => eprintln!("warning: cannot read BIOS '{}': {err}", path.display()),
        }
    }

    eprintln!("rom    {} ({} bytes)", options.rom.display(), rom.len());
    gba.load_rom(rom);
    gba.start();

    if let Some(path) = &options.state {
        load_state_into(&mut gba, path)?;
        eprintln!("state  {}", path.display());
    }

    if options.frames > 0 {
        if options.mash.is_empty() {
            eprintln!("running {} frames...", options.frames);
            gba.run_frames(options.frames);
        } else {
            eprintln!(
                "running {} frames, tapping {:?} every {} frames...",
                options.frames, options.mash, options.mash_period
            );
            run_with_mashing(&mut gba, options.frames, options.mash, options.mash_period);
        }
    }

    Ok(gba)
}

/// Advance `frames`, tapping `buttons` on a duty cycle.
///
/// Menus and dialogue boxes almost always need a *press*, not a hold, so the
/// buttons are released for half of each period. Without that, a held A does
/// nothing after the first frame and the intro never advances.
fn run_with_mashing(gba: &mut Gba, frames: u32, buttons: GbaButton, period: u32) {
    let half = (period / 2).max(1);
    for frame in 0..frames {
        let pressed = (frame % period) < half;
        gba.input.set_buttons(if pressed {
            buttons
        } else {
            GbaButton::empty()
        });
        gba.run_frames(1);
    }
    gba.input.set_buttons(GbaButton::empty());
}

fn parse_buttons(spec: &str) -> Result<GbaButton, String> {
    let mut buttons = GbaButton::empty();
    for token in spec.split([',', '+']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        buttons |= match token.to_ascii_uppercase().as_str() {
            "A" => GbaButton::A,
            "B" => GbaButton::B,
            "L" => GbaButton::L,
            "R" => GbaButton::R,
            "START" => GbaButton::START,
            "SELECT" => GbaButton::SELECT,
            "UP" => GbaButton::UP,
            "DOWN" => GbaButton::DOWN,
            "LEFT" => GbaButton::LEFT,
            "RIGHT" => GbaButton::RIGHT,
            other => return Err(format!("unknown button '{other}'")),
        };
    }
    Ok(buttons)
}

fn load_state_into(gba: &mut Gba, path: &Path) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|err| format!("cannot read state '{}': {err}", path.display()))?;
    gba.load_state_bytes(&bytes)
        .map_err(|err| format!("cannot load state '{}': {err}", path.display()))
}

/// Snapshot the searchable regions into a flat buffer per region.
fn capture(gba: &Gba, regions: Regions) -> Vec<MemoryRegion> {
    regions
        .list()
        .into_iter()
        .map(|(name, base, len)| {
            let bytes = (0..len).map(|off| gba.read_u8(base + off)).collect();
            MemoryRegion { name, base, bytes }
        })
        .collect()
}

fn report_hits(label: &str, pattern: &[u8], regions: &[MemoryRegion], codec: Codec) {
    let hits = scan::find(regions, pattern);
    println!(
        "\n{label}  pattern={}  {} hit(s){}",
        hex(pattern),
        hits.len(),
        if hits.len() > MAX_HITS {
            format!(", showing first {MAX_HITS}")
        } else {
            String::new()
        }
    );
    if hits.is_empty() {
        println!("  (nothing found — try a different --region, or a state where the value is on screen)");
        return;
    }
    for hit in hits.iter().take(MAX_HITS) {
        let context = scan::read(regions, hit.address.saturating_sub(8), 24);
        println!(
            "  {:#010X}  in {:5}  ctx {}  text {:?}",
            hit.address,
            hit.region,
            hex(&context),
            codec.decode(&scan::read(regions, hit.address, 16))
        );
    }
}

fn report_stride(regions: &[MemoryRegion], pattern: &[u8], stride: u32) {
    let hits = scan::find(regions, pattern);
    let Some(first) = hits.first() else {
        return;
    };
    println!("\nstride check: {stride} (={stride:#X}) bytes from {:#010X}", first.address);
    let mut consecutive = 0;
    for index in 0..8u32 {
        let address = first.address + index * stride;
        let bytes = scan::read(regions, address, pattern.len());
        let matches = bytes == pattern;
        if matches {
            consecutive += 1;
        }
        println!(
            "  slot {index}  {address:#010X}  {}  {}",
            if matches { "MATCH  " } else { "differs" },
            hex(&scan::read(regions, address, 16))
        );
    }
    if consecutive >= 2 {
        println!("  -> the pattern repeats; {stride:#X} is a plausible record size");
    }
}

fn report_dump(regions: &[MemoryRegion], address: u32, len: usize, codec: Codec) {
    println!("\ndump {address:#010X}  {len} bytes");
    let bytes = scan::read(regions, address, len);
    if bytes.iter().all(|b| *b == 0) {
        println!("  (all zero — address may be outside the captured region)");
    }
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let offset = address + (row * 16) as u32;
        let mut hex_part = String::new();
        for (index, byte) in chunk.iter().enumerate() {
            if index == 8 {
                hex_part.push(' ');
            }
            let _ = write!(hex_part, "{byte:02x} ");
        }
        println!(
            "  {offset:#010X}  {hex_part:<50}|{}|",
            codec.decode_lossy(chunk)
        );
    }
}

fn report_diff(before: &[MemoryRegion], after: &[MemoryRegion]) {
    println!("\ndiff: addresses that changed between the two states");
    let runs = scan::diff_runs(before, after);
    if runs.is_empty() {
        println!("  (no differences — are the two states really different?)");
        return;
    }
    println!("  {} changed run(s), showing the {} shortest", runs.len(), MAX_HITS.min(runs.len()));

    // Short runs are the interesting ones: a counter or a flag, rather than a
    // whole re-rendered buffer.
    let mut sorted = runs;
    sorted.sort_by_key(|run| (run.len, run.address));
    for run in sorted.iter().take(MAX_HITS) {
        println!(
            "  {:#010X}  {:4} byte(s)  {} -> {}",
            run.address,
            run.len,
            hex(&run.before),
            hex(&run.after)
        );
    }
}

/// List every run of decodable text of at least `min_len` characters.
///
/// This is the way in when you do not yet know how a game stores names: rather
/// than guessing an address, dump everything that decodes as text and look for
/// something recognisable from the screen.
fn report_strings(regions: &[MemoryRegion], min_len: usize, codec: Codec) {
    println!("
text runs of >= {min_len} characters");
    let mut found = 0usize;

    for region in regions {
        let mut index = 0usize;
        while index < region.bytes.len() {
            let text = codec.decode(&region.bytes[index..]);
            let consumed = text.chars().count();
            if consumed >= min_len {
                println!(
                    "  {:#010X}  {:5}  {:?}",
                    region.base + index as u32,
                    region.name,
                    text
                );
                found += 1;
                if found >= 400 {
                    println!("  (stopping after 400 runs; raise --strings to narrow)");
                    return;
                }
            }
            index += consumed.max(1);
        }
    }

    if found == 0 {
        println!("  (none — is --codec right for this game?)");
    }
}

/// Write the current frame as a PNG.
///
/// Knowing *where the game is* matters as much as the memory dump: a search
/// that finds nothing usually means the run stalled on a menu, not that the
/// pattern is wrong.
fn write_screenshot(gba: &Gba, path: &Path) -> Result<(), String> {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 160;

    let framebuffer = gba.ppu.get_framebuffer();
    let mut rgb = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for pixel in framebuffer.as_slice() {
        let rgb555 = pixel.color.to_rgb555();
        // 5-bit channels widened to 8 bits by replicating the high bits.
        let r = (rgb555 & 0x1F) as u8;
        let g = ((rgb555 >> 5) & 0x1F) as u8;
        let b = ((rgb555 >> 10) & 0x1F) as u8;
        rgb.push((r << 3) | (r >> 2));
        rgb.push((g << 3) | (g >> 2));
        rgb.push((b << 3) | (b >> 2));
    }

    image::RgbImage::from_raw(WIDTH, HEIGHT, rgb)
        .ok_or_else(|| "framebuffer size mismatch".to_string())?
        .save(path)
        .map_err(|err| format!("cannot write screenshot '{}': {err}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn run() -> Result<(), String> {
    let options = parse_args()?;
    let gba = load_gba(&options)?;
    let regions = capture(&gba, options.regions);

    for region in &regions {
        eprintln!(
            "region {:5} {:#010X}..{:#010X} ({} KB)",
            region.name,
            region.base,
            region.base + region.bytes.len() as u32,
            region.bytes.len() / 1024
        );
    }

    for text in &options.find_text {
        let pattern = options.codec.encode(text);
        if pattern.is_empty() {
            println!("\nfind-text {text:?}: nothing encodable with --codec {}", options.codec.name());
            continue;
        }
        report_hits(&format!("find-text {text:?}"), &pattern, &regions, options.codec);
        if let Some(stride) = options.stride {
            report_stride(&regions, &pattern, stride);
        }
    }

    for text in &options.find_relative {
        let hits = scan::find_relative(&regions, text);
        println!("
find-relative {text:?}  {} hit(s)", hits.len());
        if hits.is_empty() {
            println!("  (nothing — try a shorter, single-case fragment)");
        }
        for hit in hits.iter().take(MAX_HITS) {
            println!(
                "  {:#010X}  in {:5}  stride {}  first byte {:#04X}  offset {:#04X}  ctx {}",
                hit.address,
                hit.region,
                hit.stride,
                hit.first_byte,
                hit.implied_offset,
                hex(&scan::read(&regions, hit.address.saturating_sub(8), 32))
            );
        }
    }

    for pattern in &options.find_bytes {
        report_hits("find-bytes", pattern, &regions, options.codec);
        if let Some(stride) = options.stride {
            report_stride(&regions, pattern, stride);
        }
    }

    for value in &options.find_u32 {
        report_hits(
            &format!("find-u32 {value}"),
            &value.to_le_bytes(),
            &regions,
            options.codec,
        );
    }

    for value in &options.find_u16 {
        report_hits(
            &format!("find-u16 {value}"),
            &value.to_le_bytes(),
            &regions,
            options.codec,
        );
    }

    if let Some(min_len) = options.strings {
        report_strings(&regions, min_len, options.codec);
    }

    for (address, len) in &options.dump {
        report_dump(&regions, *address, *len, options.codec);
    }

    if let Some(other) = &options.diff_state {
        let mut second = load_gba(&Options {
            state: Some(other.clone()),
            frames: 0,
            ..clone_load_options(&options)
        })?;
        // Reload explicitly so a --frames on the first run does not desync the pair.
        load_state_into(&mut second, other)?;
        let after = capture(&second, options.regions);
        report_diff(&regions, &after);
    }

    if let Some(path) = &options.screenshot {
        write_screenshot(&gba, path)?;
        eprintln!("wrote screenshot {}", path.display());
    }

    if let Some(path) = &options.save_state {
        match gba.save_state_bytes() {
            Ok(bytes) => {
                fs::write(path, &bytes)
                    .map_err(|err| format!("cannot write state '{}': {err}", path.display()))?;
                eprintln!("wrote state {} ({} bytes)", path.display(), bytes.len());
            }
            Err(err) => eprintln!("warning: cannot serialize state: {err}"),
        }
    }

    if !options.wants_search()
        && options.dump.is_empty()
        && options.diff_state.is_none()
        && options.strings.is_none()
        && options.save_state.is_none()
        && options.screenshot.is_none()
    {
        println!("\nNothing to do. Pass --find-text, --find-bytes, --find-u32, --dump, or --diff.");
        println!("{}", usage());
    }

    Ok(())
}

/// Copy just the fields needed to load a second emulator instance.
fn clone_load_options(options: &Options) -> Options {
    Options {
        rom: options.rom.clone(),
        bios: options.bios.clone(),
        state: None,
        diff_state: None,
        frames: 0,
        mash: GbaButton::empty(),
        mash_period: options.mash_period,
        codec: options.codec,
        regions: options.regions,
        find_text: Vec::new(),
        find_relative: Vec::new(),
        find_bytes: Vec::new(),
        find_u32: Vec::new(),
        find_u16: Vec::new(),
        dump: Vec::new(),
        stride: None,
        save_state: None,
        strings: None,
        screenshot: None,
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) if message.is_empty() => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}\n");
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

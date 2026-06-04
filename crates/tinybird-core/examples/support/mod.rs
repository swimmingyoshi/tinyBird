use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tinybird_core::Gba;

pub fn load_gba_from_args(example_name: &str) -> Gba {
    match load_gba_from_args_inner() {
        Ok(gba) => gba,
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{}", usage(example_name));
            std::process::exit(2);
        }
    }
}

fn load_gba_from_args_inner() -> Result<Gba, String> {
    let (rom_path, bios_path) = resolve_paths()?;
    let rom = fs::read(&rom_path)
        .map_err(|err| format!("Failed to read ROM '{}': {err}", rom_path.display()))?;

    let mut gba = Gba::new();
    if let Some(path) = bios_path {
        let bios = fs::read(&path)
            .map_err(|err| format!("Failed to read BIOS '{}': {err}", path.display()))?;
        eprintln!("Loaded BIOS: {} ({} bytes)", path.display(), bios.len());
        gba.load_bios(bios);
    }

    eprintln!("Loaded ROM: {} ({} bytes)", rom_path.display(), rom.len());
    gba.load_rom(rom);
    gba.start();
    Ok(gba)
}

fn resolve_paths() -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut rom_path: Option<PathBuf> = None;
    let mut bios_path: Option<PathBuf> = None;
    let mut args = env::args_os().skip(1);

    while let Some(arg) = args.next() {
        if arg == "--bios" {
            let Some(path) = args.next() else {
                return Err("Missing BIOS path after --bios.".to_string());
            };
            bios_path = Some(PathBuf::from(path));
            continue;
        }

        let path = PathBuf::from(arg);
        if rom_path.replace(path).is_some() {
            return Err("Only one ROM path may be provided.".to_string());
        }
    }

    let rom_path = match rom_path {
        Some(path) => path,
        None => {
            let candidates = discover_rom_candidates(Path::new("roms"));
            match candidates.as_slice() {
                [path] => path.clone(),
                [] => {
                    return Err(
                        "No ROM argument was provided and no default ROM was found in ./roms/."
                            .to_string(),
                    );
                }
                _ => {
                    return Err(format!(
                        "Found {} ROMs in ./roms/; pass the one you want explicitly.",
                        candidates.len()
                    ));
                }
            }
        }
    };

    let bios_path = bios_path.or_else(default_bios_path);
    Ok((rom_path, bios_path))
}

fn discover_rom_candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut candidates: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("gba") || ext.eq_ignore_ascii_case("bin")
                    })
        })
        .collect();
    candidates.sort();
    candidates
}

fn default_bios_path() -> Option<PathBuf> {
    let path = PathBuf::from("gba_bios.bin");
    path.is_file().then_some(path)
}

fn usage(example_name: &str) -> String {
    format!(
        "Usage: cargo run --example {example_name} -- [--bios gba_bios.bin] <rom.gba>\nIf omitted, ./gba_bios.bin is used automatically when present, and the only ROM in ./roms/ is used automatically when there is exactly one."
    )
}

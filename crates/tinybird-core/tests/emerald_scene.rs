//! Look at what Emerald actually draws, from a local savestate.
//!
//! Ignored by default: it needs a commercial ROM and a savestate, neither of
//! which is in the repository. It exists so a rendering complaint can be
//! answered by looking at the frame rather than by reasoning about the PPU.

use std::path::{Path, PathBuf};

use tinybird_core::gba::Gba;

fn workspace(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// One screen as RGB bytes.
fn screen(gba: &Gba) -> Vec<u8> {
    let mut out = Vec::with_capacity(240 * 160 * 3);
    for pixel in gba.ppu.get_framebuffer().as_slice() {
        let rgb = pixel.color.to_rgb555();
        let widen = |five: u16| ((five as u32 * 255) / 31) as u8;
        out.push(widen(rgb & 0x1F));
        out.push(widen((rgb >> 5) & 0x1F));
        out.push(widen((rgb >> 10) & 0x1F));
    }
    out
}

/// Lay several screens out left to right so a sequence reads in one picture.
fn write_film(name: &str, frames: &[Vec<u8>]) {
    if frames.is_empty() {
        return;
    }
    let (w, h) = (240usize, 160usize);
    let mut out = format!("P6\n{} {}\n255\n", w * frames.len(), h).into_bytes();
    for y in 0..h {
        for frame in frames {
            let start = y * w * 3;
            out.extend_from_slice(&frame[start..start + w * 3]);
        }
    }
    let file = workspace(&format!("{name}.ppm"));
    std::fs::write(&file, out).expect("write the filmstrip");
    println!("filmstrip -> {}", file.display());
}

#[test]
#[ignore = "Needs a commercial ROM and a local savestate"]
fn emerald_renders_its_savestate() {
    let rom_path = workspace(
        "roms/Pokemon - Emerald Version (USA, Europe)/Pokemon - Emerald Version (USA, Europe).gba",
    );
    let state_path = workspace("Pokemon_-_Emerald_Version_USA_Europe_.state");
    if !rom_path.is_file() || !state_path.is_file() {
        eprintln!("need both an Emerald ROM and a savestate; nothing to render");
        return;
    }

    let mut gba = Gba::new();
    gba.load_rom(std::fs::read(&rom_path).expect("read Emerald ROM"));
    gba.load_state_bytes(&std::fs::read(&state_path).expect("read savestate"))
        .expect("deserialize Emerald savestate");

    // A handful of frames apart, so an animation mid-cycle is obvious rather
    // than being mistaken for a still picture that is simply wrong.
    let mut frames = Vec::new();
    for _ in 0..6 {
        for _ in 0..10 {
            gba.run_frame();
        }
        frames.push(screen(&gba));
    }
    write_film("emerald-scene", &frames);
}

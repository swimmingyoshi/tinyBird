# tinyBird

A Game Boy Advance emulator written in Rust.

## Features

- ARM7TDMI CPU core (ARM + Thumb instruction sets)
- Full GBA memory map with mirroring and open bus behavior
- PPU with tile rendering, sprites, and affine transforms
- Audio via CPAL
- Controller support via gilrs
- Save states (quicksave/quickload)
- ~35 FPS on current builds

## Requirements

- Rust (stable)
- A GBA BIOS dump (`gba_bios.bin`) placed in the project root
- A GBA ROM (`.gba`)

## Build & Run

```bash
cargo run --release
```

Use the file picker to load a ROM on startup, or place one in `roms/`.

## Project Structure

```
crates/
  tinybird-core/     # Emulator core (CPU, PPU, memory, scheduler)
  tinybird-desktop/  # Desktop frontend (windowing, audio, input)
```

## License

MIT

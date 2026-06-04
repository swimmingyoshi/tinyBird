# tinyBird

A Game Boy Advance emulator written in Rust.

## Features

- ARM7TDMI CPU core (ARM + Thumb instruction sets)
- BIOS HLE, DMA, timers, and savestate support
- Full GBA memory map with mirroring and open bus behavior
- PPU rendering with backgrounds, sprites, affine transforms, windows, and effects
- Audio output via CPAL
- Controller support via gilrs
- Battery saves plus quicksave/quickload
- Desktop frontend with home screen, pause menu, speed controls, color-correction toggle, and game-specific addon exports

## Requirements

- Rust (stable)
- A GBA BIOS dump at `./gba_bios.bin` for the highest compatibility
- A GBA ROM (`.gba` or `.bin`)

## Build & Run

```bash
cargo run --release
```

You can also pass paths explicitly:

```bash
cargo run --release -- --bios gba_bios.bin roms/game.gba
```

Startup behavior:

- If `gba_bios.bin` exists in the repo root, it is loaded automatically.
- If exactly one ROM exists in `./roms/`, it is loaded automatically.
- Otherwise, launch the app and press `O` to pick a ROM.

## Controls

- `Esc`: open the pause menu / resume gameplay
- `Tab`: hold for temporary 4x fast-forward
- `1` / `2` / `4`: 1x / 2x / 4x speed (`3` still works as a legacy alias for 4x)
  Fast-forward keeps frame pacing stable and mutes output audio intentionally.
- `F1`: toggle emulator HUD
- `F2`: toggle addon team panel
- `F3`: open/close addon popout window
- `F5` / `F8`: quicksave / quickload
- `M`: mute
- `[` or `-`: volume down
- `]` or `=`: volume up
- `C`: toggle color correction
- `R`: reset
- `O`: open ROM picker

## Examples

```bash
cargo run --example headless -- roms/game.gba
cargo run --example late_trace -- --bios gba_bios.bin roms/game.gba
```

Both examples now auto-use `./gba_bios.bin` when present and the only ROM in `./roms/` when there is exactly one.

## Addon Export

The desktop frontend now writes a structured snapshot to `stream-data/current-game.json`.

- Unsupported games export ROM metadata only.
- Fire Red exports a live party snapshot that can be consumed by overlays, bots, or a future web battle client.
- Supported games can render addon data in a dedicated in-game panel with `F2`, or in a detached popout window with `F3`.
- The first time a species appears, the desktop frontend downloads and caches its PNG artwork in `stream-data/pokemon-sprites/` for later offline reuse.

## Validation

As of June 3, 2026:

- `cargo check --workspace` passes
- `cargo test` passes with `208` tests passed and `1` ignored

## Project Structure

```
crates/
  tinybird-core/     # Emulator core (CPU, PPU, memory, scheduler)
  tinybird-desktop/  # Desktop frontend (windowing, audio, input)
```

## License

MIT

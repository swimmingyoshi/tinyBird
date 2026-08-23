# tinyBird

A Game Boy Advance emulator written in Rust.

## Features

- ARM7TDMI CPU core (ARM + Thumb instruction sets)
- BIOS HLE, DMA, timers, and savestate support
- Full GBA memory map with mirroring and open bus behavior
- PPU rendering with backgrounds, sprites, affine transforms, windows, and effects
- Audio output via CPAL
- Controller support via gilrs
- Battery saves plus multiple save-state slots with overwrite confirmation
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

The local web overlay server runs separately:

```bash
cargo run -p tinybird-web
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
- `M`: mute
- `[` or `-`: volume down
- `]` or `=`: volume up
- `C`: toggle color correction
- `R`: reset
- `O`: open ROM picker

### Function Keys

- `F1`: toggle emulator HUD
- `F2`: show the team panel
- `F3`: hide addon UI
- `F4`: show the encounters panel
- `F5`: open the save-state slot menu
- `F6`: expand or dock the addon dashboard
- `F7`: cycle game/dashboard space: Compact, Balanced, Game Focus
- `F8`: open the load-state slot menu
- `F9`: toggle borderless fullscreen
- `F10`: cycle side panel width: Slim, Normal, Wide
- `F11`: cycle dashboard placement: Classic, Left Info, Split Sidebars, Cozy Game
- `F12`: open the dashboard theme menu

Save/load menus use arrow keys or `1`-`5` to choose a slot, `Enter` to confirm, and `Esc` to cancel. Saving over an existing slot asks for confirmation in the menu before overwriting.
The theme menu uses arrow keys or `1`-`4` to choose a theme, `Enter` to apply, `W` to pick a wallpaper, and `Esc` to cancel.

### Dashboard

- `W`: choose a dashboard wallpaper image (`.png`, `.jpg`, `.jpeg`)

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
- Fire Red can also export current-area encounter data for early routes/caves, including slot rates and catch rates.
- Supported games can render addon data in dedicated in-game panels with `F2` and `F4`, or in the integrated dashboard with `F6`.
- The first time a species appears, the desktop frontend downloads and caches its PNG artwork in `stream-data/pokemon-sprites/` for later offline reuse.
- The dashboard auto-loads a custom wallpaper from `TINYBIRD_WALLPAPER`, `stream-data/wallpaper.png`, `stream-data/wallpaper.jpg`, `stream-data/background.png`, or `wallpaper.png` when present.

## Web Overlay

Run the desktop app first, then start the local overlay server:

```bash
cargo run -p tinybird-web
```

Useful URLs:

- Preview page: `http://127.0.0.1:8877/`
- Overlay page: `http://127.0.0.1:8877/overlay`
- OBS browser source: `http://127.0.0.1:8877/overlay?transparent=1&layout=column`

Overlay query params:

- `transparent=1`: transparent page background for OBS scenes
- `layout=column|row|stack`: choose how party cards flow
- `align=left|center|right`: dock the overlay on screen
- `compact=1`: use tighter cards for smaller scenes
- `hideHeader=1`: render only the team cards

## Validation

As of June 3, 2026:

- `cargo check --workspace` passes
- `cargo test` passes with `208` tests passed and `1` ignored

## Project Structure

```
crates/
  tinybird-addons/   # Shared addon snapshot schema and metadata contract
  tinybird-core/     # Emulator core (CPU, PPU, memory, scheduler)
  tinybird-desktop/  # Desktop frontend (windowing, audio, input)
  tinybird-web/      # Local web overlay/API server for OBS and browser tools
```

## License

MIT

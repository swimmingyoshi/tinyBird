# tinyBird — where things stand

Last updated: 27 August 2026.

This is the short version. The long versions live in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (emulator internals),
[`docs/WEB.md`](docs/WEB.md) (the browser build and the site),
[`ADDONS.md`](ADDONS.md) (the addon contract), and
[`docs/ADDON_DEVELOPMENT.md`](docs/ADDON_DEVELOPMENT.md) (how to write one).

## What works

| Piece | State |
|---|---|
| `tinybird-core` | CPU, bus, BIOS HLE, DMA, timers, PPU, APU, save types, savestates, link cable. Five commercial ROMs boot, render, and save. |
| `tinybird-desktop` | Windowed runner: audio, keyboard and gamepad, battery saves, quicksave, stream overlay, addon dashboard. |
| `tinybird-web` | Axum server: the browser build, save vault, accounts, rooms. |
| `tinybird-wasm` | The core as a WebAssembly module with a plain C ABI. No `wasm-bindgen`. |
| `tinybird-addons` | The addon contract: schema, memory view, registry, and manifests. |
| `tinybird-games` | Shipped addons: FireRed/LeafGreen, FFTA, and a cartridge fallback. |
| `tinybird-probe` | The address-finding tool: diffing, scanning, text search. |

Roughly 510 Rust tests and 72 browser-module tests. CI builds and tests the
non-windowing crates, runs the GBA accuracy suite as a gate, and builds the
WebAssembly module.

## Known gaps

Each of these is written up where it belongs; this is the index.

- **Trading stops short of a Pokémon changing hands.** The cable itself works —
  handshake, both players in the Trade Center, position data crossing both
  ways, 18,739 transfers against 0 for the control run. The last step needs a
  button pressed on an exact tile, which is puppeteering rather than emulation.
  ([ARCHITECTURE.md](docs/ARCHITECTURE.md#testing-it-with-a-real-cartridge))
- **The stream overlay is switched off.** It reads the desktop app's JSON
  export rather than the emulator in the page beside it, so on a server run by
  itself it shows a snapshot that never changes. `TINYBIRD_WEB_OVERLAY=1`
  re-enables it. ([WEB.md](docs/WEB.md))
- **Screenshots can be taken but not looked at.** No gallery, no full-size
  view, and a screenshot cannot exist apart from a save state.
- **Frame throughput is the ceiling on fast forward.** A frame costs ~11.8ms in
  the browser against a 16.74ms budget, so fast forward tops out near 1.4x.
  The measurements and what has been tried are in
  [WEB.md](docs/WEB.md). `cargo test --release -p tinybird-core --test
  throughput -- --ignored --nocapture` reproduces them.
- **FRLG species names cover the early game only.** Moves are complete; species
  are not. Both tables are hardcoded rather than read from the cartridge, and
  finding them in the ROM by searching for a known name is the real fix.
- **Manifest addons have no host wiring.** The format works and is tested;
  nothing yet reads `addons/*.json` and passes them to `build_registry_with`.
- **Lint and format backlogs.** `cargo clippy` runs in CI as reporting only,
  and there is no `cargo fmt --check`, because the tree predates both. The
  reasoning and the path to turning them on are in `.github/workflows/ci.yml`.

## Running it

```bash
cargo run                       # desktop
cargo run -p tinybird-web       # the site, on http://127.0.0.1:8877

# the browser build needs the module first
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
```

A BIOS dump at `gba_bios.bin` is optional for most games and required for
FireRed, which decompresses its graphics through BIOS calls the HLE stand-ins
do not cover well enough.

Neither ROMs nor the BIOS are in this repository, and `.gitignore` is set up to
keep it that way.

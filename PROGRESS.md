# tinyBird — where things stand

Last updated: 17 November 2026.

This is the short version. The long versions live in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (emulator internals),
[`docs/WEB.md`](docs/WEB.md) (the browser build and the local server),
[`ADDONS.md`](ADDONS.md) (the addon contract), and
[`docs/ADDON_DEVELOPMENT.md`](docs/ADDON_DEVELOPMENT.md) (how to write one).
Newer ground has its own pages:
[`docs/automation.md`](docs/automation.md) (headless control),
[`docs/local-training.md`](docs/local-training.md) (reinforcement learning),
and [`docs/session-recovery.md`](docs/session-recovery.md).

## What works

| Piece | State |
|---|---|
| `tinybird-core` | CPU, bus, BIOS HLE, DMA, timers, PPU, APU, save types, savestates, link cable. Five commercial ROMs boot, render, and save. |
| `tinybird-desktop` | Windowed runner: audio, keyboard and gamepad, battery saves, quicksave, stream overlay, addon dashboard. |
| `tinybird-web` | Local server, loopback only: the browser player, automatic recovery, the add-on Workshop, local ROMs and BIOS, and the OBS overlay. The hosted site built on it as a proof of concept now lives in a separate private repository. |
| `tinybird-wasm` | The core as a WebAssembly module with a plain C ABI. No `wasm-bindgen`. |
| `tinybird-addons` | The addon contract: schema, memory view, registry, and manifests — addons written as JSON, loaded from `addons/` on the desktop and kept in browser storage on the web. |
| `tinybird-games` | Shipped addons: FireRed/LeafGreen, FFTA, and a cartridge fallback. |
| `tinybird-probe` | The address-finding tool: diffing, scanning, text search. |
| `tinybird-runtime` | The core without a window: `tinybird-headless` takes JSON commands on stdin and answers with frames and named memory observations. The Python client and the optional Gymnasium training tools in `examples/python` drive it. |

Roughly 610 Rust tests and 90 browser-module tests. CI builds and tests the
non-windowing crates, runs the GBA accuracy suite as a gate, builds the
WebAssembly module and smoke-tests the local server. `cargo fmt --check`,
`cargo clippy -D warnings` and `cargo audit` are gates too.

## Known gaps

Each of these is written up where it belongs; this is the index.

- **Trading stops short of a Pokémon changing hands.** The cable works —
  handshake, both players in the Trade Center, thousands of transfers against 0
  for the control run — and the harness now *proves* no exchange happens rather
  than leaving it to a filmstrip: it reads both parties by their unencrypted
  personality values and prints `traded:` either way. What is missing is the
  button sequence that opens the trade menu.
  ([ARCHITECTURE.md](docs/ARCHITECTURE.md#testing-it-with-a-real-cartridge))
- **The stream overlay is switched off.** It reads the desktop app's JSON
  export rather than the emulator in the page beside it, so on a server run by
  itself it shows a snapshot that never changes. `TINYBIRD_WEB_OVERLAY=1`
  re-enables it. ([WEB.md](docs/WEB.md))
- **No two-player mode in the browser.** The link cable works in the core and
  between two wasm instances (`link.js`, tested), but carrying input between
  browsers needed the hosted lobby. Two consoles in one tab would need no server.
- **Frame throughput is the ceiling on fast forward.** A frame cost ~11.8ms in
  the browser against a 16.74ms budget, so fast forward topped out near 1.4x.
  A profile-driven fix to the PPU's per-instruction entry has since taken about
  14% off the wasm frame side by side, which should put it near 10ms and 1.6x;
  that is an estimate, not yet re-measured in a browser. Audio is under 2% of a
  frame; the rest is the CPU interpreter, timers and bus bookkeeping.
  The measurements, how to profile, and what is left are in
  [WEB.md](docs/WEB.md). `cargo test --release -p tinybird-core --test
  throughput -- --ignored --nocapture` reproduces them.
- **PP and catch rates are still compiled tables.** Names are not: moves,
  species and items are read out of the cartridge by searching it for a known
  name, so they are right for any build including ROM hacks. The numbers beside
  them live in differently-shaped structures and have not had the same
  treatment.

## Running it

```bash
cargo run                       # desktop
cargo run -p tinybird-web       # the browser player, on http://127.0.0.1:8877

# the browser build needs the module first
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
```

A BIOS dump at `gba_bios.bin` is optional for most games and required for
FireRed, which decompresses its graphics through BIOS calls the HLE stand-ins
do not cover well enough.

Neither ROMs nor the BIOS are in this repository, and `.gitignore` is set up to
keep it that way.

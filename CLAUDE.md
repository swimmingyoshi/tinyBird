# tinyBird

A GBA emulator in Rust: a native core, a desktop runner, a WebAssembly build
with a local browser player, a headless runtime for automation, and a JSON addon
system that reads live game state.

Orientation lives in [`PROGRESS.md`](PROGRESS.md), with the long versions in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (emulator internals),
[`docs/WEB.md`](docs/WEB.md) (browser build and local server), and
[`ADDONS.md`](ADDONS.md) (the addon contract).

## Scope

This repository is the emulator and the tools around it. The hosted site that
was built on it as a proof of concept (accounts, a save vault, screenshots,
support tickets, community add-ons and multiplayer rooms) lives in a separate
private repository. Do not bring those features back here; the browser player
in this repository is for building and testing from a checkout.

**The web server listens on loopback only** and refuses any other address,
because it serves the ROM folder and the BIOS. `guard.rs` also rejects requests
naming a non-loopback host (DNS rebinding) and cross-origin writes. Keep both.

## Secrets and payloads

Neither ROMs nor `gba_bios.bin` belong in this repository, and `.gitignore` is
set up to keep it that way. A local `.env` may still hold keys from the hosted
work; nothing here reads it any more, and `scripts/check-release.py` fails if
any of its values appear in a tracked file. Never commit one.

## Running it

```bash
cargo run                       # desktop
cargo run -p tinybird-web       # the browser player, on http://127.0.0.1:8877

# the browser build needs the module first
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
```

## Testing

Roughly 610 Rust tests and 90 browser-module tests. CI builds and tests the
non-windowing crates, runs the GBA accuracy suite as a gate, builds the
WebAssembly module, runs the browser-module tests against it, and smoke-tests
the local server (`tests/server_smoke.py`).

`cargo fmt --all -- --check` and `cargo clippy -- -D warnings` are CI gates, on a
pinned toolchain, alongside `cargo audit`. Run both before pushing:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

A lint that is allowed on purpose goes under `[workspace.lints.clippy]` in
`Cargo.toml` with its reason, not in a scattered `#[allow]`. CI cannot lint
`tinybird-desktop` (no windowing stack on the runner), so that crate depends on
the local run.

Tests that need commercial ROMs are `#[ignore]`d and run locally, for example
`cargo test --release -p tinybird-core --test link_trade -- --ignored`. The
real-browser tests in `tests/browser_*.mjs` use Playwright, installed with
`npm install --prefix target/workshop-ui-check --no-save --package-lock=false playwright`.

The release profile sets `lto = "fat"` and `codegen-units = 1` for a wasm module
about 14% smaller, at roughly ten times the build time. `panic = "abort"` is
deliberately not set — it would break `cargo test --release`, which is how
`tests/link_trade.rs` is meant to run.

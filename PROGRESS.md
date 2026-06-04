# tinyBird Development Progress

Last Updated: June 3, 2026

## Current Snapshot

`tinyBird` is an active Rust Game Boy Advance emulator workspace with:

- `tinybird-core`: CPU, memory bus, BIOS HLE, DMA, timers, PPU, APU, input, and savestates
- `tinybird-desktop`: a desktop runner with windowed output, audio playback, keyboard/gamepad input, battery saves, quicksave/quickload, and an on-screen overlay

## Verified Status

The workspace was validated locally on June 3, 2026 with:

- `cargo check --workspace`
- `cargo test`
- `cargo clippy --workspace --all-targets`

Results:

- `cargo check --workspace`: passing
- `cargo test`: passing, `208` tests passed and `1` ignored
- `cargo clippy --workspace --all-targets`: passing with warning-level findings

## What Works Today

- ARM and Thumb execution
- BIOS HLE support for several common routines
- Memory map mirroring and open-bus behavior
- DMA, timers, interrupts, and scheduler plumbing
- Background, sprite, affine, window, and color-effect rendering paths
- Audio mixing and desktop playback
- Battery-backed save persistence (`.sav`)
- Savestates (`.state`)
- Keyboard and gamepad input
- Headless/debug examples for trace-driven investigation

## Active Cleanup Targets

- Reduce the existing Clippy warning backlog in older core modules
- Keep ROM compatibility moving upward with more real-game validation
- Continue tightening timing and interrupt edge cases
- Add more frontend polish such as richer UX/settings when needed

## Handy Commands

```bash
cargo check --workspace
cargo test
cargo clippy --workspace --all-targets
cargo run --release
cargo run --example headless -- roms/game.gba
```

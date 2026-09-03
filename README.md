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
- Classic menu bar and status bar, driven by mouse or keyboard, with the stream dashboard as an optional view inside it
- Settings that persist between sessions, including a recent-ROM list
- Pluggable game addons: Pokemon FireRed/LeafGreen, Final Fantasy Tactics Advance, and a cartridge fallback that works for any ROM
- `tinybird-probe`, a memory research CLI for writing addons for new games
- Runs in a browser via WebAssembly, with no `wasm-bindgen` and no npm
- Optional asset vault for ROMs and save states, backed by a public CDN
- Desktop frontend with home screen, pause menu, speed controls, color-correction toggle, and game-specific addon exports

## Requirements

- Rust (stable)
- A GBA BIOS dump at `./gba_bios.bin` for the highest compatibility
- A GBA ROM (`.gba` or `.bin`)

## Build & Run

```bash
cargo run --release --
```

You can also pass paths explicitly:

```bash
cargo run --release -- --bios gba_bios.bin roms/game.gba
```

To boot straight into a savestate — useful for reproducing a bug or testing an
addon without replaying to the same screen:

```bash
cargo run --release -- roms/game.gba --state roms/game.state
```

The local web overlay server runs separately:

```bash
cargo run -p tinybird-web
```

Startup behavior:

- If `gba_bios.bin` exists in the repo root, it is loaded automatically.
- If exactly one ROM exists in `./roms/`, it is loaded automatically.
- Otherwise, launch the app and press `O` to pick a ROM.

## Interface

tinyBird has two layers, and they are independent:

- **Classic shell** — the menu bar (`File  Emulation  Video  Audio  Addons  Tools  Help`)
  and the status bar. Everything the emulator can do is reachable here with the
  mouse; the keyboard shortcuts below are all still shortcuts *to* these menus.
- **Stream dashboard** — the pixel-art addon panels (`F6`), unchanged.

Open a menu by clicking its title, or with `Alt` / `F10`. Once one is open,
moving across the bar switches between them without another click, and hovering
a `>` entry opens its submenu. Arrow keys navigate, `Enter` activates, `Esc`
backs out one level.

Hide the shell with `Tools > Menu Bar`:

| Mode | Behaviour |
|---|---|
| `Always` | Menu bar and status bar always visible (default) |
| `Auto-hide` | Appears when the pointer reaches the top edge, or on `Alt` / `F10` |
| `Off` | Never drawn — full-screen and streaming mode |

### Settings

Preferences are stored as JSON and reloaded at startup:

- Windows: `%APPDATA%\tinyBird\settings.json`
- Linux/macOS: `$XDG_CONFIG_HOME/tinybird/settings.json`, else `~/.config/tinybird/settings.json`

Theme, dashboard layout, panel sizes, window scale, volume, mute, color
correction, fullscreen, wallpaper, and the recent-ROM list all persist. A
missing or damaged file falls back to defaults rather than failing to launch.

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

### Shortcuts

- `Ctrl+O`: open a ROM
- `Ctrl+R`: reset
- `Ctrl+S`: screenshot (written to `stream-data/screenshots/`)
- `Ctrl+,`: settings

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
- `F10`: open the menu bar (same as `Alt`)
- `F11`: cycle dashboard placement: Classic, Left Info, Split Sidebars, Cozy Game
- `F12`: open the dashboard theme menu

Side panel width moved to `Addons > Side Panel Width` when `F10` was reassigned to the menu bar.

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

## Game Addons

Addons read live state out of the running game and publish it through one shared
schema, so the dashboard, the web overlay, and any external tool all see the same
data.

| Addon | Game | Reports |
|---|---|---|
| `pokemon_frlg_party` | Pokemon FireRed / LeafGreen | party, area encounters, active battle |
| `ffta_clan` | Final Fantasy Tactics Advance (USA) | full clan with race/job portraits out of battle; deployed party with live fight HP/MP in battle |
| `cartridge` | any ROM | header, region, maker, checksum, boot logo |

The `cartridge` addon claims every ROM and runs last, so an unrecognised game
shows its cartridge details instead of an empty panel. Games without a
hand-written renderer are drawn from the schema's generic table / list /
key-value sections.

`Tools > Addon Status` reports which addons are registered, which claim the
current ROM, and whether the active one has data or is merely idle — which is
the question you actually have when a new game shows nothing.

### Writing an addon

See [`docs/ADDON_DEVELOPMENT.md`](docs/ADDON_DEVELOPMENT.md) and
[`ADDONS.md`](ADDONS.md). The workflow uses `tinybird-probe`:

```bash
cargo build -p tinybird-probe --release

# Advance past an intro and capture where you got to.
./target/release/tinybird-probe game.gba --frames 20000 --mash a,start     --save-state /tmp/s.state --screenshot /tmp/s.png

# Find a value you can see on screen.
./target/release/tinybird-probe game.gba --state /tmp/s.state --find-u16 16

# Find text under an unknown encoding (most GBA games do not store ASCII).
./target/release/tinybird-probe game.gba --state /tmp/s.state --find-relative arche

# Confirm the record size, then read the struct.
./target/release/tinybird-probe game.gba --state /tmp/s.state     --find-bytes 100010000a000a00 --stride 264
./target/release/tinybird-probe game.gba --state /tmp/s.state --dump 0x0200360C:64

# Find what one in-game action changed.
./target/release/tinybird-probe game.gba --state before.state --diff after.state
```

## Addon Export

The desktop frontend writes a structured snapshot to `stream-data/current-game.json`.

- Every ROM exports at least its cartridge identity.
- Fire Red exports a live party snapshot that can be consumed by overlays, bots, or a future web battle client.
- Fire Red can also export current-area encounter data for early routes/caves, including slot rates and catch rates.
- Supported games can render addon data in dedicated in-game panels with `F2` and `F4`, or in the integrated dashboard with `F6`.
- The first time a species appears, the desktop frontend downloads and caches its PNG artwork in `stream-data/pokemon-sprites/` for later offline reuse.
- The dashboard auto-loads a custom wallpaper from `TINYBIRD_WALLPAPER`, `stream-data/wallpaper.png`, `stream-data/wallpaper.jpg`, `stream-data/background.png`, or `wallpaper.png` when present.

## Web

The same core runs in a browser tab. See [`docs/WEB.md`](docs/WEB.md).
For the single-container VPS deployment behind Cloudflare Tunnel, see
[`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

```bash
# Build the emulator module (one-time target install)
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release

# Serve the site
cargo run -p tinybird-web
```

| URL | What |
|---|---|
| `http://127.0.0.1:8877/` | Home |
| `http://127.0.0.1:8877/play` | Play a ROM in the browser |
| `http://127.0.0.1:8877/overlay/full` | OBS overlay |

`/play` reads ROMs from your local `roms/` folder and, when configured, from
your asset vault. `/play?rom=<url>&name=<label>` boots straight into one.

### Asset storage

Optional. Backed by the [0xstash media API](https://media.0xstash.dev/docs).

```bash
cp .env.example .env    # then set TINYBIRD_MEDIA_KEY
```

Reads are public and go straight from the browser to the CDN; the API key stays
on the server and is only used to list the vault and store save states. Without
a key everything still works from local files.

**Vault reads are public to anyone with the URL.** That is the right trade for
save states and artwork; the local `roms/` folder exists for anything you would
rather not publish.

### Contact form

Optional. Backed by the [0xstash contact API](https://contact.0xstash.dev/api/help).

```bash
cp .env.example .env    # then set TINYBIRD_CONTACT_KEY
```

Adds a "get in touch" panel to the home page. Without a key the panel stays
hidden, because a form that cannot send is worse than no form. The key stays on
the server: the page posts to `/api/contact` and the server talks to the
service, the same arrangement as the vault key above.

With accounts configured, sending requires signing in — the account menu is in
the bar on every page — and the reply address comes from the session rather than
the form. The name is still asked for, prefilled with the account's username.
Each account gets three messages an hour with a minute between them.

## Web Overlay

Run the desktop app first, then start the server:

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

As of August 23, 2026:

- `cargo check --workspace` passes
- `cargo test --workspace` passes with `440` tests
- Verified running: Pokemon FireRed and Final Fantasy Tactics Advance, on the
  desktop and in a browser at 60 fps

## Project Structure

```
crates/
  tinybird-addons/   # Addon extension point: schema, MemoryView, GameAddon registry
  tinybird-core/     # Emulator core (CPU, PPU, memory, scheduler)
  tinybird-games/    # The shipped addons: pokemon_frlg, ffta, cartridge
  tinybird-desktop/  # Desktop frontend (windowing, audio, input)
    src/ui/          # Canvas, font, theme, widgets, menu bar
    src/shell.rs     # Classic shell: menu model, command dispatch, chrome
    src/settings.rs  # Persisted settings
  tinybird-wasm/     # The core as a WebAssembly module with a plain C ABI
  tinybird-probe/    # Memory research CLI for writing addons
  tinybird-web/      # Web frontend: home, browser emulator, OBS overlays
```

`tinybird-games` sits below both frontends, which is why the browser reports the
same live game data as the desktop app.

## Development Docs

- `docs/ARCHITECTURE.md` — current state, target module shape, and phase status
- `docs/UI_UX_PLAN.md` — the classic-shell design spec and progress log
- `docs/ADDON_DEVELOPMENT.md` — how to find game addresses and write an addon
- `docs/WEB.md` — the WebAssembly build, the site, and asset storage
- `ADDONS.md` — the addon contract and shipped addons

## License

MIT

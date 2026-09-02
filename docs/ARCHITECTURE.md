# tinyBird Architecture — Current State & Target Design

Last reviewed: 2026-08-23

This document is the shared map for humans and agents working on tinyBird. It
records what the code looks like **today**, what hurts, and the **target shape**
we are moving toward. Update the Status table in section 4 as work lands.

---

## 1. Workspace Layout

```
crates/
  tinybird-addons/   # Addon extension point: schema, MemoryView, GameAddon registry
  tinybird-core/     # Emulator core: CPU, PPU, APU, bus, DMA, timers, scheduler
  tinybird-games/    # The shipped addons; no filesystem or frontend dependency
  tinybird-desktop/  # Desktop frontend: winit + softbuffer + cpal + gilrs
  tinybird-wasm/     # The core as a WebAssembly module with a plain C ABI
  tinybird-probe/    # Memory research CLI for writing addons
  tinybird-web/      # Web frontend: home, browser emulator, OBS overlays
```

The dependency direction is what makes the two frontends agree:

```
        tinybird-core        tinybird-addons
              |    \            /     |
              |     tinybird-games    |
              |        /        \     |
      tinybird-wasm            tinybird-desktop
              |
      tinybird-web  (serves the module; never links the core)
```

`tinybird-games` is deliberately free of `std::fs` and of any UI, so the browser
build reports byte-identical game data to the desktop app.

`tinybird-core` is in good shape: cleanly split by subsystem, ~208 tests, no
frontend knowledge. **The problems are all in `tinybird-desktop`.**

---

## 2. Where It Hurts Today

### 2.1 Two god-files

| File | Lines | Problem |
|---|---|---|
| `tinybird-desktop/src/overlay.rs` | 3441 | Bitmap font, drawing primitives, themes, HUD, home screen, pause screen, menus, **and** the entire hand-written FireRed dashboard, all in one flat module. |
| `tinybird-desktop/src/main.rs` | 2061 | Arg parsing, window/surface setup, frame pacing, audio, gamepad, save states, wallpaper decoding, blitting, **and** every UI state field, all on one `App` struct with ~50 fields. |

### 2.2 Drawing primitives have no context object

Every primitive threads `(buf, buf_w, buf_h, x, y, w, h, color, ...)` by hand:

```rust
fill_rect(buf, buf_w, buf_h, panel_x, panel_y, panel_w, 2, accent);
draw_text_centered(buf, buf_w, buf_h, center_x, y, &title, 2, accent, panel_bg);
```

This is why `draw_addon_panel` takes 12 positional parameters and why
`present_buffer` needs a 9-tuple for the addon panel alone. It also makes
clipping and sub-region drawing impossible, so every panel computes absolute
coordinates by hand.

**Fix:** a `Canvas` value owning `(&mut [u32], w, h)` plus a clip rect, with
methods (`canvas.fill_rect(rect, color)`, `canvas.text(...)`). Sub-panels get a
`canvas.sub(rect)` so children draw in local coordinates.

### 2.3 The blit path is duplicated — **fixed in phase 6**

`App::present_buffer` contains the **same** overlay/addon/pause/menu/toast
composition block twice — once for the integer-scale fast path, once for the
generic scaler. Any new UI layer must be added in both places, or it silently
appears only at some window sizes.

**Fix:** scale the game image, then run a single `compose_ui(canvas, &UiFrame)`.

### 2.4 There is no mouse input at all — **fixed in phase 3**

`window_event` handles `KeyboardInput`, `DroppedFile`, `Resized`, and
`CloseRequested` — and nothing else. No `CursorMoved`, no `MouseInput`, no
`MouseWheel`.

Every feature is therefore bound to a function key, and we have run out:
`F1`–`F12` are all taken, plus `W`, `C`, `R`, `O`, `M`, `1/2/3/4`, `[`, `]`,
`-`, `=`, `Tab`, `Esc`. Several of those collide conceptually with the GBA
button bindings (`W`, `S`, `A`, `Q` are mapped to L/R), and the scheme is
undiscoverable — the only way to learn it is the README or the footer hint
strings baked into panels.

**Fix:** mouse plumbing plus a classic menu bar. See `docs/UI_UX_PLAN.md`.

### 2.5 Nothing persists — **fixed in phase 2**

Theme, dashboard layout, panel sizes, volume, mute, color correction,
fullscreen, and wallpaper all reset to defaults on every launch. There is no
config file anywhere in the workspace.

**Fix:** a `Settings` struct serialized to a per-user config file, plus per-ROM
profiles keyed by game code.

### 2.6 "Multi-game" is a contract without an implementation — **fixed in phase 8**

The schema is genuinely good — `tinybird-addons` defines a game-agnostic
envelope with generic `key_value` / `list` / `table` sections. But:

- `AddonData` is `enum AddonData { FireRed(FireRedSnapshot) }` — one variant.
- `detect_addon_snapshot` hardcodes `let addons: [&dyn GameAddon; 1] = [&fire_red];`
- The `GameAddon` trait is **private** and lives in `game_addons.rs` next to
  1500 lines of FireRed memory-layout code.
- The desktop dashboard renders `AddonData::FireRed` through
  `draw_firered_dashboard` / `draw_firered_party_panel` /
  `draw_firered_encounter_panel` / `draw_firered_battle_panel`. **The generic
  `sections` the schema defines are never drawn by the desktop UI** — only
  `tinybird-web` consumes them.

So loading any non-FireRed ROM yields "No addon data yet" even though the
snapshot machinery could already describe it. Encounter tables are also
hand-written Rust functions (`route1_encounters()`, `mt_moon_1f_encounters()`,
...) rather than data.

**Fix:** a public `GameAddon` trait plus registry, a generic section renderer
used as the default for every game, and rich per-game renderers as an opt-in
override.

---

## 3. Target Module Shape

```
crates/tinybird-desktop/src/
  main.rs                 # arg parsing + event loop bootstrap only
  app/
    mod.rs                # App struct, ApplicationHandler impl
    emulation.rs          # frame pacing, catch-up, speed control
    savestate.rs          # slots, battery flush, paths
    rom.rs                # loading, ROM discovery, titles
  settings.rs             # persisted Settings + per-ROM profiles
  input/
    mod.rs                # routes events to the UI layer or to the GBA
    keymap.rs             # (was input_map.rs) key/pad -> GbaButton
    mouse.rs              # pointer state, click/drag/wheel
  ui/
    mod.rs                # UiFrame: what to draw this frame
    canvas.rs             # Canvas: buffer + clip rect + primitives
    font.rs               # embedded 8x8 glyph table
    theme.rs              # Palette tokens, theme table (data, not match arms)
    widget.rs             # Button/List/Checkbox/Slider + hit testing
    menubar.rs            # classic menu bar and dropdowns
    dialog.rs             # modal dialog stack (Open ROM, Save/Load, Settings)
    hud.rs                # FPS / speed / volume HUD
    home.rs               # no-ROM home screen
    dashboard/
      mod.rs              # dashboard chrome, layout, wallpaper
      generic.rs          # renders AddonSection for ANY game  <-- key piece
  addons/
    mod.rs                # registry construction, GbaMemory, snapshot export
    cartridge.rs          # catch-all: claims every ROM, reports the header
    pokemon_frlg.rs       # FireRed / LeafGreen
    ffta/
      mod.rs              # Final Fantasy Tactics Advance addon
      text.rs             # FFTA text codec
      units.rs            # unit array parsing
```

The `GameAddon` trait, `MemoryView`, and `AddonRegistry` live in
`tinybird-addons` so they are a public extension point rather than a private
detail of one game's file.

`tinybird-core` stays as-is.

---

## 4. Sequencing

| Phase | Goal | Status |
|---|---|---|
| 0 | Assessment + docs | **done** |
| 1 | `ui/` foundation: `Canvas`, `font`, `theme` | **done** |
| 2 | `settings.rs` persistence | **done** |
| 3 | Mouse input plumbing | **done** |
| 4 | Classic menu bar + dropdowns | **done** |
| 5 | Modal dialogs: Open ROM, Save/Load State, Settings | next |
| 6 | Single-path composition (kill the duplicated blit block) | **done** |
| 7 | Split `main.rs` into `app/` | partial — `shell.rs` split out |
| 8 | Public addon registry + generic section renderer | **done** |
| 9 | Move FireRed into `addons/pokemon_frlg/` | **done** (encounter tables still code, not data) |
| 10 | Multi-game validation pass | **in progress** — FFTA verified end to end |
| 11 | WebAssembly build + web frontend + asset storage | **done** — see `docs/WEB.md` |

### What landed in phases 8–9

The addon layer became a real extension point:

| Crate / file | Purpose |
|---|---|
| `tinybird-addons/src/memory.rs` | `MemoryView` — the read-only window an addon gets — plus `SparseMemory` for tests |
| `tinybird-addons/src/registry.rs` | `GameAddon`, `AddonRegistry`, `Detection`, ROM header parsing |
| `tinybird-addons/src/schema.rs` | the export envelope (unchanged shape) |
| `tinybird-desktop/src/addons/` | registry construction and the three shipped addons |
| `tinybird-probe/` | memory research CLI for finding addresses |

Key behaviour changes:

- `Detection` distinguishes *no addon exists* from *the addon has no data yet*,
  which is the question you actually have when a new game shows nothing.
- `cartridge` claims every ROM and is registered last, so no game shows an
  empty panel.
- The desktop draws the schema's generic `key_value` / `list` / `table`
  sections when it has no per-game renderer. Previously those were consumed
  only by `tinybird-web`.
- Addons take `&dyn MemoryView`, not `&Gba`, so parsing is unit-tested against
  synthetic memory instead of a booted ROM.

Two emulator bugs surfaced while testing FFTA and were fixed:

- `Apu::sample_buffer` grew without bound when nothing drained it, and is part
  of the savestate. A long headless run produced **187 MB** savestates; capped
  at one second of audio, the same state was **18 MB**. Version 4 of the format
  later dropped the cartridge and the rendered picture from it as well, taking
  that to **630 KB** — see [WEB.md](WEB.md).
- Loading such a state handed the audio backend ~84 million samples at once,
  and `AudioHandler::push_samples` computed a drain range past the end of its
  buffer and panicked. The batch is now clamped, keeping the newest audio.

### What landed in phases 1–4 and 6

New modules under `crates/tinybird-desktop/src/`:

| File | Purpose |
|---|---|
| `ui/canvas.rs` | `Rect` + `Canvas` with a clip rect, an origin, and `sub()` for local coordinates |
| `ui/font.rs` | the 8×8 glyph table, lifted out of `overlay.rs` |
| `ui/theme.rs` | `Palette` role tokens; themes are a static table, not `match` arms |
| `ui/input.rs` | `UiInput`: pointer, buttons, wheel, keys, with press-and-release click semantics |
| `ui/widget.rs` | immediate-mode button/checkbox/radio/slider/list/tabs/panel |
| `ui/menubar.rs` | declarative `Menu`/`MenuEntry` model, renderer, hit-test, keyboard nav |
| `settings.rs` | `Settings` + `SettingsStore`, JSON at the platform config dir |
| `shell.rs` | menu model construction, `UiCommand` dispatch, chrome layout and drawing |

`present_buffer` now composes once instead of twice: `blit_game` handles scaling
(`game_dest_rect` is a pure, tested function) and `compose_layers` draws every
overlay on top. Chrome is painted last on the full buffer, while everything else
is composed into a slice covering only the content rows, so the bars reserve
space instead of overlapping the game.

The desktop crate went from 14 tests to 90.

Phases 1–6 are the "UI/UX first" block the current work is focused on. See
`docs/UI_UX_PLAN.md` for the detailed design of that block.

---

## 5. Invariants To Preserve

- The stream dashboard (`F6` today) is a **product feature**, not legacy. The
  classic UI is *additional*, never a replacement, and must be switchable off.
- `stream-data/current-game.json` is a public contract consumed by
  `tinybird-web` and potentially external tools. Bump
  `SNAPSHOT_SCHEMA_VERSION` if its shape changes.
- Frame pacing lives on the hot path. UI work must not add per-pixel cost to
  the game blit; UI compositing happens after the scale step.
- `cargo test` must stay green (208 passing / 1 ignored at last check).

---

## Three bugs worth remembering

Both were found by a game refusing to boot, and both were a single wrong
constant or a missing line rather than anything structural. They are recorded
because the symptom was nowhere near the cause in either case.

### The T bit could never be set

`Registers::set_cpsr` masked the incoming value with `0xF000_00DF`. The comment
said it kept "N, Z, C, V, Q, I, F, T", but the T bit is bit 5 and `0xDF` has bit
5 clear, so the core could never be switched into Thumb by anything that went
through `set_cpsr` — which is how `MSR` and every exception return get there.

Writing the T bit through `MSR` is UNPREDICTABLE by the architecture reference,
but ARM7TDMI silicon honours it, and self-extracting packers use it to enter the
Thumb code they have just unpacked: set T, pad with two halfwords, then `BX`.
Pokémon Pinball: Ruby & Sapphire does exactly this. With T pinned low, the
padding decoded as ARM and execution ran straight past the `BX` into the middle
of the decompressor, which then rebuilt itself out of its own output forever.

The fix was `0xF000_00FF`, plus resteering the pipeline: `Pipeline` keeps its own
`thumb_mode` for fetching, so updating the register file alone left it decoding
ARM. `MSR` now flushes and refetches in the new state when T changes.

What made this findable was transcribing the packer out of the disassembly into
a few dozen lines of Python and running it against the ROM. It terminated at
63,113 bytes; the emulator's output matched byte for byte to exactly that point
and then kept going, which located the bug at a single instruction.

### A reset that reset almost nothing

`Gba::reset` rolled back the CPU and PPU but left every byte of work RAM, video
memory, and I/O holding the previous game's values, and left the DMA and timer
units armed. Loading a second ROM therefore kept the old game on screen while
the new one booted onto hardware it had never configured. Every ROM pair was
affected, in both directions; the header panel updated because it reads the ROM
directly, which made it look like the new game had loaded.

`SimpleBus::reset` now clears the volatile half of the machine and `Gba::reset`
rebuilds the peripherals. The cartridge, the BIOS image, and battery-backed save
data survive, as they do on hardware.

### A double-size sprite drawn half a sprite away

For an affine OBJ, X and Y in OAM are the top-left of the **bounding box**, and
when the double-size bit is set that box is already the doubled one. The pixel
sampler had this right — it centres the texture inside the box it is given. The
caller did not: it shifted the origin up and left by half the extra size, which
put every double-size affine sprite half a sprite from where the hardware puts
it, 32px each way on a 64x64.

Two things made it hard to see. Ordinary affine sprites were unaffected, because
for them the box *is* the sprite and the shift computed to zero — so most
rotation and scaling looked perfect. And `is_on_scanline` never had the shift,
so it tested the box where the hardware puts it while the renderer drew
somewhere else; the rows that did get drawn sampled the wrong part of the
texture rather than simply appearing in the wrong place, which reads as a
graphical glitch rather than as a misplaced sprite.

---

## Cartridge backup

Three of the four GBA backup types are memory-mapped and live in `bus.rs`. EEPROM
is not: it is a serial part clocked one bit per halfword access through the top
of the Game Pak window, driven by DMA, and it lives in [`eeprom.rs`](../crates/tinybird-core/src/eeprom.rs).

Two details cost real time and are worth stating:

- **The address width is not reported by anything.** A 4 Kbit part takes a 6-bit
  address and a 64 Kbit part takes 14, and the only signal is that every command
  is 8 bits longer on the larger part. Commands are therefore parsed when the
  written burst *ends*, because that is the first moment its length is known.
- **A block travels big-endian.** The last byte of the game's buffer goes out
  first. Getting this backwards is not a crash: it stores a byte-reversed image,
  which the game reads back as a corrupt save. The Minish Cap stamps
  `AGBZELDA:THE MINISH CAP:` across its first three blocks, which is what the
  order is pinned against in the tests.

Save-type detection reads the tag string every commercial cartridge embeds.
`SRAM_F_V` is a real tag and does not contain `SRAM_V`, so it needs its own
check; without it the cartridge is reported as having no backup at all.

---

## The cartridge clock

Ruby, Sapphire and Emerald carry a Seiko S-3511A on the cartridge, wired to the
Game Pak's GPIO pins. FireRed and LeafGreen do not have the chip at all, which
is why berry growth there is counted in steps and in Hoenn it is counted in
days. It lives in [`rtc.rs`](../crates/tinybird-core/src/rtc.rs).

It is not memory-mapped either. Three halfwords are overlaid on the start of the
ROM — data at `0x080000C4`, pin direction at `C6`, and a control register at
`C8` — and everything else is bit-banged over four pins: raise CS, clock eight
bits of a command out on SCK, clock the answer back in. Hence a state machine
rather than a set of registers.

Three things are worth stating:

- **Reads are gated twice.** A cartridge only gets the registers if its game
  code says it has the chip, and even then only once the game sets the control
  register. Until both hold, those three halfwords are the ROM underneath them,
  which is exactly what a board with no chip on it reads as forever. Writes are
  gated only on the first: setting the control register is how a game turns the
  other two on.
- **The offset is mirrored everywhere.** `0xC4` is a cartridge offset, and every
  region masks down to the same low offsets — EWRAM's `0x020000C4` as readily as
  ROM's `0x080000C4`. The write path runs before the region match, so it has to
  check the region itself; without that, EWRAM writes at the mirrored offset
  would be swallowed by the clock.
- **The core cannot read the wall clock.** `tinybird-core` runs on
  `wasm32-unknown-unknown`, where `SystemTime::now` panics, so the host pushes
  the time in with `Gba::set_wall_clock` and the clock advances between pushes
  from the cycles the machine has run. The web host pushes every frame from
  `Date.now()`; the desktop pushes before each batch. A host that pushes once
  still gets a running clock, on emulated time — which is what fast forward
  ought to do to a cartridge that is being fast forwarded.

The clock is deliberately **not** in the savestate. A real cartridge's clock
runs on its own battery whether or not the console is on, so restoring a machine
should not wind it back; and since the host pushes the time anyway, it is right
again on the very next frame. Leaving it out also meant the savestate format did
not have to change.

Writes from the game to the date and time registers are dropped. The clock
reported is the host's, and letting a game move it would put the machine's idea
of now somewhere the host cannot follow. Only the status register is kept, which
is where the games ask for 24-hour mode.

---

## The link cable

`crates/tinybird-core/src/sio.rs`. Multi-player mode: up to four consoles, one
of them the parent, each putting a halfword on the wire per transfer and each
ending up holding all four. It is what Pokémon uses for trading and battling.

### Where the state lives

The registers — `SIOCNT`, `RCNT`, `SIOMULTI0-3`, `SIOMLT_SEND` — stay in the
bus's I/O array with every other memory-mapped register. `Sio` holds only what
a register cannot: which console this is, how many are attached, and a transfer
in flight.

That split means **the savestate format did not change**, which matters because
version 4 had just landed and people have saves in both formats. It is also the
right answer on its own terms: the parts kept in `Sio` describe a cable, not a
machine, and restoring a state should give you a console that has just been
unplugged. `Gba::sio` is `#[serde(skip)]` to say so.

### How a transfer is driven

The core never touches a network. It says a transfer wants to happen and
accepts the answer; carrying halfwords between consoles is the host's job.

```
parent sets START     ->  link_transfer_pending() is true
                          host reads link_send_value() from every console
                          host calls link_deliver(values) on every console
                          cable clocks for the wire time
                          SIOMULTI0-3 land, START clears, IRQ fires
```

`link_step(&mut [Gba])` is that loop for consoles in one process, and is what
the tests drive. A networked host does the same three things with a round trip
in the middle.

**The game's own polling loop is the synchronisation.** A child that runs ahead
sits spinning on the `START` bit until the parent clocks the cable, exactly as
it would on hardware. Nothing has to stall the emulator to keep consoles in
step, and a slow link shows up as a slow transfer rather than as desync.

### Details that are easy to get wrong

- **`SIOMULTI0-3` blank to `FFFF` when a transfer starts**, not when it ends. A
  console that has left must read as absent rather than as its last message,
  which a game would take for live data.
- **Bits 2-6 of `SIOCNT` belong to the hardware**, not the game: which end of
  the cable this is, whether everyone is ready, this console's player number.
  A game may write anything there and has to read back the truth.
- **`START` is writable by the parent only.** On a child it reports whether a
  transfer is running, so a child writing it must not thereby claim the cable.
- **A transfer takes the time the wire takes** — 18 bits per console at the
  selected baud rate, about 5,200 cycles for two consoles at 115200 bps. A
  transfer that costs nothing lets a game spin through them faster than
  hardware ever could, which is how link code loses sync.

### The cable runs at the parent's rate

The parent drives the clock line. Its baud rate is the cable's, and a child's
own `SIOCNT` baud bits do not come into it.

Timing each console from its own register looks obviously right and is wrong in
a way that only a real game shows. Pokémon raises the parent to 115200 for a
trade and leaves the child on the 9600 it was initialised with. Timed from its
own register the child held the wire for 62,914 cycles against the parent's
5,242 — so it was **still clocking when the next transfer arrived**, refused to
join it, and silently missed every transfer from then on. The game calls that
`Communication error…`.

Nothing about this is visible from either side alone. The parent sees transfers
going out. The child sees a cable attached and its registers set correctly.
Only the two together, with a real cartridge driving them, shows it — which is
what `tests/link_trade.rs` exists for: two savestates taken at the Cable Club
counter, wired together with no network in the way. The register trace showed
the parent sending `B9A0` (Pokémon's link handshake) over and over and the
child answering `0000` every time.

`Sio::deliver` therefore takes a cycle count rather than a control register, and
the host is responsible for reading it from the parent and giving it to
everyone. Over a network it travels with the data in `link_data`.

### Testing it with a real cartridge

`tests/link_trade.rs`, ignored by default: it needs a commercial ROM and two
savestates taken at the Cable Club counter, neither of which can be committed.

```bash
cargo test --release -p tinybird-core --test link_trade -- --ignored --nocapture
```

Two consoles wired together in one process, stepped in lockstep, with no
network anywhere. If a trade works here and not in a browser the fault is in
the transport; if it fails here the fault is in the serial hardware. Every bug
on this page was found with it.

**Nobody is holding the consoles, so the test presses the buttons.** Which
presses get two games from standing at the counter to trading is not something
to guess at, so `find_the_input_that_starts_a_trade` runs several strategies and
reports how far each got, and the screens are written out as a filmstrip — the
two consoles above one another, time running left to right — because reading
what the games are showing is faster than reasoning about what they should be.
Frames are captured when a screen *changes* rather than on a timer: a still
picture repeated six times says nothing, and a menu that opens for a moment is
exactly what a timer misses.

What the linked run establishes, with a real game:

- the handshake completes — save prompt, "Start link with 2 players", and the
  attendant saying "Please enter";
- both players walk into the same Trade Center and stand at the machine;
- each console draws the **other** player and follows them as they move, which
  is position data crossing the cable and nothing else;
- 18,739 transfers carrying 60-odd distinct values in each direction.

It stops short of a Pokémon changing hands, and **the harness now says so
mechanically** rather than leaving it to be judged from a filmstrip.

Personality value and original-trainer id sit at offsets 0 and 4 of a party
slot and are *unencrypted* — the rest of the slot is XOR-scrambled with a key
derived from those two. So the identity of every Pokémon in both parties can be
read without decrypting anything, and "did one change hands" becomes a set
comparison: whoever holds an id the other console held before it started.

Every strategy currently reports the same thing:

```text
both-together  transfers 2539  parent said 53 distinct  child said 50 distinct
    no trade; both parties identical
...
traded: none - every strategy links up and none completes an exchange
```

That is the useful shape of the problem. The cable is not in question — 2,539
transfers and fifty-odd distinct values each way against zero for the control
run. What is missing is the button sequence that opens the trade menu, and the
sweep will print `TRADED:` the moment one of them finds it. Until then this is
a puppeteering problem with a pass/fail signal attached, which is a much better
place to leave it than a puppeteering problem without one.

**The control run is what makes the numbers mean anything.** The same games
given the same buttons with no cable between them produce 0 transfers and
`sd=0` on both consoles, against 18,739 and `sd=1` with one. Without that
comparison the figures would just be large.

### A waiting console still has to answer the cable

`IntrWait` and `VBlankIntrWait` halt until a particular interrupt flag is set.
The gate that emulates them used to re-halt on every instruction unless *that*
flag was pending — which meant no other interrupt handler could ever run while
a game was waiting.

On hardware the wait is not exclusive. It halts; **any** enabled interrupt
wakes the CPU, the BIOS dispatcher calls the game's handler, and only then does
the wait look again at the flag it wants. The handler for an unrelated
interrupt runs, every time.

For a linked game the difference is fatal rather than slow. The serial
interrupt is how a console reads what came off the cable, so a game sitting in
`VBlankIntrWait` — which is most menus, including a Pokémon summary screen —
stopped answering the cable the instant it got there. The other console saw a
partner that had gone silent and reported a communication error immediately.

Two details matter in the fix:

- The interrupt is **taken**, not merely woken from. Clearing `halted` on its
  own lets the waiting thread run the instruction after `IntrWait`, so the wait
  returns without its flag ever having been set. Entering the handler puts the
  program counter on the IRQ vector instead.
- The wait itself is untouched: still outstanding, still on its own flag, and
  it has not consumed the unrelated interrupt's bit. Once the handler returns
  and clears `IF`, the gate finds nothing pending and halts again.

`test_intr_wait_ignores_unrelated_irq_bits` asserted the old behaviour, so it
had encoded the bug. Its intent was right — an unrelated flag must not satisfy
the wait — and that part still holds; what was wrong was expecting the console
to stay asleep through the handler.

### The cable is visible in every serial mode

`SIOCNT` bits 2 to 6 describe the **wire**: which end of the cable this is,
whether anybody is on the other end, this console's player number. They are not
part of any protocol and read the same whichever serial mode is selected.

Imposing them only in multi-player mode was a bug with a strange shape. Fire Red
passes through normal mode while setting a link up — the register trace catches
it going to `SIOCNT=5081`, normal 32-bit — and asks there whether a cable is
plugged in. It was told no, so it gave up before ever reaching multi-player
mode. Switching the cable off and on fixed it, because `link_connect` wrote
those bits regardless of mode; that is exactly the workaround players found for
themselves, and it is what pointed at this.

Bit 7 is different and stays mode-specific. In multi-player mode it follows the
wire, which is what clears it when data lands. In normal mode it is the game's
own start bit for a transfer this does not carry, so taking it away would be
answering a question that was never asked.

The port in general-purpose or JOY-bus mode is not a link cable and none of
these bits are touched at all.

### Waiting needs a way out

A console waiting for a transfer does not run. That is the whole mechanism —
it is what keeps consoles in step across a network — and it is also why a lost
message is not a hiccup but a full stop.

A child had no deadline of its own. The parent gave up after
`LINK_TIMEOUT_MS` and transferred without the stragglers, but a child that had
joined a transfer and was never sent the result sat frozen for good. On screen
that is Fire Red holding on "Awaiting linkup…" forever. It also refused data it
was waiting for if the sequence number had slipped, which is a second way into
the same hole.

Both are closed. `Sio::abandon` ends a `Waiting` phase and clears the transfer
bit so the game runs again, reading an absent partner — the same thing it would
see if the cable had been pulled mid-transfer, which every game handles. A child
now takes data whenever it is waiting regardless of sequence number (messages
arrive in the order they were sent, so a stale one cannot overtake a live one),
and gives up on its own deadline if nothing arrives at all.

Abandoning deliberately does **not** touch a transfer that is merely still
clocking: that data has arrived and is on its way to the game.

### A halfword must never be dropped

Two consoles do not share a frame clock. The parent can run its wire out, have
its game ask for another transfer, and announce it while the child has not yet
had an animation frame in which to finish the previous one.

`Sio::deliver` used to accept data whenever a transfer existed, so the new one
replaced the one still clocking and **the halfword mid-wire never reached the
game**. A trade does not ride that out — the game checks what it receives, and
a gap is what `Sorry, we have a link error` means. Two rules now hold:

- data lands only on a console in the `Waiting` phase, never over one still
  clocking, and `deliver` reports whether it was taken;
- joining a transfer first **finishes** anything still on the wire rather than
  discarding it, so the halfword reaches the game a few thousand cycles early
  instead of not at all.

A child that cannot join no longer answers either. Answering would have the
parent wait on data that will never land anywhere; going quiet lets the parent
time out and mark the seat absent, which every game already handles.

### The 9% that nearly got shipped

The first version checked a bus flag on every instruction to notice writes to
serial registers. That cost **9% of emulation speed** — the accuracy suite went
from 22.0s to 24.2s on one ROM — for a flag that a real game sets a handful of
times in a whole session, and never at all in a single-player one.

It was not the work behind the flag: instrumenting showed `sync_sio` ran fewer
than a hundred thousand times in a fifteen-billion-cycle run. It was not the
store either; making the read non-clearing changed nothing. It was reaching
into `SimpleBus` for one byte per instruction at all.

The fix is to check `Sio::connected()` first, a field on `Gba` that the
interpreter is already holding, and only then look at the bus. Single-player
went back to 21.9s, indistinguishable from having no cable code at all. The
lesson is narrow and worth keeping: in the interpreter loop, a guard that
touches only what is already hot is not tidiness.

## Accuracy testing

`crates/tinybird-core/tests/accuracy.rs` runs [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests)
against the core. Fetch the ROMs first; they are MIT-licensed homebrew, not
commercial dumps, and are downloaded rather than committed:

```bash
./scripts/fetch-test-roms.sh
cargo test -p tinybird-core --test accuracy -- --nocapture --test-threads=1
```

Each ROM runs a numbered series of checks and leaves the verdict in **`r12`**:
zero means everything passed, anything else is the number of the first failure.
That is what makes them readable from a test harness instead of by eye. Without
the ROMs every case reports as skipped, so a fresh checkout still passes.

### Why this exists

The two worst bugs found so far were both single wrong constants in the CPU, and
neither was caught by a unit test — one surfaced as a game refusing to boot, the
other as a user reporting a stuck screen. This suite finds that class of defect
in seconds. It found two more the first time it ran.

### Standing at the last run

| ROM | Result |
|---|---|
| `arm.gba` | all tests passed |
| `thumb.gba` | all tests passed |
| `memory.gba` | all tests passed |
| `unsafe.gba` | all tests passed |
| `save/none.gba`, `sram`, `flash64`, `flash128` | all tests passed |
| `ppu/*.gba` | draws |
| `bios.gba` | all four checks pass, but the verdict cannot be read — see below |

`bios.gba` never reports a failure, so the four behaviours it tests are right.
The verdict register does not survive to be read: after the checks finish,
something jumps to the reset vector at `0x00000000` and re-runs the BIOS boot
code, which leaves `r12` holding a CPSR value instead of the zero the ROM put
there. The legitimate BIOS entries either side of it — the `SWI` at `0x08` and
the IRQ at `0x18` — both preserve `r12` correctly, so the spurious jump is the
open question. The test asserts what can be trusted: that no check reports a
failure.

### A trap worth knowing about

**Read the verdict from the register the ROM actually uses.** Most of the suite
reports in `r12`, but the Thumb ROM cannot reach the high registers with the
instructions it is testing, so it uses `r7`. Reading the wrong one does not fail
loudly — it reports whatever that register happens to hold, which for the Thumb
ROM was zero. That is a silent pass for a suite that had never been run, and it
hid a real bug until the register was corrected.

### What it found

Nine defects, none of which any unit test had caught.

**`ROR` decoded as `RRX` whenever the shift amount came from a register.** Bit 4
of a data-processing operand selects a register-specified amount; the decoder
read it as "this is RRX". RRX is the encoding of an *immediate* rotate of zero.
Every `ROR Rd, Rs` therefore rotated right through carry by one.

**PC read as instruction+8 during a register-specified shift.** That shift costs
an extra cycle and the prefetch runs on during it, so PC reads as
instruction+12 — and every register read in the instruction sees the later
value, not only the shifted one.

**The Game Pak SRAM window stopped after its first mirror.** `0x0E000000` to
`0x0FFFFFFF` is all cartridge save; the region ended at `0x0E00FFFF`, so
everything above fell through to open bus.

**The cartridge save bus was treated as 16 or 32 bits wide.** It is 8. A wider
read returns the one addressed byte repeated, not the neighbouring bytes, and a
wider write delivers only the byte in the addressed lane.

**A 32 KB SRAM chip did not mirror at `0x8000`.** The upper half of its window
read as a separate, empty 32 KB that a game expects to be the memory it just
wrote.

**Cartridge ROM mirrored with `%`.** Reads past the end of the cartridge came
back with its own opening bytes. Hardware leaves the data lines undriven, so
what returns is the address the CPU put on the bus: the halfword at `addr` reads
as `addr >> 1`.

**Byte writes to video memory stored a single byte.** The video bus is 16 bits
wide. Palette and background VRAM store the byte in *both* halves; sprite VRAM
and OAM ignore the write entirely.

**Halfword and word accesses were not aligned before reaching the bus**, so a
misaligned store landed at the unaligned address instead of the aligned one.
The save chip is the exception: an 8-bit part still selects its byte from the
low bits the wider buses discard.

**Misaligned `LDRH` rotated the halfword instead of the register.** Loading
`0x0020` from an odd address gives `0x20000000`, not `0x00002000`.

**`STR pc` stored instruction+8.** It stores instruction+12, because the store
happens a cycle later than a data-processing read.

**Loads with writeback into the base register kept the address.** When the base
and the destination are the same register the load wins on ARM7TDMI; the
writeback has to happen first.

**`CMP`/`CMN`/`TST`/`TEQ` with `Rd = 15` were treated as comparisons.** With the
S bit they are the ARMv2 exception return: CPSR is restored from SPSR, which can
change processor mode and swap out the banked registers.

**`SWP` and `SWPB` were not implemented at all.** They share the `1001` pattern
with multiply and fell through to data processing, where they decoded as
something else entirely.

**The BIOS could be read from outside itself.** It refuses: instead of its own
contents it returns the opcode its prefetch holds, two instructions ahead of the
one executing. A few games read the region deliberately, as a cheap source of a
value an emulator is unlikely to reproduce.

Two of these had a sting in the tail. Adding the BIOS read protection broke
`IntrWait`, because three places detected a real BIOS by *reading address zero*
and comparing against the stub — a probe the protection had just invalidated.
That is now `Bus::has_real_bios`, which reads the image directly. And the new
BIOS latch had to be cleared on reset, or a machine that had run one cartridge
differed from a cold one; `a_switched_rom_runs_like_a_cold_boot` caught it.

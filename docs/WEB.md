# tinyBird on the Web

Last reviewed: 2026-11-17

The same emulator core that runs the desktop app also runs in a browser tab and
feeds the stream overlays. This document covers how that build works, how the
page is put together, and what the local server does.

The server is for building and testing from a checkout: it listens on this
machine only. The hosted site that once shared this crate (accounts, a save
vault, screenshots, support tickets, community add-ons and multiplayer rooms)
now lives in its own private repository.

---

## 1. Surfaces

| Path | What it is |
|---|---|
| `/` | Start. Whether this checkout is ready to play (module built and current, BIOS, ROMs found), and the way on. |
| `/play` | The emulator. Canvas rendering, keyboard and gamepad input, WebAudio, save states, automatic recovery. |
| `/addons` | Built-in and local add-ons, the Workshop for building readers beside a test game, and a JSON editor. |
| `/reader-guide` | Memory sheets: how the shipped readers find their values. |
| `/overlay/{section}` | OBS browser sources, fed from the desktop app's JSON export. Off unless `TINYBIRD_WEB_OVERLAY=1`. |

Everything is served by `tinybird-web`, a single axum binary with the pages
compiled in via `include_str!`. Static files are one table, `ASSETS` in
`main.rs`: a new page or script is one row there. The pages share their look
through one vocabulary in `console.css`.

**The server only listens on loopback**, and refuses to start on any other
address: it serves your ROM folder and BIOS. Every request must also name a
loopback host, which stops DNS rebinding, and anything that writes or opens a
WebSocket must come from the server's own pages. That is `guard.rs`.

> The emulator module is a separate build output, and `cargo build` does not
> build the `wasm32` target. The server warns at startup when the module is
> older than the sources compiled into it — `tinybird-core`, `-addons`,
> `-games`, `-wasm`. It compares against **those sources**, not against its own
> binary: every edit to a page or a stylesheet relinks this server and leaves
> the module correctly untouched, and comparing the two build outputs made the
> warning fire through whole days of front-end work with nothing wrong.

> Because assets are embedded at compile time, **editing HTML/CSS/JS requires a
> rebuild** of `tinybird-web`. This trips people up; if a change does not appear,
> that is why.

---

## 2. The WebAssembly build

`tinybird-wasm` compiles the core to `wasm32-unknown-unknown` and exposes a
plain C ABI. There is **no `wasm-bindgen` and no npm**:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
```

That produces `target/wasm32-unknown-unknown/release/tinybird_wasm.wasm`
(about 910 KB, 290 KB gzipped), which `tinybird-web` serves at `/tinybird.wasm`. The path is
configurable with `TINYBIRD_WEB_WASM` or `--wasm`.

The trade-off is that JavaScript reads the module's linear memory directly.
That is confined to one file, `assets/tinybird.js`; nothing above it deals in
pointers.

### The interface

| Group | Functions |
|---|---|
| Memory | `tb_alloc`, `tb_free` |
| Set-up | `tb_init`, `tb_load_rom`, `tb_load_bios`, `tb_load_save`, `tb_screen_width/height` |
| Running | `tb_run_frame`, `tb_run_frames`, `tb_set_buttons`, `tb_reset`, `tb_set_paused`, `tb_set_color_correction` |
| Reading | `tb_frame_ptr/len`, `tb_audio_ptr/len`, `tb_audio_sample_rate`, `tb_frame_count`, `tb_is_running` |
| Addons | `tb_refresh_snapshot`, `tb_snapshot_ptr/len` |
| States | `tb_save_state`, `tb_state_ptr/len`, `tb_load_state` |

Two things to know:

- **Buffers are invalidated by the next call in.** Growing the module's memory
  detaches the `ArrayBuffer`, so JavaScript creates a fresh view every time and
  copies anything it keeps.
- **The audio rate is 65536 Hz, not 32768.** Playing at the wrong rate pitches
  everything; `AudioSink.setSampleRate` is called once a ROM is loaded.

### The BIOS is not optional in practice

`GET /bios` serves `gba_bios.bin` from the machine running the server, and the
page loads it before any ROM.

This matters more than "highest compatibility" suggests. Games decompress their
graphics through BIOS SWI calls, and the core's high-level stand-ins are not
accurate enough for all of them. Pokemon FireRed without a BIOS renders at full
speed with missing sprites, corrupt tiles, and colour-bar artefacts — measured
at 22 distinct colours on the intro screen against 51 with a real dump, and 27%
of pixels wrong. It looks like a graphics bug in the browser build; it is not.

The image is never bundled: a BIOS dump is copyrighted, `*.bin` is gitignored,
and a 404 is normal. The page then warns rather than failing.

Override the location with `TINYBIRD_WEB_BIOS` or `--bios`.

### Why the browser gets the same game data

`tinybird-games` — the addon registry and the shipped game readers — has no
filesystem or frontend dependency, so it compiles to wasm unchanged.
`tb_refresh_snapshot` runs the same registry the desktop app runs, and the page
renders the schema's generic sections. A new addon appears in the browser with
no changes to any JavaScript.

---

## 3. Where the game actually runs

**On the player's machine, in their browser tab.** `tinybird-web` does not
depend on `tinybird-core` — there is no emulator linked into the server, so it
could not run a game if it wanted to. It reads `tinybird.wasm` off disk and
hands over the bytes; the browser calls `WebAssembly.instantiateStreaming` and
runs it.

What the server does: serve static files and read local ROMs and the BIOS off
its own disk. No game CPU, ever.

## 4. The page

`/play` is laid out around one idea: the running screen is the hero, and
everything else is instrumentation.

```
+---------------------------------------------------------------+
| tinyBird                          Home  Overlay  Play   [cart] |
+---------------------------------------------------------------+
| 60.0 FPS                        Loaded pokemon_fire_red.gba    |  <- status
+---------------+---------------------------------+---------------+
| [Party][Area] |                                 |  [cartridge]  |
+---------------+             Screen              +---------------+
|               |                                 |    [Team]     |
|   read-out    |        [ control deck ]         +---------------+
|   move -->    |    [Save & load]    [Menu]     |   <-- move    |
+---------------+---------------------------------+---------------+
```

**Both rails are the same thing**: a strip of tabs over one pane. Which
sections go in which column is the player's call — every pane has a `move`
button that sends it across, and the choice is remembered.

That is the answer to a read-out that outgrew its column. A FireRed party of
six is taller than the screen it describes. Tabs show one section at a time;
two rails mean you can watch two at once — the party on the left and the battle
on the right — and decide for yourself which two those are.

A column can also be **split**, stacking two panes. Two columns at two panes
each is four sections on screen, which is as many as rails this wide hold
before each becomes a letterbox. A split half scrolls inside itself rather than
growing the column, so splitting a party of six shows you two things instead of
making the page twice as tall.

The top slot keeps the tab strip; a split slot gets a dropdown instead. That
asymmetry is the second design. Drawing the strip in both slots spent four rows
of a narrow column on two copies of the same buttons, and left every tab
rendered twice with no way to tell which copy a click would answer. The lower
pane is an "and also show…", so it costs one row and is styled like the
afterthought it is. A tab the lower pane already holds is marked rather than
disabled — clicking it swaps the two.

**Panes scroll; they do not stretch.** A section has no upper bound on what it
reports — an area with grass, surf and three rods on it is twenty-odd species —
and a pane that grows to fit pushes the deck and the screen down the page.
Bodies are capped and scroll inside themselves. Rows help too: a card with no
detail behind it is drawn dense, at roughly half the height of a party card,
since there is no reason to leave room for what a click would have opened.

**A section can flag its own tab.** `badge` is drawn on the tab rather than
inside the section, which is the point: it has to be readable from the tab you
are *not* looking at. FireRed uses it for an opponent whose IVs are worth
stopping for, so an encounter interrupts you wherever you were reading.

### Focus mode

**Tab** hides the header bar, the control deck and the key legend, and leaves
the screen and the two rails. The rails stay on purpose: they are what you read
*while* you play, and hiding them would make this a fullscreen button, which
the deck already has.

It also lifts two width caps — the page's 1440px reading measure, which exists
for text there is now less of, and the rails, which give up some of the width
they were using for comfort. The picture is width-bound (the screen keeps a
240x160 aspect ratio, so height follows width), and on a 1600px display that
buys a whole integer step of scale: 480x320 to 720x480.

Taking `Tab` is not free — it is how a keyboard moves between controls — so it
is only taken when it would otherwise do nothing: nothing focused, no dialog
open, and never when the player has bound Tab to the game themselves. Tabbing
through the deck still works. `Escape` also leaves, because it is what everyone
tries first, and the status line and a corner note both say how to get out.

**The rails only work if the sections are worth having.** Splitting a column is
no use when the things to put in it are filler, and FireRed briefly had two
kinds of filler: a `Team` section of numbers that were arithmetic on the party
beside it, and a section per encounter method that turned "what is on this
route" into four places to look. Both are gone — the team summary is the
caption over the party, and the methods are badges in one Area list — which is
why the strip is three tabs and not six. `docs/ADDON_DEVELOPMENT.md` has the
rule; it is the thing to check before adding a section, not after.

Saves is a pane like any other. It defaults to the right because that is where
it is useful and because the rail would otherwise be empty before a cartridge
loads, but it moves like everything else. It is one node moved between the
columns rather than two kept in step, so it parks in a hidden holder while the
rail showing it displays something else.

What used to be in that right rail and is not any more:

| Was | Now | Why |
|---|---|---|
| Cartridge panel | One-line strip | The header bar already names the ROM and the read-out heading already names the addon; the box was mostly those facts a second time. |
| Library tab | The empty screen | A list of ROMs is one you need exactly once — when there is no cartridge in. It is now part of the "No cartridge" state and gone once a game runs. |

### Input

Which key or controller button drives which GBA button lives in
[`controls.js`](../crates/tinybird-web/src/assets/controls.js), apart from the
emulator loop and from the DOM. It answers three questions — what action is
this key, what actions are the pads holding, and what are the bindings — which
is what lets it be tested against plain objects instead of a browser.

Keyboard and controller state are held **separately** and combined each time
either changes. Merged into one set, releasing a key would cancel a button
still held on a pad, which is what happens to anyone playing with a controller
who brushes the keyboard.

Controllers are polled from the frame loop rather than on a timer of their own:
the Gamepad API reports no events for presses, and a controller read that is
out of step with the frames being run is a controller that feels late.

Face buttons map by position, not by letter. A GBA's A sits under the right
thumb, which on a standard pad is button 1 — mapping it to the pad's "A"
(button 0) would put the two buttons in the wrong places under the same
fingers.

Bindings persist per browser. Anything stored by an older version of the page
is repaired rather than trusted: unknown actions are dropped and malformed
entries fall back to their defaults, so an upgrade never leaves someone unable
to press A.

**Frame rate and status sit above the deck, not below it.** They are what you
glance at while playing, and at the foot of a tall page they were usually
scrolled out of view. The strip has a fixed height so a message arriving never
nudges the screen down mid-game.

**Addons left, game centre, cartridge and storage right.** The rails are fixed
widths and the centre is fluid, so the screen takes every pixel the rails do not
need and stays optically centred. Placement is by named grid area rather than
source order: the screen comes first in the markup because it is what the page
is for, and that is the order both a keyboard and a narrow viewport should get.
Below 1180px the addon rail drops full width under the screen; below 900px
everything stacks.

The addon rail always says something. With no addon claiming the cartridge it
explains what an addon is, rather than leaving a blank stripe.

**The picture fills the bezel, and the whole-pixel step happens off screen.**
A fractional scale over a 240x160 canvas makes some source pixels two device
pixels wide and others three, and that uneven ripple is what made an emulator
look worse than the hardware — but scaling by whole numbers and letterboxing
the remainder threw away most of a step, running a 690px column at 2x with
black either side. So `fitScreen` does both: the frame is blown up by a whole
number onto a backing store large enough to cover the bezel in device pixels,
and the CSS size fills the bezel exactly, leaving the browser one fractional
step it resamples smoothly. Frames land on a 240x160 `frameCanvas` first,
because `putImageData` ignores scaling — and that canvas is what screenshots
and recovery thumbnails read, so a capture is always the picture the game drew. A `ResizeObserver` on the bezel re-runs it, so the
window and full screen are the same code path.

> **Every element gets one name in the `el` map.** A duplicate key in an object
> literal is silent in JavaScript: the last one wins. Adding a second `screen`
> image under a key that the console's `screen` bezel already used
> meant `startRom` set `data-mode="running"` on a hidden thumbnail, so the
> picture stayed at 240x160 with "no cartridge" over it while the game ran
> perfectly behind it — with no error anywhere to point at it. Two tests in
> `tinybird-web` now guard the map: one rejects a repeated key, the other
> rejects an id the page has not got.

- **Type.** Silkscreen, a true bitmap face, for headings and the cartridge
  label only — it is not legible enough for body text. Everything else is IBM
  Plex Mono, because nearly all the content is machine data (game codes, HP
  values, byte counts) and a mono face makes those columns line up.
- **Colour.** Amber marks identity, teal marks values read out of the running
  game. Both are inherited from the desktop app so the two frontends read as
  one product.
- **No rounded corners anywhere.** A GBA is a moulded brick and a grid of square
  pixels; rounding would fight both.

### Speed and sound

**The emulator is paced against the wall clock, not against the display.** It
used to run one emulated frame per `requestAnimationFrame`, which ties the game
to whatever the monitor does: correct on a 60 Hz panel, **2.4 times too fast on
a 144 Hz one**, with the audio pitched to match.

The rate is the hardware's: 16.78 MHz over 280,896 cycles a frame, or
**59.7275 Hz** — not 60. The difference sounds pedantic and is not: rounding to
60 drifts a frame every few seconds, which audio notices.

The policy lives in [`pacing.js`](../crates/tinybird-web/src/assets/pacing.js),
free of any DOM or emulator reference so it can be checked without a browser:

```bash
node --test crates/tinybird-web/src/assets/pacing.test.mjs
```

Those tests simulate 60 Hz, 144 Hz, and 30 Hz displays and assert the same
59.7275 frames a second comes out of all three. The clock advances by exact
frame steps rather than snapping to the current time, and it is that fractional
carry that keeps the average right instead of rounding to the callback rate.

**Catching up is capped at four frames.** A backgrounded tab stops receiving
callbacks; without a cap the first one after it returns would try to run every
frame it missed, which takes longer than a frame and leaves it further behind
still. Past the cap the missed time is given up rather than carried.

| Control | What it does |
|---|---|
| `Fast forward` button, or hold `Space` | runs at the selected multiple |
| Fast forward: 2× / 4× / 8× | paced, just faster |
| Fast forward: Unlimited | runs frames for a slice of wall time per callback |
| `Full screen` button, or `Escape` to leave | gives the picture the whole display |
| Volume | 0–100, remembered per browser |

**Full screen takes the bezel, not the canvas.** Requesting it on the canvas
would let the browser stretch the picture to the display's shape; requesting it
on the frame around it keeps the scanlines, the boot flash and the whole-pixel
scaling, and the picture stays centred on black. On a 1600x1000 display that is
6x — 1440x960 with the remainder letterboxed, the same rule the windowed view
follows.

**Audio only plays at normal speed.** The samples carry no timing of their own,
so handing them over while running at four times the rate would queue four
seconds of audio for every second of play. The status line reports the real
multiple achieved (`73.8 fps · 1.2×`), which is honest about a machine that
cannot reach the speed asked for.

The unlimited budget is deliberately longer than a display interval. It is
checked *after* each frame, so a budget below the cost of one frame yields
exactly one frame per callback — which is not unlimited, it is the refresh rate
wearing a different label.

### The signature element

The cartridge label. The emulator reads the real 192-byte GBA header, and the
addon registry decides which game module claims a ROM by matching the first
three characters of the game code and reading the region from the fourth. So the
code is set large and split at exactly that boundary — `BPR` then an amber `E`.
It is the one piece of typography allowed to be loud, and it is showing
something true rather than decorating.

### The overlay, in the same page

`Stream overlay` on the play page embeds `/overlay/full` in an iframe and feeds
it snapshots from the emulator running in that same page, over `postMessage`.

It is the **same overlay page OBS loads**, not a copy. The overlay accepts a
pushed snapshot as an alternative to polling `/api/snapshot`, and stops polling
once it receives one, so there is no second renderer to keep in step. A change
to the overlay shows up in both places.

```
play page                         overlay iframe
  emulator ──snapshot──▶ postMessage ──▶ renderSnapshot()
                                            ▲
OBS ─────▶ /overlay/full ──poll /api/snapshot┘   (desktop app's export)
```

The frame is grown to its content height on each update, because the overlay
changes height as a battle starts or ends and a fixed height clips the party
cards.

### The local library

`GET /api/local` lists ROMs in the ROM folder **and one level of subfolders**,
because dumps are conventionally kept one folder per game. One level, not a full
walk: it finds them all without the cost of descending an arbitrary tree, and
anything deeper is still loadable through the file picker.

`/local/{*path}` serves them. The scan is the allowlist — a request is answered
only if its path matches something the scan produced — so the wildcard route
cannot be walked upwards however it is spelled. Paths are percent-encoded in the
listing, since real dump names carry spaces, `&`, and brackets.

### Switching cartridges

Loading a second ROM is a power cycle, not just a new file. The bus clears work
RAM, video memory, and the I/O registers, and the DMA and timer units are
rebuilt; the cartridge, the BIOS image, and battery-backed save data all stay.

This is worth stating because it was wrong: `reset()` used to roll back only the
CPU and PPU, so every byte of RAM and every I/O register kept the previous
game's values. Switching cartridges left the old game on screen while the new
one booted onto hardware it had never configured — and since the peripherals
kept their own copies of the control registers, clearing I/O alone would not
have stopped a transfer the last game armed. Every ROM pair was affected, in
both directions. `a_switched_rom_runs_like_a_cold_boot` in `gba.rs` pins it: the
same frames must produce the same machine whether the ROM was switched into or
booted cold.

### Linking to a game

`/play?rom=<url>&name=<label>` boots straight into a ROM: one from the library
(`/local/...`) or any URL the browser can fetch.

### Cartridge saves are not save states

A save state is a photograph of the whole machine. A **battery save** is what
the cartridge itself keeps, and it is what the game writes when the player picks
"save" from a menu. They are not interchangeable, and a page that persists only
save states loses progress the game believes it has already committed — which is
what a game means when it reports a slot as corrupt or missing.

The browser keeps both:

| Where | What |
|---|---|
| `localStorage`, keyed by game code | the battery save, written whenever the game touches the cartridge |
| inside the automatic recovery checkpoint | the battery save, alongside the state, the ROM and a screenshot |

Restoring either one restores both halves together. `tb_battery_dirty()`
consumes a flag rather than reporting a level, so the page copies the save only
when the game has actually written to it rather than several times a second.

All four GBA backup types work: SRAM, flash 64K, flash 128K, and EEPROM.

### Saves from the hosted site

States downloaded from the hosted site's vault are wrapped in a small container
(`TBCT`, a thumbnail, then the state, optionally gzipped). `unpackSave` in
`saveformat.js` accepts that, a bare gzip blob, or a raw state, so any of them
loads here. Its suite runs anywhere Node does:

```bash
node --test crates/tinybird-web/src/assets/saveformat.test.mjs
```


## 5. Verifying it

The emulator can be driven without a browser, which is the quickest way to tell
an emulation bug from a page bug:

```bash
node - <<'EOF'
import { readFileSync } from "node:fs";
const { instance } = await WebAssembly.instantiate(
  readFileSync("target/wasm32-unknown-unknown/release/tinybird_wasm.wasm"), {});
const e = instance.exports;
e.tb_init();
const rom = readFileSync("roms/game.gba");
const ptr = e.tb_alloc(rom.length);
new Uint8Array(e.memory.buffer, ptr, rom.length).set(rom);
e.tb_load_rom(ptr, rom.length);
for (let i = 0; i < 600; i++) e.tb_run_frame();
e.tb_refresh_snapshot();
console.log(Buffer.from(new Uint8Array(
  e.memory.buffer, e.tb_snapshot_ptr(), e.tb_snapshot_len())).toString());
EOF
```

Measured this way the core runs at roughly 98 fps under Node and a steady
60 fps in a browser tab, on a 16 MB ROM.

The save container has its own suite, which runs anywhere Node does:

```bash
node --test crates/tinybird-web/src/assets/saveformat.test.mjs
```

It exists because the format shipped with a magic number that collided with the
core's, and the cheapest proof that a container and a save state stay
distinguishable is to feed it both. Point it at a real state to check the whole
path end to end:

```bash
node --input-type=module -e '
import { readFileSync } from "node:fs";
const { unpackSave, gzip, packSave } =
  await import("./crates/tinybird-web/src/assets/saveformat.js");
const raw = readFileSync(process.argv[1]);
const c = await gzip(raw);
const packed = packSave(c.bytes, c.gzipped, new Uint8Array([0x89, 0x50]));
const back = Buffer.from(await unpackSave(packed.buffer));
console.log("round trip:", back.equals(raw));
' path/to/game.state
```

---

### Where the frame time goes

Measured in the browser with FireRed running, all figures per call, before the
November 2026 PPU change described below took about 14% off `runFrame`:

| | ms |
|---|---|
| **`emu.runFrame`** | **11.8** |
| `emu.snapshot` (read + JSON parse) | 1.9, four times a second |
| `JSON.stringify(sections)` (the redraw check) | 0.016 |
| `emu.frameView`, `emu.takeAudio` | ~0 |

A GBA frame is 16.74ms, so emulation alone is about 70% of the budget and
everything else on the page is rounding error. That one number explains what
look like two separate problems:

- **Fast forward does not reach its multiplier.** 4x needs four frames inside
  one frame's time, or 4.2ms each. At 11.8ms the ceiling is about 1.4x, which
  is what both `4x` and `Unlimited` measure at. The pacing is not broken; there
  is nothing left to pace with.
- **The occasional drop.** The headroom left cannot always absorb a garbage
  collection or a browser repaint.

Things measured and found *not* to be the problem: the read-out (its redraw
check costs 16 microseconds and is skipped when nothing changed), the pixel and
audio handoff, and the snapshot — the largest non-emulation cost and still
under 1% of a second.

`cargo test --release -p tinybird-core --test throughput -- --ignored --nocapture`
measures the same thing natively, where a profiler can see inside it.

#### What has been tried

**Batching the APU tick: kept, worth 19% in the browser.** `Apu::tick` walks
four channels, the frame sequencer and the sample generator on every call, and
it was called once per *instruction* — a hundred thousand times a frame,
usually with one to five cycles — when the chip only emits a sample every ~380
cycles. Accumulating cycles and flushing every 64 (or before any register write
reaches the chip, so a frequency change is still heard at the right point) took
the browser from 14.5ms to 11.8ms and fast forward from 1.2x to 1.4x.

Native only improved 9.0ms to 8.5ms, about 6%. The gap is the interesting part:
WebAssembly pays more per call than native does, so cutting the call count
helps it roughly three times as much. **Measure the browser, not the desktop,
when the browser is what feels slow.**

The flush happens at the end of `run_frame`, not at the drain sites. Every
caller runs a frame and then drains, so settling there means none of them has
to know the chip is batched — a buffer short by up to a batch, with the gap
landing somewhere different each time, is not a bug worth leaving lying around.

**Cross-crate inlining: no effect on speed, kept for size.** A release profile
with `lto = "fat"` and `codegen-units = 1` left frame time unchanged inside
noise. It made the wasm module 14% smaller at the time (610KB to 523KB), which
is worth having on a page loaded over a network, so it stayed for that reason
and the comment in `Cargo.toml` says so. The module has since grown to about
910KB with save states, addons and game readers, 291KB gzipped. 143KB of that
is the function-name section; stripping it would save only 20KB after
compression and cost readable stack traces when the module traps, so it stays.

**Audio was never a third of the frame.** An earlier note here put audio at
2.7ms of an 8.5ms native frame. The "audio off" measurement booted the
cartridge cold and timed the intro, while the "audio on" one restored an
overworld save state, so the gap was mostly two different workloads. On the
same scene the two are indistinguishable inside noise, and a profile puts the
whole APU under 2% of a frame. `tests/throughput.rs` now runs both from the
same state.

**The PPU's per-instruction entry: kept, about 11% native and 14% in the
browser.** The first real profile (November 2026) found 8% of all samples in a
block of about forty address computations at the top of `Ppu::step_cycles`.
With the scanline renderer inlined into it, the compiler set those up on entry,
and `step_cycles` runs after every instruction while only a few hundred calls a
frame cross a boundary. It now has an inlined fast path that just advances the
cycle counter, and the loop that renders lives out of line. Native went from
about 10.3ms to 9.0ms on the FireRed overworld scene; Node, as a stand-in for
the browser, from 14.3ms to 12.3ms.

**The timer prescaler divide: kept, small.** The hottest single instruction in
that profile was a hardware `div` in `TimerController::tick`, dividing by a
prescaler read from a table. The prescalers are powers of two, so it is a shift
and a mask now. The effect is below what this machine can measure, but it is
strictly less work for the same result.

Both changes were checked by running the old and new core side by side from the
same state for 1,000–1,500 frames on FireRed, Emerald, The Minish Cap and FFTA
and comparing a hash of every frame and every audio sample: identical.

#### How to profile

On Windows without an elevated shell, ETW-based profilers (WPR, xperf, samply)
are not available. What worked: a small harness that runs frames on one thread
while a second thread suspends it about every half millisecond, reads the
instruction pointer with `GetThreadContext`, and resumes it. Build with
`debug = "line-tables-only"`, then resolve the addresses afterwards with
`llvm-symbolizer --inlining` against the PDB. Attribute samples to source lines
rather than functions: fat LTO inlines nearly everything into `Gba::step`, so a
function-level view says almost nothing.

Frame-time A/B tests need interleaved runs and the best of several, and even
then a busy desktop swings 10% between rounds. Thread CPU time
(`GetThreadTimes`) is steadier than wall time.

#### What is left

FireRed in the overworld runs about 85,000 instructions a frame and never
halts, so the cost is per instruction: roughly 100ns each natively. From the
profile, in rough order:

- **The CPU** (about 35%): decoding into an `Instruction` on every execution,
  and register access going through a mode-to-bank lookup on every read and
  write (`mode_to_bank_index` alone is about 1.7%). An active register array
  swapped on mode change would remove the lookup, but `Registers` is part of the
  save state, so it needs a migration.
- **Timers** (about 6%): every instruction loops over four timers and writes
  each enabled counter back into the I/O mirror. Settling them lazily (when one
  is due to overflow, when the game writes a timer register, or when something
  reads one) would remove nearly all of it. The I/O mirror's `TMxCNT_L` slots
  hold both the written reload value and the copied counter, which also means
  a reload written while a timer runs is lost; hardware keeps it for the next
  overflow.
- **Bus access timing** (about 6%): `record_cpu_access`, the wait-state lookup
  and address masking run on every access, through `Cell`s.
- **`step()` bookkeeping** (about 10%), including the opcode read in
  `current_hle_swi_comment` on every instruction.

---

## 6. Known gaps

Nothing outstanding on the five local dumps: Fire Red, Pokémon Pinball, Final
Fantasy Tactics Advance, The Minish Cap, and Super Mario Advance 4 all boot,
render, and save. Two emulator bugs that used to sit here are written up in
[ARCHITECTURE.md](ARCHITECTURE.md#two-bugs-worth-remembering).

- Sections carry pictures only where their addon named one per card. A
  `KeyValue` or `Table` section has no way to hold an image, so a team summary
  is text even when the party it summarises is not.
- **The stream overlay is switched off.** It still reads the desktop app's JSON
  export rather than the emulator in the page beside it, so on a server run by
  itself it shows a snapshot that never changes, and it has had far less use
  than the rest of this. `/overlay/*` serves a note saying so, the deck's
  "Stream overlay" toggle is hidden, and the nav link is gone — none of the
  code was deleted. `TINYBIRD_WEB_OVERLAY=1` brings all of it back for anyone
  working on it. Pointing it at the wasm snapshot is what would make it worth
  switching on again.
- Gamepad axes other than the left stick are ignored, so a pad with only a
  right stick or a hat switch reports no direction.
- The page does not load the server's `addons/` folder. Local add-ons live in
  browser storage, built in the Workshop or imported as JSON; the folder is what
  the desktop app reads, and `/api/addons` serves it for other tools.
- Two players cannot link in the browser. The lockstep engine that would do it,
  `link.js`, is here and tested against two real wasm instances, but carrying
  input between browsers needed the hosted lobby. Two consoles in one tab would
  need no server at all.
- **Trading still stops short of a Pokémon changing hands.** The cable itself
  is done — the handshake completes, both players reach the same Trade Center,
  and position data crosses in both directions
  ([ARCHITECTURE.md](ARCHITECTURE.md#testing-it-with-a-real-cartridge)). What is
  missing is the last step: opening the trade menu needs a button pressed on an
  exact tile, which is a puppeteering problem rather than an emulation one.
- Species sprites need one network fetch each the first time a party is seen.
  After that they are on disk and the page works offline; a host that never had
  a network shows cards with no pictures, which is a supported way to run.

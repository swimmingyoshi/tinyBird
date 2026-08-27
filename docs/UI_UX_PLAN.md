# tinyBird UI/UX Plan — Classic Shell + Stream Dashboard

Last reviewed: 2026-08-23
Companion to `docs/ARCHITECTURE.md`. This is the working spec for phases 1–6.

---

## 0. The Goal In One Paragraph

tinyBird currently has one UI: a bespoke, keyboard-only, stream-oriented
dashboard driven entirely by function keys. It looks good, and it stays. What is
missing underneath it is the **boring, conventional emulator shell** that people
expect: a menu bar, mouse-clickable dialogs for opening ROMs and managing save
states, a settings window with tabs, and settings that survive a restart. The
plan is to build that classic shell as a separate, optional layer, so the
dashboard becomes one *view* inside a normal application rather than the whole
application.

---

## 1. Two Layers, Explicitly Separated

```
+-------------------------------------------------------------+
|  File  Emulation  Video  Audio  Addons  Tools  Help          |  <- classic shell
+-------------------------------------------------------------+
|                                                             |
|                   game viewport / dashboard                 |  <- content
|                                                             |
+-------------------------------------------------------------+
|  status bar: ROM title | 60.2 FPS | 1x | vol 80%  [slot 3]  |  <- classic shell
+-------------------------------------------------------------+
```

- **Classic shell** — menu bar, status bar, modal dialogs. Mouse-first,
  fully keyboard-navigable. Always available regardless of which content view
  is active.
- **Content** — either the plain scaled game viewport, or the stream dashboard
  with the game inset and addon panels around it. Unchanged in spirit from
  today.

The shell is optional. `Settings.ui.chrome` has three values (Tools > Menu Bar):

| Value | Behaviour |
|---|---|
| `Always` | Menu bar and status bar always drawn. Default for a fresh install. |
| `Auto` | Hidden until the pointer moves within 4px of the top edge, or `Alt`/`F10` is pressed. Hides again on menu close plus a short delay. |
| `Off` | Never drawn. Pure fullscreen/stream mode — today's behaviour. |

Entering fullscreen implies `Auto` for the session unless the user has chosen
`Off`.

---

## 2. Menu Structure

Accelerators listed are the *existing* bindings where one exists; the menu is
the discoverable path, the key stays the shortcut.

### File
- `Open ROM...`  `Ctrl+O`
- `Open Recent` &raquo;  (up to 10, from settings; `Clear List` at the bottom)
- `Reload ROM`
- ---
- `Save State` &raquo;  (`Slot 1..5`, each showing its timestamp or `<empty>`)
- `Load State` &raquo;  (same, empty slots disabled)
- `Save State Manager...`  `F5`
- ---
- `Take Screenshot`
- `Open Data Folder`  (reveals `stream-data/`)
- ---
- `Exit`

### Emulation
- `Pause` / `Resume`  `Esc`   (checkable)
- `Reset`  `Ctrl+R`
- ---
- `Speed` &raquo;  (`1x` / `2x` / `4x` / `Unlimited`, radio group)
- `Fast Forward (hold)`  `Tab`
- ---
- `Frame Advance`  `Ctrl+.`   *(new; needs core support, phase 6+)*

### Video
- `Fullscreen`  `F11`  (checkable)
- ---
- `Window Scale` &raquo;  (`1x` / `2x` / `3x` / `4x` / `Fit`, radio group)
- `Integer Scaling`  (checkable)
- ---
- `LCD Color Correction`  `C`  (checkable)
- `Show HUD`  `F1`  (checkable)

### Audio
- `Mute`  `M`  (checkable)
- `Volume Up` / `Volume Down`  `]` / `[`
- `Volume` &raquo;  (`0%`..`100%` in tens, radio group)

### Addons
- `Dashboard`  `F6`  (checkable)
- `Team Panel`  `F2`
- `Encounters Panel`  `F4`
- `Hide Addon UI`  `F3`
- ---
- `Dashboard Layout` &raquo;  (Classic / Left Info / Split Sidebars / Cozy Game)
- `Game Size` &raquo;  (Compact / Balanced / Game Focus)
- `Side Panel Width` &raquo;  (Slim / Normal / Wide)
- `Theme` &raquo;  (4 themes)
- `Choose Wallpaper...`  `W`
- ---
- `Export Snapshot JSON`  (checkable)

### Tools
- `Settings...`  `Ctrl+,`
- ---
- `ROM Information...`  (header dump: title, game code, maker, revision, size)
- `Addon Status...`  (which addon matched, why, capability list)

### Help
- `Controls...`
- `About tinyBird...`

**Note on `Tools > Addon Status`:** this is the single most useful thing we can
add for testing new games. It answers "why is this ROM showing nothing?" without
a debugger.

---

## 3. Interaction Rules

These are the conventions people already have muscle memory for. Match them
exactly; do not invent.

**Mouse**
- Click a menu title to open it. Click again (or click elsewhere) to close.
- While *any* menu is open, hovering a different title switches to it without a
  click.
- Hovering a submenu item opens the submenu after ~250ms; moving off closes it
  after a short grace period.
- Click an item to activate and dismiss the whole menu chain.
- Click-drag from title to item, releasing over the item, also activates.

**Keyboard**
- `Alt` or `F10` opens the first menu (or closes an open one).
- `Left`/`Right` move between menus, `Up`/`Down` between items, wrapping.
- `Right` on a submenu parent opens it; `Left` closes back to the parent.
- `Enter`/`Space` activates. `Esc` closes one level.
- Disabled items are skipped by arrow navigation.
- **While a menu or dialog is open, GBA input is suppressed.** This is a real
  bug risk: today `handle_key` inserts into `keyboard_buttons` *before* checking
  menu state for anything except the two existing menus.

**Dialogs**
- Modal. Dim the content behind them (`dim_screen` already exists).
- `Esc` cancels, `Enter` confirms the default button.
- `Tab`/`Shift+Tab` cycle focus; focused control gets a visible ring.
- Every dialog has explicit `OK` / `Cancel` buttons even when it is keyboard
  driven — that is the point of the classic shell.

---

## 4. Settings Dialog

Tabbed, because that is what people expect. Tabs on the left, content on the
right, `OK` / `Cancel` / `Apply` at the bottom.

| Tab | Contents |
|---|---|
| General | Chrome mode (Always/Auto/Off), confirm-on-overwrite, pause when window loses focus, recent-ROM count |
| Video | Scale, integer scaling, color correction, HUD, fullscreen-on-launch |
| Audio | Volume slider, mute, mute-on-fast-forward, buffer target |
| Input | Keyboard binding table (click a row, press a key), gamepad binding table, deadzone slider |
| Addons | Enable/disable per addon, export JSON toggle, export path, sprite cache path, dashboard defaults |
| Paths | BIOS path, ROM folder, save folder, state folder, wallpaper |

The Input tab is the one that pays for itself — the current hardcoded
`Z/X/A/S/Enter/Space/arrows` map is not negotiable at runtime, and `A`/`S`/`W`/`Q`
being GBA shoulder buttons is exactly why single-letter shortcuts like `W`
(wallpaper) and `C` (color correction) are a problem.

---

## 5. Widget Set To Build

Small and boring on purpose. All immediate-mode, all hit-tested against a
`Rect`, all drawn with `Canvas`.

```rust
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

pub struct UiInput {
    pub pointer: Option<(i32, i32)>,
    pub pressed: bool,        // left button currently down
    pub clicked: bool,        // left button released this frame
    pub wheel: f32,
    pub keys: Vec<KeyPress>,  // consumed by focused widget
}
```

- `button(rect, label) -> bool`
- `toggle(rect, label, &mut bool) -> bool`
- `radio_row(rect, labels, &mut usize) -> bool`
- `slider(rect, range, &mut f32) -> bool`
- `list(rect, items, &mut selected) -> Option<usize>`  (scrollable, wheel-aware)
- `tab_strip(rect, labels, &mut usize) -> bool`
- `text_field(rect, &mut String) -> bool`  (paths only; lowest priority)

Immediate mode is the right call here: the app already redraws every frame, and
retained widgets would mean a second source of truth alongside `App`'s fields.

---

## 6. Ordered Work Items

Each is independently shippable and leaves `cargo check` green.

1. ~~**`ui/font.rs` + `ui/canvas.rs`**~~ — **done.** `Canvas` owns the buffer,
   a clip rect, and an origin; `sub()` gives a panel local coordinates.
   `overlay.rs` still has its own copies of the old primitives and keeps
   working; retiring those is folded into item 9 below.
2. ~~**`ui/theme.rs`**~~ — **done.** `Palette` holds role-named tokens and
   `THEMES` is a static table. The four existing themes are preserved.
3. ~~**`settings.rs`**~~ — **done.** `Settings` + `SettingsStore` (dirty
   tracking, write-on-change) at `%APPDATA%/tinyBird/settings.json` or the XDG
   equivalent. Every field is `#[serde(default)]` and values are clamped on
   load, so a stale or hand-edited file cannot stop the app from starting.
4. ~~**Mouse plumbing**~~ — **done.** `CursorMoved`, `CursorLeft`,
   `MouseInput`, `MouseWheel`, and `ModifiersChanged` feed `UiInput`.
   A click requires press *and* release in the same rect.
5. ~~**`ui/widget.rs`**~~ — **done.** button, checkbox, radio row, slider,
   scrollable list, tab strip, panel, label. Geometry is factored into pure
   functions so it is tested without a frame buffer. Waiting on item 7 for a
   consumer.
6. ~~**`ui/menubar.rs`**~~ — **done.** `Menu`/`MenuEntry`/`EntryKind`/`Mark` as
   data, one renderer, one hit-tester, emitting `UiCommand`. Mouse and keyboard
   navigation both covered by tests that drive a real draw pass.
7. **`ui/dialog.rs`** — modal stack, then port the existing save-state and theme
   menus onto it, then add the Settings dialog. **Next up.**
8. ~~**Single-path composition**~~ — **done.** `blit_game` +
   `compose_layers`, called once. `game_dest_rect` is pure and tested.
9. **Retire the duplicated primitives in `overlay.rs`** — port its draw
   functions onto `Canvas` and delete its private `FONT`, `fill_rect`,
   `draw_text`, `lerp_color`, and `alpha_blend`.

---

## 7. Explicit Non-Goals For This Block

- No GPU renderer. `softbuffer` stays. Everything is CPU rasterised into the
  same `u32` buffer.
- No external UI toolkit (egui, iced). They would fight the existing pixel-art
  aesthetic and the frame-pacing loop, and the widget needs here are tiny.
- No redesign of the dashboard's visual language. The dashboard keeps its look;
  it only gains a normal application frame around it.
- No changes to `tinybird-core`.

---

## 8. Progress Log

Append entries here as work lands. Newest last.

- **2026-08-23** — Assessment complete. Documented current state in
  `docs/ARCHITECTURE.md` and this plan. Confirmed by inspection: no mouse
  events are handled anywhere in the workspace, no config file exists, and
  `present_buffer` duplicates its UI composition block. Baseline
  `cargo check --workspace` passes.

- **2026-08-23** — Phases 1–4 and 6 landed. The classic shell is live: a menu
  bar with seven menus, a status bar, mouse input, and persisted settings.

  Verified in the running app against Pokemon FireRed:
  - The menu bar and status bar draw, and the game viewport is inset between
    them rather than overlapped.
  - Clicking a title opens its dropdown; sliding across the bar switches menus
    without a second click; hovering a submenu parent opens the submenu.
  - Disabled entries render greyed (`Hide Addon UI` with no panel shown),
    check marks render (`Export Snapshot JSON`), radio marks render
    (`Theme > Midnight`), and shortcut hints are right-aligned.
  - `settings.json` is written to `%APPDATA%/tinyBird/` and the auto-loaded
    ROM appears in `recent_roms`.

  Behaviour changes worth knowing:
  - **`F10` now opens the menu bar** (with `Alt`), replacing "cycle side panel
    width". That setting moved to `Addons > Side Panel Width`.
  - `Ctrl+O` / `Ctrl+R` / `Ctrl+S` / `Ctrl+,` are new shortcuts. All previous
    plain-key bindings still work.
  - Key *presses* are withheld from the emulator while a menu is open, but
    releases always get through — otherwise opening a menu with a direction
    held would leave that button stuck down.

  Also verified: clicking `Addons > Theme > Cobalt` applied the theme, showed
  the toast, and wrote `dashboard.theme: 2` to `settings.json` — the whole
  menu -> `UiCommand` -> `App` -> persistence path end to end.

  Tests: `tinybird-desktop` went from 14 to 90; workspace `cargo test` green
  (`tinybird-core` still 208 passed / 1 ignored). `cargo clippy` reports no
  findings in `ui/`, `shell.rs`, or `settings.rs`; the pre-existing backlog in
  `overlay.rs` and `tinybird-core` is untouched.

- **2026-08-23** — Multi-game addon support (phases 8–9). The addon layer is now
  a public extension point rather than a hardcoded array of one.

  - `tinybird-addons` gained `MemoryView` (the read-only window an addon gets),
    `SparseMemory` (so addons are tested without booting a ROM), and
    `GameAddon` / `AddonRegistry` / `Detection`.
  - The desktop draws the schema's generic `key_value` / `list` / `table`
    sections when it has no per-game renderer, and a `cartridge` addon claims
    every ROM so nothing shows an empty panel.
  - New `tinybird-probe` CLI for finding addresses in a running game:
    relative text search (finds names under an unknown encoding), stride
    testing, savestate diffing, screenshots. See `docs/ADDON_DEVELOPMENT.md`.
  - New Final Fantasy Tactics Advance addon, verified end to end: with the
    tutorial battle on screen showing `Ritz HP 16/16 MP 10/10`, the export
    reports three units at `16/16`, `10/10`, `10/10` and the player name.
  - Desktop gained `--state PATH` to boot straight into a savestate.

  UI fixes this exposed:
  - The generic panel reused FireRed's side-by-side layout, so the centred game
    preview painted over it. Non-rich addons now get their own right-anchored
    card with a capped width, because a key/value row spread across 600px reads
    as two unrelated columns.
  - The panel subtitle showed the raw 12-byte header title, which for FFTA is
    the build tag `FFTA_USVER.`. It now shows game code and region.
  - A discovered wallpaper silently overrode an explicit theme setting.

  Tests: 400 across the workspace (addons 23, core 212, desktop 143, probe 20,
  web 2).

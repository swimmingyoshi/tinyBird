# tinyBird Addon Architecture

Addons read live state out of a running game and publish it through one stable
contract, so a stream overlay, a bot, or a web client can consume any supported
game without knowing anything about it.

For the practical "how do I write one" guide, see
[`docs/ADDON_DEVELOPMENT.md`](docs/ADDON_DEVELOPMENT.md).

---

## Shipped addons

| Addon id | Game | Reports |
|---|---|---|
| `pokemon_frlg_party` | Pokemon FireRed / LeafGreen (`BPR*`, `BPG*`) | two sections: **party** with species sprites (nature, ability, held item, stats, IVs, EVs, moves and PP, status, team summary in the caption) and **dex**, which is the opponent while a battle is on and the area's encounters otherwise, with an IV flag on the tab |
| `ffta_clan` | Final Fantasy Tactics Advance (`AFX*`; live data USA only) | player name, unit HP/MP with wounded flags |
| `cartridge` | any ROM | header, region, maker, checksum, boot logo |

`cartridge` is registered last and claims everything, so an unrecognised game
shows its cartridge details rather than an empty panel.

---

## The three pieces

`tinybird-addons` is the extension point and has no dependency on the emulator,
so `tinybird-web` can consume the schema without pulling in `tinybird-core`.

### `schema` — the wire format

```rust
StreamSnapshot { schema_version, rom, addon }
AddonSnapshot  { addon_id, display_name, version, capabilities, sections, overlay_lines, data }
AddonSection   { section_id, title, note, content }
AddonSectionContent::{ KeyValue, List, Table, Cards }
AddonField     { label, value, meter, tone, hint }
AddonCard      { title, subtitle, image, lead, badges, fields }
AddonImage     { src, alt }
```

Every addon fills in `sections` **as well as** its typed `data`. The typed
payload drives the desktop's per-game renderers; `sections` is what everything
else draws, including the desktop's own generic renderer for games without one.

The vocabulary is small on purpose, and every part of it is drawable without
knowing the game:

| Part | What it is for |
|---|---|
| `meter` | a bounded quantity, so a consumer draws a bar instead of parsing `"9/36"` |
| `tone` | `good` / `warn` / `bad` — the addon's reading, because only it knows that 4 HP of 50 is critical and 4 PP of 5 is fine |
| `hint` | the detail that did not fit in the value: a stat spread, a source address, a caveat |
| `note` | one line under a section title saying what it is reading |
| `Cards` | several things of the same shape with more to say than one row each |
| `badge` (on a section) | a short flag drawn on the section's tab, for something worth leaving another section to look at. Use sparingly: a flag that is always there is a flag nobody sees |
| `image` | a picture for a card, named as a path the host resolves — the addon never decides where pictures come from |

A `Cards` section names its headline (`lead`), its flags (`badges`), and its
detail (`fields`) separately, which is what lets each consumer lay it out for
its own medium — the web rail collapses the detail behind a click, the stream
overlay draws all of it, the desktop overlay prints it as text.

A card's `image` is a **path, not a URL**: the FireRed addon says
`/sprites/32`, and the host decides what that means. `tinybird-web` serves it
from `crates/tinybird-web/src/sprites.rs`, which fetches the sprite once and
caches it to disk so later runs work offline. This keeps the addon a thing that
only reads memory, and it means a host with no network — or no interest in
pictures — simply 404s and every consumer falls back to the card's words, which
is why `alt` exists.

`AddonTone::from_fraction` is the shared default reading of "how much is left",
so a health bar means the same thing in every game.

### `memory` — what an addon is given

```rust
pub trait MemoryView {
    fn read_u8(&self, addr: u32) -> u8;
    fn read_u16(&self, addr: u32) -> u16;
    fn read_u32(&self, addr: u32) -> u32;
    fn read_bytes(&self, addr: u32, len: usize) -> Vec<u8>;
}
```

Read-only, infallible, no emulator handle. Unmapped addresses read as zero.
`SparseMemory` implements it over sparse regions so addons are unit-tested
against a handful of bytes rather than a booted ROM.

### `registry` — discovery

```rust
pub trait GameAddon<T>: Send + Sync {
    fn info(&self) -> AddonInfo;
    fn supports(&self, rom: &RomIdentity) -> bool;
    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot<T>>;
}
```

`AddonRegistry::detect` walks the registered addons and returns:

| Outcome | Meaning |
|---|---|
| `Active { info, snapshot }` | an addon claimed the ROM and produced data |
| `Idle { info }` | an addon claimed it but has nothing yet (title screen, no save loaded) |
| `Unsupported` | nothing claims this ROM |

The distinction is the point: "no addon exists" and "the addon has no data yet"
look identical on screen and need completely different responses. `Tools >
Addon Status` reports which it is.

`Idle` does not stop the walk, which is what lets `cartridge` sit behind every
specific addon.

---

## Export envelope

Written to `stream-data/current-game.json` whenever it changes:

```json
{
  "schema_version": 2,
  "rom": { "title": "FFTA_USVER.", "game_code": "AFXE", "maker_code": "01", "revision": 0 },
  "addon": {
    "addon_id": "ffta_clan",
    "display_name": "Final Fantasy Tactics Advance",
    "version": "0.1.0",
    "capabilities": ["units", "player"],
    "sections": [
      {
        "section_id": "units",
        "title": "Units",
        "kind": "table",
        "payload": {
          "columns": ["Slot", "HP", "MP"],
          "rows": [["1", "16/16", "10/10"], ["2", "10/10", "10/10"]]
        }
      }
    ],
    "overlay_lines": ["Player: Marche", "Units: 3"],
    "data": { "type": "ffta", "payload": { "units": [] } }
  }
}
```

`schema_version` is a public contract. Bump `SNAPSHOT_SCHEMA_VERSION` if the
envelope shape changes.

---

## Adding a game

1. Implement `GameAddon` in `crates/tinybird-desktop/src/addons/<game>/`.
2. Add a variant to `AddonData`, or use `AddonData::Generic` if generic
   sections are enough.
3. Add one line to `build_registry()`, before `CartridgeAddon`.

Nothing else in the workspace changes. The dashboard, the JSON export, and the
web overlay all pick it up from the schema.

Use `tinybird-probe` to find the addresses —
[`docs/ADDON_DEVELOPMENT.md`](docs/ADDON_DEVELOPMENT.md) walks through the loop.

---

## Still to do

- External addon folders loaded from a local `addons/` directory with a
  manifest, so a game can be supported without recompiling.
- Per-addon enable/disable in settings.
- FFTA: identify the two unlabelled `u16` stats in the unit record, the clan
  name, and gil; verify the Japanese and European layouts.
- FRLG: **species** names still cover the early game only, so a late-game party
  can report an unnamed species. Moves are complete (all 354 of Generation 3).
- FRLG: move names and PP are a table in `pokemon_frlg.rs` rather than being
  read out of the cartridge. The ROM has both, but at addresses that move
  between FireRed, LeafGreen and their revisions, and a wrong address gives
  confident nonsense where a table gives a known answer. Finding the tables by
  searching the ROM for a known name — rather than hardcoding an address —
  would fix names, PP and species together, and is the right long-term fix.
- FRLG: encounter tables are hand-entered per map, so an area with none names
  the map it is missing rather than being read from the ROM's wild-encounter
  header.

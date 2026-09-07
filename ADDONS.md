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
| `pokemon_emerald_party` | Pokemon Emerald (`BPE*`) | the same party and battle read-out, behind Emerald's own addresses. Hoenn's encounter tables are not entered, so the dex reports the map you are on and says so rather than borrowing Kanto's |
| `pokemon_rs_party` | Pokemon Ruby / Sapphire (`AXV*`, `AXP*`) | party only. Their battle and save-block addresses are not written down, so those are switched off rather than borrowed from a game whose are |
| `ffta_clan` | Final Fantasy Tactics Advance (`AFX*`; live data USA only) | full clan with race/job portraits out of battle; deployed party with live HP/MP and wounded flags in battle |
| `cartridge` | any ROM | header, region, maker, checksum, boot logo |

`cartridge` is registered last and claims everything, so an unrecognised game
shows its cartridge details rather than an empty panel.

The two Pokemon addons share every parser: a Generation 3 party slot is the
same 100 bytes with the same four personality-keyed substructures in Emerald as
in FireRed, so only the addresses are written down twice, as a `Gen3Layout`.
Ruby and Sapphire are the same shape again, and read by the same parsers — but
they keep the party in IWRAM rather than EWRAM, and only their party is
reported. The scan is what makes that possible: it validates every candidate
before accepting one, so pointing it at the right 32KB finds a party nobody has
measured the address of. A battle cannot be found that way — it is read from
fixed addresses, and a wrong one reports a battle that is not happening, with an
opponent assembled out of whatever those bytes hold. So it is left switched off,
and the addon's capabilities say `party` rather than claiming otherwise.

### Knowing which cartridge, not just which game

`RomIdentity` carries a `fingerprint`: a hash of four kilobytes sampled across
the ROM. Everything else it holds comes from the 192-byte header, and a ROM
hack inherits every one of those fields from the game it was built on — Pokemon
Ultra Violet reports itself as `BPRE`, "POKEMON FIRE", maker 01, revision 0, in
16MB, exactly like the original.

Two things depend on telling them apart. The name tables read out of the
cartridge are cached against it, and keying that cache on the game code alone
served a hack the names of the game before it. And `pokemon_frlg_party` will
only report its compiled-in encounter tables for a dump they were entered
against: everything read from live memory follows a hack wherever it went, but
a table keyed by map number would have Route 1's original encounters drawn as
fact on a hack that changed them. An unrecognised build keeps its party and its
battles and is told the tables do not describe it.

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

## Addons written as data

**Desktop adapter and browser manifest runtime.** `crates/tinybird-addons/src/manifest.rs`, with a worked
example in `addons/example.firered-trainer.json`.

A manifest is a JSON file saying *where to read* and *what to call it*:

```json
{
  "addon_id": "custom.firered_money",
  "display_name": "Money",
  "matches": { "game_code_prefix": ["BPR"] },
  "sections": [{
    "id": "wallet", "title": "Wallet", "kind": "key_value",
    "fields": [{ "label": "Money", "read": { "u32": { "at": "0x03005008", "deref": [290] } } }]
  }]
}
```

`ManifestAddon` implements `GameAddon`, so nothing downstream can tell the
difference — the registry, the export envelope, the web read-out and the stream
overlay all see an `AddonSnapshot`. `build_registry_with(manifests)` slots them
**between** the compiled addons and the cartridge fallback: ahead of the
fallback because a real reading beats a header dump, behind the compiled ones
because a hand-written file should not quietly replace the module that decrypts
FireRed's party.

Loading the files is the host's job, and both hosts do it:

| Host | How |
|---|---|
| Desktop | `addon_manifests::install()` at startup reads `addons/*.json`. `TINYBIRD_ADDONS` moves the directory. |
| Browser | `/addons` manages local readers and account installations. `tb_install_manifests` atomically replaces the session-owned set; readers append sections alongside built-ins. See `docs/WEB_ADDONS.md`. |

Desktop installation happens once before its first snapshot. Browser installation
can happen between frames, including replacement and removal. Its validation
path uses owned metadata rather than the desktop's static registry. Invalid
replacement preserves the previous installation.

### Why this shape

The two halves of writing an addon are not equally hard. Deciding a number is
worth a row, and what to call it, is mechanical. Working out that `0x02024284`
is the party block and not a buffer that happens to look like one is the work,
and it comes from `tinybird-probe` — two savestates either side of a change,
`--diff` between them, `--find-u16` to narrow the survivors.

So the split is: **something finds the addresses, and the manifest is the cheap
half.** That cheap half is small and closed enough to be a reliable generation
target, which is the point — it is the piece a language model could write, with
the probe still doing the part that needs a running game.

A manifest naming a wrong address would otherwise produce confident nonsense,
so match exact game codes/revisions and use an optional `when` readiness condition. Numeric zero is valid data; null pointers remain idle.

### What a field can read

| `read` | What it is |
|---|---|
| `{"u8"/"u16"/"u32": "0x…"}` | A little-endian number. The address may be a `{"at":…,"deref":[…]}` pointer chain. |
| `{"text": {"at": "0x…", "len": 16}}` | ASCII, stopping at the first byte that is not. |
| `{"literal": "Slot"}` | A fixed string, for a heading the game does not store. |
| `{"index": null}` | Which repeat this is, counting from one. |
| `{"gen3_text": {"at": "0x…", "len": 10}}` | Text in Generation 3's own alphabet. A nickname sits at `record + 8`. |
| `{"gen3_species": "0x…"}` | The species of the record starting here — decrypted, un-permuted, and named from the cartridge's own table. Reads as `#21` when the tables were not found. |

Adding `"max"` to a field, in the same shape, turns it into a gauge: the
renderer draws a bar and colours it from the fraction, which is where `tone`
comes from without anyone choosing one. A `cards` section may also carry
`"image": {"gen3_species_sprite": "0x…"}`.

The two `gen3_*` reads are in `crates/tinybird-addons/src/gen3.rs` and
`gen3_names.rs`, deliberately in the shared crate rather than beside the
FireRed reader, because a manifest is evaluated in `tinybird-addons` and could
not otherwise reach them. Finding the cartridge's name tables is one pass over
the whole ROM — far outside the per-update read budget — so the host calls
`Manifest::prepare` once, and only for a manifest whose
`needs_cartridge_names()` says it will use them.

### What it cannot do yet

| Missing | Why it matters |
|---|---|
| Decryption beyond species | `gen3_species` undoes the XOR and the personality permutation, but only for the species field. Moves, IVs and EVs are in the same encrypted block and have no read of their own. |
| Arithmetic | No totals, no percentages, no derived stats. |
| Per-section conditions | A root `when` readiness condition is supported; individual sections do not yet have separate conditions. |
| Tone and badge rules | A gauge gets a tone from its fraction; nothing can flag itself the way the IV check does. |

A manifest can express a **reader**, not an **interpreter**. Per-section conditions and
simple arithmetic are useful next extensions.

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

- Manifests cannot express decryption, arithmetic, per-section conditions, or custom tone rules.
  See "What it cannot do yet" below.
- Per-addon enable/disable in settings.
- FFTA: identify the two unlabelled `u16` stats in the unit record, level, JP,
  clan name, and gil; verify the Japanese and European layouts.
- FRLG: names and PP are tables in `pokemon_frlg.rs` rather than being read out
  of the cartridge. Both are complete for Generation 3 — all 354 moves and all
  386 species — but see below for why reading them from the ROM would be
  better still.
- FRLG: PP and catch rates are still compiled tables. Names are read from the
  cartridge (see `gen3_names.rs`), but the numbers beside them live in
  differently-shaped structures — `gBattleMoves` and the species base stats —
  and would each need their own anchor to find.
- FRLG: encounter tables are hand-entered per map, so an area with none names
  the map it is missing rather than being read from the ROM's wild-encounter
  header.

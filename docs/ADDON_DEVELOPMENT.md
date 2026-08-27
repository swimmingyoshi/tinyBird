# Writing a tinyBird Game Addon

Last reviewed: 2026-08-23

An addon reads live state out of a running game and reports it as structured
data. tinyBird ships three:

| Addon | Game | Reports |
|---|---|---|
| `pokemon_frlg_party` | Pokemon FireRed / LeafGreen | party, area encounters, active battle |
| `ffta_clan` | Final Fantasy Tactics Advance (USA) | player name, unit HP/MP |
| `cartridge` | anything | ROM header, region, checksum, boot logo |

`cartridge` is registered last and claims every ROM, so a game nobody has
written an addon for still shows something useful instead of an empty panel.

---

## 1. The contract

An addon implements one trait from `tinybird-addons`:

```rust
pub trait GameAddon<T>: Send + Sync {
    fn info(&self) -> AddonInfo;
    fn supports(&self, rom: &RomIdentity) -> bool;
    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot<T>>;
}
```

Three rules follow from the signature:

- **You get memory reads and nothing else.** [`MemoryView`] has `read_u8`,
  `read_u16`, `read_u32`, and `read_bytes`. An addon cannot stall, write to, or
  otherwise perturb emulation.
- **`snapshot` runs on a timer** (every 250 ms while the game runs). Keep it to
  a bounded number of reads; do not scan all of EWRAM.
- **Return `None` when there is nothing to report yet.** Title screens, loading,
  before a save is loaded. That is reported as `Detection::Idle`, not an error,
  and lets the next addon — ultimately `cartridge` — describe the ROM instead.

Register it in `crates/tinybird-desktop/src/addons/mod.rs`:

```rust
pub fn build_registry() -> AddonRegistry<AddonData> {
    AddonRegistry::new()
        .with(Box::new(pokemon_frlg::PokemonFrlgAddon))
        .with(Box::new(ffta::FftaAddon))
        .with(Box::new(cartridge::CartridgeAddon))  // must stay last
}
```

Order matters: the first addon that both claims the ROM **and** returns data
wins.

---

## 2. Finding the addresses

This is the actual work, and `tinybird-probe` exists for it.

```
cargo build -p tinybird-probe --release
./target/release/tinybird-probe --help
```

### Step 1 — get the game to the screen showing what you want

Play in the desktop app and press `F5` to write a save state, or drive it
headlessly:

```bash
tinybird-probe game.gba --frames 20000 --mash a,start \
    --save-state /tmp/intro.state --screenshot /tmp/intro.png
```

`--mash` taps buttons on a duty cycle rather than holding them, because menus
need a press edge. `--screenshot` matters more than it sounds: a search that
finds nothing is usually a run stalled on a menu, not a wrong pattern. Look at
the picture.

Chain runs to navigate: each `--save-state` is the next run's `--state`.

### Step 2 — search for a value you can see on screen

```bash
# A number: HP, gil, level. Little-endian, decimal or 0x.
tinybird-probe game.gba --state battle.state --find-u16 16

# A known byte layout: HP 16, max 16, MP 10, max 10 as consecutive u16s.
tinybird-probe game.gba --state battle.state --find-bytes 100010000a000a00

# Text, when you know the encoding.
tinybird-probe game.gba --state battle.state --find-text Marche --codec ffta

# Text, when you DO NOT know the encoding. This is the one to reach for first.
tinybird-probe game.gba --state battle.state --find-relative arche
```

`--find-relative` matches any byte run whose consecutive *differences* match
the search word. That holds for every encoding where the alphabet is one
contiguous ascending block, which is nearly all of them, and it reports the
offset it implies so you can build a codec from the answer. It also tries
strides 1, 2, and 4, so 16-bit text is found too.

Very few GBA games store plain ASCII. FFTA stores `A` as `0xB1`, which is why
searching its ROM for `Marche` finds nothing at all.

If you have no idea what to look for:

```bash
tinybird-probe game.gba --state save.state --strings 5 --codec ffta
```

lists every decodable text run, so you can look for something recognisable.

### Step 3 — confirm the record size

One match is a coincidence until it repeats:

```bash
tinybird-probe game.gba --state battle.state \
    --find-bytes 100010000a000a00 --stride 264
```

This reads the same offset at `base + n*264` and prints whether it matches. If
the next two slots hold the other two units in the battle and the rest are
zero, the stride is right.

### Step 4 — read the struct around it

```bash
tinybird-probe game.gba --state battle.state --dump 0x0200360C:64
```

### Step 5 — identify a field by changing it

Save a state, do one thing in game (take damage, spend gil, move one unit),
save another, then:

```bash
tinybird-probe game.gba --state before.state --diff after.state
```

Differences are grouped into runs and sorted shortest-first, because a changed
counter is four adjacent bytes and a re-rendered buffer is thousands. The short
runs are the interesting ones.

### Step 6 — check it is not a transient buffer

Advance a few thousand frames and re-dump the same address. A UI scratch buffer
moves; a real game structure does not.

```bash
tinybird-probe game.gba --state battle.state --frames 2500 --save-state later.state
tinybird-probe game.gba --state later.state --dump 0x0200360C:32
```

---

## 3. Writing the addon

Put it in `crates/tinybird-desktop/src/addons/<game>/`.

**Record how you found each address.** `addons/ffta/units.rs` opens with the
exact probe commands and the on-screen values they were matched against. When a
number looks wrong six months from now, that comment is the difference between
re-deriving it in ten minutes and starting over.

**Validate before reporting.** Game memory keeps stale contents between
battles, so a plausibility check is what stops the panel showing a party that is
no longer there:

```rust
fn is_plausible(&self) -> bool {
    self.max_hp > 0
        && self.max_hp <= MAX_PLAUSIBLE_HP
        && self.hp <= self.max_hp
}
```

**Do not report guesses.** FFTA's unit record has two more `u16` fields after
the vitals that are probably attack and defence. They are not exposed, because
a guess rendered as a labelled stat is worse than no stat.

**Scope to the region you verified.** Addresses from a US build are wrong for a
Japanese one. Claim every region in `supports` so `Tools > Addon Status` reports
the game as recognised, and return `None` for the regions you have not checked:

```rust
if rom.region_code() != Some('E') {
    return None;
}
```

**Fill in `sections` as well as the typed payload.** The typed payload drives
the desktop dashboard; `sections` is what the web overlay and every other
consumer render. An addon with only a typed payload shows nothing outside the
dashboard.

**Say what a number means; do not make the consumer work it out.** A field can
carry a bar and a tone, and the builders make that a single call:

```rust
// "9/36" as text, a bar at 25%, and a warn tone, from one call.
AddonField::gauge("HP", current_hp, max_hp)

// When the addon knows something the fraction does not say.
AddonField::new("Catch chance", "good, weakened")
    .with_tone(AddonTone::Good)
    .with_hint("base catch rate 255/255")
```

Tone is the addon's judgement, and it has to be: only the addon knows that 4 HP
out of 50 is critical while 4 PP out of 5 is fine. `AddonTone::from_fraction`
is the default reading — empty is `Bad`, a quarter or less is `Warn` — so use it
unless the game says otherwise.

**Use a card when a row is not enough.** Anything the game has several of and
each of which has more to say than one line — a party member, a squad unit, an
opponent — is a `Cards` section rather than a `Table`:

```rust
AddonSection::cards("party", "Party", vec![
    AddonCard::new(nickname)
        .with_subtitle(species)
        .with_lead(AddonField::gauge("HP", hp, max_hp))
        .with_badges(vec![AddonBadge::toned("Poisoned", AddonTone::Warn)])
        .with_fields(detail),
])
.with_note("Live party block")
```

A card names which part is the headline, which parts are flags, and which are
detail, so each consumer can lay it out for its own medium: the read-out rail
collapses the detail behind a click, the stream overlay draws it all at once,
and the desktop overlay prints it as text. None of them needs to know the game.

**Name pictures as paths, not URLs.** A card can carry an image, and the addon
says where to ask rather than where it is:

```rust
.with_image(AddonImage::new(format!("/sprites/{species_id}")).with_alt(species_name))
```

The host resolves the path, which is what lets it cache, work offline, or
decline. Always set `alt` — a consumer that cannot show the picture is the
normal case, not the failure case.

**Put everything you parsed into the sections.** The commonest failure here is
an addon that recovers natures, abilities, IVs and PP into its typed payload and
then exports a table of names and levels. Every consumer except the desktop
dashboard sees only what the sections carry.

**Every section costs a tab, so make each one worth opening.** The opposite
failure is splitting one question across several sections. FireRed briefly had
a `Team` section — slots used, total HP, average level — beside its `Party`, and
a section per encounter method beside its `Area`. Both were tabs you opened to
learn something the tab next to them already showed: the team numbers are the
party's own bars added up, and separating grass from surf from fishing turned
"what is on this route" into four places to look.

The fixes are worth copying. An arithmetic summary of the section under it
belongs in that section's `note`, which is one line rather than one tab. A
distinction between rows — grass against surf — belongs in a badge on the row,
which keeps them apart without keeping them in separate places.

**One section per question, not per source.** FireRed reports the opponent
during a battle and the area's encounters outside one — and those were two
tabs before they should have been one. They answer the same question, "what is
that", and they are never both the answer: an encounter list is no use while a
Rattata is on screen. One `dex` section swaps its content to match, and the
strip stopped carrying a tab that was idle exactly when the other one mattered.

**Emit a section every time, even when it has nothing to say.** A section that
appears and disappears takes its tab with it, and a tab strip that changes
width whenever a wild Pokemon shows up moves every other tab out from under the
cursor. FireRed's battle section is always there; idle, it says
`"Nothing is fighting."` and notes what will appear.

**A section can raise a flag on its own tab.** A consumer that shows one
section at a time is one where "open" means "the others are not", so `badge` is
how an addon says *look at this now* to someone reading a different tab:

```rust
AddonSection::cards("battle", "In battle", vec![card])
    .with_badge(AddonBadge::toned("3 perfect IVs", AddonTone::Good))
```

Reserve it for what would genuinely interrupt someone. FireRed raises one only
for individual values worth stopping a run for, judged both ways a player cares
about — how many stats are maxed, and how good the spread is overall — because
three 31s and three terrible stats is a breeding catch that no total would
flag. An ordinary wild Pokemon raises nothing: a flag that is always there is a
flag nobody sees.

---

## 4. Testing without the emulator

`SparseMemory` implements `MemoryView` over sparse regions, so a test places the
bytes it cares about at the right address:

```rust
let memory = SparseMemory::new().with(UNIT_VITALS_BASE, vec![0x10, 0x00, 0x10, 0x00]);
assert_eq!(read_units(&memory)[0].max_hp, 16);
```

Use the **real bytes from the probe dump** as the fixture. `addons/ffta/text.rs`
asserts against byte arrays copied verbatim out of the ROM's name tables, so if
the derived encoding is ever wrong the test says so rather than every search
silently finding nothing.

There is no need to boot a ROM, load a BIOS, or play to the right screen.

---

## 5. Checking it works

`Tools > Addon Status` answers "why is this ROM showing nothing?" — it reports
which addons are registered, which claim the current ROM, and whether the
active one produced data or is idle.

The live export is at `stream-data/current-game.json` and is the fastest way to
see exactly what an addon reports:

```bash
tinybird "game.gba" --state battle.state
cat stream-data/current-game.json
```

`--state` boots straight into a savestate, which is what makes this repeatable
without replaying to the same screen. Name the ROM as well, as above: a
savestate no longer carries the cartridge inside it, so it is restored against
whichever game is loaded.

---

## 6. Worked example: Final Fantasy Tactics Advance

What the probe produced, in order:

1. `--find-relative ontblanc` against the ROM gave a hit and an implied offset,
   establishing that letters are stored contiguously starting at `0xB1` for `A`
   and `0xCB` for `a` — not ASCII.
2. Decoding the ROM at that offset produced clean job and character name tables
   (`Soldier`, `Thief`, `Beastmaster`, `Marche`, `Montblanc`), confirming the
   table. A second, `0x80`-escaped form offset by one turned up in menu strings
   with spaces (`White Mage`).
3. Mashing `a,start` for 20000 frames stalled at Marche's name-entry screen —
   visible only because of `--screenshot`. Navigating it (`left` selects `Yes`
   on the confirm dialog) and mashing on reached the tutorial battle.
4. With `Ritz  HP 16/16  MP 10/10` on screen, `--find-bytes 100010000a000a00`
   returned exactly one EWRAM match: `0x0200360C`.
5. `--stride 264` showed the next two slots holding the battle's other two
   units and every later slot zero.
6. `--find-relative arche` found the player name at `0x02001F1C`, stride 2 —
   stored as `0x80 <char>` pairs, the escaped form.
7. Re-checking 2500 frames later confirmed both addresses were stable.

Total: about a dozen probe invocations. The results are in
`crates/tinybird-desktop/src/addons/ffta/`.

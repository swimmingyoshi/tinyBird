# tinyBird Addon Architecture

tinyBird addons are moving toward a stable host contract that can support many
games without requiring every web client or tool to understand emulator internals.

## Current Shape

- `tinybird-addons` owns the shared snapshot envelope:
  - `StreamSnapshot`
  - `RomIdentity`
  - `AddonSnapshot`
  - `SNAPSHOT_SCHEMA_VERSION`
- `tinybird-desktop` still owns built-in game-specific readers such as the
  FireRed/LeafGreen party addon.
- `tinybird-web` consumes the exported JSON from `stream-data/current-game.json`
  and remains compatible with the existing FireRed payload.

## Addon Snapshot Contract

An addon export is wrapped in a stable envelope:

```json
{
  "schema_version": 1,
  "rom": {
    "title": "POKEMON FIRE",
    "game_code": "BPRE",
    "maker_code": "01",
    "revision": 0
  },
  "addon": {
    "addon_id": "pokemon_frlg_party",
    "display_name": "FireRed Party",
    "version": "0.1.0",
    "capabilities": ["party", "area", "battle"],
    "sections": [
      {
        "section_id": "party",
        "title": "Party",
        "kind": "table",
        "payload": {
          "columns": ["Slot", "Name", "Species", "Level", "HP"],
          "rows": [["1", "BULBASAUR", "Bulbasaur", "5", "20/20"]]
        }
      }
    ],
    "overlay_lines": [],
    "data": {
      "type": "fire_red",
      "payload": {}
    }
  }
}
```

## Near-Term Direction

The web overlay can now render generic addon sections for unknown addons, while
FireRed/LeafGreen keeps a custom rich renderer. The supported generic section
content kinds are:

- `key_value`: a list of `{ "label": "...", "value": "..." }` fields
- `list`: a list of display strings
- `table`: `{ "columns": ["..."], "rows": [["..."]] }`

The next useful boundary is external addon folders loaded from a local `addons/`
directory with a manifest and optional web assets.

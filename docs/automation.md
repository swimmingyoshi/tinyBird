# Headless automation

The browser player also supports [automatic session recovery](session-recovery.md),
including restoring the observation configuration associated with a checkpoint.

## Workshop to Python

1. Open the Workshop and load your game in its test player.
2. Use **Find in game** to search and narrow a numeric value. Select its address.
3. Click **Use this address for automation**. Name it, for example `player.hp`,
   and click **Add observation**. Its current unsigned value appears in the list.
4. Add more values, or edit/remove existing ones, under **Automation observations**.
5. Click **Export observations** to download `CODE-rN.observations.json`.

Observations are saved locally per account, game code, and revision. They are
independent of the reader manifest and can be exported without publishing a reader.
Importing an observation configuration replaces the current observation list;
an invalid or incompatible import leaves the list intact. The panel also supports
entering researched addresses directly.

Build the host and run the included Python example from the repository root:

```sh
cargo build -p tinybird-runtime
python examples/python/run_observations.py game.gba CODE-rN.observations.json --executable target/debug/tinybird-headless --steps 60 --frames 4 --buttons RIGHT
```

On Windows use `target/debug/tinybird-headless.exe`. Add `--state game.state`
to start at the same saved point used for discovery, and `--bios gba_bios.bin`
if needed. The example prints the initial observation followed by each action's
result. It requires only Python's standard library.

For your own experiment, keep `tinybird.py` next to your script or add
`examples/python` to your import path:

```python
from tinybird import TinyBird

with TinyBird("game.gba", executable="target/debug/tinybird-headless",
              observations="CODE-rN.observations.json") as env:
    observation = env.reset()
    for _ in range(60):
        observation = env.step(["RIGHT"], frames=4)
        print(observation["memory"]["player.hp"])
```

The CLI also accepts `--observations PATH`. From an existing Python client,
`env.configure_observations(path_or_dict)` applies a new configuration atomically.
Reset and checkpoint restoration retain the active observation configuration.

### Configuration version 1

```json
{
  "schema_version": 1,
  "game": { "code": "TBST", "revision": 2 },
  "fields": [
    { "name": "player.hp", "address": 33554436, "width": 2 }
  ]
}
```

This is a synthetic example, not a researched game address. Widths are 1, 2, or
4 bytes, read little-endian. Addresses are JSON integers (the UI accepts hex).
Names use dot-separated identifiers, starting each segment with a letter or
underscore; subsequent characters can also be digits. Names must be unique and
at most 128 ASCII characters. A configuration has 1–256 fields.

The runtime checks the four-character cartridge game code, including region,
and the revision byte before replacing observations. A mismatch, unsupported
version, duplicate name, invalid width, or overflowing range rejects the whole
configuration. Header matching does not distinguish ROM hacks that retain the
original code and revision; verify addresses on the ROM you will use.

Version 1 supports direct unsigned numbers only. Pointer chains, decrypted
Pokémon fields, text, repeating arrays, reward logic, and Gymnasium export are
not part of this configuration. A bar's current numeric value can be selected;
add its maximum as a separate named observation if you need it.

## Headless protocol

Build the local process host:

```sh
cargo build -p tinybird-runtime --release
target/release/tinybird-headless game.gba
```

On Windows the executable ends in `.exe`. Optional `--bios PATH` and
`--state PATH` load an explicit BIOS and initial core save state. Without a BIOS,
the existing core HLE is used. No ROM or BIOS is bundled.

The process accepts one JSON object per stdin line and flushes one response per
line to stdout. There is no background emulation: only `step` advances the game.
Audio output is disabled; the PPU still renders. There is no real-time pacing.
Each process has its own emulator, so independent Python workers can use separate
processes. The transport is local stdio, not an HTTP server.

```json
{"op":"set_observations","fields":[{"name":"example.counter","address":33554432,"width":2}]}
{"op":"step","frames":4,"buttons":["A","RIGHT"]}
{"op":"save_state","slot":0}
{"op":"get_frame"}
{"op":"load_state","slot":0}
{"op":"reset"}
```

The address above is illustrative, not a known game variable. Use the existing
Workshop memory search or `tinybird-probe` to find addresses for your ROM revision.
Named fields are unsigned little-endian integers of width 1, 2, or 4 bytes,
including unaligned values. Dotted names are returned as flat keys in `memory`.
This is a runtime observation configuration, not the existing addon manifest format.

Successful replies are `{"ok":true,"result":...}`; errors are
`{"ok":false,"error":"..."}`. Invalid JSON and arguments leave the process usable.
Commands larger than 1 MiB terminate it. Operations:

| Operation | Arguments | Result |
| --- | --- | --- |
| `reset` | none | Restore initial episode, return observation |
| `step` | `frames` (1–600), `buttons` (array) | Observation after action |
| `get_state` | none | Frame count, cycles, PC, named memory |
| `get_frame` | none | Width, height, `rgb888` format, flat RGB byte array |
| `get_memory` | `address`, `length` (0–65536) | Byte array |
| `set_observations` | `fields` (up to 256) | Replace configuration, return observation |
| `configure_observations` | `config` (versioned Workshop export) | Check game compatibility, replace observations, return observation |
| `save_state` | `slot` (0–15) | Save in-process checkpoint, return observation |
| `load_state` | `slot` | Restore checkpoint, return observation |

Buttons are A, B, SELECT, START, RIGHT, LEFT, UP, DOWN, R, L (case insensitive).
Every step replaces input for its duration, then releases all buttons. Pass `[]`
to wait. Reset restores the captured startup machine, including battery memory;
it does not carry progress from the previous episode. Observation definitions and
checkpoint slots survive reset. Slots last for the process lifetime and include
the latest image because core save states omit rendered pixels. An imported core
state may have a blank image until the first step renders it.

The host initializes cartridge time to 2000-01-01 UTC unless an initial state
provides its own clock; it never synchronizes to wall time during an episode.
Replays require the same ROM, BIOS, baseline, and core version. Each frame has
an execution budget; a stalled frame or pending link transfer returns an error
after partial advancement. Reset or load a checkpoint to recover. This host is
for independent consoles, not link-cable sessions.

## Python

Add `examples/python` to your Python import path or copy `tinybird.py` next to
your experiment. No third-party Python packages are required.

```python
from tinybird import TinyBird

with TinyBird("game.gba", executable="target/release/tinybird-headless") as env:
    observation = env.reset()
    env.save_state()
    for _ in range(60):
        observation = env.step(["RIGHT"], frames=4)
    image = env.get_frame()  # flat RGB bytes, 240 x 160 x 3
    env.load_state()
```

The client is synchronous; use one client per worker. Reward and termination
logic belong in the experiment and can consume `observation["memory"]`. This
client is not yet a Gymnasium environment and does not invent game-specific
rewards or completion rules.

## Feedback implementation status

The Workshop already supports memory discovery, narrowing searches, reader
authoring/testing, and community publishing. The browser already has lobbies
and link-cable multiplayer. This change adds the missing reusable runtime and
local automation interface, with pixels and named memory observations. The
Workshop can now author, preview, import, and export the versioned observation
configuration consumed directly by the runtime and Python client.

Further work from the feedback remains separate product features:

- Shared semantic schemas consumed by browser addons, with ROM compatibility.
- A Workshop training-environment exporter and Gymnasium adapter with explicit
  action spaces, reward definitions, and termination rules.
- Memory-change history connected to a bounded rewind timeline.
- Higher-level online services such as leaderboards beyond existing lobbies/link.
- An HTTP API, vectorized worker orchestration, and rendering-free execution.

Validation uses a synthetic ARM loop, without commercial ROM fixtures:

```sh
cargo test -p tinybird-runtime
cargo build -p tinybird-runtime
python examples/python/test_tinybird.py
node --test crates/tinybird-web/src/assets/workshop-observations.test.mjs
node tests/browser_observations.mjs
```

The browser test uses the Playwright installation described in
`tests/browser_workshop_ui.mjs`. It discovers a synthetic RAM value in the
Workshop, downloads the configuration, and passes that exact file to Python
and a synthetic ROM running in Rust.

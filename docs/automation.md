# Headless automation

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
local automation interface, with pixels and named memory observations.

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
```

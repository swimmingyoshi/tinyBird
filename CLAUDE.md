# tinyBird

A GBA emulator in Rust: a native core, a desktop runner, a WebAssembly build, a
browser site, and a JSON addon system that reads live game state.

Orientation lives in [`PROGRESS.md`](PROGRESS.md), with the long versions in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (emulator internals),
[`docs/WEB.md`](docs/WEB.md) (browser build and site), and
[`ADDONS.md`](ADDONS.md) (the addon contract).

## Shared project state

Development state — tasks, decisions, handoffs — lives in the Agent Control
Plane, reachable through the `acp` MCP server. It is shared with agents on the
VPS, so it is the source of truth for *what is happening*, while this repository
is the source of truth for *the code*.

The project slug is `tinybird`.

### Start every session with these, before touching code

1. `project_get_context` — what is in flight, what is blocked, what has been
   decided, which services exist.
2. `project_get_tree` — how the project is structured and which areas are
   finished, stalled or untouched.
3. `handoff_get_latest` — what the previous session, possibly on the VPS, left
   for whoever came next.
4. `agent_get_inbox` — anything addressed to you specifically.

Read `recent_decisions` before proposing any change of direction.

### While you work

- `task_add_update` as you go, not only at the end.
- `task_complete` when something is genuinely done.
- `artifact_create` for a commit, deployment or document — the reference, never
  the contents.
- When breaking work down, create the parent task first and pass its id as
  `parent_task_id`. That is what gives the tree its shape.

### Before you finish

`handoff_create` with a one-sentence `summary`, `completed_items` for what is
actually done, `next_items` as concrete actions, and `blocked_items` for what is
stuck and on what. Do this even when the work is unfinished — especially then.

### Proposing versus deciding

Use `proposal_create`. `decision_create` will refuse you with a 403, deliberately:
accepted architecture is the user's call.

## Services this depends on

Three shared services run on the VPS. tinyBird does not reimplement any of them,
and that is a recorded decision, not an accident.

| Service | Host | Used for | Client |
|---|---|---|---|
| auth | `auth.0xstash.dev` | Sign-in, project slug `tinybird` | `crates/tinybird-web/src/auth.rs` |
| media | `media.0xstash.dev` | Save vault and screenshots | `crates/tinybird-web/src/media.rs` |
| contact | `contact.0xstash.dev` | Help form and ticketing | `crates/tinybird-web/src/contact.rs` |

**The browser never holds an auth token.** The page gets an opaque first-party
session cookie (`tinybird_session`) and this server does the talking to the auth
service. The reasoning — third-party cookie blocking, and the intermittent
sign-out bug it causes — is at the top of `auth.rs`. Do not "simplify" this into
a direct browser-to-auth flow.

## Secrets

Configuration is read from environment variables; `.env.example` lists them.
`TINYBIRD_AUTH_PROJECT_SECRET`, `TINYBIRD_MEDIA_KEY` and `TINYBIRD_CONTACT_KEY`
are secrets. Never put their values in code, in the coordination service, or in
a commit — the coordination service records only the *names*.

Neither ROMs nor `gba_bios.bin` belong in this repository, and `.gitignore` is
set up to keep it that way.

## Running it

```bash
cargo run                       # desktop
cargo run -p tinybird-web       # the site, on http://127.0.0.1:8877

# the browser build needs the module first
rustup target add wasm32-unknown-unknown
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
```

## Testing

Roughly 650 Rust tests and 72 browser-module tests. CI builds and tests the
non-windowing crates, runs the GBA accuracy suite as a gate, and builds the
WebAssembly module.

`cargo clippy` is reporting-only and there is no `cargo fmt --check`, because the
tree predates both. Do not reformat files you are not otherwise changing: it
buries the real diff.

The release profile sets `lto = "fat"` and `codegen-units = 1` for a wasm module
about 14% smaller, at roughly ten times the build time. `panic = "abort"` is
deliberately not set — it would break `cargo test --release`, which is how
`tests/link_trade.rs` is meant to run.

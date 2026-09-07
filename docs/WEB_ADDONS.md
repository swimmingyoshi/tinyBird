# Community add-ons

Open `/addons` to create a reader, import/export its JSON, validate it with the
same WASM runtime used by Play, install it locally, or publish a release.
The starter form creates a single numeric field; the editor supports multiple
fields, repeated cards, pointer chains, text reads, meters, and an optional
readiness condition. Addresses must be researched for the intended game;
validation checks the format and bounds, not whether an address means HP.

`Preview in running game` asks an open Play tab on the same origin/account to
evaluate the draft once. It returns only the draft's rendered sections and
status. The previous installation is restored before the next frame. Preview
does not upload a ROM, save, or memory dump. When several Play tabs are open,
the first response is used; keep the intended game open for an unambiguous test.

## Runtime

Browser manifests are owned by the emulator instance. `tb_install_manifests`
replaces the complete set atomically; `[]` removes it. Failed validation leaves
the previous set intact and exposes an error string. Built-in Rust readers
remain registered separately. Community sections append to the selected
built-in/fallback reader and have namespaced IDs. `community_addons` is an
additive snapshot field with active, idle, incompatible, or error status.

Section IDs are owned strings, eliminating the previous per-snapshot leak.
The legacy desktop manifest adapter still owns startup-lifetime metadata via
the static registry; it is not used for browser installation or server validation.

Manifest versions 1 and 2 are accepted; new templates use 2. Numeric zero is
now valid for both. Null pointers and false `when` conditions remain idle.
For example:

```json
{
  "manifest_version": 2,
  "addon_id": "my.counter",
  "display_name": "My counter",
  "version": "0.1.0",
  "matches": { "game_code": ["BPRE"], "revision": [0] },
  "sections": [{
    "id": "stats", "title": "Stats", "kind": "key_value",
    "fields": [{ "label": "Value", "read": { "u16": "0x02000000" } }]
  }]
}
```

The address above is only a placeholder. Optional `when` has the shape
`{"read":{"u8":"0x02000000"},"equals":1}`. Compatibility combines full game
code, legacy three-character prefixes, or titles with an optional revision
filter. It does not yet verify full ROM hashes; hacks retaining the original
header need manual compatibility testing.

Execution is read-only and has no scripting, filesystem, or network capability.
Only EWRAM, IWRAM, and cartridge ROM may be read. Pointer targets are checked as
well as direct addresses. Limits: 64 KiB per manifest, 8 sections, 32 fields per
section/card, 64 cards per repeat, 64 bytes per text read, 4 pointer dereferences,
2048 estimated value reads and a hard 16384-byte read budget per snapshot.
Output is capped at 32 KiB per reader. An exhausted read/output budget reports an error with no partial sections.
Readers refresh on the existing 250 ms snapshot timer, not each emulated frame.

## Accounts and releases

Browsing and local creation need no account. Publishing, community installation,
and reporting require the existing authenticated session. Public metadata uses
the author's display name, never their email or authentication token.
An author owns the `(account ID, manifest addon_id)` namespace. Publishing the
same ID creates the next release; another author cannot replace that package.
Release contents are immutable. Share links pin both package and release.
Users explicitly choose another release to upgrade or roll back.

Each account can install 12 community readers; each browser/account can also
keep 4 local readers. Local drafts and readers use account-specific localStorage
keys, with a separate guest namespace. Account changes clear the old runtime
set before loading the new one. A session-owner header rejects stale writes
after signing in as another account in a different tab.

Installation changes reach open Play tabs through BroadcastChannel, storage
events, window focus, and a 30-second refresh while visible. Withdrawals stop
catalog/distribution immediately and stop installed readers on their next
refresh. A reader already loaded offline cannot be remotely erased.

Authors can withdraw their packages, and users whose verified auth role is
`admin` can review reports and withdraw any package. Withdrawal cannot be undone
through the UI. Reports allow one per user/package, at most 10 new reports per
hour. Publishing allows 10 releases/hour, 50 packages/account and 50 releases/package.

## Storage and deployment

`TINYBIRD_ADDON_DB` defaults to `stream-data/community-addons.sqlite3`. The Docker
image sets `/app/data/community-addons.sqlite3`; persist that data volume.
SQLite stores the catalog, releases, installations, and reports. Writes use
transactions, foreign keys and a busy timeout; blocking DB operations run on
Tokio's blocking pool. Back up with SQLite's backup API or while the service is
stopped, accounting for WAL files. Do not copy just the live `.sqlite3` file.
This fits the current single-host deployment. Multiple replicas need shared
database/auth-session infrastructure rather than independent SQLite files.

The existing `/api/addons` directory endpoint remains for legacy tooling.
Play now activates the current user's explicit installations rather than
automatically installing server-directory manifests for every visitor.

## API

| Endpoint | Behavior |
|---|---|
| GET `/api/community-addons?q=&offset=0` | 40 latest package releases, searchable |
| POST `/api/community-addons` | Publish `{manifest,description,license}` |
| GET `/api/community-addons/{id}` | Public immutable releases and manifests |
| GET `/api/community-addons/installed` | Current account's installations and published package IDs |
| PUT `/api/community-addons/{id}/installation` | Pin `{release,enabled}` for the current account |
| DELETE `/api/community-addons/{id}/installation` | Remove own installation |
| DELETE `/api/community-addons/{id}` | Author/admin withdrawal |
| POST `/api/community-addons/{id}/reports` | Submit `{reason}` |
| GET `/api/community-addons/reports` | Admin report queue |

Mutations require `X-Tinybird-Addons: 1`. No CORS permission is granted, so
cross-origin forms cannot make authenticated mutations. JSON bodies, prepared
SQL statements, server-side validation, account ownership checks, and text-only
DOM rendering form the trust boundary. No community HTML or JavaScript executes.

## Workshop and reader management

Open **Manage add-ons** in Play's bottom controls, then select **Workshop**
(`/addon#workshop`; `/addons` remains an alias). The dedicated page has Manage,
Workshop, Community, and advanced JSON views. Play itself keeps its original
layout. The workshop places a live add-on preview on the left, a test emulator in the center, and the
field workspace on the right. Load a game or a save
state into this test emulator; it is independent of another open Play session.

The **Live reader** panel on the left
renders the current draft with Play's normal field, meter, table, list, and card
components before it is installed. The bottom **Save & load** panel includes the existing account and
cartridge-specific vault list and load controls. Loading a saved moment updates
the same test emulator that the memory finder and preview inspect.

Manage lists all five preinstalled readers as well as community and local
installations. Built-in enable/disable choices are stored per account in this
browser and propagated to open players. Disabled readers are skipped before
reading memory. Community readers still work if every built-in is disabled.
These preferences do not currently sync across devices.

1. Load a game, name the reader, and choose **Add your first field**. Give it
   a label and choose its display type. **Open / new** holds saved readers,
   templates, imports, and the link to shared memory sheets.
2. Enter an address or choose **Find in game** in the field editor. Search
   for a visible value, play to change it, then narrow the matches. Search
   settings include number size, memory area, text, and byte patterns.
3. Choose a result and check the field's **Live value** before adding it.
   Bars have a separate **Find maximum** search; each address retains its own
   results. Labels, notes, and edit state survive result selection.
4. **Add to reader** returns to the field list. **Add & next field** opens a
   fresh field in the same category with empty addresses. Added fields appear
   in the live reader. Category setup and repeating-card settings are inline.
5. **Finish & share** includes a completed field still in the editor, then
   opens a dialog with **Save & enable**, **Download JSON**, and optional
   community publishing. Incomplete fields stay in the editor with an error.
   Downloading a draft is available even without a running test game.

The builder sets game code and revision from the loaded cartridge. Advanced
manifests, including pointer reads, can be imported or edited as JSON. Drafts
use the same account-scoped browser storage as the standalone creator. Local
readers can be reopened in the workshop. Unfinished field inputs and the field
being edited are also stored and restored after a reload. Returning from
another add-on view preserves searches when the stored draft has not changed.
Signing into another account restores that account's draft and clears the
previous account's work from the panel. Structure changes require saving or
canceling an unfinished field, so edits cannot target a row that moved.

### Categories, tracker types and reordering

The field list draws the draft as a tree of **categories** rather than as one flat
list of fields, which is the shape `sections` always had in the manifest. Two
kinds:

- A plain category is a `key_value` section: a heading with rows under it.
- A **repeating** category is a `cards` section. One entry is described once
  and drawn `count` times, stepping every address by `stride` — so a party of
  six is six cards from six fields, not thirty-six. Fields are given the
  addresses of the *first* entry. **Settings** opens the count and stride
  controls, plus optional Pokémon party headings that use a Generation 3
  record for each entry's species name, nickname, and sprite.

Each field picks how it is shown. `bar` is the one worth knowing: it takes a
second address holding the maximum, and a field with a maximum is what makes
the renderer draw a meter and colour it — green, amber, red — so a health bar
is not a separate feature but a field with a `max`. `gen3_text` and
`gen3_species` decode the two things a plain read cannot: the games' own
alphabet, and the encrypted, personality-permuted species field. The full list
is in ADDONS.md under *What a field can read*.

Rows carry a grip and are dragged to reorder, including into another category;
a category is dragged by its header strip. A drop that lands somewhere
impossible leaves the draft as it was. **edit** opens a field back in the form
with its label, address, type and note intact — editing used to mean deleting
the row and adding it again, which lost the note and the ordering.
**headline** promotes a field to a card's lead stat, which the renderer draws
larger than the rest; **demote** puts it back in the list rather than
discarding it.

**Party template** fills the draft with a complete FireRed or Emerald party —
a repeating category, a decrypted species heading, a nickname, a sprite, an HP
bar, and the six stats — as a worked example of every part above. Its
addresses still want checking against the ROM in hand before publishing.

Memory scans run only on explicit clicks over 256 KiB main RAM or 32 KiB fast
RAM, using aligned little-endian values. All matching addresses are retained;
the result list shows at most 80. Changing region, number size, cartridge, or
rewinding resets the search. The read-only WASM API rejects IO and ranges that
cross region boundaries. Returned bytes are copies. Live draft previews run
twice per second while the workshop is visible and live preview is enabled, and restore the
installed readers synchronously before emulation resumes. Community packages
cannot invoke the workshop's memory API or run JavaScript.

Encrypted values and dynamic addresses may need advanced pointer-based
manifests; the scanner does not automatically decode them — `gen3_species` is
the one exception, and it decodes species only. Desktop layouts place the tools
beside a sticky test game; smaller screens stack them.

Inside the test frame the screen is pinned to the top, so opening **Save &
load** no longer pushes the game out of view to reach the controls that act on
it, and an open drawer scrolls within itself rather than growing with the
number of saves in the vault. The vault list itself is a grid that fits as many
saves across as the container has room for: one column in Play's rail, three or
four in the workshop's wider column.

## Public memory sheets and discovery

A sheet uses the existing validated manifest format, with
`"$comment":{"kind":"memory_sheet","memory_sheet_version":1}`. Field `hint`
contains discovery notes; `read` retains the exact direct address, pointer path,
or text definition. Conditions, maximum values, and repeated cards stay intact.
It can be installed as a reader or copied as a new workshop draft. Copies retain
source release, author, and license metadata and receive a new add-on ID.

Choose **Also list as a public memory sheet** in Finish before publishing or downloading.
Community's memory-sheet filter is also linked from Open / new. Public search accepts
game codes, labels, and address strings, and supports `kind=memory_sheet` on
`GET /api/community-addons`. Release pages display a human-readable memory table.
Normal release ownership, pinning, withdrawal, reporting, and licensing apply.
The public index is author-contributed, not an automatic verification service.

Find supports unsigned numbers, inclusive numeric ranges, unknown-value and
change comparisons, optional unaligned scans, printable ASCII text with optional
case folding, and hex byte patterns with `??` wildcard bytes. Text and patterns
are limited to 64 bytes. Searches retain all candidates and display the first 80;
Reset search discards previous candidates. Only the selected RAM region is scanned.
Custom character encodings, encryption, and automatic pointer discovery are not
implemented by the finder. The guide at `/reader-guide` explains those boundaries
and includes examples from the compiled FireRed/Emerald party reader.

## Verification

```sh
cargo test -p tinybird-web --bin tinybird-web community_addons
cargo test -p tinybird-addons -p tinybird-wasm --lib
cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
node tests/wasm_addons.mjs
node --test crates/tinybird-web/src/assets/*.test.mjs
npm install --prefix target/workshop-ui-check --no-save --package-lock=false playwright
node tests/browser_workshop_ui.mjs
```

The WASM regression uses the existing local FireRed ROM/state. It checks
coexisting readers, identical emulator state before/after reads, failed
replacement rollback, compatibility, readiness, uninstall, and memory growth
across repeated edits. Server tests cover authentication, cross-account
protection, withdrawal, immutable versions, pinned installs and disk persistence.

The workshop browser regression requires a local web server (set
`TINYBIRD_TEST_BASE`; default port 8879) and Chromium. It uses Edge on Windows;
`TINYBIRD_TEST_BROWSER` can specify another executable. It serves current source
assets through browser routes and uses controlled RAM to test discovery,
independent maximum searches, field editing, draft recovery, download, local
installation, responsive layouts, and publishing against an intercepted endpoint.

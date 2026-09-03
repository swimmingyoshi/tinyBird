# tinyBird on the Web

Last reviewed: 2026-08-24

The same emulator core that runs the desktop app also runs in a browser tab and
feeds the stream overlays. This document covers how that build works, how the
page is put together, and how asset storage is wired.

---

## 1. Surfaces

| Path | What it is |
|---|---|
| `/` | Home. A landing page: the claim, the three surfaces, the way on. |
| `/play` | The emulator. Canvas rendering, keyboard input, WebAudio, save states. |
| `/info` | Where things stand: what works, what is in hand, what is knowingly missing — plus this server's live state and how to run your own. |
| `/contact` | The contact form, and the sign-in in front of it. |
| `/support/tickets` | What you have sent and what came back. One conversation at `/support/tickets/{id}`. |
| `/overlay/{section}` | OBS browser sources, fed from the desktop app's JSON export. |

Everything is served by `tinybird-web`, a single axum binary with the pages
compiled in via `include_str!`.

The three pages outside the emulator share their header through
[`chrome.js`](../crates/tinybird-web/src/assets/chrome.js) — `mountChrome()`
sets the server-up dot and mounts the account menu — and their look through one
vocabulary in `console.css`: a `.strip` with a `.strip__title`, holding
`.tiles`. The home page is only that, three times.

The home page used to carry a live storage read-out, the commands to run it
yourself, and the contact form. All three were real and none of them answered a
question somebody arriving at the front door had asked yet, so the first two
moved to `/info` and the third to `/contact`.

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
(~525 KB), which `tinybird-web` serves at `/tinybird.wasm`. The path is
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

What the server does: serve static files, read local ROMs and the BIOS off its
own disk, hold the storage key so the browser never sees it, and do the auth
handshake. No game CPU, ever.

That shapes everything multiplayer. A room of four costs the same as a room of
one, because the work is happening on four different machines and only small
JSON crosses the wire.

## 4. Rooms

Everyone in a room runs their own emulator. What travels is the addon snapshot
— the same read-out `/api/snapshot` serves and the stream overlay renders — so
a room can show what everyone is playing without anything being emulated
centrally, plus a JPEG of each sharer's screen for the people watching.

Rooms also carry the **link cable**, which is what trading and battling need.
The core side is in [the link cable](ARCHITECTURE.md#the-link-cable); what
follows is how it crosses a network.

### Seats

A cable takes four consoles and a room takes eight, so the server gives each
member a `seat` — 0 to 3, or none for the later arrivals, who watch instead.
Seat 0 is the host's and nobody else's: it is the parent, the console that
clocks the cable.

Seats are assigned by the server rather than worked out from the member list,
because that list is sorted by name. Two people with the same name, or one of
them renaming mid-session, would otherwise swap seats in the middle of a trade.
A seat is freed when its console leaves and reused by the next arrival.

### Carrying a transfer

```
parent  START written by the game, core stops the frame there
        -> link_start(seq, linked frame, offset within frame)
child   catches up to that frame-relative position, link_join()
        -> link_value(seq, its halfword)
parent  collects every seat, delivers locally, -> link_data(seq, values)
child   link_deliver(values)
```

One round trip. When the seated consoles are tabs in the same browser profile,
they discover each other through `BroadcastChannel` and carry these four link
messages directly. This removes the WebSocket and server round trip from the
hot path. A mixed room, separate browsers, or separate devices automatically
falls back to the server relay.

On that relay the server never looks inside — it has no idea what a halfword
means — but it does enforce **who may drive**: `link_start` and
`link_data` from anyone but the host are dropped. A cable has one parent, and
letting any member announce a transfer or publish its result would let them
corrupt somebody else's trade. That check is on the server because in the page
it would be advice.

The frame-relative position matters after the first exchange. The parent receives a
child's value and resumes before the child receives `link_data`, so the parent
can reach the next transfer while the child is still one network hop behind.
Sampling the child's send register immediately would repeat the previous
protocol word; Fire Red reports this as a link error after the confirmation
prompts. Absolute cycle counts cannot be compared because browsers attach the
cable at different times, so the child catches up to the parent's linked frame
and then its offset within that frame before answering.

### Waiting is the synchronisation

**While a transfer is outstanding, the console does not run.** This is the whole
design and it is worth being clear about why.

A transfer between two consoles takes about 5,200 cycles — a third of a
scanline. A round trip to another player takes tens of milliseconds, thousands
of times longer. Run frames across that gap and the game sees a transfer no
cable could have produced, and link code answers that by deciding the cable came
out.

Freezing spends the latency in wall-clock time instead. The game runs slower
while linked, but every transfer costs it exactly what the hardware would, so
nothing times out. Two things make it exact:

- `Gba::run_frame` **stops part-way** when a transfer starts, rather than
  finishing the frame the game set `START` in. Without this, up to a frame of
  emulated time passes inside a transfer before the page can notice.
- The frame loop skips running frames while `linkPending`, and resets the
  pacing clock so no catch-up debt accrues to be spent racing ahead afterwards.

Measured across two browsers: **zero frames** of emulated time pass between the
game setting `START` and the data arriving.

The cost is throughput: one transfer per round trip. Measured between two
browsers on one machine:

| | |
|---|---|
| no room, no cable | 60.0 fps |
| cable attached, game not using it | 60.0 fps |
| transferring flat out | 56.8 transfers a second |

A linked game asks for roughly 60 transfers a second, so that is about 95% of
full speed on a local link, and proportionally less as latency rises. Attaching
a cable costs nothing until a game actually uses it.

Two things had to be right to get there, and both were wrong first:

- **Settle the moment the last console answers**, in the message handler, not
  on the next animation frame. A linked game asks for a transfer every frame
  and the console is frozen for the whole of each one, so deferring to the
  frame clock added up to 16 ms to every transfer.
- **Announce a transfer in the tick that started it.** The core stops the frame
  the instant one begins, so the page can act then instead of a frame later.

### When somebody stops answering

A transfer cannot complete until every seat has answered, so a player who
closes their tab mid-trade would hang everyone else. The parent gives up after
`LINK_TIMEOUT_MS` (2 s) and transfers without them, leaving their slot reading
`FFFF` — which is exactly what the hardware reads when a cable comes out, and
what every game already knows how to handle.

### Running a frame in slices

Fire Red exchanges about **seven halfwords per frame** during a trade — some
416 a second. The page managed 45, and what the game makes of that is a black
screen and "Communication error".

The bottleneck was not the network. JavaScript is single-threaded, so a message
arriving while a whole frame is being run waits for the end of it, and a frame
costs about 14ms on this emulator. Measured round trips were **12.8ms between
two browsers on one machine**, where a socket should manage under one.

So while a cable is attached the frame is run in slices with a yield between
them, and the socket is serviced in the gaps. `tb_run_slice` runs at most
`SLICE_STEPS` instructions and says whether the frame finished; the yield uses
a `MessageChannel` rather than `setTimeout`, which is clamped to about four
milliseconds once nested and would cost more than it saves.

| | before | after |
|---|---|---|
| round trip | 12.8 ms | **0.98 ms** |
| transfers a second | 45 | **246** |

Unlinked play keeps the plain synchronous path. It has nothing to gain here and
the slicing costs a few percent in yields.

#### It still will not cross the internet

246 transfers a second is enough for a local link and a tunnel to the same
machine. It is not a general solution: the ceiling is the round trip, so a
player 30ms away gets about 33 transfers a second against the 416 a trade asks
for. **Pokémon trading over the internet is not reachable by relaying halfwords
at all**, which is why mGBA replicates the whole linked set on every machine
and sends only inputs. That path needs roughly 2.5x the emulation speed we
have — see the note on local replication above.

### The frame barrier

Relaying halfwords is not on its own enough to keep two consoles together.

Each browser has its own frame clock. Between transfers a child runs freely
while the parent is stopped waiting for it, so the child gains most of a frame
on every transfer. Pokémon exchanges a halfword every frame and a handshake is
dozens of them, so by the end the two games are nowhere near each other. This
is the failure the emulator research describes as *"if one side is a little
late, the game can wait forever"* — and it is why the same ROM and savestates
that link perfectly in `tests/link_trade.rs`, where the two consoles are
stepped in lockstep, fail between two browsers.

**The parent is the clock, and only children need restraining.** The parent is
already held back by every transfer, because it cannot finish a frame until the
children have answered. So the barrier is one-way: the parent says how far it
has got (`link_tick`), and a child never runs past that. It costs one small
message a frame and no round trip of its own.

Measured between two browsers with the parent's machine throttled to a quarter
speed:

| | parent | child | apart |
|---|---|---|---|
| before | 96 frames | 478 frames | **382** |
| after | 108 frames | 108 frames | **0** |

`LINK_MAX_LEAD` is 1: a child may draw level with the parent and no further.
Zero would pin it a frame behind and starve it whenever a tick was late.

**A barrier that never lifts is a hang.** A child that has heard no tick for
`LINK_TICK_TIMEOUT_MS` runs regardless, so a parent that paused, closed its tab
or left the room does not take the other player's game down with it. Verified:
after the parent leaves, the child runs on past the barrier rather than
stopping.

#### What this is not

This is a frame barrier on a relay, not the deterministic lockstep that mGBA
uses, where every participant runs the whole linked set locally and only inputs
cross the wire. That is the more reliable model and it is out of reach for now:
one console runs at about 1.2x real time in WebAssembly, so two would run at
0.6x. Presentation is only 1.7% of a frame, so there is nothing to reclaim by
not drawing the second one. Local replication needs roughly 2.5x the emulation
speed we have, which makes core performance the thing that would unlock it.

### Re-seating is not free

A member list arrives whenever anyone loads a game or changes what they are
playing, not only when somebody joins or leaves. Re-seating the cable on every
one of them tore down a link that had not changed and abandoned whatever
transfer was on the wire — which is what made the link appear to work only
after being switched off and on again. The page now compares the seating it
would produce against the seating it has, and does nothing when they match.

### Telling one link failure from another

Every link failure looks the same from outside: the game stops and says the
link broke. What tells the causes apart is on the page, under the Link cable
toggle, once the cable is carrying anything:

```
Player 1 of 2, driving the cable · 219/s · 1.2ms · direct
```

Transfers a second, and how long the other console takes to answer. If any
transfer is lost the line says so and turns red. `direct` confirms that every
seated console is a local tab and the server relay is being bypassed. That
read-out is there rather
than in devtools because the numbers are wanted by somebody in the middle of a
trade, not by somebody debugging one afterwards. `tinybird.link` still has the
full tally.

The difference between "the other console never answered", "we ran out of time
waiting", and "data arrived for a transfer we were not in" is the whole
diagnosis, so the page keeps a tally. In devtools:

```js
tinybird.link
// { started, answered, settled, timedOut, couldNotJoin, refusedDelivery,
//   abandoned, phase, seat, cable, sending, recent }
```

`recent` is the last dozen exchanges as `parent/child` hex. A stalled link looks
identical from outside whichever console went quiet; what crossed the wire says
which. A console still taking part keeps changing what it sends, and one that
has dropped out repeats the same value or `ffff`.

A healthy link has `started`, `answered` and `settled` in step and the last
three at zero. Measured over a sustained local run: 132 started, 132 answered,
132 settled on both consoles, nothing dropped.

`tinybird.emu` and `tinybird.lobby` are there too, and
`tinybird.emu.debugRead(0x4000128)` reads `SIOCNT` live.

### The cable is opt-in

A checkbox, off by default, remembered per browser. Two reasons: linking two
games is a thing you agree to rather than something a room does to you, and an
attached cable makes the core watch its serial registers on every instruction —
about 9% of emulation speed — which a game that will never use the link should
not be paying.

| Route | Purpose |
|---|---|
| `POST /api/lobby` | open a room, returning its code |
| `GET /api/lobby/ws?room=CODE` | join it, over a WebSocket |

Over that socket: `playing`, `snapshot` and `frame` in both directions, `lock`
from the host, and `link_start` / `link_value` / `link_data` for the cable.

### Who you are is not the browser's to say

With accounts configured, a member's name comes from the session and nothing
else will do. The page still sends a `name` in the query string; the server
ignores it. Otherwise anyone could appear as anyone, which is the same rule that
governs who owns a save and for the same reason.

Without accounts — a server you run for yourself — the offered name is used,
because the only person who can reach it is the person running it.

### A code has to be a room

`POST /api/lobby` opens a room; joining a code that was never opened is an
error. That matters more than it sounds: an earlier version created a room for
any code that arrived, so a mistyped code silently opened an empty room instead
of saying the code was wrong, and every typo counted against the server's room
limit.

**Codes are meant to be read aloud.** Five characters from an alphabet with no
vowels — so a random code cannot spell something unfortunate — and no `O`/`0`
or `I`/`1`, because those get misheard. Matched case-insensitively with
punctuation stripped, so `bc-df` and `BCDF` are the same room. Generation
retries on collision rather than handing someone else's room to whoever asked
next.

### The host

Whoever opened the room. The host can **lock** it, which turns away everyone
else while still letting the host back in — locking yourself out by reconnecting
would be a trap. Anyone else asking to lock is ignored silently: a member who is
not the host has no business being told the room's rules.

The account behind a member is never broadcast. The room needs it to recognise
the host; the other members do not, and there is a test asserting the subject
claim does not appear in what goes over the wire.

### Rooms that outlive a dropped connection

An empty room is kept for five minutes rather than destroyed. A connection that
drops should not take the room with it while its only occupant is reconnecting —
which is the common case behind a proxy that closes idle sockets. Past that, an
abandoned room is just a code nobody can reuse, so it is swept.

Sweeping is lazy, on create and join, rather than a background task: rooms only
matter when somebody is asking about one.

**Limits, because a public deployment has no other gate.** Eight members to a
room, two hundred rooms to a server. Joining a room that already exists never
counts against the room limit, so a busy server turns away new rooms rather than
splitting up groups that are already playing.

### Watching someone play

Two different things travel between members, and it is worth being clear about
which is which.

| | What it is | How often |
|---|---|---|
| Snapshot | the addon read-out — party, HP, map | twice a second |
| Frame | a picture of the screen, as a JPEG data URL | ten times a second |

The snapshot is what the panel uses to say what everyone is playing. It cannot
show you anyone's screen, because it is a description rather than a picture —
which is why seeing a room-mate's game needed frames as well.

**Sharing is opt-in and off by default.** It costs upload bandwidth and shows
people your screen; neither should happen because you joined a room. A member
who has it on sends a frame every hundred milliseconds, and only while somebody
else is in the room — broadcasting to nobody is just cost.

Ten a second, not sixty: a frame is a few kilobytes where a read-out is a few
hundred bytes, and it is relayed to everyone in the room. That number decides
whether a room of four is comfortable or is a hundred kilobytes a second leaving
somebody's house. JPEG rather than PNG for the same reason — about a tenth the
size at this scale, and the artefacts are unobtrusive in the half-width peer
view used while two games share the play space.

Nobody announces that they have stopped sharing; they just stop sending. A
member counts as live for two seconds after their last frame, and the check for
that runs *outside* the "is my game running" test — watching a room-mate has to
keep working while your own game is paused.

**This is a picture of a screen, not link play.** It is the difference between
watching someone play and playing with them; the second one needs the link cable
emulated, which is a separate piece of work.

The transport is the existing WebSocket, which means frames pass through this
server. That is fine for a handful of friends and is the wrong shape for more:
`canvas.captureStream()` over WebRTC would encode properly and go peer to peer,
and the message boundary here is deliberately narrow enough to swap.

### Staying connected

The server pings every idle socket every thirty seconds. A member with no game
loaded publishes nothing, and a proxy in front of this server will close a
connection that has been silent for a minute or two — Cloudflare does. The page
reconnects on its own, but a room that drops everyone every hundred seconds and
quietly rebuilds itself is not something to leave in.

A refusal is not retried. A room that does not exist will not start existing
because the page asked again, so the page says why and stops rather than
flickering.

The connection policy is in [`lobby.js`](../crates/tinybird-web/src/assets/lobby.js),
kept free of DOM references so it can be tested without a browser — including
the thing that only breaks once deployed, which is that a page served over TLS
may not open a plain `ws://` socket.

### When a console cannot take a transfer

A child that cannot join a transfer — it is ahead of the parent, or too far
behind to catch up to the instruction the parent started on — **says so**
rather than going quiet.

Silence reaches the same conclusion. The difference is when: the parent waits
out its full timeout first, and the parent's game is frozen for the whole of
it, so a run of missed transfers becomes a run of stalls. A game watching its
link go quiet for hundreds of milliseconds at a time is a game about to report
a communication error, and that is true of any linked game, not just Pokémon.

`link_skip` costs one message and turns a stall into a decision. The parent
records the seat as `0xFFFF` — what hardware reads from a slot nobody is
driving, and what a game already knows how to handle — and settles at once.

The message is relayed like `link_value`, by any member about its own seat,
and travels the same-machine path as well as the server one. A skip that only
worked over the relay would leave two tabs on one computer stalling exactly
where the message exists to stop them.

### Reading the cable when it misbehaves

The link note under the cable toggle names the **dominant** failure, not just a
count:

```text
Player 2 of 2 · 58/s · 12 lost, mostly out of step
```

The four ways a transfer is lost want four different fixes, so a bare total
says a link is unhappy and nothing about why:

| Shown | Means |
|---|---|
| out of step | a console could not reach the instruction the parent started on |
| no answer | nobody replied in time — the network, or a console that stopped |
| arrived late | data reached a console that was no longer waiting for it |
| given up | a wait abandoned, usually because the room went away |

`window.tinybird.link` has the raw tally for anything finer.

---

## 5. Asset storage

Storage is the [0xstash media API](https://media.0xstash.dev/docs). The split
that matters:

```
browser  ──fetch bytes──▶  https://media.0xstash.dev/<vault>/<asset>   (public CDN)
                                                    ▲
server   ──list, upload──┘  X-Media-Key: ...        (key never leaves here)
```

Reads are public and cached by Cloudflare, so the page fetches ROM bytes
directly with no credential. Writes need the key, so listing and uploading go
through `tinybird-web`, which reads `TINYBIRD_MEDIA_KEY` from `.env`.

**The key is never sent to the page.** If you find yourself wanting it in the
browser, the operation belongs on the server instead.

| Route | Purpose |
|---|---|
| `GET /api/library` | List the vault. Returns `configured: false` rather than an error when no key is set. |
| `POST /api/library/upload` | Push a file (a save state) into the vault. |
| `GET /api/local` | ROMs in the local `roms/` folder. |
| `GET /local/{name}` | Serve one local ROM. Only names the listing produced are accepted. |

### Configuration

```bash
cp .env.example .env
# then set TINYBIRD_MEDIA_KEY
```

| Variable | Default | Meaning |
|---|---|---|
| `TINYBIRD_MEDIA_KEY` | none | API key. Storage is off without it. |
| `TINYBIRD_MEDIA_VAULT` | `default` | Vault to list and upload into. |
| `TINYBIRD_MEDIA_URL` | `https://media.0xstash.dev` | API base URL. |

`.env` is gitignored; `.env.example` is the committed template. Real environment
variables always win over the file, so containers and CI override it without
editing anything.

### Accounts

Optional. Without `TINYBIRD_AUTH_PROJECT_SECRET` the sign-in panel stays hidden
and every save belongs to `TINYBIRD_MEDIA_OWNER`, which is how a server you run
for yourself works. With it, saves belong to whoever is signed in.

| Route | Purpose |
|---|---|
| `POST /api/auth/register` | create an account; `email`, `password`, optional `displayName` |
| `POST /api/auth/login` | sign in |
| `POST /api/auth/logout` | sign out here and at the service |
| `GET /api/auth/me` | who the caller is, and whether accounts exist at all |

The sign-in panel is a dropdown hung off the header rather than a rail panel:
it is checked rarely and read never, so it should cost no vertical space. It is
**the same menu on every page**, built by
[`account.js`](../crates/tinybird-web/src/assets/account.js) into a one-line
host each page carries:

```html
<div class="account" id="account" hidden></div>
```

`mountAccount({ onChange })` returns a small controller — `refresh()`,
`user`, `noticeSignedOut()`, and an `extras` element a page can hang its own
buttons in, which is where `/play` puts "Claim old saves". Nothing else has
anything to add. It began as markup in `play.html` with its behaviour in
`play.js`, which was fine while `/play` was the only page that had one; signing
in is how you send a message, claim a save, or be someone in a room, so it
belongs in the bar rather than bolted to whichever form happens to need it. The
bar shows only whether you are signed in — a hollow dot or a lit one — and the
address appears only once the menu is open, which keeps it out of a stream
capture. The same reason the status line says "Signed in" and not "signed in as
<address>": that line is the one part of the page that is always visible.

**The browser never holds a token.** The obvious design has the page call
`auth.0xstash.dev` directly and refresh a 15-minute token from a cookie. That
cookie is set by the auth service while the page runs on `127.0.0.1:8877`, which
makes it a *third-party* cookie: Safari blocks those by default and Chrome and
Edge are phasing them out. When it is blocked the refresh silently sends nothing
and the player is signed out every fifteen minutes and on every reload,
intermittently, depending on browser settings.

So this server does the talking and hands the browser only an opaque first-party
session id:

```
browser ──cookie: tinybird_session──▶ tinyBird ──bearer token──▶ auth service
```

The cookie is `HttpOnly`, so page scripts cannot read it either. Verified from
inside the page: `document.cookie` is empty and nothing token-shaped is in
`localStorage`.

**Identity comes from introspection, not from the login response.** After
signing in, the server calls `POST /api/auth/introspect` with the project secret
and takes `claims.sub` as the owner. That does two jobs: it yields the canonical
subject, and it proves the token was minted for this project rather than another
one on the same service. Never the email — an address can change, and it would
put a personal identifier in object names.

**Sessions live in memory**, so restarting the server signs everyone out. That
is the right trade for a server you run yourself; more than one process would
need shared storage.

### Whose save is whose

The owner is part of a save's name, because the downloads project is shared and
a name is all there is to say who a save belongs to:

```
tb_BPRE_1787570960801_route4.savestate            written before accounts
tb_<owner>_BPRE_1787570960801_route4.savestate    since
```

The timestamp tells the two apart: it is the only field that is always a number,
and a game code never is. Saves from before accounts belong to `local` and keep
listing.

Three rules the tests pin:

- **Listing is filtered by owner**, on both backends. `owner` is a parameter, not
  a query field: a caller who could name it could read anyone's saves.
- **Deleting checks ownership** against a fresh listing first. Neither backend
  does this for us — the downloads project is shared, and a private asset id is
  only scoped by the URL we build. A save that is not yours answers exactly like
  one that does not exist, so the two cannot be told apart by probing.
- **The five-per-game limit is per person.** One player filling their slots must
  not evict somebody else's progress.

`POST /api/saves/claim` moves the pre-accounts saves to the signed-in caller.

A save's name is part of its filename, and neither backend can rewrite an
object's key in place, so `PATCH` is an upload under the new name followed by a
delete of the old one — the same order an overwrite uses, and for the same
reason: the copy is stored before the original goes, so a failure anywhere
leaves the save where it was. The bytes make a round trip through the server to
do it, which is the price of the name living in the key; it is paid on a
rename, which is rare, and not on a save, which is not. A name may hold letters,
numbers and hyphens — underscore separates the fields of the filename, so it
cannot appear inside one — and the page trims as you type with the same rule so
what is accepted is what is stored.
Neither backend can rename an object, so each is re-uploaded under the new owner
and the original removed afterwards, preserving timestamps and labels. It is a
one-time migration and whoever claims them gets them: they have no owner to
check against. That suits a server you run yourself and would not suit a public
one.

### Cloud saves

Save states go to the storage host's **downloads** API, not the media vault —
the vault only takes image, audio, and video and verifies the bytes match.

| Route | Purpose |
|---|---|
| `GET /api/saves?game=BPRE` | saves for one cartridge, newest first |
| `POST /api/saves` | store one; `file`, `game`, optional `label` and `replace` |
| `PATCH /api/saves/{id}` | rename one; JSON `{"label": "..."}` |
| `DELETE /api/saves/{id}` | remove one |
| `GET /api/proxy?url=…` | relay storage bytes to the browser |

**Five per game.** On a successful upload the server lists that game's saves and
deletes the oldest beyond five. Pruning happens *after* the upload lands, never
before — losing the oldest save to make room for one that then fails would be
the worst outcome. The retention rule is a pure function, `saves_to_evict`, so
it is tested on its own.

"Per user" is the API key: one key, one project namespace. A key shared between
people shares the quota.

**Naming.** Everything needed lives in the name, which the API preserves as
`originalName`:

```
tb_BPRE_1787570960801_route4.savestate
│  │    │             └── optional label
│  │    └── milliseconds since the epoch
│  └── cartridge code, which is what the per-game limit keys on
└── marks the file as ours, so other things in the project are left alone
```

**No `type` field is sent.** Since storage 0.5.0 the server infers the extension
from the filename and checks it against the project's
`allowedDownloadExtensions`; `type` and `filetype` are deprecated fallbacks. A
rejection returns a structured detail naming the allowed list, which the UI
shows verbatim. Saves written under the older `.bin` naming still list and load.

**Where saves live is decided by the project, not by this code.**
`GET /api/vaults/{vaultId}/config` is read at the time of use:

| Condition | Backend |
|---|---|
| `privateEnabled` and `allowedPrivateExtensions` contains `.savestate` | private, owner-scoped, expiring signed URLs |
| otherwise | public `/downloads` URLs |

Private is preferred — a save is someone's progress and does not belong on a
guessable public URL. To switch, an administrator adds the extension:

```bash
curl -X PATCH https://media.0xstash.dev/api/vaults/{vaultId}/config   -H "Authorization: Bearer $MASTER_KEY" -H 'Content-Type: application/json'   -d '{"allowedPrivateExtensions":[".jpg",".png",".webp",".gif",".mp3",".ogg",
       ".wav",".m4a",".mp4",".webm",".mov",".sav",".save",".savestate"]}'
```

No restart or code change is needed; the panel starts reporting `private`.

**Owner identity.** Private assets are scoped by `ownerId`, which comes from
`TINYBIRD_MEDIA_OWNER` (default `local`). tinyBird has no user accounts — the
server runs on your machine with your key — so one install is one owner. A
multi-user deployment must derive it from an authenticated session and never
from anything the browser sends.

**What is in a save state, and what is not.** Around 630 KB raw, gzipped by the
browser with `CompressionStream` to roughly 160 KB before it is uploaded. Two
things are deliberately left out, both added back on restore:

| Left out | Why | Was costing |
|---|---|---|
| the cartridge | the player has it; a state is only ever restored with the same game inserted | up to 32 MB |
| the rendered picture | drawn again from video memory as soon as the state runs | 1.1 MB |

Version 4 of the state format is what leaves them out. Earlier states carry both
and still load — the fields are still there, written empty, so the layout has not
changed and a version 3 blob deserializes exactly as it always did.

This is not a micro-optimisation. Before it, one save was 17.7 MB raw and 5.3 MB
gzipped, and storing one took between twelve and thirty seconds because the whole
thing crossed the network twice: once from the browser to the server, once from
the server to storage, with the prune and any overwrite waiting behind it. Over a
tunnel that was long enough for the connection to be dropped, which reached the
player as `Could not save: Failed to fetch`. The same save now takes about two
seconds.

Restoring a version 4 state needs the cartridge already loaded. Without one the
core refuses rather than restoring a console with an empty slot, and says so.

#### Cartridge saves are not save states

A save state is a photograph of the whole machine. A **battery save** is what
the cartridge itself keeps, and it is what the game writes when the player picks
"save" from a menu. They are not interchangeable, and a page that persists only
save states loses progress the game believes it has already committed — which is
what a game means when it reports a slot as corrupt or missing.

The browser keeps both:

| Where | What |
|---|---|
| `localStorage`, keyed by game code | the battery save, written whenever the game touches the cartridge |
| inside every vault save | the battery save, alongside the state and the screenshot |

Restoring either one restores both halves together. `tb_battery_dirty()`
consumes a flag rather than reporting a level, so the page copies the save only
when the game has actually written to it rather than several times a second.

All four GBA backup types work: SRAM, flash 64K, flash 128K, and EEPROM.

#### The save container

A stored save carries a screenshot so a slot can be told apart at a glance. The
image lives *inside* the save rather than beside it, so the two can never orphan
or disagree: one object to upload, prune, and delete.

```
0..4   "TBCT"
4      container version
5      flags, bit 0 = the state that follows is gzipped
6..10  thumbnail length, u32 little-endian
10..   thumbnail (PNG, 240x160)
then   the save state
```

Reading a thumbnail costs one ranged request for the first 32 KB rather than the
whole save, which is what makes showing them in a list affordable. The
rest of the save stays on the CDN until someone actually loads it. Thumbnails
are decorative: every failure path leaves the placeholder, because a missing
screenshot must never stop a save from being loadable.

`unpackSave` accepts three shapes — a container, a bare gzip blob, and a raw
state — so a save from any earlier version, or one written by the desktop app,
still opens.

> **The magic must not be `TBSV`.** That is what the emulator core already writes
> at the head of every save state ([`gba.rs`](../crates/tinybird-core/src/gba.rs)).
> The first version of this container reused it, so every raw state parsed as a
> container whose "thumbnail length" was really the state's version field, and
> loading one failed with `TB_ERR_FAILED`. The header check now validates the
> version and rejects an implausible thumbnail length as well, since a four-byte
> match is weak evidence and misreading a state corrupts it silently rather than
> failing loudly.

The format is deliberately free of any DOM reference so it can be tested outside
a browser:

```bash
node --test crates/tinybird-web/src/assets/saveformat.test.mjs
```

**The browser fetches storage directly.** Storage 0.5.0 sends
`Access-Control-Allow-Origin: *` on the public delivery plane, so ROM and save
bytes go straight from the CDN to the page and never touch this server. It also
supports `HEAD` with `Content-Length` and byte ranges (`206` with
`Content-Range`).

`/api/proxy` remains as a fallback for when a direct fetch fails: it relays
bytes and refuses any URL not on the configured storage host, so it cannot
become an open relay. Without it, a CORS regression upstream would surface as
nothing but "Failed to fetch".

### Who can see what

The vault is one bucket shared by everything, so what keeps files apart is the
route that lists them, not the store.

| Route | Scope | Why |
|---|---|---|
| `/api/library` | Everyone, **cartridges only** | A shared ROM library is the point of it |
| `/api/saves` | The signed-in account | Save states belong to whoever made them |
| `/api/shots` | The signed-in account | So do screenshots |

Two rules make that hold:

**The owner is part of the stored name, and the server writes the name.** The
vault has no notion of an owner, so a filename is all there is to say whose
file this is — which means a caller who could choose the name could file a
picture under someone else's account, or read theirs by asking for a listing of
it. `/api/shots` builds the name from the session; nothing the browser sends is
trusted. It is the same rule `/api/saves` follows, for the same reason.

**The unscoped route returns cartridges and nothing else.** `/api/library` is
deliberately not scoped, so everything it returns is visible to anyone who can
reach the server. It used to list the whole vault, which advertised every
screenshot in it — name, size and a working URL — to any visitor. The page had
always filtered to ROM extensions before drawing anything, so it *looked*
right; the filtering is now on the server, where it is a boundary rather than a
formatting choice.

Filtering in the page would have made privacy a setting anyone could turn off
in the developer tools.

**What is not solved yet.** A vault URL is a public CDN URL: it is unlisted,
not protected, so anyone given one can open it. That is fine for a picture you
chose to share and wrong as a privacy guarantee, and the private-backend path
that `/api/saves` can use has no equivalent here yet.

**Screenshots taken before this** were named by the browser and carry no owner,
so they parse as nobody's and appear in no listing. Unreachable rather than
everybody's is the safe direction for that mistake to fall, but they are still
in the vault and still on public URLs.

### What the vault will and will not take

Verified against the live API on 2026-08-24: it accepts **image, audio, and
video only**, and it sniffs the bytes to confirm they match the declared type.
Mislabelling does not get around it.

| Asset | Result |
|---|---|
| Screenshots (`image/png`) | works — this is what `Screenshot to vault` uses |
| Sprites, artwork, wallpapers | works |
| Save states, ROMs | rejected — these go to the downloads API instead |

Save states use the downloads API, which does accept arbitrary bytes.

Because of the content check, the browser's own content type has to reach the
API — the upload route passes it through rather than substituting one.

### A note on what you upload

Vault reads are **public to anyone with the URL**. That is what makes the CDN
fast and keyless, and it is the right trade for screenshots and artwork. Think
about it before putting commercial ROMs there; the local `roms/` folder exists
so a ROM you would rather not publish can still be played.

---

## 6. The page

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
|               |                                 | [Team][Saves] |
|   read-out    |        [ control deck ]         +---------------+
|   move -->    |    [Controls]    [Lobby]        |   <-- move    |
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
| Lobby panel | A dialog | Hosting or joining happens once at the start of a session. What lasts — the watched screen, and whether the cable is live — stayed on the page: local and watched screens become equal columns above the controls, stack on a narrow display, and the deck key carries a badge. |

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
because `putImageData` ignores scaling — and that canvas is what screenshots,
save thumbnails and shared lobby frames read, so a capture is always the
picture the game drew. A `ResizeObserver` on the bezel re-runs it, so the
window and full screen are the same code path.

> **Every element gets one name in the `el` map.** A duplicate key in an object
> literal is silent in JavaScript: the last one wins. Adding the lobby's
> `screen` image under a key that the console's `screen` bezel already used
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

### What you can do to a stored save

Each row in **Saves** carries its screenshot, when it was written, its size, and
three actions:

| Action | What happens |
|---|---|
| `overwrite` | replaces that slot with the game as it is now, keeping its label |
| `download` | saves it to disk as a plain `.state` |
| `delete` | removes it |

**Overwrite and delete both arm before they act.** The first click changes the
label (`replace?`, `sure?`); the second does the work. A stored save is the only
copy of that progress and there is no undo. The armed state lapses after four
seconds rather than staying hot, so a forgotten click cannot be completed later
by someone aiming at the button beside it.

**An overwrite is an upload followed by a delete, in that order.** Neither
storage backend can rewrite an object in place — the timestamp is part of the
name — so the old save is removed only once the replacement has landed. A failed
overwrite therefore costs nothing. The label is carried across so the row still
reads as the same slot, and the timestamp is new, because it is a new save
point.

**A download is unpacked on the way out.** What lands on disk is the raw save
state, not this page's container: the desktop build reads save states, and a
file you cannot open in the other half of the project is not much of a download.

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

### Shareable links

`/play?rom=<url>&name=<label>` boots straight into a ROM, so a vault asset can
be shared as a link.

### The contact form

`/contact` is the form, backed by the
[0xstash contact API](https://contact.0xstash.dev/api/help). Optional: without
`TINYBIRD_CONTACT_KEY` the panel says the form is switched off and who can turn
it on, and the home page's signpost card for it does not appear — a card
promising to reach a person should not lead to a form that cannot send. The nav
link stays either way: it is a fixed bar on every page, and the page it leads to
explains itself.

It began as a panel at the foot of the home page, which put a form nobody had
come for underneath everything they had. Its own page also gives the sign-in
room to be a sign-in rather than a gate bolted to the front of something else.

| Variable | Default | Meaning |
|---|---|---|
| `TINYBIRD_CONTACT_KEY` | none | Project form key. The form is hidden without it. |
| `TINYBIRD_CONTACT_ORIGIN` | none | `Origin` to present to the service. |
| `TINYBIRD_CONTACT_URL` | `https://contact.0xstash.dev` | API base URL. |

| Route | Purpose |
|---|---|
| `GET /api/contact` | whether there is a form, whether it needs a sign-in, and who it would send as |
| `POST /api/contact` | `name`, `subject`, `message`, optional `category` and `siteUrl`; plus `email` only when accounts are off |

A `202` from a signed-in sender carries `ticket`, the id the service filed
the message under, and the form turns it into a link straight to
`/support/tickets/{id}` — knowing there is a ticket and being able to open
it are different things. The id is checked as an id before it is handed to
the page, because the page puts it in a URL; a service that answers with
something else costs the sender a link and nothing more. It is withheld
without a session, where the message was never bound to an account and the
link would only lead to a panel saying so.

That leaves the honeypot's `202` distinguishable from a real one, which it
was not before. Telling them apart needs a bot that has registered, verified
an address, signed in, and then filled in a field it cannot see — by which
point the per-account quota is the limit doing the work.

**Sending requires an account** wherever accounts exist — that is,
whenever `TINYBIRD_AUTH_PROJECT_SECRET` is set. Without it there is nobody to
sign in as, the server is one person's own machine, and the form takes a name
and an address the way any contact form does. The same `is_configured()` fork
`save_owner` and `lobby_identity` use.

**A signed-in sender's address comes from their session**, and the body's is
ignored. This is the whole point of requiring the account: a form that demands
a sign-in and then lets the page name the reply address has bought nothing,
because the reply address is the part that matters. It is how a signed-in
stranger would ask for somebody else's account to be looked at. So the email
field goes away when signed in and the panel shows *Signed in as …* instead —
offering an input whose contents are discarded is a lie the page would tell.

**The name is still asked for.** It is a courtesy, not an identity claim: a
username is a handle and often not a name at all, and whoever answers a support
message would rather have something to call you. The field starts filled with
the account's username, so leaving it alone is a valid answer, and typing over
it changes nothing about who the message is from. Both travel with it — what
was typed as `name`, and the account's own handle as `metadata.username`,
beside the `sub` claim in `metadata.account`. Two fields rather than one,
because collapsing them means either losing the handle support searches by or
addressing somebody as `ash_1996`.

**Signing in is the bar's account menu**, the same one every page carries, not
a login box on this page. `/contact` shows a sentence saying so where the form
would be, and redraws itself when the menu reports a change — a second login
form here would be a second thing to keep in step for no reason.

**The page does not hold the form key.** The published integration has the
browser post straight to `contact.0xstash.dev` with the key in an
`Authorization` header, pinned to a registered `Origin`. That fits a hosted
marketing site. It does not fit this: tinyBird is a server people run on their
own machine, so the page's origin is `http://127.0.0.1:8877`, which nobody can
register, and a key shipped to that page would be a key committed to this
repository. So the same arrangement as accounts and storage:

```
browser ──POST /api/contact──▶ tinyBird ──bearer form key──▶ contact service
```

`TINYBIRD_CONTACT_ORIGIN` exists because of the half of that trade which does
cost something. A request from this server carries no `Origin` of its own, so
if the key is registered against specific origins, name one here and it is sent.
Left unset nothing is sent, which is right for a key with no origin restriction.

**Validation happens twice.** The service asks for a name, an address, a
subject, and ten characters of message; [`contact.rs`](../crates/tinybird-web/src/contact.rs)
checks the same things before spending a request, so the form can say which
field is wrong immediately and in its own words. It also caps the message at
4000 characters — not a service limit, a ceiling so a relayed request cannot
push megabytes through this server's key.

**The honeypot is answered, not refused.** The form carries a `website` field
positioned off screen and out of the tab order; a person never sees it, so
anything in it came from a script. A filled one is answered with the same `202`
a real message gets and nothing is sent. Telling the sender which field gave
them away only teaches them to leave it alone next time.

**Three limits, in front of each other.** The form spends this server's key, so
on a public host it is otherwise an open relay into somebody's inbox.

| Limit | Value | Refusal |
|---|---|---|
| Cooldown between one sender's messages | 60s | `429`, `Cooldown` |
| One sender's messages per hour | 3 | `429`, `Quota` |
| Whole process, whoever is sending | 30 per 10 min | `429`, `Throttled` |

Counted per account, not per address. That only became worth doing once
sign-in was required: an address is typed by whoever is sending, and anyone
varying it walks straight past a per-address limit, whereas a `sub` claim is
not theirs to vary. The process ceiling stays underneath as a backstop — it is
what a pile of fresh accounts runs into, and what covers the no-accounts case,
where every message counts against the single `anonymous` sender.

The first two refusals say **how long**, in words rather than seconds
(`about 40 minutes`, not `2400`), and set `Retry-After` for anything in front
of the server reading headers rather than sentences. The process ceiling sets
no `Retry-After`: when it clears depends on what everyone else does, and a
promised moment that turns out not to be one is worse than no promise.

A refusal by the ceiling does not spend the sender's own allowance — the checks
all run before anything is recorded, so a message stopped by one limit is not
also charged against another.

### Tickets

A sent message becomes a ticket, and the service keeps the conversation:
the original, whatever support replied, whatever the sender said next.
`/support/tickets` is that list and `/support/tickets/{id}` is one of them —
the same document either way, drawing whichever the path names, so a ticket
link survives being mailed, pasted and reloaded.

| Route | Purpose |
|---|---|
| `GET /api/tickets` | the signed-in account's tickets; optional `limit` and `offset` |
| `GET /api/tickets/{id}` | one ticket |
| `GET /api/tickets/{id}/messages` | that ticket's conversation |
| `POST /api/tickets/{id}/messages` | `message` and `idempotencyKey` |

**The credentials swap places.** A submission goes out under this server's form
key with the sender named in an `X-0xstash-User-Token` header; a ticket read
goes out the other way round — the sender's access token is the bearer, and the
form key rides in `X-Contact-Project-Key`. That is not an inconsistency in the
API, it is the security model. A ticket read is authorised by whoever the token
introspects to, and the project key only says which project is being asked
about. The key alone reads nothing, and neither does a request id: a guessed one
answers exactly as a wrong one does.

```
browser ──GET /api/tickets──▶ tinyBird ──bearer session token──▶ contact service
                                        + X-Contact-Project-Key
```

**The token never reaches the page.** The published integration keeps the
access token in browser memory and calls the service directly. tinyBird cannot:
its origin is `http://127.0.0.1:8877`, which nobody can register, and it has no
in-browser token to keep — [`auth.rs`](../crates/tinybird-web/src/auth.rs) holds
sessions server-side behind an `HttpOnly` cookie. So the relay is the same one
the form uses, and it lands on the same side of every acceptance test: nothing
in `localStorage`, nothing in a URL, no key in a built asset, and no ticket
readable by anyone but the account that opened it.

**A stale token is retried once.** `current_session` refreshes anything within a
minute of expiring, so the ordinary case never reaches the service stale. When
it does anyway — a token revoked early, clocks apart by more than that minute —
the `401` triggers one refresh and one retry, and a second refusal is an answer.
`ticket_call` in `main.rs` is where that lives; a loop there would spend a
refresh token per attempt and never stop.

**Replies are idempotent, and the page owns the key.** The composer mints a
`crypto.randomUUID()` when a draft is first sent and keeps it in `localStorage`
beside the draft itself. A send that times out leaves both in place, so pressing
the button again carries the same `Idempotency-Key` and the service files one
message rather than two; both are cleared only once a send has actually
succeeded. A key minted per request would make every retry a new message, which
is the failure it exists to prevent. The server refuses a reply that arrives
without a usable one rather than inventing it — inventing it here would put the
key back on the wrong side of the retry.

**Nothing about a ticket's shape is assumed.** The service has published no
schema for one, so the relay passes its JSON through whole rather than reshaping
it, and [`tickets.js`](../crates/tinybird-web/src/assets/tickets.js) reads every
field through a `pick` that takes the first name actually present — `status` or
`state`, `createdAt` or `created_at`. A field nobody anticipated is simply not
drawn, and a renamed one keeps rendering. Message bodies are set as
`textContent` and shown `pre-wrap`: it is somebody's writing arriving over a
network, and the only safe thing to do with it is show it as writing.

**Which side said what is a guess, and says so.** The service names it
differently in different places, so a message that matches neither pattern is
left unattributed rather than credited to the wrong person — a support reply
shown as your own is worse than one shown as nobody's.

**Getting there is not an email round trip.** A signed-in sender's message is
bound to their `sub` claim as it goes, so the ticket is readable the moment the
service takes it. The form links straight to it and `/contact` carries a
standing link to the list. The emailed link still works and still matters — it
is what reaches somebody who has closed the tab — but nobody has to wait for it.

**The portal stays hosted.** Emailed ticket links open the service's own private
page, which is right while this is a thing people run on `localhost`: a link has
to work on whatever device opens the mail. Pointing the project's portal at
`https://gba.0xstash.dev/support/tickets/{ticketId}` is a change made in the
operator dashboard once that route is live, not from here.

---

## 7. Verifying it

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

Measured in the browser with FireRed running, all figures per call:

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
noise. It does make the wasm module 14% smaller (610KB to 523KB), which is
worth having on a page loaded over a network, so it stayed for that reason and
the comment in `Cargo.toml` says so.

#### What is left

Audio still costs about 2.7ms of a frame natively (8.5ms with, 5.8ms without),
and that remainder is real signal processing rather than call overhead —
`generate_sample` and the channel loops do the same work however they are
batched. The rest is the CPU and PPU.

The next thing worth doing is finding out where inside a frame the time goes:
CPU dispatch, the PPU scanline renderer, or bus reads. `step()` is a good place
to start looking — it does a lot of bookkeeping per instruction, including a
full bus read of the current opcode in `current_hle_swi_comment` purely to see
whether it is an SWI, on every instruction, when the result is only used if a
HALT follows.

---

## 8. Known gaps

Nothing outstanding on the five local dumps: Fire Red, Pokémon Pinball, Final
Fantasy Tactics Advance, The Minish Cap, and Super Mario Advance 4 all boot,
render, and save. Two emulator bugs that used to sit here are written up in
[ARCHITECTURE.md](ARCHITECTURE.md#two-bugs-worth-remembering).

- Sections carry pictures only where their addon named one per card. A
  `KeyValue` or `Table` section has no way to hold an image, so a team summary
  is text even when the party it summarises is not.
- Screenshots have no editing or deleting: the gallery lists what is in the
  vault and opens one full size, and removing a picture means removing it from
  the vault by hand.
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
- Addon manifests are fetched from `/api/addons` once at boot and cannot be
  changed without a reload, because the registry is built on first use.
- **Trading still stops short of a Pokémon changing hands.** The cable itself
  is done — the handshake completes, both players reach the same Trade Center,
  and position data crosses in both directions
  ([ARCHITECTURE.md](ARCHITECTURE.md#testing-it-with-a-real-cartridge)). What is
  missing is the last step: opening the trade menu needs a button pressed on an
  exact tile, which is a puppeteering problem rather than an emulation one.
- Species sprites need one network fetch each the first time a party is seen.
  After that they are on disk and the page works offline; a host that never had
  a network shows cards with no pictures, which is a supported way to run.

---

## 9. Hosting it publicly

Everything above assumes the server is yours and reachable only by you. Putting
it behind a name — `gba.0xstash.dev`, say — changes three things.

**Stop serving your ROM folder.**

```bash
TINYBIRD_LOCAL_ROMS=off
```

`/local/{path}` reads cartridge dumps off the server's disk. That is exactly
what you want on your own machine and exactly what you do not want facing the
internet. With it off, the library lists nothing local and the route answers
404; players load their own ROMs through the file picker or their own vault.

**Bind where the proxy can reach it.** Behind a `cloudflared` tunnel running on
the same host, the default `127.0.0.1` is right and nothing needs changing. On a
server where the proxy is elsewhere, set `TINYBIRD_WEB_HOST=0.0.0.0`.

**Nothing else needs configuring**, and one thing that would have needed it does
not: neither the auth service nor the contact service ever sees the new origin,
because the browser never talks to either. The server does, from wherever it
runs. That was the point of the backend-for-frontend arrangement in
[`auth.rs`](../crates/tinybird-web/src/auth.rs) and
[`contact.rs`](../crates/tinybird-web/src/contact.rs), and it means a new domain
needs no CORS registration.

The contact form needs no attention either, as long as accounts are on: it
requires a sign-in, limits each account to three messages an hour with a minute
between them, and caps the whole process at thirty per ten minutes. Those are
sized for a server a handful of people use. A busier one wants the numbers in
[`contact.rs`](../crates/tinybird-web/src/contact.rs) raised and a real limiter
in front of it — the counters live in memory, so a restart forgives everyone,
and more than one process would not share them.

The session cookie picks up `Secure` on its own: `is_secure` reads
`X-Forwarded-Proto`, which Cloudflare sets. WebSockets proxy through Cloudflare
without special handling, and the page picks `wss://` from its own location.

### What is still open

With accounts configured, opening or joining a room requires being signed in,
and a member's name comes from their session. Within a room, a code is still a
code: anyone holding one can join unless the host locks the room. That is the
right model for sharing a code with friends, and it is why nothing sensitive
travels through a room — only the read-out.

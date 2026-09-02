// Deterministic lockstep for the link cable.
//
// # Why the cable is not allowed on the network
//
// The obvious design is to relay halfwords: the parent says a transfer is
// starting, everyone answers with what they are putting on the wire, the parent
// hands the set back. That is what this page used to do, and it cannot be made
// fast enough. The numbers are not close.
//
// Pokémon's link driver arms Timer 3 at `-197` on the 64-cycle prescaler, so it
// starts a transfer every 12,608 cycles — about 751µs. It needs nine of them
// per frame: eight command halfwords and a checksum. At every VBlank the master
// checks that nine landed, and if they did not it sets `LAG_MASTER` and the
// game stops with "Communication error". So the budget is
//
//     (16.74ms frame - 9 x 751µs of wire time) / 9 transfers = 1.1ms
//
// per round trip, for 540 round trips a second. A WebSocket to localhost does
// not manage 1.1ms. A real network is twenty times worse, and because the nine
// trips are serialised, a 30ms round trip turns one frame into a third of a
// second. There is no timeout to tune and no slice size to pick that changes
// this: relaying the cable is arithmetically dead.
//
// # What this does instead
//
// Every browser runs *every* console. The cable is then a function call between
// two WebAssembly instances in one page — no network, no waiting, the same code
// path `Gba::link_step` uses in the core's own tests. What travels between
// players is one button mask per player per frame: 60 messages a second instead
// of 540 round trips, and none of them is on the critical path, because inputs
// are exchanged a few frames ahead of the frame that needs them.
//
// This is the design mGBA's lockstep coordinator uses locally, the one
// RetroArch's netplay uses over the wire, and the one the browser mGBA port
// documented at daudau.cc arrived at. Nobody has made the relay work, including
// mGBA, whose networked-link issue is still open.
//
// # What it costs and what it demands
//
// It costs emulation: two players means two cores per browser, so a frame costs
// twice what it did. It demands determinism, which is the real constraint —
// both browsers must compute bit-identical states from the same starting point,
// so the cartridge clock has to be seeded rather than read from `Date.now`, and
// every console starts from a save state the players exchanged rather than from
// wherever each happened to be.
//
// Divergence is not repaired. A state hash is compared every few seconds and a
// mismatch ends the session, because a link that has quietly diverged will
// write a corrupt Pokémon into somebody's cartridge, and stopping is better.

import { gzip, gunzip } from "./saveformat.js";

/**
 * Instructions per slice while stepping a console.
 *
 * Nothing waits on the event loop here — the cable is resolved in this process
 * — so this only has to be large enough that the per-call overhead disappears.
 * The core stops a slice by itself the moment a transfer begins, which is the
 * boundary that actually matters, so a slice almost never runs to this bound.
 */
const SLICE_STEPS = 0x40000;

/**
 * How many times round the stepping loop before declaring a frame wedged.
 *
 * A frame is nine transfers and a handful of slices, so double figures is
 * normal. This is four thousand: high enough that no legitimate frame reaches
 * it, low enough that a wedge is caught within one frame rather than hanging
 * the page.
 */
const MAX_ROUNDS = 4096;

/** Frames of input history kept. Must exceed `MAX_DELAY` by a wide margin. */
const RING = 256;

/** What an input slot holds before anybody has said what was pressed. */
const UNKNOWN = -1;

/** Frames of input delay, at the least. Below this, ordinary jitter starves. */
export const MIN_DELAY = 2;

/** Frames of input delay, at the most. Beyond this the pad feels detached. */
export const MAX_DELAY = 12;

/** How often the two browsers compare what they have computed. */
export const HASH_INTERVAL = 300;

/** Every byte this many apart goes into the state hash. */
const HASH_STRIDE = 7;

/** The largest link message we will build or accept, before base64. */
export const MAX_STATE_BYTES = 4 * 1024 * 1024;

/**
 * Frames of input delay for a measured round trip.
 *
 * To run frame F this browser needs the other player's mask for F, and they
 * sent it when they ran frame F minus the delay. So the delay has to cover the
 * one-way trip, which is half the round trip, plus a frame for the jitter that
 * a mean does not describe.
 *
 * Delay is what lockstep has instead of rollback. Rollback would hide it, at
 * the price of a save state and a re-simulation per mispredicted frame — 540
 * of them a second here, on a state that costs milliseconds to serialise. For
 * games played in menus, which is what a link cable is for, three frames of
 * delay is not perceptible and rollback is not worth its complexity.
 */
export function delayForRoundTrip(ms) {
  const oneWay = Math.max(0, ms) / 2;
  const frames = Math.ceil(oneWay / (1000 / 60)) + 1;
  return Math.min(MAX_DELAY, Math.max(MIN_DELAY, frames));
}

/**
 * Carry one transfer between consoles in this process.
 *
 * A direct port of `Gba::link_step`. The parent is the only console that may
 * begin a transfer; everyone else is pulled into it whether they were ready or
 * not, because that is what the parent driving the clock line means. A seat
 * with nobody in it reads as `0xFFFF` — the hardware's "absent", which games
 * already understand — rather than zero, which they would take for data.
 *
 * Returns whether a transfer was carried.
 */
export function carryTransfer(consoles) {
  const parent = consoles[0];
  if (consoles.length < 2 || !parent?.linkPending) return false;

  for (let i = 1; i < consoles.length; i += 1) consoles[i].linkJoin();

  const values = [0xffff, 0xffff, 0xffff, 0xffff];
  for (let i = 0; i < consoles.length && i < 4; i += 1) {
    values[i] = consoles[i].linkSendValue;
  }

  // The parent clocks the cable, so its baud rate is the cable's. A child
  // timing the wire from its own register would hold it twelve times too long
  // — Pokémon leaves children at 9600 while the parent moves to 115200 — and
  // miss the next transfer entirely.
  const cycles = parent.linkTransferCycles;
  for (const console of consoles) console.linkDeliver(values, cycles);
  return true;
}

/** A short, stable fingerprint of some bytes. Used to match cartridges. */
export async function romFingerprint(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest, 0, 8)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

/** Compress and base64 a state for the wire. */
export async function packState(bytes) {
  const { bytes: squeezed } = await gzip(bytes);
  let binary = "";
  for (const byte of squeezed) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/** Undo `packState`. */
export async function unpackState(text) {
  const binary = atob(text);
  const squeezed = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) squeezed[i] = binary.charCodeAt(i);
  // `gzip` passes bytes through untouched where the browser has no
  // CompressionStream, so a state may arrive uncompressed.
  if (squeezed[0] !== 0x1f || squeezed[1] !== 0x8b) return squeezed;
  return new Uint8Array(await gunzip(squeezed));
}

/**
 * One linked session: every console, and the inputs that drive them.
 *
 * The session owns emulated time. Nothing else may run a console while one
 * exists, because a frame run outside it is a frame the other browser did not
 * run, and that is a desync.
 */
export class LinkSession {
  /** Consoles by seat. `consoles[mySeat]` is the page's own emulator. */
  consoles;
  /** Which seat this browser plays. */
  mySeat;
  /** The frame about to be run. Counts from zero at the start of the session. */
  frame = 0;
  /** Frames between pressing a button and the frame that sees it. */
  delay;
  /** Identifies this session, so a stale message from the last one is ignored. */
  id;
  /** Set when the two browsers have computed different states. */
  desyncedAt = null;
  /** Set when a frame could not be completed. */
  wedged = false;

  /** Input masks by frame and seat, as a ring. `UNKNOWN` where not yet known. */
  #inputs;
  /** State hashes this browser computed, by frame, awaiting a peer's word. */
  #mine = new Map();
  /** State hashes the peer sent for frames this browser has not reached. */
  #theirs = new Map();

  constructor({ id, consoles, mySeat, delay }) {
    this.id = id;
    this.consoles = consoles;
    this.mySeat = mySeat;
    this.delay = delay;
    this.#inputs = new Int32Array(RING * consoles.length).fill(UNKNOWN);

    // Nothing was pressed before the session began, and saying so lets the
    // first frames run without waiting for a message that describes a time
    // when there was no session to describe.
    for (let frame = 0; frame < delay; frame += 1) {
      for (let seat = 0; seat < consoles.length; seat += 1) this.#set(frame, seat, 0);
    }
  }

  get players() {
    return this.consoles.length;
  }

  /** The page's own console: the one that is drawn, heard and saved. */
  get local() {
    return this.consoles[this.mySeat];
  }

  #slot(frame, seat) {
    return (frame % RING) * this.consoles.length + seat;
  }

  #set(frame, seat, keys) {
    this.#inputs[this.#slot(frame, seat)] = keys & 0x3ff;
  }

  #get(frame, seat) {
    return this.#inputs[this.#slot(frame, seat)];
  }

  /**
   * Record what this player is pressing, and say which frame it belongs to.
   *
   * Returns the frame to publish, or `null` if the ring has already been told
   * about it — which happens when the session is stalled and `tick` runs again
   * before a frame has been consumed.
   */
  pushLocal(keys) {
    const frame = this.frame + this.delay;
    if (this.#get(frame, this.mySeat) !== UNKNOWN) return null;
    this.#set(frame, this.mySeat, keys);
    return frame;
  }

  /**
   * Take a mask another player sent.
   *
   * A frame already behind us cannot be used and a frame beyond the ring
   * cannot be stored; both mean this browser and the sender have drifted
   * further apart than the buffer covers, which the caller reports rather than
   * papers over.
   */
  acceptInput(seat, frame, keys) {
    if (seat === this.mySeat || seat < 0 || seat >= this.consoles.length) return false;
    if (!Number.isSafeInteger(frame) || frame < this.frame) return false;
    if (frame >= this.frame + RING) return false;
    this.#set(frame, seat, keys);
    return true;
  }

  /** Whether every player's mask for the next frame is in. */
  get ready() {
    if (this.desyncedAt !== null || this.wedged) return false;
    for (let seat = 0; seat < this.consoles.length; seat += 1) {
      if (this.#get(this.frame, seat) === UNKNOWN) return false;
    }
    return true;
  }

  /**
   * How many frames of other people's input are buffered ahead of this one.
   *
   * A healthy session sits at roughly the delay. Zero means the next frame is
   * waiting on the network, which is the only thing that can stall a lockstep
   * session and the number worth putting in front of a player.
   */
  get bufferedFrames() {
    let ahead = 0;
    for (let frame = this.frame; frame < this.frame + RING; frame += 1) {
      let complete = true;
      for (let seat = 0; seat < this.consoles.length; seat += 1) {
        if (this.#get(frame, seat) === UNKNOWN) {
          complete = false;
          break;
        }
      }
      if (!complete) break;
      ahead += 1;
    }
    return ahead;
  }

  /**
   * Run one frame on every console.
   *
   * The consoles are stepped in slices rather than whole frames because a
   * console stops the instant a transfer begins, and the transfer has to be
   * carried before it can go on. That is the whole loop: slice everybody who
   * can run, carry whatever the parent raised, repeat until every console has
   * finished the frame.
   *
   * Returns false if the frame could not be completed, which means something
   * is wrong with the cable rather than with the network.
   */
  runFrame() {
    const cores = this.consoles;
    const targets = new Array(cores.length);
    for (let seat = 0; seat < cores.length; seat += 1) {
      cores[seat].setButtons(this.#get(this.frame, seat));
      targets[seat] = cores[seat].frameCount + 1;
    }

    let finished = false;
    for (let round = 0; round < MAX_ROUNDS && !finished; round += 1) {
      let advanced = false;

      for (let seat = 0; seat < cores.length; seat += 1) {
        const core = cores[seat];
        // A console with a transfer outstanding must not run: emulated time
        // may not cross a transfer that has not been carried. Carrying it is
        // the next thing this loop does.
        if (core.frameCount >= targets[seat] || core.linkPending) continue;
        core.runSlice(SLICE_STEPS);
        advanced = true;
      }

      if (carryTransfer(cores)) advanced = true;

      finished = true;
      for (let seat = 0; seat < cores.length; seat += 1) {
        if (cores[seat].frameCount < targets[seat]) {
          finished = false;
          break;
        }
      }

      // Nobody ran and nothing was carried: the cable is in a state this loop
      // cannot get out of. Stopping beats spinning.
      if (!finished && !advanced) break;
    }

    if (!finished) {
      this.wedged = true;
      return false;
    }

    // The frame is spent. Clearing it keeps the ring honest: a slot that still
    // held last time round's mask would let a frame run on stale input if the
    // ring ever wrapped onto it before a message arrived.
    for (let seat = 0; seat < cores.length; seat += 1) {
      this.#inputs[this.#slot(this.frame, seat)] = UNKNOWN;
    }
    this.frame += 1;
    return true;
  }

  /** Whether the frame just run is one the two browsers should compare. */
  get hashDue() {
    return this.frame > 0 && this.frame % HASH_INTERVAL === 0;
  }

  /**
   * A fingerprint of every console's state.
   *
   * Strided rather than exhaustive: a divergence large enough to matter — a
   * different Pokémon, a different RNG seed, a different frame counter — moves
   * far more than one byte in seven, and a full pass over two 650KB states
   * every frame it is asked for is not worth the certainty it buys.
   */
  hash() {
    let value = 0x811c9dc5;
    for (const core of this.consoles) {
      const bytes = core.saveState();
      for (let i = 0; i < bytes.length; i += HASH_STRIDE) {
        value ^= bytes[i];
        value = Math.imul(value, 0x01000193);
      }
    }
    return value >>> 0;
  }

  /**
   * Record what this browser computed, and check it against the peer.
   *
   * Returns the frame at which the two disagree, or null.
   */
  recordHash(frame, value) {
    const theirs = this.#theirs.get(frame);
    if (theirs === undefined) {
      this.#mine.set(frame, value);
    } else {
      this.#theirs.delete(frame);
      if (theirs !== value) this.desyncedAt = frame;
    }
    this.#forget();
    return this.desyncedAt;
  }

  /** Take a peer's fingerprint, comparing it if this browser has reached it. */
  acceptHash(frame, value) {
    const mine = this.#mine.get(frame);
    if (mine === undefined) {
      this.#theirs.set(frame, value);
    } else {
      this.#mine.delete(frame);
      if (mine !== value) this.desyncedAt = frame;
    }
    this.#forget();
    return this.desyncedAt;
  }

  /** Drop fingerprints for frames the other side is never going to answer. */
  #forget() {
    const oldest = this.frame - HASH_INTERVAL * 4;
    for (const map of [this.#mine, this.#theirs]) {
      for (const frame of map.keys()) if (frame < oldest) map.delete(frame);
    }
  }

  /**
   * Unplug every console.
   *
   * The page's own emulator survives — it is the one with the player's game in
   * it — so only the seats this session created are given up.
   */
  detach() {
    for (let seat = 0; seat < this.consoles.length; seat += 1) {
      const core = this.consoles[seat];
      try {
        core.linkDisconnect();
      } catch {
        // A console that is already gone does not need unplugging.
      }
    }
    this.consoles = [this.local];
  }
}

/**
 * Build a session from what every player published about themselves.
 *
 * `seats` is one entry per console, in seat order, each carrying the state that
 * seat starts from and the fingerprint of the cartridge it needs.
 *
 * Every console is restored from an exchanged state, **including this
 * browser's own**. That looks wasteful and is the point: the player has been
 * running frames since they published theirs, and if this browser carried on
 * from where it got to while the other restored from the published state, the
 * two would begin the session already disagreeing. Losing the second or two
 * since the state was taken is the price of starting from the same place.
 */
export async function openSession({
  id,
  seats,
  mySeat,
  localEmu,
  delay,
  seed,
  bios,
  makeConsole,
  resolveRom,
}) {
  const consoles = new Array(seats.length);

  for (const entry of seats) {
    const core = entry.seat === mySeat ? localEmu : await makeConsole();
    const rom = await resolveRom(entry);

    core.loadRom(rom);
    // Before the state, which carries the BIOS with it but only if the state
    // was taken on a machine that had one.
    if (bios) core.loadBios(bios);
    core.loadState(entry.state);

    // The cartridge clock is the one thing in the machine that does not come
    // from the state: the core cannot read a clock on wasm32, so the host says
    // what time it is, and if the two hosts say different things Ruby, Sapphire
    // and Emerald diverge within a frame. One seeded value, pushed once, and
    // the clock then runs from the cycles the machine actually executes — which
    // is also the only sense in which a linked cartridge clock can be right.
    core.setWallClock(seed);
    core.linkConnect(entry.seat, seats.length);
    consoles[entry.seat] = core;
  }

  return new LinkSession({ id, consoles, mySeat, delay });
}

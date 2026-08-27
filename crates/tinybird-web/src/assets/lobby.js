// Joining a room, and staying in it.
//
// The connection details only: what a room *is* lives on the server, and what
// a member's game looks like is the overlay's job. Kept free of DOM references
// so the reconnect policy can be checked without a browser — `lobby.test.mjs`
// does that.
//
// Nothing here emulates anything. Each member runs their own emulator; what
// travels between them is the addon snapshot, which is a read-out rather than
// a game state.

/** Where to connect for a room, from the page's own location. */
export function socketUrl(location, room, name) {
  // `wss` when the page is served over TLS. Getting this wrong is invisible
  // locally and fails outright once the site is behind HTTPS.
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const query = new URLSearchParams({ room, name });
  return `${scheme}//${location.host}/api/lobby/ws?${query}`;
}

/**
 * How long to wait before trying again after the connection drops.
 *
 * Backs off so a server that is down does not get hammered, but starts short
 * because the common case is a laptop lid closing for a moment.
 */
export function retryDelay(attempt) {
  const base = 500 * 2 ** Math.min(attempt, 5);
  // Spread retries out, so everyone kicked off at once does not come back at
  // the same instant.
  return base + Math.floor(Math.random() * 250);
}

/** How often a member republishes their read-out. */
export const SNAPSHOT_INTERVAL_MS = 500;

/**
 * How often a member sharing their screen sends a picture of it.
 *
 * Ten a second, not sixty. A frame is a few kilobytes where a read-out is a few
 * hundred bytes, and it is relayed to everyone in the room — so this is the
 * number that decides whether a room of four is comfortable or is a hundred
 * kilobytes a second going out of somebody's house.
 */
export const FRAME_INTERVAL_MS = 100;

/**
 * How hard to compress a shared frame.
 *
 * Pixel art survives JPEG badly at low settings, and the picture is only 240 by
 * 160, so there is not much to save by going lower.
 */
export const FRAME_QUALITY = 0.6;

/**
 * How long the parent waits for a console before transferring without it.
 *
 * A link transfer cannot complete until every console has answered, so a
 * player who closed their tab mid-trade would otherwise hang the others
 * forever. Going ahead leaves their slot reading as absent, which is exactly
 * what the hardware does when a cable comes out, and every game already knows
 * how to handle it.
 *
 * Long enough to survive an ordinary hiccup — a stalled tab, a slow phone —
 * and short enough that a real disconnection is not mistaken for one.
 */
export const LINK_TIMEOUT_MS = 2000;

/**
 * How many frames a child may be ahead of the parent.
 *
 * The parent is the clock. It is already held back by every transfer — it
 * cannot finish a frame until the children have answered — so only the
 * children need restraining, and one message a frame does it with no round
 * trip of its own.
 *
 * Zero would pin a child exactly one frame behind and starve it whenever a
 * tick was late. One lets it draw level with the parent and no further, which
 * is as close as two machines on a network get.
 */
export const LINK_MAX_LEAD = 1;

/**
 * How long a child waits for the parent's clock before running regardless.
 *
 * A barrier that never lifts is a hang, and a paused or closed parent must not
 * take the other player's game down with it.
 */
export const LINK_TICK_TIMEOUT_MS = 1500;

/**
 * A member's connection to one room.
 *
 * Reconnects on its own. `onMembers` and `onSnapshot` are called as messages
 * arrive; `onStatus` reports the connection itself so the page can say whether
 * it is connected without inventing its own state machine.
 */
export class LobbyConnection {
  #socket = null;
  #attempt = 0;
  #closed = false;
  #timer = 0;
  #local = null;
  #localIds = new Set();

  constructor({
    room,
    name,
    onMembers,
    onSnapshot,
    onFrame,
    onLinkTick,
    onLinkStart,
    onLinkValue,
    onLinkData,
    onStatus,
    onRefused,
    onLocked,
    open,
    openLocal,
  }) {
    this.room = room;
    this.name = name;
    this.you = null;
    this.members = [];
    this.onMembers = onMembers ?? (() => {});
    this.onSnapshot = onSnapshot ?? (() => {});
    this.onFrame = onFrame ?? (() => {});
    this.onLinkTick = onLinkTick ?? (() => {});
    this.onLinkStart = onLinkStart ?? (() => {});
    this.onLinkValue = onLinkValue ?? (() => {});
    this.onLinkData = onLinkData ?? (() => {});
    this.onStatus = onStatus ?? (() => {});
    // A refusal is final: a room that does not exist will not start existing
    // because we asked again, so retrying would spin forever.
    this.onRefused = onRefused ?? (() => {});
    this.onLocked = onLocked ?? (() => {});
    this.refused = false;
    this.locked = false;
    // Injectable so the reconnect policy can be tested without a server.
    this.open = open ?? ((url) => new WebSocket(url));
    this.openLocal =
      openLocal ??
      ((name) =>
        typeof window.BroadcastChannel === "function" ? new window.BroadcastChannel(name) : null);
    this.#connect();
  }

  get connected() {
    return this.#socket !== null && this.#socket.readyState === 1;
  }

  #connect() {
    if (this.#closed) return;
    this.onStatus("connecting");

    const socket = this.open(socketUrl(window.location, this.room, this.name));
    this.#socket = socket;

    socket.addEventListener("open", () => {
      this.#attempt = 0;
      this.onStatus("connected");
    });

    socket.addEventListener("message", (event) => {
      let message;
      try {
        message = JSON.parse(event.data);
      } catch {
        return;
      }
      this.#handle(message);
    });

    socket.addEventListener("close", () => {
      this.#socket = null;
      if (this.#closed || this.refused) return;
      this.onStatus("reconnecting");
      // A dropped room is worth rejoining: the code is still valid and the
      // other members are still in it.
      this.#timer = setTimeout(() => this.#connect(), retryDelay(this.#attempt));
      this.#attempt += 1;
    });

    // An error is always followed by a close, which is where retrying happens.
    socket.addEventListener("error", () => {});
  }

  #handle(message) {
    switch (message.type) {
      case "welcome":
        this.you = message.you;
        this.room = message.room;
        this.members = message.members ?? [];
        this.#openLocal();
        this.onMembers(this.members);
        break;
      case "members":
        this.members = message.members ?? [];
        this.#localIds = new Set(
          [...this.#localIds].filter((id) => this.members.some((member) => member.id === id)),
        );
        // A new tab can announce itself just before this membership update
        // arrives. Re-announce after every roster change so filtering that
        // early hello cannot leave the two tabs stuck on the slower relay.
        this.#announceLocal();
        this.onMembers(this.members);
        break;
      case "snapshot":
        // Your own snapshot comes back too; the page already has it.
        if (message.from !== this.you) this.onSnapshot(message.from, message.snapshot);
        break;
      case "frame":
        // Your own frame comes back; you are already looking at it.
        if (message.from !== this.you) this.onFrame(message.from, message.frame);
        break;
      // The link cable. Unlike snapshots and frames these are not filtered
      // by sender: the parent has to hear its own transfer announced so every
      // console starts one from the same message, and a child hearing its own
      // value back is harmless.
      case "link_tick":
        this.onLinkTick(message.frame);
        break;
      case "link_start":
        this.onLinkStart(message.seq, message.frame ?? 0, message.offset ?? 0);
        break;
      case "link_value":
        this.onLinkValue(message.from, message.seq, message.value);
        break;
      case "link_data":
        this.onLinkData(message.seq, message.values, message.cycles);
        break;
      case "locked":
        this.locked = Boolean(message.locked);
        this.onLocked(this.locked);
        break;
      case "error":
        // The server refused the join and is about to close. Say so and stop,
        // rather than reconnecting into the same refusal every few seconds.
        this.refused = true;
        this.onRefused(message.message ?? "The room refused the connection.");
        this.close();
        break;
      default:
        break;
    }
  }

  #send(payload) {
    if (!this.connected) return false;
    this.#socket.send(JSON.stringify(payload));
    return true;
  }

  /** Open the zero-hop transport shared by tabs in this browser profile. */
  #openLocal() {
    this.#local?.close();
    this.#local = this.openLocal(`tinybird-link-${this.room}`);
    this.#localIds = new Set(this.you ? [this.you] : []);
    if (!this.#local) return;
    this.#local.addEventListener("message", (event) => this.#handleLocal(event.data));
    this.#announceLocal();
  }

  #announceLocal() {
    if (this.#local && this.you) {
      this.#local.postMessage({ type: "hello", from: this.you, reply: false });
    }
  }

  #handleLocal(message) {
    if (!message || message.from === this.you) return;
    if (message.type === "hello") {
      this.#localIds.add(message.from);
      if (!message.reply) {
        this.#local?.postMessage({ type: "hello", from: this.you, reply: true });
      }
      return;
    }

    const sender = this.members.find((member) => member.id === message.from);
    if (!sender || sender.seat === undefined || sender.seat === null) return;
    const parent = sender.seat === 0;
    if (message.type === "link_tick" && parent) this.onLinkTick(message.frame);
    else if (message.type === "link_start" && parent) {
      this.onLinkStart(message.seq, message.frame ?? 0, message.offset ?? 0);
    } else if (message.type === "link_value") {
      this.onLinkValue(message.from, message.seq, message.value);
    } else if (message.type === "link_data" && parent) {
      this.onLinkData(message.seq, message.values, message.cycles);
    }
  }

  /** Whether every console on this cable is another tab we can reach directly. */
  #canSendLocal() {
    if (!this.#local || !this.you) return false;
    const seated = this.members.filter(
      (member) => member.seat !== undefined && member.seat !== null,
    );
    return seated.length >= 2 && seated.every((member) => this.#localIds.has(member.id));
  }

  #sendLink(payload) {
    if (this.#canSendLocal()) {
      this.#local.postMessage({ ...payload, from: this.you });
      return true;
    }
    return this.#send(payload);
  }

  get linkTransport() {
    return this.#canSendLocal() ? "local" : "relay";
  }

  /** Tell the room what this member is playing. */
  setPlaying(playing, gameCode) {
    return this.#send({ type: "playing", playing, game_code: gameCode });
  }

  /** Publish a snapshot to the room. */
  publish(snapshot) {
    return this.#send({ type: "snapshot", snapshot });
  }

  /** Publish a picture of this member's screen. */
  publishFrame(frame) {
    return this.#send({ type: "frame", frame });
  }

  /** Say how far the parent has got, in frames. Host only. */
  publishLinkTick(frame) {
    return this.#sendLink({ type: "link_tick", frame });
  }

  /** Announce a link transfer. Host only; the server ignores anyone else. */
  publishLinkStart(seq, frame, offset) {
    return this.#sendLink({ type: "link_start", seq, frame, offset });
  }

  /** Offer this console's halfword for the transfer in progress. */
  publishLinkValue(seq, value) {
    return this.#sendLink({ type: "link_value", seq, value });
  }

  /** Publish what every console sent, in seat order. Host only. */
  publishLinkData(seq, values, cycles) {
    return this.#sendLink({ type: "link_data", seq, values, cycles });
  }

  /** Close the room to new arrivals, or open it again. Host only. */
  setLocked(locked) {
    return this.#send({ type: "lock", locked });
  }

  /** Leave for good. No reconnect follows. */
  close() {
    this.#closed = true;
    clearTimeout(this.#timer);
    this.#socket?.close();
    this.#socket = null;
    this.#local?.close();
    this.#local = null;
    this.#localIds.clear();
    this.onStatus("closed");
  }
}

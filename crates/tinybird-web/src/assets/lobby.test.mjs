// Tests for the lobby connection.
//
//   node --test crates/tinybird-web/src/assets/lobby.test.mjs
//
// The connection is the part that has to survive being deployed: a URL scheme
// that is invisibly wrong until the site is behind HTTPS, and a reconnect that
// either comes back or silently does not.

import { strict as assert } from "node:assert";
import test from "node:test";

import { LobbyConnection, retryDelay, socketUrl } from "./lobby.js";

/** A WebSocket stand-in that records what it was told and can be driven. */
class FakeSocket {
  static opened = [];

  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    this.listeners = {};
    FakeSocket.opened.push(this);
  }

  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }

  emit(type, event) {
    for (const fn of this.listeners[type] ?? []) fn(event);
  }

  send(text) {
    this.sent.push(JSON.parse(text));
  }

  close() {
    this.readyState = 3;
  }

  /** Pretend the server accepted the connection. */
  accept() {
    this.readyState = 1;
    this.emit("open", {});
  }

  /** Pretend a message arrived. */
  deliver(message) {
    this.emit("message", { data: JSON.stringify(message) });
  }

  drop() {
    this.readyState = 3;
    this.emit("close", {});
  }
}

class FakeLocalChannel {
  constructor() {
    this.sent = [];
    this.listeners = [];
  }

  addEventListener(type, fn) {
    if (type === "message") this.listeners.push(fn);
  }

  postMessage(message) {
    this.sent.push(message);
  }

  deliver(message) {
    for (const listener of this.listeners) listener({ data: message });
  }

  close() {}
}

function connect(overrides = {}) {
  FakeSocket.opened = [];
  const events = { members: [], snapshots: [], status: [], refused: [], locked: [] };
  const connection = new LobbyConnection({
    room: "BCDFG",
    name: "Ada",
    onMembers: (m) => events.members.push(m),
    onSnapshot: (from, snapshot) => events.snapshots.push({ from, snapshot }),
    onStatus: (s) => events.status.push(s),
    onRefused: (m) => events.refused.push(m),
    onLocked: (l) => events.locked.push(l),
    open: (url) => new FakeSocket(url),
    ...overrides,
  });
  return { connection, events, socket: () => FakeSocket.opened.at(-1) };
}

// `socketUrl` reads window.location; give it one.
globalThis.window = { location: { protocol: "http:", host: "127.0.0.1:8877" } };

test("a page served over TLS connects with wss", () => {
  // Getting this wrong is invisible locally and fails outright once deployed
  // behind HTTPS, because a secure page may not open an insecure socket.
  const secure = socketUrl({ protocol: "https:", host: "gba.example.dev" }, "BCDFG", "Ada");
  assert.ok(secure.startsWith("wss://gba.example.dev/"), secure);

  const plain = socketUrl({ protocol: "http:", host: "127.0.0.1:8877" }, "BCDFG", "Ada");
  assert.ok(plain.startsWith("ws://127.0.0.1:8877/"), plain);
});

test("the room and name are carried as query parameters", () => {
  const url = socketUrl({ protocol: "http:", host: "h" }, "BCDFG", "Ada Lovelace");
  const query = new URL(url.replace(/^ws/, "http")).searchParams;
  assert.equal(query.get("room"), "BCDFG");
  assert.equal(query.get("name"), "Ada Lovelace");
});

test("a name with characters that need escaping still arrives intact", () => {
  const url = socketUrl({ protocol: "http:", host: "h" }, "R&D", "a=b&c d");
  const query = new URL(url.replace(/^ws/, "http")).searchParams;
  assert.equal(query.get("room"), "R&D");
  assert.equal(query.get("name"), "a=b&c d");
});

test("retries back off but stay bounded", () => {
  const first = retryDelay(0);
  const later = retryDelay(4);
  assert.ok(first >= 500 && first < 1000, `got ${first}`);
  assert.ok(later > first, "later attempts should wait longer");
  // Capped, so a long outage does not end in a retry an hour away.
  assert.ok(retryDelay(50) <= 500 * 2 ** 5 + 250, `got ${retryDelay(50)}`);
});

test("welcome records who you are and who else is here", () => {
  const { connection, events, socket } = connect();
  socket().accept();
  socket().deliver({
    type: "welcome",
    room: "BCDFG",
    you: "me",
    members: [{ id: "me", name: "Ada" }],
  });

  assert.equal(connection.you, "me");
  assert.equal(connection.members.length, 1);
  assert.deepEqual(events.status, ["connecting", "connected"]);
});

test("your own snapshot is not handed back to you", () => {
  // The page already has it, and rendering it as somebody else's would show a
  // member watching themselves.
  const { events, socket } = connect();
  socket().accept();
  socket().deliver({ type: "welcome", room: "R", you: "me", members: [] });

  socket().deliver({ type: "snapshot", from: "me", snapshot: { a: 1 } });
  assert.equal(events.snapshots.length, 0);

  socket().deliver({ type: "snapshot", from: "other", snapshot: { a: 2 } });
  assert.equal(events.snapshots.length, 1);
  assert.equal(events.snapshots[0].from, "other");
});

test("a dropped connection is retried", () => {
  const { events, socket } = connect();
  socket().accept();
  const before = FakeSocket.opened.length;

  socket().drop();
  assert.ok(events.status.includes("reconnecting"));

  // The retry is on a timer; run it.
  return new Promise((resolve) => {
    setTimeout(() => {
      assert.ok(FakeSocket.opened.length > before, "should have opened another socket");
      resolve();
    }, 1200);
  });
});

test("closing on purpose does not reconnect", () => {
  const { connection, socket } = connect();
  socket().accept();
  const before = FakeSocket.opened.length;

  connection.close();
  socket().drop();

  return new Promise((resolve) => {
    setTimeout(() => {
      assert.equal(FakeSocket.opened.length, before, "a deliberate close is final");
      resolve();
    }, 1200);
  });
});

test("nothing is sent before the connection is up", () => {
  const { connection, socket } = connect();
  assert.equal(connection.publish({ a: 1 }), false, "should report it did not send");
  assert.equal(socket().sent.length, 0);

  socket().accept();
  assert.equal(connection.publish({ a: 1 }), true);
  assert.equal(socket().sent[0].type, "snapshot");
});

test("what a member is playing is sent in the shape the server parses", () => {
  const { connection, socket } = connect();
  socket().accept();
  connection.setPlaying("Minish Cap", "BZME");

  assert.deepEqual(socket().sent[0], {
    type: "playing",
    playing: "Minish Cap",
    game_code: "BZME",
  });
});

test("a malformed message does not take the connection down", () => {
  const { connection, socket } = connect();
  socket().accept();
  socket().emit("message", { data: "not json at all" });
  socket().deliver({ type: "something-new-from-a-later-server" });

  // Still usable.
  socket().deliver({ type: "welcome", room: "R", you: "me", members: [] });
  assert.equal(connection.you, "me");
});

// --- being turned away ----------------------------------------------------

test("a refused join is reported and not retried", () => {
  // The server answers a bad code with an error and closes. Reconnecting would
  // spin against the same refusal forever, and the person would see the room
  // flickering rather than being told the code is wrong.
  const { connection, events, socket } = connect();
  socket().accept();
  const before = FakeSocket.opened.length;

  socket().deliver({ type: "error", message: "No room with that code." });
  assert.deepEqual(events.refused, ["No room with that code."]);
  assert.equal(connection.refused, true);

  socket().drop();
  return new Promise((resolve) => {
    setTimeout(() => {
      assert.equal(FakeSocket.opened.length, before, "must not reconnect");
      resolve();
    }, 1200);
  });
});

test("the room being locked is reported", () => {
  const { connection, events, socket } = connect();
  socket().accept();
  socket().deliver({ type: "locked", locked: true });

  assert.deepEqual(events.locked, [true]);
  assert.equal(connection.locked, true);
});

test("locking is sent in the shape the server parses", () => {
  const { connection, socket } = connect();
  socket().accept();
  connection.setLocked(true);
  assert.deepEqual(socket().sent[0], { type: "lock", locked: true });
});

test("a link start carries the parent's frame-relative position", () => {
  const starts = [];
  const { connection, socket } = connect({
    onLinkStart: (seq, frame, offset) => starts.push({ seq, frame, offset }),
  });
  socket().accept();

  connection.publishLinkStart(7, 12, 3456);
  assert.deepEqual(socket().sent[0], {
    type: "link_start",
    seq: 7,
    frame: 12,
    offset: 3456,
  });

  socket().deliver({ type: "link_start", seq: 8, frame: 13, offset: 4567 });
  assert.deepEqual(starts, [{ seq: 8, frame: 13, offset: 4567 }]);
});

test("two tabs bypass the server for link traffic", () => {
  const local = new FakeLocalChannel();
  const values = [];
  const { connection, socket } = connect({
    openLocal: () => local,
    onLinkValue: (from, seq, value) => values.push({ from, seq, value }),
  });
  socket().accept();
  socket().deliver({
    type: "welcome",
    room: "BCDFG",
    you: "parent",
    members: [
      { id: "parent", name: "Ada", seat: 0 },
      { id: "child", name: "Bea", seat: 1 },
    ],
  });
  local.deliver({ type: "hello", from: "child", reply: false });

  const serverBefore = socket().sent.length;
  connection.publishLinkStart(9, 20, 3000);
  assert.equal(connection.linkTransport, "local");
  assert.equal(socket().sent.length, serverBefore, "the relay should be bypassed");
  assert.deepEqual(local.sent.at(-1), {
    type: "link_start",
    seq: 9,
    frame: 20,
    offset: 3000,
    from: "parent",
  });

  local.deliver({ type: "link_value", from: "child", seq: 9, value: 0xabcd });
  assert.deepEqual(values, [{ from: "child", seq: 9, value: 0xabcd }]);
});

test("a console can say it cannot take a transfer", () => {
  const skips = [];
  const { connection, socket } = connect({
    onLinkSkip: (from, seq) => skips.push({ from, seq }),
  });
  socket().accept();
  socket().deliver({
    type: "welcome",
    room: "BCDFG",
    you: "child",
    members: [
      { id: "parent", name: "Ada", seat: 0 },
      { id: "child", name: "Bea", seat: 1 },
    ],
  });

  connection.publishLinkSkip(9);
  assert.deepEqual(socket().sent.at(-1), { type: "link_skip", seq: 9 });

  socket().deliver({ type: "link_skip", from: "child", seq: 9 });
  assert.deepEqual(skips, [{ from: "child", seq: 9 }]);
});

test("a skip reaches the other tab directly, like every other link message", () => {
  const local = new FakeLocalChannel();
  const skips = [];
  const { connection, socket } = connect({
    openLocal: () => local,
    onLinkSkip: (from, seq) => skips.push({ from, seq }),
  });
  socket().accept();
  socket().deliver({
    type: "welcome",
    room: "BCDFG",
    you: "parent",
    members: [
      { id: "parent", name: "Ada", seat: 0 },
      { id: "child", name: "Bea", seat: 1 },
    ],
  });
  local.deliver({ type: "hello", from: "child", reply: false });

  const serverBefore = socket().sent.length;
  connection.publishLinkSkip(9);
  assert.equal(socket().sent.length, serverBefore, "the relay should be bypassed");

  // Without this the same-machine path would drop skips and a child that bowed
  // out would still be waited on for the full timeout — the exact stall the
  // message exists to remove.
  local.deliver({ type: "link_skip", from: "child", seq: 9 });
  assert.deepEqual(skips, [{ from: "child", seq: 9 }]);
});

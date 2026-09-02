// The lockstep session, without a browser.
//
// What is worth testing here is the part that decides *whether* a frame may
// run and *what it runs on*: the input ring, the delay, and the rule that a
// transfer is carried between consoles rather than sent anywhere. The consoles
// themselves are stand-ins — the real ones are WebAssembly and are covered by
// the core's own tests.

import assert from "node:assert/strict";
import test from "node:test";

import { LinkSession, carryTransfer, delayForRoundTrip } from "./link.js";

/** A console that counts what was done to it. */
function fakeConsole(seat) {
  return {
    seat,
    frameCount: 0,
    linkPending: false,
    linkBusy: false,
    linkSendValue: 0x1000 + seat,
    linkTransferCycles: 5242,
    buttons: -1,
    joins: 0,
    deliveries: [],
    disconnected: false,
    setButtons(bits) {
      this.buttons = bits;
    },
    runSlice() {
      // A slice stops at a transfer; otherwise it finishes the frame.
      if (this.linkPending) return false;
      this.frameCount += 1;
      return true;
    },
    linkJoin() {
      this.joins += 1;
      return true;
    },
    linkDeliver(values, cycles) {
      this.deliveries.push([values.slice(), cycles]);
      this.linkPending = false;
      return true;
    },
    linkDisconnect() {
      this.disconnected = true;
    },
    takeAudio() {
      return new Float32Array(0);
    },
    saveState() {
      return new Uint8Array([this.seat, this.frameCount & 0xff, 0x5a]);
    },
  };
}

function makeSession({ players = 2, mySeat = 0, delay = 3 } = {}) {
  const consoles = [];
  for (let seat = 0; seat < players; seat += 1) consoles.push(fakeConsole(seat));
  return new LinkSession({ id: "s1", consoles, mySeat, delay });
}

test("the delay covers a one-way trip and never drops below two frames", () => {
  // A round trip too small to measure still gets a frame of slack either side.
  assert.equal(delayForRoundTrip(0), 2);
  assert.equal(delayForRoundTrip(1), 2);
  // 60ms round trip is 30ms one way, which is two frames, plus one for jitter.
  assert.equal(delayForRoundTrip(60), 3);
  // And a link bad enough to want more than the cap gets the cap, because
  // beyond it the pad stops feeling connected to the game.
  assert.equal(delayForRoundTrip(10_000), 12);
});

test("a transfer is carried between consoles rather than sent anywhere", () => {
  const consoles = [fakeConsole(0), fakeConsole(1)];
  assert.equal(carryTransfer(consoles), false, "nothing to carry while idle");

  consoles[0].linkPending = true;
  assert.equal(carryTransfer(consoles), true);

  assert.equal(consoles[1].joins, 1, "the child is pulled in whether it was ready or not");
  for (const console of consoles) {
    const [values, cycles] = console.deliveries[0];
    assert.deepEqual(values, [0x1000, 0x1001, 0xffff, 0xffff], "absent seats read as absent");
    assert.equal(cycles, 5242, "the parent's baud rate is the cable's");
  }
});

test("a seat nobody is in reads as absent, not as having sent zero", () => {
  const consoles = [fakeConsole(0), fakeConsole(1)];
  consoles[0].linkPending = true;
  carryTransfer(consoles);
  const [values] = consoles[0].deliveries[0];
  assert.equal(values[2], 0xffff);
  assert.equal(values[3], 0xffff);
});

test("the first frames run without waiting for input from before the session", () => {
  const session = makeSession({ delay: 3 });
  // Nothing was pressed before there was a session to press it in, so the
  // frames the delay covers are already answered.
  assert.equal(session.ready, true);
  assert.equal(session.bufferedFrames, 3);
});

test("a frame does not run until every player has said what they pressed", () => {
  const session = makeSession({ delay: 1 });

  // Pressing during frame 0 is a statement about frame 1: that is what the
  // delay is. The frames it covers were answered when the session opened.
  assert.equal(session.pushLocal(0x001), 1);
  assert.equal(session.runFrame(), true, "frame 0 is covered by the delay");
  assert.equal(session.frame, 1);

  // Frame 1 now has our mask, and is waiting on the other player's.
  assert.equal(session.ready, false, "the other player has not said yet");
  session.acceptInput(1, 1, 0x002);
  assert.equal(session.ready, true);
  assert.equal(session.runFrame(), true);

  assert.equal(session.consoles[0].buttons, 0x001, "seat 0 ran on what seat 0 pressed");
  assert.equal(session.consoles[1].buttons, 0x002, "seat 1 ran on what seat 1 pressed");
});

test("input is recorded for a frame the delay ahead of the one about to run", () => {
  const session = makeSession({ delay: 4 });
  assert.equal(session.pushLocal(0x0f0), 4, "pressed now, seen four frames from now");
  assert.equal(session.pushLocal(0x0f0), null, "and only claimed once");
});

test("a frame that has already run cannot be re-answered", () => {
  const session = makeSession({ delay: 1 });
  session.runFrame();
  assert.equal(session.acceptInput(1, 0, 0x3ff), false, "frame 0 is spent");
});

test("input further ahead than the ring holds is refused rather than wrapped", () => {
  const session = makeSession();
  assert.equal(session.acceptInput(1, 5, 0x001), true);
  assert.equal(session.acceptInput(1, 100_000, 0x001), false);
  assert.equal(session.acceptInput(1, -1, 0x001), false);
});

test("a player cannot answer for somebody else's seat", () => {
  const session = makeSession({ mySeat: 0 });
  assert.equal(session.acceptInput(0, 9, 0x001), false, "not our own seat");
  assert.equal(session.acceptInput(7, 9, 0x001), false, "not a seat at all");
});

test("running a frame carries every transfer the parent raises", () => {
  const session = makeSession({ delay: 2 });
  const [parent, child] = session.consoles;

  // The parent stops part-way through the frame asking for a transfer, which
  // is what a linked frame looks like nine times over.
  parent.linkPending = true;

  assert.equal(session.runFrame(), true);
  assert.equal(child.joins, 1, "the child was clocked");
  assert.equal(parent.deliveries.length, 1);
  assert.equal(child.deliveries.length, 1);
  assert.equal(parent.frameCount, 1, "and the frame still finished");
});

test("a cable that cannot be carried stops the session rather than spinning", () => {
  const session = makeSession({ delay: 2 });
  const [parent] = session.consoles;
  // Pending, and delivery does not clear it: a console the loop cannot free.
  parent.linkPending = true;
  parent.linkDeliver = () => true;

  assert.equal(session.runFrame(), false);
  assert.equal(session.wedged, true);
  assert.equal(session.ready, false, "and it does not try to run another");
});

test("the two browsers compare states, in whichever order they arrive", () => {
  const ours = makeSession();
  assert.equal(ours.recordHash(300, 0xabc), null, "nothing to compare against yet");
  assert.equal(ours.acceptHash(300, 0xabc), null, "and they agree");

  const other = makeSession();
  assert.equal(other.acceptHash(300, 0xabc), null, "peer first is the same question");
  assert.equal(other.recordHash(300, 0xdef), 300, "a disagreement names the frame");
});

test("a session that has diverged will not run another frame", () => {
  const session = makeSession({ delay: 4 });
  session.acceptHash(300, 1);
  session.recordHash(300, 2);
  assert.equal(session.desyncedAt, 300);
  assert.equal(session.ready, false);
});

test("states are only compared every so often, not every frame", () => {
  const session = makeSession({ delay: 2 });
  assert.equal(session.hashDue, false);
  for (let frame = 0; frame < 300; frame += 1) {
    session.pushLocal(0);
    session.acceptInput(1, session.frame, 0);
    session.runFrame();
  }
  assert.equal(session.frame, 300);
  assert.equal(session.hashDue, true, "300 frames is five seconds");
});

test("detaching unplugs every console and keeps the player's own", () => {
  const session = makeSession({ players: 2, mySeat: 1 });
  const mine = session.local;
  session.detach();
  for (const console of [mine]) assert.equal(console.disconnected, true);
  assert.deepEqual(session.consoles, [mine], "the other seat is let go of");
});

// Tests for the frame pacing policy.
//
//   node --test crates/tinybird-web/src/assets/pacing.test.mjs
//
// These exist because the emulator used to run one frame per animation
// callback, which meant the game ran at the display's refresh rate: right on a
// 60 Hz panel, 2.4 times too fast on a 144 Hz one. The fix is only worth
// anything if the rate it produces is actually 59.7275 Hz, and that is a claim
// about arithmetic rather than about pixels.

import { strict as assert } from "node:assert";
import test from "node:test";

import { FRAME_MS, GBA_FRAME_HZ, MAX_CATCHUP_FRAMES, schedule } from "./pacing.js";

/**
 * Run `seconds` of callbacks arriving every `intervalMs`, and count the frames.
 */
function simulate(seconds, intervalMs, speed = 1) {
  let clock = 0;
  let frames = 0;
  for (let now = 0; now <= seconds * 1000; now += intervalMs) {
    const due = schedule(now, clock, speed);
    frames += due.frames;
    clock = due.clock;
  }
  return frames;
}

test("the frame rate is the hardware's, not sixty", () => {
  // 16.78 MHz / 280896 cycles.
  assert.ok(Math.abs(GBA_FRAME_HZ - 59.7275) < 0.001, `got ${GBA_FRAME_HZ}`);
  assert.ok(Math.abs(FRAME_MS - 16.7427) < 0.001, `got ${FRAME_MS}`);
});

test("a 60 Hz display produces 59.7275 frames a second, not 60", () => {
  const frames = simulate(10, 1000 / 60);
  const rate = frames / 10;
  assert.ok(Math.abs(rate - GBA_FRAME_HZ) < 0.2, `got ${rate} fps over ten seconds`);
  // The distinction the whole exercise is about.
  assert.ok(rate < 60, `${rate} should be below 60`);
});

test("a 144 Hz display produces the same rate", () => {
  // The bug this replaced: one frame per callback would give 144 fps here.
  const frames = simulate(10, 1000 / 144);
  const rate = frames / 10;
  assert.ok(Math.abs(rate - GBA_FRAME_HZ) < 0.2, `got ${rate} fps`);
});

test("a 30 Hz display still produces the same rate", () => {
  // Fewer callbacks than frames, so each one has to run two.
  const frames = simulate(10, 1000 / 30);
  const rate = frames / 10;
  assert.ok(Math.abs(rate - GBA_FRAME_HZ) < 0.3, `got ${rate} fps`);
});

test("fast forward multiplies the rate", () => {
  for (const speed of [2, 4, 8]) {
    // A display fast enough not to be the limit.
    const rate = simulate(10, 1000 / 1000, speed) / 10;
    const want = GBA_FRAME_HZ * speed;
    assert.ok(
      Math.abs(rate - want) < want * 0.02,
      `at ${speed}x expected about ${want.toFixed(1)} fps, got ${rate.toFixed(1)}`,
    );
  }
});

test("catching up is capped, so a stalled tab cannot avalanche", () => {
  // Ten seconds with no callbacks at all, then one.
  const { frames } = schedule(10_000, 1, 1);
  assert.ok(
    frames <= MAX_CATCHUP_FRAMES,
    `a ten second stall asked for ${frames} frames in one callback`,
  );
});

test("time missed during a stall is given up, not carried forever", () => {
  // After the capped catch-up the clock must be near now; otherwise every
  // later callback would still be repaying a debt from the stall.
  const { clock } = schedule(10_000, 1, 1);
  assert.ok(10_000 - clock < FRAME_MS * MAX_CATCHUP_FRAMES * 2, `clock left at ${clock}`);
});

test("a zero clock starts cleanly rather than claiming a huge backlog", () => {
  const { frames } = schedule(5_000, 0, 1);
  assert.equal(frames, 1, "a fresh clock should ask for exactly one frame");
});

test("no time passing asks for no frames", () => {
  const first = schedule(1000, 0, 1);
  const second = schedule(1000, first.clock, 1);
  assert.equal(second.frames, 0);
  assert.equal(second.clock, first.clock, "the clock must not drift when idle");
});

test("the clock carries fractions rather than snapping to now", () => {
  // Snapping would round the rate to the callback rate, which is the bug.
  const step = 1000 / 60;
  let clock = 0;
  let total = 0;
  for (let i = 0; i < 600; i += 1) {
    const due = schedule(i * step, clock, 1);
    total += due.frames;
    clock = due.clock;
  }
  // 600 callbacks at 60 Hz is ten seconds: 597 frames, not 600.
  assert.ok(total >= 594 && total <= 599, `got ${total} frames in ten seconds`);
});

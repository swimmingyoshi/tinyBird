// How fast the emulator is allowed to run.
//
// Kept free of any DOM or emulator reference so the policy can be checked
// without a browser; `pacing.test.mjs` does exactly that. The numbers here are
// the difference between a game that runs at the right speed and one that runs
// at whatever the monitor happens to do.

/**
 * The Game Boy Advance frame rate: 16.78 MHz over 280,896 cycles a frame.
 *
 * 59.7275 Hz, not 60. Running one emulated frame per `requestAnimationFrame`
 * looks right on a 60 Hz panel and is 2.4 times too fast on a 144 Hz one, so
 * frames are paced against the wall clock instead of against the display.
 */
export const GBA_FRAME_HZ = 16_777_216 / 280_896;

/** Milliseconds per emulated frame at normal speed. */
export const FRAME_MS = 1000 / GBA_FRAME_HZ;

/**
 * How far behind the clock the emulator will try to catch up, in frames.
 *
 * A backgrounded tab stops receiving callbacks. Without a cap, the first one
 * after it returns would try to run every frame it missed — which takes longer
 * than a frame, leaving it further behind still.
 */
export const MAX_CATCHUP_FRAMES = 4;

/**
 * Decide how many emulated frames are due.
 *
 * `clock` is when the next frame falls due; pass `0` to start or restart. The
 * returned clock is carried into the next call, and it advances by exact frame
 * steps rather than being set to `now` — that fractional carry is what keeps
 * the average at 59.7275 Hz instead of rounding to the callback rate.
 */
export function schedule(now, clock, speed) {
  const step = FRAME_MS / speed;
  let next = clock === 0 ? now - step : clock;

  const limit = MAX_CATCHUP_FRAMES * speed;
  let frames = 0;
  while (now - next >= step && frames < limit) {
    next += step;
    frames += 1;
  }

  // Still behind after running everything it was allowed to? The machine
  // cannot keep up, and carrying the debt forward only makes the next callback
  // worse. Give up on the missed time rather than accumulate it forever.
  if (now - next > step * limit) next = now;

  return { frames, clock: next };
}

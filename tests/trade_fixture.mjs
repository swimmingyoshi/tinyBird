/** FireRed trade-counter fixtures; inputs only, never writes to game memory. */
export function tradeButtons(frame, seat, scenario = "direct") {
  const a = frame % 40 < 6 ? 1 : 0;
  if (frame < 820) return a;
  if (frame < 1000) return 64;
  if (frame < 1060) return seat === 0 ? 32 : 16;
  if (frame < 1160) return 64;
  if (frame < 1200) return a;
  // From the upper corners to the two chairs at (4, 5) and (7, 5).
  if (frame < 1248) return seat === 0 ? 16 : frame < 1216 ? 32 : 0;
  if (frame < 1280) return 128;
  if (frame === 1280) return seat === 0 ? 16 : 32;
  if (frame < 1800) return 0;
  if (scenario === "summary") {
    // Browse Summary before trading; Player 2 acts independently, 137 frames later.
    const f = frame - (seat === 1 ? 137 : 0);
    if (f < 1800) return 0;
    for (const [start, key] of [[1800, 1], [1840, 1], [2000, 16], [2040, 16],
      [2200, 2], [2400, 1], [2440, 128], [2480, 1]]) {
      if (f >= start && f < start + 6) return key;
    }
    return f >= 2600 && f % 40 < 6 ? 1 : 0;
  }
  if (frame < 1806) return 1; // Select the first Pokemon.
  if (frame < 1840) return 0;
  if (frame < 1846) return 128; // Summary -> Trade.
  if (frame < 1880) return 0;
  if (frame < 1886) return 1;
  if (frame < 2000) return 0;
  return a; // Confirm the exchange and advance the trade animation.
}

/** IDs and encrypted-data checksums, read without changing the game. */
export function readParty(core) {
  const read32 = at => (core.debugRead(at) | (core.debugRead(at + 2) << 16)) >>> 0;
  const count = core.debugRead(0x02024028) >>> 8;
  if (count > 6) throw new Error(`Invalid party count: ${count}`);
  return Array.from({ length: count }, (_, slot) => {
    const at = 0x02024284 + slot * 100;
    const personality = read32(at);
    const trainer = read32(at + 4);
    const key = personality ^ trainer;
    let sum = 0;
    for (let offset = 32; offset < 80; offset += 4) {
      const word = read32(at + offset) ^ key;
      sum = (sum + (word & 0xffff) + (word >>> 16)) & 0xffff;
    }
    return { id: `${personality}:${trainer}`, valid: sum === core.debugRead(at + 28) };
  });
}

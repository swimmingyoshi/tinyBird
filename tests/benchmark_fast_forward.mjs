import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { TinyBird } from '../crates/tinybird-web/src/assets/tinybird.js';

const root = new URL('../', import.meta.url);
const [wasm, rom, state] = await Promise.all([
  readFile(new URL('target/wasm32-unknown-unknown/release/tinybird_wasm.wasm', root)),
  readFile(new URL('roms/pokemon_fire_red.gba', root)),
  readFile(new URL('TradeTest1.state', root)),
]);
async function create() {
  const { instance } = await WebAssembly.instantiate(wasm, {});
  const emu = new TinyBird(instance.exports);
  emu.loadRom(rom);
  emu.loadState(state);
  return emu;
}
const baseline = await create();
const batched = await create();
const chunks = [];
for (let i = 0; i < 8; i++) { baseline.runFrame(); chunks.push(baseline.takeAudio()); }
batched.runAudioFrames(8);
const expected = new Float32Array(chunks.reduce((n, a) => n + a.length, 0));
let offset = 0;
for (const a of chunks) { expected.set(a, offset); offset += a.length; }
assert.deepEqual(batched.takeAudio(), expected, 'Every sample must survive batching');
assert.deepEqual(batched.frameView(), baseline.frameView(), 'Final image must match');
assert.deepEqual(batched.saveState(), baseline.saveState(), 'Emulated state must match');

const frames = Number(process.env.TINYBIRD_BENCH_FRAMES ?? 600);
const results = { individual: [], batch4: [], batch8: [] };
for (let round = 0; round < 5; round++) {
  const modes = round % 2 ? Object.keys(results).reverse() : Object.keys(results);
  for (const mode of modes) {
    const emu = mode === 'individual' ? baseline : batched;
    emu.loadState(state);
    emu.runFrames(60);
    const count = mode === 'individual' ? 1 : mode === 'batch4' ? 4 : 8;
    const start = performance.now();
    for (let i = 0; i < frames; i += count) {
      if (count === 1) emu.runFrame();
      else emu.runAudioFrames(Math.min(count, frames - i));
      emu.takeAudio();
    }
    results[mode].push(frames * 1000 / (performance.now() - start));
  }
}
for (const [mode, values] of Object.entries(results)) {
  const median = [...values].sort((a,b) => a-b)[2];
  console.log(`${mode}: median ${median.toFixed(1)} emulated FPS; runs ${values.map(n => n.toFixed(1)).join(', ')}`);
}
console.log('PASS: identical audio, framebuffer and save state after batching.');

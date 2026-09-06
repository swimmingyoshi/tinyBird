// Real WASM regression: requires the user's local ROM and trade-counter states.
// node tests/wasm_link_trade.mjs
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { TinyBird } from '../crates/tinybird-web/src/assets/tinybird.js';
import { LinkSession } from '../crates/tinybird-web/src/assets/link.js';
import { tradeButtons, readParty } from './trade_fixture.mjs';

const root = new URL('../', import.meta.url);
const wasmPath = process.env.TINYBIRD_TEST_WASM ?? new URL('target/wasm32-unknown-unknown/release/tinybird_wasm.wasm', root);
const romPath = process.env.TINYBIRD_TEST_ROM ?? new URL('roms/pokemon_fire_red.gba', root);
const statePaths = [process.env.TINYBIRD_TEST_STATE_1 ?? new URL('TradeTest1.state', root),
  process.env.TINYBIRD_TEST_STATE_2 ?? new URL('TradeTest2.state', root)];
const output = resolve(process.env.TINYBIRD_TEST_OUTPUT ?? 'target/trade-test');
await mkdir(output, { recursive: true });
const module = await WebAssembly.compile(await readFile(wasmPath));
const rom = await readFile(romPath);
const cores = [];
for (const file of statePaths) {
  const core = new TinyBird((await WebAssembly.instantiate(module, {})).exports);
  core.loadRom(rom);
  core.loadState(await readFile(file));
  core.linkConnect(cores.length, 2);
  cores.push(core);
}
const before = cores.map(readParty);
assert.ok(before.every(party => party.length === 6 && party.every(mon => mon.valid)), 'invalid starting parties');
assert.notEqual(before[0][0].id, before[1][0].id, 'fixtures must have different first Pokemon');
const expected = before.map((party, seat) => [before[seat ^ 1][0].id, ...party.slice(1).map(mon => mon.id)]);
const session = new LinkSession({ id: 'full-trade', consoles: cores, mySeat: 0, delay: 3 });
const scenario = process.env.TINYBIRD_TEST_SCENARIO ?? "direct";
assert.ok(["direct", "summary"].includes(scenario), "unknown trade scenario");
let exchangedAt = null;
const start = performance.now();
for (let frame = 0; frame < 8000; frame++) {
  session.pushLocal(tradeButtons(frame, 0, scenario));
  assert.equal(session.acceptInput(1, frame + session.delay, tradeButtons(frame, 1, scenario)), true);
  assert.equal(session.runFrame(), true, `cable wedged at frame ${frame}`);
  if (frame % 100 === 0) {
    const now = cores.map(readParty);
    if (now.every((party, seat) => party[0]?.id === expected[seat][0])) exchangedAt ??= frame;
  }
  if (frame % 600 === 599) console.log(JSON.stringify({ frame: frame + 1, transfers: session.transfers, exchangedAt }));
  // Leave time for the animation and automatic save to finish.
  if (exchangedAt !== null && frame >= exchangedAt + 1200) break;
}
// Release both pads and let menu fades settle; repeated confirmation presses
// can otherwise leave the final capture halfway into another Summary screen.
const transfersBeforeSettling = session.transfers;
for (let frame = 0; frame < 240; frame++) {
  session.pushLocal(0);
  assert.equal(session.acceptInput(1, session.frame + session.delay, 0), true);
  assert.equal(session.runFrame(), true, 'cable wedged while settling after trade');
}
assert.ok(session.transfers > transfersBeforeSettling, 'cable stopped after the exchange');
const after = cores.map(readParty);
for (let seat = 0; seat < 2; seat++) {
  await writeFile(resolve(output, `player-${seat + 1}.state`), cores[seat].saveState());
  await writeFile(resolve(output, `player-${seat + 1}.sav`), cores[seat].batterySave());
  const rgba = cores[seat].frameView();
  const rgb = Buffer.alloc(240 * 160 * 3);
  for (let i = 0; i < 240 * 160; i++) rgb.set(rgba.subarray(i * 4, i * 4 + 3), i * 3);
  await writeFile(resolve(output, `player-${seat + 1}.ppm`), Buffer.concat([Buffer.from('P6\n240 160\n255\n'), rgb]));
  assert.deepEqual(after[seat].map(mon => mon.id), expected[seat], `Player ${seat + 1} did not receive the expected Pokemon`);
  assert.ok(after[seat].every(mon => mon.valid), `Player ${seat + 1} has corrupt party data`);
}
assert.notEqual(exchangedAt, null, 'no trade completed');
session.detach();
const bootFrames = [];
for (let seat = 0; seat < 2; seat++) {
  const core = cores[seat];
  core.loadRom(rom);
  core.loadSave(await readFile(resolve(output, `player-${seat + 1}.sav`)));
  let restored = false;
  for (let frame = 0; frame < 1800; frame++) {
    core.setButtons(frame % 60 < 6 ? 9 : 0); // Start at the title, A at Continue.
    core.runFrame();
    if (frame % 100 !== 99) continue;
    const party = readParty(core);
    if (party.length !== 6 || party[0].id !== expected[seat][0]) continue;
    assert.deepEqual(party, after[seat], `Player ${seat + 1}'s battery save lost the exchange`);
    bootFrames.push(frame + 1);
    restored = true;
    break;
  }
  assert.ok(restored, `Player ${seat + 1}'s traded party did not survive a reboot`);
}
const report = { result: 'TRADED', scenario, bootFrames, exchangedAt, frames: session.frame,
  transfers: session.transfers, seconds: (performance.now() - start) / 1000, before, after, output };
await writeFile(resolve(output, 'result.json'), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report));

// Real WASM + IndexedDB recovery, with a synthetic ROM and isolated browser.
import { chromium } from '../target/workshop-ui-check/node_modules/playwright/index.mjs';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';
const assets = 'crates/tinybird-web/src/assets/';
const rom = Buffer.alloc(192);
[0xe3a00402, 0xe3a01063, 0xe1c010b4, 0xeafffffe].forEach((instruction, i) => rom.writeUInt32LE(instruction, i * 4));
rom.write('RecoveryTest', 0xa0); rom.write('TBST', 0xac); rom[0xbc] = 2;
const browser = await chromium.launch({ headless: true, ...(process.platform === 'win32' ? { channel: 'msedge' } : {}) });
await mkdir('target/browser-recovery', { recursive: true });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  let user = null;
  await page.addInitScript(() => {
    // Accelerate the real periodic trigger, leaving emulation clocks intact.
    const interval = window.setInterval;
    window.setInterval = (fn, delay, ...args) => interval(fn, delay === 30000 ? 1000 : delay, ...args);
  });
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/tinybird.wasm') return route.fulfill({ contentType: 'application/wasm', body: await readFile('target/wasm32-unknown-unknown/release/tinybird_wasm.wasm') });
    if (path === '/bios') return route.fulfill({ status: 404 });
    if (path === '/api/auth/me') return route.fulfill({ json: { configured: true, user } });
    if (path.startsWith('/api/')) return route.fulfill({ json: { configured: false, assets: [], roms: [], installed: [], saves: [], shots: [] } });
    if (path === '/console.css') return route.fulfill({ contentType: 'text/css', body: (await Promise.all(['console.css', 'site.css', 'playroom.css'].map(name => readFile(assets + name, 'utf8')))).join('\n') });
    if (path === '/play' || /^\/[\w.-]+\.(js|css)$/.test(path)) return route.fulfill({ contentType: path === '/play' ? 'text/html' : path.endsWith('.js') ? 'text/javascript' : 'text/css', body: await readFile(assets + (path === '/play' ? 'play.html' : path.slice(1))) });
    return route.abort();
  });
  const boot = async () => { await page.goto('http://127.0.0.1:8879/play'); await page.waitForFunction(() => window.tinybird?.emu); };
  const record = () => page.evaluate(async () => (await import('/recovery.js')).readRecovery('guest'));
  await boot();
  await page.evaluate(() => {
    localStorage.setItem('tinybird:placement', JSON.stringify({ party: 'right' }));
    localStorage.setItem('tinybird:observations:guest:TBST:2', JSON.stringify({ schema_version: 1, game: { code: 'TBST', revision: 2 }, fields: [{ name: 'player.hp', address: 0x02000004, width: 2 }] }));
  });
  await boot();
  await page.locator('#file-rom').setInputFiles({ name: 'recovery-test.gba', mimeType: 'application/octet-stream', buffer: rom });
  await page.waitForFunction(() => window.tinybird.emu.readMemory(0x02000004, 2)[0] === 99);
  await page.waitForFunction(async () => !!await (await import('/recovery.js')).readRecovery('guest'));
  await page.locator('#btn-play').click();
  await page.waitForTimeout(250);
  assert.equal((await record()).context.placement.party, 'right');
  await page.evaluate(() => {
    localStorage.setItem('tinybird:placement', '{}');
  });
  // Reload triggers a final capture; mutate configuration after it, on the empty page.
  await boot();
  await page.evaluate(() => localStorage.removeItem('tinybird:observations:guest:TBST:2'));
  await page.waitForFunction(() => !document.querySelector('#recovery-card').hidden);
  assert.equal(await page.locator('#recovery-name').textContent(), 'recovery-test.gba');
  await page.screenshot({ path: 'target/browser-recovery/continue-desktop.png' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: 'target/browser-recovery/continue-mobile.png' });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await page.locator('#continue-playing').click();
  await page.waitForFunction(() => tinybird.emu.hasRom);
  assert.equal(await page.evaluate(() => tinybird.emu.readMemory(0x02000004, 2)[0]), 99);
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('tinybird:placement')).party), 'right');
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('tinybird:observations:guest:TBST:2')).fields[0].name), 'player.hp');
  // Check the state stored after pagehide, which reflects the last configuration.
  await page.locator('#btn-play').click();
  await page.waitForTimeout(200);
  // Failed replacement must preserve the previous checkpoint.
  const savedAt = (await record()).savedAt;
  await page.evaluate(() => {
    window.originalPut = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function(...args) {
      if (this.name === 'checkpoints') { this.transaction.abort(); return; }
      return window.originalPut.apply(this, args);
    };
  });
  await page.locator('#btn-play').click(); await page.locator('#btn-play').click();
  await page.waitForFunction(() => document.querySelector('#recovery-status').textContent.includes('Could not save recovery'));
  assert.equal((await record()).savedAt, savedAt);
  await page.evaluate(() => { IDBObjectStore.prototype.put = window.originalPut; });
  await boot();
  user = { id: 'other-account', email: 'test@example.invalid' };
  await page.evaluate(() => tinybird.refreshAccount());
  await page.waitForTimeout(200);
  assert.equal(await page.locator('#recovery-card').isVisible(), false);
  user = null; await page.evaluate(() => tinybird.refreshAccount());
  await page.waitForFunction(() => !document.querySelector('#recovery-card').hidden);
  assert.deepEqual(await page.evaluate(async () => {
    const { readRecovery, validateRecovery } = await import('/recovery.js');
    const record = await readRecovery('guest');
    const errors = [];
    for (const bad of [{ ...record, version: 999 }, { ...record, game: { ...record.game, revision: 3 } }, { ...record, biosHash: 'different' }, { ...record, romHash: 'different' }]) {
      try { await validateRecovery(bad, null); errors.push('accepted'); } catch { errors.push('rejected'); }
    }
    return errors;
  }), ['rejected', 'rejected', 'rejected', 'rejected']);
  await page.evaluate(async () => {
    const { readRecovery, writeRecovery } = await import('/recovery.js');
    const record = await readRecovery('guest'); record.state[0] ^= 255;
    await writeRecovery('guest', record, record.rom);
  });
  await page.locator('#continue-playing').click();
  await page.waitForFunction(() => document.querySelector('#recovery-status').textContent.includes('damaged'));
  assert.equal(await page.evaluate(() => tinybird.emu.hasRom), false);
  assert.deepEqual(errors, []);
  console.log('PASS: periodic/pause recovery, real WASM resume, layout and observation restore, account isolation, failed-write preservation, compatibility/corruption rejection and mobile fit.');
} finally { await browser.close(); }

// End-to-end Workshop export -> Python -> Rust using synthetic memory and ROM.
// Uses the Playwright installation shared with browser_workshop_ui.mjs.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
const { chromium } = await import(process.env.TINYBIRD_PLAYWRIGHT ?? '../target/workshop-ui-check/node_modules/playwright/index.mjs');
const output = resolve('target/workshop-observations');
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.platform === 'win32' ? { channel: 'msedge' } : {}) });
try {
  const page = await browser.newPage({ viewport: { width: 900, height: 900 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') return route.fulfill({ contentType: 'text/html', body: `<!doctype html><html><head><link rel="stylesheet" href="/console.css"><link rel="stylesheet" href="/addons.css"><link rel="stylesheet" href="/workshop.css"></head><body>
      <main class="addons-page"><details class="workshop" id="workshop" open></details></main><script type="module">
      import { mountWorkshop } from '/workshop.js';
      const ram = new Uint8Array(0x40000); new DataView(ram.buffer).setUint16(4, 99, true);
      window.user = null; window.rom = { game_code: 'TBST', revision: 2, title: 'Synthetic test' };
      window.emu = { hasRom: true, frameCount: 0, snapshot: () => ({rom}), readMemory: (address, length) => ram.slice(address - 0x02000000, address - 0x02000000 + length) };
      window.workshop = mountWorkshop({ root: document.getElementById('workshop'), getEmulator: () => emu,
        getUser: () => user, preview: () => ({status:'active',sections:[]}), installed: () => {}, focusGame: () => {} });
      </script></body></html>` });
    if (path === '/console.css') return route.fulfill({ contentType: 'text/css', body: (await Promise.all(['console.css', 'site.css', 'playroom.css'].map(file => readFile(`crates/tinybird-web/src/assets/${file}`, 'utf8')))).join('\n') });
    if (/^\/[\w.-]+\.(js|css)$/.test(path)) {
      return route.fulfill({ contentType: path.endsWith('.js') ? 'text/javascript' : 'text/css', body: await readFile(`crates/tinybird-web/src/assets${path}`, 'utf8') });
    }
    return route.abort();
  });
  const $ = name => page.locator(`[data-${name}]`);
  await page.goto('http://127.0.0.1:8879/'); await page.waitForFunction(() => window.workshop);
  await $('start-field').click(); await $('field-type').selectOption('u16');
  await $('find-address').click(); await $('value').fill('99'); await $('scan').click();
  assert.equal(await $('results').locator('button').count(), 1);
  await $('results').locator('button').click(); await $('use-observation').click();
  assert.equal(await $('observation-address').inputValue(), '0x02000004');
  await $('observation-name').fill('player.hp'); await $('observation-submit').click();
  await page.waitForFunction(() => document.querySelector('[data-observation-value]')?.textContent === '99');
  const downloadEvent = page.waitForEvent('download'); await $('observation-export').click();
  const download = await downloadEvent; const configPath = resolve(output, download.suggestedFilename());
  await download.saveAs(configPath);
  const config = JSON.parse(await readFile(configPath, 'utf8'));
  assert.deepEqual(config, {schema_version:1,game:{code:'TBST',revision:2},fields:[{name:'player.hp',address:0x02000004,width:2}]});
  await page.screenshot({path:resolve(output,'workshop-export.png')});
  await page.setViewportSize({width:390,height:844});
  await $('observations').scrollIntoViewIfNeeded();
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await page.screenshot({path:resolve(output,'workshop-export-mobile.png')});
  // Check reload persistence and game/account separation.
  await page.reload(); await page.waitForFunction(() => window.workshop);
  await $('observations').locator('summary').click();
  assert.equal(await $('observation-count').textContent(), '1');
  await page.evaluate(() => { window.user = {id:'other-account'}; });
  await page.waitForFunction(() => document.querySelector('[data-observation-count]').textContent === '0');
  await page.evaluate(() => { window.user = null; });
  await page.waitForFunction(() => document.querySelector('[data-observation-count]').textContent === '1');
  await page.evaluate(() => { window.rom = {...rom,revision:3}; });
  await page.waitForFunction(() => document.querySelector('[data-observation-count]').textContent === '0');
  await $('observation-import').setInputFiles(configPath);
  await page.waitForFunction(() => document.querySelector('[data-observation-status]').textContent.includes('requires TBST revision 2'));
  assert.equal(await $('observation-count').textContent(), '0');
  assert.deepEqual(errors, []);
  // ARM: write 99 to EWRAM+4, then loop. No commercial ROM/BIOS fixtures.
  const rom = Buffer.alloc(192);
  [0xe3a00402, 0xe3a01063, 0xe1c010b4, 0xeafffffe].forEach((word, i) => rom.writeUInt32LE(word, i * 4));
  rom.write('TBST', 0xac); rom[0xbc] = 2;
  const romPath = resolve(output, 'synthetic.gba'); await writeFile(romPath, rom);
  const binary = resolve('target/debug', process.platform === 'win32' ? 'tinybird-headless.exe' : 'tinybird-headless');
  const result = spawnSync(process.env.PYTHON ?? 'python', ['examples/python/run_observations.py', romPath, configPath, '--executable', binary, '--steps', '2', '--buttons', 'RIGHT'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  const observations = result.stdout.trim().split('\n').map(line => JSON.parse(line));
  assert.equal(observations[0].memory['player.hp'], 0);
  assert.equal(observations[1].memory['player.hp'], 99);
  assert.equal(observations[2].frame, 2);
  console.log('PASS: memory discovery -> semantic name -> exported file -> Python frame actions; live preview, persistence and compatibility rejection.');
} finally { await browser.close(); }

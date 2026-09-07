// Workshop flow regression in a real Chromium browser, with deterministic RAM.
// npm install --prefix target/workshop-ui-check --no-save --package-lock=false playwright
// Start tinybird-web, then set TINYBIRD_TEST_BASE (default http://127.0.0.1:8879).
// Source assets are served through browser routes so a Rust rebuild is unnecessary.
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const { chromium } = await import(process.env.TINYBIRD_PLAYWRIGHT ?? '../target/workshop-ui-check/node_modules/playwright/index.mjs');
const ROOT = fileURLToPath(new URL('../', import.meta.url));
const BASE = process.env.TINYBIRD_TEST_BASE ?? 'http://127.0.0.1:8879';
const OUTPUT = resolve(ROOT, 'target/workshop-ui');
await mkdir(OUTPUT, { recursive: true });
const browser = await chromium.launch({ headless: true,
  ...(process.env.TINYBIRD_TEST_BROWSER ? { executablePath: process.env.TINYBIRD_TEST_BROWSER } : process.platform === 'win32' ? { channel: 'msedge' } : {}) });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  let publications = 0;
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.pathname === '/workshop-ui-test') return route.fulfill({ contentType: 'text/html', body: `<!doctype html><html><head><link rel="stylesheet" href="/console.css"><link rel="stylesheet" href="/addons.css"><link rel="stylesheet" href="/workshop.css"></head><body>
      <div class="addons-page"><div class="workshop-layout"><div class="workshop-output">Live reader<div id="test-preview"></div></div><div class="workshop-game">Test game</div><details class="workshop" id="addon-workshop" open></details></div></div>
      <script type="module">
      import { mountWorkshop } from '/workshop.js';
      import { setAddonAccount } from '/addon-client.js';
      window.testUser = null; window.installedCount = 0;
      const ram = new Uint8Array(0x40000), view = new DataView(ram.buffer);
      window.setValue = (offset, value) => view.setUint16(offset, value, true);
      setValue(0, 100); setValue(2, 200); setValue(4, 100);
      window.emu = { hasRom: true, frameCount: 1,
        readMemory: (at, length) => at === 0x02000000 ? ram.slice(0, length) : new Uint8Array(length),
        snapshot: () => ({ rom: { title: 'Test game', game_code: 'TEST', revision: 0 } }) };
      const read = spec => spec.u16 ? view.getUint16(Number(spec.u16) - 0x02000000, true) : spec.literal ?? '1';
      const preview = manifest => ({ status: 'active', sections: manifest.sections.map(section => ({ ...section,
        payload: (section.fields ?? section.card?.fields ?? []).map(field => ({ label: field.label, value: String(read(field.read)) + (field.max ? '/' + read(field.max) : '') })) })) });
      window.testWorkshop = mountWorkshop({ root: document.getElementById('addon-workshop'), getEmulator: () => emu,
        getUser: () => testUser, preview, installed: () => installedCount++, focusGame: () => {},
        onPreview: result => { document.getElementById('test-preview').textContent = JSON.stringify(result); } });
      window.changeUser = user => { window.testUser = user; setAddonAccount(user); testWorkshop.accountChanged(); };
      </script></body></html>` });
    if (url.pathname === '/api/community-addons' && route.request().method() === 'POST') {
      publications++;
      const release = route.request().postDataJSON();
      assert.equal(release.description, 'Tested with controlled game values.');
      return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ id: 'test-reader', release: 1 }) });
    }
    const asset = ({ '/addons': 'addons.html', '/play': 'play.html' })[url.pathname] ?? url.pathname.slice(1);
    if (/^[\w.-]+\.(js|css|html)$/.test(asset)) {
      try {
        const assets = resolve(ROOT, 'crates/tinybird-web/src/assets');
        const body = asset === 'console.css' ? (await Promise.all(['console.css', 'site.css', 'playroom.css'].map(file => readFile(resolve(assets, file), 'utf8')))).join('\n') : await readFile(resolve(assets, asset), 'utf8');
        return route.fulfill({ body, contentType: asset.endsWith('.js') ? 'text/javascript' : asset.endsWith('.css') ? 'text/css' : 'text/html' });
      } catch {}
    }
    await route.continue();
  });
  const $ = name => page.locator(`[data-${name}]`);
  const draft = () => page.evaluate(() => JSON.parse(localStorage.getItem('tinybird:local-addons:guest:draft')));
  await page.goto(`${BASE}/workshop-ui-test`);
  await page.waitForFunction(() => window.testWorkshop);
  assert.equal(await $('empty').isVisible(), true);
  await $('name').fill('My reader');
  await $('start-field').click();
  await $('label').fill('HP');
  await $('field-type').selectOption('bar');
  await $('notes').locator('summary').click();
  await $('field-note').fill('Checked in battle');
  await $('find-address').click();
  assert.equal(await $('mode').locator('option[value=changed]').evaluate(option => option.disabled), true);
  await $('value').fill('100'); await $('scan').click();
  assert.equal(await $('results').locator('button').count(), 2);
  await page.evaluate(() => setValue(4, 99));
  await $('mode').selectOption('changed'); await $('scan').click();
  assert.equal(await $('results').locator('button').count(), 1);
  await $('results').locator('button').click();
  assert.equal(await $('label').inputValue(), 'HP');
  assert.equal(await $('field-note').inputValue(), 'Checked in battle');
  assert.equal(await $('address').inputValue(), '0x02000004');
  await $('find-max').click();
  assert.equal(await $('mode').inputValue(), 'equal');
  assert.equal(await $('results').locator('button').count(), 0);
  await $('value').fill('200'); await $('scan').click();
  await $('results').locator('button').click();
  assert.equal(await $('max').inputValue(), '0x02000002');
  assert.equal(await $('field-live').innerText(), '99/200');
  await $('find-address').click();
  assert.equal(await $('mode').inputValue(), 'changed');
  assert.equal(await $('results').locator('button').count(), 1);
  await page.evaluate(() => testWorkshop.reloadDraft());
  assert.equal(await $('results').locator('button').count(), 1);
  await $('close-finder').click();
  await $('add-another').click();
  assert.equal((await draft()).editor.values.address, '');
  assert.equal((await draft()).editor.values.max, '');
  assert.equal(await $('field-note').inputValue(), '');
  await $('label').fill('Money'); await $('address').fill('0x02000002');
  await $('finish').click(); // Includes the finished in-flight field.
  assert.equal(await $('finish-dialog').isVisible(), true);
  assert.match(await $('share-summary').innerText(), /2 fields/);
  assert.equal(await $('publish').isDisabled(), true);
  const downloadPromise = page.waitForEvent('download'); await $('export').click();
  const exported = JSON.parse(await readFile(await (await downloadPromise).path(), 'utf8'));
  assert.equal(exported.sections[0].fields.length, 2);
  assert.equal(exported.display_name, 'My reader');
  await $('save').click();
  assert.equal(await page.evaluate(() => installedCount), 1);
  await $('back-build').click();
  await page.locator('.workshop-field').filter({ hasText: 'HP' }).getByRole('button', { name: 'edit', exact: true }).click();
  await $('label').fill('Health');
  await $('outline').locator('summary').first().click();
  await page.locator('.workshop-field').filter({ hasText: 'Money' }).getByRole('button', { name: 'remove', exact: true }).click();
  assert.match(await $('field-error').innerText(), /save this field/);
  assert.equal(JSON.parse((await draft()).text).sections[0].fields.length, 2);
  await $('add').click();
  assert.equal(JSON.parse((await draft()).text).sections[0].fields[0].label, 'Health');
  await page.locator('.workshop-field').filter({ hasText: 'Money' }).getByRole('button', { name: 'edit', exact: true }).click();
  await $('label').fill('Coins'); await $('address').fill('invalid');
  await page.reload(); await page.waitForFunction(() => window.testWorkshop);
  assert.equal(await $('label').inputValue(), 'Coins');
  assert.equal(await $('address').inputValue(), 'invalid');
  await $('finish').click();
  assert.equal(await $('finish-dialog').isVisible(), false);
  assert.notEqual(await $('field-error').innerText(), '');
  await $('address').fill('0x02000002'); await $('finish').click();
  assert.equal(JSON.parse((await draft()).text).sections[0].fields.length, 2);
  await page.keyboard.press('Escape');
  assert.equal(await $('finish-dialog').isVisible(), false);
  await $('add-section').click(); await $('category-name').fill('Party');
  await $('category-kind').selectOption('cards'); await $('category-save').click();
  const sections = JSON.parse((await draft()).text).sections;
  assert.equal(sections[1].repeat.count, 6);
  assert.equal(sections[1].repeat.stride, 100);
  await $('new-field').click(); await $('label').fill('Unfinished');
  await page.evaluate(() => changeUser({ id: 'another-user' }));
  assert.equal(await $('field-editor').isVisible(), false);
  await page.evaluate(() => changeUser(null));
  assert.equal(await $('label').inputValue(), 'Unfinished');
  page.once('dialog', dialog => dialog.accept()); await $('cancel-edit').click();
  await $('finish').click();
  await page.screenshot({ path: resolve(OUTPUT, 'finish.png') });
  await page.keyboard.press('Escape');
  for (const width of [1440, 980, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, `no page overflow at ${width}px`);
    await page.screenshot({ path: resolve(OUTPUT, `workspace-${width}.png`), fullPage: true });
  }
  // Publish only against the intercepted endpoint, never the community service.
  await page.evaluate(() => {
    localStorage.setItem('tinybird:local-addons:publisher:draft', localStorage.getItem('tinybird:local-addons:guest:draft'));
    changeUser({ id: 'publisher' });
  });
  await $('finish').click(); await $('description').fill('Tested with controlled game values.');
  await $('publish').click();
  await $('share').locator('a').waitFor(); assert.equal(publications, 1);
  assert.deepEqual(errors, []);
  console.log('Workshop browser checks passed: search/narrow, independent maxima, live field, add-next, finish/download/install, edit guards, reload/account recovery, categories, responsive layout, mocked publishing.');
} finally { await browser.close(); }

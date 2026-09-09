import { chromium } from '../target/workshop-ui-check/node_modules/playwright/index.mjs';
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const executable = resolve(process.argv[2] ?? 'target/debug/tinybird-web.exe');
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('TINYBIRD_')));
const child = spawn(executable, ['--mode', 'local', '--port', '18893', '--roms', 'tests/fixtures', '--bios', 'target/no-test-bios.bin'], { env, windowsHide: true, stdio: 'ignore' });
const base = 'http://127.0.0.1:18893';
let browser;
try {
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) throw new Error('Test server exited');
    try { if ((await fetch(base + '/api/health')).ok) break; } catch {}
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({headless:true, ...(process.platform === 'win32' ? {channel:'msedge'} : {})});
  const page = await browser.newPage(); const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(base + '/play');
  await page.waitForFunction(() => window.tinybird?.emu && document.documentElement.dataset.deployment === 'local');
  assert.equal(await page.locator('#btn-vault').isVisible(), false);
  assert.equal(await page.locator('#account').isVisible(), false);
  const rom = Buffer.alloc(192);
  [0xe3a00402, 0xe3a01063, 0xe1c010b4, 0xeafffffe].forEach((instruction, i) => rom.writeUInt32LE(instruction, i*4));
  rom.write('TBST', 0xac); rom[0xbc] = 2;
  await page.locator('#file-rom').setInputFiles({name:'local-test.gba',mimeType:'application/octet-stream',buffer:rom});
  await page.waitForFunction(() => tinybird.emu.readMemory(0x02000004,2)[0] === 99);
  const download = page.waitForEvent('download');
  await page.locator('#canvas').hover(); await page.locator('#quick-screenshot').click();
  assert.match((await download).suggestedFilename(), /\.png$/);
  await page.evaluate(() => document.querySelector('#btn-eject').click());
  await page.locator('#file-bios').setInputFiles({name:'test-bios.bin',mimeType:'application/octet-stream',buffer:Buffer.alloc(16384)});
  await page.waitForFunction(() => localStorage.getItem('tinybird:bios')?.length > 1000);
  await page.goto(base + '/addons#workshop');
  await page.waitForFunction(() => document.documentElement.dataset.deployment === 'local');
  assert.equal(await page.locator('[data-addon-tab="community"]').isVisible(), false);
  await page.waitForFunction(() => document.querySelector('#workshop-player').contentWindow?.tinybird?.emu);
  assert.deepEqual(errors, []);
  console.log('PASS: real local edition, hidden hosted controls, screenshot download, browser BIOS and embedded Workshop under security headers.');
} finally { await browser?.close(); child.kill(); }

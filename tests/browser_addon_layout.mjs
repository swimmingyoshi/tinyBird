// Browser regression for add-on panes and the Vault gallery.
// Uses the same Playwright installation as browser_workshop_ui.mjs.
// Run from the repository root; TINYBIRD_TEST_BASE defaults to localhost:8877.
import { chromium } from '../target/workshop-ui-check/node_modules/playwright/index.mjs';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
const BASE = process.env.TINYBIRD_TEST_BASE ?? 'http://127.0.0.1:8877';
const browser=await chromium.launch({channel:'msedge',headless:true});
try {
 const page=await browser.newPage({viewport:{width:1440,height:1000}}); const errors=[]; page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/*',async route=>{
  const path=new URL(route.request().url()).pathname;
  if(path==='/api/shots') return route.fulfill({json:{configured:true,shots:[1,2,3].map(i=>({url:`https://fixture.invalid/${i}.svg`,game_code:'BPRE',taken_at_ms:i*10000000}))}});
  if(path.startsWith('/api/saves')) return route.fulfill({json:{configured:true,backend:'private',limit:5,saves:[]}});
  if(new URL(route.request().url()).host==='fixture.invalid') return route.fulfill({contentType:'image/svg+xml',body:'<svg xmlns="http://www.w3.org/2000/svg" width="240" height="160"><rect width="240" height="160" fill="teal"/></svg>'});
  if(path==='/console.css') return route.fulfill({contentType:'text/css',body:(await Promise.all(['console.css','site.css','playroom.css'].map(n=>readFile('crates/tinybird-web/src/assets/'+n,'utf8')))).join('\n')});
  if(path==='/play'|| /\.(js|css)$/.test(path)) {
   try {return route.fulfill({contentType:path==='/play'?'text/html':path.endsWith('.js')?'text/javascript':'text/css',body:await readFile('crates/tinybird-web/src/assets/'+(path==='/play'?'play.html':path.slice(1)),'utf8')});}catch{}
  }
  return route.continue();
 });
 await page.goto(BASE+'/play'); await page.waitForFunction(()=>window.tinybird?.emu);
 await page.locator('#file-rom').setInputFiles('roms/pokemon_fire_red.gba');
 await page.waitForFunction(()=>tinybird.emu.running);
 await page.locator('[data-tool=saves]').click(); await page.locator('#file-state').setInputFiles('TradeTest1.state');
 await page.waitForTimeout(200); if(await page.evaluate(()=>tinybird.emu.running))await page.locator('#btn-play').click();
 await page.locator('#left-pane .pane-arrange').first().click();
 await page.locator('#pane-layout-actions button').filter({hasText:'Split with'}).first().click();
 assert.equal(await page.locator('#left-pane').getAttribute('data-split'),'true');
 await page.locator('.rail-divider').first().focus(); await page.keyboard.press('ArrowDown');
 assert.equal(await page.locator('.rail-divider').first().getAttribute('aria-valuenow'),'55');
 await page.locator('#btn-vault').click(); await page.locator('#vault-tab-shots').click();
 await page.waitForFunction(()=>document.querySelector('#shot-position').textContent==='1 of 3');
 await page.locator('#shot-next').click(); assert.equal(await page.locator('#shot-position').textContent(),'2 of 3');
 await page.locator('#vault-tab-saves').focus(); await page.keyboard.press('ArrowRight');
 assert.equal(await page.locator('#vault-tab-shots').getAttribute('aria-selected'),'true');
 assert.equal(await page.locator('#shot-position').textContent(),'2 of 3');
 await page.locator('#shot-next').focus();await page.keyboard.press('ArrowRight');
 assert.equal(await page.locator('#shot-position').textContent(),'3 of 3');
 await page.screenshot({path:'target/browser-play-ui/vault-final.png'});
 await page.setViewportSize({width:390,height:844});
 assert.equal(await page.locator('#vault-modal').evaluate(e=>e.scrollWidth>e.clientWidth),false);
 await page.keyboard.press('Escape');
 await page.reload();await page.waitForFunction(()=>window.tinybird?.emu);
 await page.goto(BASE+'/play?embed=workshop');await page.waitForFunction(()=>window.tinybird?.emu);
 assert.equal(errors.length,0,errors.join('\n'));
 console.log('PASS: current shared-workspace UI boots, split resizing, Vault gallery, tab/gallery keyboard separation, mobile fit, reload and embedded Workshop; no page errors.');
} finally {await browser.close();}

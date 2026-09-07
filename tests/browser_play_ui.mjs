// Playroom UI regression using the local ROM and trade-state fixtures.
// Requires a browser listening on port 9223 and tinybird-web on port 8878.

import assert from "node:assert/strict";
import { writeFile, mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const BASE = process.env.TINYBIRD_TEST_BASE ?? "http://127.0.0.1:8878";
const DEVTOOLS = (process.env.TINYBIRD_TEST_DEVTOOLS ??
  "http://127.0.0.1:9223,http://127.0.0.1:9224").split(",");
const ROOT = new URL("../", import.meta.url);
const { fileURLToPath } = await import("node:url");
const ROM = process.env.TINYBIRD_TEST_ROM ?? fileURLToPath(new URL("roms/pokemon_fire_red.gba", ROOT));
const STATES = [process.env.TINYBIRD_TEST_STATE_1 ?? fileURLToPath(new URL("TradeTest1.state", ROOT)),
  process.env.TINYBIRD_TEST_STATE_2 ?? fileURLToPath(new URL("TradeTest2.state", ROOT))];
const OUTPUT = resolve(process.env.TINYBIRD_TEST_OUTPUT ?? "target/browser-play-ui");
await mkdir(OUTPUT, { recursive: true });

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

class Cdp {
  constructor(url) {
    this.next = 1;
    this.pending = new Map();
    this.socket = new WebSocket(url);
    this.ready = new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    });
  }

  async call(method, params = {}) {
    await this.ready;
    const id = this.next++;
    const answer = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
    this.socket.send(JSON.stringify({ id, method, params }));
    return answer;
  }

  async eval(expression, awaitPromise = true) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      awaitPromise,
      returnByValue: true,
    });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }
}

async function page(devtools) {
  const target = await fetch(`${devtools}/json/new?${encodeURIComponent(`${BASE}/play`)}`, {
    method: "PUT",
  }).then((response) => response.json());
  const client = new Cdp(target.webSocketDebuggerUrl);
  client.target = target.id;
  client.devtools = devtools;
  await client.call("Runtime.enable");
  await client.call("DOM.enable");
  await client.call("Page.enable");
  await client.call("Emulation.setFocusEmulationEnabled", { enabled: true });
  await client.call("Page.setWebLifecycleState", { state: "active" });
  for (let n = 0; n < 100; n++) {
    if (await client.eval("Boolean(window.tinybird?.emu)")) return client;
    await sleep(100);
  }
  throw new Error("emulator did not boot");
}

async function choose(client, selector, file) {
  const { root } = await client.call("DOM.getDocument");
  const { nodeId } = await client.call("DOM.querySelector", { nodeId: root.nodeId, selector });
  await client.call("DOM.setFileInputFiles", { nodeId, files: [file] });
}

async function waitFor(client, expression, message) {
  for (let n = 0; n < 200; n++) {
    if (await client.eval(expression)) return;
    await sleep(100);
  }
  throw new Error(message);
}


const { readdir, stat } = await import("node:fs/promises");
const client = await page(DEVTOOLS[0]);
const click = selector => client.eval(`document.querySelector(${JSON.stringify(selector)}).click(); true`);
const mode = () => client.eval('document.querySelector("#rig").dataset.playView');
async function key(code, modifiers = 0) {
  const key = code === "Space" ? " " : code;
  for (const type of ["keyDown", "keyUp"]) {
    await client.call("Input.dispatchKeyEvent", { type, key, code, modifiers, text: type === "keyDown" && code === "Enter" ? "\r" : undefined,
      windowsVirtualKeyCode: code === "Space" ? 32 : code === "Enter" ? 13 : code === "Tab" ? 9 : code === "Escape" ? 27 : 0 });
  }
}
async function screenshot(name) {
  await sleep(150);
  const shot = await client.call("Page.captureScreenshot", { format: "png" });
  await writeFile(resolve(OUTPUT, name + ".png"), Buffer.from(shot.data, "base64"));
}
await client.call("Emulation.setDeviceMetricsOverride", { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
await client.call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
await click('[data-play-view-button="desk"]');
await screenshot("empty-desktop");
await choose(client, "#file-rom", ROM);
await waitFor(client, "tinybird.emu.running", "ROM did not load");
await click('[data-tool="saves"]');
assert.equal(await client.eval('document.querySelector("#tool-saves").hidden'), false);
await choose(client, "#file-state", STATES[0]);
await sleep(500);
await click("#btn-play");
assert.equal(await client.eval("tinybird.emu.running"), false);
await client.eval('document.querySelector("#btn-play").focus(); true');
await key("Space");
assert.equal(await client.eval("tinybird.emu.running"), true, "Space must activate the focused pause button");
assert.equal(await client.eval("tinybird.buttons"), 0, "UI keyboard activation must not press game buttons");
await key("Enter");
assert.equal(await client.eval("tinybird.emu.running"), false);
await click('[data-tool="settings"]');
assert.equal(await client.eval('document.querySelector("#tool-saves").hidden'), true);
assert.equal(await client.eval('document.querySelector("#tool-settings").hidden'), false);
await screenshot("settings-desktop");
assert.ok(await client.eval('document.querySelector("#tool-settings").getBoundingClientRect().bottom <= innerHeight + 1'), "Opened tools must scroll into view");
await click('[data-tool="settings"]');
await client.eval("document.activeElement.blur(); true");
for (const view of ["focus", "cinema", "desk"]) {
  await key("Tab");
  assert.equal(await mode(), view);
  if (view === "cinema") {
    await sleep(200);
    assert.ok(await client.eval('document.documentElement.scrollHeight <= innerHeight + 1'), 'Cinema must fit vertically');
  }
  assert.ok(await client.eval(`(() => {
    const screen = document.querySelector('.screen').getBoundingClientRect();
    return Math.abs(screen.left + screen.width / 2 - document.documentElement.clientWidth / 2) < 2;
  })()`), `Game must be centered in ${view}`);
  if (view !== "cinema") assert.ok(await client.eval(`(() => {
    const left = document.querySelector('.readout--addons').getBoundingClientRect();
    const right = document.querySelector('.readout--info').getBoundingClientRect();
    const screen = document.querySelector('.screen-col').getBoundingClientRect();
    return left.right <= screen.left && right.left >= screen.right && Math.abs(left.width - right.width) < 2;
  })()`), "Addons must flank the game in equally sized side rails");
  await screenshot(view);
}
await key("Tab", 8);
assert.equal(await mode(), "cinema");
await key("Escape");
assert.equal(await mode(), "desk");
await client.eval('document.querySelector("[data-play-view-button=focus]").focus(); true');
await key("Tab");
assert.equal(await mode(), "desk", "Tab on a control must retain native navigation");
await click('[data-play-view-button="focus"]');
await click("#btn-controls");
assert.equal(await client.eval('document.querySelector("#controls-sheet").open'), true);
await screenshot("controls-dialog");
await key("Escape");
assert.equal(await client.eval('document.querySelector("#controls-sheet").open'), false);
assert.equal(await mode(), "focus", "Closing a dialog must not also leave Focus");
await click("#btn-lobby");
assert.equal(await client.eval('document.querySelector("#lobby-sheet").open'), true);
await screenshot("multiplayer-dialog");
await key("Escape");
await key("Escape");
assert.equal(await mode(), "desk");
await click('[data-tool="saves"]');
const downloads = resolve(OUTPUT, `downloads-${Date.now()}`);
await mkdir(downloads, { recursive: true });
await client.call("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: downloads });
await click("#btn-save");
let downloaded;
for (let attempt = 0; attempt < 100; attempt++) {
  downloaded = (await readdir(downloads)).find(name => name.endsWith(".state"));
  if (downloaded) break;
  await sleep(100);
}
assert.ok(downloaded, "Save to file must still download a state");
assert.ok((await stat(resolve(downloads, downloaded))).size > 1024);
await click('[data-play-view-button="cinema"]');
// Exercise the shared-screen layout without claiming a multiplayer connection.
await client.eval(`document.querySelector('#lobby-workspace').dataset.view='shared';
  document.querySelector('#lobby-watch').hidden=false;
  document.querySelector('#lobby-screen').src=document.querySelector('#canvas').toDataURL(); true`);
await sleep(100);
assert.ok(await client.eval('document.querySelector("#lobby-watch").getBoundingClientRect().width > 200'));
await screenshot("cinema-shared");
for (const width of [320, 390, 768, 1440]) {
  await client.call("Emulation.setDeviceMetricsOverride", { width, height: 844, deviceScaleFactor: 1, mobile: width < 500 });
  for (const view of ["desk", "focus", "cinema"]) {
    await click(`[data-play-view-button="${view}"]`);
    await sleep(100);
    assert.ok(await client.eval("document.documentElement.scrollWidth <= innerWidth + 1"), `Overflow at ${width}px in ${view}`);
  }
}
await client.call("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
await screenshot("mobile-shared");
await client.call("Page.reload");
await waitFor(client, "!!window.tinybird?.emu", "reload failed");
assert.equal(await mode(), "cinema", "View must persist across reload");
await screenshot("empty-mobile");
for (const route of ["/", "/info", "/contact", "/support/tickets"]) {
  for (const width of [320, 390, 768, 1440]) {
    await client.call("Emulation.setDeviceMetricsOverride", { width, height: 1000, deviceScaleFactor: 1, mobile: width < 500 });
    await client.call("Page.navigate", { url: BASE + route });
    await sleep(900);
    assert.ok(await client.eval("document.querySelector('h1')?.textContent.trim().length > 0"), route);
    assert.ok(await client.eval("document.documentElement.scrollWidth <= innerWidth + 1"), `${route} overflow at ${width}`);
    assert.ok(await client.eval("[...document.querySelectorAll('.bar__nav a')].every(a => a.getBoundingClientRect().width > 0)"), `${route} navigation hidden at ${width}`);
    if (width === 390 || width === 1440) await screenshot(`${route.replaceAll('/', '_')}-${width}`);
  }
}
console.log("PASS: playback, saves, dialogs, centered views; Home, Info, Contact and Tickets navigation and 320-1440px layouts");
await fetch(`${client.devtools}/json/close/${client.target}`);
client.socket.close();
process.exit(0);

// Live two-tab link-cable reproduction through Chromium's DevTools protocol.
// Requires a browser listening on port 9223 and tinybird-web on port 8878.

import assert from "node:assert/strict";
import { writeFile, mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { tradeButtons, readParty } from "./trade_fixture.mjs";

const BASE = process.env.TINYBIRD_TEST_BASE ?? "http://127.0.0.1:8878";
const DEVTOOLS = (process.env.TINYBIRD_TEST_DEVTOOLS ??
  "http://127.0.0.1:9223,http://127.0.0.1:9224").split(",");
const ROOT = new URL("../", import.meta.url);
const { fileURLToPath } = await import("node:url");
const ROM = process.env.TINYBIRD_TEST_ROM ?? fileURLToPath(new URL("roms/pokemon_fire_red.gba", ROOT));
const STATES = [process.env.TINYBIRD_TEST_STATE_1 ?? fileURLToPath(new URL("TradeTest1.state", ROOT)),
  process.env.TINYBIRD_TEST_STATE_2 ?? fileURLToPath(new URL("TradeTest2.state", ROOT))];
const SCENARIO = process.env.TINYBIRD_TEST_SCENARIO ?? "direct";
const OUTPUT = resolve(process.env.TINYBIRD_TEST_OUTPUT ?? "target/browser-trade-test");
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

const pages = [await page(DEVTOOLS[0]), await page(DEVTOOLS[1])];
for (let index = 0; index < pages.length; index++) {
  await pages[index].eval(`{
    const toggle = document.querySelector("#opt-link");
    toggle.checked = false; toggle.dispatchEvent(new Event("change"));
  } true`);
  await pages[index].eval(`document.querySelector("#file-rom").addEventListener("change", event => {
    window.__tradeRomBytes = event.target.files[0].arrayBuffer();
  }, { capture: true, once: true }); true`);
  await choose(pages[index], "#file-rom", ROM);
  await waitFor(pages[index], "tinybird.emu.running", `ROM ${index} did not load`);
  await choose(pages[index], "#file-state", STATES[index]);
  await sleep(500);

}

// Optional latency and jitter retain socket order, as a real WebSocket does.
const networkDelay = Number(process.env.TINYBIRD_TEST_INPUT_DELAY_MS ?? 0);
if (networkDelay > 0) {
  for (const client of pages) await client.eval(`{
    const send = WebSocket.prototype.send;
    let lastDelivery = 0;
    let sent = 0;
    WebSocket.prototype.send = function (data) {
      let message;
      try { message = JSON.parse(data); } catch { return send.call(this, data); }
      if (message.type !== "link_input") return send.call(this, data);
      const jitter = (++sent % 30 === 0) ? 60 : 0;
      const delivery = Math.max(lastDelivery, performance.now() + ${networkDelay} + jitter);
      lastDelivery = delivery;
      setTimeout(() => { if (this.readyState === 1) send.call(this, data); }, delivery - performance.now());
    };
  } true`);
}

const before = await Promise.all(pages.map(client => client.eval(`(${readParty.toString()})(tinybird.emu)`)));
const expected = before.map((party, seat) => [before[seat ^ 1][0].id, ...party.slice(1).map(mon => mon.id)]);
for (const client of pages) {
  await client.eval(`import("/link.js").then(({ LinkSession }) => {
    const script = ${tradeButtons.toString()};
    const original = LinkSession.prototype.pushLocal;
    let held = 0;
    const codes = [[1,"KeyZ"],[2,"KeyX"],[16,"ArrowRight"],[32,"ArrowLeft"],[64,"ArrowUp"],[128,"ArrowDown"]];
    LinkSession.prototype.pushLocal = function () {
      const mask = script(this.frame, this.mySeat, ${JSON.stringify(SCENARIO)});
      for (const [bit, code] of codes) {
        if ((held & bit) !== (mask & bit)) window.dispatchEvent(new KeyboardEvent(mask & bit ? "keydown" : "keyup", {code}));
      }
      held = mask;
      return original.call(this, window.tinybird.buttons);
    };
  })`);
}

const room = await pages[0].eval(
  `fetch("/api/lobby", { method: "POST" }).then(async r => {
    const body = await r.json();
    if (!r.ok || !body.room) throw new Error(body.error || "Could not create test room: " + r.status);
    return body.room;
  })`,
);
for (const client of pages) {
  await client.eval(
    `document.querySelector("#lobby-code").value = ${JSON.stringify(room)};
     document.querySelector("#btn-join").click(); true`,
  );
  try {
    await waitFor(client, "tinybird.lobby?.connected", "room did not connect");
  } catch (error) {
    const detail = await client.eval(`({
      room: document.querySelector("#lobby-code").value,
      note: document.querySelector("#lobby-note").textContent,
      lobby: tinybird.lobby && { room: tinybird.lobby.room,
        connected: tinybird.lobby.connected, refused: tinybird.lobby.refused }
    })`);
    throw new Error(`${error.message}: ${JSON.stringify(detail)}`);
  }
}
await sleep(500);
// A slow second device must retain inputs received while its peer core loads.
await pages[1].eval(`import("/tinybird.js").then(({ TinyBird }) => {
  const original = TinyBird.load;
  TinyBird.load = async function (...args) {
    await new Promise(resolve => setTimeout(resolve, 1500));
    return original.apply(this, args);
  };
})`);
for (const client of pages) {
  await client.eval(
    `const link = document.querySelector("#opt-link");
     link.checked = true; link.dispatchEvent(new Event("change")); true`,
  );
  await sleep(1000);
}

await Promise.all(pages.map(client => waitFor(client,
  'tinybird.link.sessionPhase === "live" && tinybird.link.sessionFrame > 10',
  "linked session did not start")));

let exchangedAt = null;
const deadline = Date.now() + 360_000;
let lastReport = 0;
while (Date.now() < deadline) {
  const states = await Promise.all(pages.map(client => client.eval(`({
    link: tinybird.link, party: (${readParty.toString()})(tinybird.emu)
  })`)));
  for (const state of states) assert.equal(state.link.sessionPhase, "live", state.link.sessionNote);
  const frame = Math.min(...states.map(state => state.link.sessionFrame));
  if (states.every((state, seat) => state.party[0]?.id === expected[seat][0])) exchangedAt ??= frame;
  if (frame >= lastReport + 600) {
    console.log(JSON.stringify({ frame, exchangedAt, verified: states.map(state => state.link.lastVerifiedFrame) }));
    lastReport = frame;
  }
  if (exchangedAt !== null && frame >= exchangedAt + 1200) break;
  await sleep(250);
}

const final = await Promise.all(
  pages.map((client) => client.eval(`({ link: tinybird.link,
    transport: tinybird.lobby?.linkTransport,
    note: document.querySelector("#lobby-link-note").textContent,
    party: (${readParty.toString()})(tinybird.emu),
    image: document.querySelector("#canvas").toDataURL("image/png") })`)),
);
for (let index = 0; index < final.length; index++) {
  const encoded = final[index].image.slice(final[index].image.indexOf(",") + 1);
  await writeFile(resolve(OUTPUT, `player-${index + 1}.png`), Buffer.from(encoded, "base64"));
  delete final[index].image;
}
assert.notEqual(exchangedAt, null, "No Pokemon changed hands before the deadline");
for (const [seat, result] of final.entries()) {
  assert.deepEqual(result.party.map(mon => mon.id), expected[seat], `Player ${seat + 1} did not complete the trade`);
  assert.ok(result.party.every(mon => mon.valid), "The trade corrupted a Pokemon");
  assert.ok(result.link.lastVerifiedFrame >= exchangedAt, "No matching state hash after the exchange");
  assert.equal(result.link.sessionPhase, "live", result.link.sessionNote);
  assert.ok(result.link.sessionFrame > 300, "session must pass a state hash check");
}
assert.equal(final[0].link.sessionId, final[1].link.sessionId);
await pages[0].eval(`{ const toggle = document.querySelector("#opt-link");
  toggle.checked = false; toggle.dispatchEvent(new Event("change")); } true`);
await waitFor(pages[1], 'tinybird.link.sessionPhase === "failed"', "peer did not report cable disconnection");
await pages[0].eval(`{ const toggle = document.querySelector("#opt-link");
  toggle.checked = true; toggle.dispatchEvent(new Event("change")); } true`);
await Promise.all(pages.map(client => waitFor(client,
  'tinybird.link.sessionPhase === "live" && tinybird.link.sessionFrame > 10',
  "cable did not reconnect")));
await pages[1].eval('document.querySelector("#btn-leave").click(); true');
await waitFor(pages[1], 'tinybird.link.sessionPhase === "off"', "leaving did not detach");
await waitFor(pages[0], 'tinybird.link.sessionPhase !== "live"', "peer stayed linked after leave");
const persisted = [];
for (let seat = 0; seat < pages.length; seat++) {
  const party = await pages[seat].eval(`(async () => {
    const { TinyBird } = await import("/tinybird.js");
    const saved = tinybird.emu.batterySave();
    const rom = new Uint8Array(await window.__tradeRomBytes);
    const fresh = await TinyBird.load();
    fresh.loadRom(rom);
    fresh.loadSave(saved);
    const readParty = ${readParty.toString()};
    for (let frame = 0; frame < 1800; frame++) {
      fresh.setButtons(frame % 60 < 6 ? 9 : 0);
      fresh.runFrame();
      if (frame % 100 !== 99) continue;
      const party = readParty(fresh);
      if (party.length === 6 && party[0].id === ${JSON.stringify(expected[seat][0])}) return party;
    }
    throw new Error("The traded Pokemon did not survive rebooting from the battery save");
  })()`);
  assert.deepEqual(party.map(mon => mon.id), expected[seat]);
  assert.ok(party.every(mon => mon.valid));
  persisted.push(true);
}
process.stdout.write(`${JSON.stringify({ result: "TRADED", exchangedAt, persisted, final, output: OUTPUT })}\n`);
for (const client of pages) {
  await fetch(`${client.devtools}/json/close/${client.target}`);
  client.socket.close();
}
process.exit(0);

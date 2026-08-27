// Live two-tab link-cable reproduction through Chromium's DevTools protocol.
// Requires a browser listening on port 9223 and tinybird-web on port 8878.

import { writeFile } from "node:fs/promises";

const BASE = process.env.TINYBIRD_TEST_BASE ?? "http://127.0.0.1:8878";
const DEVTOOLS = (process.env.TINYBIRD_TEST_DEVTOOLS ??
  "http://127.0.0.1:9223,http://127.0.0.1:9224").split(",");
const BEATS = Number(process.env.TINYBIRD_TEST_BEATS ?? 90);
const ROM = "B:\\Coding Projects\\Hermetic\\New folder\\tinyBird\\roms\\pokemon_fire_red.gba";
const STATES = [
  "B:\\Coding Projects\\Hermetic\\New folder\\tinyBird\\TradeTest1.state",
  "B:\\Coding Projects\\Hermetic\\New folder\\tinyBird\\TradeTest2.state",
];

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
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
    return result.result.value;
  }
}

async function page(devtools) {
  const target = await fetch(`${devtools}/json/new?${encodeURIComponent(`${BASE}/play`)}`, {
    method: "PUT",
  }).then((response) => response.json());
  const client = new Cdp(target.webSocketDebuggerUrl);
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

async function pulseA(client) {
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyZ" })); true`,
  );
  await sleep(100);
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keyup", { code: "KeyZ" })); true`,
  );
}

async function pulseB(client) {
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyX" })); true`,
  );
  await sleep(100);
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keyup", { code: "KeyX" })); true`,
  );
}

async function pulseDown(client) {
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keydown", { code: "ArrowDown" })); true`,
  );
  await sleep(100);
  await client.eval(
    `window.dispatchEvent(new KeyboardEvent("keyup", { code: "ArrowDown" })); true`,
  );
}

const pages = [await page(DEVTOOLS[0]), await page(DEVTOOLS[1])];
for (let index = 0; index < pages.length; index++) {
  await choose(pages[index], "#file-rom", ROM);
  await waitFor(pages[index], "tinybird.emu.running", `ROM ${index} did not load`);
  await choose(pages[index], "#file-state", STATES[index]);
  await sleep(500);
  if (index === 0) {
    await pulseDown(pages[index]);
    await pulseA(pages[index]);
    await sleep(2500);
    await pulseB(pages[index]);
  }
  await pulseB(pages[index]);
}

const room = await pages[0].eval(
  `fetch("/api/lobby", { method: "POST" }).then(r => r.json()).then(x => x.room)`,
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
for (const client of pages) {
  await client.eval(
    `const link = document.querySelector("#opt-link");
     link.checked = true; link.dispatchEvent(new Event("change")); true`,
  );
}

for (let beat = 0; beat < BEATS; beat++) {
  await Promise.all(pages.map(pulseA));
  await sleep(565);
  if (beat % 10 === 0) {
    const status = await Promise.all(
      pages.map((client) => client.eval(`({ link: tinybird.link,
        transport: tinybird.lobby?.linkTransport,
        note: document.querySelector("#lobby-link-note").textContent })`)),
    );
    process.stdout.write(`${JSON.stringify({ beat, status })}\n`);
  }
}

const final = await Promise.all(
  pages.map((client) => client.eval(`({ link: tinybird.link,
    transport: tinybird.lobby?.linkTransport,
    note: document.querySelector("#lobby-link-note").textContent,
    image: document.querySelector("#canvas").toDataURL("image/png") })`)),
);
for (let index = 0; index < final.length; index++) {
  const encoded = final[index].image.slice(final[index].image.indexOf(",") + 1);
  await writeFile(`link-live-${index + 1}.png`, Buffer.from(encoded, "base64"));
  delete final[index].image;
}
process.stdout.write(`${JSON.stringify({ final })}\n`);
process.exit(0);

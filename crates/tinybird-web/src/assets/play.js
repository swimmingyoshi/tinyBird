// Wiring for the play page: input, the frame loop, the live read-out, and the
// vault. Everything that touches WebAssembly memory lives in tinybird.js.

import { AudioSink, EmulatorError, TinyBird } from "/tinybird.js";
import { mountAccount } from "/account.js";
import { ACTIONS, Controls, keyLabel, padLabel } from "/controls.js";
import {
  THUMBNAIL_PREFIX_BYTES,
  gzip,
  packSave,
  readBattery,
  readThumbnail,
  unpackSave,
} from "/saveformat.js";
import { GBA_FRAME_HZ, MAX_CATCHUP_FRAMES, schedule } from "/pacing.js";
import {
  HASH_INTERVAL,
  delayForRoundTrip,
  openSession,
  packState,
  romFingerprint,
  unpackState,
} from "/link.js";
import {
  FRAME_INTERVAL_MS,
  FRAME_QUALITY,
  LINK_MAX_LEAD,
  LINK_TICK_TIMEOUT_MS,
  LINK_TIMEOUT_MS,
  LobbyConnection,
  SNAPSHOT_INTERVAL_MS,
} from "/lobby.js";

const $ = (id) => document.getElementById(id);

const el = {
  screen: $("screen"),
  canvas: $("canvas"),
  boot: $("boot"),
  linkState: $("link-state"),
  linkLabel: $("link-label"),
  play: $("btn-play"),
  reset: $("btn-reset"),
  eject: $("btn-eject"),
  ff: $("btn-ff"),
  full: $("btn-full"),
  save: $("btn-save"),
  store: $("btn-store"),
  fileState: $("file-state"),
  loadState: $("load-state"),
  fileRom: $("file-rom"),
  optAudio: $("opt-audio"),
  optSpeed: $("opt-speed"),
  optVolume: $("opt-volume"),
  optLcd: $("opt-lcd"),
  optScan: $("opt-scan"),
  panel: $("screen-panel"),
  optOverlay: $("opt-overlay"),
  optOverlayRow: $("opt-overlay-row"),
  overlayStage: $("overlay-stage"),
  cloud: $("btn-cloud"),
  lobbyNote: $("lobby-note"),
  lobbyOut: $("lobby-out"),
  lobbyIn: $("lobby-in"),
  lobbyCode: $("lobby-code"),
  lobbyRoom: $("lobby-room"),
  lobbyMembers: $("lobby-members"),
  lobbyHint: $("lobby-hint"),
  join: $("btn-join"),
  host: $("btn-host"),
  leave: $("btn-leave"),
  lock: $("btn-lock"),
  share: $("opt-share"),
  link: $("opt-link"),
  linkNote: $("lobby-link-note"),
  lobbyWorkspace: $("lobby-workspace"),
  lobbyWatch: $("lobby-watch"),
  lobbyScreen: $("lobby-screen"),
  lobbyCaption: $("lobby-caption"),
  copyCode: $("btn-copy-code"),
  savesPanel: $("saves-panel"),
  savesList: $("saves-list"),
  overlayFrame: $("overlay-frame"),
  cart: $("cart"),
  cartPrefix: $("cart-prefix"),
  cartRegion: $("cart-region"),
  cartRegionName: $("cart-region-name"),
  cartRev: $("cart-rev"),
  cartAddon: $("cart-addon"),
  livePanel: $("live-panel"),
  addonEmpty: $("addon-empty"),
  liveTitle: $("live-title"),
  liveNote: $("live-note"),
  vaultList: $("vault-list"),
  vaultNote: $("vault-note"),
  footPerf: $("foot-perf"),
  footMsg: $("foot-msg"),
  drop: $("drop"),
  rig: $("rig"),
  controlsSheet: $("controls-sheet"),
  controlsBody: $("controls-body"),
  controlsOpen: $("btn-controls"),
  controlsReset: $("btn-controls-reset"),
  padBadge: $("pad-badge"),
  padStatus: $("pad-status"),
  padStatusText: $("pad-status-text"),
  padStatusPad: $("pad-status-pad"),
  deckHint: $("deck-hint"),
  savesOff: $("saves-off"),
  savesPane: $("pane-saves"),
  shotsPane: $("pane-shots"),
  shotsList: $("shots-list"),
  shotsOff: $("shots-off"),
  shotViewer: $("shot-viewer"),
  shotFull: $("shot-full"),
  shotWhen: $("shot-when"),
  shotOpen: $("shot-open"),
  panePark: $("pane-park"),
  leftPane: $("left-pane"),
  rightPane: $("right-pane"),
  lobbySheet: $("lobby-sheet"),
  lobbyOpen: $("btn-lobby"),
  lobbyBadge: $("lobby-badge"),
};

const ctx = el.canvas.getContext("2d", { alpha: false });
// Nearest-neighbour for the blow-up onto the backing store. That step is
// always a whole multiple, so every source pixel covers the same number of
// backing pixels and the edges stay hard.
ctx.imageSmoothingEnabled = false;

/**
 * The frame at the size the game drew it.
 *
 * The display canvas is a whole multiple of that now, and putImageData ignores
 * scaling, so the frame lands here first and is blown up from here. It doubles
 * as what every capture reads: a screenshot, a save thumbnail and a shared
 * frame all want the picture the game drew, not the size the layout stretched
 * it to.
 */
const frameCanvas = document.createElement("canvas");
frameCanvas.width = 240;
frameCanvas.height = 160;
const frameCtx = frameCanvas.getContext("2d", { alpha: false });
/** Reused so each frame is one putImageData rather than an allocation. */
const image = frameCtx.createImageData(240, 160);

/** How far the frame may be blown up before the browser takes over. */
const MAX_BACKING_SCALE = 12;

/** Put the frame on the display canvas, at whatever size it currently is. */
function blit() {
  ctx.drawImage(frameCanvas, 0, 0, el.canvas.width, el.canvas.height);
}

let emu = null;
let audio = null;
let buttons = 0;
let running = false;
let fastForward = false;
let romName = "";
let lastSnapshotAt = 0;
/** Cartridge code of the loaded ROM, e.g. BPRE. Keys the per-game save limit. */
let gameCode = "";
let fpsWindowStart = 0;

// --- status -------------------------------------------------------------

function say(message, tone = "") {
  el.footMsg.textContent = message;
  el.footMsg.dataset.tone = tone;
}

function setLink(state, label) {
  el.linkState.dataset.state = state;
  el.linkLabel.textContent = label;
}

// --- input --------------------------------------------------------------

// Which key or controller button drives which GBA button lives in controls.js,
// so it can be tested without a browser and changed without touching the
// emulator loop. This file only asks it questions.
const controls = new Controls(safeStorage());

/** localStorage, or nothing at all in a private window that refuses it. */
function safeStorage() {
  try {
    // Touching the property is itself what throws in some configurations, so
    // the access has to be inside the try rather than tested for.
    return window.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Actions currently held on the keyboard, kept apart from the pad's. */
const heldKeys = new Set();
/** Actions currently held on a controller, refreshed once per frame. */
let heldPad = new Set();

function onKey(event, down) {
  // Let the browser have the page while the user is typing, and let the
  // rebinding panel have every key while it is listening for one.
  if (capturing) return;
  if (event.target instanceof HTMLInputElement) return;
  if (event.target instanceof HTMLSelectElement) return;

  const action = controls.actionForKey(event.code);
  if (action === null) return;

  event.preventDefault();
  if (down) heldKeys.add(action);
  else heldKeys.delete(action);
  applyInput();
}

/**
 * Push the union of keyboard and controller to the emulator.
 *
 * Held separately and combined here so releasing a key does not cancel a
 * button still held on a pad — which is exactly what happens to someone
 * playing with a controller who bumps the keyboard.
 */
function applyInput() {
  const held = new Set([...heldKeys, ...heldPad]);
  const mask = Controls.maskFor(held);
  if (mask !== buttons) {
    buttons = mask;
    if (emu) emu.setButtons(buttons);
  }
  // The deck button latches and the binding is momentary, so either one alone
  // is enough. Without the latch in this expression, pressing any other key
  // while fast forward was switched on at the deck would switch it back off.
  setFastForward(ffLatched || held.has("FAST_FORWARD"));
}

/** Fast forward switched on at the deck, as opposed to held on an input. */
let ffLatched = false;

// --- focus mode ---------------------------------------------------------
//
// Tab hides the header and the deck and leaves the game and the read-out.
//
// Taking Tab is not free: it is how a keyboard moves between controls, and a
// page that swallows it everywhere is a page that cannot be used without a
// mouse. So it is only taken when the key would otherwise do nothing useful —
// nothing focused, no dialog open — and never when someone has bound it to the
// game themselves. Tabbing through the deck still works.

function setFocusMode(on) {
  el.rig.dataset.focus = on ? "on" : "off";
  remember("focus", on ? "on" : "off");
  // The screen is sized to its container, and the container just changed.
  fitScreen();
  say(on ? "Focus mode. Tab to leave." : "");
}

function focusModeWantsTab(event) {
  if (event.code !== "Tab" || event.altKey || event.ctrlKey || event.metaKey) return false;
  // Someone who bound Tab to the game meant it for the game.
  if (controls.actionForKey("Tab") !== null) return false;
  if (document.querySelector("dialog[open]")) return false;

  // Only when the key is not being used to move between controls: a focused
  // button, field or link is someone navigating, and Tab belongs to them.
  const at = document.activeElement;
  return !at || at === document.body || at === el.canvas;
}

window.addEventListener(
  "keydown",
  (event) => {
    if (!focusModeWantsTab(event)) return;
    event.preventDefault();
    setFocusMode(el.rig.dataset.focus !== "on");
  },
  true,
);

// Escape leaves as well, because it is what everyone tries first.
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && el.rig.dataset.focus === "on") setFocusMode(false);
});

setFocusMode(recall("focus") === "on");

window.addEventListener("keydown", (e) => onKey(e, true));
window.addEventListener("keyup", (e) => onKey(e, false));
// A window that loses focus mid-press would otherwise keep the button held.
window.addEventListener("blur", () => {
  heldKeys.clear();
  heldPad = new Set();
  applyInput();
});

// --- controllers --------------------------------------------------------
//
// The Gamepad API has no events for button presses, only for the pad arriving
// and leaving, so the state has to be polled. It is polled from the frame loop
// rather than on a timer of its own: a controller read that is not in step
// with the frames being run is a controller that feels late.

let padConnected = false;

/** Read the pads and fold them into the held set. Called once per frame. */
function pollPads() {
  if (!padConnected) return;

  const pads = navigator.getGamepads?.() ?? [];
  const next = controls.padActions(pads);

  // Sets are compared rather than replaced blindly so that a pad sitting still
  // — which is most frames — costs one comparison and no emulator call.
  if (next.size !== heldPad.size || [...next].some((action) => !heldPad.has(action))) {
    heldPad = next;
    applyInput();
  }
}

function refreshPadPresence() {
  const pads = navigator.getGamepads?.() ?? [];
  const live = [...pads].filter(Boolean);
  padConnected = live.length > 0;

  el.padBadge.hidden = !padConnected;
  if (el.padStatus) {
    el.padStatus.dataset.pad = padConnected ? "on" : "off";
    el.padStatusPad.textContent = padConnected
      ? `${live.length} controller${live.length > 1 ? "s" : ""} connected: ${live[0].id}`
      : "No controller detected. Plug one in and press a button.";
  }

  if (!padConnected && heldPad.size > 0) {
    // An unplugged controller must not leave a direction held forever.
    heldPad = new Set();
    applyInput();
  }
}

window.addEventListener("gamepadconnected", () => {
  refreshPadPresence();
  say("Controller connected", "good");
});
window.addEventListener("gamepaddisconnected", refreshPadPresence);

// --- rebinding ----------------------------------------------------------

/** The binding waiting for an input, or null. Also mutes the game while set. */
let capturing = null;

function openControls() {
  renderControls();
  refreshPadPresence();
  el.controlsSheet.showModal();
}

/**
 * Draw the binding list.
 *
 * Rebuilt whole on every change rather than patched: it is a few dozen nodes,
 * it is only on screen while the dialog is open, and a list that rebuilds is a
 * list that cannot drift out of step with the bindings it is showing.
 */
function renderControls() {
  const groups = new Map();
  for (const action of ACTIONS) {
    if (!groups.has(action.group)) groups.set(action.group, []);
    groups.get(action.group).push(action);
  }

  el.controlsBody.replaceChildren(
    ...[...groups].map(([name, actions]) => {
      const block = document.createElement("section");
      block.className = "binds";
      block.append(span("binds__label", name));
      for (const action of actions) block.append(renderBinding(action));
      return block;
    }),
  );
}

function renderBinding(action) {
  const row = document.createElement("div");
  row.className = "bind";
  row.append(span("bind__label", action.label));

  const chips = document.createElement("div");
  chips.className = "bind__chips";

  for (const code of controls.keysFor(action.id)) {
    chips.append(bindChip(action, "key", code, keyLabel(code)));
  }
  for (const index of controls.padsFor(action.id)) {
    chips.append(bindChip(action, "pad", index, padLabel(index)));
  }
  if (controls.keysFor(action.id).length + controls.padsFor(action.id).length === 0) {
    chips.append(span("bind__none", "unbound"));
  }

  chips.append(addChip(action));
  row.append(chips);
  return row;
}

/**
 * One bound input. Clicking it listens for a replacement; the × removes it.
 *
 * Replace-on-click rather than add-on-click because that is what someone
 * clicking their own binding means. Adding a second one is what the + is for.
 */
function bindChip(action, kind, value, label) {
  const chip = document.createElement("span");
  chip.className = "chip chip--bind";
  chip.dataset.kind = kind;

  const swap = document.createElement("button");
  swap.type = "button";
  swap.className = "chip__swap";
  swap.textContent = label;
  swap.title = `Rebind ${action.label}`;
  swap.addEventListener("click", () => capture(action, { replace: { kind, value } }));

  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "chip__drop";
  drop.textContent = "\u00d7";
  drop.title = `Remove this binding from ${action.label}`;
  drop.addEventListener("click", () => {
    controls.unbind(action.id, kind, value);
    renderControls();
    paintShortcuts();
  });

  chip.append(swap, drop);
  return chip;
}

function addChip(action) {
  const add = document.createElement("button");
  add.type = "button";
  add.className = "chip chip--add";
  add.textContent = "+";
  add.title = `Add another binding for ${action.label}`;
  add.addEventListener("click", () => capture(action, {}));
  return add;
}

/**
 * Listen for the next key or controller button and bind it.
 *
 * Both are listened for at once, because someone rebinding for a controller
 * should not have to say so first. Escape cancels, which is why Escape is not
 * bindable — a rebinding UI with no way out is a page that needs reloading.
 */
function capture(action, { replace }) {
  capturing = action;
  el.controlsSheet.dataset.capturing = "on";
  el.padStatusText.textContent = `Press an input for ${action.label}, or Escape to cancel.`;

  const finish = (kind, value) => {
    window.removeEventListener("keydown", onCaptureKey, true);
    cancelAnimationFrame(padWatch);
    capturing = null;
    el.controlsSheet.dataset.capturing = "off";
    el.padStatusText.textContent = DEFAULT_CONTROLS_LEAD;

    if (kind !== null) {
      if (replace) controls.unbind(action.id, replace.kind, replace.value);
      controls.bind(action.id, kind, value);
    }
    renderControls();
    paintShortcuts();
  };

  function onCaptureKey(event) {
    event.preventDefault();
    event.stopPropagation();
    finish(event.code === "Escape" ? null : "key", event.code);
  }

  // Capture phase, so this sees the key before the game handler would.
  window.addEventListener("keydown", onCaptureKey, true);

  // The pad has no event to wait on, so watch it until something is pressed.
  let padWatch = requestAnimationFrame(function watch() {
    for (const pad of navigator.getGamepads?.() ?? []) {
      if (!pad) continue;
      const index = pad.buttons?.findIndex((button) => button.pressed);
      if (index !== undefined && index >= 0) {
        finish("pad", index);
        return;
      }
    }
    padWatch = requestAnimationFrame(watch);
  });
}

const DEFAULT_CONTROLS_LEAD =
  "Click a binding to change it, \u00d7 to remove it, + to add another.";

/** Print each key's own shortcut on it, so the deck teaches the keyboard. */
function paintShortcuts() {
  for (const node of document.querySelectorAll("[data-shortcut]")) {
    const keys = controls.keysFor(node.dataset.shortcut);
    node.textContent = keys.length ? keyLabel(keys[0]) : "";
    node.hidden = keys.length === 0;
  }
  paintDeckHint();
}

/**
 * The line under the deck, built from the bindings rather than written out.
 *
 * It used to be fixed markup reading "Z A · X B · Enter start". The moment
 * those became rebindable, fixed markup became a line that confidently tells
 * you the wrong keys.
 */
function paintDeckHint() {
  const parts = [];

  // The four directions almost always share a shape, so name the set rather
  // than printing four near-identical pairs.
  const arrows = ["UP", "DOWN", "LEFT", "RIGHT"]
    .map((id) => controls.keysFor(id)[0])
    .filter(Boolean);
  if (arrows.length === 4) {
    parts.push([arrows.map(keyLabel).join(""), "move"]);
  }

  for (const [id, what] of [
    ["A", "A"],
    ["B", "B"],
  ]) {
    const key = controls.keysFor(id)[0];
    if (key) parts.push([keyLabel(key), what]);
  }

  // The shoulders share a word, because "A L - S R" reads as nonsense until
  // you already know which half is the key and which is the button.
  const shoulders = ["L", "R"].map((id) => controls.keysFor(id)[0]).filter(Boolean);
  if (shoulders.length === 2) {
    parts.push([shoulders.map(keyLabel).join("/"), "shoulders"]);
  }

  for (const [id, what] of [
    ["START", "start"],
    ["SELECT", "select"],
  ]) {
    const key = controls.keysFor(id)[0];
    if (key) parts.push([keyLabel(key), what]);
  }

  el.deckHint.replaceChildren(
    ...parts.flatMap(([key, what], index) => {
      const strong = document.createElement("b");
      strong.textContent = key;
      const tail = document.createTextNode(`\u00a0${what}`);
      return index === 0
        ? [strong, tail]
        : [document.createTextNode(" \u00b7 "), strong, tail];
    }),
  );
}

/**
 * Give the picture the whole bezel, without the uneven pixels.
 *
 * Scaling by a whole number and letterboxing the remainder threw away most of
 * a step: a 690px column ran the game at 2x, a 480px picture with black either
 * side. Filling the bezel from a 240x160 backing store is the other extreme —
 * a fractional scale lands some source pixels two device pixels wide and
 * others three, and that ripple is what made the browser build look worse than
 * the desktop one.
 *
 * So do both. The backing store is a whole multiple of 240x160, large enough
 * to cover the bezel; the CSS size fills the bezel exactly. The blow-up is
 * exact, and the one fractional step left is the browser's own, which it
 * resamples smoothly instead of dropping rows.
 */
function fitScreen() {
  // The content box, not `getBoundingClientRect` — the bezel is border-box
  // with a 1px edge, so its outer size is 2px more than the panel can have,
  // and a panel built to the outer size loses a pixel each side to `overflow:
  // hidden` instead of showing it.
  const boxWidth = el.screen.clientWidth;
  const boxHeight = el.screen.clientHeight;
  if (boxWidth === 0 || boxHeight === 0) return;

  // The largest 240x160-shaped rectangle the bezel holds. Floored, so a
  // rounding crumb never spills past the edge.
  const scale = Math.min(boxWidth / 240, boxHeight / 160);
  const width = Math.floor(240 * scale);
  const height = Math.floor(160 * scale);

  // Sized in device pixels: on a 2x display the backing store has to be twice
  // as big to be covering one source pixel per device pixel.
  const dpr = window.devicePixelRatio || 1;
  const backing = Math.min(
    MAX_BACKING_SCALE,
    Math.max(1, Math.ceil((width * dpr) / 240)),
  );
  if (el.canvas.width !== 240 * backing) {
    el.canvas.width = 240 * backing;
    el.canvas.height = 160 * backing;
    // Resizing a canvas resets its context, the smoothing flag included.
    ctx.imageSmoothingEnabled = false;
  }

  el.canvas.style.width = `${width}px`;
  el.canvas.style.height = `${height}px`;
  el.panel.style.width = `${width}px`;
  el.panel.style.height = `${height}px`;
  // Drives the scanline period, one line per source pixel row. Fractional on
  // purpose: a whole number of CSS pixels would drift a row out of step with
  // the picture by the bottom of the screen.
  el.screen.style.setProperty("--px", `${height / 160}px`);
  el.screen.dataset.scale = String(Math.max(1, Math.round(scale)));
  // Resizing cleared the canvas, and a paused game has no next frame coming
  // to draw it back.
  blit();
}

const resizeObserver = new ResizeObserver(fitScreen);
resizeObserver.observe(el.screen);
window.addEventListener("resize", fitScreen);

/**
 * Give the picture the whole display, or hand it back.
 *
 * The bezel goes fullscreen rather than the canvas, so the scanlines, the
 * boot flash and the empty state come with it and the picture stays centred
 * on black instead of being stretched by the browser.
 */
async function toggleFullscreen() {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await el.screen.requestFullscreen({ navigationUI: "hide" });
  } catch (error) {
    // Refused: an iframe without the permission, or a browser that wants the
    // click closer to the gesture. Nothing is broken, so just say so.
    say(`Full screen was refused: ${error.message}`, "bad");
  }
}

el.full.addEventListener("click", toggleFullscreen);

document.addEventListener("fullscreenchange", () => {
  const on = document.fullscreenElement === el.screen;
  el.full.setAttribute("aria-pressed", String(on));
  el.full.textContent = on ? "Exit full screen" : "Full screen";
  // The element's box changes without the window resizing, and the observer
  // fires before the new size settles in some browsers.
  fitScreen();
});

function setFastForward(on) {
  if (fastForward === on) return;
  fastForward = on;
  el.ff.setAttribute("aria-pressed", String(on));
}

el.ff.addEventListener("click", () => {
  ffLatched = !ffLatched;
  applyInput();
});

// --- frame loop ---------------------------------------------------------
//
// The emulator is paced against the wall clock, not against the display. An
// earlier version ran one emulated frame per `requestAnimationFrame`, which
// ties the game to whatever the monitor happens to do: correct on a 60 Hz
// panel, and 2.4 times too fast on a 144 Hz one, with the audio pitched to
// match.

/**
 * How long an unlimited-speed callback may spend running frames.
 *
 * Longer than a display interval, deliberately. The budget is checked *after*
 * each frame, so anything below the cost of one frame yields exactly one frame
 * per callback — which is not "unlimited", it is the refresh rate wearing a
 * different label. Overrunning the interval is the point: unlimited trades
 * smooth presentation for speed, and yielding between callbacks is what keeps
 * the page answering clicks.
 */
const UNLIMITED_BUDGET_MS = 24;
/** A hard stop, so a slow machine cannot wedge the page inside one callback. */
const UNLIMITED_MAX_FRAMES = 240;

/** When the next emulated frame falls due. Zero means "resynchronise". */
let frameClock = 0;
/** What fast forward multiplies by; zero means as fast as it will go. */
let fastForwardSpeed = 4;
/** Emulated frames run since the last FPS report. */
let framesRun = 0;

function present() {
  const frame = emu.frameView();
  if (!frame) return;
  image.data.set(frame);
  frameCtx.putImageData(image, 0, 0);
  blit();
}

/**
 * Run the frames that are due, and no more.
 *
 * Audio is only handed over at normal speed. The samples carry no timing of
 * their own, so pushing them while running at four times the rate would queue
 * four seconds of audio for every second of play.
 */
function runPaced(now, speed) {
  const due = schedule(now, frameClock, speed);
  frameClock = due.clock;

  const before = emu.frameCount;
  for (let i = 0; i < due.frames; i += 1) {
    emu.runFrame();
    if (speed === 1 && audio && audio.ready) audio.push(emu.takeAudio());
  }
  // A frame can stop part-way through, at a link transfer, so what was asked
  // for and what was finished are not the same number.
  lastFinished = emu.frameCount - before;
  return due.frames;
}

/** How many of the frames just asked for actually finished. */
let lastFinished = 0;
function framesFinished(asked) {
  return Math.min(asked, lastFinished);
}

/** Run frames for a slice of wall time rather than to a schedule. */
function runUnlimited() {
  const until = performance.now() + UNLIMITED_BUDGET_MS;
  let ran = 0;
  do {
    emu.runFrame();
    ran += 1;
  } while (ran < UNLIMITED_MAX_FRAMES && performance.now() < until);
  // The schedule means nothing while unlimited; start it afresh afterwards.
  frameClock = 0;
  return ran;
}

function tick(now) {
  requestAnimationFrame(tick);

  // Room upkeep first, and outside the check below: watching a room-mate must
  // keep working while your own game is paused, and somebody who stopped
  // sharing should stop looking live whether or not you are playing.
  if (liveSince.size > 0) expireStaleSharers();

  if (!emu || !running) {
    // Nothing accrues while paused, so there is nothing to catch up on.
    frameClock = 0;
    return;
  }

  // A lockstep session offers itself as soon as the room and the cartridge
  // allow. Doing it here rather than only on a membership change covers the
  // orders that arrive the other way round — a game loaded after the room was
  // joined, a fingerprint that finished hashing after the roster settled.
  if (LOCKSTEP && sessionPhase === "off" && lockstepWanted()) offerConsole();

  // The parent decides when a transfer happens, and has to be able to do so
  // while frozen waiting for the last one — otherwise the first transfer of a
  // session would be the only one.
  if (!session) driveLink(now);

  // A link transfer waiting on the network freezes emulated time.
  //
  // The cable is the one place where the game's sense of time and the
  // network's disagree, and they disagree by a lot. A transfer between two
  // consoles takes about a third of a scanline; a round trip to another player
  // takes tens of milliseconds, thousands of times longer. Running frames
  // while we wait would show the game a transfer no cable could have produced,
  // and link code answers that by deciding the cable came out.
  //
  // Freezing spends the latency in wall-clock time instead. The game runs
  // slower while linked, but every transfer still costs it exactly what the
  // hardware would, so nothing times out and nothing loses sync. The clock is
  // reset for the same reason it is when paused: debt accrued while waiting
  // would be spent racing ahead the moment the data lands.
  //
  // Everything below this still runs. The room has to keep being serviced
  // while the cable waits, because the room is what the cable is waiting on.
  //
  // A lockstep session has no such wait: the cable is carried inside this
  // page, so a transfer costs a function call and there is nothing to freeze
  // for. What a session waits on instead is the next frame's input, which is
  // once a frame rather than nine times, and `runLockstep` handles it.
  const linkWaiting = !session && (emu.linkPending || heldAtBarrier(now));
  if (linkWaiting) frameClock = 0;

  const unlimited = fastForward && fastForwardSpeed === 0;

  // Ruby, Sapphire and Emerald read a clock on the cartridge, and the core
  // cannot read one itself: it runs on wasm32, where the host is the only
  // thing that knows what time it is. Pushed every frame rather than once, so
  // berry growth and the tides in Shoal Cave follow the wall clock rather than
  // however long the emulator happened to be running.
  //
  // Except in a lockstep session, where the seed was pushed once when the
  // session opened and must never be pushed again. Two browsers each reading
  // their own `Date.now` is precisely the divergence a session exists to
  // prevent: Emerald reads the cartridge clock, and two consoles told
  // different times stop agreeing within a frame. The clock then advances from
  // the cycles the machine actually runs, which is the only sense in which a
  // linked cartridge clock can be right on both machines at once.
  if (!session) emu.setWallClock(Date.now() / 1000);

  // With a cable attached the running is done by the slice pump, so that the
  // other console never waits a whole frame for an answer. This loop then only
  // decides how much the pump is allowed to run — and carries on, because
  // everything below still has to happen while linked. Returning here instead
  // stopped the battery being flushed, which is not a luxury during a trade:
  // trading writes to the cartridge.
  let ran = 0;
  if (session) {
    // Every console in this browser, one frame each, cable and all. The only
    // thing that can stop it is input that has not arrived.
    ran = runLockstep(now);
  } else if (emu.linkConnected) {
    if (!linkWaiting) {
      const due = schedule(now, frameClock, fastForward ? fastForwardSpeed : 1);
      frameClock = due.clock;
      framesOwed = Math.min(framesOwed + due.frames, MAX_CATCHUP_FRAMES);
    }
    pumpSlices();
  } else {
    ran = linkWaiting
      ? 0
      : unlimited
        ? runUnlimited()
        : runPaced(now, fastForward ? fastForwardSpeed : 1);
    framesRun += ran;
  }

  // A transfer that began inside the frame just run goes out now rather than
  // a frame later. The core stops the frame the moment one starts, so this is
  // the first opportunity, and taking it saves another 16ms per transfer.
  if (ran > 0 && !session) driveLink(now);

  // Count what was run and, on the parent, tell the room.
  //
  // Only frames this loop finished: one cut short by a transfer is counted by
  // `resumeAfterTransfer` when it actually completes, and counting it here as
  // well would have the parent claim frames it has not run.
  if (ran > 0 && !session && emu.linkConnected) {
    linkFrame += framesFinished(ran);
    if (isParent() && lobby?.connected) lobby.publishLinkTick(linkFrame);
  }

  // Only draw when something changed; at normal speed most callbacks on a
  // high-refresh display have no new frame to show.
  if (ran > 0) present();

  // Controllers report no events, so they are read here — in step with the
  // frames being run, rather than on a timer of their own.
  pollPads();

  // The addon snapshot is a parse and a registry walk; four times a second is
  // plenty for a read-out and keeps it off the frame budget.
  if (now - lastSnapshotAt > 250) {
    lastSnapshotAt = now;
    const snapshot = emu.snapshot();
    renderSnapshot(snapshot);
    pushToOverlay(snapshot);

    // And to the room, less often: it crosses a network, and a read-out twice
    // a second is plenty for "what is everyone playing".
    if (lobby?.connected && now - lastPublishedAt > SNAPSHOT_INTERVAL_MS) {
      lastPublishedAt = now;
      lobby.publish(snapshot);
    }
  }

  // Sharing runs on its own clock: a picture is worth sending ten times a
  // second where a read-out is worth two.
  shareFrame(now);

  // Cheap: the flag is a bool, and the copy only happens when the game has
  // actually written to the cartridge.
  if (now - lastBatteryCheck > BATTERY_CHECK_MS) {
    lastBatteryCheck = now;
    flushBattery();
  }

  // How fast the cable is actually carrying, alongside the frame rate.
  if (emu.linkConnected) {
    if (now - rateWindowStart >= 1000) {
      linkRate = (linkTally.settled - rateWindowCount) * (1000 / (now - rateWindowStart));
      rateWindowCount = linkTally.settled;
      rateWindowStart = now;
      renderLinkNote();
    }
  }

  if (now - fpsWindowStart >= 1000) {
    // Emulated frames per second, which is the number that means something.
    // Counting callbacks would just report the monitor's refresh rate.
    const fps = (framesRun * 1000) / (now - fpsWindowStart);
    const speed = fps / GBA_FRAME_HZ;
    el.footPerf.textContent = fastForward
      ? `${fps.toFixed(1)} fps · ${speed.toFixed(1)}\u00d7`
      : `${fps.toFixed(1)} fps`;
    framesRun = 0;
    fpsWindowStart = now;
  }
}

// --- live read-out ------------------------------------------------------

const REGION_NAMES = {
  E: "USA",
  P: "Europe",
  J: "Japan",
  D: "Germany",
  F: "France",
  I: "Italy",
  S: "Spain",
  K: "Korea",
  C: "China",
};

/** Show either the addon read-out or the note explaining what one is. */
function showAddon(visible) {
  el.livePanel.hidden = !visible;
  el.addonEmpty.hidden = visible;
}

// Detail on a party member is opt-in. The headline — name, sprite, level, HP,
// status — is always visible, which is what the rail is for; the stat spread
// and move list are one click away. Kept here rather than in the DOM because
// the read-out is rebuilt whenever the snapshot changes, and state held in the
// nodes would be thrown away with them.
const openCards = new Set();

/** What the last render drew, so an unchanged snapshot costs nothing. */
let renderedSignature = "";

/** The snapshot currently on screen, so a toggle can redraw without the core. */
let lastSnapshot = null;

function toggleCard(key) {
  const opening = !openCards.has(key);
  if (opening) openCards.add(key);
  else openCards.delete(key);
  // Redraw from the snapshot we already have rather than waiting up to 250ms
  // for the next one; a click that does nothing for a quarter second reads as
  // a click that did not land.
  renderedSignature = "";
  if (lastSnapshot) renderSnapshot(lastSnapshot);

  // The redraw destroyed the button the click landed on, so focus fell back to
  // the body: the next Tab restarted from the top of the page, and a card
  // opened near the bottom of a scrolled rail could open off screen. Put focus
  // back on the card and bring what just appeared into view.
  const head = el.rig.querySelector(`[data-card-key="${CSS.escape(key)}"]`);
  if (!head) return;
  head.focus({ preventScroll: true });
  (opening ? (head.closest(".card") ?? head) : head).scrollIntoView({
    block: "nearest",
    behavior: "smooth",
  });
}

// --- rails --------------------------------------------------------------
//
// Both columns are the same thing: a strip of tabs over one pane. Which
// sections go in which column is the player's call, not the page's — a party
// of six is taller than the screen it describes, and being able to put the
// party on the left and the battle on the right is the difference between
// reading one at a time and watching both.
//
// Saves is a pane like any other. It starts on the right because that is where
// it is useful, and it moves like everything else.

const SIDES = ["left", "right"];
const SAVES_PANE = "saves";
const SHOTS_PANE = "shots";
/** Most panes one column will stack. Two each is four on screen at once. */
const MAX_SLOTS = 2;

/** Where each pane sits. Anything unlisted is on the left. */
const placement = recallPlacement();
/**
 * What each rail is showing, top first.
 *
 * One entry is the ordinary case; two means the column is split and both are
 * drawn, one above the other. Four panes at once — two columns, two each — is
 * as much as a rail this wide can show without every one of them being a
 * letterbox.
 */
const slots = recallSlots();
/** The count drawn on the Saves tab, which no longer has an element of its own. */
let savesCount = "";
/** The sections last drawn, so a move can redraw without waiting for a frame. */
let lastSections = [];

function recallPlacement() {
  try {
    const raw = JSON.parse(localStorage.getItem("tinybird:placement") ?? "{}");
    // There are only two sides. Anything else was written by a version of this
    // page that had more of them, and is dropped rather than trusted.
    return Object.fromEntries(
      Object.entries(raw).filter(([, side]) => SIDES.includes(side)),
    );
  } catch {
    return {};
  }
}

function recallSlots() {
  const fallback = { left: [], right: [] };
  try {
    const raw = JSON.parse(localStorage.getItem("tinybird:slots") ?? "{}");
    for (const side of SIDES) {
      if (Array.isArray(raw[side])) {
        fallback[side] = raw[side]
          .filter((id) => typeof id === "string")
          .slice(0, MAX_SLOTS);
      }
    }
  } catch {
    // Defaults are a fine answer to unparseable or unavailable storage.
  }
  return fallback;
}

function saveSlots() {
  remember("slots", JSON.stringify(slots));
}

function sideOf(id) {
  // Saves and shots belong to no game, so they default to the right: without
  // them that rail would be empty until an addon claimed a cartridge.
  const defaultsRight = id === SAVES_PANE || id === SHOTS_PANE || id === "enemies";
  return placement[id] ?? (defaultsRight ? "right" : "left");
}

function moveTo(id, side) {
  const from = sideOf(id);
  placement[id] = side;
  remember("placement", JSON.stringify(placement));

  // Take the pane out of the column it left and put it in the one it joined,
  // so "move" lands you looking at the thing you just moved.
  slots[from] = slots[from].filter((slot) => slot !== id);
  if (!slots[side].includes(id)) {
    if (slots[side].length < MAX_SLOTS) slots[side].push(id);
    else slots[side][0] = id;
  }
  saveSlots();
  renderRails(lastSections);
}

/** Put `id` in slot `index` of a rail, replacing whatever was there. */
function selectTab(side, index, id) {
  // Showing the same pane twice in one column wastes half of it, so a pane
  // chosen into one slot is taken out of the other.
  const other = slots[side].findIndex((slot, at) => slot === id && at !== index);
  if (other !== -1) slots[side][other] = slots[side][index];
  slots[side][index] = id;
  saveSlots();
  renderRails(lastSections);
}

function splitRail(side, available) {
  if (slots[side].length >= MAX_SLOTS) return;
  // Open the split on something you are not already looking at, otherwise the
  // new half is a copy of the old one and the button looks broken.
  const next = available.find((item) => !slots[side].includes(item.id));
  if (!next) return;
  slots[side].push(next.id);
  saveSlots();
  renderRails(lastSections);
}

function closeSlot(side, index) {
  slots[side].splice(index, 1);
  saveSlots();
  renderRails(lastSections);
}

/**
 * Everything that can sit in a rail: the addon's sections, then saves.
 *
 * Saves is described the same way a section is, so the rail code never has to
 * know which is which. The only difference is that its body is a node that
 * already exists rather than one built from the snapshot.
 */
function railItems(sections) {
  return [
    ...sections.map((section) => ({
      id: section.section_id,
      title: section.title,
      note: section.note,
      badge: section.badge,
      // What this pane's body is built from, so a rail can tell "the same
      // numbers again" from "something moved" without rebuilding to find out.
      sign: JSON.stringify(section),
      build: () => renderContent(section),
    })),
    { id: SAVES_PANE, title: "Saves", note: savesCount, node: el.savesPane, sign: "saves" },
    { id: SHOTS_PANE, title: "Shots", note: shotsCount, node: el.shotsPane, sign: "shots" },
  ];
}

/**
 * The block each rail slot is showing, and the signatures it was built from.
 *
 * An addon reports four times a second and a battle moves a number on nearly
 * every one of them. Rebuilding both columns whenever any byte changed meant
 * fresh `<img>` elements that decode a frame late, a tab strip that dropped
 * its focus ring mid-click, and a scrolled rail that snapped back to the top —
 * four times a second, for as long as the battle lasted. Keyed by side and
 * position, so only the pane whose own signature moved is rebuilt.
 */
const slotCache = new Map();

function renderRails(sections) {
  const enemyAppeared =
    sections.some((section) => section.section_id === "enemies") &&
    !lastSections.some((section) => section.section_id === "enemies");
  lastSections = sections;
  const items = railItems(sections);

  // Rebuilding a pane briefly shortens the column, and the browser clamps
  // scrollTop to whatever height it finds at that moment. Reading it back
  // afterwards is how the rail stays where it was left.
  const scrolls = [el.leftPane, el.rightPane]
    .map((pane) => pane.closest(".readout"))
    .filter(Boolean)
    .map((box) => ({ box, top: box.scrollTop }));

  for (const side of SIDES) {
    const mine = items.filter((item) => sideOf(item.id) === side);
    const host = side === "left" ? el.leftPane : el.rightPane;

    // Entering formation/battle should immediately reveal the newly useful
    // opponent roster. This happens only on the transition, so the player can
    // still switch this rail back to Saves while the fight continues.
    if (side === "right" && enemyAppeared && mine.some((item) => item.id === "enemies")) {
      slots.right = ["enemies"];
      saveSlots();
    }

    if (mine.length === 0) {
      slots[side] = [];
      host.replaceChildren(
        span("rail-note", "Nothing here. Move a section across to fill it."),
      );
      continue;
    }

    // Drop panes that have gone — a battle that ended, a section moved to the
    // other column — and make sure at least one slot is filled.
    slots[side] = slots[side].filter((id) => mine.some((item) => item.id === id));
    if (slots[side].length === 0) slots[side] = [mine[0].id];
    if (mine.length === 1) slots[side] = [mine[0].id];

    const blocks = slots[side].map((id, index) =>
      renderSlot(side, index, mine, mine.find((item) => item.id === id)),
    );
    // `replaceChildren` removes and reinserts even a child that is already
    // there, which would undo every reused node above. Only touch the host
    // when the list it holds is actually a different list.
    const settled =
      host.childNodes.length === blocks.length &&
      blocks.every((block, index) => host.childNodes[index] === block);
    if (!settled) host.replaceChildren(...blocks);
  }

  // Panes that exist whether or not a game is loaded sit in the park when no
  // rail is showing them, rather than being detached from the document.
  for (const pane of [el.savesPane, el.shotsPane]) {
    const shown = el.leftPane.contains(pane) || el.rightPane.contains(pane);
    if (!shown && pane.parentElement !== el.panePark) el.panePark.append(pane);
    pane.hidden = !shown;
  }

  for (const { box, top } of scrolls) {
    if (box.scrollTop !== top) box.scrollTop = top;
  }
}

/**
 * One slot, reusing as much of the last one as still applies.
 *
 * The chrome — the tab strip and the head — and the body change on different
 * clocks. The chrome moves when the player moves something; the body moves
 * with the game. Signing them separately is what lets a battle redraw its
 * numbers without taking the tab strip down with them.
 */
function renderSlot(side, index, available, item) {
  const key = `${side}:${index}`;
  const cached = slotCache.get(key);

  const chromeSig = JSON.stringify([
    index,
    item.id,
    item.title,
    item.note ?? "",
    slots[side],
    available.map((o) => [o.id, o.title, o.badge?.text ?? "", o.badge?.tone ?? ""]),
    savesCount,
    shotsCount,
  ]);
  // Which of this section's cards are open is state the section's own payload
  // knows nothing about, so it has to be signed too — otherwise a click that
  // only opens a card would find the signature unchanged and draw nothing.
  const openHere = [...openCards]
    .filter((card) => card.startsWith(`${item.id}:`))
    .sort()
    .join(",");
  const bodySig = `${item.sign}|${openHere}`;

  if (cached && cached.chromeSig === chromeSig && cached.bodySig === bodySig) {
    // A pane that is a node rather than a drawing can have been taken by the
    // other column since this block was built.
    if (item.node && !cached.body.contains(item.node)) cached.body.append(item.node);
    return cached.block;
  }

  if (cached && cached.chromeSig === chromeSig) {
    cached.body.replaceChildren(item.node ?? item.build());
    cached.bodySig = bodySig;
    return cached.block;
  }

  const built = buildSlot(side, index, available, item);
  slotCache.set(key, { ...built, chromeSig, bodySig });
  return built.block;
}

/**
 * One pane, and the control that chooses which one it is.
 *
 * The top slot gets the tab strip: it is the rail's main view and picking it
 * should be one click. A split slot gets a dropdown instead.
 *
 * That asymmetry is the point. Drawing the strip twice — which is what this
 * did first — spent four rows of a narrow column on two copies of the same six
 * buttons, and left every tab rendered twice with no way to tell which copy a
 * click would answer. The second pane is an "and also show…", so it is styled
 * like the afterthought it is and costs one row.
 */
function buildSlot(side, index, available, item) {
  const block = document.createElement("div");
  block.className = "slot";

  if (index === 0) {
    const tabs = document.createElement("div");
    tabs.className = "tabs";
    tabs.setAttribute("role", "tablist");
    tabs.append(
      ...available.map((option) => {
        const tab = document.createElement("button");
        tab.type = "button";
        tab.className = "tabs__tab";
        tab.setAttribute("role", "tab");
        tab.setAttribute("aria-selected", String(option.id === item.id));
        // Marked, not hidden: the pane below has it, and swapping the two is a
        // reasonable thing to want.
        if (option.id !== item.id && slots[side].includes(option.id)) {
          tab.dataset.elsewhere = "on";
        }
        tab.append(span("tabs__name", option.title));
        // The addon's flag, which is the whole reason it is on the tab rather
        // than inside the section: it has to be readable from the tab you are
        // not looking at.
        if (option.badge?.text) {
          const flag = span("tabs__flag", option.badge.text);
          if (option.badge.tone) flag.dataset.tone = option.badge.tone;
          tab.append(flag);
        } else if (option.id === SAVES_PANE && savesCount) {
          tab.append(span("tabs__count", savesCount));
        } else if (option.id === SHOTS_PANE && shotsCount) {
          tab.append(span("tabs__count", shotsCount));
        }
        tab.addEventListener("click", () => selectTab(side, index, option.id));
        return tab;
      }),
    );
    block.append(tabs);
  }

  const head = document.createElement("div");
  head.className = "pane__head";

  if (index === 0) {
    head.append(span("pane__note", item.note ?? ""));
  } else {
    // The lower pane picks from a dropdown, which is one row rather than a
    // second copy of the strip above it.
    const pick = document.createElement("select");
    pick.className = "pane__pick";
    pick.setAttribute("aria-label", "Section shown in the lower pane");
    for (const option of available) {
      const choice = document.createElement("option");
      choice.value = option.id;
      choice.textContent = option.badge?.text
        ? `${option.title} - ${option.badge.text}`
        : option.title;
      choice.selected = option.id === item.id;
      pick.append(choice);
    }
    pick.addEventListener("change", () => selectTab(side, index, pick.value));
    head.append(pick);
    head.append(span("pane__note", item.note ?? ""));
  }

  const other = side === "left" ? "right" : "left";
  head.append(
    railButton(
      side === "left" ? "move \u2192" : "\u2190 move",
      `Show ${item.title} in the ${other} column instead`,
      () => moveTo(item.id, other),
    ),
  );

  if (slots[side].length > 1) {
    head.append(
      railButton("close", `Stop showing ${item.title} under the other pane`, () =>
        closeSlot(side, index),
      ),
    );
  } else if (available.length > 1) {
    // Only offered when there is something else in this column to put below.
    head.append(
      railButton("split", "Show a second section under this one", () =>
        splitRail(side, available),
      ),
    );
  }
  block.append(head);

  const body = document.createElement("div");
  body.className = "slot__body";
  // A pane is either built from the snapshot or is a node that already exists.
  // Appending the existing one moves it out of whichever rail last had it.
  body.append(item.node ?? item.build());
  block.append(body);

  return { block, body };
}

function railButton(label, title, onClick) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "pane__move";
  button.textContent = label;
  button.title = title;
  button.addEventListener("click", onClick);
  return button;
}

function renderSnapshot(snapshot) {
  lastSnapshot = snapshot;

  if (!snapshot || !snapshot.rom) {
    el.cart.hidden = true;
    showAddon(false);
    // The right rail carries saves whether or not a game is loaded, so the
    // rails are drawn even when there is no addon to put in them.
    renderRails([]);
    renderedSignature = "";
    return;
  }

  const rom = snapshot.rom;
  const code = rom.game_code || "";
  el.cart.hidden = false;
  // The ROM name is not repeated here: the header bar already carries it, and
  // repeating it was most of what made the old cartridge panel a tall box.
  // Split where the addon registry splits it: three characters identify the
  // game, the fourth is the region.
  el.cartPrefix.textContent = code.slice(0, 3) || "\u00b7\u00b7\u00b7";
  el.cartRegion.textContent = code.slice(3, 4) || "\u00b7";
  el.cartRegionName.textContent = REGION_NAMES[code.slice(3, 4)] ?? "Unknown";
  // The maker code is trivia nobody reads twice, so it rides in the strip's
  // tooltip rather than taking a row of its own.
  el.cart.title = `Maker ${rom.maker_code || "unknown"} \u00b7 ${romName || rom.title || ""}`.trim();
  if (code && code !== gameCode) {
    // A different cartridge means a different set of saves.
    gameCode = code;
    refreshSaves();
  }
  el.cartRev.textContent = String(rom.revision ?? 0);

  const addon = snapshot.addon;
  el.cartAddon.textContent = addon ? addon.display_name : "none";

  if (!addon || !addon.sections || addon.sections.length === 0) {
    showAddon(false);
    renderRails([]);
    renderedSignature = "";
    return;
  }

  // Most frames report exactly what the last one did — standing still in a
  // menu changes nothing an addon can see. Rebuilding the rails anyway would
  // drop text selection and fight the browser for no gain.
  const signature = `${addon.addon_id}|${JSON.stringify(addon.sections)}`;
  if (signature === renderedSignature) return;
  renderedSignature = signature;

  showAddon(true);
  el.liveTitle.textContent = addon.display_name;
  // Which addon this is and what it claims to read. Small, but it is the
  // difference between "the panel is thin" and "the addon reads three things
  // and this game is showing all three".
  el.liveNote.textContent = (addon.capabilities ?? []).join(" \u00b7 ");
  el.liveNote.title = addon.version
    ? `${addon.addon_id} v${addon.version}`
    : addon.addon_id;
  renderRails(addon.sections);
}

// The schema has four section kinds and the desktop dashboard draws the same
// four, so a new addon shows up here with no changes to this file. Everything
// below is driven by the snapshot alone; nothing knows what game it came from.
function renderContent(section) {
  const frag = document.createDocumentFragment();

  if (section.kind === "key_value") {
    for (const field of section.payload ?? []) frag.append(renderField(field));
  } else if (section.kind === "list") {
    for (const item of section.payload ?? []) frag.append(span("stat", item));
  } else if (section.kind === "table") {
    frag.append(renderTable(section.payload));
  } else if (section.kind === "cards") {
    const cards = section.payload ?? [];
    for (const [index, card] of cards.entries()) {
      // A section with one card is about that card — a battle, a boss, the
      // thing on screen — so it gets the room to be looked at rather than
      // scanned. A list of six is a list, and stays a list.
      frag.append(renderCard(card, `${section.section_id}:${index}`, cards.length === 1));
    }
  } else {
    // A section kind newer than this page. Say so rather than drawing nothing,
    // so an addon built against a later schema is visibly newer, not broken.
    frag.append(span("stat stat--muted", `Unsupported section: ${section.kind}`));
  }

  return frag;
}

/**
 * A labelled value, plus whatever the addon attached to it: a bar when the
 * value is bounded, a tone when the addon has a view on how it reads, and a
 * hint for the detail that would not fit in the value.
 */
function renderField(field) {
  const row = document.createElement("div");
  row.className = "stat";
  if (field.tone) row.dataset.tone = field.tone;

  const label = span("stat__label", field.label);
  const value = span("stat__value", field.value);
  row.append(label, value);

  /** A hint too long to sit beside the value, appended after the bar. */
  let block = null;

  // A hint on its own line costs as much vertical space as the value it
  // annotates, and a party member has one on nearly every row — four moves
  // alone went from four lines to twelve. So a short hint rides beside the
  // value, and only one too long to fit takes a line of its own. The cutoff
  // is a width, not a meaning: "30/35 PP" fits next to a move name, a
  // six-stat legend does not.
  if (field.hint) {
    const hint = span("stat__hint", field.hint);
    if (field.hint.length <= HINT_INLINE_MAX) {
      hint.classList.add("stat__hint--inline");
      value.append(hint);
    } else {
      label.title = field.hint;
      label.dataset.hinted = "on";
      block = hint;
    }
  }

  // The bar belongs directly under the value it measures, before any hint
  // that had to take a line of its own.
  if (field.meter) row.append(renderMeter(field.meter, field.tone));
  if (block) row.append(block);

  return row;
}

/**
 * Longest hint that still fits beside a value in a 288px rail.
 *
 * Sized to the widest thing worth putting there: a six-stat spread
 * ("17 6 21 13 5 29"). A stat legend or a sentence goes below instead.
 */
const HINT_INLINE_MAX = 16;

/**
 * The card's picture, when it has one.
 *
 * The frame is always drawn at its full size, whether or not the picture
 * arrives: a rail that reflows every time a sprite loads is worse than one
 * with a gap in it, and the host may have no network to fetch them with. A
 * picture that fails to load simply leaves its frame empty — the card still
 * carries the name, which is the whole reason `alt` is in the schema.
 */
function renderCardImage(image, key) {
  // A rebuilt card used to get a brand new `<img>`, and a fresh element
  // decodes asynchronously even when the bytes are already in the cache — one
  // blank frame per rebuild, which during a battle is a portrait that blinks
  // four times a second. The same slot showing the same picture keeps the
  // element it already decoded.
  const kept = spriteFrames.get(key);
  if (kept && kept.src === image.src) {
    const shown = kept.frame.firstElementChild;
    if (shown) shown.alt = image.alt ?? "";
    return kept.frame;
  }

  const frame = document.createElement("div");
  frame.className = "card__portrait";
  if (image.src.startsWith("/ffta/jobs/")) {
    frame.classList.add("card__portrait--label");
  }

  const img = document.createElement("img");
  img.className = "card__sprite";
  img.src = image.src;
  img.alt = image.alt ?? "";
  img.loading = "lazy";
  img.decoding = "async";
  img.addEventListener("error", () => {
    frame.dataset.failed = "on";
    img.remove();
  });

  frame.append(img);
  spriteFrames.set(key, { src: image.src, frame });
  return frame;
}

/**
 * One frame per card slot, so a redraw does not re-decode a picture it has.
 *
 * Keyed by slot rather than by source: two of the same species in one party
 * are two cards, and a single element cannot be in both of them.
 */
const spriteFrames = new Map();

/** A bar. The percentage comes from the addon's numbers, never from the text. */
function renderMeter(meter, tone) {
  const max = Number(meter.max) || 0;
  const value = Number(meter.value) || 0;
  const percent = max === 0 ? 0 : Math.min(100, Math.round((value / max) * 100));

  const bar = document.createElement("div");
  bar.className = "meter";
  if (tone) bar.dataset.tone = tone;
  bar.setAttribute("role", "img");
  bar.setAttribute("aria-label", `${value} of ${max}`);

  const fill = document.createElement("div");
  fill.className = "meter__fill";
  fill.style.width = `${percent}%`;
  bar.append(fill);

  return bar;
}

/**
 * One member of a repeated set. The heading is always drawn; the detail rows
 * are behind a click, because six party members at ten rows each would bury
 * everything else in the rail.
 */
function renderCard(card, key, featured = false) {
  const open = openCards.has(key);
  const wrap = document.createElement("article");
  wrap.className = "card";
  if (featured) wrap.dataset.featured = "on";
  wrap.dataset.open = open ? "on" : "off";
  if (card.lead?.tone) wrap.dataset.tone = card.lead.tone;

  const hasDetail = (card.fields ?? []).length > 0;
  // A card with nothing to expand does not need the room to be expanded into.
  // An area list runs to twenty species; at the party's spacing that is a pane
  // you scroll for a while, and none of the extra height is showing anything.
  if (!hasDetail && !featured) wrap.dataset.dense = "on";

  const head = document.createElement(hasDetail ? "button" : "div");
  head.className = "card__head";
  if (hasDetail) {
    head.type = "button";
    head.dataset.cardKey = key;
    head.setAttribute("aria-expanded", String(open));
    head.addEventListener("click", () => toggleCard(key));
  }

  if (card.image) head.append(renderCardImage(card.image, key));

  const title = document.createElement("div");
  title.className = "card__id";
  title.append(span("card__title", card.title));
  if (card.subtitle) title.append(span("card__subtitle", card.subtitle));
  head.append(title);

  if (card.lead) {
    const lead = document.createElement("div");
    lead.className = "card__lead";
    lead.append(span("card__lead-value", card.lead.value));
    if (card.lead.meter) lead.append(renderMeter(card.lead.meter, card.lead.tone));
    head.append(lead);
  }

  wrap.append(head);

  if (card.badges?.length) {
    const chips = document.createElement("div");
    chips.className = "chips";
    for (const badge of card.badges) {
      const chip = span("chip", badge.text);
      if (badge.tone) chip.dataset.tone = badge.tone;
      chips.append(chip);
    }
    // On a dense row the chips sit with the name rather than under the whole
    // card, which is what keeps the row one line instead of two.
    if (wrap.dataset.dense === "on") title.append(chips);
    else wrap.append(chips);
  }

  if (hasDetail) {
    const detail = document.createElement("div");
    detail.className = "card__detail";
    for (const field of card.fields) detail.append(renderField(field));
    wrap.append(detail);
  }

  return wrap;
}

function renderTable(payload) {
  const table = document.createElement("table");
  table.className = "rows";

  const head = document.createElement("tr");
  for (const column of payload?.columns ?? []) {
    const th = document.createElement("th");
    th.textContent = column;
    head.append(th);
  }
  table.append(head);

  for (const row of payload?.rows ?? []) {
    const tr = document.createElement("tr");
    for (const cell of row) {
      const td = document.createElement("td");
      td.textContent = cell;
      tr.append(td);
    }
    table.append(tr);
  }

  return table;
}

function span(className, text) {
  const node = document.createElement("div");
  node.className = className;
  node.textContent = text;
  return node;
}

// --- battery saves ------------------------------------------------------
//
// A save state is a photograph of the whole machine and it is ours; a battery
// save is what the cartridge keeps and it is the game's own. When a player
// picks "save" in a menu the game writes the battery, so a page that persists
// only save states loses progress the game believes it already committed —
// which is what a game means when it says a slot is corrupt or missing.
//
// The browser keeps them per cartridge, so returning to a game finds its save
// where it left it. They also ride inside every vault save, so restoring a slot
// restores both halves together.

/** Remember a preference across visits. Failing is not worth interrupting. */
function remember(key, value) {
  try {
    localStorage.setItem(`tinybird:${key}`, String(value));
  } catch {
    // A private window or a full store; the setting just will not persist.
  }
}

function recall(key) {
  try {
    return localStorage.getItem(`tinybird:${key}`);
  } catch {
    return null;
  }
}

/** Put the remembered speed and volume back on the controls. */
function restorePreferences() {
  const speed = recall("speed");
  if (speed !== null && [...el.optSpeed.options].some((o) => o.value === speed)) {
    el.optSpeed.value = speed;
  }
  fastForwardSpeed = Number(el.optSpeed.value);

  // Explicitly against null: `Number(null)` is 0, not NaN, so treating a
  // missing preference as a number silently starts every visitor muted.
  el.share.checked = recall("share") === "1";
  // Off unless asked for: a cable is a thing you agree to, and an unwanted one
  // costs emulation speed for a game that will never use it.
  el.link.checked = recall("link") === "1";
  renderLinkNote();

  const stored = recall("volume");
  const volume = stored === null ? NaN : Number(stored);
  if (Number.isFinite(volume) && volume >= 0 && volume <= 100) {
    el.optVolume.value = String(volume);
  }
}

const BATTERY_KEY_PREFIX = "tinybird:battery:";
/** How often to check whether the cartridge was written. */
const BATTERY_CHECK_MS = 1000;
let lastBatteryCheck = 0;

const batteryKey = (code) => `${BATTERY_KEY_PREFIX}${code}`;

/** Store the cartridge save for `code`, if the browser will keep it. */
function persistBattery(code, bytes) {
  if (!code || !bytes || bytes.byteLength === 0) return;
  try {
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    localStorage.setItem(batteryKey(code), btoa(binary));
  } catch {
    // A full or disabled store is not worth interrupting play for; the save
    // still lives in the emulator and still goes to the vault.
  }
}

/** The stored cartridge save for `code`, or null. */
function storedBattery(code) {
  if (!code) return null;
  try {
    const encoded = localStorage.getItem(batteryKey(code));
    if (!encoded) return null;
    const binary = atob(encoded);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return bytes;
  } catch {
    return null;
  }
}

/** Hand the emulator a cartridge save and remember that we did. */
function applyBattery(bytes, code) {
  if (!bytes || bytes.byteLength === 0) return false;
  try {
    emu.loadSave(bytes);
    // Loading counts as writing, so the flag would otherwise fire immediately.
    emu.takeBatteryDirty();
    if (code) persistBattery(code, bytes);
    return true;
  } catch {
    return false;
  }
}

/** Persist the cartridge save if the game has written to it. */
function flushBattery() {
  if (!emu || !gameCode || !emu.hasBattery) return;
  if (!emu.takeBatteryDirty()) return;
  persistBattery(gameCode, emu.batterySave());
}

// --- loading ------------------------------------------------------------

function setControlsEnabled(enabled) {
  for (const button of [el.play, el.reset, el.eject, el.ff, el.save, el.store, el.cloud]) {
    button.disabled = !enabled;
  }

  // "Load from file" goes with them. `requireCartridge` already refuses the
  // action, but refusing after a file picker, a file chosen and a read is a
  // worse way to learn it than a key that was never live — and the title says
  // why rather than leaving a dead control unexplained.
  el.fileState.disabled = !enabled;
  el.loadState.dataset.disabled = String(!enabled);
  el.loadState.title = enabled
    ? "Open a save state from this computer"
    : "Load a game first — a save state does not carry one";
}

/**
 * Refuse an action that only means something with a cartridge in.
 *
 * A save state is a picture of a machine *around* a cartridge — it does not
 * carry one. Restoring into an empty console produced either an error from the
 * core or, worse, a machine running nothing in particular; either way the
 * message named a serialisation problem rather than the actual mistake.
 */
function requireCartridge(action) {
  if (emu?.hasRom) return true;
  say(`Load a game before you ${action}. A save state does not carry one.`, "bad");
  return false;
}

/**
 * Take the cartridge out.
 *
 * The battery is flushed first: everything the cartridge wrote is still owed
 * to it, and ejecting is exactly the moment that debt comes due.
 */
function ejectRom() {
  if (!emu?.hasRom) return;

  flushBattery();
  emu.eject();

  running = false;
  fastForward = false;
  romName = "";
  gameCode = "";
  lastSnapshotAt = 0;
  el.ff.setAttribute("aria-pressed", "false");
  el.screen.dataset.mode = "empty";
  el.play.textContent = "Resume";
  setControlsEnabled(false);
  // Back to the "no cartridge" rail, and a black screen rather than the last
  // frame of a game that is no longer in the machine.
  renderSnapshot(null);
  frameCtx.clearRect(0, 0, 240, 160);
  blit();
  // The same words the bar carries before anything is loaded, because that is
  // the state the machine is now in.
  setLink("idle", "no cartridge");
  lobby?.setPlaying(null, null);
  say("Cartridge ejected");
}

el.eject.addEventListener("click", ejectRom);

async function startRom(bytes, name) {
  try {
    emu.loadRom(bytes);
  } catch (error) {
    say(error.message, "bad");
    return;
  }

  // Kept, rather than handed over and forgotten. A lockstep session runs the
  // other player's console in this browser too, and a console with no
  // cartridge in it cannot run anything — so when both players are on the same
  // game, which is every battle and most trades, these are the bytes that go
  // into the second core. Sixteen megabytes is a lot to hold; fetching the
  // same file again mid-session, from a vault URL that may have expired, is
  // worse.
  romBytes = new Uint8Array(bytes.slice(0));
  romHash = "";
  romFingerprint(romBytes)
    .then((hash) => {
      romHash = hash;
    })
    .catch(() => {
      // Without a fingerprint a session cannot prove the two cartridges match,
      // so it will refuse to start. That is the right failure.
    });

  romName = name;
  running = true;
  fastForward = false;
  el.ff.setAttribute("aria-pressed", "false");
  el.screen.dataset.mode = "running";
  el.play.textContent = "Pause";
  setControlsEnabled(true);
  setLink("live", name);

  // Power-on wipe, then the first frame.
  el.screen.classList.remove("is-booting");
  void el.screen.offsetWidth; // restart the animation
  el.screen.classList.add("is-booting");

  emu.setColorCorrection(el.optLcd.checked);
  emu.setButtons(buttons);
  // The sample rate is only meaningful once a cartridge is in.
  if (audio) audio.setSampleRate(emu.sampleRate);
  renderSnapshot(emu.snapshot());

  // Put the cartridge's own save back before the game looks for it. The
  // snapshot above is what tells us which cartridge this is.
  const restored = emu.hasBattery && applyBattery(storedBattery(gameCode), gameCode);
  say(
    `Loaded ${name} · ${formatSize(bytes.byteLength)}` +
      (restored ? " · cartridge save restored" : ""),
    "good",
  );

  markVaultSelection(name);
  lobby?.setPlaying(name, gameCode || null);
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/**
 * Fetch a storage URL.
 *
 * The CDN sends `Access-Control-Allow-Origin: *`, so the browser reads it
 * directly and the bytes never touch our server. The proxy is kept as a
 * fallback because a CORS regression upstream would otherwise break loading
 * saves and vault ROMs with nothing but "Failed to fetch" to go on.
 */
async function fetchStorage(url) {
  if (!/^https?:\/\//i.test(url) || url.startsWith(window.location.origin)) {
    return fetch(url);
  }

  try {
    const direct = await fetch(url);
    if (direct.ok) return direct;
  } catch {
    // Cross-origin failure; fall through to the proxy.
  }
  return fetch(`/api/proxy?url=${encodeURIComponent(url)}`);
}

async function loadFromUrl(url, name) {
  say(`Fetching ${name}…`);
  try {
    const response = await fetchStorage(url);
    if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    await startRom(await response.arrayBuffer(), name);
  } catch (error) {
    say(`Could not fetch ${name}: ${error.message}`, "bad");
  }
}

async function loadFromFile(file) {
  await startRom(await file.arrayBuffer(), file.name);
}

// --- lobby --------------------------------------------------------------
//
// A room is people, not a game. Everyone in it runs their own emulator on their
// own machine; what travels is the addon snapshot — the same read-out the
// stream overlay renders — so the panel can say what everyone is playing
// without anything being emulated on the server.

/** The room we are in, or null. */
let lobby = null;
/** When a snapshot was last published. */
let lastPublishedAt = 0;
/** When a frame was last published. */
let lastFrameAt = 0;
/** Who we are watching, or null. */
let watching = null;

// --- the link cable -------------------------------------------------------
//
// One console is the parent and the rest are children, exactly as on hardware.
// The parent decides when a transfer happens; every console contributes one
// halfword; everybody ends up holding all four.
//
// Over a cable that costs a third of a scanline. Over a network it costs a
// round trip, which is why `tick` refuses to run frames while one is in
// flight: emulated time must not advance across the wait, or the game sees a
// transfer no cable could have produced and concludes the link is broken.

/** Which transfer we are on. The parent owns this number. */
let linkSeq = 0;
/** `idle`, or `collecting` while the parent waits for the other consoles. */
let linkPhase = "idle";
/** Halfwords that have arrived for the transfer being collected, by member. */
let linkAnswers = new Map();
/** When the parent gives up waiting and transfers without the stragglers. */
let linkDeadline = 0;
/** When a child gives up on a transfer that never arrived. */
let childDeadline = 0;
/** The transfer a child has joined, so a late or repeated result is ignored. */
let childSeq = -1;
/** The cable as it currently stands, as "seat/players", or "none". */
let cableShape = "none";
/** Core cycle count at the start of the current linked frame. */
let linkFrameCycle = 0;

// --- the frame barrier ----------------------------------------------------
//
// Relaying halfwords is not on its own enough to keep two consoles together.
// Each browser has its own frame clock, and between transfers a child runs
// freely while the parent is stopped waiting for it — so the child gains most
// of a frame on every transfer and the two drift apart. Pokémon exchanges a
// halfword every frame; a handshake is dozens of them, and by the end the two
// games are nowhere near each other. That is what "if one side is a little
// late, the game can wait forever" looks like from the inside.
//
// The parent is the clock. It is already held back by every transfer, because
// it cannot finish a frame until the children have answered, so only the
// children need restraining. One small message a frame does it, and costs no
// round trip of its own: the parent says how far it has got, and a child never
// runs past that.

/** Frames run since the cable was plugged in. */
let linkFrame = 0;
/** How far the parent says it has got. Children only. */
let grantedFrame = 0;
/** When the parent's clock was last heard from. */
let lastTickAt = 0;

// --- running a frame in slices --------------------------------------------
//
// JavaScript is single-threaded, so a message that arrives while a whole frame
// is being run waits for the end of it. A frame costs about fourteen
// milliseconds on this emulator, and a game trading Pokémon asks for a
// transfer roughly every two, so the answer was always late: measured round
// trips were 12.8ms on a machine talking to itself, where a socket should
// manage under one.
//
// So while a cable is attached the frame is run in slices with a yield
// between them, and the socket gets serviced in the gaps. Unlinked play keeps
// the plain synchronous path, which is proven and has nothing to gain here.

/**
 * Instructions per slice.
 *
 * Measured across a range, by round trip and by transfers a second:
 *
 * ```text
 *  2000   1.0ms   246/s
 *  4000   1.0ms   242/s
 *  8000   2.5ms   198/s
 * 16000   3.1ms   109/s
 * ```
 *
 * Below about four thousand the round trip stops improving — the benchmark
 * runs out of transfers to ask for before the pump runs out of speed — so this
 * takes the largest slice that still answers in a millisecond, which is half
 * the yields of the alternative for the same result.
 */
const SLICE_STEPS = 4000;
/** Small slices keep catch-up from running past the parent's transfer point. */
const LINK_CATCHUP_STEPS = 256;
/** A malformed or incompatible clock message must not lock the page. */
// Joining browsers do not attach on the same animation frame. A slower child
// can be tens of frames behind before FireRed begins its first transfer, so the
// bound must cover startup skew as well as an ordinary network hop. At 256
// instructions per slice this covers roughly a second of emulated frames, while
// still bounding a corrupt clock message.
const LINK_CATCHUP_MAX_SLICES = 65536;

/** Frames owed to the pacing clock but not yet run. */
let framesOwed = 0;
/** Whether the slice pump is already going round. */
let pumping = false;

/**
 * Hand control back to the event loop for the shortest time possible.
 *
 * `setTimeout` is clamped to about four milliseconds once nested, which would
 * cost more than it saves. A message channel posts back on the next macrotask
 * without that floor.
 */
const yieldToEvents = (() => {
  const channel = new MessageChannel();
  let waiting = null;
  channel.port1.onmessage = () => {
    const resume = waiting;
    waiting = null;
    resume?.();
  };
  return () =>
    new Promise((resume) => {
      waiting = resume;
      channel.port2.postMessage(0);
    });
})();

/** Run the frames the clock has allowed, a slice at a time. */
async function pumpSlices() {
  // A lockstep session owns emulated time completely. A frame run from here
  // would be a frame the other browser did not run, which is a desync — and
  // one that would not show up until a state hash caught it seconds later.
  if (session) return;
  if (pumping) return;
  pumping = true;
  try {
    while (framesOwed > 0) {
      // Everything that stops a frame being run at all, re-checked every time
      // round: the cable can come out and a transfer can begin mid-frame.
      if (!emu || !running || !emu.linkConnected) break;
      if (emu.linkPending || heldAtBarrier(performance.now())) break;

      if (emu.runSlice(SLICE_STEPS)) {
        framesOwed -= 1;
        framesRun += 1;
        linkFrame += 1;
        linkFrameCycle = emu.cycleCount;
        if (isParent() && lobby?.connected) lobby.publishLinkTick(linkFrame);
        if (audio && audio.ready && !fastForward) audio.push(emu.takeAudio());
        present();
      } else if (emu.linkPending) {
        // The slice stopped for a transfer rather than running out of steps.
        // Announcing it here is the whole point of slicing.
        driveLink(performance.now());
      }

      await yieldToEvents();
    }
  } finally {
    pumping = false;
  }
}

/**
 * A tally of what the cable has done, for when a game says the link broke.
 *
 * None of this is on screen. It is here because a link failure looks identical
 * from the outside whatever caused it, and the difference between "the other
 * console never answered", "we ran out of time waiting", and "data arrived for
 * a transfer we were not in" is the whole diagnosis. Readable in devtools as
 * `tinybird.link`.
 */
const linkTally = {
  started: 0,
  answered: 0,
  settled: 0,
  timedOut: 0,
  couldNotJoin: 0,
  /** Transfers a console bowed out of, saying so rather than going quiet. */
  skipped: 0,
  refusedDelivery: 0,
  abandoned: 0,
  /** Frames carried on part-way through, rather than waiting for the next. */
  resumed: 0,
  /** Of those, the ones that stopped at yet another transfer. */
  resumedIntoAnother: 0,
  /** Milliseconds a transfer takes to come back, averaged and at worst. */
  roundTripMs: 0,
  worstRoundTripMs: 0,
};

/** When the parent asked, so the answer can be timed. */
let askedAt = 0;
/** Transfers a second, over a rolling window. */
let linkRate = 0;
let rateWindowStart = 0;
let rateWindowCount = 0;

/**
 * The last few exchanges, newest last, as "parent/child" hex.
 *
 * A stalled link looks the same from outside whichever console went quiet.
 * Seeing what actually crossed the wire says which: a console that has stopped
 * taking part sends the same value forever, or `ffff`, while the other keeps
 * changing.
 */
const linkRecent = [];
const LINK_RECENT_KEPT = 12;

/** This console's seat on the cable, or null if it has none. */
function mySeat() {
  const me = lobby?.members.find((member) => member.id === lobby.you);
  return me?.seat ?? null;
}

/** Whether this console is the one that clocks the cable. */
function isParent() {
  return mySeat() === 0;
}

/** Everyone on the cable, including us. */
function seatedMembers() {
  return (lobby?.members ?? []).filter((member) => member.seat !== undefined && member.seat !== null);
}

/**
 * Plug into the cable, or unplug, to match the room and the toggle.
 *
 * Called whenever the membership or the toggle changes. Attaching a cable
 * makes the core watch its serial registers on every instruction, so a room
 * you joined only to watch someone should not be paying for one.
 */
function reseatCable() {
  if (!emu) return;
  if (LOCKSTEP) {
    reseatLockstep();
    return;
  }

  const seated = seatedMembers();
  const seat = mySeat();
  // The number of seats, not the number of people: somebody leaving seat 1 of
  // three leaves a gap, and the gap still reads as an absent console.
  const players = seated.length ? Math.max(...seated.map((m) => m.seat)) + 1 : 0;
  const canLink = el.link.checked && lobby?.connected && players >= 2 && seat !== null;

  // A member list arrives every time anyone loads a game or changes what they
  // are playing, not only when somebody joins or leaves. Re-seating on all of
  // them tore down a cable that had not changed and abandoned whatever
  // transfer was on it, which is what made the link work only after being
  // switched off and on again.
  const shape = canLink ? `${seat}/${players}` : "none";
  if (shape !== cableShape) {
    cableShape = shape;
    if (canLink) {
      emu.linkConnect(seat, players);
      linkFrameCycle = emu.cycleCount;
    } else {
      emu.linkDisconnect();
      linkFrameCycle = 0;
    }
    linkPhase = "idle";
    childSeq = -1;
    // A new cable is a new clock; counting on from the old one would leave a
    // child holding a barrier that the parent has never heard of.
    linkFrame = 0;
    grantedFrame = 0;
    lastTickAt = 0;
  }

  renderLinkNote();
}

/**
 * Say what the cable is doing, because none of it is otherwise visible.
 *
 * Once it is carrying traffic this turns into a read-out. A link that fails
 * looks the same from the outside whatever went wrong, and the numbers that
 * tell the causes apart — how fast the other console answers, how many
 * transfers were lost and how — are otherwise only reachable from devtools,
 * which is no use to somebody in the middle of a trade.
 */
function renderLinkNote() {
  if (!el.linkNote) return;
  if (!el.link.checked) {
    el.linkNote.textContent = "Off. Games will not see each other.";
    return;
  }
  if (!lobby?.connected) {
    el.linkNote.textContent = "On. Start or join a room to link.";
    return;
  }

  // A lockstep session reports something different from a relay, because the
  // numbers that matter are different. There is no round trip per transfer to
  // report and no lost transfers to count; what can go wrong is that a frame's
  // input did not arrive, so that is what is shown.
  if (LOCKSTEP) {
    el.linkNote.dataset.tone = sessionPhase === "failed" ? "bad" : "";
    if (sessionPhase === "failed") {
      el.linkNote.textContent = `Link stopped: ${sessionNote}`;
      return;
    }
    if (!session) {
      el.linkNote.textContent =
        sessionPhase === "opening"
          ? "Starting both consoles…"
          : sessionPhase === "offering"
            ? "Offering this console…"
            : "Waiting for a second console.";
      return;
    }
    const who =
      session.mySeat === 0
        ? `Player 1 of ${session.players}, driving the cable`
        : `Player ${session.mySeat + 1} of ${session.players}`;
    const stalls = stalledFrames > 0 ? ` · ${stalledFrames} waits` : "";
    el.linkNote.textContent =
      `${who} · ${session.players} consoles here · ${session.delay}f delay` +
      ` · ${session.bufferedFrames}f buffered${stalls}`;
    return;
  }

  if (!emu?.linkConnected) {
    el.linkNote.textContent = "Waiting for a second console.";
    return;
  }

  const seat = mySeat();
  const who =
    seat === 0
      ? `Player 1 of ${seatedMembers().length}, driving the cable`
      : `Player ${seat + 1} of ${seatedMembers().length}`;
  const transport = lobby?.linkTransport === "local" ? " · direct" : "";

  // Nothing has crossed yet: the games are linked but not talking.
  if (linkTally.settled === 0) {
    el.linkNote.textContent = `${who}${transport}. No transfers yet.`;
    return;
  }

  const rate = Math.round(linkRate);

  // Only the parent asks, so only the parent can time the answer. Showing a
  // child a round trip of zero would be inventing a number.
  const trip = seat === 0 ? ` · ${linkTally.roundTripMs.toFixed(1)}ms` : "";
  const trouble = describeLinkLoss();
  el.linkNote.textContent = `${who} · ${rate}/s${trip}${transport}${trouble}`;
  // A link that is losing transfers is the thing worth noticing, so it is the
  // one state that changes colour.
  el.linkNote.dataset.tone = trouble ? "bad" : "";
}

/**
 * What is going wrong with the cable, not just how much.
 *
 * A bare count of lost transfers says a link is unhappy and nothing about why,
 * and the four ways it can go wrong want four different fixes. Naming the one
 * that dominates turns "12 lost" into a direction to look in — which is the
 * difference between a bug report and a diagnosis.
 */
function describeLinkLoss() {
  const causes = [
    // A console asked to join a transfer it could not reach: it was ahead of
    // the parent, or too far behind to catch up. A synchronisation problem.
    [linkTally.couldNotJoin + linkTally.skipped, "out of step"],
    // Nobody answered in time. The network, or a console that stopped running.
    [linkTally.timedOut, "no answer"],
    // Data arrived at a console that was not waiting for it.
    [linkTally.refusedDelivery, "arrived late"],
    // A wait given up on, usually because the room went away.
    [linkTally.abandoned, "given up"],
  ];

  const lost = causes.reduce((total, [count]) => total + count, 0);
  if (lost === 0) return "";

  const [, worst] = causes.reduce((a, b) => (b[0] > a[0] ? b : a));
  return ` · ${lost} lost, mostly ${worst}`;
}

// --- lockstep -------------------------------------------------------------
//
// The relay above is kept because it is proven and because `?relay=1` is a way
// back if this turns out to have a hole in it. It is not the default and
// should not be: it cannot reach full speed, for reasons `link.js` sets out at
// length. Everything below runs every console in this browser and puts only
// input on the wire.

/**
 * Match the session to the room, tearing the old one down if the room changed.
 *
 * A session is built from a fixed list of seats and a state each of them
 * started from, so a seat appearing or leaving is a different session rather
 * than an adjustment to this one. The replacement is offered on the next tick;
 * doing it here would race the membership update that caused it.
 */
function reseatLockstep() {
  const want = lockstepWanted();
  const shape = want
    ? seatedMembers()
        .sort((a, b) => a.seat - b.seat)
        .map((member) => member.id)
        .join(",")
    : "none";

  if (shape !== cableShape) {
    cableShape = shape;
    endSession();
  }
  renderLinkNote();
}

/** Put the relay's cable away, so two cables are never on one console. */
function stopRelayCable() {
  if (emu?.linkConnected) emu.linkDisconnect();
  linkPhase = "idle";
  childSeq = -1;
  linkFrame = 0;
  grantedFrame = 0;
  lastTickAt = 0;
  linkFrameCycle = 0;
}

/** Whether to run linked play in lockstep rather than by relaying halfwords. */
const LOCKSTEP = new URLSearchParams(window.location.search).get("relay") !== "1";

/** The live session, or null. */
let session = null;
/** `off`, `offering`, `opening`, `live` or `failed`. */
let sessionPhase = "off";
/** Why the last session stopped, for the read-out. */
let sessionNote = "";
/** What every seated console published about itself, by member id. */
const hellos = new Map();
/**
 * Which console each member is, by member id.
 *
 * The room's own seat numbers are not used directly: a room may leave a gap —
 * somebody in seat 2 with seat 1 empty — and a cable may not, so a session
 * compacts them and this is the mapping it settled on.
 */
let sessionSeats = new Map();

/** Which console a member is in the running session, or null. */
function seatOf(id) {
  const seat = sessionSeats.get(id);
  return seat === undefined ? null : seat;
}
/** The cartridge in the machine, and its fingerprint. */
let romBytes = null;
let romHash = "";
/** The BIOS, so a second console can be given the same one. */
let biosBytes = null;
/** Consoles built for other players' seats, reused across sessions. */
const peerConsoles = new Map();
/** The session this browser has asked the room to open, before it opened. */
let begunSession = "";
/** When our own hello went out, so the echo measures the trip to the server. */
let helloSentAt = 0;
/** Round trip to the relay, in milliseconds. Decides the input delay. */
let relayRoundTripMs = 60;
/** Frames the session wanted to run but could not, for want of input. */
let stalledFrames = 0;
/** Library listing, fetched once when a session needs somebody else's game. */
let libraryCache = null;

/** Whether a lockstep session should be running, given the room and the toggle. */
function lockstepWanted() {
  if (!LOCKSTEP || !emu?.hasRom || !el.link.checked) return false;
  if (!lobby?.connected) return false;
  const seated = seatedMembers();
  return seated.length >= 2 && mySeat() !== null;
}

/**
 * Offer this console to the room, so a session can be built from it.
 *
 * What is offered is a save state, not a battery save. A battery save would
 * mean resetting to the title screen to start a link, which is not what a cable
 * does; a state means both players carry on from where they are. It costs a
 * few hundred kilobytes once, against nothing per frame afterwards.
 *
 * The state is also what makes determinism achievable at all: every browser
 * restores every console from the same bytes, so the two start from a position
 * they provably agree on rather than from two independently-arrived-at guesses.
 */
async function offerConsole() {
  // Only from a standing start. A failed session stays failed until the room
  // changes: retrying a link that just corrupted itself, once a frame, is how
  // a bad state becomes an unusable page.
  if (sessionPhase !== "off") return;
  if (!lockstepWanted() || !romHash) return;

  sessionPhase = "offering";
  sessionNote = "";
  renderLinkNote();

  try {
    const state = await packState(emu.saveState());
    helloSentAt = performance.now();
    lobby.publishLinkHello({
      seat: mySeat(),
      romHash,
      romName,
      gameCode,
      state,
    });
  } catch (error) {
    failSession(`could not offer this console: ${error.message}`);
  }
}

/**
 * Take somebody's offer, and open a session once every seat has made one.
 *
 * Only the parent decides when. Everyone else waits to be told, which is what
 * makes the terms — the seed, the delay, the order of seats — the same set of
 * terms on every machine rather than three machines' opinions.
 */
function acceptHello(from, message) {
  hellos.set(from, message);

  if (from === lobby.you && helloSentAt) {
    // Our own hello coming back has been to the server and returned, which is
    // the round trip the input delay has to cover. Measuring it here costs no
    // message of its own.
    relayRoundTripMs = performance.now() - helloSentAt;
    helloSentAt = 0;
  }

  if (!isParent() || sessionPhase === "live" || sessionPhase === "opening") return;

  const seated = seatedMembers().sort((a, b) => a.seat - b.seat);
  if (seated.length < 2 || !seated.every((member) => hellos.has(member.id))) return;

  // Asking is remembered separately from having opened, because between the
  // two there is a round trip. A fourth player's hello arriving inside it
  // would otherwise ask a second time, and the room would be told to open two
  // sessions whose order nobody agrees on.
  if (begunSession) return;
  begunSession = `${lobby.you}:${Date.now().toString(36)}`;

  lobby.publishLinkBegin({
    session: begunSession,
    // The cartridge clock every console starts from. Whole seconds, because
    // that is the resolution the chip has, and one number rather than each
    // browser's own `Date.now`, which is the whole point.
    seed: Math.floor(Date.now() / 1000),
    delay: delayForRoundTrip(relayRoundTripMs + 20),
    // Seat order, compacted. A room can leave a gap — somebody in seat 2 with
    // seat 1 empty — and a cable cannot, so the console a player sits in is
    // their position in this list.
    seats: seated.map((member) => member.id),
  });
}

/** Open the session the parent has described. */
async function beginSession(message) {
  if (sessionPhase === "live" || sessionPhase === "opening") return;
  if (!lockstepWanted()) return;

  const ids = Array.isArray(message.seats) ? message.seats : [];
  const mine = ids.indexOf(lobby.you);
  if (mine < 0) return;

  const entries = [];
  for (let seat = 0; seat < ids.length; seat += 1) {
    const hello = hellos.get(ids[seat]);
    if (!hello) {
      failSession("a player did not say what they were running");
      return;
    }
    entries.push({
      seat,
      id: ids[seat],
      romHash: hello.rom_hash,
      romName: hello.rom_name,
      gameCode: hello.game_code,
      packed: hello.state,
    });
  }

  sessionPhase = "opening";
  sessionNote = "";
  sessionSeats = new Map(ids.map((id, seat) => [id, seat]));
  renderLinkNote();

  try {
    // The relay's cable must come out first. Both would otherwise be attached
    // to the same console, and the relay's timeouts would abandon transfers
    // the session had already carried.
    stopRelayCable();

    for (const entry of entries) entry.state = await unpackState(entry.packed);

    session = await openSession({
      id: message.session,
      seats: entries,
      mySeat: mine,
      localEmu: emu,
      delay: Math.min(12, Math.max(2, Number(message.delay) || 3)),
      seed: Number(message.seed) || Math.floor(Date.now() / 1000),
      bios: biosBytes,
      makeConsole: () => spareConsole(entries.length),
      resolveRom: resolvePeerRom,
    });

    sessionPhase = "live";
    stalledFrames = 0;
    framesOwed = 0;
    frameClock = 0;
    running = true;
    say(`Linked with ${entries.length - 1} other player. Running both consoles here.`);
  } catch (error) {
    failSession(error.message);
  } finally {
    renderLinkNote();
  }
}

/**
 * A second emulator for somebody else's seat.
 *
 * The module is instantiated again rather than the core being taught about
 * multiple machines. Each instantiation gets its own linear memory, so the
 * `static mut EMULATOR` inside is a singleton per instance and two of them do
 * not see each other — which is exactly the isolation two consoles want, for
 * no Rust at all.
 */
async function spareConsole(players) {
  for (const [seat, core] of peerConsoles) {
    if (seat < players) {
      peerConsoles.delete(seat);
      return core;
    }
  }
  return TinyBird.load();
}

/**
 * Find the cartridge a seat needs.
 *
 * The common case is that it is the one already in the machine: two people
 * battling, or trading between two copies of the same game. Beyond that we can
 * only offer what this browser already has — ROMs are not relayed, so a link
 * between two different games needs both games on both machines.
 */
async function resolvePeerRom(entry) {
  if (entry.romHash === romHash && romBytes) return romBytes;

  if (libraryCache === null) {
    const [vault, local] = await Promise.all([fetchVault(), fetchLocal()]);
    libraryCache = [...vault.assets, ...local];
  }

  const named = libraryCache.filter((asset) => asset.name === entry.romName);
  for (const asset of named) {
    try {
      const response = await fetchStorage(asset.url);
      if (!response.ok) continue;
      const bytes = new Uint8Array(await response.arrayBuffer());
      if ((await romFingerprint(bytes)) === entry.romHash) return bytes;
    } catch {
      // Try the next candidate rather than failing the whole session on one
      // unreachable vault entry.
    }
  }

  throw new Error(
    `this link needs ${entry.romName || "another cartridge"}, which is not in this browser`,
  );
}

/** Stop the session, saying why. */
function failSession(reason) {
  if (session) {
    session.detach();
    for (let seat = 0; seat < session.consoles.length; seat += 1) {
      if (seat !== session.mySeat) peerConsoles.set(seat, session.consoles[seat]);
    }
  }
  session = null;
  sessionPhase = "failed";
  sessionNote = reason;
  hellos.clear();
  begunSession = "";
  say(`Link stopped: ${reason}`, "bad");
  renderLinkNote();
}

/** Take the session down without calling it a failure. */
function endSession() {
  if (session) {
    if (lobby?.connected) lobby.publishLinkBye(session.id);
    // Kept for the next session. Instantiating the module means fetching and
    // compiling it again, which is a visible pause, and a room's membership
    // changes every time anybody loads a game.
    for (let seat = 0; seat < session.consoles.length; seat += 1) {
      if (seat !== session.mySeat) peerConsoles.set(seat, session.consoles[seat]);
    }
    session.detach();
  }
  session = null;
  sessionPhase = "off";
  sessionNote = "";
  hellos.clear();
  begunSession = "";
}

/** Publish this player's mask for a frame far enough ahead to arrive in time. */
function publishInput() {
  const at = session.pushLocal(buttons);
  if (at !== null) lobby.publishLinkInput(session.id, at, buttons);
}

/**
 * Run the session for this animation frame.
 *
 * Where the relay stalls nine times a frame — once per transfer — this stalls
 * at most once, and only when a message for the next frame has not arrived
 * yet. Everything else, the whole cable included, happens here without leaving
 * the page.
 */
function runLockstep(now) {
  const due = schedule(now, frameClock, 1);
  frameClock = due.clock;
  framesOwed = Math.min(framesOwed + due.frames, MAX_CATCHUP_FRAMES);

  publishInput();

  let ran = 0;
  while (framesOwed > 0) {
    if (!session.ready) {
      // The next frame is waiting on somebody else. Debt accrued while waiting
      // would be spent racing ahead the moment it lands, so it is dropped —
      // the same rule the unlinked loop uses when the page was paused.
      stalledFrames += 1;
      framesOwed = 0;
      frameClock = 0;
      break;
    }

    if (!session.runFrame()) {
      failSession("a frame could not be completed");
      return 0;
    }

    framesOwed -= 1;
    framesRun += 1;
    ran += 1;
    publishInput();

    if (audio && audio.ready) audio.push(session.local.takeAudio());
    // The other consoles produce sound too, into a buffer nobody empties.
    // Emptying it is what keeps that from growing for the length of a session.
    for (let seat = 0; seat < session.consoles.length; seat += 1) {
      if (seat !== session.mySeat) session.consoles[seat].takeAudio();
    }

    if (session.hashDue) {
      const value = session.hash();
      lobby.publishLinkHash(session.id, session.frame, value);
      if (session.recordHash(session.frame, value) !== null) {
        failSession("the two consoles computed different states");
        return ran;
      }
    }
  }

  // Drawing is the caller's, which already knows to do it only when something
  // changed.
  return ran;
}

/**
 * Carry a transfer, if the parent has one to carry.
 *
 * Only the parent does anything here. A child answers when it is asked, which
 * arrives as a message rather than on the frame clock.
 */
function driveLink(now) {
  // A transfer that cannot possibly be answered — the cable came out, or the
  // room went away — has to be let go of, or this console never runs again.
  if (!emu?.linkConnected || !lobby?.connected) {
    if (emu?.linkPending && emu.linkAbandon()) linkTally.abandoned++;
    return;
  }

  if (!isParent()) {
    // A child waits for data it has been promised, and that wait is what keeps
    // it in step. If the promise is not kept it has to give up rather than sit
    // there: the parent has a deadline, and until this was added a child had
    // none at all, so one lost message stopped its game for good.
    if (emu.linkPending && now >= childDeadline && emu.linkAbandon()) {
      linkTally.abandoned++;
    }
    return;
  }

  if (linkPhase === "idle" && emu.linkPending) {
    linkSeq = (linkSeq + 1) & 0xffff;
    linkPhase = "collecting";
    linkAnswers = new Map([[lobby.you, emu.linkSendValue]]);
    linkDeadline = now + LINK_TIMEOUT_MS;
    askedAt = now;
    linkTally.started++;
    const offset = Math.max(0, Math.floor(emu.cycleCount - linkFrameCycle));
    lobby.publishLinkStart(linkSeq, linkFrame, offset);
    return;
  }

  if (linkPhase !== "collecting") return;

  // Going ahead without a straggler is not a failure mode, it is the one the
  // hardware has: a console that does not answer reads as absent, and the
  // game's own link code already knows what to do about that. Waiting forever
  // instead would hang a game that a player simply closed the tab on.
  if (everyoneAnswered()) {
    settleLink();
  } else if (now >= linkDeadline) {
    linkTally.timedOut++;
    settleLink();
  }
}

/**
 * Carry on through a frame that a transfer interrupted.
 *
 * The core stops a frame the instant a transfer begins, so a frame containing
 * several transfers needs several resumes before it is finished. Pokémon asks
 * for about seven a frame during a trade — four hundred a second — and waiting
 * for the next animation frame between each capped the link at one per display
 * frame, about sixty. Eight times too slow, and what the game makes of that is
 * a black screen and "Communication error".
 *
 * This is not extra emulated time. The console stopped part-way through a
 * frame the pacing had already allowed; this finishes that same frame. The
 * rate is bounded by the network round trip, because each resume can only
 * happen once the previous transfer's data has come back.
 */
function resumeAfterTransfer() {
  if (!emu?.linkConnected || !running) return;
  linkTally.resumed++;
  // The pump does the running; this only wakes it. It stopped because a
  // transfer was outstanding, and that transfer has just landed.
  pumpSlices();
}

/**
 * Bring a child to the instruction-time at which the parent started a transfer.
 *
 * The parent resumes as soon as it has collected the previous halfword, while
 * the child receives that result one network hop later. Without this catch-up,
 * the next start can make the child answer with the previous protocol value.
 * FireRed tolerates a few milliseconds of that skew, then reports a link error
 * immediately after the two confirmation prompts.
 */
function catchUpToLinkStart(targetFrame, targetOffset) {
  // Relay-only, like everything it runs. In a session the two consoles are
  // already at the same instruction, because they are both in this browser.
  if (session) return false;
  const frame = Number(targetFrame);
  const offset = Number(targetOffset);
  if (!Number.isSafeInteger(frame) || !Number.isSafeInteger(offset) || frame < 0 || offset < 0) {
    return false;
  }

  // Being past the parent's frame cannot be repaired without rollback. The
  // frame barrier should make this impossible; refusing is safer than sending
  // a value from the future and corrupting every later protocol word.
  if (linkFrame > frame) return false;

  // This is synchronization work, not pacing debt. Do not race ahead again on
  // the next animation callback to make up the wall time spent doing it.
  frameClock = 0;
  let slices = 0;
  while (
    linkFrame < frame &&
    !emu.linkPending &&
    slices < LINK_CATCHUP_MAX_SLICES
  ) {
    const finished = emu.runSlice(LINK_CATCHUP_STEPS);
    slices += 1;
    if (finished) {
      framesOwed = Math.max(0, framesOwed - 1);
      framesRun += 1;
      linkFrame += 1;
      linkFrameCycle = emu.cycleCount;
      if (audio && audio.ready && !fastForward) audio.push(emu.takeAudio());
      present();
    }
  }
  if (linkFrame !== frame) return false;

  const targetCycle = linkFrameCycle + offset;
  while (
    emu.cycleCount < targetCycle &&
    linkFrame === frame &&
    !emu.linkPending &&
    slices < LINK_CATCHUP_MAX_SLICES
  ) {
    // Use broad slices while far away, then approach one instruction at a
    // time. A broad final slice can cross VBlank, complete the next frame and
    // turn a child that was microscopically behind into one a frame ahead.
    const remaining = targetCycle - emu.cycleCount;
    const steps = remaining > 4096 ? LINK_CATCHUP_STEPS : remaining > 256 ? 16 : 1;
    const finished = emu.runSlice(steps);
    slices += 1;
    if (finished) {
      framesOwed = Math.max(0, framesOwed - 1);
      framesRun += 1;
      linkFrame += 1;
      linkFrameCycle = emu.cycleCount;
      if (audio && audio.ready && !fastForward) audio.push(emu.takeAudio());
      present();
    }
  }
  return linkFrame === frame && emu.cycleCount >= targetCycle;
}

/**
 * Whether this console has run as far as the parent has, and must wait.
 *
 * Only children wait: the parent is the clock, and holding it back would leave
 * nobody to follow. A child that has heard nothing for a while runs anyway —
 * a barrier that never lifts is a hang, and a parent that paused or closed its
 * tab must not take the other player's game with it.
 */
function heldAtBarrier(now) {
  if (!emu?.linkConnected || isParent()) return false;
  if (now - lastTickAt > LINK_TICK_TIMEOUT_MS) return false;
  // A lead of one means the child may advance from one frame behind to level,
  // but may not begin the frame that would put it ahead.
  return linkFrame + LINK_MAX_LEAD > grantedFrame;
}

/** Whether every console on the cable has offered its halfword. */
function everyoneAnswered() {
  return seatedMembers()
    .filter((member) => member.id !== lobby.you)
    .every((member) => linkAnswers.has(member.id));
}

/** Hand the collected halfwords to every console, our own included. */
function settleLink() {
  const values = [0xffff, 0xffff, 0xffff, 0xffff];
  for (const member of seatedMembers()) {
    const answer = linkAnswers.get(member.id);
    if (answer !== undefined) values[member.seat] = answer & 0xffff;
  }

  // The parent drives the clock line, so its transfer time is the cable's and
  // travels with the data. A child timing the wire from its own baud setting
  // would hold it far too long and miss the next transfer.
  const cycles = emu.linkTransferCycles;

  // How long the answer took. This is the whole budget: a game asking for
  // several hundred transfers a second cannot have them if each costs more
  // than a couple of milliseconds.
  if (askedAt) {
    const trip = performance.now() - askedAt;
    askedAt = 0;
    linkTally.roundTripMs = linkTally.roundTripMs
      ? linkTally.roundTripMs * 0.9 + trip * 0.1
      : trip;
    linkTally.worstRoundTripMs = Math.max(linkTally.worstRoundTripMs, trip);
  }

  linkRecent.push(values.map((v) => v.toString(16).padStart(4, "0")).join("/"));
  if (linkRecent.length > LINK_RECENT_KEPT) linkRecent.shift();

  linkPhase = "idle";
  if (emu.linkDeliver(values, cycles)) linkTally.settled++;
  else linkTally.refusedDelivery++;
  lobby.publishLinkData(linkSeq, values, cycles);

  // And straight on with the frame, rather than idling until the next one.
  resumeAfterTransfer();
}
/** Members whose frames have arrived recently, by id. */
const liveSince = new Map();

/**
 * How long a member counts as sharing after their last frame.
 *
 * Somebody who stops sharing sends nothing rather than announcing it, so this
 * is what makes their light go out.
 */
const LIVE_TIMEOUT_MS = 2000;

/** Remove the remote picture and return the play space to its solo layout. */
function clearWatchedScreen() {
  el.lobbyWatch.hidden = true;
  el.lobbyScreen.removeAttribute("src");
  el.lobbyScreen.alt = "Shared game screen";
  el.lobbyCaption.textContent = "\u2014";
  el.lobbyWorkspace.dataset.view = "solo";
}

/** Whether a member's screen is currently arriving. */
function isLive(id) {
  const at = liveSince.get(id);
  return at !== undefined && performance.now() - at < LIVE_TIMEOUT_MS;
}

/**
 * Notice when somebody's screen stops arriving.
 *
 * Nobody announces that they stopped sharing; they just stop sending. Without
 * this the light stays on and their last frame sits there looking live.
 */
function expireStaleSharers() {
  let changed = false;
  for (const id of [...liveSince.keys()]) {
    if (!isLive(id)) {
      liveSince.delete(id);
      changed = true;
      if (watching === id) {
        watching = null;
        clearWatchedScreen();
      }
    }
  }
  if (changed && lobby) renderMembers(lobby.members);
}

function sayLobby(text, tone) {
  el.lobbyHint.textContent = text;
  if (tone) {
    el.lobbyHint.dataset.tone = tone;
  } else {
    delete el.lobbyHint.dataset.tone;
  }
}

/**
 * The name to offer when joining.
 *
 * Only used on a server with no accounts. When there are accounts the server
 * takes the name from the session and ignores this entirely — a member's name
 * is not something the browser gets to choose, for the same reason it does not
 * get to choose who owns a save.
 */
function lobbyName() {
  return recall("lobbyName") || "guest";
}

function renderMembers(members) {
  el.lobbyMembers.replaceChildren(
    ...members.map((member) => {
      const item = document.createElement("li");
      item.className = "lobby__member";
      if (lobby && member.id === lobby.you) item.classList.add("is-you");

      const live = isLive(member.id);
      if (live) {
        item.classList.add("is-live");
        item.append(span("lobby__live", ""));
        // Clicking a member who is sharing switches to watching them.
        item.addEventListener("click", () => watchMember(member.id));
      }
      if (watching === member.id) item.classList.add("is-watched");

      item.append(span("lobby__name", member.name));
      if (member.host) item.append(span("lobby__host", "host"));
      item.append(
        span("lobby__playing", member.playing ? member.playing : "nothing loaded"),
      );
      return item;
    }),
  );
  el.lobbyNote.textContent = `${members.length} here`;

  // Only the host can lock the room, so only the host is offered the button.
  const you = members.find((m) => lobby && m.id === lobby.you);
  el.lock.hidden = !you?.host;
}

/** Start showing a member's screen, or stop if they are already shown. */
function onMembersChanged(members) {
  renderMembers(members);
  // Somebody arriving or leaving changes who is on the cable and where.
  reseatCable();
}

/** Start showing a member's screen, or stop if they are already shown. */
function watchMember(id) {
  const next = watching === id ? null : id;
  // A frame belongs to the old selection until the newly selected member sends
  // one. Clear it now so their name is never attached to somebody else's game.
  if (next !== watching) clearWatchedScreen();
  watching = next;
  if (lobby) renderMembers(lobby.members);
}

/** Show a frame that has just arrived, if it is from whoever we are watching. */
function showFrame(from, frame) {
  const wasLive = isLive(from);
  liveSince.set(from, performance.now());

  // The first frame from anyone changes the list: their row becomes a target.
  if (!wasLive && lobby) renderMembers(lobby.members);

  // Nobody chosen yet? Watch the first person who shares, so that turning it
  // on at the other end is enough to see something.
  if (!watching) {
    watching = from;
    if (lobby) renderMembers(lobby.members);
  }
  if (from !== watching) return;

  el.lobbyWatch.hidden = false;
  el.lobbyScreen.src = frame;
  const member = lobby?.members.find((m) => m.id === from);
  el.lobbyScreen.alt = member
    ? `${member.name}'s shared game screen`
    : "Shared game screen";
  el.lobbyCaption.textContent = member
    ? `${member.name}${member.playing ? ` · ${member.playing}` : ""}`
    : "watching";
  el.lobbyWorkspace.dataset.view = "shared";
}

/** Send a picture of our screen, if anyone is there to see it. */
function shareFrame(now) {
  if (!el.share.checked || !lobby?.connected) return;
  // Nobody else in the room: the bandwidth would go nowhere.
  if (lobby.members.length < 2) return;
  if (now - lastFrameAt < FRAME_INTERVAL_MS) return;

  lastFrameAt = now;
  // JPEG rather than PNG: a tenth of the size at this scale, and the artefacts
  // are invisible once the picture is a thumbnail in someone's sidebar.
  lobby.publishFrame(frameCanvas.toDataURL("image/jpeg", FRAME_QUALITY));
}

el.share.addEventListener("change", () => {
  remember("share", el.share.checked ? "1" : "0");
  if (!el.share.checked) lastFrameAt = 0;
});

el.link.addEventListener("change", () => {
  remember("link", el.link.checked ? "1" : "0");
  reseatCable();
});

async function joinRoom(code) {
  const room = code.trim();
  if (!room) {
    sayLobby("Enter a room code, or start a room.", "bad");
    return;
  }

  leaveRoom();
  sayLobby("Connecting…");

  lobby = new LobbyConnection({
    room,
    name: lobbyName(),
    onMembers: onMembersChanged,
    onRefused: (message) => {
      // A bad code, a full room, or a locked one. All final, all worth saying
      // plainly rather than leaving the panel spinning.
      lobby = null;
      el.lobbyOut.hidden = false;
      el.lobbyIn.hidden = true;
      el.lobbyNote.textContent = "";
  paintLobbyBadge();
      el.lock.hidden = true;
      sayLobby(message, "bad");
    },
    onLocked: (locked) => {
      el.lock.textContent = locked ? "unlock" : "lock";
      el.lobbyNote.textContent = locked ? "locked" : `${lobby?.members.length ?? 0} here`;
      paintLobbyBadge();
    },
    onSnapshot: () => {
      // Somebody else's read-out. What it is for — showing a room-mate's party
      // in the overlay — is still to come; the picture is the useful half.
    },
    onFrame: showFrame,

    onLinkTick: (frame) => {
      if (isParent()) return;
      grantedFrame = frame;
      lastTickAt = performance.now();
    },

    // The parent hears its own announcement back. That is deliberate: it is
    // one message defining one transfer, so nobody has to agree separately on
    // when it began.
    onLinkStart: (seq, frame, offset) => {
      if (!emu?.linkConnected || isParent()) return;

      // Do not sample SIOMLT_SEND until this game has reached the instruction
      // where the parent began the transfer. Network latency otherwise leaves
      // the child answering one protocol word behind.
      // A console that cannot take this transfer says so rather than going
      // quiet. Silence reaches the same conclusion, but only after the
      // parent's timeout — and the parent's game is frozen for the whole of
      // it, so a run of missed transfers turned into a run of stalls. That is
      // what a game sees as a link that has stopped responding.
      //
      // Answering with a halfword would be worse than either: the parent would
      // hand back data this console cannot receive.
      if (!catchUpToLinkStart(frame, offset)) {
        linkTally.couldNotJoin++;
        lobby.publishLinkSkip(seq);
        return;
      }

      if (!emu.linkJoin()) {
        linkTally.couldNotJoin++;
        lobby.publishLinkSkip(seq);
        return;
      }
      childSeq = seq;
      childDeadline = performance.now() + LINK_TIMEOUT_MS;
      lobby.publishLinkValue(seq, emu.linkSendValue);
    },

    onLinkSkip: (from, seq) => {
      if (!isParent() || linkPhase !== "collecting" || seq !== linkSeq) return;

      // Counted as answered, with nothing on the wire. On hardware a console
      // that does not drive its slot reads as 0xFFFF, which is exactly what a
      // game expects from a seat that is not there — and it arrives now rather
      // than after a timeout the parent spends frozen.
      linkAnswers.set(from, 0xffff);
      linkTally.skipped++;
      if (everyoneAnswered()) settleLink();
    },

    onLinkValue: (from, seq, value) => {
      if (!isParent() || linkPhase !== "collecting" || seq !== linkSeq) return;
      linkAnswers.set(from, value);
      linkTally.answered++;

      // Settle here, the moment the last console answers, rather than leaving
      // it to the next animation frame. A linked game asks for a transfer
      // every frame and the console is frozen for the whole of each one, so
      // waiting for the frame clock added up to 16ms to every transfer — which
      // is most of where the speed went.
      if (everyoneAnswered()) settleLink();
    },

    onLinkData: (seq, values, cycles) => {
      // The parent already handed these to its own console; delivering twice
      // would start a second transfer that nobody asked for.
      if (isParent()) return;

      // Taken whenever this console is waiting, whatever the sequence number
      // says. Refusing data it is waiting for left it frozen with no way back:
      // one slipped number and the game sat on "Awaiting linkup" for good.
      // Messages arrive in the order they were sent, so a stale one cannot
      // overtake a live one anyway, and `linkDeliver` refuses anything that
      // arrives when this console is not waiting.
      childSeq = -1;
      if (emu.linkDeliver(values, cycles)) linkTally.settled++;
      else linkTally.refusedDelivery++;
      resumeAfterTransfer();
    },

    // --- lockstep ---------------------------------------------------------

    onLinkHello: (from, message) => {
      if (!LOCKSTEP) return;
      acceptHello(from, message);
    },

    onLinkBegin: (message) => {
      if (!LOCKSTEP) return;
      beginSession(message);
    },

    onLinkInput: (from, id, frame, keys) => {
      if (!session || id !== session.id) return;
      const seat = seatOf(from);
      if (seat === null) return;
      if (!session.acceptInput(seat, frame, keys)) {
        // A mask for a frame already run, or one further ahead than the ring
        // holds. Either way the two browsers are further apart than the buffer
        // covers, and carrying on would run a frame on input that is not the
        // input the other browser used.
        failSession("the two consoles drifted too far apart");
      }
    },

    onLinkHash: (from, id, frame, hash) => {
      if (!session || id !== session.id) return;
      if (session.acceptHash(frame, hash) !== null) {
        failSession("the two consoles computed different states");
      }
    },

    onLinkBye: (from, id) => {
      if (!session || id !== session.id) return;
      // Said on purpose, so this is not a failure — the other player closed
      // the cable rather than fell off it.
      endSession();
      say("The other player left the link.");
      renderLinkNote();
    },
    onStatus: (status) => {
      if (status === "connected") {
        el.lobbyOut.hidden = true;
        el.lobbyIn.hidden = false;
        el.lobbyRoom.textContent = lobby.room;
        // The "Connecting…" line has done its job; the member count says the
        // rest, and a stale status is worse than none.
        sayLobby("");
        remember("room", lobby.room);
        // Tell the room what is already loaded, rather than waiting for the
        // next cartridge change.
        lobby.setPlaying(romName || null, gameCode || null);
      } else if (status !== "closed") {
        // "closed" is something we did on purpose, and follows a refusal we
        // have already explained; reporting it would overwrite the reason.
        el.lobbyNote.textContent = status;
      }
    },
  });
}

function leaveRoom() {
  lobby?.close();
  lobby = null;
  el.lobbyOut.hidden = false;
  el.lobbyIn.hidden = true;
  el.lobbyNote.textContent = "";
  paintLobbyBadge();
  el.lock.hidden = true;
  el.lock.textContent = "lock";
  el.lobbyMembers.replaceChildren();
  watching = null;
  liveSince.clear();
  clearWatchedScreen();

  // The cable goes with the room. `lobby` is already null, so reseating
  // unplugs, and the shape is cleared so rejoining is seen as a change.
  cableShape = "none";
  emu?.linkDisconnect();
  linkPhase = "idle";
  childSeq = -1;
  renderLinkNote();
}

el.join.addEventListener("click", () => joinRoom(el.lobbyCode.value));

el.lobbyCode.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    joinRoom(el.lobbyCode.value);
  }
});

el.lock.addEventListener("click", () => {
  if (!lobby) return;
  lobby.setLocked(!lobby.locked);
});

el.host.addEventListener("click", async () => {
  el.host.disabled = true;
  try {
    const response = await fetch("/api/lobby", { method: "POST" });
    const body = await response.json();
    // 401 here means accounts are on and nobody is signed in, which is a
    // different problem from the server being busy.
    if (response.status === 401) {
      sayLobby("Sign in to start a room.", "bad");
      return;
    }
    if (!response.ok || !body.room) throw new Error(body.error ?? `${response.status}`);
    el.lobbyCode.value = body.room;
    await joinRoom(body.room);
  } catch (error) {
    sayLobby(`Could not start a room: ${error.message}`, "bad");
  } finally {
    el.host.disabled = false;
  }
});

el.leave.addEventListener("click", () => {
  leaveRoom();
  say("Left the room");
});

el.copyCode.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(el.lobbyRoom.textContent);
    el.copyCode.textContent = "copied";
    setTimeout(() => (el.copyCode.textContent = "copy"), 2000);
  } catch {
    // Clipboard access can be refused; the code is on screen to read.
  }
});

// --- lobby dialog -------------------------------------------------------
//
// Hosting or joining is a thing you do once at the start of a session, so it
// lives behind a button rather than in a third of a rail. What lasts — someone
// else's screen, and whether the cable is live — stays on the page: the badge
// on the key says you are in a room, and the watched screen sits under your
// own.

el.lobbyOpen.addEventListener("click", () => el.lobbySheet.showModal());

/** Keep the deck key in step with the room, since the panel is usually shut. */
function paintLobbyBadge() {
  const inRoom = Boolean(lobby?.connected);
  el.lobbyBadge.hidden = !inRoom;
  el.lobbyBadge.textContent = emu?.linkConnected ? "link" : "in";
  el.lobbyOpen.title = inRoom
    ? "You are in a room. Open the lobby to leave or share your screen."
    : "Host or join a room and play alongside other people";
}

// --- controls dialog ----------------------------------------------------

el.controlsOpen.addEventListener("click", openControls);

el.controlsReset.addEventListener("click", () => {
  controls.reset();
  renderControls();
  paintShortcuts();
  say("Controls reset to defaults");
});

// Closing mid-capture would otherwise leave the page listening for a key that
// no longer has anywhere to go.
el.controlsSheet.addEventListener("close", () => {
  if (capturing) capturing = null;
  el.controlsSheet.dataset.capturing = "off";
  // A key held when the dialog opened was never released as far as the game
  // knows, so start the player from a clean slate.
  heldKeys.clear();
  applyInput();
});

// A controller plugged in while the panel is open should show up in it.
window.addEventListener("gamepadconnected", () => {
  if (el.controlsSheet.open) renderControls();
});

paintShortcuts();
refreshPadPresence();
refreshShots();

// --- account ------------------------------------------------------------
//
// The menu itself is account.js, shared with every other page. What is left
// here is the part only this page has: saves and screenshots belong to whoever
// is signed in, so a change of identity has to reload them.

/** The account menu, once startup has built it. */
let accounts = null;

/**
 * The one button this page adds to the menu.
 *
 * Built here rather than in the shared markup: claiming saves means nothing on
 * a page with no saves on it.
 */
const claimButton = document.createElement("button");
claimButton.className = "key key--slim";
claimButton.type = "button";
claimButton.hidden = true;

/** Whether the first read of who-we-are has landed. */
let accountKnown = false;

/**
 * React to a change of identity.
 *
 * The first call is startup asking; everything after it is a real change, and
 * a real change means the vault on screen belongs to the wrong person.
 */
function onAccountChange(user) {
  offerClaim();
  if (!accountKnown) {
    accountKnown = true;
    return;
  }
  // Deliberately not "signed in as <address>": the status line is the one part
  // of the page always on screen, and putting the address there undoes the
  // reason the menu hides it.
  say(user ? "Signed in" : "Signed out");
  // Shots as well as saves: leaving the last account's screenshots on screen
  // would outlive the session that was allowed to see them.
  refreshVault();
}

/**
 * Offer to take over saves stored before accounts existed.
 *
 * Only shown when there is something to take: a button that does nothing is
 * worse than no button.
 */
async function offerClaim() {
  claimButton.hidden = true;
  if (!accounts?.user) return;
  try {
    const response = await fetch("/api/saves?legacy=1");
    if (!response.ok) return;
    const body = await response.json();
    claimButton.hidden = (body.legacy ?? 0) === 0;
    if (!claimButton.hidden) {
      claimButton.textContent = `Claim ${body.legacy} old save${body.legacy === 1 ? "" : "s"}`;
    }
  } catch {
    // Nothing to offer; the button stays hidden.
  }
}

claimButton.addEventListener("click", async () => {
  claimButton.disabled = true;
  say("Claiming saves…");
  try {
    const response = await fetch("/api/saves/claim", { method: "POST" });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error ?? `${response.status}`);
    say(`Claimed ${body.claimed} save${body.claimed === 1 ? "" : "s"}`, "good");
    if (body.failed?.length) console.warn("could not claim:", body.failed);
    await refreshSaves();
    await offerClaim();
  } catch (error) {
    say(`Could not claim those saves: ${error.message}`, "bad");
  } finally {
    claimButton.disabled = false;
  }
});

// --- cloud saves --------------------------------------------------------

/** A PNG of the current screen, for identifying a save at a glance. */
async function captureThumbnail() {
  const blob = await new Promise((resolve) => frameCanvas.toBlob(resolve, "image/png"));
  if (!blob) return null;
  return new Uint8Array(await blob.arrayBuffer());
}

/**
 * Store the current state against the loaded cartridge.
 *
 * The server files it under the game code and prunes that game's oldest beyond
 * the limit, so the list stays bounded without the player managing it.
 */
el.cloud.addEventListener("click", () => storeSave());

/**
 * Pack the current state and send it to the vault.
 *
 * `replace` overwrites an existing slot, keeping its label so the row still
 * reads as the same slot. The server only removes the old save once the new one
 * has landed, so a failed overwrite costs nothing.
 */
async function storeSave(replace = null) {
  if (!gameCode) {
    say("Load a game before saving to the vault.", "bad");
    return false;
  }

  let state;
  try {
    state = emu.saveState();
  } catch (error) {
    say(error.message, "bad");
    return false;
  }

  el.cloud.disabled = true;
  say(`Compressing ${formatSize(state.byteLength)}…`);

  let packed;
  try {
    const thumbnail = await captureThumbnail();
    const battery = emu.hasBattery ? emu.batterySave() : null;
    const compressed = await gzip(state);
    packed = packSave(compressed.bytes, compressed.gzipped, thumbnail, battery);
  } catch (error) {
    say(`Could not prepare the save: ${error.message}`, "bad");
    el.cloud.disabled = false;
    return false;
  }

  say(`Uploading ${formatSize(packed.byteLength)}…`);

  const form = new FormData();
  form.append("file", new Blob([packed], { type: "application/octet-stream" }), "save.bin");
  form.append("game", gameCode);
  form.append("label", replace?.label || baseName(romName).slice(0, 16));
  if (replace) form.append("replace", replace.id);

  try {
    const response = await fetch("/api/saves", { method: "POST", body: form });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error ?? `${response.status}`);
    say(
      `${replace ? "Overwrote" : "Saved to the vault"} · ` +
        `${formatSize(packed.byteLength)} (from ${formatSize(state.byteLength)})`,
      "good",
    );
    await refreshSaves();
    return true;
  } catch (error) {
    say(`Could not save: ${error.message}`, "bad");
    return false;
  } finally {
    el.cloud.disabled = false;
  }
}

/**
 * Show either the saves list or the line explaining why there is not one.
 *
 * The saves panel used to simply hide itself, which was fine when it was one
 * box in a stack. Inside a tab it cannot: a tab you can select and that then
 * shows nothing at all reads as broken, so the empty case has to say something.
 */
function showSaves(visible) {
  el.savesPanel.hidden = !visible;
  el.savesOff.hidden = visible;
  if (!visible) setSavesCount("");
}

/**
 * The line under the Saves tab.
 *
 * The count used to be an element inside a fixed panel. The tabs are generated
 * now, so it is state rather than a node, and setting it redraws the rail that
 * happens to be holding saves.
 */
function setSavesCount(text) {
  if (savesCount === text) return;
  savesCount = text;
  renderRails(lastSections);
}

// --- screenshots --------------------------------------------------------
//
// Taking one already worked; looking at one did not. The vault holds them as
// ordinary images, so listing them is a filter on the same call the library
// already makes, and the only new work is showing them at a size worth looking
// at — a 240x160 picture in a 60px thumbnail is a reminder that a screenshot
// exists rather than a way to see it.

/** The count drawn on the Shots tab. */
let shotsCount = "";

function setShotsCount(text) {
  if (shotsCount === text) return;
  shotsCount = text;
  renderRails(lastSections);
}

/**
 * The screenshots in the vault, newest first.
 *
 * Names carry the millisecond they were taken, which is the only ordering the
 * vault gives us — it lists by whatever order it likes.
 */
async function refreshShots() {
  let shots = [];
  try {
    // Scoped to this account by the server. Asking for the whole vault and
    // filtering here would make privacy a setting anyone could turn off in the
    // developer tools.
    const response = await fetch("/api/shots");
    if (response.status === 401) {
      accounts?.noticeSignedOut();
      el.shotsOff.textContent = "Sign in to keep screenshots.";
      showShots(false);
      setShotsCount("");
      return;
    }
    const body = await response.json();
    if (!body.configured) {
      el.shotsOff.textContent =
        "Screenshots need the vault. Set TINYBIRD_MEDIA_KEY in .env and restart.";
      showShots(false);
      return;
    }
    shots = body.shots ?? [];
  } catch {
    showShots(false);
    return;
  }

  if (shots.length === 0) {
    showShots(false);
    setShotsCount("");
    return;
  }

  showShots(true);
  setShotsCount(`${shots.length}`);
  el.shotsList.replaceChildren(...shots.map(renderShot));
}

function showShots(visible) {
  el.shotsList.hidden = !visible;
  el.shotsOff.hidden = visible;
}

function renderShot(shot) {
  const item = document.createElement("li");

  const open = document.createElement("button");
  open.type = "button";
  open.className = "shots__item";
  open.title = shot.game_code ? `${shot.game_code} screenshot` : "Screenshot";

  const image = document.createElement("img");
  image.className = "shots__thumb";
  image.src = shot.url;
  image.alt = "";
  image.loading = "lazy";
  open.append(image);

  if (shot.taken_at_ms) open.append(span("shots__when", formatWhen(shot.taken_at_ms)));

  open.addEventListener("click", () => showShot(shot, shot.taken_at_ms));
  item.append(open);
  return item;
}

function showShot(asset, when) {
  el.shotFull.src = asset.url;
  el.shotFull.alt = "";
  el.shotWhen.textContent = when ? formatWhen(when) : "";
  el.shotOpen.href = asset.url;
  el.shotViewer.showModal();
}

/**
 * Reload everything the vault keeps per account.
 *
 * Saves and screenshots are both scoped to whoever is signed in, so both have
 * to be re-asked for when that changes. Only saves were, which is why a
 * screenshot taken before a sign-out reappeared only after the next one was
 * taken — the list was right on the server and stale on the page.
 */
async function refreshVault() {
  await Promise.all([refreshSaves(), refreshShots()]);
}

/** Reload the saves list for the current cartridge. */
async function refreshSaves() {
  if (!gameCode) {
    showSaves(false);
    return;
  }

  let listing;
  try {
    const response = await fetch(`/api/saves?game=${encodeURIComponent(gameCode)}`);
    if (response.status === 401) {
      // Accounts are on and nobody is signed in. Say so where the saves would
      // be, rather than hiding the panel as though the vault were off.
      accounts?.noticeSignedOut();
      showSaves(true);
      setSavesCount("signed out");
      el.savesList.replaceChildren(savesNote("Sign in to keep saves in the vault."));
      return;
    }
    listing = await response.json();
  } catch {
    showSaves(false);
    return;
  }

  if (!listing.configured) {
    showSaves(false);
    el.savesOff.textContent = "The vault is not configured on this server.";
    return;
  }

  showSaves(true);

  if (listing.error) {
    setSavesCount("error");
    el.savesList.replaceChildren(savesNote(listing.error));
    return;
  }

  // "private" is worth showing: it is the difference between a save on a
  // signed, owner-scoped URL and one on a public one.
  const where = listing.backend === "private" ? "private" : "public";
  setSavesCount(`${where} \u00b7 ${listing.saves.length} / ${listing.limit}`);

  if (listing.saves.length === 0) {
    el.savesList.replaceChildren(
      savesNote("None yet. Save to vault keeps the last few here."),
    );
    return;
  }

  el.savesList.replaceChildren(...listing.saves.map(renderSaveRow));
}

function renderSaveRow(save) {
  const item = document.createElement("li");
  item.className = "saves__item";

  const shot = document.createElement("img");
  shot.className = "saves__shot";
  shot.alt = "";
  shot.width = 96;
  shot.height = 64;
  // Pull only the container prefix; the rest of the save stays on the CDN
  // until someone actually loads it.
  loadThumbnail(save, shot);

  const meta = document.createElement("div");
  meta.className = "saves__meta";

  // Drawn from what we already have, so cancelling a rename costs nothing and
  // a finished one is repainted by the refresh that follows it.
  const paintMeta = () => {
    const name = span("saves__name", save.label || "Unnamed");
    if (!save.label) name.dataset.empty = "on";

    // Renaming belongs beside the name, not in a row of verbs: it is the one
    // action here that acts on a word rather than on the save.
    const pencil = document.createElement("button");
    pencil.type = "button";
    pencil.className = "saves__pencil";
    pencil.textContent = "✎";
    pencil.title = "Rename this save";
    pencil.setAttribute("aria-label", "Rename this save");
    pencil.addEventListener("click", () => beginRename(save, meta, paintMeta));

    const line = document.createElement("div");
    line.className = "saves__nameline";
    line.append(name, pencil);

    const when = span("saves__when", formatWhenShort(save.saved_at_ms));
    // The parts that got cut: the full date, and a size that is the same 220KB
    // on every row and so tells you nothing by being on all of them.
    when.title = `${formatWhen(save.saved_at_ms)} · ${formatSize(save.size)}`;

    meta.replaceChildren(line, when);
  };
  paintMeta();

  const actions = document.createElement("div");
  actions.className = "saves__actions";

  // Loading throws away the run in progress, and a button that says "load"
  // does not say that. So it arms first, like the two that destroy the save
  // itself: what is at risk is different, the misclick is the same.
  const load = arming({
    label: "load",
    armed: "confirm?",
    className: "saves__act saves__act--go",
    title: "Load this save, discarding the game in progress",
    run: () => loadSaveFromVault(save),
  });

  // Overwriting throws away what is in the slot, so it arms first, exactly like
  // delete.
  const overwrite = arming({
    label: "overwrite",
    armed: "replace?",
    className: "saves__act saves__act--danger",
    title: "Replace this save with the game as it is now",
    run: () => storeSave(save),
  });

  const fetchToDisk = document.createElement("button");
  fetchToDisk.type = "button";
  fetchToDisk.className = "saves__act";
  fetchToDisk.textContent = "download";
  fetchToDisk.title = "Download this save as a .state file";
  fetchToDisk.addEventListener("click", () => downloadSave(save, fetchToDisk));

  const remove = arming({
    label: "delete",
    armed: "sure?",
    className: "saves__act saves__act--danger",
    title: "Delete this save",
    run: () => deleteSave(save),
  });

  actions.append(load, overwrite, fetchToDisk, remove);
  item.append(shot, meta, actions);
  return item;
}

/** Longest name the vault keeps. The rest would be trimmed off the filename. */
const SAVE_LABEL_MAX = 24;

/**
 * Rename a save, in the row it sits in.
 *
 * The name lives in the stored filename, so what a name may contain is what a
 * filename may contain: letters, numbers and hyphens. Trimming as it is typed
 * says that once and immediately, rather than letting someone finish a name
 * the server then quietly reduces to something else.
 */
function beginRename(save, meta, paintMeta) {
  const form = document.createElement("form");
  form.className = "saves__rename";

  const input = document.createElement("input");
  input.className = "saves__name-input";
  input.value = save.label ?? "";
  input.maxLength = SAVE_LABEL_MAX;
  input.placeholder = "letters, numbers, hyphens";
  input.setAttribute("aria-label", "Save name");
  input.addEventListener("input", () => {
    const clean = input.value.replace(/[^A-Za-z0-9-]/g, "").slice(0, SAVE_LABEL_MAX);
    if (clean !== input.value) input.value = clean;
  });

  const keep = document.createElement("button");
  keep.type = "submit";
  keep.className = "saves__act saves__act--go";
  keep.textContent = "save";

  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "saves__act";
  cancel.textContent = "cancel";
  cancel.addEventListener("click", paintMeta);

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    keep.disabled = true;
    renameSave(save, input.value);
  });
  // Escape belongs to the field while the field is open. Without this it
  // reaches the page's own handler and leaves focus mode instead.
  form.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    paintMeta();
  });

  form.append(input, keep, cancel);
  meta.replaceChildren(form);
  input.focus();
  input.select();
}

async function renameSave(save, label) {
  if (label === (save.label ?? "")) {
    await refreshSaves();
    return;
  }

  say("Renaming…");
  try {
    // The name is part of the object's key in the vault, so the server has to
    // restore it under a new one; there is no field to edit in place.
    const response = await fetch(`/api/saves/${encodeURIComponent(save.id)}`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ label }),
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error ?? `${response.status}`);
    say(label ? `Renamed to ${label}` : "Name cleared", "good");
  } catch (error) {
    say(`Could not rename: ${error.message}`, "bad");
  }
  await refreshSaves();
}

/**
 * A button that has to be clicked twice.
 *
 * A stored save is the only copy of that progress and there is no undo, so
 * every action that destroys one asks first. The armed state lapses on its own
 * rather than staying hot, so a forgotten click cannot be completed later by
 * someone aiming at the button next to it.
 */
function arming({ label, armed, className, title, run }) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = className;
  button.textContent = label;
  button.title = title;
  // The armed state is a colour and a changed word. Neither reaches a screen
  // reader on its own, and "click again" is the whole contract of the control.
  button.setAttribute("aria-label", title);

  let timer = 0;
  const disarm = () => {
    clearTimeout(timer);
    button.dataset.confirm = "false";
    button.textContent = label;
    button.title = title;
    button.setAttribute("aria-label", title);
  };

  button.addEventListener("click", async () => {
    if (button.dataset.confirm !== "true") {
      button.dataset.confirm = "true";
      button.textContent = armed;
      button.title = `${title} - click again to confirm`;
      button.setAttribute("aria-label", button.title);
      timer = setTimeout(disarm, 4000);
      return;
    }
    disarm();
    await run();
  });
  return button;
}

/**
 * Save a stored slot to disk as a plain `.state`.
 *
 * Unpacked on the way out rather than handed over as the container: the desktop
 * build reads save states, not this page's wrapper, and a file you cannot open
 * in the other half of the project is not much of a download.
 */
async function downloadSave(save, button) {
  button.disabled = true;
  const previous = button.textContent;
  button.textContent = "…";
  try {
    const response = await fetchStorage(save.url);
    if (!response.ok) throw new Error(`${response.status}`);

    const name = `${baseName(save.original_name)}.state`;
    download(await unpackSave(await response.arrayBuffer()), name);
    say(`Downloaded ${name}`, "good");
  } catch (error) {
    say(`Could not download that save: ${error.message}`, "bad");
  } finally {
    button.textContent = previous;
    button.disabled = false;
  }
}

/**
 * Fill a save row's thumbnail using a ranged request.
 *
 * Purely decorative, so every failure path just leaves the placeholder: a
 * missing screenshot must never stop a save from being loadable.
 */
async function loadThumbnail(save, image) {
  try {
    const response = await fetch(save.url, {
      headers: { Range: `bytes=0-${THUMBNAIL_PREFIX_BYTES - 1}` },
    });
    if (!response.ok) return;

    const thumbnail = readThumbnail(await response.arrayBuffer());
    if (!thumbnail) return;

    const url = URL.createObjectURL(new Blob([thumbnail], { type: "image/png" }));
    image.addEventListener("load", () => URL.revokeObjectURL(url), { once: true });
    image.src = url;
    image.classList.add("is-loaded");
  } catch {
    // No thumbnail; the row still works.
  }
}

async function loadSaveFromVault(save) {
  // The vault list is filed per cartridge, so a slot shown here belongs to the
  // game that was loaded when it was shown. That is not the same as a game
  // being loaded *now* — the list survives an eject.
  if (!requireCartridge("load a save state")) return;
  say("Fetching save…");
  try {
    const response = await fetchStorage(save.url);
    if (!response.ok) throw new Error(`${response.status}`);
    const buffer = await response.arrayBuffer();
    emu.loadState(await unpackSave(buffer));
    // The cartridge save goes back too. Restoring the machine without it
    // leaves the game believing in a save file that is no longer there.
    applyBattery(readBattery(buffer), gameCode);
    running = true;
    el.play.textContent = "Pause";
    present();
    say(`Loaded save from ${formatWhen(save.saved_at_ms)}`, "good");
  } catch (error) {
    say(`Could not load that save: ${error.message}`, "bad");
  }
}

async function deleteSave(save) {
  try {
    const response = await fetch(`/api/saves/${encodeURIComponent(save.id)}`, {
      method: "DELETE",
    });
    if (!response.ok) {
      const body = await response.json().catch(() => ({}));
      throw new Error(body.error ?? `${response.status}`);
    }
    say("Save deleted");
    await refreshSaves();
  } catch (error) {
    say(`Could not delete: ${error.message}`, "bad");
  }
}

function savesNote(text) {
  const item = document.createElement("li");
  item.className = "saves__empty";
  item.textContent = text;
  return item;
}

/** A short, local, human timestamp. */
/**
 * The same moment as `formatWhen`, in as few characters as still tell two
 * saves apart.
 *
 * A save row has about 170px beside its thumbnail, and "8/27/2026 01:05 AM"
 * wrapped onto a second line there — spending most of a line on a year that is
 * nearly always this one. Today keeps only the time, this year keeps the day
 * and the month, and only a save old enough for the year to matter pays for
 * it. The full date and the size stay in the row's tooltip.
 */
function formatWhenShort(ms) {
  const when = new Date(ms);
  if (Number.isNaN(when.getTime())) return "unknown";
  const now = new Date();
  const time = when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (when.toDateString() === now.toDateString()) return `Today ${time}`;

  const day = when.toLocaleDateString([], { day: "numeric", month: "short" });
  return when.getFullYear() === now.getFullYear()
    ? `${day} ${time}`
    : `${day} ${when.getFullYear()}`;
}

function formatWhen(ms) {
  const when = new Date(ms);
  if (Number.isNaN(when.getTime())) return "unknown";
  const today = new Date();
  const sameDay = when.toDateString() === today.toDateString();
  const time = when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return sameDay ? `Today ${time}` : `${when.toLocaleDateString()} ${time}`;
}

// --- embedded overlay ---------------------------------------------------

let overlayReady = false;

/**
 * Send the current snapshot to the embedded overlay.
 *
 * The overlay page normally polls the desktop app's export. Here the emulator
 * is in this very page, so we hand it the same schema directly — one renderer,
 * two sources, no chance of the two drifting apart.
 */
function pushToOverlay(snapshot) {
  if (!overlayReady || !snapshot) return;
  el.overlayFrame.contentWindow?.postMessage(
    { type: "tinybird:snapshot", snapshot, status: romName || "live" },
    window.location.origin,
  );
  fitOverlay();
}

/**
 * Grow the frame to fit the overlay.
 *
 * A fixed height clips it — the party cards sit below the area panel and the
 * whole thing changes height as a battle starts or ends. Same-origin, so the
 * content height is readable directly.
 */
function fitOverlay() {
  const body = el.overlayFrame.contentDocument?.body;
  if (!body) return;
  const height = Math.max(body.scrollHeight, 220);
  const next = `${height}px`;
  if (el.overlayFrame.style.height !== next) el.overlayFrame.style.height = next;
}

function overlayUrl() {
  const layout = el.optOverlayRow.checked ? "row" : "column";
  // transparent=1 so it sits on the console background rather than its own.
  return `/overlay/full?transparent=1&layout=${layout}&showEmpty=1`;
}

function setOverlayVisible(visible) {
  el.overlayStage.hidden = !visible;
  if (!visible) {
    overlayReady = false;
    el.overlayFrame.src = "about:blank";
    return;
  }
  el.overlayFrame.src = overlayUrl();
}

// The overlay tells us when it is ready; until then postMessage would be lost.
window.addEventListener("message", (event) => {
  if (event.origin !== window.location.origin) return;
  if (event.data?.type !== "tinybird:overlay-ready") return;
  overlayReady = true;
  if (emu) pushToOverlay(emu.snapshot());
  // The first layout settles a frame or two after the content arrives.
  setTimeout(fitOverlay, 120);
  setTimeout(fitOverlay, 600);
});

el.optOverlay.addEventListener("change", () => {
  setOverlayVisible(el.optOverlay.checked);
});

el.optOverlayRow.addEventListener("change", () => {
  if (!el.optOverlay.checked) return;
  overlayReady = false;
  el.overlayFrame.src = overlayUrl();
});

// --- BIOS ---------------------------------------------------------------

/**
 * Fetch and install the BIOS image, if the server has one.
 *
 * Returns false when there is none; the emulator still runs, but games that
 * lean on BIOS decompression will look wrong.
 */
async function loadBios() {
  try {
    const response = await fetch("/bios");
    if (!response.ok) return false;
    // Kept, because a lockstep session builds a second console in this browser
    // and it has to be given the same BIOS as the first. Two consoles running
    // different BIOS images agree on nothing for long.
    biosBytes = new Uint8Array(await response.arrayBuffer());
    emu.loadBios(biosBytes);
    return true;
  } catch {
    return false;
  }
}

// --- vault --------------------------------------------------------------

function markVaultSelection(name) {
  for (const button of el.vaultList.querySelectorAll(".vault__item")) {
    button.setAttribute("aria-current", String(button.dataset.name === name));
  }
}

/**
 * Fill the library from both sources.
 *
 * The vault needs a key and gives shareable CDN links; the local folder needs
 * nothing and works offline. Showing them in one list, tagged, means the page
 * is useful before any storage is configured.
 */
async function loadLibrary() {
  const [vault, local] = await Promise.all([fetchVault(), fetchLocal()]);
  const entries = [...vault.assets, ...local];

  el.vaultNote.textContent = vault.note;

  if (entries.length === 0) {
    el.vaultList.replaceChildren(
      note(
        vault.configured
          ? "No ROMs found. Drop a .gba file to play one."
          : "No ROMs in roms/. Drop a .gba file, or set TINYBIRD_MEDIA_KEY in .env to use your vault.",
      ),
    );
    return;
  }

  el.vaultList.replaceChildren(...entries.map(renderLibraryRow));
}

async function fetchVault() {
  try {
    const response = await fetch("/api/library");
    const body = await response.json();
    if (!body.configured) return { assets: [], configured: false, note: "local only" };
    if (body.error) return { assets: [], configured: true, note: "error" };
    return {
      configured: true,
      note: body.vault,
      assets: body.assets
        .filter((asset) => /\.(gba|bin|agb)$/i.test(asset.name))
        .map((asset) => ({ ...asset, source: "vault" })),
    };
  } catch {
    return { assets: [], configured: false, note: "unreachable" };
  }
}

async function fetchLocal() {
  try {
    const response = await fetch("/api/local");
    const body = await response.json();
    return body.assets ?? [];
  } catch {
    return [];
  }
}

function renderLibraryRow(asset) {
  const item = document.createElement("li");
  const button = document.createElement("button");
  button.type = "button";
  button.className = "vault__item";
  button.dataset.name = asset.name;
  button.dataset.source = asset.source;
  button.append(span("vault__tag", asset.source));
  button.append(span("vault__name", asset.name));
  if (asset.size) button.append(span("vault__size", formatSize(asset.size)));
  button.addEventListener("click", () => loadFromUrl(asset.url, asset.name));
  item.append(button);
  return item;
}

function note(text) {
  const item = document.createElement("li");
  item.className = "vault__empty";
  item.textContent = text;
  return item;
}

// --- save states --------------------------------------------------------

el.save.addEventListener("click", () => {
  try {
    const state = emu.saveState();
    download(state, `${baseName(romName)}.state`);
    say(`Saved ${formatSize(state.byteLength)} to your downloads`, "good");
  } catch (error) {
    say(error.message, "bad");
  }
});

// The media host accepts images, audio, and video, and sniffs the contents to
// confirm the declared type — so a save state cannot go there, but a
// screenshot is exactly what it is for. States stay local downloads.
el.store.addEventListener("click", async () => {
  const blob = await new Promise((resolve) =>
    frameCanvas.toBlob(resolve, "image/png"),
  );
  if (!blob) {
    say("The screen could not be captured.", "bad");
    return;
  }

  say(`Uploading ${formatSize(blob.size)}…`);

  // `/api/shots` rather than the general vault upload: the server names the
  // file, and the owner is part of that name. A name the browser chose would
  // let anyone file a picture under someone else's account, or read theirs.
  const form = new FormData();
  form.append("game", gameCode ?? "");
  form.append("file", blob, "screenshot.png");

  try {
    const response = await fetch("/api/shots", { method: "POST", body: form });
    const body = await response.json();
    if (!response.ok) throw new Error(body.error ?? `${response.status}`);
    say("Screenshot saved", "good");
    // Straight into the gallery, so the picture is there when you look.
    refreshShots();
    lastUploadUrl = body.url ?? null;
  } catch (error) {
    say(`Screenshot failed: ${error.message}`, "bad");
  }
});

let lastUploadUrl = null;

el.fileState.addEventListener("change", async (event) => {
  const [file] = event.target.files;
  if (!file) return;
  if (!requireCartridge("load a save state")) {
    event.target.value = "";
    return;
  }
  try {
    // A file downloaded from the vault is a container; a local one is raw.
    emu.loadState(await unpackSave(await file.arrayBuffer()));
    running = true;
    present();
    say(`Loaded ${file.name}`, "good");
  } catch (error) {
    say(error.message, "bad");
  }
  event.target.value = "";
});

el.fileRom.addEventListener("change", async (event) => {
  const [file] = event.target.files;
  if (file) await loadFromFile(file);
  event.target.value = "";
});

function baseName(name) {
  return (name || "tinybird").replace(/\.[^.]+$/, "").replace(/[^\w.-]+/g, "_");
}

function download(bytes, name) {
  const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

// --- transport ----------------------------------------------------------

el.play.addEventListener("click", () => {
  running = !running;
  emu.setPaused(!running);
  el.play.textContent = running ? "Pause" : "Resume";
  setLink(running ? "live" : "idle", running ? romName : "paused");
});

el.reset.addEventListener("click", () => {
  emu.reset();
  running = true;
  el.play.textContent = "Pause";
  say("Reset");
});

el.optLcd.addEventListener("change", () => {
  if (emu) emu.setColorCorrection(el.optLcd.checked);
});

el.optScan.addEventListener("change", () => {
  el.screen.dataset.grid = el.optScan.checked ? "on" : "off";
});
el.screen.dataset.grid = el.optScan.checked ? "on" : "off";

el.optSpeed.addEventListener("change", () => {
  fastForwardSpeed = Number(el.optSpeed.value);
  remember("speed", el.optSpeed.value);
  if (fastForward) frameClock = 0;
});

el.optVolume.addEventListener("input", () => {
  const volume = Number(el.optVolume.value) / 100;
  if (audio) audio.setVolume(volume);
  remember("volume", el.optVolume.value);
});

el.optAudio.addEventListener("change", async () => {
  if (!el.optAudio.checked) {
    if (audio) audio.close();
    audio = null;
    return;
  }
  audio = new AudioSink(emu ? emu.sampleRate : 32768);
  audio.setVolume(Number(el.optVolume.value) / 100);
  // Browsers only allow this from a user gesture, which the change event is.
  if (!(await audio.resume())) {
    say("The browser blocked audio. Click the page and try again.", "bad");
    el.optAudio.checked = false;
    audio = null;
  }
});

// --- drag and drop ------------------------------------------------------

let dragDepth = 0;

window.addEventListener("dragenter", (event) => {
  event.preventDefault();
  dragDepth += 1;
  el.drop.hidden = false;
});

window.addEventListener("dragover", (event) => event.preventDefault());

window.addEventListener("dragleave", () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) el.drop.hidden = true;
});

window.addEventListener("drop", async (event) => {
  event.preventDefault();
  dragDepth = 0;
  el.drop.hidden = true;

  const [file] = event.dataTransfer.files;
  if (!file) return;
  if (/\.(state|savestate)$/i.test(file.name)) {
    if (!requireCartridge("load a save state")) return;
    try {
      emu.loadState(await unpackSave(await file.arrayBuffer()));
      running = true;
      present();
      say(`Loaded ${file.name}`, "good");
    } catch (error) {
      say(error.message, "bad");
    }
    return;
  }
  await loadFromFile(file);
});

// --- boot ---------------------------------------------------------------

/**
 * Addon manifests, fetched before anything reads a snapshot.
 *
 * The browser has no filesystem, so the server hands them over as one array.
 * Failing is not worth interrupting a boot for: the compiled addons are still
 * there, and a page with no manifests is how this worked until recently.
 */
/** How many manifest addons loaded at boot, for the devtools handle. */
let manifestsInstalled = 0;

async function installManifests() {
  try {
    const response = await fetch("/api/addons");
    if (!response.ok) return;
    const manifests = await response.json();
    const installed = emu.installManifests(manifests);
    manifestsInstalled = installed;
    if (installed > 0) {
      say(`Loaded ${installed} addon manifest${installed === 1 ? "" : "s"}`);
    }
  } catch {
    // No manifests, which is the ordinary case.
  }
}

async function boot() {
  try {
    emu = await TinyBird.load();
    // Before the first snapshot: the registry is built on first read.
    await installManifests();
    // A handle for devtools. Everything the page does goes through these two
    // objects, so a session can be inspected — or a register read out while a
    // link misbehaves — without instrumenting the page to find out.
    window.tinybird = {
      get emu() {
        return emu;
      },
      get lobby() {
        return lobby;
      },
      /** Addon manifests installed at boot, beyond the compiled addons. */
      get manifests() {
        return manifestsInstalled;
      },
      /** What the emulator is being told to hold, and by what. */
      get buttons() {
        return buttons;
      },
      get input() {
        return {
          keys: [...heldKeys],
          pad: [...heldPad],
          padConnected,
          bindings: controls.bindings,
        };
      },
      get link() {
        return {
          ...linkTally,
          frame: linkFrame,
          granted: grantedFrame,
          phase: linkPhase,
          seat: mySeat(),
          cable: cableShape,
          sending: emu ? emu.linkSendValue.toString(16).padStart(4, "0") : null,
          recent: [...linkRecent],
        };
      },
    };
  } catch (error) {
    setLink("error", "emulator unavailable");
    say(
      error instanceof EmulatorError
        ? error.message
        : "The emulator module could not be loaded.",
      "bad",
    );
    el.vaultList.replaceChildren(note("Build the emulator module to play here."));
    return;
  }

  fitScreen();
  restorePreferences();

  // Load the BIOS before any ROM. Games decompress graphics through BIOS SWI
  // calls, and the core's high-level stand-ins are not accurate enough for all
  // of them — without this, FireRed renders with missing sprites and corrupt
  // tiles while running at full speed, which looks like a graphics bug.
  const biosLoaded = await loadBios();

  setLink("idle", "ready");
  say(
    biosLoaded
      ? "Pick a ROM to start."
      : "No BIOS found — some games will render incorrectly. Pick a ROM to start.",
    biosLoaded ? "" : "bad",
  );

  // A ?rom= link makes a vault asset shareable: open the URL and it boots.
  const requested = new URLSearchParams(location.search).get("rom");
  requestAnimationFrame((now) => {
    fpsWindowStart = now;
    tick(now);
  });

  // Before the library, because a ?rom= link boots straight into a game and
  // that immediately asks for its saves. Mounted here rather than at load, so
  // the first answer arrives in step with the rest of startup.
  accounts = mountAccount({ onChange: onAccountChange });
  accounts?.extras.append(claimButton);
  await accounts?.refresh();
  await loadLibrary();

  if (requested) {
    const label =
      new URLSearchParams(location.search).get("name") ??
      decodeURIComponent(requested.split("/").pop() ?? "cartridge");
    await loadFromUrl(requested, label);
  }
}

boot();

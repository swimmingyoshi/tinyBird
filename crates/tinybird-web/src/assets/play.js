// Wiring for the play page: input, the frame loop, the live read-out, and the
// local ROM library. Everything that touches WebAssembly memory lives in
// tinybird.js.

import { AudioSink, EmulatorError, TinyBird } from "/tinybird.js";
import { RECOVERY_INTERVAL, RECOVERY_OWNER, observationKey, fingerprint, readRecovery, writeRecovery, validateRecovery } from '/recovery.js';
import { validateObservationConfig, matchObservationGame } from '/workshop-observations.js';
if (window.parent !== window && new URLSearchParams(location.search).get('embed') === 'workshop') {
  document.body.dataset.workshopPlayer = 'true';
  const style = document.createElement('link'); style.rel = 'stylesheet'; style.href = '/workshop-player.css'; document.head.append(style);
}
import { localAddons, enabledManifests, disabledBuiltins, saveDisabledBuiltins, saveLocalAddons, hiddenPanels, saveHiddenPanels } from "/addon-client.js";
import { ACTIONS, Controls, keyLabel, padLabel } from "/controls.js";
import { unpackSave } from "/saveformat.js";
import { GBA_FRAME_HZ, schedule } from "/pacing.js";
import {
  BACKGROUND_LIMIT,
  THEMES,
  backgroundSettings,
  clearBackground,
  currentTheme,
  mountTheme,
  paintBackground,
  readoutsHidden,
  saveBackground,
  setBackgroundSettings,
  setReadoutsHidden,
  setSurfaceSettings,
  setTheme,
  surfaceSettings,
} from "/theme.js";

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
  optFrameMode: $("opt-frame-mode"),
  optFastAudio: $("opt-fast-audio"),
  optVolume: $("opt-volume"),
  optLcd: $("opt-lcd"),
  optScan: $("opt-scan"),
  panel: $("screen-panel"),
  optOverlay: $("opt-overlay"),
  optOverlayRow: $("opt-overlay-row"),
  overlayStage: $("overlay-stage"),
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
  leftPane: $("left-pane"),
  rightPane: $("right-pane"),
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
  // Centring the bar caps how wide this can be, so a long ROM name ends in an
  // ellipsis. Keep the whole of it a hover away.
  el.linkState.title = label;
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
  if (capturing) return;
  const interactive = event.target instanceof Element && event.target.closest(
    "button, input, select, textarea, a, label, summary, [role=button], [contenteditable=true]",
  );
  const inDialog = document.querySelector("dialog[open], .game-menu:popover-open");
  // Space and Enter activate the focused UI control, not the game. Always
  // release keys that began in the game even if focus moved before keyup.
  if (down && (interactive || inDialog)) return;
  const action = controls.actionForKey(event.code);
  if (action === null || (!down && !heldKeys.has(action))) return;
  if (!interactive && !inDialog) event.preventDefault();
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

// --- play views ---------------------------------------------------------
//
// Tab cycles Desk, Focus (game and read-outs), and Cinema (games only).
//
// Taking Tab is not free: it is how a keyboard moves between controls, and a
// page that swallows it everywhere is a page that cannot be used without a
// mouse. So it is only taken when the key would otherwise do nothing useful —
// nothing focused, no dialog open — and never when someone has bound it to the
// game themselves. Tabbing through the deck still works.

const playViews = ["desk", "focus", "cinema"];

function setPlayView(view, announce = true) {
  if (!playViews.includes(view)) view = "desk";
  el.rig.dataset.playView = view;
  el.rig.dataset.focus = view === "desk" ? "off" : "on";
  remember("play-view", view);
  document.querySelectorAll("[data-play-view-button]").forEach(button => {
    button.setAttribute("aria-pressed", String(button.dataset.playViewButton === view));
  });
  fitScreen();
  if (announce) say(`${view[0].toUpperCase() + view.slice(1)} view. Tab switches views.`);
}

for (const button of document.querySelectorAll("[data-play-view-button]")) {
  button.addEventListener("click", () => setPlayView(button.dataset.playViewButton));
}

// All command panels use one anchored, non-modal popover at a time.
const gameMenu = $("game-menu");
const gameMenuButton = $("btn-game-menu");
const popupAnchors = new Map([[gameMenu, gameMenuButton]]);
const popupTriggers = [...document.querySelectorAll('[data-popup], [popovertarget="game-menu"]')];
const popupControls = 'button:not(:disabled), a[href], input:not([hidden]):not(:disabled), select:not(:disabled), label[tabindex]:not([data-disabled="true"])';
function positionGameMenu() {
  for (const popup of document.querySelectorAll('.game-menu:popover-open')) {
    const anchor = (popupAnchors.get(popup) ?? gameMenuButton).getBoundingClientRect();
    const bounds = popup.getBoundingClientRect();
    popup.style.left = `${Math.max(12, Math.min(anchor.right - bounds.width, innerWidth - bounds.width - 12))}px`;
    const top = anchor.top >= bounds.height + 20 ? anchor.top - bounds.height - 8 : anchor.bottom + 8;
    popup.style.top = `${Math.max(12, Math.min(top, innerHeight - bounds.height - 12))}px`;
  }
}
function closeToolSheets() {
  for (const popup of document.querySelectorAll('.game-menu:popover-open')) popup.hidePopover();
}
function openToolPopup(popup, trigger) {
  const anchor = trigger.closest('.game-menu') ? gameMenuButton : trigger;
  const wasOpen = popup.matches(':popover-open');
  closeToolSheets();
  if (wasOpen) return;
  popupAnchors.set(popup, anchor);
  popup.showPopover();
  positionGameMenu();
}
for (const trigger of document.querySelectorAll('[data-popup]')) {
  trigger.addEventListener('click', () => openToolPopup($(trigger.dataset.popup), trigger));
}
for (const popup of document.querySelectorAll('.game-menu')) {
  popup.addEventListener('beforetoggle', event => {
    for (const trigger of popupTriggers.filter(button => (button.dataset.popup ?? button.getAttribute('popovertarget')) === popup.id)) {
      trigger.setAttribute('aria-expanded', String(event.newState === 'open'));
    }
    const anchor = popupAnchors.get(popup);
    if (anchor?.getAttribute('aria-controls') === popup.id) anchor.setAttribute('aria-expanded', String(event.newState === 'open'));
  });
  popup.addEventListener('toggle', () => {
    if (!popup.matches(':popover-open')) return;
    positionGameMenu();
    const controls = [...popup.querySelectorAll(popupControls)].filter(item => item.getClientRects().length);
    (controls.find(item => !item.matches('[data-close-popup]')) ?? controls[0])?.focus({ preventScroll: true });
    if (popup.id === 'addon-menu') renderPlayAddons();
  });
  popup.addEventListener('click', event => {
    const close = event.target.closest('[data-close-popup]');
    if (close) {
      popup.hidePopover();
      (popupAnchors.get(popup) ?? gameMenuButton).focus({ preventScroll: true });
    }
    const action = event.target.closest('.popup-actions button, .popup-actions label');
    if (action && !action.disabled && action.dataset.disabled !== 'true') popup.hidePopover();
  });
  popup.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation();
      popup.hidePopover();
      (popupAnchors.get(popup) ?? gameMenuButton).focus({ preventScroll: true });
      return;
    }
    // Let native sliders, selects, and text fields keep their own arrow keys.
    if (event.target.matches('input, select, textarea') || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const items = [...popup.querySelectorAll(popupControls)].filter(item => item.getClientRects().length);
    const index = items.indexOf(document.activeElement);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
      : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  });
  new ResizeObserver(positionGameMenu).observe(popup);
}
// Close the root menu before opening the retained Controls dialog/fullscreen.
gameMenu.addEventListener('click', event => {
  if (event.target.closest('button:not([data-popup]), a')) gameMenu.hidePopover();
}, true);
window.addEventListener('resize', positionGameMenu);
document.addEventListener('scroll', positionGameMenu, true);

// Native file inputs stay visually hidden; their labels expose keyboard access.
for (const [label, input] of [[el.loadState, el.fileState], [$("vault-load"), el.fileRom]]) {
  label.tabIndex = 0;
  label.setAttribute("role", "button");
  label.addEventListener("keydown", event => {
    if (!["Enter", " "].includes(event.key)) return;
    event.preventDefault();
    if (!input.disabled) input.click();
  });
}
el.canvas.tabIndex = 0;
el.canvas.addEventListener("pointerdown", () => el.canvas.focus({ preventScroll: true }));

function focusModeWantsTab(event) {
  if (event.code !== "Tab" || event.altKey || event.ctrlKey || event.metaKey) return false;
  // Someone who bound Tab to the game meant it for the game.
  if (controls.actionForKey("Tab") !== null) return false;
  if (document.querySelector("dialog[open], .game-menu:popover-open")) return false;

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
    if (event.repeat) return;
    const index = playViews.indexOf(el.rig.dataset.playView);
    setPlayView(playViews[(index + (event.shiftKey ? 2 : 1)) % playViews.length]);
  },
  true,
);

// Escape leaves as well, because it is what everyone tries first.
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && el.rig.dataset.playView !== "desk"
      && !document.querySelector("dialog[open], .game-menu:popover-open")) setPlayView("desk");
});

setPlayView(recall("play-view") ?? (recall("focus") === "on" ? "focus" : "desk"), false);

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

  // The page's own key rather than a binding, so it is named only when the
  // page actually gets it — the same test the handler makes before taking it.
  if (controls.actionForKey("Tab") === null) parts.push(["Tab", "switch view"]);

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
/**
 * How tall the picture may be in Desk and Focus, measured rather than guessed.
 *
 * The cap was `100dvh - 400px`, and the 400 stood in for the bar, the status
 * line, the transport and the hint. Being a constant it was right at one window
 * height and wrong at every other: on a tall one it left a band of dead space
 * under the hint, and a short one would have run the transport off the bottom.
 * Every one of those pieces is on the page and can be asked its height, so add
 * them up instead. Cinema does its own sum a few lines below.
 */
function fitDeckCap() {
  const rig = document.querySelector('#rig');
  if (rig.dataset.playView === 'cinema') return;
  const column = el.screen.closest('.screen-col');
  const screens = column?.querySelector('.screens');
  if (!screens) return;

  const gap = parseFloat(getComputedStyle(column).gap) || 0;
  const bottom = parseFloat(getComputedStyle(rig).paddingBottom) || 0;

  // Only what sits below the picture in its own column. Whatever is above it
  // is already paid for by where the picture starts.
  let below = 0;
  let seen = false;
  for (const child of column.children) {
    if (child === screens) {
      seen = true;
      continue;
    }
    // offsetParent is null for the hidden helpers in here, which take no room.
    if (!seen || child.offsetParent === null) continue;
    below += child.getBoundingClientRect().height + gap;
  }

  // The last 2px are the bezel's own border, the same crumb cinema allows for.
  const available = innerHeight - (screens.getBoundingClientRect().top + scrollY)
    - below - bottom - 2;
  const cap = `${Math.floor(Math.max(260, available))}px`;
  // Only on a change: this runs from a ResizeObserver that the write itself
  // would otherwise wake again.
  if (rig.style.getPropertyValue('--deck-cap') !== cap) {
    rig.style.setProperty('--deck-cap', cap);
  }

  // How far the side rails may reach: the top of the deck down to the underside
  // of the button bar. Taken from the bar rather than from the column, because
  // the column also holds the key hints, and a rail level with those is already
  // past the bar. The rails cannot affect either measurement, so this settles
  // rather than chasing itself.
  const transport = column.querySelector('.transport');
  if (transport) {
    const reach = transport.getBoundingClientRect().bottom
      - column.getBoundingClientRect().top;
    const railCap = `${Math.floor(Math.max(160, reach))}px`;
    if (rig.style.getPropertyValue('--rail-cap') !== railCap) {
      rig.style.setProperty('--rail-cap', railCap);
    }
  }
}

function fitScreen() {
  fitDeckCap();
  if (document.querySelector('#rig').dataset.playView === 'cinema' && el.screen.dataset.mode === 'running') {
    const column = el.screen.closest('.screen-col');
    const screens = column.querySelector('.screens');
    const transport = column.querySelector('.transport');
    const rig = document.querySelector('#rig');
    const gap = parseFloat(getComputedStyle(column).gap) || 0;
    const bottom = parseFloat(getComputedStyle(rig).paddingBottom) || 0;
    const shared = screens.dataset.view === 'shared';
    const stacked = shared && getComputedStyle(screens).gridTemplateColumns.split(' ').length === 1;
    const available = innerHeight - (screens.getBoundingClientRect().top + scrollY)
      - transport.getBoundingClientRect().height - gap - bottom - (shared ? 32 : 0) - 2;
    const width = Math.max(120, available / (stacked ? 2 : 1) * (shared && !stacked ? 3 : 1.5));
    rig.style.setProperty('--cinema-width', `${Math.floor(width)}px`);
  }
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
resizeObserver.observe(document.querySelector('.transport'));
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
const quickMute = $('quick-mute');
const quickVolume = $('quick-volume');
const quickFullscreen = $('quick-fullscreen');
const quickScreenshot = $('quick-screenshot');
function syncQuickAudio() {
  quickMute.setAttribute('aria-pressed', String(!el.optAudio.checked));
  quickMute.title = el.optAudio.checked ? 'Mute' : 'Unmute';
  quickVolume.value = el.optVolume.value;
  quickVolume.title = `Volume: ${el.optVolume.value}%`;
}
quickMute.addEventListener('click', () => {
  el.optAudio.checked = !el.optAudio.checked;
  el.optAudio.dispatchEvent(new Event('change', { bubbles: true }));
});
quickVolume.addEventListener('input', () => {
  el.optVolume.value = quickVolume.value;
  el.optVolume.dispatchEvent(new Event('input', { bubbles: true }));
});
quickScreenshot.addEventListener('click', () => el.store.click());
quickFullscreen.addEventListener('click', toggleFullscreen);
syncQuickAudio();

// Hovering the picture is not the same as wanting the three widgets that sit
// on top of it. The pointer usually comes to rest on the screen after the last
// click and stays there, and hover alone leaves the buttons parked over the
// game for as long as it does. So a pointer that has stopped moving counts as
// gone and they fade out; any movement brings them straight back.
const QUICK_IDLE_MS = 2000;
let quickIdle = 0;

function wakeQuickControls() {
  clearTimeout(quickIdle);
  delete el.screen.dataset.quick;
  quickIdle = setTimeout(() => {
    // A pointer resting on a control is aiming at it, not abandoning it.
    // Keyboard focus is held by the stylesheet, which keeps a focused control
    // lit whatever this says.
    if (el.screen.querySelector('.screen-quick:hover')) {
      wakeQuickControls();
      return;
    }
    el.screen.dataset.quick = 'idle';
  }, QUICK_IDLE_MS);
}

el.screen.addEventListener('pointermove', wakeQuickControls);
// Off the picture the hover rule hides them anyway; drop the timer so coming
// back does not land mid-countdown.
el.screen.addEventListener('pointerleave', () => {
  clearTimeout(quickIdle);
  delete el.screen.dataset.quick;
});

// --- appearance ----------------------------------------------------------
//
// The theme and the background belong to the device, so this panel only ever
// talks to localStorage and IndexedDB. Both are applied by theme.js, which
// every other page calls too: changing the palette here changes it on the
// start and add-on pages as well.

const themePick = $('opt-theme');
const themeSwatches = $('theme-swatches');
const bgPanel = $('tool-appearance');
const bgFile = $('bg-file');
const bgNote = $('bg-note');
const bgDim = $('opt-bg-dim');
const bgBlur = $('opt-bg-blur');
const bgClear = $('bg-clear');

/** Five colours off the theme, so the names are not the only thing to go on. */
function paintSwatches(id) {
  const theme = THEMES.find((entry) => entry.id === id) ?? THEMES[0];
  themeSwatches.replaceChildren(
    ...['--void', '--shell', '--amber', '--teal', '--play-accent'].map((token) => {
      const chip = document.createElement('span');
      chip.style.background = theme.tokens[token];
      return chip;
    }),
  );
}

for (const theme of THEMES) {
  const option = document.createElement('option');
  option.value = theme.id;
  option.textContent = theme.name;
  themePick.append(option);
}
themePick.value = currentTheme();
paintSwatches(themePick.value);
themePick.addEventListener('change', () => paintSwatches(setTheme(themePick.value)));

function showBackground(name) {
  bgPanel.dataset.hasBg = name ? 'yes' : 'no';
  bgClear.disabled = !name;
  bgNote.dataset.tone = '';
  bgNote.textContent = name
    ? `${name} — kept in this browser only.`
    : 'No background set.';
}

function refuse(message) {
  bgNote.dataset.tone = 'bad';
  bgNote.textContent = message;
}

$('bg-choose').addEventListener('click', () => bgFile.click());

bgFile.addEventListener('change', async () => {
  const file = bgFile.files?.[0];
  // Cleared straight away, so picking the same file twice still counts as a
  // change and the panel does not go quiet on the second try.
  bgFile.value = '';
  if (!file) return;
  if (!file.type.startsWith('image/')) {
    refuse('That file is not an image.');
    return;
  }
  if (file.size > BACKGROUND_LIMIT) {
    const mb = (size) => Math.round(size / (1024 * 1024));
    refuse(`That image is ${mb(file.size)}MB. The limit is ${mb(BACKGROUND_LIMIT)}MB.`);
    return;
  }
  try {
    await saveBackground(file);
  } catch {
    // A browser keeping no site data, or a full store. The picture would not
    // survive the next load, so do not pretend it is set.
    refuse('This browser would not store the image.');
    return;
  }
  paintBackground(file);
  remember('bg-name', file.name);
  showBackground(file.name);
});

bgClear.addEventListener('click', async () => {
  await clearBackground();
  paintBackground(null);
  remember('bg-name', '');
  showBackground('');
});

for (const [input, key] of [[bgDim, 'dim'], [bgBlur, 'blur']]) {
  input.addEventListener('input', () => setBackgroundSettings({ [key]: Number(input.value) }));
}

const panelAlpha = $('opt-panel-alpha');
const screenAlpha = $('opt-screen-alpha');

for (const [input, key] of [[panelAlpha, 'panel'], [screenAlpha, 'screen']]) {
  input.addEventListener('input', () => {
    setSurfaceSettings({ [key]: Number(input.value) });
    // The picture is sized from the bezel, which these do not resize — but the
    // panels can reflow a fraction as their fill changes, so re-fit rather than
    // leave the canvas a pixel out.
    if (key === 'panel') fitScreen();
  });
}

const readoutToggle = $('opt-hide-readouts');
readoutToggle.addEventListener('change', () => {
  setReadoutsHidden(readoutToggle.checked);
  // Both lines are counted when the deck cap is measured, so re-fit and the
  // picture takes the height they just gave back.
  fitScreen();
});

{
  const stored = backgroundSettings();
  bgDim.value = String(stored.dim);
  bgBlur.value = String(stored.blur);
  const surfaces = surfaceSettings();
  panelAlpha.value = String(surfaces.panel);
  screenAlpha.value = String(surfaces.screen);
  readoutToggle.checked = readoutsHidden();
}

// The picture has to come out of a database, so the panel is filled in when it
// arrives rather than left claiming there is nothing there.
mountTheme().then((blob) => showBackground(blob ? recall('bg-name') || 'Background image' : ''));

document.addEventListener("fullscreenchange", () => {
  const on = document.fullscreenElement === el.screen;
  el.full.setAttribute("aria-pressed", String(on));
  el.full.textContent = on ? "Exit full screen" : "Full screen";
  quickFullscreen.setAttribute('aria-pressed', String(on));
  quickFullscreen.setAttribute('aria-label', el.full.textContent);
  quickFullscreen.title = el.full.textContent;
  // The element's box changes without the window resizing, and the observer
  // fires before the new size settles in some browsers.
  fitScreen();
});

function setFastForward(on) {
  if (fastForward === on) return;
  fastForward = on;
  frameClock = 0;
  audio?.flush();
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
let fastFrameMode = "fast";
let fastAudio = true;
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
 * Solo fast-forward batches retain audio, played at the same speed as the game.
 */
function runPaced(now, speed) {
  const due = schedule(now, frameClock, speed);
  frameClock = due.clock;

  const before = emu.frameCount;
  if (speed > 1 && due.frames > 0 && fastFrameMode === "fast") {
    emu.runAudioFrames(due.frames);
    if (fastAudio && audio?.ready) audio.push(emu.takeAudio(), speed);
  } else {
    const until = performance.now() + 6;
    for (let i = 0; i < due.frames; i += 1) {
      emu.runFrame();
      if ((speed === 1 || fastAudio) && audio?.ready) audio.push(emu.takeAudio(), speed);
      if (speed > 1 && performance.now() >= until) {
        if (i + 1 < due.frames) frameClock = 0;
        break;
      }
    }
  }
  return emu.frameCount - before;
}

/** Run frames for a slice of wall time rather than to a schedule. */
function runUnlimited() {
  const start = performance.now();
  const until = start + (fastFrameMode === "smooth" ? 6 : UNLIMITED_BUDGET_MS);
  const elapsed = unlimitedLast > 0 ? Math.max(1, start - unlimitedLast) : 1000 / 60;
  unlimitedLast = start;
  const chunks = [];
  let ran = 0;
  do {
    const count = fastFrameMode === "smooth" ? 1 : Math.min(UNLIMITED_MAX_FRAMES - ran, Math.max(1, Math.min(8, Math.floor((until - performance.now()) / unlimitedFrameMs))));
    const batchStart = performance.now();
    if (fastFrameMode === "smooth") emu.runFrame();
    else emu.runAudioFrames(count);
    unlimitedFrameMs = Math.max(0.1, (performance.now() - batchStart) / count);
    if (fastAudio && audio?.ready) {
      const samples = emu.takeAudio();
      if (samples) chunks.push(samples);
    }
    ran += count;
  } while (ran < UNLIMITED_MAX_FRAMES && performance.now() < until);
  if (chunks.length) {
    const samples = new Float32Array(chunks.reduce((n, chunk) => n + chunk.length, 0));
    let offset = 0;
    for (const chunk of chunks) { samples.set(chunk, offset); offset += chunk.length; }
    audio.push(samples, Math.max(1, ran * 1000 / GBA_FRAME_HZ / elapsed));
  }
  // The schedule means nothing while unlimited; start it afresh afterwards.
  frameClock = 0;
  return ran;
}
let unlimitedLast = 0;
let unlimitedFrameMs = UNLIMITED_BUDGET_MS;

function tick(now) {
  requestAnimationFrame(tick);
  if (!fastForward || fastForwardSpeed !== 0 || !running) unlimitedLast = 0;

  if (!emu || !running) {
    // Nothing accrues while paused, so there is nothing to catch up on.
    frameClock = 0;
    return;
  }

  const unlimited = fastForward && fastForwardSpeed === 0;

  // Ruby, Sapphire and Emerald read a clock on the cartridge, and the core
  // cannot read one itself: it runs on wasm32, where the host is the only
  // thing that knows what time it is. Pushed every frame rather than once, so
  // berry growth and the tides in Shoal Cave follow the wall clock rather than
  // however long the emulator happened to be running.
  emu.setWallClock(Date.now() / 1000);

  const ran = unlimited
    ? runUnlimited()
    : runPaced(now, fastForward ? fastForwardSpeed : 1);
  framesRun += ran;

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
  }

  // Cheap: the flag is a bool, and the copy only happens when the game has
  // actually written to the cartridge.
  if (now - lastBatteryCheck > BATTERY_CHECK_MS) {
    lastBatteryCheck = now;
    flushBattery();
  }

  if (now - fpsWindowStart >= 1000) {
    // Emulated frames per second, which is the number that means something.
    // Counting callbacks would just report the monitor's refresh rate.
    const fps = (framesRun * 1000) / (now - fpsWindowStart);
    const speed = fps / GBA_FRAME_HZ;
    el.footPerf.textContent = fastForward
      ? `${fps.toFixed(1)} fps · ${speed.toFixed(1)}×`
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
  const head = el.rig.querySelector(`[data-card-key="${CSS.escape(key)}"]`) ?? workshopReadout?.querySelector(`[data-card-key="${CSS.escape(key)}"]`);
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

const SIDES = ["left", "right"];
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
  // Opponents appear opposite the player party by default: the left rail is
  // where a reader's own sections belong.
  return placement[id] ?? (id === "enemies" ? "right" : "left");
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

function closeSlot(side, index) {
  slots[side].splice(index, 1);
  saveSlots();
  renderRails(lastSections);
}

/** Everything that can sit in a rail: the addon's sections. */
function railItems(sections) {
  return sections.map((section) => ({
      id: section.section_id,
      title: section.title,
      note: section.note,
      badge: section.badge,
      // What this pane's body is built from, so a rail can tell "the same
      // numbers again" from "something moved" without rebuilding to find out.
      sign: JSON.stringify(section),
      build: () => renderContent(section),
    }));
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
const dividerCache = new Map();
function railDivider(side, host) {
  if (dividerCache.has(side)) return dividerCache.get(side);
  const divider = document.createElement('div');
  divider.className = 'rail-divider'; divider.tabIndex = 0;
  divider.setAttribute('role', 'separator');
  divider.setAttribute('aria-label', `Resize ${side} panes`);
  divider.setAttribute('aria-orientation', 'horizontal');
  divider.setAttribute('aria-valuemin', '25'); divider.setAttribute('aria-valuemax', '75');
  function resize(value) {
    const ratio = Math.max(25, Math.min(75, value));
    host.style.setProperty('--pane-ratio', `${ratio}fr`);
    host.style.setProperty('--pane-rest', `${100 - ratio}fr`);
    divider.setAttribute('aria-valuenow', String(Math.round(ratio)));
    remember(`split-${side}`, String(ratio));
  }
  const saved = Number(recall(`split-${side}`));
  resize(saved >= 25 && saved <= 75 ? saved : 50);
  divider.addEventListener('pointerdown', event => {
    event.preventDefault(); divider.focus(); divider.setPointerCapture(event.pointerId);
  });
  divider.addEventListener('pointermove', event => {
    if (!divider.hasPointerCapture(event.pointerId)) return;
    const box = host.getBoundingClientRect();
    resize((event.clientY - box.top) / box.height * 100);
  });
  divider.addEventListener('pointerup', event => {
    if (divider.hasPointerCapture(event.pointerId)) divider.releasePointerCapture(event.pointerId);
  });
  divider.addEventListener('keydown', event => {
    const value = Number(divider.getAttribute('aria-valuenow'));
    const next = { ArrowUp: value - 5, ArrowDown: value + 5, Home: 25, End: 75 }[event.key];
    if (next === undefined) return;
    event.preventDefault(); event.stopPropagation(); resize(next);
  });
  dividerCache.set(side, divider);
  return divider;
}
let workshopReadout = null;
let workshopDraftPreview = { status: 'idle', sections: [], error: 'Add a field in the reader builder to see your add-on here.' };
let workshopPreviewSignature = '';

function renderWorkshopReadout() {
  if (!workshopReadout) {
    workshopReadout = document.createElement('section');
    workshopReadout.className = 'workshop-readout panel';
    workshopReadout.innerHTML = '<div data-workshop-preview></div>';
    document.querySelector('.transport').before(workshopReadout);
  }
  const signature = JSON.stringify([workshopDraftPreview, [...openCards]]);
  if (signature === workshopPreviewSignature) return;
  workshopPreviewSignature = signature;
  const host = workshopReadout.querySelector('[data-workshop-preview]');
  host.replaceChildren();
  if (workshopDraftPreview.status !== 'active' || workshopDraftPreview.error) {
    host.append(span('rail-note', workshopDraftPreview.error || (workshopDraftPreview.status === 'incompatible' ? 'This reader targets a different game or revision.' : 'Waiting for the reader to be ready.')));
    return;
  }
  for (const section of workshopDraftPreview.sections ?? []) {
    const block = document.createElement('section'); block.className = 'workshop-readout-section';
    block.append(span('panel__eyebrow', section.title), renderContent(section)); host.append(block);
  }
}

function renderRails(sections) {
  if (document.body.dataset.workshopPlayer === 'true') {
    lastSections = sections; renderWorkshopReadout(); return;
  }
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
      host.replaceChildren();
      host.dataset.split = 'false';
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
    host.dataset.split = String(blocks.length === 2);
    if (blocks.length === 2) blocks.splice(1, 0, railDivider(side, host));
    // `replaceChildren` removes and reinserts even a child that is already
    // there, which would undo every reused node above. Only touch the host
    // when the list it holds is actually a different list.
    const settled =
      host.childNodes.length === blocks.length &&
      blocks.every((block, index) => host.childNodes[index] === block);
    if (!settled) host.replaceChildren(...blocks);
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
    return cached.block;
  }

  if (cached && cached.chromeSig === chromeSig) {
    cached.body.replaceChildren(item.build());
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
  block.dataset.section = item.id;

  if (index === 0) {
    const tabs = document.createElement("div");
    tabs.className = "tabs";
    tabs.setAttribute("role", "tablist");
    tabs.setAttribute('aria-label', `${side} add-on sections`);
    tabs.append(
      ...available.map((option) => {
        const tab = document.createElement("button");
        tab.type = "button";
        tab.className = "tabs__tab";
        tab.setAttribute("role", "tab");
        tab.setAttribute("aria-selected", String(option.id === item.id));
        tab.tabIndex = option.id === item.id ? 0 : -1;
        tab.id = `tab-${side}-${available.indexOf(option)}`;
        tab.setAttribute('aria-controls', `pane-body-${side}-${index}`);
        tab.addEventListener('keydown', event => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault();
          const at = available.indexOf(option);
          const next = event.key === 'Home' ? 0 : event.key === 'End' ? available.length - 1
            : (at + (event.key === 'ArrowRight' ? 1 : -1) + available.length) % available.length;
          selectTab(side, index, available[next].id);
          $(`tab-${side}-${next}`)?.focus({ preventScroll: true });
        });
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

  const arrange = railButton('\u22ef', `Arrange ${item.title}`, () => {
    const menu = $('pane-layout-menu');
    $('pane-layout-title').textContent = item.title;
    const actions = $('pane-layout-actions');
    actions.replaceChildren();
    function action(label, run) {
      const button = railButton(label, label, () => {
        menu.hidePopover();
        run();
        const host = side === 'left' ? el.leftPane : el.rightPane;
        const moved = document.querySelector(`.slot[data-section="${CSS.escape(item.id)}"]`);
        (moved ?? host).querySelector('[aria-selected="true"], select, button')?.focus({ preventScroll: true });
      });
      button.className = 'key'; actions.append(button);
    }
    const other = side === 'left' ? 'right' : 'left';
    action(`Move to ${other} sidebar`, () => moveTo(item.id, other));
    if (slots[side].length > 1) {
      action('Keep only this pane', () => { slots[side] = [item.id]; saveSlots(); renderRails(lastSections); });
      action('Close this pane', () => closeSlot(side, index));
    } else {
      for (const option of available.filter(option => option.id !== item.id)) {
        action(`Split with ${option.title}`, () => { slots[side] = [item.id, option.id]; saveSlots(); renderRails(lastSections); });
      }
    }
    openToolPopup(menu, arrange);
  });
  arrange.className = 'pane-arrange';
  arrange.setAttribute('aria-label', `Arrange ${item.title}`);
  arrange.setAttribute('aria-expanded', 'false');
  arrange.setAttribute('aria-controls', 'pane-layout-menu');
  head.append(arrange);
  block.append(head);

  const body = document.createElement("div");
  body.className = "slot__body";
  body.id = `pane-body-${side}-${index}`;
  if (index === 0) {
    body.setAttribute('role', 'tabpanel');
    body.setAttribute('aria-labelledby', `tab-${side}-${available.indexOf(item)}`);
  } else body.setAttribute('aria-label', item.title);
  // A pane is either built from the snapshot or is a node that already exists.
  // Appending the existing one moves it out of whichever rail last had it.
  body.append(item.build());
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
  paintAddonStatus(snapshot);
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
  gameCode = code;
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
// where it left it. They also ride inside the automatic recovery checkpoint,
// so restoring one restores both halves together.

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

  fastFrameMode = recall("frame-mode") === "smooth" ? "smooth" : "fast";
  el.optFrameMode.value = fastFrameMode;
  fastAudio = recall("fast-audio") !== "0";
  el.optFastAudio.checked = fastAudio;

  // Explicitly against null: `Number(null)` is 0, not NaN, so treating a
  // missing preference as a number silently starts every visitor muted.
  const stored = recall("volume");
  const volume = stored === null ? NaN : Number(stored);
  if (Number.isFinite(volume) && volume >= 0 && volume <= 100) {
    el.optVolume.value = String(volume);
  }
  syncQuickAudio();
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
    // still lives in the emulator and in the recovery checkpoint.
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
  for (const button of [el.play, el.reset, el.eject, el.ff, el.save, el.store, quickScreenshot]) {
    button.disabled = !enabled;
  }

  // "Load from file" goes with them. `requireCartridge` already refuses the
  // action, but refusing after a file picker, a file chosen and a read is a
  // worse way to learn it than a key that was never live — and the title says
  // why rather than leaving a dead control unexplained.
  el.fileState.disabled = !enabled;
  el.loadState.dataset.disabled = String(!enabled);
  el.loadState.setAttribute("aria-disabled", String(!enabled));
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

  checkpointRecovery();
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
  say("Cartridge ejected");
}

el.eject.addEventListener("click", () => { closeToolSheets(); ejectRom(); });

async function startRom(bytes, name, recovered = null) {
  if (!recovered) { checkpointRecovery(); flushBattery(); }
  try {
    if (recovered) emu = recovered;
    else emu.loadRom(bytes);
  } catch (error) {
    say(error.message, "bad");
    return;
  }

  // Kept, rather than handed over and forgotten: the automatic checkpoint
  // stores the cartridge beside the state so it can be restored on its own.
  romBytes = new Uint8Array(bytes.slice(0));

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
  const restored = !recovered && emu.hasBattery && applyBattery(storedBattery(gameCode), gameCode);
  say(
    `Loaded ${name} · ${formatSize(bytes.byteLength)}` +
      (restored ? " · cartridge save restored" : ""),
    "good",
  );

  markLibrarySelection(name);
  return true;
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

async function loadFromUrl(url, name) {
  say(`Fetching ${name}…`);
  try {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
    await startRom(await response.arrayBuffer(), name);
  } catch (error) {
    say(`Could not fetch ${name}: ${error.message}`, "bad");
  }
}

async function loadFromFile(file) {
  await startRom(await file.arrayBuffer(), file.name);
}

// --- automatic recovery -------------------------------------------------
//
// One checkpoint in this browser, written every half minute and when the tab
// is hidden, so a closed tab or a crash costs at most that much play.

/** The cartridge in the machine. */
let romBytes = null;
/** The BIOS, which a checkpoint records so it is restored against the same one. */
let biosBytes = null;
let recoveryGeneration = 0;
let recoveryRestoring = false;
let recoveryPending = 0;
let recoveryErrorShown = false;

async function refreshRecovery() {
  const generation = ++recoveryGeneration;
  const owner = RECOVERY_OWNER;
  $('recovery-card').hidden = true;
  $('recovery-name').textContent = '';
  $('recovery-image').removeAttribute('src');
  try {
    const record = await readRecovery(owner);
    if (generation !== recoveryGeneration) return;
    if (!record) return;
    $('recovery-name').textContent = record.name;
    $('recovery-time').textContent = `Last played ${new Date(record.savedAt).toLocaleString()} · automatic checkpoint`;
    if (record.image?.startsWith('data:image/png;base64,')) $('recovery-image').src = record.image;
    $('recovery-card').hidden = false;
  } catch {
    if (generation === recoveryGeneration) $('recovery-status').textContent = 'Automatic recovery is unavailable: this browser could not open its storage.';
  }
}

// Copy the machine synchronously, before a switch/eject can replace it. Hashing
// and the atomic IndexedDB write can then finish without blocking emulation.
async function checkpointRecovery() {
  if (!emu?.hasRom || !romBytes || recoveryRestoring) return;
  const owner = RECOVERY_OWNER;
  recoveryPending++;
  try {
    const game = emu.snapshot().rom;
    const rom = romBytes;
    let observations = null;
    try {
      const text = localStorage.getItem(observationKey(owner, game));
      if (text) { observations = validateObservationConfig(JSON.parse(text)); matchObservationGame(observations, game); }
    } catch { observations = null; }
    const record = {
      version: 1, savedAt: Date.now(), name: romName, game,
      state: emu.saveState(), battery: emu.hasBattery ? emu.batterySave() : null,
      image: frameCanvas.toDataURL('image/png'),
      context: { placement: { ...placement }, slots: structuredClone(slots), splits: Object.fromEntries(SIDES.map(side => [side, Number(recall(`split-${side}`)) || 50])), hidden: hiddenPanels(), observations },
    };
    const bios = biosBytes;
    [record.romHash, record.stateHash, record.biosHash] = await Promise.all([fingerprint(rom), fingerprint(record.state), bios ? fingerprint(bios) : null]);
    await writeRecovery(owner, record, rom);
    $('recovery-status').textContent = 'Automatic recovery stays in this browser.';
    recoveryErrorShown = false;
    await refreshRecovery();
  } catch (error) {
    $('recovery-status').textContent = `Could not save recovery: ${error.message}`;
    if (!recoveryErrorShown) say('Automatic recovery could not be saved. You can still save to a file.', 'bad');
    recoveryErrorShown = true;
  } finally { recoveryPending--; }
}

$('continue-playing').addEventListener('click', async () => {
  if (recoveryRestoring || !emu || emu.hasRom) return;
  const owner = RECOVERY_OWNER;
  recoveryRestoring = true;
  $('continue-playing').disabled = true;
  try {
    const record = await readRecovery(owner);
    await validateRecovery(record, biosBytes);
    const context = record.context;
    if (context.observations) {
      validateObservationConfig(context.observations);
      matchObservationGame(context.observations, record.game);
    }
    // Validate in a separate machine. An incompatible state cannot damage the
    // current emulator or change any persisted layout/configuration.
    const candidate = await TinyBird.load();
    if (biosBytes) candidate.loadBios(biosBytes);
    candidate.loadRom(record.rom);
    candidate.loadState(record.state);
    if (record.battery) candidate.loadSave(record.battery);
    candidate.setPaused(false);
    candidate.setButtons(0);
    candidate.installManifests(activeManifests);
    candidate.setDisabledBuiltins(disabledBuiltins());
    if (emu.hasRom) throw new Error('A game was loaded meanwhile. Eject it and choose Continue playing again.');
    for (const key of Object.keys(placement)) delete placement[key];
    Object.assign(placement, Object.fromEntries(Object.entries(context.placement).filter(([, side]) => SIDES.includes(side))));
    for (const side of SIDES) slots[side] = (context.slots[side] ?? []).filter(id => typeof id === 'string').slice(0, MAX_SLOTS);
    for (const side of SIDES) {
      const ratio = Math.max(25, Math.min(75, context.splits?.[side] ?? 50));
      remember(`split-${side}`, String(ratio));
      const divider = dividerCache.get(side);
      if (divider) {
        const host = divider.parentElement;
        host?.style.setProperty('--pane-ratio', `${ratio}fr`);
        host?.style.setProperty('--pane-rest', `${100 - ratio}fr`);
        divider.setAttribute('aria-valuenow', String(Math.round(ratio)));
      }
    }
    remember('placement', JSON.stringify(placement)); saveSlots();
    saveHiddenPanels(Array.isArray(context.hidden) ? context.hidden : []);
    let configSaved = true;
    try {
      const key = observationKey(owner, record.game);
      if (context.observations) localStorage.setItem(key, JSON.stringify(context.observations));
      else localStorage.removeItem(key);
    } catch { configSaved = false; }
    await startRom(record.rom, record.name, candidate);
    frameClock = 0;
    present();
    flushBattery();
    say(configSaved ? `Continued ${record.name} from your automatic checkpoint.` : 'Game restored, but browser storage could not restore the observation config.', configSaved ? 'good' : 'bad');
  } catch (error) {
    $('recovery-status').textContent = `Could not continue: ${error.message}`;
    say(`Could not continue: ${error.message}`, 'bad');
  } finally {
    recoveryRestoring = false;
    $('continue-playing').disabled = false;
  }
});

setInterval(() => { if (running && !recoveryPending) checkpointRecovery(); }, RECOVERY_INTERVAL);
document.addEventListener('visibilitychange', () => { if (document.hidden) checkpointRecovery(); });
window.addEventListener('pagehide', () => { checkpointRecovery(); flushBattery(); });
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
    const stored = recall('bios');
    if (stored) {
      const bytes = Uint8Array.from(atob(stored), char => char.charCodeAt(0));
      emu.loadBios(bytes); biosBytes = bytes; return true;
    }
    const response = await fetch("/bios");
    if (!response.ok) return false;
    // Kept, because the recovery checkpoint records which BIOS a state was
    // saved against and refuses to restore it against another.
    biosBytes = new Uint8Array(await response.arrayBuffer());
    emu.loadBios(biosBytes);
    return true;
  } catch {
    return false;
  }
}

$('file-bios').addEventListener('change', async event => {
  const file = event.target.files[0]; event.target.value = '';
  if (!file) return;
  try {
    if (!emu || emu.hasRom) throw new Error('Eject the game before choosing a BIOS.');
    if (file.size !== 16384) throw new Error('Choose a 16 KiB GBA BIOS image.');
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (emu.hasRom) throw new Error('The game changed. Eject it and choose the BIOS again.');
    emu.loadBios(bytes); biosBytes = bytes;
    let saved = true;
    try { localStorage.setItem('tinybird:bios', btoa(String.fromCharCode(...bytes))); } catch { saved = false; }
    say(saved ? 'BIOS ready and saved in this browser.' : 'BIOS ready for this session; browser storage was unavailable.', 'good');
  } catch (error) { say(error.message, 'bad'); }
});

// --- library ------------------------------------------------------------

function markLibrarySelection(name) {
  for (const button of el.vaultList.querySelectorAll(".vault__item")) {
    button.setAttribute("aria-current", String(button.dataset.name === name));
  }
}

/** List the ROMs in the server's local folder. */
async function loadLibrary() {
  let entries = [];
  try {
    const response = await fetch("/api/local");
    entries = (await response.json()).assets ?? [];
  } catch {
    el.vaultNote.textContent = "The ROM folder could not be read. You can still open a game from disk.";
  }
  el.vaultList.hidden = entries.length === 0;
  el.vaultList.replaceChildren(...entries.map(renderLibraryRow));
}

function renderLibraryRow(asset) {
  const item = document.createElement("li");
  const button = document.createElement("button");
  button.type = "button";
  button.className = "vault__item";
  button.dataset.name = asset.name;
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

el.store.addEventListener("click", async () => {
  const blob = await new Promise((resolve) =>
    frameCanvas.toBlob(resolve, "image/png"),
  );
  if (!blob) {
    say("The screen could not be captured.", "bad");
    return;
  }
  download(new Uint8Array(await blob.arrayBuffer()), `${baseName(romName)}.png`);
  say("Screenshot downloaded.", "good");
});

el.fileState.addEventListener("change", async (event) => {
  const [file] = event.target.files;
  if (!file) return;
  closeToolSheets();
  if (!requireCartridge("load a save state")) {
    event.target.value = "";
    return;
  }
  try {
    // A state saved by the hosted site's vault is a container; a local one is
    // raw. unpackSave accepts both.
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
  if (!running) checkpointRecovery();
});

el.reset.addEventListener("click", () => {
  closeToolSheets();
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
  unlimitedLast = 0;
  audio?.flush();
});

el.optFrameMode.addEventListener("change", () => {
  fastFrameMode = el.optFrameMode.value === "smooth" ? "smooth" : "fast";
  remember("frame-mode", fastFrameMode);
  frameClock = 0;
  unlimitedLast = 0;
  unlimitedFrameMs = UNLIMITED_BUDGET_MS;
  audio?.flush();
});

el.optFastAudio.addEventListener("change", () => {
  fastAudio = el.optFastAudio.checked;
  remember("fast-audio", fastAudio ? "1" : "0");
  audio?.flush();
});

el.optVolume.addEventListener("input", () => {
  const volume = Number(el.optVolume.value) / 100;
  if (audio) audio.setVolume(volume);
  remember("volume", el.optVolume.value);
  syncQuickAudio();
});

el.optAudio.addEventListener("change", async () => {
  syncQuickAudio();
  if (!el.optAudio.checked) {
    if (audio) audio.close();
    audio = null;
    return;
  }
  audio = new AudioSink(emu ? emu.sampleRate : 32768);
  const sink = audio;
  sink.setVolume(Number(el.optVolume.value) / 100);
  // Browsers only allow this from a user gesture, which the change event is.
  if (!(await sink.resume()) && audio === sink) {
    say("The browser blocked audio. Click the page and try again.", "bad");
    el.optAudio.checked = false;
    audio = null;
    sink.close();
    syncQuickAudio();
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

/** How many manifest addons are installed, for the devtools handle. */
let manifestsInstalled = 0;
let playAddonBusy = false;

// The same preferences as the add-ons page, toggled without leaving the game.
function renderPlayAddons() {
  const list = $('play-addon-list');
  const focused = document.activeElement?.dataset.addonToggle;
  list.replaceChildren();
  function row(key, name, description, enabled, change) {
    const label = document.createElement('label');
    label.className = 'addon-switch';
    const text = document.createElement('span');
    const title = document.createElement('strong'); title.textContent = name;
    const note = document.createElement('small'); note.textContent = description;
    text.append(title, note);
    const input = document.createElement('input');
    input.type = 'checkbox'; input.checked = enabled;
    input.dataset.addonToggle = key;
    input.disabled = playAddonBusy;
    input.addEventListener('change', async () => {
      const next = input.checked;
      playAddonBusy = true;
      for (const toggle of list.querySelectorAll('input')) toggle.disabled = true;
      $('play-addon-message').textContent = '';
      try {
        await change(next);
        await installManifests();
        addonChannel?.postMessage({ type: 'changed' });
      } catch (error) {
        $('play-addon-message').textContent = error.message;
      } finally {
        playAddonBusy = false;
        renderPlayAddons();
        if ($('addon-menu').matches(':popover-open')) {
          [...list.querySelectorAll('input')].find(toggle => toggle.dataset.addonToggle === key)?.focus({ preventScroll: true });
        }
      }
    });
    label.append(text, input); list.append(label);
  }
  const disabled = disabledBuiltins();
  for (const info of emu?.snapshot().builtin_addons ?? []) {
    row(`builtin:${info.addon_id}`, info.display_name, 'Built-in', !disabled.includes(info.addon_id), enabled => {
      const next = new Set(disabledBuiltins());
      if (enabled) next.delete(info.addon_id); else next.add(info.addon_id);
      saveDisabledBuiltins([...next]);
    });
  }
  for (const item of localAddons()) {
    row(`local:${item.manifest.addon_id}`, item.manifest.display_name, 'Local', item.enabled, enabled => {
      saveLocalAddons(localAddons().map(other => other.manifest.addon_id === item.manifest.addon_id ? { ...other, enabled } : other));
    });
  }
  if (!list.childElementCount) list.textContent = emu ? 'No add-ons installed.' : 'Loading add-ons…';
  if (focused) [...list.querySelectorAll('input')].find(input => input.dataset.addonToggle === focused)?.focus({ preventScroll: true });
}
let addonGeneration = 0;
let activeManifests = [];
let addonChannel;
function previewWorkshop(manifest) {
  if (!emu?.hasRom) throw new Error('Load a game to test your reader.');
  try {
    emu.installManifests([manifest]);
    const snapshot = emu.snapshot();
    return { status: snapshot.community_addons?.[0]?.status ?? 'idle',
      error: snapshot.community_addons?.[0]?.error,
      sections: snapshot.addon?.sections?.filter(section => section.section_id.startsWith(`community:${manifest.addon_id}:`)) ?? [] };
  } finally { emu.installManifests(activeManifests); }
}
try {
  addonChannel = new BroadcastChannel('tinybird:addons');
  addonChannel.addEventListener('message', event => {
    const request = event.data;
    if (request?.type === 'changed') { installManifests(); return; }
    if (request?.type !== 'preview' || !emu?.hasRom) return;
    try {
      const manifest = { ...request.manifest, addon_id: 'preview.reader' };
      emu.installManifests([manifest]);
      const snapshot = emu.snapshot();
      addonChannel.postMessage({ type: 'preview-result', request: request.request,
        status: snapshot.community_addons?.[0]?.status ?? 'idle', error: snapshot.community_addons?.[0]?.error,
        sections: snapshot.addon?.sections?.filter(section => section.section_id.startsWith('community:preview.reader:')) ?? [] });
    } catch (error) {
      addonChannel.postMessage({ type: 'preview-result', request: request.request, error: error.message });
    } finally { emu.installManifests(activeManifests); }
  });
} catch {}
window.addEventListener('storage', event => { if (event.key?.startsWith('tinybird:local-addons:') && !event.key.endsWith(':draft')) installManifests(); });
window.addEventListener('focus', () => { if (emu) installManifests(); });

/**
 * Install this browser's enabled local add-ons beside the compiled ones.
 *
 * Failing is not worth interrupting a boot for: the compiled addons are still
 * there.
 */
async function installManifests() {
  if (!emu) return;
  const generation = ++addonGeneration;
  try {
    applyBuiltinPreferences();
    const manifests = enabledManifests(localAddons());
    const installed = emu.installManifests(manifests);
    activeManifests = manifests;
    manifestsInstalled = installed;
    const snapshot = emu.snapshot();
    renderSnapshot(snapshot);
    paintAddonStatus(snapshot);
    $('play-addon-message').textContent = '';
    if ($('addon-menu').matches(':popover-open')) renderPlayAddons();
  } catch (error) {
    if (generation === addonGeneration) {
      $('addon-runtime-status').textContent = `Add-ons: ${error.message}`;
      $('play-addon-message').textContent = `Could not refresh add-ons: ${error.message}`;
    }
  }
}

function applyBuiltinPreferences() {
  const known = new Set((emu.snapshot().builtin_addons ?? []).map(info => info.addon_id));
  emu.setDisabledBuiltins(disabledBuiltins().filter(id => known.has(id)));
}

function paintAddonStatus(snapshot) {
  const statuses = snapshot?.community_addons ?? [];
  const status = statuses.length
    ? statuses.map(item => `${item.display_name}: ${item.error ?? item.status}`).join(' · ')
    : manifestsInstalled ? 'Load a game to activate your add-ons.' : 'Build readers on the Add-ons page.';
  if ($('addon-runtime-status').textContent !== status) $('addon-runtime-status').textContent = status;
}

async function boot() {
  try {
    emu = await TinyBird.load();
    // Before the first snapshot: the registry is built on first read.
    await installManifests();
    // A handle for devtools and for the Workshop's embedded player. Everything
    // the page does goes through the emulator, so a session can be inspected
    // without instrumenting the page to find out.
    window.tinybird = {
      previewAddon: previewWorkshop,
      showAddonPreview(result) {
        if (document.body.dataset.workshopPlayer !== 'true') return;
        workshopDraftPreview = result; renderWorkshopReadout();
      },
      attachAddonPreview(host) {
        if (document.body.dataset.workshopPlayer !== 'true') return;
        renderWorkshopReadout();
        if (workshopReadout.parentElement !== host) host.replaceChildren(workshopReadout);
      },
      refreshAddons: installManifests,
      get emu() {
        return emu;
      },
      /** Addon manifests installed, beyond the compiled addons. */
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
    };
  } catch (error) {
    setLink("error", "emulator unavailable");
    say(
      error instanceof EmulatorError
        ? error.message
        : "The emulator module could not be loaded.",
      "bad",
    );
    el.vaultList.hidden = false;
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

  requestAnimationFrame((now) => {
    fpsWindowStart = now;
    tick(now);
  });

  await refreshRecovery();
  await loadLibrary();

  // A ?rom= link boots straight into a game, from the library or any URL the
  // browser can fetch.
  const requested = new URLSearchParams(location.search).get("rom");
  if (requested) {
    const label =
      new URLSearchParams(location.search).get("name") ??
      decodeURIComponent(requested.split("/").pop() ?? "cartridge");
    await loadFromUrl(requested, label);
  }
}

boot();

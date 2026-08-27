// Input bindings: which key or gamepad button drives which GBA button.
//
// This module owns the mapping and nothing else — it never touches the
// emulator, the DOM, or the page. It answers three questions:
//
//   - given a keyboard event code, which action is that?      (`actionForKey`)
//   - given the connected gamepads, which actions are held?   (`padActions`)
//   - what are the bindings, and how do I change them?        (`bindings`, `bind`)
//
// Keeping it separate is what makes it testable without a browser: the tests
// hand it plain objects shaped like `Gamepad` and assert on the result.

/** Button bit positions. Must match BUTTON in tinybird.js. */
export const BUTTON = Object.freeze({
  A: 1 << 0,
  B: 1 << 1,
  SELECT: 1 << 2,
  START: 1 << 3,
  RIGHT: 1 << 4,
  LEFT: 1 << 5,
  UP: 1 << 6,
  DOWN: 1 << 7,
  R: 1 << 8,
  L: 1 << 9,
});

/**
 * Everything that can be bound, in the order the settings panel lists them.
 *
 * `bit` is the GBA button an action presses. Fast forward has none: it is a
 * thing the emulator host does, not a thing the cartridge can see, and giving
 * it a fake bit would send it to the game.
 */
export const ACTIONS = Object.freeze([
  { id: "UP", label: "Up", bit: BUTTON.UP, group: "Pad" },
  { id: "DOWN", label: "Down", bit: BUTTON.DOWN, group: "Pad" },
  { id: "LEFT", label: "Left", bit: BUTTON.LEFT, group: "Pad" },
  { id: "RIGHT", label: "Right", bit: BUTTON.RIGHT, group: "Pad" },
  { id: "A", label: "A", bit: BUTTON.A, group: "Buttons" },
  { id: "B", label: "B", bit: BUTTON.B, group: "Buttons" },
  { id: "L", label: "L shoulder", bit: BUTTON.L, group: "Buttons" },
  { id: "R", label: "R shoulder", bit: BUTTON.R, group: "Buttons" },
  { id: "START", label: "Start", bit: BUTTON.START, group: "Buttons" },
  { id: "SELECT", label: "Select", bit: BUTTON.SELECT, group: "Buttons" },
  { id: "FAST_FORWARD", label: "Fast forward", bit: 0, group: "Emulator" },
]);

/**
 * The bindings a fresh browser starts with.
 *
 * The keyboard half matches the desktop frontend so muscle memory carries
 * between the two. The gamepad half is the W3C "standard" layout, which is
 * what a browser reports for essentially every modern controller:
 * 0-3 are the face buttons bottom/right/left/top, 4-7 the shoulders and
 * triggers, 8/9 select and start, 12-15 the d-pad.
 *
 * Face buttons are mapped by position, not by letter. A GBA holds A under the
 * thumb on the right and B to its left; on a standard pad those positions are
 * button 1 and button 0. Mapping GBA A to the pad's "A" would put the two
 * buttons in the wrong places under the same fingers.
 */
export const DEFAULTS = Object.freeze({
  UP: { keys: ["ArrowUp"], pads: [12] },
  DOWN: { keys: ["ArrowDown"], pads: [13] },
  LEFT: { keys: ["ArrowLeft"], pads: [14] },
  RIGHT: { keys: ["ArrowRight"], pads: [15] },
  A: { keys: ["KeyZ", "KeyJ"], pads: [1] },
  B: { keys: ["KeyX", "KeyK"], pads: [0] },
  L: { keys: ["KeyA", "KeyQ"], pads: [4] },
  R: { keys: ["KeyS", "KeyW"], pads: [5] },
  START: { keys: ["Enter"], pads: [9] },
  SELECT: { keys: ["ShiftRight", "ShiftLeft", "Backspace"], pads: [8] },
  FAST_FORWARD: { keys: ["Space"], pads: [7] },
});

const STORAGE_KEY = "tinybird:controls";

/**
 * Below this, a stick is at rest.
 *
 * Analogue sticks do not return to exactly zero, and a worn one can sit at a
 * tenth of its travel. Without a deadzone the menu cursor drifts on its own,
 * which reads as a broken emulator rather than as a tired controller.
 */
export const STICK_DEADZONE = 0.5;

/** The left stick, reported as a d-pad. Index pairs are [axis, actions]. */
const STICK_AXES = [
  { axis: 0, negative: "LEFT", positive: "RIGHT" },
  { axis: 1, negative: "UP", positive: "DOWN" },
];

function emptyBindings() {
  return Object.fromEntries(ACTIONS.map((action) => [action.id, { keys: [], pads: [] }]));
}

/** A deep copy, so a caller mutating what it gets back cannot corrupt DEFAULTS. */
export function defaultBindings() {
  const out = emptyBindings();
  for (const [id, binding] of Object.entries(DEFAULTS)) {
    out[id] = { keys: [...binding.keys], pads: [...binding.pads] };
  }
  return out;
}

/**
 * Coerce whatever came out of storage into a usable binding table.
 *
 * Storage is not trusted input in the security sense, but it is old input: it
 * may have been written by a version of this page that had different actions.
 * Anything unrecognised is dropped and anything missing falls back to its
 * default, so an upgrade never leaves someone unable to press A.
 */
export function normalize(raw) {
  const out = defaultBindings();
  if (!raw || typeof raw !== "object") return out;

  for (const action of ACTIONS) {
    const entry = raw[action.id];
    if (!entry || typeof entry !== "object") continue;

    // An explicitly empty list is a real choice — someone unbound the action —
    // so only a missing or malformed list falls back to the default.
    if (Array.isArray(entry.keys)) {
      out[action.id].keys = entry.keys.filter((code) => typeof code === "string" && code);
    }
    if (Array.isArray(entry.pads)) {
      out[action.id].pads = entry.pads
        .map((index) => Number(index))
        .filter((index) => Number.isInteger(index) && index >= 0 && index < 32);
    }
  }

  return out;
}

/**
 * The binding table plus the lookups the page needs on every frame.
 *
 * `storage` is injected so the tests can run without a browser, and so a
 * private window with storage disabled degrades to in-memory bindings rather
 * than throwing on load.
 */
export class Controls {
  #bindings;
  #keyIndex = new Map();
  #storage;

  constructor(storage = null) {
    this.#storage = storage;
    this.#bindings = normalize(this.#read());
    this.#reindex();
  }

  #read() {
    try {
      const raw = this.#storage?.getItem(STORAGE_KEY);
      return raw ? JSON.parse(raw) : null;
    } catch {
      // Unparseable or unavailable; defaults are a fine answer to both.
      return null;
    }
  }

  #write() {
    try {
      this.#storage?.setItem(STORAGE_KEY, JSON.stringify(this.#bindings));
    } catch {
      // A full or disabled store. The bindings still work for this session.
    }
  }

  /**
   * Keyboard lookup is a flat map rebuilt on every change rather than a walk
   * over the actions, because it runs on every keydown and keyup.
   */
  #reindex() {
    this.#keyIndex.clear();
    for (const action of ACTIONS) {
      for (const code of this.#bindings[action.id].keys) {
        this.#keyIndex.set(code, action.id);
      }
    }
  }

  /** The live table. Treat as read-only; go through `bind` to change it. */
  get bindings() {
    return this.#bindings;
  }

  /** Which action this keyboard code drives, or null. */
  actionForKey(code) {
    return this.#keyIndex.get(code) ?? null;
  }

  /** Every key bound to an action, in binding order. */
  keysFor(actionId) {
    return this.#bindings[actionId]?.keys ?? [];
  }

  /** Every gamepad button index bound to an action. */
  padsFor(actionId) {
    return this.#bindings[actionId]?.pads ?? [];
  }

  /**
   * Bind an input to an action, taking it away from whatever held it before.
   *
   * Stealing rather than sharing is deliberate: one physical key driving two
   * GBA buttons is almost always a mistake someone is about to make, and the
   * alternative is a conflict warning nobody reads. An action may still have
   * several keys — that direction is useful, and how Z and J both mean A.
   */
  bind(actionId, kind, value) {
    if (!this.#bindings[actionId]) return false;
    const field = kind === "pad" ? "pads" : "keys";

    for (const action of ACTIONS) {
      this.#bindings[action.id][field] = this.#bindings[action.id][field].filter(
        (existing) => existing !== value,
      );
    }
    this.#bindings[actionId][field].push(value);

    this.#reindex();
    this.#write();
    return true;
  }

  /** Drop one binding. */
  unbind(actionId, kind, value) {
    if (!this.#bindings[actionId]) return;
    const field = kind === "pad" ? "pads" : "keys";
    this.#bindings[actionId][field] = this.#bindings[actionId][field].filter(
      (existing) => existing !== value,
    );
    this.#reindex();
    this.#write();
  }

  /** Back to the shipped bindings. */
  reset() {
    this.#bindings = defaultBindings();
    this.#reindex();
    this.#write();
  }

  /**
   * Which actions the connected pads are asking for.
   *
   * Takes the gamepad list rather than calling `navigator` itself, so the tests
   * can pass plain objects and so a caller that already polled does not poll
   * twice. Several pads are OR-ed together: two people on one machine, or one
   * person who left a second controller plugged in, both just work.
   */
  padActions(gamepads) {
    const held = new Set();
    for (const pad of gamepads ?? []) {
      if (!pad) continue;

      for (const action of ACTIONS) {
        for (const index of this.#bindings[action.id].pads) {
          if (pad.buttons?.[index]?.pressed) held.add(action.id);
        }
      }

      // The left stick doubles as the d-pad. Bound implicitly rather than
      // through the table: an axis is not a button, and nobody wants to
      // rebind "left stick left" separately from "d-pad left".
      for (const { axis, negative, positive } of STICK_AXES) {
        const value = pad.axes?.[axis] ?? 0;
        if (value <= -STICK_DEADZONE) held.add(negative);
        else if (value >= STICK_DEADZONE) held.add(positive);
      }
    }
    return held;
  }

  /** The GBA button mask for a set of held actions. */
  static maskFor(actionIds) {
    let mask = 0;
    for (const action of ACTIONS) {
      if (actionIds.has(action.id)) mask |= action.bit;
    }
    return mask;
  }
}

/**
 * A keyboard code as something worth showing a person.
 *
 * `KeyZ` and `ArrowUp` are how the browser names keys and not how anyone
 * reads them.
 */
export function keyLabel(code) {
  if (!code) return "";
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("Numpad")) return `Num ${code.slice(6)}`;
  if (code.startsWith("Arrow")) return { Up: "↑", Down: "↓", Left: "←", Right: "→" }[code.slice(5)] ?? code;

  return (
    {
      Space: "Space",
      Enter: "Enter",
      Escape: "Esc",
      Backspace: "Bksp",
      ShiftLeft: "L Shift",
      ShiftRight: "R Shift",
      ControlLeft: "L Ctrl",
      ControlRight: "R Ctrl",
      AltLeft: "L Alt",
      AltRight: "R Alt",
      Tab: "Tab",
    }[code] ?? code
  );
}

/**
 * A gamepad button index as a name.
 *
 * The standard layout names positions, not the letters printed on any
 * particular controller — a pad whose bottom face button says "B" still
 * reports it as index 0 — so these are described by where they are.
 */
export function padLabel(index) {
  return (
    {
      0: "Face down",
      1: "Face right",
      2: "Face left",
      3: "Face up",
      4: "L bumper",
      5: "R bumper",
      6: "L trigger",
      7: "R trigger",
      8: "Select",
      9: "Start",
      10: "L stick",
      11: "R stick",
      12: "Pad up",
      13: "Pad down",
      14: "Pad left",
      15: "Pad right",
    }[index] ?? `Button ${index}`
  );
}

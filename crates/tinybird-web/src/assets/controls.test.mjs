import assert from "node:assert/strict";
import test from "node:test";

import {
  ACTIONS,
  BUTTON,
  Controls,
  DEFAULTS,
  STICK_DEADZONE,
  defaultBindings,
  keyLabel,
  normalize,
  padLabel,
} from "./controls.js";

/** A localStorage stand-in, so the tests never need a browser. */
function memoryStorage(seed = {}) {
  const map = new Map(Object.entries(seed));
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => map.set(key, String(value)),
    map,
  };
}

/** A gamepad stand-in: just the parts the module reads. */
function pad({ pressed = [], axes = [0, 0] } = {}) {
  const buttons = Array.from({ length: 16 }, (_, index) => ({
    pressed: pressed.includes(index),
  }));
  return { buttons, axes };
}

test("every action ships with a default binding", () => {
  for (const action of ACTIONS) {
    assert.ok(DEFAULTS[action.id], `${action.id} has no default`);
  }
});

test("the keyboard defaults match the desktop frontend", () => {
  const controls = new Controls(memoryStorage());
  assert.equal(controls.actionForKey("KeyZ"), "A");
  assert.equal(controls.actionForKey("KeyX"), "B");
  assert.equal(controls.actionForKey("Enter"), "START");
  assert.equal(controls.actionForKey("ShiftRight"), "SELECT");
  assert.equal(controls.actionForKey("ArrowUp"), "UP");
  assert.equal(controls.actionForKey("Space"), "FAST_FORWARD");
  assert.equal(controls.actionForKey("KeyP"), null);
});

test("fast forward drives no GBA button", () => {
  // It is a thing the host does. A bit here would send it to the cartridge.
  const fastForward = ACTIONS.find((action) => action.id === "FAST_FORWARD");
  assert.equal(fastForward.bit, 0);
  assert.equal(Controls.maskFor(new Set(["FAST_FORWARD"])), 0);
});

test("a mask combines every held action", () => {
  const mask = Controls.maskFor(new Set(["A", "UP", "START"]));
  assert.equal(mask, BUTTON.A | BUTTON.UP | BUTTON.START);
});

test("binding an input takes it away from whatever held it", () => {
  const controls = new Controls(memoryStorage());
  // Z means A out of the box; give it to B instead.
  controls.bind("B", "key", "KeyZ");

  assert.equal(controls.actionForKey("KeyZ"), "B");
  assert.ok(!controls.keysFor("A").includes("KeyZ"), "A should have lost Z");
  // A still has its other key, so the change costs nothing else.
  assert.ok(controls.keysFor("A").includes("KeyJ"));
});

test("an action can hold several keys at once", () => {
  const controls = new Controls(memoryStorage());
  controls.bind("A", "key", "KeyN");
  assert.deepEqual(controls.keysFor("A"), ["KeyZ", "KeyJ", "KeyN"]);
  assert.equal(controls.actionForKey("KeyN"), "A");
});

test("unbinding leaves the action with nothing rather than the default", () => {
  const controls = new Controls(memoryStorage());
  controls.unbind("A", "key", "KeyZ");
  controls.unbind("A", "key", "KeyJ");

  assert.deepEqual(controls.keysFor("A"), []);
  assert.equal(controls.actionForKey("KeyZ"), null);
});

test("bindings survive a reload", () => {
  const storage = memoryStorage();
  new Controls(storage).bind("A", "key", "KeyN");

  const reloaded = new Controls(storage);
  assert.equal(reloaded.actionForKey("KeyN"), "A");
});

test("reset restores the shipped bindings", () => {
  const controls = new Controls(memoryStorage());
  controls.bind("B", "key", "KeyZ");
  controls.reset();

  assert.equal(controls.actionForKey("KeyZ"), "A");
  assert.deepEqual(controls.bindings, defaultBindings());
});

test("a storage that throws still yields working bindings", () => {
  const hostile = {
    getItem() {
      throw new Error("private window");
    },
    setItem() {
      throw new Error("private window");
    },
  };

  const controls = new Controls(hostile);
  assert.equal(controls.actionForKey("KeyZ"), "A");
  // And a write that cannot persist is not a write that crashes the page.
  assert.doesNotThrow(() => controls.bind("A", "key", "KeyN"));
  assert.equal(controls.actionForKey("KeyN"), "A");
});

test("stored bindings from an older page are repaired, not trusted", () => {
  const stored = normalize({
    A: { keys: ["KeyM"], pads: [3] },
    // Junk of every shape the old page could plausibly have left behind.
    B: { keys: "KeyX", pads: [999, -1, "two", 2] },
    GONE: { keys: ["KeyQ"] },
    UP: null,
  });

  assert.deepEqual(stored.A, { keys: ["KeyM"], pads: [3] });
  // A malformed list falls back to the default; a bad index is dropped.
  assert.deepEqual(stored.B.keys, DEFAULTS.B.keys);
  assert.deepEqual(stored.B.pads, [2]);
  // An action this page no longer has simply does not appear.
  assert.ok(!("GONE" in stored));
  // And a missing one is still fully playable.
  assert.deepEqual(stored.UP, { keys: ["ArrowUp"], pads: [12] });
});

test("an explicitly emptied binding is kept as a choice", () => {
  // Someone who unbound an action and reloaded should not find it back.
  const stored = normalize({ A: { keys: [], pads: [] } });
  assert.deepEqual(stored.A, { keys: [], pads: [] });
});

test("gamepad face buttons sit where a GBA has them, not where the letters do", () => {
  const controls = new Controls(memoryStorage());
  // Standard layout: 0 is the bottom face button, 1 the right one. A GBA's A
  // is the right-hand thumb button, so it belongs on 1.
  assert.deepEqual(controls.padsFor("A"), [1]);
  assert.deepEqual(controls.padsFor("B"), [0]);

  assert.deepEqual([...controls.padActions([pad({ pressed: [1] })])], ["A"]);
});

test("the d-pad and shoulders map to the standard layout", () => {
  const controls = new Controls(memoryStorage());
  const held = controls.padActions([pad({ pressed: [12, 4, 5, 9] })]);
  assert.deepEqual([...held].sort(), ["L", "R", "START", "UP"]);
});

test("the left stick works as a d-pad past the deadzone", () => {
  const controls = new Controls(memoryStorage());

  // At rest, including the drift a worn stick sits at.
  assert.equal(controls.padActions([pad({ axes: [0, 0] })]).size, 0);
  assert.equal(controls.padActions([pad({ axes: [0.3, -0.2] })]).size, 0);

  const held = controls.padActions([pad({ axes: [-1, STICK_DEADZONE] })]);
  assert.deepEqual([...held].sort(), ["DOWN", "LEFT"]);
});

test("two controllers are combined rather than one winning", () => {
  const controls = new Controls(memoryStorage());
  const held = controls.padActions([pad({ pressed: [1] }), pad({ pressed: [9] })]);
  assert.deepEqual([...held].sort(), ["A", "START"]);
});

test("a disconnected slot is skipped", () => {
  // navigator.getGamepads() returns nulls in the gaps after an unplug.
  const controls = new Controls(memoryStorage());
  assert.doesNotThrow(() => controls.padActions([null, pad({ pressed: [1] }), null]));
  assert.deepEqual([...controls.padActions([null, pad({ pressed: [1] })])], ["A"]);
});

test("binding a gamepad button adds it alongside what the action already has", () => {
  const controls = new Controls(memoryStorage());
  controls.bind("A", "pad", 3);

  // Both now press A, the same way Z and J both do on the keyboard.
  assert.deepEqual(controls.padsFor("A"), [1, 3]);
  assert.deepEqual([...controls.padActions([pad({ pressed: [3] })])], ["A"]);
  assert.deepEqual([...controls.padActions([pad({ pressed: [1] })])], ["A"]);
});

test("replacing one binding is unbind then bind, which is what the panel does", () => {
  const controls = new Controls(memoryStorage());
  // Clicking an existing chip and pressing something else should swap that
  // one input, not add a second.
  controls.unbind("A", "pad", 1);
  controls.bind("A", "pad", 3);

  assert.deepEqual(controls.padsFor("A"), [3]);
  assert.equal(controls.padActions([pad({ pressed: [1] })]).size, 0);
});

test("keys and pad buttons are bound independently", () => {
  const controls = new Controls(memoryStorage());
  // Rebinding a pad button must not disturb the keyboard, and vice versa.
  controls.bind("A", "pad", 3);
  assert.deepEqual(controls.keysFor("A"), ["KeyZ", "KeyJ"]);

  controls.bind("A", "key", "KeyN");
  assert.deepEqual(controls.padsFor("A"), [1, 3]);
});

test("labels read as keys people recognise", () => {
  assert.equal(keyLabel("KeyZ"), "Z");
  assert.equal(keyLabel("Digit1"), "1");
  assert.equal(keyLabel("ArrowUp"), "↑");
  assert.equal(keyLabel("ShiftRight"), "R Shift");
  assert.equal(keyLabel("Space"), "Space");
  // Anything unrecognised is shown as-is rather than hidden.
  assert.equal(keyLabel("IntlBackslash"), "IntlBackslash");
  assert.equal(keyLabel(""), "");
});

test("gamepad labels name positions, not the letters on any one pad", () => {
  assert.equal(padLabel(0), "Face down");
  assert.equal(padLabel(1), "Face right");
  assert.equal(padLabel(12), "Pad up");
  assert.equal(padLabel(27), "Button 27");
});

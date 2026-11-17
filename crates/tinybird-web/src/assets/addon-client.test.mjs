import test from 'node:test';
import assert from 'node:assert/strict';
import { localAddons, saveLocalAddons, enabledManifests, parseManifest, disabledBuiltins, saveDisabledBuiltins, hiddenPanels, saveHiddenPanels } from './addon-client.js';

function withStorage(run) {
  const memory = new Map();
  globalThis.localStorage = { getItem: key => memory.get(key), setItem: (key, value) => memory.set(key, value) };
  try { run(memory); } finally { delete globalThis.localStorage; }
}

test('local add-ons round-trip and only enabled ones are activated', () => withStorage(() => {
  const on = { id: 'uuid', enabled: true, manifest: { addon_id: 'mine', display_name: 'Mine' } };
  const off = { id: 'other', enabled: false, manifest: { addon_id: 'off', display_name: 'Off' } };
  assert.deepEqual(localAddons(), []);
  saveLocalAddons([on, off]);
  assert.deepEqual(localAddons(), [on, off]);
  assert.deepEqual(enabledManifests(localAddons()).map(m => m.addon_id), ['local.uuid']);
  assert.throws(() => saveLocalAddons(Array(5).fill(on)), /four/);
}));

test('add-ons saved before local-only mode still load', () => withStorage(memory => {
  const saved = { id: 'old', enabled: true, manifest: { addon_id: 'old', display_name: 'Old' } };
  memory.set('tinybird:local-addons:guest', JSON.stringify([saved]));
  memory.set('tinybird:local-addons:guest:builtins', JSON.stringify(['pokemon_frlg_party']));
  assert.deepEqual(localAddons(), [saved]);
  assert.deepEqual(disabledBuiltins(), ['pokemon_frlg_party']);
}));

test('built-in and panel choices persist', () => withStorage(() => {
  assert.deepEqual(disabledBuiltins(), []);
  saveDisabledBuiltins(['pokemon_frlg_party']);
  assert.deepEqual(disabledBuiltins(), ['pokemon_frlg_party']);
  assert.deepEqual(hiddenPanels(), []);
  saveHiddenPanels(['shots']);
  assert.deepEqual(hiddenPanels(), ['shots']);
}));

test('manifests are bounded single objects', () => {
  assert.throws(() => parseManifest('[]'), /object/);
  assert.throws(() => parseManifest(' '.repeat(65537)), /64 KiB/);
  assert.equal(parseManifest('{"addon_id":"a"}').addon_id, 'a');
});

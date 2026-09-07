import test from 'node:test';
import assert from 'node:assert/strict';
import { localAddons, saveLocalAddons, enabledManifests, parseManifest, disabledBuiltins, saveDisabledBuiltins } from './addon-client.js';
test('local installations are account-scoped and only enabled available releases are activated', () => {
  const memory = new Map();
  globalThis.localStorage = { getItem: key => memory.get(key), setItem: (key, value) => memory.set(key, value) };
  try {
    const local = { id: 'uuid', enabled: true, manifest: { addon_id: 'mine', display_name: 'Mine' } };
    saveLocalAddons({ id: 'alice' }, [local]);
    saveDisabledBuiltins({ id: 'alice' }, ['pokemon_frlg_party']);
    assert.deepEqual(disabledBuiltins({ id: 'alice' }), ['pokemon_frlg_party']);
    assert.deepEqual(disabledBuiltins({ id: 'bob' }), []);
    assert.deepEqual(disabledBuiltins(null), []);
    assert.deepEqual(localAddons({ id: 'bob' }), []);
    assert.deepEqual(localAddons(null), []);
    assert.deepEqual(localAddons({ id: 'alice' }), [local]);
    const result = enabledManifests([
      { enabled: true, available: true, manifest: { addon_id: 'community.one' } },
      { enabled: false, available: true, manifest: { addon_id: 'disabled' } },
      { enabled: true, available: false, manifest: { addon_id: 'withdrawn' } },
    ], [local]);
    assert.deepEqual(result.map(m => m.addon_id), ['community.one', 'local.uuid']);
    assert.throws(() => saveLocalAddons(null, Array(5).fill(local)), /four/);
    assert.throws(() => parseManifest('[]'), /object/);
    assert.throws(() => parseManifest(' '.repeat(65537)), /64 KiB/);
  } finally { delete globalThis.localStorage; }
});

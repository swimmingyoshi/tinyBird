import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { observationConfig, validateObservationConfig, matchObservationGame } from './workshop-observations.js';

const fixture = JSON.parse(await readFile(new URL('../../../../tests/fixtures/observations.json', import.meta.url), 'utf8'));
test('Workshop exports the same configuration consumed by Rust and Python', () => {
  assert.deepEqual(observationConfig({ game_code: 'TBST', revision: 2 }, [{ name: 'player.hp', address: 0x02000001, width: 2 }]), fixture);
  matchObservationGame(fixture, { game_code: 'TBST', revision: 2 });
  assert.throws(() => matchObservationGame(fixture, { game_code: 'TBST', revision: 3 }));
});
test('malformed configs, duplicate names and wrapping addresses are rejected', () => {
  for (const mutate of [
    c => c.schema_version = 2, c => c.game.code = 'BPR', c => c.game.revision = 256,
    c => c.fields.push(c.fields[0]), c => c.fields[0].name = 'player..hp',
    c => c.fields[0].width = 0, c => c.fields[0].address = 0xffffffff,
    c => c.fields[0].address = 1.5, c => c.fields = [], c => c.extra = true,
  ]) {
    const changed = structuredClone(fixture); mutate(changed);
    assert.throws(() => validateObservationConfig(changed));
  }
});

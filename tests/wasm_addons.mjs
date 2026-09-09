// Real browser ABI regression using local FireRed fixtures; never modifies them.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { TinyBird } from '../crates/tinybird-web/src/assets/tinybird.js';
import { builtinSheet, memorySheet } from '../crates/tinybird-web/src/assets/memory-sheets.js';
import { partyTemplate } from '../crates/tinybird-web/src/assets/workshop-model.js';
const root = new URL('../', import.meta.url);
const [wasm, rom, state] = await Promise.all([
  readFile(new URL('target/wasm32-unknown-unknown/release/tinybird_wasm.wasm', root)),
  readFile(new URL('roms/pokemon_fire_red.gba', root)), readFile(new URL('TradeTest1.state', root)),
]);
const { instance } = await WebAssembly.instantiate(wasm, {});
const emu = new TinyBird(instance.exports); emu.loadRom(rom); emu.loadState(state);
const base = emu.snapshot(); const initialState = emu.saveState();
const header = emu.readMemory(0x080000ac, 4);
assert.equal(new TextDecoder().decode(header), 'BPRE');
header[0] = 0;
assert.equal(emu.readMemory(0x080000ac, 1)[0], 66, 'memory finder receives a copy');
assert.equal(emu.readMemory(0x02000000, 0x40000).length, 0x40000);
assert.equal(emu.readMemory(0x03000000, 0x8000).length, 0x8000);
for (const [address, length] of [[0x04000000, 4], [0x0203ffff, 2], [0x03008000, 1], [0x02000000, 0x40001], [-1, 4]]) {
  assert.throws(() => emu.readMemory(address, length), 'IO, invalid and out-of-bounds memory must be rejected');
}
assert.deepEqual(emu.saveState(), initialState, 'memory searches do not alter the machine');
const reader = id => ({ manifest_version: 2, addon_id: id, display_name: id,
  matches: { game_code: ['BPRE'], revision: [0] },
  sections: [{ id: 'header', title: 'Header', kind: 'key_value', fields: [{ label: 'First code byte', read: { u8: '0x080000AC' } }] }],
});
const readers = [reader('reader.one'), reader('reader.two')];
assert.equal(emu.installManifests(readers), 2);
let snapshot = emu.snapshot();
assert.equal(snapshot.addon.addon_id, base.addon.addon_id, 'built-in reader remains active');
assert.equal(snapshot.addon.sections.length, base.addon.sections.length + 2);
assert.equal(new Set(snapshot.addon.sections.map(s => s.section_id)).size, snapshot.addon.sections.length);
assert.equal(snapshot.addon.sections.at(-1).payload[0].value, '66');
assert.deepEqual(snapshot.community_addons.map(a => a.status), ['active', 'active']);
assert.deepEqual(emu.saveState(), initialState, 'readers and previews cannot alter emulation');
const bad = reader('bad'); bad.sections[0].fields[0].read.u8 = '0x04000000';
assert.throws(() => emu.installManifests([bad]), /RAM or cartridge ROM/);
assert.equal(emu.snapshot().community_addons.length, 2, 'failed replacement keeps active set');
assert.throws(() => emu.installManifests([readers[0], readers[0]]), /unique/);
const incompatible = reader('wrong'); incompatible.matches.revision = [255];
emu.installManifests([incompatible]);
assert.equal(emu.snapshot().community_addons[0].status, 'incompatible');
const idle = reader('idle'); idle.when = { read: { u8: '0x080000AC' }, equals: 0 };
emu.installManifests([idle]); assert.equal(emu.snapshot().community_addons[0].status, 'idle');
for (let i = 0; i < 50; i++) { emu.installManifests(readers); emu.snapshot(); }
const warmMemory = emu.memoryBytes;
for (let i = 0; i < 500; i++) { emu.installManifests(readers); emu.snapshot(); }
assert.ok(emu.memoryBytes <= warmMemory + 65536, 'repeated edits and snapshots must not leak');
assert.equal(emu.installManifests([]), 0);
assert.deepEqual(emu.snapshot().addon.sections, base.addon.sections);
const builtins = emu.snapshot().builtin_addons;
assert.equal(builtins.length, 5, 'manager lists every preinstalled reader');
emu.setDisabledBuiltins(['pokemon_frlg_party']);
assert.equal(emu.snapshot().addon.addon_id, 'cartridge', 'disabled game reader falls back to cartridge details');
emu.setDisabledBuiltins(builtins.map(info => info.addon_id));
assert.equal(emu.snapshot().addon, undefined, 'all preinstalled readers can be disabled');
emu.installManifests(readers);
assert.equal(emu.snapshot().addon.sections.length, 2, 'community readers still work without built-ins');
assert.throws(() => emu.setDisabledBuiltins(['unknown']));
assert.equal(emu.snapshot().addon.sections.length, 2, 'invalid settings do not change enabled readers');
emu.installManifests([]); emu.setDisabledBuiltins([]);
assert.deepEqual(emu.snapshot().addon.sections, base.addon.sections, 'preinstalled readers can be restored');
assert.deepEqual(emu.saveState(), initialState, 'reader preferences never alter game state');
const sheet = builtinSheet(base.rom);
assert.equal(emu.installManifests([sheet]), 1);
let sheetFields = emu.snapshot().addon.sections.at(-1).payload;
assert.equal(sheetFields.find(field => field.label === 'Level').value, String(emu.readMemory(0x02024284 + 84, 1)[0]));
const textSheet = memorySheet({ ...reader('sheet.text'), sections: [{ id: 'text', title: 'Header', kind: 'key_value', fields: [{ label: 'Code', read: { text: { at: '0x080000AC', len: 4 } }, hint: 'ASCII cartridge header' }] }] });
emu.installManifests([textSheet]);
assert.equal(emu.snapshot().addon.sections.at(-1).payload[0].value, 'BPRE');
assert.deepEqual(emu.saveState(), initialState, 'shared sheets and text readers are read-only');
emu.installManifests([]);

// The Generation 3 reads, against the real cartridge rather than a fixture.
// Species is encrypted and permuted by personality, and its name comes from a
// table found by scanning the ROM — neither is provable with planted bytes.
const party = partyTemplate(base.rom);
assert.equal(emu.installManifests([party]), 1);
const partySection = emu.snapshot().addon.sections.at(-1);
assert.equal(partySection.kind, 'cards');
assert.ok(partySection.payload.length > 0, 'the loaded save has a party');

const first = partySection.payload[0];
// A name, not "#21": the cartridge's own species table was found and read.
assert.match(first.title, /^[A-Z][a-z]/, `species should be named, got ${first.title}`);
assert.match(first.image.src, /^\/sprites\/\d+$/);
// The heading comes from the encrypted block; the bar beside it comes from the
// plain tail of the same record. That pairing is the whole point of the read.
assert.deepEqual(first.lead.meter,
  { value: emu.readMemory(0x02024284 + 86, 2)[0] | (emu.readMemory(0x02024284 + 86, 2)[1] << 8),
    max: emu.readMemory(0x02024284 + 88, 2)[0] | (emu.readMemory(0x02024284 + 88, 2)[1] << 8) });
assert.ok(['good', 'warn', 'bad'].includes(first.lead.tone), 'a bar carries a colour');
assert.equal(first.fields.find(field => field.label === 'Level').value,
  String(emu.readMemory(0x02024284 + 84, 1)[0]));
// Empty slots are absent rather than drawn blank, so the count is the party.
assert.ok(partySection.payload.length <= 6);
assert.deepEqual(emu.saveState(), initialState, 'decrypting a record never alters the machine');

// The encrypted half of a record, against the real cartridge. This is the part
// no offset can reach: the block is XOR-encrypted and permuted per Pokemon, so
// a passing assertion here means the decrypt, the permutation and the ROM name
// tables all agreed with the compiled reader that reads the same bytes.
const detail = (label, field, extra = {}) => ({ label, read: { gen3: { at: '0x02024284', field } }, ...extra });
const deep = {
  manifest_version: 2, addon_id: 'reader.detail', display_name: 'Detail',
  matches: { game_code: ['BPRE'], revision: [0] },
  sections: [{
    id: 'party', title: 'Party', kind: 'cards',
    repeat: { count: 6, stride: 100 },
    card: {
      title: { gen3: { at: '0x02024284', field: 'species' } },
      fields: [
        detail('Move 1', 'move1'), detail('Move 2', 'move2'),
        detail('PP 1', 'pp1'),
        detail('Nature', 'nature'), detail('Shiny', 'is_shiny'), detail('Held item', 'held_item'),
        detail('IV total', 'iv_total', { max: { const: 186 } }),
        detail('EV total', 'ev_total', { max: { const: 510 } }),
        detail('IV Atk', 'iv_attack', { max: { const: 31 } }),
        detail('EV Atk', 'ev_attack', { max: { const: 252 } }),
      ],
    },
  }],
};
assert.equal(emu.installManifests([deep]), 1);
const deepSection = emu.snapshot().addon.sections.at(-1);
const slot = deepSection.payload[0];
const detailOf = label => slot.fields.find(field => field.label === label);

// Move names come from tables found by scanning the cartridge, so this fails
// if either the decrypt or the table search regressed.
assert.equal(detailOf('Move 1').value, 'Leer');
assert.equal(detailOf('Move 2').value, 'Peck');
assert.equal(detailOf('PP 1').value, '30');
// Nature is not stored anywhere; it is the personality mod 25.
assert.equal(detailOf('Nature').value, 'Adamant');
// A flag reads as a word rather than as a bare 1.
assert.equal(detailOf('Shiny').value, 'No');
// An empty slot is a state, not a zero to be shown as an item called "#0".
assert.equal(detailOf('Held item').value, '—');

// A constant maximum gives a bar to a value whose ceiling is a rule.
assert.deepEqual(detailOf('IV total').meter, { value: 91, max: 186 });
assert.deepEqual(detailOf('EV total').meter, { value: 17, max: 510 });
assert.deepEqual(detailOf('IV Atk').meter, { value: 6, max: 31 });
assert.deepEqual(detailOf('EV Atk').meter, { value: 3, max: 252 });
assert.equal(detailOf('EV Atk').tone, 'warn', 'a quarter-full-or-less bar reads as warn; only an empty one is bad');

// Every card decrypts its own record and no other, so a party of six with ten
// decrypted fields each still resolves every slot independently.
assert.equal(deepSection.payload.length, 6);
assert.deepEqual(deepSection.payload.map(card => card.title),
  ['Nidoran♂', 'Ivysaur', 'Paras', 'Spearow', 'Clefairy', 'Mankey']);
assert.equal(deepSection.payload[1].fields.find(f => f.label === 'Move 1').value, 'Tackle');
assert.deepEqual(emu.saveState(), initialState, 'decrypting a whole party never alters the machine');
emu.installManifests([]);

// The same reader on a cartridge it does not claim stays quiet rather than
// reporting whatever those addresses happen to hold.
const elsewhere = structuredClone(party); elsewhere.matches = { game_code: ['BPEE'], revision: [0] };
emu.installManifests([elsewhere]);
assert.equal(emu.snapshot().community_addons[0].status, 'incompatible');

emu.installManifests([]);
emu.runFrame(); assert.ok(emu.frameCount > 0);
console.log('PASS: composition, read-only state, atomic replacement, compatibility, readiness, bounded memory, Gen 3 decoding of the encrypted block, and removal.');

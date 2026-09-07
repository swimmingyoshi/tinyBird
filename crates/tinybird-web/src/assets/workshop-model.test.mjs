import test from 'node:test';
import assert from 'node:assert/strict';
import { scanMemory, readNumber, numberValue, appendField, starterManifest, searchPattern, scanPattern,
  fieldSpec, fieldForm, addSection, updateSection, removeSection, moveSection,
  addField, updateField, removeField, moveField, setLead, sectionFields, partyTemplate } from './workshop-model.js';
import { memorySheet, sheetRows, builtinSheet } from './memory-sheets.js';

test('text and wildcard scans support case folding, overlapping matches and narrowing', () => {
  const bytes = new TextEncoder().encode('HP hp Hx abaBA');
  assert.deepEqual(scanPattern(bytes, searchPattern('HP', 'text')), [0]);
  assert.deepEqual(scanPattern(bytes, searchPattern('HP', 'text'), true), [0, 3]);
  assert.deepEqual(scanPattern(bytes, searchPattern('48 ??', 'pattern')), [0, 6]);
  assert.deepEqual(scanPattern(bytes, searchPattern('aba', 'text'), true), [9, 11]);
  assert.deepEqual(scanPattern(bytes, searchPattern('hp', 'text'), true, [3]), [3]);
  assert.deepEqual(scanPattern(bytes, searchPattern('HP', 'text'), false, []), []);
  for (const text of ['', '?? ??', '0G', '123']) assert.throws(() => searchPattern(text, 'pattern'));
  assert.throws(() => searchPattern('é', 'text'));
});
test('ranges and unaligned reads find values ordinary aligned scans miss', () => {
  const bytes = new Uint8Array([0, 15, 0, 0]);
  assert.deepEqual(scanMemory(bytes, 'u16', 'range', [10, 20]), []);
  assert.deepEqual(scanMemory(bytes, 'u16', 'range', [10, 20], null, null, true), [1]);
});
test('sheets preserve runnable read definitions, source explanations and notes', () => {
  const draft = starterManifest({ game_code: 'BPRE', revision: 0 });
  const sheet = memorySheet(appendField(draft, 'Game code', 'text', 0x080000ac, 4, 'Header ASCII'));
  assert.equal(sheet.$comment.kind, 'memory_sheet');
  assert.equal(sheetRows(sheet)[0].notes, 'Header ASCII');
  assert.match(sheetRows(sheet)[0].read, /ASCII text \(4 bytes\)/);
  assert.deepEqual(sheet.sections[0].fields[0].read, { text: { at: '0x080000AC', len: 4 } });
  assert.throws(() => appendField(draft, 'Bad', 'text', 0x03007fff, 4));
  assert.throws(() => appendField(draft, 'Bad', 'text', 0x02000000, 65));
  const example = builtinSheet({ game_code: 'BPRE', revision: 0, title: 'FireRed' });
  assert.deepEqual(example.sections[0].fields[2].read, { u16: '0x20242DA' });
  assert.throws(() => builtinSheet({ game_code: 'BPRE', revision: 1 }));
});

test('find a visible value, play, narrow changes and make a compatible reader', () => {
  const before = new Uint8Array([100, 0, 100, 0, 100, 0, 1, 0]);
  const first = scanMemory(before, 'u16', 'equal', 100);
  assert.deepEqual(first, [0, 2, 4]);
  const after = new Uint8Array([100, 0, 75, 0, 101, 0, 0, 0]);
  const narrowed = scanMemory(after, 'u16', 'decreased', 0, before, first);
  assert.deepEqual(narrowed, [2]);
  const draft = starterManifest({ game_code: 'BPEE', revision: 0 });
  const reader = appendField(draft, 'HP', 'u16', 0x02000000 + narrowed[0]);
  assert.deepEqual(reader.matches, { game_code: ['BPEE'], revision: [0] });
  assert.deepEqual(reader.sections[0].fields, [{ label: 'HP', read: { u16: '0x02000002' } }]);
  assert.equal(draft.sections[0].fields.length, 0);
});
test('unknown search and comparisons retain candidates without widening after zero matches', () => {
  const before = new Uint8Array([1, 2, 3, 4]);
  const after = new Uint8Array([1, 9, 0, 4]);
  assert.deepEqual(scanMemory(before, 'u8', 'unknown', 0), [0, 1, 2, 3]);
  assert.deepEqual(scanMemory(after, 'u8', 'changed', 0, before), [1, 2]);
  assert.deepEqual(scanMemory(after, 'u8', 'unchanged', 0, before), [0, 3]);
  assert.deepEqual(scanMemory(after, 'u8', 'increased', 0, before), [1]);
  assert.deepEqual(scanMemory(after, 'u8', 'equal', 9, before, []), []);
  assert.throws(() => scanMemory(after, 'u8', 'changed', 0), /Start a search/);
});
test('unsigned widths, endian order, input errors and address bounds', () => {
  assert.equal(readNumber(new Uint8Array([255, 255, 255, 255]), 0, 'u32'), 4294967295);
  assert.equal(numberValue('0xFFFF', 'u16'), 65535);
  for (const text of ['', '-1', '1.5', '256', '1e2']) assert.throws(() => numberValue(text, 'u8'));
  const draft = starterManifest({ game_code: 'BPRE', revision: 1 });
  for (const address of [0x04000000, 0x0203ffff, 0x03008000, NaN]) assert.throws(() => appendField(draft, 'Test', 'u32', address));
  assert.throws(() => appendField(draft, '', 'u8', 0x02000000));
  assert.deepEqual(scanMemory(new Uint8Array([1, 0, 1]), 'u16', 'equal', 1), [0]);
});

test('a tracker type becomes the manifest read it stands for, and reads back', () => {
  const bar = fieldSpec({ label: 'HP', kind: 'bar', address: 0x020242da, max: 0x020242dc, size: 'u16' });
  // A bar is a number plus a maximum. That pairing is the whole of what makes
  // the renderer draw a meter and colour it, so it must survive a round trip.
  assert.deepEqual(bar, { label: 'HP', read: { u16: '0x020242DA' }, max: { u16: '0x020242DC' } });
  assert.equal(fieldForm(bar).kind, 'bar');
  assert.equal(fieldForm(bar).max, 0x020242dc);

  assert.deepEqual(fieldSpec({ label: 'Name', kind: 'gen3_text', address: 0x0202428c, length: 10 }).read,
    { gen3_text: { at: '0x0202428C', len: 10 } });
  assert.deepEqual(fieldSpec({ label: 'Species', kind: 'gen3_species', address: 0x02024284 }).read,
    { gen3_species: '0x02024284' });
  assert.deepEqual(fieldSpec({ label: 'Slot', kind: 'index' }).read, { index: null });
  assert.equal(fieldForm({ label: 'Slot', read: 'index' }).kind, 'index');

  // A species read needs eighty bytes, so one near the end of EWRAM is refused
  // here for the same reason the Rust validator refuses it.
  assert.throws(() => fieldSpec({ label: 'Species', kind: 'gen3_species', address: 0x0203fff0 }));
  assert.throws(() => fieldSpec({ label: 'HP', kind: 'bar', address: 0x02000000 }), /maximum/);
  assert.throws(() => fieldSpec({ label: '', kind: 'u16', address: 0x02000000 }));
  assert.throws(() => fieldSpec({ label: 'X', kind: 'nonsense', address: 0x02000000 }));
});

test('categories nest fields, and a repeating one describes a party once', () => {
  let draft = starterManifest({ game_code: 'BPRE', revision: 0 });
  draft = addSection(draft, { title: 'Party', kind: 'cards', count: 6, stride: 100 });
  const party = 1;
  assert.deepEqual(draft.sections[party].repeat, { count: 6, stride: 100 });
  assert.equal(draft.sections[party].id, 'party');

  draft = addField(draft, party, { label: 'HP', kind: 'bar', address: 0x020242da, max: 0x020242dc });
  draft = addField(draft, party, { label: 'Attack', kind: 'u16', address: 0x020242de });
  assert.equal(sectionFields(draft.sections[party]).length, 2);
  // Fields in a repeating category live on the card, not on the section.
  assert.equal(draft.sections[party].fields, undefined);

  // The headline stat is one of the fields, promoted — so demoting it puts it
  // back rather than losing it.
  draft = setLead(draft, party, 0);
  assert.equal(draft.sections[party].card.lead.label, 'HP');
  assert.equal(sectionFields(draft.sections[party]).length, 1);
  draft = setLead(draft, party, null);
  assert.equal(draft.sections[party].card.lead, undefined);
  assert.equal(sectionFields(draft.sections[party])[0].label, 'HP');

  // The section's own name and the card's heading are both "title", which is
  // exactly why the card's live under `card`.
  draft = updateSection(draft, party, {
    title: 'My party',
    card: {
      title: { kind: 'gen3_species', address: 0x02024284 },
      subtitle: { kind: 'gen3_text', address: 0x0202428c, length: 10 },
      image: 0x02024284,
    },
  });
  assert.equal(draft.sections[party].title, 'My party');
  assert.deepEqual(draft.sections[party].card.title, { gen3_species: '0x02024284' });
  assert.deepEqual(draft.sections[party].card.image, { gen3_species_sprite: '0x02024284' });
  // Clearing the heading falls back to the slot number rather than to nothing.
  assert.deepEqual(updateSection(draft, party, { card: { title: null, image: null } }).sections[party].card,
    { title: { index: null }, subtitle: { gen3_text: { at: '0x0202428C', len: 10 } }, fields: draft.sections[party].card.fields });

  assert.throws(() => addField(draft, 99, { label: 'X', kind: 'u16', address: 0x02000000 }), /category/);
  assert.throws(() => updateSection(draft, party, { repeat: { count: 0 } }));
  // Two categories named the same still get distinct ids, because the reader
  // rejects a manifest whose section ids collide.
  const twice = addSection(addSection(draft, { title: 'Bag' }), { title: 'Bag' });
  assert.equal(new Set(twice.sections.map(s => s.id)).size, twice.sections.length);
});

test('fields move between categories, and an impossible drop changes nothing', () => {
  let draft = starterManifest({ game_code: 'BPRE', revision: 0 });
  draft = addSection(draft, { title: 'Party', kind: 'cards' });
  draft = addField(draft, 0, { label: 'Money', kind: 'u32', address: 0x02000100 });
  draft = addField(draft, 0, { label: 'Badges', kind: 'u8', address: 0x02000110 });

  const moved = moveField(draft, { section: 0, field: 0 }, { section: 1, field: 0 });
  assert.deepEqual(sectionFields(moved.sections[0]).map(f => f.label), ['Badges']);
  assert.deepEqual(sectionFields(moved.sections[1]).map(f => f.label), ['Money']);

  // Reordering inside one category is the same call.
  const reordered = moveField(draft, { section: 0, field: 1 }, { section: 0, field: 0 });
  assert.deepEqual(sectionFields(reordered.sections[0]).map(f => f.label), ['Badges', 'Money']);

  // A drop on a category that is gone leaves the draft exactly as it was,
  // rather than dropping the field on the floor.
  assert.equal(moveField(draft, { section: 0, field: 0 }, { section: 9, field: 0 }), draft);
  assert.equal(moveField(draft, { section: 0, field: 7 }, { section: 1, field: 0 }), draft);
  assert.deepEqual(moveSection(draft, 1, 0).sections.map(s => s.id), ['party', 'stats']);

  draft = updateField(draft, 0, 0, { label: 'Coins', kind: 'u32', address: 0x02000100 });
  assert.equal(sectionFields(draft.sections[0])[0].label, 'Coins');
  draft = removeField(draft, 0, 0);
  assert.deepEqual(sectionFields(draft.sections[0]).map(f => f.label), ['Badges']);
  assert.throws(() => removeSection(starterManifest({ game_code: 'BPRE', revision: 0 }), 0), /at least one/);
});

test('the party template pairs a decrypted name with a stat from the plain tail', () => {
  const party = partyTemplate({ game_code: 'BPRE', revision: 0, title: 'POKEMON FIRE' });
  const card = party.sections[0].card;
  // The species and the nickname come out of the encrypted and the custom-
  // alphabet parts respectively; the HP bar beside them is a plain u16 pair.
  assert.deepEqual(card.title, { gen3_species: '0x02024284' });
  assert.deepEqual(card.subtitle, { gen3_text: { at: '0x0202428C', len: 10 } });
  assert.deepEqual(card.image, { gen3_species_sprite: '0x02024284' });
  assert.deepEqual(card.lead.max, { u16: '0x020242DC' });
  assert.deepEqual(party.sections[0].repeat, { count: 6, stride: 100 });
  assert.throws(() => partyTemplate({ game_code: 'AGBJ', revision: 0, title: 'X' }), /FireRed/);
});

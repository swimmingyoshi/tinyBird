export const WIDTHS = { u8: 1, u16: 2, u32: 4 };
export const REGIONS = { ewram: [0x02000000, 0x40000], iwram: [0x03000000, 0x8000] };
export const hex = value => `0x${value.toString(16).toUpperCase().padStart(8, '0')}`;
export function numberValue(text, type) {
  if (!/^(0x[\da-f]+|\d+)$/i.test(text.trim())) throw new Error('Enter a whole number, such as 100 or 0x64.');
  const value = Number(text);
  if (!WIDTHS[type] || !Number.isInteger(value) || value < 0 || value >= 2 ** (WIDTHS[type] * 8)) throw new Error('That value does not fit the selected number type.');
  return value;
}
export function readNumber(bytes, offset, type) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return type === 'u8' ? view.getUint8(offset) : type === 'u16' ? view.getUint16(offset, true) : view.getUint32(offset, true);
}
/** Scan only on explicit user action; retain all matches but render a small page. */
export function scanMemory(bytes, type, mode, target, previous = null, candidates = null, unaligned = false) {
  const width = WIDTHS[type];
  if (!width || !['equal', 'range', 'unknown', 'changed', 'unchanged', 'increased', 'decreased'].includes(mode)) throw new Error('Choose a supported scan.');
  if (!['equal', 'range', 'unknown'].includes(mode) && (!previous || previous.length !== bytes.length)) throw new Error('Start a search before comparing changes.');
  const found = [];
  const check = offset => {
    if (offset < 0 || offset + width > bytes.length) return;
    const value = readNumber(bytes, offset, type);
    const old = previous ? readNumber(previous, offset, type) : null;
    if (mode === 'unknown' || (mode === 'equal' && value === target) ||
        (mode === 'range' && value >= target[0] && value <= target[1]) ||
        (mode === 'changed' && value !== old) || (mode === 'unchanged' && value === old) ||
        (mode === 'increased' && value > old) || (mode === 'decreased' && value < old)) found.push(offset);
  };
  if (candidates) candidates.forEach(check);
  else for (let offset = 0; offset + width <= bytes.length; offset += unaligned ? 1 : width) check(offset);
  return found;
}
export function starterManifest(rom) {
  return { manifest_version: 2, addon_id: `reader.${crypto.randomUUID()}`, display_name: 'My game reader', version: '0.1.0',
    matches: { game_code: [rom.game_code], revision: [rom.revision] },
    sections: [{ id: 'stats', title: 'Stats', kind: 'key_value', fields: [] }] };
}

/* -- What a field can be -------------------------------------------------
 *
 * These are the "tracker types" the builder offers, and every one of them is
 * something the manifest format already understood — the builder simply never
 * let anybody reach past a plain number. `bar` is the interesting one: giving
 * a field a `max` is what makes the renderer draw a meter *and* colour it,
 * because tone is derived from the fraction rather than chosen. So "a health
 * bar that goes red" is not a separate feature, it is a field with a maximum.
 */
export const FIELD_KINDS = {
  u8: { label: '8-bit number', address: true },
  u16: { label: '16-bit number', address: true },
  u32: { label: '32-bit number', address: true },
  bar: { label: 'Bar (value out of a maximum)', address: true, max: true, size: true },
  text: { label: 'ASCII text', address: true, length: true },
  gen3_text: { label: 'Pokémon name text (Gen 3 alphabet)', address: true, length: true },
  gen3_species: { label: 'Pokémon species (decrypted)', address: true, record: true },
  literal: { label: 'Fixed text', address: false },
  index: { label: 'Slot number', address: false },
};

/** Bytes a read of this kind touches, for the bounds check. */
function readWidth(kind, { length = 16, size = 'u16' } = {}) {
  if (kind === 'gen3_species') return 80;
  if (kind === 'text' || kind === 'gen3_text') return length;
  if (kind === 'bar') return WIDTHS[size];
  return WIDTHS[kind] ?? 0;
}

const RANGES = [[0x02000000, 0x02040000], [0x03000000, 0x03008000], [0x08000000, 0x0a000000]];
function checkAddress(address, width) {
  if (!Number.isInteger(address) || !RANGES.some(([lo, hi]) => address >= lo && address + width <= hi)) {
    throw new Error('Choose an address inside game RAM or ROM.');
  }
}

/**
 * The manifest `read` object for one tracker type.
 *
 * `bar` has no read of its own — it is a numeric read plus a `max`, which
 * [`fieldSpec`] assembles. Everything else maps one to one.
 */
export function readSpec(kind, { address = 0, length = 16, size = 'u16', literal = '' } = {}) {
  if (kind === 'index') return { index: null };
  if (kind === 'literal') {
    const text = literal.trim();
    if (!text || text.length > 256) throw new Error('Fixed text needs 1–256 characters.');
    return { literal: text };
  }
  const width = readWidth(kind, { length, size });
  checkAddress(address, width);
  if (kind === 'text' || kind === 'gen3_text') {
    if (!Number.isInteger(length) || length < 1 || length > 64) throw new Error('Text reads must be 1–64 bytes.');
    return { [kind]: { at: hex(address), len: length } };
  }
  if (kind === 'gen3_species') return { gen3_species: hex(address) };
  if (kind === 'bar') return { [size]: hex(address) };
  return { [kind]: hex(address) };
}

/**
 * A complete field, validated. `max` only appears for a bar, which is what
 * earns the field its meter and its colour.
 */
export function fieldSpec({ label, kind, address = 0, length = 16, size = 'u16', literal = '', max = null, hint = '' }) {
  if (!FIELD_KINDS[kind]) throw new Error('Choose a supported tracker type.');
  if (!label.trim() || label.length > 120) throw new Error('Give your field a label (up to 120 characters).');
  if (hint.length > 256) throw new Error('Keep field notes to 256 characters.');
  const field = { label: label.trim(), read: readSpec(kind, { address, length, size, literal }) };
  if (kind === 'bar') {
    if (!Number.isInteger(max)) throw new Error('A bar needs the address that holds its maximum, such as max HP.');
    checkAddress(max, WIDTHS[size]);
    field.max = { [size]: hex(max) };
  }
  if (hint.trim()) field.hint = hint.trim();
  return field;
}

/** Read a field back into the shape the editor works in. */
export function fieldForm(field) {
  // Serde writes a unit variant as a bare string but accepts the object form,
  // so a manifest that has been through the Rust side comes back as "index".
  const read = typeof field.read === 'string' ? { [field.read]: null } : field.read;
  const [name, value] = Object.entries(read ?? {})[0] ?? [];
  const at = text => (typeof text === 'string' ? Number(text) : 0);
  const form = { label: field.label ?? '', hint: field.hint ?? '', kind: name ?? 'u16', address: 0, length: 16, size: 'u16', literal: '', max: null };
  if (name === 'literal') form.literal = value ?? '';
  else if (name === 'index') form.kind = 'index';
  else if (name === 'text' || name === 'gen3_text') { form.address = at(value?.at); form.length = value?.len ?? 16; }
  else if (name === 'gen3_species') form.address = at(value);
  else if (WIDTHS[name]) {
    form.address = at(value);
    // A numeric read carrying a maximum *is* a bar; the editor should open it
    // as one rather than as a number with a mysterious extra key.
    const [maxName, maxValue] = Object.entries(field.max ?? {})[0] ?? [];
    if (WIDTHS[maxName]) { form.kind = 'bar'; form.size = name; form.max = at(maxValue); }
  }
  return form;
}

/* -- Categories ----------------------------------------------------------
 *
 * A section is a category, and a `cards` section is a category with one entry
 * per repeat — which is how "Party → Slot 1 → HP, Attack" is expressed. The
 * builder edits *one* card and the reader draws `count` of them, stepping the
 * addresses by `stride`, so a party of six is described once.
 */
export const MAX_SECTIONS = 8;
export const MAX_FIELDS = 32;

function slug(title, taken) {
  const base = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || 'section';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

export function addSection(manifest, { title, kind = 'key_value', count = 6, stride = 100 }) {
  if (!title.trim() || title.length > 120) throw new Error('Give the category a name (up to 120 characters).');
  const copy = structuredClone(manifest);
  if (copy.sections.length >= MAX_SECTIONS) throw new Error(`A reader holds at most ${MAX_SECTIONS} categories.`);
  const id = slug(title, copy.sections.map(section => section.id));
  if (kind === 'cards') {
    if (!Number.isInteger(count) || count < 1 || count > 64) throw new Error('A repeating category needs 1–64 entries.');
    if (!Number.isInteger(stride) || stride < 1 || stride > 0x10000) throw new Error('The step between entries must be 1–65,536 bytes.');
    copy.sections.push({ id, title: title.trim(), kind: 'cards', repeat: { count, stride },
      card: { title: { index: null }, fields: [] } });
  } else {
    copy.sections.push({ id, title: title.trim(), kind: 'key_value', fields: [] });
  }
  return copy;
}

/** The list a section's fields live in, whichever kind it is. */
function fieldsOf(section) {
  if (section.kind === 'cards') {
    section.card ??= { title: { index: null }, fields: [] };
    section.card.fields ??= [];
    return section.card.fields;
  }
  section.fields ??= [];
  return section.fields;
}

export function sectionFields(section) {
  return (section.kind === 'cards' ? section.card?.fields : section.fields) ?? [];
}

export function updateSection(manifest, index, patch) {
  const copy = structuredClone(manifest);
  const section = copy.sections[index];
  if (!section) throw new Error('That category is no longer there.');
  if (patch.title !== undefined) {
    if (!patch.title.trim() || patch.title.length > 120) throw new Error('Give the category a name (up to 120 characters).');
    section.title = patch.title.trim();
  }
  if (patch.note !== undefined) {
    const note = patch.note.trim();
    if (note.length > 512) throw new Error('Keep category notes to 512 characters.');
    if (note) section.note = note; else delete section.note;
  }
  if (patch.repeat && section.kind === 'cards') {
    const { count = section.repeat.count, stride = section.repeat.stride } = patch.repeat;
    if (!Number.isInteger(count) || count < 1 || count > 64) throw new Error('A repeating category needs 1–64 entries.');
    if (!Number.isInteger(stride) || stride < 1 || stride > 0x10000) throw new Error('The step between entries must be 1–65,536 bytes.');
    section.repeat = { count, stride };
  }
  // The heading, the second line and the picture on each card. Kept under
  // `card` because a section's own `title` is a plain string and a card's is a
  // read — one `title` key meaning both is how they get confused.
  //
  // All three are optional, and clearing one takes the key out rather than
  // leaving an empty read behind. A card with no heading falls back to the
  // slot number, which is the one thing every repeat always has.
  for (const slot of ['title', 'subtitle', 'image']) {
    const value = patch.card?.[slot];
    if (value === undefined || section.kind !== 'cards') continue;
    if (value === null) {
      if (slot === 'title') section.card.title = { index: null }; else delete section.card[slot];
    } else if (slot === 'image') {
      checkAddress(value, 80);
      section.card.image = { gen3_species_sprite: hex(value) };
    } else {
      section.card[slot] = readSpec(value.kind, value);
    }
  }
  return copy;
}

export function removeSection(manifest, index) {
  const copy = structuredClone(manifest);
  if (copy.sections.length <= 1) throw new Error('A reader needs at least one category.');
  copy.sections.splice(index, 1);
  return copy;
}

/**
 * Add a field to a named category, creating nothing implicitly.
 *
 * The old builder guessed which section a field belonged in, which is why
 * every field landed in one flat list. Now the category is the caller's.
 */
export function addField(manifest, sectionIndex, form) {
  const copy = structuredClone(manifest);
  const section = copy.sections[sectionIndex];
  if (!section) throw new Error('Choose a category for this field.');
  const fields = fieldsOf(section);
  if (fields.length >= MAX_FIELDS) throw new Error(`A category holds at most ${MAX_FIELDS} fields.`);
  fields.push(fieldSpec(form));
  return copy;
}

export function updateField(manifest, sectionIndex, fieldIndex, form) {
  const copy = structuredClone(manifest);
  const fields = fieldsOf(copy.sections[sectionIndex] ?? {});
  if (!fields[fieldIndex]) throw new Error('That field is no longer there.');
  fields[fieldIndex] = fieldSpec(form);
  return copy;
}

export function removeField(manifest, sectionIndex, fieldIndex) {
  const copy = structuredClone(manifest);
  fieldsOf(copy.sections[sectionIndex] ?? {}).splice(fieldIndex, 1);
  return copy;
}

/** Promote a field to the card's headline stat, or demote the one there. */
export function setLead(manifest, sectionIndex, fieldIndex) {
  const copy = structuredClone(manifest);
  const section = copy.sections[sectionIndex];
  if (section?.kind !== 'cards') throw new Error('Only a repeating category has a headline stat.');
  const fields = fieldsOf(section);
  if (fieldIndex === null) {
    if (!section.card.lead) return copy;
    if (fields.length >= MAX_FIELDS) throw new Error(`A category holds at most ${MAX_FIELDS} fields.`);
    fields.unshift(section.card.lead);
    delete section.card.lead;
    return copy;
  }
  const [field] = fields.splice(fieldIndex, 1);
  if (!field) throw new Error('That field is no longer there.');
  // Whatever was the headline goes back into the list rather than vanishing.
  if (section.card.lead) fields.unshift(section.card.lead);
  section.card.lead = field;
  return copy;
}

/**
 * Move a field, possibly into another category. This is what the drag handles
 * call, and it is deliberately the only way order changes: a drop that lands
 * somewhere impossible leaves the draft exactly as it was.
 */
export function moveField(manifest, from, to) {
  const copy = structuredClone(manifest);
  const source = copy.sections[from.section];
  const target = copy.sections[to.section];
  if (!source || !target) return manifest;
  const out = fieldsOf(source);
  const [field] = out.splice(from.field, 1);
  if (!field) return manifest;
  const into = fieldsOf(target);
  if (source !== target && into.length >= MAX_FIELDS) throw new Error(`A category holds at most ${MAX_FIELDS} fields.`);
  into.splice(Math.max(0, Math.min(to.field, into.length)), 0, field);
  return copy;
}

export function moveSection(manifest, from, to) {
  const copy = structuredClone(manifest);
  const [section] = copy.sections.splice(from, 1);
  if (!section) return manifest;
  copy.sections.splice(Math.max(0, Math.min(to, copy.sections.length)), 0, section);
  return copy;
}

/**
 * The original one-call "add a field somewhere sensible".
 *
 * Kept because saved drafts, the worked example and the memory-sheet helpers
 * all go through it, and because the quickest path from a search result to a
 * visible number should stay one click.
 */
export function appendField(manifest, label, type, address, textLength = 16, hint = '') {
  const copy = structuredClone(manifest);
  let index = copy.sections.findIndex(item => item.id === 'workshop' && item.kind === 'key_value');
  if (index < 0) index = copy.sections.findIndex(item => item.kind === 'key_value' && Array.isArray(item.fields) && item.fields.length < MAX_FIELDS);
  if (index < 0) {
    if (copy.sections.length >= MAX_SECTIONS) throw new Error('This reader already has eight sections. Edit an existing section in JSON.');
    copy.sections.push({ id: 'workshop', title: 'My fields', kind: 'key_value', fields: [] });
    index = copy.sections.length - 1;
  }
  if (copy.sections[index].fields.length >= MAX_FIELDS) throw new Error('This section is full. Start another section in the JSON editor.');
  return addField(copy, index, { label, kind: type, address, length: textLength, hint });
}

export function searchPattern(query, kind) {
  if (kind === 'text') {
    if (!/^[\x20-\x7e]{1,64}$/.test(query)) throw new Error('Enter 1–64 ASCII characters. Custom game alphabets need a byte pattern.');
    return Array.from(query, char => char.charCodeAt(0));
  }
  const parts = query.trim().split(/\s+/);
  if (parts.length > 64 || !parts.every(part => /^(?:[\da-f]{2}|\?\?)$/i.test(part)) || parts.every(part => part === '??')) throw new Error('Use up to 64 hex bytes, with ?? for wildcards. Example: 0F ?? 2A 00.');
  return parts.map(part => part === '??' ? null : parseInt(part, 16));
}
export function scanPattern(bytes, pattern, ignoreCase = false, candidates = null) {
  const fold = value => ignoreCase && value >= 65 && value <= 90 ? value + 32 : value;
  const matches = offset => offset >= 0 && offset + pattern.length <= bytes.length && pattern.every((value, index) => value === null || fold(bytes[offset + index]) === fold(value));
  const result = [];
  if (candidates) { for (const offset of candidates) if (matches(offset)) result.push(offset); }
  else for (let offset = 0; offset + pattern.length <= bytes.length; offset++) if (matches(offset)) result.push(offset);
  return result;
}

/* -- The worked example --------------------------------------------------
 *
 * A Generation 3 party, described the way the format wants: one repeating
 * category, one card, six entries a hundred bytes apart. It is the shortest
 * demonstration of every part the builder now exposes — a decrypted species as
 * the heading, the nickname under it, a sprite, a coloured HP bar, and plain
 * numbers for the rest.
 */
export const PARTY_BASE = { BPRE: 0x02024284, BPEE: 0x020244ec };
export function partyTemplate(rom) {
  const base = PARTY_BASE[rom.game_code];
  if (!base) throw new Error('The party template covers FireRed (BPRE) and Emerald (BPEE). Other games need their own address.');
  const at = offset => hex(base + offset);
  return {
    manifest_version: 2, addon_id: `reader.${crypto.randomUUID()}`, display_name: `${rom.title} — party`, version: '0.1.0',
    matches: { game_code: [rom.game_code], revision: [rom.revision] },
    sections: [{
      id: 'party', title: 'Party', kind: 'cards',
      note: 'One card per slot. Stats come from the unencrypted tail of each 100-byte record; the name and species are decrypted.',
      repeat: { count: 6, stride: 100 },
      card: {
        title: { gen3_species: at(0) },
        subtitle: { gen3_text: { at: at(8), len: 10 } },
        image: { gen3_species_sprite: at(0) },
        lead: { label: 'HP', read: { u16: at(86) }, max: { u16: at(88) }, hint: 'Record +86 / +88' },
        fields: [
          { label: 'Level', read: { u8: at(84) } },
          { label: 'Attack', read: { u16: at(90) } },
          { label: 'Defense', read: { u16: at(92) } },
          { label: 'Speed', read: { u16: at(94) } },
          { label: 'Sp. Atk', read: { u16: at(96) } },
          { label: 'Sp. Def', read: { u16: at(98) } },
        ],
      },
    }],
  };
}

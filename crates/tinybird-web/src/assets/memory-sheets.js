// A sheet is a normal, validated manifest with a discoverable metadata tag.
export function memorySheet(manifest) {
  const copy = structuredClone(manifest);
  copy.$comment = { ...(copy.$comment && typeof copy.$comment === 'object' && !Array.isArray(copy.$comment) ? copy.$comment : { notes: copy.$comment ?? '' }), kind: 'memory_sheet', memory_sheet_version: 1 };
  return copy;
}
const READ_NAMES = {
  text: value => `ASCII text (${value.len} bytes)`,
  // The two Generation 3 reads exist because neither is expressible as a plain
  // one: the alphabet is the game's own, and species is encrypted in place.
  gen3_text: value => `Pokémon-alphabet text (${value.len} bytes)`,
  gen3_species: () => 'Pokémon species, decrypted from the record',
  gen3: value => `${String(value?.field ?? '?').replace(/_/g, ' ')}, decrypted from the record`,
};

export function describeRead(read) {
  // A unit variant comes back from the Rust side as a bare string.
  const spec = typeof read === 'string' ? { [read]: null } : read;
  if (!spec || typeof spec !== 'object') return 'Unknown read';
  const [type, value] = Object.entries(spec)[0] ?? [];
  if (type === 'literal') return `Fixed text: ${value}`;
  if (type === 'index') return 'Repeated row number';
  if (type === 'const') return `Fixed number: ${value}`;
  const address = ['text', 'gen3_text', 'gen3'].includes(type) ? value?.at : value;
  const path = typeof address === 'string' ? address : `${address?.at} → ${(address?.deref ?? []).map(n => `read pointer, ${n < 0 ? '-' : '+'}0x${Math.abs(n).toString(16)}`).join(' → ')}`;
  return `${READ_NAMES[type] ? READ_NAMES[type](value) : type} at ${path}`;
}
export function sheetRows(manifest) {
  return (manifest.sections ?? []).flatMap(section => (section.fields ?? [
    ...(section.card?.title ? [{ label: 'Card title', read: section.card.title }] : []),
    ...(section.card?.subtitle ? [{ label: 'Card subtitle', read: section.card.subtitle }] : []),
    ...(section.card?.lead ? [section.card.lead] : []), ...(section.card?.fields ?? []),
  ]).map(field => ({
    label: field.label, read: describeRead(field.read), notes: field.hint ?? '', section: section.title,
    repeat: section.repeat ? JSON.stringify(section.repeat) : '', max: field.max ? describeRead(field.max) : '',
  })));
}
// These expose only unencrypted fields from the compiled reader's layout.
// See pokemon_frlg.rs: party profiles and parse_party_member (100-byte records).
export function builtinSheet(rom) {
  const profile = ({ BPRE: [0x02024284, 0x0202402a], BPEE: [0x020244ec, 0x020244e9] })[rom.game_code];
  if (!profile || rom.revision !== 0) throw new Error('This example covers English FireRed revision 0 and Emerald revision 0. Other versions need verified addresses.');
  const [base, count] = profile;
  const at = n => `0x${n.toString(16).toUpperCase()}`;
  return memorySheet({ manifest_version: 2, addon_id: `sheet.${crypto.randomUUID()}`, display_name: `${rom.title} — party basics`, version: '0.1.0',
    matches: { game_code: [rom.game_code], revision: [rom.revision] },
    sections: [{ id: 'party', title: 'First party member', kind: 'key_value', fields: [
      { label: 'Party size', read: { u8: at(count) }, hint: 'Party count from the compiled reader profile.' },
      { label: 'Level', read: { u8: at(base + 84) }, hint: 'Party record +84. Each record is 100 bytes.' },
      { label: 'HP', read: { u16: at(base + 86) }, max: { u16: at(base + 88) }, hint: 'Party record +86 / +88, little-endian. These fields are not encrypted.' },
    ] }],
    $comment: { kind: 'memory_sheet', memory_sheet_version: 1, source: 'tinybird-games/src/pokemon_frlg.rs: party profiles and parse_party_member', notes: 'Read only after loading a save with a party. Empty or uninitialized slots may contain zero or stale values. Species and other boxed data require decryption; this sheet does not implement that decoder. Verify ROM hacks separately.' },
  });
}

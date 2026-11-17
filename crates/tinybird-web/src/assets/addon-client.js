// Local add-ons: manifests kept in this browser, shared by the add-ons page,
// the Workshop and Play. A manifest is data; nothing in one is executed.
export const MAX_LOCAL_ADDONS = 4;
export const MAX_MANIFEST_BYTES = 65536;

// The keys predate local-only mode, when storage was scoped per account and
// "guest" was the signed-out scope. They are kept as they were so add-ons and
// drafts saved before still load.
const KEY = 'tinybird:local-addons:guest';

export function localKey() { return KEY; }
export function disabledBuiltins() {
  try { const ids = JSON.parse(localStorage.getItem(`${KEY}:builtins`) ?? '[]'); return Array.isArray(ids) ? ids.filter(id => typeof id === 'string') : []; } catch { return []; }
}
export function saveDisabledBuiltins(ids) { localStorage.setItem(`${KEY}:builtins`, JSON.stringify(ids)); }
// Rail cards Play can put away. They are not add-ons in the manifest sense, but
// they are the same kind of choice, so they are kept and offered alongside.
export function hiddenPanels() {
  try { const ids = JSON.parse(localStorage.getItem(`${KEY}:panels`) ?? '[]'); return Array.isArray(ids) ? ids.filter(id => typeof id === 'string') : []; } catch { return []; }
}
export function saveHiddenPanels(ids) {
  try { localStorage.setItem(`${KEY}:panels`, JSON.stringify(ids)); } catch { /* nothing to persist to */ }
}
export function localAddons() {
  try {
    const items = JSON.parse(localStorage.getItem(KEY) ?? '[]');
    return Array.isArray(items) ? items.filter(item => item && typeof item === 'object' && item.manifest && typeof item.manifest.addon_id === 'string' && typeof item.manifest.display_name === 'string').slice(0, MAX_LOCAL_ADDONS) : [];
  } catch { return []; }
}
export function saveLocalAddons(items) {
  if (items.length > MAX_LOCAL_ADDONS) throw new Error('Keep at most four local add-ons. Remove one first.');
  localStorage.setItem(KEY, JSON.stringify(items));
}
export function parseManifest(text) {
  if (new TextEncoder().encode(text).length > MAX_MANIFEST_BYTES) throw new Error('Add-ons must be at most 64 KiB.');
  const value = JSON.parse(text);
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error('Import one manifest object.');
  return value;
}
/** The manifests of every enabled local add-on, with ids that cannot collide with built-ins. */
export function enabledManifests(local) {
  return local.filter(item => item.enabled && item.manifest).map((item, index) => ({ ...item.manifest, addon_id: `local.${item.id ?? index}` }));
}

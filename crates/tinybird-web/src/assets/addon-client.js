// Shared by the catalog and Play. No code from a community package is executed.
export const MAX_LOCAL_ADDONS = 4;
export const MAX_MANIFEST_BYTES = 65536;
let expectedOwner = null;
export function setAddonAccount(user) { expectedOwner = user?.id ?? null; }
export function localKey(user) { return `tinybird:local-addons:${user?.id ?? 'guest'}`; }
export function disabledBuiltins(user) {
  try { const ids = JSON.parse(localStorage.getItem(`${localKey(user)}:builtins`) ?? '[]'); return Array.isArray(ids) ? ids.filter(id => typeof id === 'string') : []; } catch { return []; }
}
export function saveDisabledBuiltins(user, ids) { localStorage.setItem(`${localKey(user)}:builtins`, JSON.stringify(ids)); }
// The two rail cards Play can put away — vault saves and screenshots. They are
// not add-ons in the manifest sense and run no code of their own, but they are
// the same kind of choice, so they are kept with the same account-scoped key
// and offered in the same list.
export function hiddenPanels(user) {
  // Screenshots default to the Vault gallery; explicit sidebar choices persist.
  try { const ids = JSON.parse(localStorage.getItem(`${localKey(user)}:panels`) ?? '["shots"]'); return Array.isArray(ids) ? ids.filter(id => typeof id === 'string') : ['shots']; } catch { return ['shots']; }
}
export function saveHiddenPanels(user, ids) {
  try { localStorage.setItem(`${localKey(user)}:panels`, JSON.stringify(ids)); } catch { /* nothing to persist to */ }
}
export function localAddons(user) {
  try {
    const items = JSON.parse(localStorage.getItem(localKey(user)) ?? '[]');
    return Array.isArray(items) ? items.filter(item => item && typeof item === 'object' && item.manifest && typeof item.manifest.addon_id === 'string' && typeof item.manifest.display_name === 'string').slice(0, MAX_LOCAL_ADDONS) : [];
  } catch { return []; }
}
export function saveLocalAddons(user, items) {
  if (items.length > MAX_LOCAL_ADDONS) throw new Error('Keep at most four local add-ons. Remove one first.');
  localStorage.setItem(localKey(user), JSON.stringify(items));
}
export async function api(path, method = 'GET', body) {
  const response = await fetch(`/api/community-addons${path}`, {
    method, headers: { 'Content-Type': 'application/json', 'X-Tinybird-Addons': '1', ...(expectedOwner ? { 'X-Tinybird-Addon-Owner': expectedOwner } : {}) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok) { const error = new Error(result.error ?? `Request failed (${response.status}).`); error.status = response.status; throw error; }
  return result;
}
export function parseManifest(text) {
  if (new TextEncoder().encode(text).length > MAX_MANIFEST_BYTES) throw new Error('Add-ons must be at most 64 KiB.');
  const value = JSON.parse(text);
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error('Import one manifest object.');
  return value;
}
export function enabledManifests(installed, local) {
  return [
    ...installed.filter(item => item.enabled && item.available && item.manifest).map(item => item.manifest),
    ...local.filter(item => item.enabled && item.manifest).map((item, index) => ({ ...item.manifest, addon_id: `local.${item.id ?? index}` })),
  ];
}

import { TinyBird } from '/tinybird.js';
import { mountWorkshop } from '/workshop.js';
import { localAddons, saveLocalAddons, localKey, parseManifest, disabledBuiltins, saveDisabledBuiltins } from '/addon-client.js';

const $ = id => document.getElementById(id);
let validator;
let channel;
let workshop;
let builtinCatalog = [];
const testPlayer = () => $('workshop-player').contentWindow?.tinybird ?? null;
function showView() {
  const requested = location.hash.slice(1);
  const view = ['manage', 'workshop', 'advanced'].includes(requested) ? requested : 'manage';
  for (const panel of document.querySelectorAll('[data-addon-view]')) panel.hidden = panel.dataset.addonView !== view;
  for (const tab of document.querySelectorAll('[data-addon-tab]')) {
    if (tab.dataset.addonTab === view) tab.setAttribute('aria-current', 'page'); else tab.removeAttribute('aria-current');
  }
  if (view === 'workshop') {
    const frame = $('workshop-player'); if (!frame.getAttribute('src')) frame.src = '/play?embed=workshop';
    if (!workshop) workshop = mountWorkshop({ root: $('addon-workshop'),
      getEmulator: () => { const player = testPlayer(); player?.attachAddonPreview($('workshop-output')); return player?.emu; },
      onPreview: result => { const player = testPlayer(); player?.attachAddonPreview($('workshop-output')); player?.showAddonPreview(result); },
      preview: manifest => { const player = testPlayer(); if (!player) throw new Error('The test emulator is starting. Load a game in the center to begin.'); return player.previewAddon(manifest); },
      focusGame: () => { frame.contentWindow?.document.getElementById('canvas')?.focus(); },
      installed: async () => { await testPlayer()?.refreshAddons(); broadcast(); renderMine(); },
    });
    else workshop.reloadDraft();
  } else if (view === 'advanced') loadDraft();
  if (view === 'manage') renderMine();
}
window.addEventListener('hashchange', showView);
try { channel = new BroadcastChannel('tinybird:addons'); } catch {}
const message = (text, error = false) => { $('addon-message').textContent = text; $('addon-message').dataset.error = String(error); };
const node = (tag, text) => { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; return el; };
function button(text, action) {
  const el = node('button', text); el.type = 'button'; el.className = 'key';
  el.addEventListener('click', () => perform(el, action)); return el;
}
async function perform(control, action) {
  control.disabled = true;
  try { await action(); } catch (error) { message(error.message, true); }
  finally { control.disabled = false; }
}
function broadcast() { channel?.postMessage({ type: 'changed' }); }
const draftKey = () => `${localKey()}:draft`;
function saveDraft() {
  try { localStorage.setItem(draftKey(), JSON.stringify({ text: $('manifest-editor').value })); }
  catch { message('Your browser could not save this draft. Export JSON to keep a copy.', true); }
}
function template() {
  return { manifest_version: 2, addon_id: $('starter-id').value, display_name: $('starter-name').value,
    version: '0.1.0', matches: { game_code: [$('starter-game').value.toUpperCase()], revision: [Number($('starter-revision').value)] },
    sections: [{ id: 'stats', title: 'Stats', kind: 'key_value', fields: [{ label: $('starter-label').value, read: { [$('starter-type').value]: $('starter-address').value } }] }] };
}
function loadDraft() {
  let draft;
  try { draft = JSON.parse(localStorage.getItem(draftKey()) ?? 'null'); } catch {}
  $('manifest-editor').value = typeof draft?.text === 'string' ? draft.text : JSON.stringify(template(), null, 2);
}
async function validated() {
  const manifest = parseManifest($('manifest-editor').value);
  validator ??= TinyBird.load().catch(error => { validator = null; throw error; });
  (await validator).installManifests([manifest]);
  return manifest;
}
function card(title, description) {
  const el = node('article'); el.className = 'addon-card';
  el.append(node('h3', title), node('p', description)); return el;
}
function renderMine() {
  const disabled = disabledBuiltins();
  $('builtin-list').replaceChildren(...builtinCatalog.map(info => {
    const off = disabled.includes(info.addon_id);
    const el = card(info.display_name, info.supported_games);
    el.append(node('p', off ? 'Disabled' : 'Enabled'), button(off ? 'Enable' : 'Disable', async () => {
      const next = off ? disabledBuiltins().filter(id => id !== info.addon_id) : [...new Set([...disabledBuiltins(), info.addon_id])];
      saveDisabledBuiltins(next); renderMine(); broadcast(); await testPlayer()?.refreshAddons();
    })); return el;
  }));
  const local = $('local-list'); local.replaceChildren();
  const items = localAddons();
  if (!items.length) local.append(node('p', 'No local readers yet. Build one in the Workshop or import a JSON manifest.'));
  for (const item of items) {
    const el = card(item.manifest.display_name, `Local · ${item.enabled ? 'Enabled' : 'Disabled'}`);
    const actions = node('div'); actions.className = 'addon-actions';
    actions.append(button(item.enabled ? 'Disable' : 'Enable', () => {
      saveLocalAddons(localAddons().map(other => other.manifest.addon_id === item.manifest.addon_id ? { ...other, enabled: !item.enabled } : other)); renderMine(); broadcast();
    }), button('Edit', () => {
      $('manifest-editor').value = JSON.stringify(item.manifest, null, 2); saveDraft(); location.hash = 'advanced';
    }), button('Remove', () => {
      saveLocalAddons(localAddons().filter(other => other.manifest.addon_id !== item.manifest.addon_id)); renderMine(); broadcast();
    }));
    el.append(actions); local.append(el);
  }
}

$('starter-form').addEventListener('submit', event => {
  event.preventDefault();
  if ($('manifest-editor').value.trim() && !confirm('Replace the editor contents with this template?')) return;
  $('manifest-editor').value = JSON.stringify(template(), null, 2); saveDraft();
});
$('manifest-editor').addEventListener('input', saveDraft);
$('validate-addon').addEventListener('click', event => perform(event.currentTarget, async () => { await validated(); message('Manifest is valid. Test its addresses against the intended game.'); }));
$('try-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const manifest = await validated();
  const items = localAddons().filter(item => item.manifest.addon_id !== manifest.addon_id);
  const previous = localAddons().find(item => item.manifest.addon_id === manifest.addon_id);
  saveLocalAddons([...items, { id: previous?.id ?? crypto.randomUUID(), enabled: true, manifest }]); renderMine(); broadcast(); message('Installed locally. Open Play or return to your running game to see its panels.');
}));
$('export-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const manifest = await validated(); const url = URL.createObjectURL(new Blob([JSON.stringify(manifest, null, 2)], { type: 'application/json' }));
  const a = node('a', ''); a.href = url; a.download = `${manifest.addon_id}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}));
$('import-addon').addEventListener('change', event => perform(event.currentTarget, async () => {
  const file = event.target.files[0]; if (!file) return;
  if (file.size > 65536) throw new Error('Add-ons must be at most 64 KiB.');
  const manifest = parseManifest(await file.text());
  $('manifest-editor').value = JSON.stringify(manifest, null, 2); saveDraft(); await validated(); message('Imported and validated.');
}));
$('preview-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const manifest = await validated(); if (!channel) throw new Error('Live preview is unavailable in this browser. Install locally and open Play.');
  const request = crypto.randomUUID();
  const result = await new Promise((resolve, reject) => {
    const receive = event => { if (event.data?.type !== 'preview-result' || event.data.request !== request) return; cleanup(); resolve(event.data); };
    const timer = setTimeout(() => { cleanup(); reject(new Error('Open Play in another tab and load your game, then preview again.')); }, 5000);
    const cleanup = () => { clearTimeout(timer); channel.removeEventListener('message', receive); };
    channel.addEventListener('message', receive); channel.postMessage({ type: 'preview', request, manifest });
  });
  if (result.error) throw new Error(result.error);
  $('preview-result').hidden = false;
  $('preview-result').replaceChildren(node('h3', 'Live preview'), node('p', result.status), node('pre', JSON.stringify(result.sections, null, 2)));
}));

loadDraft();
try {
  validator ??= TinyBird.load().catch(error => { validator = null; throw error; });
  builtinCatalog = (await validator).snapshot().builtin_addons ?? [];
} catch (error) { message(`Preinstalled readers could not load: ${error.message}`, true); }
showView();

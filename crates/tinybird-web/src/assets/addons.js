import { mountChrome } from '/chrome.js';
import { TinyBird } from '/tinybird.js';
import { mountWorkshop } from '/workshop.js';
import { sheetRows } from '/memory-sheets.js';
import { api, localAddons, saveLocalAddons, localKey, parseManifest, setAddonAccount, disabledBuiltins, saveDisabledBuiltins } from '/addon-client.js';

const $ = id => document.getElementById(id);
$('catalog-kind').value = new URLSearchParams(location.search).get('kind') === 'memory_sheet' ? 'memory_sheet' : '';
let user = null;
let library = { installed: [], published: [] };
let accountEpoch = 0;
let catalogEpoch = 0;
let nextOffset = null;
let validator;
let channel;
let workshop;
let builtinCatalog = [];
function testPlayer() {
  const player = $('workshop-player').contentWindow?.tinybird;
  return player && player.addonOwner === (user?.id ?? null) ? player : null;
}
function showView() {
  const requested = location.hash.slice(1);
  const view = ['manage', 'workshop', 'community', 'advanced'].includes(requested) ? requested : new URLSearchParams(location.search).has('id') ? 'community' : 'manage';
  for (const panel of document.querySelectorAll('[data-addon-view]')) panel.hidden = panel.dataset.addonView !== view;
  for (const tab of document.querySelectorAll('[data-addon-tab]')) {
    if (tab.dataset.addonTab === view) tab.setAttribute('aria-current', 'page'); else tab.removeAttribute('aria-current');
  }
  if (view === 'workshop') {
    const frame = $('workshop-player'); if (!frame.getAttribute('src')) frame.src = '/play?embed=workshop';
    if (!workshop) workshop = mountWorkshop({ root: $('addon-workshop'), getUser: () => user,
      getEmulator: () => { const player = testPlayer(); player?.attachAddonPreview($('workshop-output')); return player?.emu; },
      onPreview: result => { const player = testPlayer(); player?.attachAddonPreview($('workshop-output')); player?.showAddonPreview(result); },
      preview: manifest => { const player = testPlayer(); if (!player) throw new Error('The test emulator is starting. Load a game in the center to begin.'); return player.previewAddon(manifest); },
      focusGame: () => { frame.contentWindow?.document.getElementById('canvas')?.focus(); },
      installed: async () => { await testPlayer()?.refreshAddons(); broadcast(); await refreshMine(); },
    });
    else workshop.reloadDraft();
  } else if (view === 'advanced') loadDraft();
  if (view === 'manage') renderMine();
}
window.addEventListener('hashchange', showView);
try { channel = new BroadcastChannel('tinybird:addons'); } catch {}
const message = (text, error = false) => { $('addon-message').textContent = text; $('addon-message').dataset.error = String(error); };
const node = (tag, text) => { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; return el; };
const link = (text, href) => { const el = node('a', text); el.href = href; return el; };
function button(text, action, disabled = false) {
  const el = node('button', text); el.type = 'button'; el.className = 'key'; el.disabled = disabled;
  el.addEventListener('click', () => perform(el, action)); return el;
}
async function perform(control, action) {
  control.disabled = true;
  try { await action(); } catch (error) { message(error.message, true); }
  finally { control.disabled = false; }
}
function broadcast() { channel?.postMessage({ type: 'changed' }); }
function draftKey() { return `${localKey(user)}:draft`; }
function saveDraft() {
  try { localStorage.setItem(draftKey(), JSON.stringify({ text: $('manifest-editor').value, description: $('publish-description').value, license: $('publish-license').value })); }
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
  $('publish-description').value = draft?.description ?? '';
  $('publish-license').value = draft?.license ?? 'MIT';
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
async function refreshMine(epoch = accountEpoch) {
  const result = user ? await api('/installed') : { installed: [], published: [] };
  if (epoch !== accountEpoch) return;
  library = result;
  renderMine();
}
function renderMine() {
  const disabled = disabledBuiltins(user);
  $('builtin-list').replaceChildren(...builtinCatalog.map(info => {
    const off = disabled.includes(info.addon_id);
    const el = card(info.display_name, info.supported_games);
    el.append(node('p', off ? 'Disabled' : 'Enabled'), button(off ? 'Enable' : 'Disable', async () => {
      const next = off ? disabledBuiltins(user).filter(id => id !== info.addon_id) : [...new Set([...disabledBuiltins(user), info.addon_id])];
      saveDisabledBuiltins(user, next); renderMine(); broadcast(); await testPlayer()?.refreshAddons();
    })); return el;
  }));
  const installed = $('installed-list'); installed.replaceChildren();
  if (!library.installed.length) installed.append(node('p', user ? 'No community add-ons installed yet.' : 'Sign in to sync community installations.'));
  for (const item of library.installed) {
    const el = card(item.name, `Release ${item.release} · ${item.available ? item.enabled ? 'Enabled' : 'Disabled' : 'Withdrawn'}`);
    const actions = node('div'); actions.className = 'addon-actions';
    actions.append(button(item.enabled ? 'Disable' : 'Enable', async () => {
      await api(`/${item.id}/installation`, 'PUT', { release: item.release, enabled: !item.enabled });
      await refreshMine(); broadcast();
    }, !item.available), button('Remove', async () => {
      await api(`/${item.id}/installation`, 'DELETE'); await refreshMine(); broadcast();
    }));
    if (item.available) actions.append(link('Versions', `/addons?id=${item.id}&release=${item.release}`));
    el.append(actions); installed.append(el);
  }
  const local = $('local-list'); local.replaceChildren();
  const items = localAddons(user);
  for (const item of items) {
    const el = card(item.manifest.display_name, `Local · ${item.enabled ? 'Enabled' : 'Disabled'}`);
    const actions = node('div'); actions.className = 'addon-actions';
    actions.append(button(item.enabled ? 'Disable' : 'Enable', () => {
      saveLocalAddons(user, localAddons(user).map(other => other.manifest.addon_id === item.manifest.addon_id ? { ...other, enabled: !item.enabled } : other)); renderMine(); broadcast();
    }), button('Edit', () => {
      $('manifest-editor').value = JSON.stringify(item.manifest, null, 2); saveDraft(); location.hash = 'workshop';
    }), button('Remove', () => {
      saveLocalAddons(user, localAddons(user).filter(other => other.manifest.addon_id !== item.manifest.addon_id)); renderMine(); broadcast();
    }));
    el.append(actions); local.append(el);
  }
  $('published-list').replaceChildren(...library.published.map(item => {
    const el = card(item.addon_id, item.available ? 'Published' : 'Withdrawn');
    if (item.available) el.append(link('View releases', `/addons?id=${item.id}`), button('Withdraw add-on', async () => {
      if (!confirm('Withdraw all releases? This stops distribution and disables installed copies when players next refresh. This cannot be undone.')) return;
      await api(`/${item.id}`, 'DELETE'); await refreshMine(); await browse(); broadcast();
    }));
    return el;
  }));
}
async function browse(offset = 0) {
  const epoch = ++catalogEpoch;
  const result = await api(`?q=${encodeURIComponent($('search-query').value)}&kind=${encodeURIComponent($('catalog-kind').value)}&offset=${offset}`);
  if (epoch !== catalogEpoch) return;
  if (offset === 0) $('catalog').replaceChildren();
  if (!result.addons.length && offset === 0) $('catalog').append(node('p', 'No matching add-ons yet. Create and share the first one.'));
  for (const item of result.addons) {
    const el = card(item.name, item.description);
    el.append(node('p', item.kind === 'memory_sheet' ? 'Memory sheet · inspect and reuse its discoveries' : 'Reader'));
    el.append(node('p', `By ${item.author} · Release ${item.release} · ${item.license}`), link('View and install', `/addons?id=${item.id}&release=${item.release}`));
    $('catalog').append(el);
  }
  nextOffset = result.next_offset;
  $('load-more').hidden = nextOffset === null;
}
async function showRelease(id, requested) {
  const result = await api(`/${encodeURIComponent(id)}`);
  const release = result.releases.find(item => item.release === Number(requested)) ?? (requested ? null : result.releases[0]);
  if (!release) throw new Error('That release does not exist.');
  const detail = $('release-detail'); detail.hidden = false; detail.replaceChildren();
  detail.append(node('h3', release.name), node('p', release.description), node('p', `By ${release.author} · ${release.license}`));
  const label = node('label', 'Release'); const select = node('select');
  for (const item of result.releases) { const option = node('option', `${item.release} · ${item.manifest.version ?? 'unversioned'}`); option.value = item.release; select.append(option); }
  select.value = release.release; label.append(select); detail.append(label);
  select.addEventListener('change', () => { location.href = `/addons?id=${id}&release=${select.value}`; });
  detail.append(node('p', `Compatibility: ${JSON.stringify(release.manifest.matches)}`));
  if (release.manifest.when) detail.append(node('p', `Readiness condition: ${JSON.stringify(release.manifest.when)}`));
  const rows = sheetRows(release.manifest);
  if (rows.length) {
    detail.append(node('h4', 'Memory sheet'));
    const table = node('table'); table.className = 'memory-sheet-table';
    const head = node('tr'); for (const title of ['Value', 'How it is read', 'Discovery notes']) head.append(node('th', title)); table.append(head);
    for (const item of rows) { const row = node('tr'); row.append(node('td', `${item.section} / ${item.label}`), node('td', `${item.read}${item.max ? `; maximum: ${item.max}` : ''}${item.repeat ? `; repeat: ${item.repeat}` : ''}`), node('td', item.notes)); table.append(row); }
    detail.append(table);
  }
  const actions = node('div'); actions.className = 'addon-actions';
  actions.append(button(user ? `Install release ${release.release}` : 'Sign in to install', async () => {
    await api(`/${id}/installation`, 'PUT', { release: release.release, enabled: true });
    await refreshMine(); broadcast(); message('Installed. Your open Play tab will refresh its readers.');
  }, !user), button('Copy share link', async () => {
    await navigator.clipboard.writeText(`${location.origin}/addons?id=${id}&release=${release.release}`); message('Release link copied.');
  }), button('Use as a starting sheet', () => {
    if (localStorage.getItem(draftKey()) && !confirm('Replace your current draft with a copy of this release?')) return;
    const copy = structuredClone(release.manifest); copy.addon_id = `reader.${crypto.randomUUID()}`;
    copy.$comment = { ...(copy.$comment && typeof copy.$comment === 'object' && !Array.isArray(copy.$comment) ? copy.$comment : { notes: copy.$comment ?? '' }), source_release: `/addons?id=${id}&release=${release.release}`, source_author: release.author, source_license: release.license };
    $('manifest-editor').value = JSON.stringify(copy, null, 2);
    $('publish-description').value = release.description; $('publish-license').value = release.license;
    saveDraft(); location.hash = 'workshop'; message('Loaded into your workshop draft. Publishing creates a release under your account.');
  }));
  detail.append(actions);
  const inspect = node('details'); inspect.append(node('summary', 'Inspect manifest'), node('pre', JSON.stringify(release.manifest, null, 2))); detail.append(inspect);
  if (user) {
    const reportLabel = node('label', 'Report a problem'); const reason = node('textarea'); reason.maxLength = 1000; reportLabel.append(reason);
    detail.append(reportLabel, button('Submit report', async () => { await api(`/${id}/reports`, 'POST', { reason: reason.value }); message('Report saved for review.'); }));
  }
}

$('starter-form').addEventListener('submit', event => {
  event.preventDefault();
  if ($('manifest-editor').value.trim() && !confirm('Replace the editor contents with this template?')) return;
  $('manifest-editor').value = JSON.stringify(template(), null, 2); saveDraft();
});
for (const id of ['manifest-editor', 'publish-description', 'publish-license']) $(id).addEventListener('input', saveDraft);
$('validate-addon').addEventListener('click', event => perform(event.currentTarget, async () => { await validated(); message('Manifest is valid. Test its addresses against the intended game before publishing.'); }));
$('try-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const epoch = accountEpoch; const manifest = await validated(); if (epoch !== accountEpoch) return;
  const items = localAddons(user).filter(item => item.manifest.addon_id !== manifest.addon_id);
  const previous = localAddons(user).find(item => item.manifest.addon_id === manifest.addon_id);
  saveLocalAddons(user, [...items, { id: previous?.id ?? crypto.randomUUID(), enabled: true, manifest }]); renderMine(); broadcast(); message('Installed locally. Open Play or return to your running game to see its panels.');
}));
$('export-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const manifest = await validated(); const url = URL.createObjectURL(new Blob([JSON.stringify(manifest, null, 2)], { type: 'application/json' }));
  const a = link('', url); a.download = `${manifest.addon_id}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}));
$('import-addon').addEventListener('change', event => perform(event.currentTarget, async () => {
  const file = event.target.files[0]; if (!file) return;
  if (file.size > 65536) throw new Error('Add-ons must be at most 64 KiB.');
  const manifest = parseManifest(await file.text());
  $('manifest-editor').value = JSON.stringify(manifest, null, 2); saveDraft(); await validated(); message('Imported and validated.');
}));
$('publish-form').addEventListener('submit', event => {
  event.preventDefault(); perform($('publish-addon'), async () => {
    if (!user) throw new Error('Sign in to publish a release.');
    const epoch = accountEpoch; const manifest = await validated(); if (epoch !== accountEpoch) return;
    const result = await api('', 'POST', { manifest, description: $('publish-description').value, license: $('publish-license').value });
    message(`Published release ${result.release}.`); await refreshMine(); await browse();
    history.replaceState(null, '', result.url); await showRelease(result.id, result.release);
  });
});
$('addon-search').addEventListener('submit', event => { event.preventDefault(); browse().catch(error => message(error.message, true)); });
$('catalog-kind').addEventListener('change', () => browse().catch(error => message(error.message, true)));
$('load-more').addEventListener('click', event => perform(event.currentTarget, () => browse(nextOffset ?? 0)));
$('preview-addon').addEventListener('click', event => perform(event.currentTarget, async () => {
  const manifest = await validated(); if (!channel) throw new Error('Live preview is unavailable in this browser. Install locally and open Play.');
  const request = crypto.randomUUID();
  const result = await new Promise((resolve, reject) => {
    const receive = event => { if (event.data?.type !== 'preview-result' || event.data.request !== request) return; cleanup(); resolve(event.data); };
    const timer = setTimeout(() => { cleanup(); reject(new Error('Open Play in another tab and load your game, then preview again.')); }, 5000);
    const cleanup = () => { clearTimeout(timer); channel.removeEventListener('message', receive); };
    channel.addEventListener('message', receive); channel.postMessage({ type: 'preview', request, manifest, owner: user?.id ?? null });
  });
  if (result.error) throw new Error(result.error);
  $('preview-result').hidden = false;
  $('preview-result').replaceChildren(node('h3', 'Live preview'), node('p', result.status), node('pre', JSON.stringify(result.sections, null, 2)));
}));

async function accountChanged(next) {
  user = next; setAddonAccount(user); const epoch = ++accountEpoch;
  workshop?.accountChanged(); $('workshop-player').contentWindow?.tinybird?.refreshAccount();
  library = { installed: [], published: [] }; renderMine(); loadDraft();
  $('publish-addon').disabled = !user;
  $('account-note').textContent = user ? 'Your installations sync to this account. Drafts and local readers stay in this browser.' : 'Sign in to publish and sync installations. Local add-ons work without an account.';
  $('moderation').hidden = user?.role !== 'admin';
  try {
    await refreshMine(epoch); if (epoch !== accountEpoch) return;
    const params = new URLSearchParams(location.search); if (params.has('id')) await showRelease(params.get('id'), params.get('release'));
    if (user?.role === 'admin') {
      const result = await api('/reports'); if (epoch !== accountEpoch) return;
      $('report-list').replaceChildren(...result.reports.map(report => {
        const el = card('Reported add-on', report.reason); el.append(link('Inspect', `/addons?id=${report.id}`), button('Withdraw', async () => {
          if (!confirm('Withdraw this add-on from all community installations?')) return;
          await api(`/${report.id}`, 'DELETE'); el.remove(); broadcast(); await browse();
        })); return el;
      }));
    }
  } catch (error) { if (epoch === accountEpoch) message(error.message, true); }
}
const accounts = await mountChrome({ onAccountChange: accountChanged });
window.addEventListener('focus', () => accounts?.refresh());
if (!accountEpoch) await accountChanged(null);
await browse().catch(error => message(error.message, true));
try {
  validator ??= TinyBird.load().catch(error => { validator = null; throw error; });
  builtinCatalog = (await validator).snapshot().builtin_addons ?? [];
  renderMine();
} catch (error) { message(`Preinstalled readers could not load: ${error.message}`, true); }
showView();

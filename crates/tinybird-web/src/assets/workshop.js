import { api, localAddons, localKey, saveLocalAddons, parseManifest } from '/addon-client.js';
import { REGIONS, hex, numberValue, readNumber, scanMemory, starterManifest, searchPattern, scanPattern,
  FIELD_KINDS, GEN3_FIELDS, fieldSpec, fieldForm, addSection, updateSection, removeSection, moveSection,
  addField, updateField, removeField, moveField, setLead, sectionFields, partyTemplate } from '/workshop-model.js';
import { memorySheet, builtinSheet, describeRead } from '/memory-sheets.js';
import { mountObservations } from '/workshop-observations.js';

// These tools are first-party panels. Shared add-ons remain read-only data.
export function mountWorkshop({ root, getEmulator, getUser, preview, installed, focusGame, onPreview = () => {} }) {
  root.innerHTML = `
    <summary>Add-on workshop <span>Build while you play</span></summary>
    <div class="workshop-body">
      <header class="workshop-header">
        <div class="workshop-heading"><span class="workshop-eyebrow">READER WORKSHOP</span><span data-storage role="status">Draft saved in this browser</span></div>
        <label class="workshop-name">Reader name<input data-name maxlength="120" placeholder="My game reader"></label>
        <div class="workshop-toolbar">
          <details class="workshop-reader-menu" data-reader-menu><summary>Open / new</summary>
            <div class="workshop-menu-body">
              <label>Saved readers<select data-saved></select></label><button type="button" data-load>Open reader</button>
              <div class="workshop-actions"><button type="button" data-new>New blank reader</button><button type="button" data-party>Party template</button><button type="button" data-example>Documented example</button></div>
              <label>Import a reader<input data-import type="file" accept=".json,application/json"></label>
              <a href="/addons?kind=memory_sheet#community" target="_blank" rel="noopener">Browse shared memory sheets</a>
            </div>
          </details>
          <button type="button" data-finish>Finish &amp; share</button>
        </div>
        <p data-game>Load a game in the test player to begin.</p>
      </header>
      <div class="workshop-scroll">
        <p data-message role="status" aria-live="polite"></p>
        <div class="workshop-empty" data-empty>
          <span class="workshop-eyebrow">YOUR FIRST FIELD</span><h3>What do you want to track?</h3>
          <p>Try a value you can see in the game, like HP, money, or lives. Find its address and watch it update in your reader.</p>
          <button type="button" data-start-field>Add your first field</button>
          <p class="workshop-muted">Already have a reader? Use Open / new above.</p>
        </div>
        <details class="workshop-outline" data-outline open>
          <summary>Your fields <span data-field-count>0</span></summary>
          <div class="workshop-tree" data-tree></div>
          <div class="workshop-actions"><button type="button" data-new-field>+ Add field</button><button type="button" data-add-section>+ Category</button></div>
          <form class="workshop-category-editor" data-category-editor hidden>
            <h4 data-category-title>New category</h4>
            <label>Name<input data-category-name maxlength="120" required placeholder="Stats, party, inventory..."></label>
            <label>Layout<select data-category-kind><option value="key_value">A list of values</option><option value="cards">Repeating cards (party, inventory...)</option></select></label>
            <div class="workshop-grid" data-repeat-options hidden><label>Entries<input data-category-count type="number" min="1" max="64" value="6"></label><label>Bytes between entries<input data-category-stride type="number" min="1" max="65536" value="100"></label></div>
            <p data-category-help hidden>Use the addresses of the first entry. Each card reads the next entry using this spacing.</p>
            <details data-card-options hidden><summary>Pokémon party headings</summary><label><input type="checkbox" data-card-names> Show Gen 3 species, nickname, and sprite</label><label data-card-address-label hidden>First party record address<input data-card-address placeholder="0x02024284" spellcheck="false"></label></details>
            <div class="workshop-actions"><button type="submit" data-category-save>Add category</button><button type="button" data-category-cancel>Cancel</button></div>
          </form>
        </details>
        <section class="workshop-editor" data-field-editor hidden aria-label="Field editor">
          <div class="workshop-editor-heading"><div><span class="workshop-eyebrow">FIELD DETAILS</span><h3 data-editor-title>Add a field</h3></div><button type="button" data-cancel-edit>Cancel</button></div>
          <div class="workshop-grid"><label>Label<input data-label maxlength="120" placeholder="HP, money, lives..."></label><label>Category<select data-target aria-label="Category this field goes in"></select></label></div>
          <label>Display as<select data-field-type></select></label><p data-type-help></p>
          <div class="workshop-grid">
            <div data-address-label><label>Value address<input data-address placeholder="0x02000000" spellcheck="false"></label><button type="button" data-find-address>Find in game</button></div>
            <label data-size-label hidden>Number size<select data-field-size><option value="u8">8-bit</option><option value="u16" selected>16-bit</option><option value="u32">32-bit</option></select></label>
            <label data-gen3-label hidden>Which part of the record<select data-gen3-field></select></label>
            <div data-max-label hidden><label>Maximum address<input data-max placeholder="0x02000002" spellcheck="false"></label><button type="button" data-find-max>Find maximum</button></div>
            <label data-cap-label hidden>Bar out of<input data-cap type="number" min="1" placeholder="leave blank for no bar"></label>
            <label data-length-label hidden>Length in bytes<input data-text-length type="number" min="1" max="64" value="16"></label>
            <label data-literal-label hidden>Fixed text<input data-literal maxlength="256" placeholder="Shown as-is"></label>
          </div>
          <section class="workshop-finder" data-finder hidden aria-label="Find an address">
            <div class="workshop-finder-heading"><h4 data-finder-title>Find the value address</h4><button type="button" data-close-finder>Close search</button></div>
            <p data-finder-help>Search for the value shown in your game. Play to change it, then narrow the matches.</p>
            <div class="workshop-grid">
              <label>Match<select data-mode><option value="equal">Equals</option><option value="range">Between</option><option value="unknown">Unknown starting value</option><option value="changed">Changed</option><option value="unchanged">Unchanged</option><option value="increased">Increased</option><option value="decreased">Decreased</option></select></label>
              <label>Value in game<input data-value inputmode="numeric" placeholder="e.g. 100"></label>
              <label data-upper-label hidden>Upper value<input data-upper inputmode="numeric" placeholder="e.g. 200"></label>
            </div>
            <details class="workshop-search-options"><summary data-search-settings>Search settings</summary>
              <div class="workshop-grid"><label>Search for<select data-search-kind><option value="number">A number</option><option value="text">Text (ASCII)</option><option value="pattern">Bytes / wildcard pattern</option></select></label><label>Number size<select data-type><option value="u8">8-bit (0–255)</option><option value="u16" selected>16-bit (0–65,535)</option><option value="u32">32-bit</option></select></label></div>
              <label>Memory area<select data-region><option value="ewram">Main RAM</option><option value="iwram">Fast RAM</option></select></label>
              <label><input type="checkbox" data-ignore-case> Ignore ASCII letter case</label><label><input type="checkbox" data-unaligned> Include unaligned numbers</label><p data-search-help></p>
            </details>
            <div class="workshop-actions"><button type="button" data-scan>Search memory</button><button type="button" data-search-play>Play to change value</button><button type="button" data-clear>Reset search</button></div>
            <p data-search-error role="alert"></p><p data-count role="status">Enter a value to start searching.</p><div class="workshop-results" data-results></div>
          </section>
          <div class="workshop-field-live"><span class="workshop-eyebrow">LIVE VALUE</span><output data-field-live>Choose an address to test this field.</output></div>
          <button type="button" data-use-observation>Use this address for automation</button>
          <details data-notes><summary>Discovery notes <span class="workshop-muted">optional</span></summary><label>How you verified this value<input data-field-note maxlength="256" placeholder="What it means and when it is valid"></label></details>
          <p data-field-error role="alert"></p>
          <div class="workshop-editor-actions"><button type="button" data-add>Add to reader</button><button type="button" data-add-another>Add &amp; next field</button></div>
        </section>
        <div data-observation-workspace></div>
        <details class="workshop-advanced" data-advanced><summary>Advanced tools</summary>
          <details class="workshop-diagnostics"><summary>Reader value details</summary><div class="workshop-preview" data-preview>Load a game and add a field to begin.</div></details>
          <label>Reader JSON<textarea data-json rows="12" spellcheck="false"></textarea></label><button type="button" data-apply>Apply JSON</button><a href="/reader-guide" target="_blank" rel="noopener">Format guide</a>
        </details>
      </div>
      <footer class="workshop-session"><button type="button" data-return>Play / test</button><label><input type="checkbox" data-live checked> Live preview</label></footer>
    </div>
    <dialog class="workshop-finish" data-finish-dialog aria-labelledby="workshop-finish-title">
      <div class="workshop-finish-heading"><div><span class="workshop-eyebrow">FINISH YOUR READER</span><h3 id="workshop-finish-title" data-finish-title>Your reader</h3></div><button type="button" data-back-build>Back to editing</button></div>
      <p data-share-summary></p><p data-finish-status role="status"></p>
      <section class="workshop-delivery"><h4>Use it yourself</h4><p>Your draft is saved in this browser. Enable it beside your game, or download a copy to keep.</p><div class="workshop-actions"><button type="button" data-save>Save &amp; enable</button><button type="button" data-export>Download JSON</button></div></section>
      <section class="workshop-publish"><h4>Share with the community</h4><p>Test in a few game situations, then describe what works and any limitations.</p>
        <label>Description<textarea data-description rows="3" maxlength="2000" placeholder="What it shows, where you tested it, and any limitations"></textarea></label>
        <div class="workshop-grid"><label>License<select data-license><option>MIT</option><option>Apache-2.0</option><option>CC0-1.0</option><option>CC-BY-4.0</option></select></label></div>
        <label><input data-sheet type="checkbox"> Also list as a public memory sheet</label><p class="workshop-muted">A sheet lets others reuse your addresses and discovery notes.</p>
        <p data-account></p><button type="button" data-publish>Publish release</button><p data-share></p>
      </section>
      <p data-finish-message role="status" aria-live="polite"></p>
    </dialog>
`;
  const $ = name => root.querySelector(`[data-${name}]`);
  const observations = mountObservations({ root: $('observation-workspace'), getEmulator, getUser });
  $('use-observation').addEventListener('click', () => act(() => {
    requireGame();
    const kind = $('field-type').value;
    const type = kind === 'bar' ? $('field-size').value : kind;
    const width = { u8: 1, u16: 2, u32: 4 }[type];
    if (!width) throw new Error('Automation observations currently support plain unsigned numbers and bar values.');
    observations.useAddress(numberValue($('address').value, 'u32'), width);
  }));
  const node = (tag, text) => { const el = document.createElement(tag); el.textContent = text; return el; };
  let owner, draft = null, previous = null, candidates = null, scanKey = '', gameKey = '', lastFrame = 0, currentEmulator;
  let jsonDirty = false, previewKey = '', lastStored = null;
  let editing = null, dragging = null, categoryEditing = null, categoryCardChanged = false, formBaseline = '', publishing = false;
  let addressTarget = 'address';
  const searches = new Map();
  const finder = $('finder');
  const formKeys = ['label', 'field-type', 'address', 'max', 'field-size', 'gen3-field', 'cap', 'text-length', 'literal', 'field-note', 'target'];
  const rawForm = () => Object.fromEntries(formKeys.map(key => [key, $(key).value]));
  const formChanged = () => !$('field-editor').hidden && JSON.stringify(rawForm()) !== formBaseline;
  const tell = text => { ($('finish-dialog').open ? $('finish-message') : $('message')).textContent = text; };
  function previewMessage(text) { $('preview').textContent = text; onPreview({ status: 'idle', sections: [], error: text }); previewKey = ''; }
  const draftKey = () => `${localKey(getUser())}:draft`;
  const requireGame = () => { const emu = getEmulator(); if (!emu?.hasRom) throw new Error('Load a game in the test player first.'); return emu; };
  function clearSearch() {
    previous = candidates = null; scanKey = '';
    $('results').replaceChildren(); $('search-error').textContent = '';
    $('count').textContent = 'Enter a value to start searching.'; $('scan').textContent = 'Search memory';
    if (!['equal', 'range', 'unknown'].includes($('mode').value)) $('mode').value = 'equal';
    searchControls();
  }
  function persist() {
    try {
      const serialized = JSON.stringify({ text: $('json').value, description: $('description').value, license: $('license').value,
        editor: $('field-editor').hidden ? null : { values: rawForm(), editing, baseline: formBaseline }, name: $('name').value });
      localStorage.setItem(draftKey(), serialized); lastStored = serialized;
      $('storage').textContent = formChanged() ? 'Unfinished field saved in draft' : 'Draft saved in this browser';
    } catch { $('storage').textContent = 'Draft could not be saved'; tell('Browser storage is full. Download your reader to keep a copy.'); }
  }
  function listSaved() {
    $('saved').replaceChildren(...localAddons(getUser()).map(item => { const option = node('option', item.manifest.display_name); option.value = item.manifest.addon_id; return option; }));
    $('load').disabled = !$('saved').options.length;
  }
  function setDraft(manifest) {
    draft = manifest; jsonDirty = false; previewKey = '';
    $('json').value = manifest ? JSON.stringify(manifest, null, 2) : '';
    $('name').value = manifest?.display_name ?? '';
    $('sheet').checked = manifest?.$comment?.kind === 'memory_sheet';
    const sections = Array.isArray(manifest?.sections) ? manifest.sections : [];
    $('field-count').textContent = String(sections.reduce((count, section) =>
      count + sectionFields(section).length + (section.card?.lead ? 1 : 0), 0));
    // An edit in flight points at a position, and the draft it pointed into is
    // gone. Anything else would apply the form to whatever moved into that slot.
    renderTargets(sections);
    renderTree(sections);
    updateWorkspace();
  }

  function updateWorkspace() {
    const empty = Number($('field-count').textContent) === 0;
    $('empty').hidden = !empty || !$('field-editor').hidden;
    $('outline').hidden = !draft || (!draft.sections?.length && empty);
    $('finish').disabled = !draft && $('field-editor').hidden;
  }

  // Keep each address search independent. Looking for max HP must not narrow
  // the remaining candidates from the current-HP search.
  const searchKeys = ['value', 'upper', 'mode', 'type', 'region', 'search-kind'];
  function keepSearch() {
    searches.set(addressTarget, { previous, candidates, scanKey,
      values: Object.fromEntries(searchKeys.map(key => [key, $(key).value])),
      unaligned: $('unaligned').checked, ignoreCase: $('ignore-case').checked,
      results: [...$('results').childNodes], count: $('count').textContent });
  }
  function openFinder(target) {
    if (target !== addressTarget) keepSearch();
    const changedTarget = target !== addressTarget;
    addressTarget = target;
    const held = searches.get(target);
    if (changedTarget && held) {
      ({ previous, candidates, scanKey } = held);
      for (const key of searchKeys) $(key).value = held.values[key];
      $('unaligned').checked = held.unaligned; $('ignore-case').checked = held.ignoreCase;
      $('results').replaceChildren(...held.results); $('count').textContent = held.count;
      $('scan').textContent = previous ? 'Narrow matches' : 'Search memory';
    } else if (changedTarget || !scanKey) {
      clearSearch(); $('value').value = ''; $('upper').value = '';
      const kind = $('field-type').value;
      $('type').value = kind === 'bar' ? $('field-size').value : ['u8', 'u16', 'u32'].includes(kind) ? kind : 'u8';
      $('search-kind').value = target === 'address' && kind === 'text' ? 'text' : 'number';
    }
    $('finder-title').textContent = target === 'max' ? 'Find the maximum address' : 'Find the value address';
    $('search-error').textContent = ''; searchControls();
    finder.hidden = false; $('value').focus();
  }
  $('find-address').addEventListener('click', () => openFinder('address'));
  $('find-max').addEventListener('click', () => openFinder('max'));
  $('close-finder').addEventListener('click', () => { finder.hidden = true; $(addressTarget).focus(); });
  $('search-play').addEventListener('click', focusGame);
  $('start-field').addEventListener('click', () => act(() => newField()));
  $('new-field').addEventListener('click', () => act(() => newField()));
  $('cancel-edit').addEventListener('click', () => {
    if (formChanged() && !confirm('Discard the changes to this field?')) return;
    stopEditing(); persist(); $('new-field').focus();
  });
  function requireSettledField() {
    if (formChanged()) {
      $('field-error').textContent = 'Add or save this field before changing the reader structure.';
      $('label').focus();
      throw new Error('Finish the field you are editing first. Its details are kept in your draft.');
    }
    stopEditing();
  }
  function openCategory(index = null) {
    checkJson(); requireSettledField(); ensureDraft();
    categoryEditing = index;
    const section = index === null ? null : draft.sections[index];
    $('category-title').textContent = section ? 'Category settings' : 'New category';
    $('category-name').value = section?.title ?? '';
    $('category-kind').value = section?.kind ?? 'key_value';
    $('category-kind').disabled = !!section;
    $('category-count').value = String(section?.repeat?.count ?? 6);
    $('category-stride').value = String(section?.repeat?.stride ?? 100);
    categoryCardChanged = false;
    $('card-names').checked = !!section?.card?.title?.gen3_species;
    $('card-address').value = section?.card?.title?.gen3_species ?? '';
    $('card-address-label').hidden = !$('card-names').checked;
    $('category-save').textContent = section ? 'Save category' : 'Add category';
    categoryControls(); $('category-editor').hidden = false; $('outline').open = true; $('category-name').focus();
  }
  function categoryControls() {
    $('repeat-options').hidden = $('category-help').hidden = $('card-options').hidden = $('category-kind').value !== 'cards';
  }
  $('add-section').addEventListener('click', () => act(() => openCategory()));
  $('category-kind').addEventListener('change', categoryControls);
  $('card-names').addEventListener('change', () => { categoryCardChanged = true; $('card-address-label').hidden = !$('card-names').checked; });
  $('card-address').addEventListener('input', () => { categoryCardChanged = true; });
  $('category-cancel').addEventListener('click', () => { $('category-editor').hidden = true; });
  $('category-editor').addEventListener('submit', event => {
    event.preventDefault();
    act(() => {
      checkJson(); requireSettledField();
      const title = $('category-name').value, kind = $('category-kind').value;
      const count = Number($('category-count').value), stride = Number($('category-stride').value);
      const index = categoryEditing ?? draft.sections.length;
      let next = categoryEditing === null ? addSection(ensureDraft(), { title, kind, count, stride }) :
        updateSection(draft, categoryEditing, { title, ...(kind === 'cards' ? { repeat: { count, stride } } : {}) });
      if (kind === 'cards' && categoryCardChanged) {
        const address = $('card-names').checked ? numberValue($('card-address').value, 'u32') : null;
        next = updateSection(next, index, { card: address === null ? { title: null, subtitle: null, image: null } : {
          title: { kind: 'gen3_species', address }, subtitle: { kind: 'gen3_text', address: address + 8, length: 10 }, image: address,
        } });
      }
      setDraft(next);
      $('category-editor').hidden = true; $('target').value = String(index); persist(); tick();
    });
  });
  function replaceDraft(manifest) {
    stopEditing(); searches.clear(); clearSearch(); setDraft(manifest);
    $('description').value = ''; $('license').value = 'MIT'; $('share').replaceChildren();
    $('category-editor').hidden = true; $('reader-menu').open = false; persist(); tick();
  }
  const canReplace = () => !($('json').value || formChanged()) || confirm('Replace this draft? Download it from Finish & share first if you want to keep a copy.');
  $('party').addEventListener('click', () => act(() => {
    const manifest = partyTemplate(identity()); if (!canReplace()) return; replaceDraft(manifest);
    tell('Party template loaded. Check its addresses against your game.');
  }));
  function openFinish() {
    if (formChanged()) commitField();
    else if (!$('field-editor').hidden) stopEditing();
    checkJson();
    if (!draft || !Number($('field-count').textContent)) throw new Error('Add at least one field before finishing your reader.');
    persist();
    $('finish-title').textContent = draft.display_name;
    const matches = draft.matches ?? {};
    $('share-summary').textContent = `${$('field-count').textContent} fields in ${draft.sections.length} categories · ${(matches.game_code ?? []).join(', ')} · revision ${(matches.revision ?? []).join(', ')}`;
    let status;
    try {
      const result = validate();
      status = result.status === 'active' ? 'Reader is running in the test game.' : result.status === 'incompatible' ? 'Load a compatible game to test this reader before publishing.' : 'Waiting for the game to reach the reader’s readiness condition.';
    } catch (error) { status = error.message; }
    $('finish-status').textContent = status; $('finish-message').textContent = '';
    $('finish-dialog').showModal();
  }
  $('finish').addEventListener('click', () => act(openFinish));
  $('back-build').addEventListener('click', () => $('finish-dialog').close());

  /* -- The structure ------------------------------------------------------
   *
   * A category is a section, and a repeating category is a `cards` section:
   * one card described once and drawn `count` times. That is how a party of
   * six becomes "Party -> Spearow (slot 1) -> HP, Attack" without six copies
   * of the same six fields written out by hand.
   *
   * The tree is rebuilt from the draft on every change rather than patched.
   * The draft is small, and a tree that is a pure function of it cannot drift
   * out of step with the JSON the reader actually runs.
   */

  /** The field an edit refers to, or nothing if the draft moved under it. */
  function fieldAt(sections, at) {
    const section = sections[at.section];
    if (!section) return null;
    return at.field === null ? section.card?.lead ?? null : sectionFields(section)[at.field] ?? null;
  }

  function renderTargets(sections) {
    const chosen = $('target').value;
    $('target').replaceChildren(...sections.map((section, index) => {
      const option = node('option', section.kind === 'cards' ? `${section.title} (each entry)` : section.title);
      option.value = String(index);
      return option;
    }));
    if (sections.some((_, index) => String(index) === chosen)) $('target').value = chosen;
  }

  function grip(label) {
    const handle = node('span', '⠿');
    handle.className = 'workshop-grip';
    handle.title = label;
    handle.setAttribute('aria-hidden', 'true');
    return handle;
  }

  function tinyButton(text, title, onClick) {
    const button = node('button', text);
    button.type = 'button';
    button.className = 'workshop-tiny';
    button.title = title;
    button.addEventListener('click', onClick);
    return button;
  }

  function chip(text) {
    const span = node('span', text);
    span.className = 'workshop-chip';
    return span;
  }

  function renderTree(sections) {
    $('tree').replaceChildren();
    if (!sections.length) {
      $('tree').append(node('p', 'Add a category to start.'));
      return;
    }

    for (const [index, section] of sections.entries()) {
      const block = document.createElement('article');
      block.className = 'workshop-cat';
      block.dataset.section = String(index);

      const head = document.createElement('header');
      head.className = 'workshop-cat__head';
      head.draggable = true;
      head.addEventListener('dragstart', event => {
        dragging = { kind: 'section', section: index };
        event.dataTransfer.effectAllowed = 'move';
        // Firefox starts no drag at all unless the transfer carries something.
        event.dataTransfer.setData('text/plain', section.title);
      });
      head.addEventListener('dragend', () => { dragging = null; });
      // A category only accepts a category drop, so dragging a field across
      // one never silently reorders the categories underneath it.
      head.addEventListener('dragover', event => {
        if (dragging?.kind !== 'section' || dragging.section === index) return;
        event.preventDefault();
        head.dataset.over = 'on';
      });
      head.addEventListener('dragleave', () => { delete head.dataset.over; });
      head.addEventListener('drop', event => {
        delete head.dataset.over;
        if (dragging?.kind !== 'section') return;
        event.preventDefault();
        const from = dragging.section;
        dragging = null;
        act(() => { checkJson(); requireSettledField(); setDraft(moveSection(draft, from, index)); persist(); tick(); });
      });

      const title = document.createElement('input');
      title.value = section.title;
      title.maxLength = 120;
      title.className = 'workshop-cat__title';
      title.setAttribute('aria-label', 'Category name');
      title.addEventListener('change', () => act(() => {
        checkJson(); requireSettledField();
        setDraft(updateSection(draft, index, { title: title.value }));
        persist();
      }));

      head.append(grip('Drag to reorder this category'), title);
      if (section.kind === 'cards') head.append(chip(`×${section.repeat?.count ?? 0} · ${section.repeat?.stride ?? 0}B`));
      head.append(tinyButton('remove', 'Delete this category and its fields', () => act(() => {
        checkJson(); requireSettledField();
        const held = sectionFields(section).length + (section.card?.lead ? 1 : 0);
        if (held && !confirm(`Delete "${section.title}" and its ${held} field(s)?`)) return;
        setDraft(removeSection(draft, index));
        persist();
        tick();
      })));
      block.append(head);

      if (section.note) {
        const note = node('p', section.note);
        note.className = 'workshop-cat__note';
        block.append(note);
      }
      if (section.kind === 'cards') block.append(renderCardSlots(section, index));

      const list = document.createElement('ul');
      list.className = 'workshop-fields';
      if (section.card?.lead) list.append(fieldRow(section, index, section.card.lead, null));
      for (const [position, field] of sectionFields(section).entries()) {
        list.append(fieldRow(section, index, field, position));
      }
      if (!list.children.length) {
        const empty = node('li', 'No fields yet.');
        empty.className = 'workshop-fields__empty';
        list.append(empty);
      }
      // Dropping on the list rather than on a row is how a field reaches an
      // empty category, and how it lands at the end of a full one.
      list.addEventListener('dragover', event => {
        if (dragging?.kind !== 'field') return;
        event.preventDefault();
        list.dataset.over = 'on';
      });
      list.addEventListener('dragleave', () => { delete list.dataset.over; });
      list.addEventListener('drop', event => {
        delete list.dataset.over;
        if (dragging?.kind !== 'field') return;
        event.preventDefault();
        drop({ section: index, field: sectionFields(section).length });
      });
      block.append(list);

      block.append(tinyButton('+ Add field', 'Add a field to this category', () => act(() => newField(index))));

      $('tree').append(block);
    }
  }

  /** The heading, second line and picture that every card in a repeat gets. */
  function renderCardSlots(section, index) {
    const strip = document.createElement('div');
    strip.className = 'workshop-cat__card';
    strip.append(node('span', `Each entry: ${section.card?.title ? describeRead(section.card.title) : 'slot number'}`));
    if (section.card?.subtitle) strip.append(node('span', `· under it: ${describeRead(section.card.subtitle)}`));
    if (section.card?.image) strip.append(node('span', '· with a sprite'));

    strip.append(tinyButton('Settings', 'Edit this repeating category', () => act(() => openCategory(index))));

    return strip;
  }

  function fieldRow(section, index, field, position) {
    const row = document.createElement('li');
    row.className = 'workshop-field';
    // The headline stat is not in the field list, so it has no position to be
    // dragged to or from. It is promoted and demoted rather than moved.
    const lead = position === null;

    if (lead) {
      row.dataset.lead = 'on';
      const star = node('span', '★');
      star.className = 'workshop-grip workshop-grip--lead';
      star.title = 'The headline stat on every entry';
      row.append(star);
    } else {
      row.draggable = true;
      row.addEventListener('dragstart', event => {
        dragging = { kind: 'field', section: index, field: position };
        event.dataTransfer.effectAllowed = 'move';
        event.dataTransfer.setData('text/plain', field.label);
        row.dataset.dragging = 'on';
      });
      row.addEventListener('dragend', () => { dragging = null; delete row.dataset.dragging; });
      row.addEventListener('dragover', event => {
        if (dragging?.kind !== 'field') return;
        event.preventDefault();
        row.dataset.over = 'on';
      });
      row.addEventListener('dragleave', () => { delete row.dataset.over; });
      row.addEventListener('drop', event => {
        delete row.dataset.over;
        if (dragging?.kind !== 'field') return;
        // The list behind this row is also a drop target; the row wins.
        event.preventDefault();
        event.stopPropagation();
        drop({ section: index, field: position });
      });
      row.append(grip('Drag to reorder, or into another category'));
    }

    const words = document.createElement('div');
    words.className = 'workshop-field__words';
    const name = node('span', field.label);
    name.className = 'workshop-field__label';
    const read = node('span', describeRead(field.read) + (field.max ? ' · bar' : ''));
    read.className = 'workshop-field__read';
    words.append(name, read);
    if (field.hint) {
      const hint = node('span', field.hint);
      hint.className = 'workshop-field__hint';
      words.append(hint);
    }
    row.append(words);

    const acts = document.createElement('div');
    acts.className = 'workshop-field__acts';
    acts.append(tinyButton('edit', 'Change this field', () => startEditing(index, position)));
    if (section.kind === 'cards') {
      acts.append(tinyButton(lead ? 'demote' : 'headline',
        lead ? 'Put this back in the list' : 'Make this the headline stat on every entry',
        () => act(() => { checkJson(); requireSettledField(); setDraft(setLead(draft, index, position)); persist(); tick(); })));
    }
    acts.append(tinyButton('remove', 'Delete this field', () => act(() => {
      checkJson(); requireSettledField();
      setDraft(lead ? withoutLead(index) : removeField(draft, index, position));
      persist();
      tick();
    })));
    row.append(acts);
    return row;
  }

  /** Delete the headline stat outright rather than demoting it into the list. */
  function withoutLead(index) {
    const copy = structuredClone(draft);
    delete copy.sections[index].card.lead;
    return copy;
  }

  function drop(to) {
    const from = dragging;
    dragging = null;
    if (!from || (from.section === to.section && from.field === to.field)) return;
    act(() => { checkJson(); requireSettledField(); setDraft(moveField(draft, from, to)); persist(); tick(); });
  }
  function checkJson() { if (jsonDirty) throw new Error('Apply your JSON edits before changing or saving the reader.'); }
  function identity() { return requireGame().snapshot().rom; }
  function ensureDraft() {
    if (!draft) {
      const manifest = starterManifest(identity());
      if ($('name').value.trim()) manifest.display_name = $('name').value.trim();
      setDraft(manifest);
    }
    return draft;
  }
  function validate() {
    checkJson(); if (!draft) throw new Error('Add a field or import a reader first.');
    const result = preview(draft); if (result.error) throw new Error(result.error); return result;
  }
  async function act(action) { try { synchronize(); await action(); } catch (error) { tell(error.message); } }
  function accountChanged() {
    if (owner === (getUser()?.id ?? null)) return;
    owner = getUser()?.id ?? null;
    $('finish-dialog').close(); stopEditing(); setDraft(null); previewMessage('Load a game and add a field to begin.'); $('share').replaceChildren();
    $('message').textContent = ''; $('storage').textContent = 'Draft saved in this browser';
    $('description').value = ''; $('license').value = 'MIT';
    try {
      lastStored = localStorage.getItem(draftKey());
      const saved = JSON.parse(lastStored ?? 'null');
      if (saved) {
        $('description').value = saved.description ?? ''; $('license').value = saved.license ?? 'MIT';
        if (saved.text) {
          try { setDraft(parseManifest(saved.text)); } catch { $('json').value = saved.text; jsonDirty = true; $('advanced').open = true; }
        }
        if (!draft && typeof saved.name === 'string') $('name').value = saved.name;
        const held = saved.editor;
        if (held?.values && draft && (!held.editing || fieldAt(draft.sections ?? [], held.editing))) {
          editing = held.editing ?? null;
          for (const key of formKeys) if (typeof held.values[key] === 'string') $(key).value = held.values[key];
          fieldControls(); showEditor();
          formBaseline = typeof held.baseline === 'string' ? held.baseline : '';
          $('notes').open = !!$('field-note').value;
          $('storage').textContent = formChanged() ? 'Unfinished field restored' : 'Draft restored';
        }
      }
    } catch {}
    listSaved(); $('publish').disabled = !getUser() || publishing;
    updateWorkspace();
    $('account').textContent = getUser() ? 'Publish a public release under your account. Publishing again updates this reader with a new release.' : 'Sign in using the account menu to publish. You can build and save locally as a guest.';
  }
  function synchronize() {
    if (owner !== (getUser()?.id ?? null)) accountChanged();
    const emu = getEmulator();
    const header = emu?.hasRom ? Array.from(emu.readMemory(0x080000a0, 32)).join(',') : '';
    if (emu !== currentEmulator || header !== gameKey || (emu && emu.frameCount < lastFrame)) {
      currentEmulator = emu; gameKey = header; searches.clear(); clearSearch(); previewKey = '';
    }
    lastFrame = emu?.frameCount ?? 0;
    return emu;
  }
  $('scan').addEventListener('click', () => act(() => {
    try {
    $('search-error').textContent = '';
    synchronize(); const emu = requireGame(); const type = $('type').value, region = $('region').value, mode = $('mode').value, kind = $('search-kind').value;
    const key = `${region}:${type}:${kind}:${$('unaligned').checked}:${$('ignore-case').checked}`; if (scanKey && scanKey !== key) clearSearch();
    const [base, length] = REGIONS[region]; const bytes = emu.readMemory(base, length);
    let pattern;
    if (kind === 'number') {
      let target = ['equal', 'range'].includes(mode) ? numberValue($('value').value, type) : 0;
      if (mode === 'range') { const upper = numberValue($('upper').value, type); if (upper < target) throw new Error('Upper value must be at least the lower value.'); target = [target, upper]; }
      candidates = scanMemory(bytes, type, mode, target, previous, candidates, $('unaligned').checked);
    } else {
      pattern = searchPattern($('value').value, kind);
      candidates = scanPattern(bytes, pattern, kind === 'text' && $('ignore-case').checked, candidates);
    }
    previous = bytes; scanKey = key;
    $('scan').textContent = 'Narrow matches';
    searchControls();
    $('count').textContent = candidates.length === 0 ? 'No matches. Reset the search to try another value or number size.' : `${candidates.length.toLocaleString()} ${candidates.length === 1 ? 'match' : 'matches'}${candidates.length > 80 ? ' · first 80 shown' : ''}. ${candidates.length === 1 ? 'Use this address, then check the live value.' : 'Change the value in game and narrow again, or choose an address below.'}`;
    $('results').replaceChildren(...candidates.slice(0, 80).map(offset => {
      const value = kind === 'number' ? readNumber(bytes, offset, type) : Array.from(bytes.slice(offset, offset + Math.min(pattern.length, 16)), n => n.toString(16).padStart(2, '0')).join(' ');
      const button = node('button', `${hex(base + offset)} · ${value} · Use`); button.type = 'button';
      button.addEventListener('click', () => {
        $(addressTarget).value = hex(base + offset);
        if (addressTarget === 'address' && !editing && $('field-type').value !== 'bar') {
          $('field-type').value = kind === 'text' ? 'text' : kind === 'pattern' ? 'u8' : type;
          if (kind === 'text') $('text-length').value = pattern.length;
        }
        if ($('field-type').value === 'bar' && kind === 'number') $('field-size').value = type;
        fieldControls();
        finder.hidden = true;
        $('field-error').textContent = ''; persist(); sampleField();
        $(addressTarget).focus();
        tell(addressTarget === 'max' ? 'Maximum address selected. Save the field when ready.' : 'Address selected. Finish the field details, then save it.');
      }); return button;
    }));
    } catch (error) { $('search-error').textContent = error.message; }
  }));
  $('clear').addEventListener('click', clearSearch);
  $('return').addEventListener('click', focusGame);
  function searchControls() {
    const numeric = $('search-kind').value === 'number';
    $('mode').closest('label').hidden = !numeric; $('type').disabled = !numeric;
    $('upper-label').hidden = !numeric || $('mode').value !== 'range';
    $('value').disabled = numeric && !['equal', 'range'].includes($('mode').value);
    $('value').inputMode = numeric ? 'numeric' : 'text';
    $('ignore-case').closest('label').hidden = $('search-kind').value !== 'text';
    $('unaligned').closest('label').hidden = !numeric;
    for (const option of $('mode').options) option.disabled = !previous && !['equal', 'range', 'unknown'].includes(option.value);
    $('search-settings').textContent = `${numeric ? `${$('type').value.slice(1)}-bit numbers` : $('search-kind').value === 'text' ? 'ASCII text' : 'Byte pattern'} · ${$('region').value === 'ewram' ? 'Main RAM' : 'Fast RAM'} · settings`;
    $('clear').disabled = !previous;
    $('search-help').textContent = numeric ? 'Numbers are little-endian. Use a range when the exact value is uncertain.' : $('search-kind').value === 'text' ? 'ASCII only. Pokémon and other games may use custom alphabets; try byte patterns for those.' : 'Hex bytes separated by spaces; ?? matches any byte. Example: 0F ?? 2A 00.';
  }
  for (const key of ['region', 'type', 'search-kind', 'unaligned', 'ignore-case']) $(key).addEventListener('change', () => { clearSearch(); searchControls(); });
  $('mode').addEventListener('change', searchControls); searchControls();
  $('field-type').replaceChildren(...Object.entries(FIELD_KINDS).map(([kind, spec]) => {
    const option = node('option', spec.label);
    option.value = kind;
    return option;
  }));
  $('field-type').value = 'u16';

  $('gen3-field').replaceChildren(...Object.entries(GEN3_FIELDS).map(([value, spec]) => {
    const option = node('option', spec.label);
    option.value = value;
    return option;
  }));
  // Picking "IV — Attack" should not also mean looking up that IVs stop at 31.
  // Changing the field offers its usual ceiling; clearing the box opts out.
  $('gen3-field').addEventListener('change', () => {
    const cap = GEN3_FIELDS[$('gen3-field').value]?.cap;
    $('cap').value = cap ? String(cap) : '';
    fieldControls();
    // `input` fires on a select before `change`, so the sample taken by the
    // shared listener above used the previous ceiling. Take another.
    sampleField();
  });

  const TYPE_HELP = {
    bar: 'Two addresses: the value now, and the maximum it is out of. The reader draws a bar and colours it — green, amber, then red as it empties.',
    text: 'Plain ASCII. Most games store menus this way; Pokémon names do not.',
    gen3_text: 'Generation 3 stores names in an alphabet of its own, so a plain text read gives punctuation. This applies the right table. A nickname sits 8 bytes into a party record.',
    gen3_species: 'Species is encrypted and shuffled inside the record, so point this at the START of the 100-byte record, not at a species field. The reader decrypts it and names it from the cartridge.',
    gen3: 'Moves, EVs, IVs, nature and the rest live in an encrypted block that is shuffled differently for every Pokémon, so you pick the part by name rather than by address. Point this at the START of the 100-byte record. Moves and held items are named from the cartridge; EVs and IVs come with their usual ceiling filled in, so they draw a bar.',
    literal: 'A fixed word. Useful as a heading when the game stores no name.',
    index: 'Which entry this is, counting from one. Only means anything in a repeating category.',
  };

  /** Show only the inputs the chosen tracker type actually uses. */
  function fieldControls() {
    const kind = $('field-type').value;
    const spec = FIELD_KINDS[kind] ?? {};
    $('address-label').hidden = !spec.address;
    $('size-label').hidden = !spec.size;
    $('max-label').hidden = !spec.max;
    $('gen3-label').hidden = !spec.gen3;
    $('cap-label').hidden = !spec.cap;
    $('length-label').hidden = !spec.length;
    $('literal-label').hidden = kind !== 'literal';
    if (!spec.address || (addressTarget === 'max' && !spec.max)) finder.hidden = true;
    $('type-help').textContent = TYPE_HELP[kind] ?? 'A plain number, shown as it is read.';
  }
  $('field-type').addEventListener('change', () => {
    fieldControls(); searches.clear(); clearSearch(); finder.hidden = true;
    persist(); sampleField();
  });
  fieldControls();

  /** Everything the form is saying, as the model's field shape. */
  function readForm() {
    const kind = $('field-type').value;
    const spec = FIELD_KINDS[kind] ?? {};
    return {
      label: $('label').value,
      kind,
      hint: $('field-note').value,
      literal: $('literal').value,
      size: $('field-size').value,
      length: Number($('text-length').value),
      address: spec.address ? numberValue($('address').value, 'u32') : 0,
      max: spec.max ? numberValue($('max').value, 'u32') : null,
      gen3Field: $('gen3-field').value,
      cap: spec.cap && $('cap').value.trim() ? Number($('cap').value) : null,
    };
  }

  function fillForm(form) {
    $('label').value = form.label;
    $('field-type').value = form.kind;
    $('address').value = form.address ? hex(form.address) : '';
    $('max').value = form.max ? hex(form.max) : '';
    $('field-size').value = form.size;
    $('text-length').value = String(form.length);
    $('literal').value = form.literal;
    $('gen3-field').value = form.gen3Field ?? 'species';
    $('cap').value = form.cap ? String(form.cap) : '';
    $('field-note').value = form.hint;
    fieldControls();
  }

  /**
   * Open an existing field in the form.
   *
   * Editing used to mean deleting the row and adding it again, which lost the
   * discovery note and the field's place in the order — so nobody did it, and
   * a typo in a label meant living with the typo.
   */
  function showEditor() {
    $('field-editor').hidden = false; $('outline').open = false;
    $('category-editor').hidden = true;
    $('target').disabled = !!editing;
    $('editor-title').textContent = editing ? 'Edit field' : 'Add a field';
    $('add').textContent = editing ? 'Save changes' : 'Add to reader';
    $('add-another').textContent = editing ? 'Save & next field' : 'Add & next field';
    $('field-error').textContent = ''; updateWorkspace();
  }
  function startEditing(section, field) {
    act(() => {
      checkJson(); requireSettledField();
      const target = fieldAt(draft?.sections ?? [], { section, field });
      if (!target) throw new Error('That field is no longer there.');
      editing = { section, field };
      fillForm(fieldForm(target)); $('target').value = String(section);
      showEditor(); $('notes').open = !!target.hint;
      formBaseline = JSON.stringify(rawForm()); persist(); sampleField(); $('label').focus();
    });
  }
  function newField(section = null) {
    checkJson(); requireSettledField();
    const base = ensureDraft();
    if (!base.sections.length) setDraft(addSection(base, { title: 'Stats' }));
    fillForm({ label: '', kind: 'u16', address: 0, max: null, size: 'u16', length: 16, literal: '', hint: '', gen3Field: 'species', cap: null });
    if (section !== null) $('target').value = String(section);
    showEditor(); formBaseline = JSON.stringify(rawForm()); persist(); sampleField(); $('label').focus();
  }
  function stopEditing() {
    editing = null; searches.clear(); addressTarget = 'address';
    finder.hidden = true; clearSearch();
    $('field-editor').hidden = true; $('target').disabled = false; $('outline').open = true;
    $('field-error').textContent = ''; $('notes').open = false;
    for (const key of ['label', 'field-note', 'address', 'max', 'literal']) $(key).value = '';
    formBaseline = ''; updateWorkspace();
  }
  function commitField(another = false) {
    checkJson();
    const base = ensureDraft();
    let form;
    try {
      form = readForm(); fieldSpec(form);
    } catch (error) {
      $('field-error').textContent = error.message;
      const missing = !$('label').value.trim() ? 'label' : FIELD_KINDS[$('field-type').value]?.address && !$('address').value.trim() ? 'address' : 'add';
      $(missing).focus(); throw error;
    }
    const at = editing, target = at?.section ?? Number($('target').value);
    let next;
    if (at?.field === null) {
      next = structuredClone(base); next.sections[at.section].card.lead = fieldSpec(form);
    } else next = at ? updateField(base, at.section, at.field, form) : addField(base, target, form);
    stopEditing(); setDraft(next); persist(); tick();
    tell(`${form.label.trim()} ${at ? 'updated' : 'added'}. Watch it in the live reader.`);
    if (another) newField(target);
    else $('new-field').focus();
  }
  function sampleField() {
    if ($('field-editor').hidden) return;
    try {
      if (!getEmulator()?.hasRom) { $('field-live').textContent = 'Load a game to test this field.'; return; }
      const base = ensureDraft(), form = { ...readForm(), label: $('label').value.trim() || 'Value' };
      const field = fieldSpec(form);
      const sample = { ...base, sections: [{ id: 'field_preview', title: 'Field preview', kind: 'key_value', fields: [field] }] };
      const result = preview(sample);
      const value = result.sections?.[0]?.payload?.[0];
      $('field-live').textContent = result.error ? result.error
        : result.status === 'incompatible' ? 'This reader targets a different game.'
        : value ? value.value
        : base.when ? 'Waiting for the reader’s readiness condition.'
        : emptyRead(form.kind);
    } catch { $('field-live').textContent = 'Complete the value settings to test this field.'; }
  }
  /**
   * Why a read came back with nothing, while the game is running and matched.
   *
   * This used to say "Waiting for the game" — the same words it uses when no
   * game is loaded at all. So the one mistake this editor exists to catch,
   * pointing a species read at a stat address rather than at the record that
   * contains it, looked like a machine that had not finished booting.
   */
  function emptyRead(kind) {
    if (kind === 'gen3_species') {
      return 'No Pokémon here. This wants the START of a 100-byte record, not a stat inside one — a stat address is usually 84 to 98 bytes past it.';
    }
    if (kind === 'gen3_text') return 'No name here. A Generation 3 nickname sits 8 bytes into the record.';
    if (kind === 'text') return 'No readable ASCII here. Pokémon names need the Gen 3 text type instead.';
    return 'Nothing reads at this address right now — the game may not have filled it in yet.';
  }
  for (const key of formKeys) $(key).addEventListener('input', () => {
    $('field-error').textContent = ''; persist(); sampleField();
  });
  $('add').addEventListener('click', () => act(() => commitField()));
  $('add-another').addEventListener('click', () => act(() => commitField(true)));
  $('example').addEventListener('click', () => act(() => {
    const example = builtinSheet(identity()); if (!canReplace()) return; replaceDraft(example);
    tell('Example loaded. Test it against your game before sharing.');
  }));
  $('name').addEventListener('input', () => {
    if (draft && !jsonDirty) {
      draft = { ...draft, display_name: $('name').value };
      $('json').value = JSON.stringify(draft, null, 2);
    }
    persist();
  });
  $('new').addEventListener('click', () => act(() => {
    const manifest = starterManifest(identity()); if (canReplace()) replaceDraft(manifest);
  }));
  $('load').addEventListener('click', () => act(() => {
    const item = localAddons(getUser()).find(item => item.manifest.addon_id === $('saved').value);
    if (item && canReplace()) replaceDraft(structuredClone(item.manifest));
  }));

  $('json').addEventListener('input', () => { jsonDirty = true; persist(); });
  $('apply').addEventListener('click', () => act(() => {
    requireSettledField();
    const manifest = parseManifest($('json').value); const result = preview(manifest);
    if (result.error) throw new Error(result.error); setDraft(manifest); persist(); tick(); tell('JSON applied to the live reader.');
  }));
  $('import').addEventListener('change', () => act(async () => {
    const file = $('import').files[0]; if (!file) return;
    if (file.size > 65536) throw new Error('Readers must be at most 64 KiB.');
    if (!canReplace()) return;
    const capturedOwner = owner; const text = await file.text(); if (capturedOwner !== (getUser()?.id ?? null)) return;
    const manifest = parseManifest(text); const result = preview(manifest); if (result.error) throw new Error(result.error);
    replaceDraft(manifest);
  }));
  for (const key of ['description', 'license']) $(key).addEventListener('input', persist);
  $('sheet').addEventListener('change', () => act(() => {
    checkJson(); if (!draft) throw new Error('Build a reader first.');
    if ($('sheet').checked) draft = memorySheet(draft);
    else if (draft.$comment?.kind === 'memory_sheet') { delete draft.$comment.kind; }
    setDraft(draft); persist();
  }));
  $('save').addEventListener('click', () => act(async () => {
    validate(); const items = localAddons(getUser()), old = items.find(item => item.manifest.addon_id === draft.addon_id);
    saveLocalAddons(getUser(), [...items.filter(item => item !== old), { id: old?.id ?? crypto.randomUUID(), enabled: true, manifest: structuredClone(draft) }]);
    persist(); listSaved(); await installed(); tell('Saved and enabled. Your reader is now in the game companion.');
  }));
  $('export').addEventListener('click', () => act(() => {
    checkJson(); parseManifest(JSON.stringify(draft));
    const url = URL.createObjectURL(new Blob([JSON.stringify(draft, null, 2)], { type: 'application/json' }));
    const a = node('a', ''); a.href = url; a.download = `${draft.addon_id}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
    tell('Reader downloaded. Your browser draft is still available.');
  }));
  $('publish').addEventListener('click', () => act(async () => {
    synchronize(); if (!getUser()) throw new Error('Sign in to publish.');
    const result = validate(); if (result.status === 'incompatible') throw new Error('Load a compatible game and test this reader before publishing.');
    if (!$('description').value.trim()) throw new Error('Describe what your reader does and where you tested it.');
    const capturedOwner = owner; publishing = true; $('publish').disabled = true;
    try {
      const release = await api('', 'POST', { manifest: structuredClone(draft), description: $('description').value, license: $('license').value });
      if (capturedOwner !== (getUser()?.id ?? null)) return;
      const a = node('a', `Release ${release.release} is live — open its community page to share`); a.href = `/addons?id=${encodeURIComponent(release.id)}&release=${release.release}`; a.target = '_blank'; a.rel = 'noopener'; $('share').replaceChildren(a); tell('Published to the community library.');
    } finally { publishing = false; $('publish').disabled = !getUser(); }
  }));
  function tick() {
    try {
      observations.update();
      if (owner !== (getUser()?.id ?? null)) accountChanged();
      if (!root.open || document.hidden || root.closest('[data-addon-view]')?.hidden) return;
      synchronize();
      const emu = getEmulator(); const rom = emu?.hasRom ? identity() : null;
      $('game').textContent = rom ? `${rom.title} · ${rom.game_code} · rev ${rom.revision}` : 'Load a game in the test player to begin.';
      if (!$('live').checked) return;
      sampleField();
      if (!emu?.hasRom || !draft || jsonDirty) { previewMessage(jsonDirty ? 'Apply JSON edits to resume the preview.' : 'Load a game and add a field to begin.'); return; }
      if (draft.sections?.length === 1 && draft.sections[0].kind === 'key_value' && draft.sections[0].fields?.length === 0) {
        previewMessage('Add your first field to start the live preview.'); return;
      }
      const result = preview(draft); onPreview(result); const key = JSON.stringify(result); if (key === previewKey) return; previewKey = key;
      $('preview').replaceChildren();
      if (result.error || result.status !== 'active') { $('preview').textContent = result.error ?? (result.status === 'incompatible' ? 'This reader targets a different game or revision.' : 'Waiting for the reader’s readiness condition.'); return; }
      for (const section of result.sections) {
        $('preview').append(node('h4', section.title));
        if (section.kind === 'key_value' && Array.isArray(section.payload)) {
          const list = node('dl', '');
          for (const field of section.payload) list.append(node('dt', field.label), node('dd', field.value));
          $('preview').append(list);
        } else $('preview').append(node('p', 'See this section rendered in the Your add-on panel on the left.'));
      }
    } catch (error) { previewMessage(error.message); }
  }
  root.addEventListener('toggle', tick);
  if (location.hash === '#workshop') root.open = true;
  accountChanged(); setInterval(tick, 500);
  return { accountChanged, reloadDraft() {
    try { if (owner === (getUser()?.id ?? null) && localStorage.getItem(draftKey()) === lastStored) return; } catch {}
    owner = undefined; accountChanged();
  } };
}

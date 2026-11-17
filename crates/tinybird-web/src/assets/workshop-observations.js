// Shared observation contract for the Workshop and tinybird-runtime.
const NAME = /^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*$/;
const WIDTHS = new Set([1, 2, 4]);
function onlyKeys(value, keys) {
  return value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).every(key => keys.includes(key));
}
export function observationConfig(game, fields) {
  const config = { schema_version: 1, game: { code: game.game_code ?? game.code, revision: game.revision }, fields };
  return validateObservationConfig(config);
}
export function validateObservationConfig(config) {
  if (!onlyKeys(config, ['schema_version', 'game', 'fields']) || config.schema_version !== 1) throw new Error('Unsupported observation configuration. Expected schema version 1.');
  if (!onlyKeys(config.game, ['code', 'revision']) || typeof config.game.code !== 'string' || !/^[A-Z0-9]{4}$/.test(config.game.code)
      || !Number.isInteger(config.game.revision) || config.game.revision < 0 || config.game.revision > 255) throw new Error('Choose a game with a four-character code and a valid revision.');
  if (!Array.isArray(config.fields) || config.fields.length < 1 || config.fields.length > 256) throw new Error('Add between 1 and 256 observations.');
  const names = new Set();
  for (const field of config.fields) {
    if (!onlyKeys(field, ['name', 'address', 'width']) || typeof field.name !== 'string' || field.name.length > 128 || !NAME.test(field.name)) throw new Error('Use dot-separated names such as player.hp (letters, numbers, underscores; start each part with a letter or underscore).');
    if (names.has(field.name)) throw new Error(`Observation name already exists: ${field.name}`);
    names.add(field.name);
    if (!WIDTHS.has(field.width) || !Number.isInteger(field.address) || field.address < 0 || field.address + field.width - 1 > 0xffffffff) throw new Error(`Invalid address or number size for ${field.name}.`);
  }
  return structuredClone(config);
}
export function matchObservationGame(config, game) {
  if (config.game.code !== (game.game_code ?? game.code) || config.game.revision !== game.revision) throw new Error(`This configuration requires ${config.game.code} revision ${config.game.revision}.`);
}

export function mountObservations({ root, getEmulator, getUser }) {
  root.innerHTML = `<details class="workshop-observations" data-observations>
    <summary>Automation observations <span data-observation-count>0</span></summary>
    <p>Name memory values for Python experiments. These are separate from your reader's display fields.</p>
    <p data-observation-game></p>
    <form data-observation-form>
      <label>Observation name<input data-observation-name maxlength="128" placeholder="player.hp" required spellcheck="false"></label>
      <div class="workshop-grid"><label>Address<input data-observation-address placeholder="0x02000000" required spellcheck="false"></label>
      <label>Number size<select data-observation-width><option value="1">8-bit unsigned</option><option value="2" selected>16-bit unsigned</option><option value="4">32-bit unsigned</option></select></label></div>
      <div class="workshop-actions"><button type="submit" data-observation-submit>Add observation</button><button type="button" data-observation-cancel hidden>Cancel edit</button></div>
    </form>
    <ul data-observation-list></ul>
    <div class="workshop-actions"><button type="button" data-observation-export>Export observations</button><label class="observation-import">Import configuration<input type="file" data-observation-import accept=".json,application/json"></label></div>
    <p>In Python: <code>TinyBird(rom, observations="game.observations.json")</code>. Names appear in <code>observation["memory"]</code>.</p>
    <a href="/reader-guide#automation" target="_blank" rel="noopener">Python setup and example</a>
    <p data-observation-status role="status" aria-live="polite"></p>
  </details>`;
  const $ = name => root.querySelector(`[data-observation-${name}]`);
  let key = null, game = null, fields = [], editing = null;
  const addressText = address => `0x${address.toString(16).toUpperCase().padStart(8, '0')}`;
  function message(text) { $('status').textContent = text; }
  function cancel() {
    editing = null; $('name').value = ''; $('address').value = '';
    $('submit').textContent = 'Add observation'; $('cancel').hidden = true;
  }
  function render() {
    $('count').textContent = String(fields.length);
    $('export').disabled = !game || !fields.length;
    $('list').replaceChildren(...fields.map((field, index) => {
      const row = document.createElement('li');
      const details = document.createElement('span');
      const name = document.createElement('strong'); name.textContent = field.name;
      const address = document.createElement('small'); address.textContent = `${addressText(field.address)} · ${field.width * 8}-bit`;
      details.append(name, address);
      const value = document.createElement('output'); value.dataset.observationValue = index;
      value.setAttribute('aria-label', `${field.name} current value`);
      const edit = document.createElement('button'); edit.type = 'button'; edit.textContent = 'Edit';
      edit.addEventListener('click', () => {
        if (update()) return;
        editing = field.name; $('name').value = field.name; $('address').value = addressText(field.address); $('width').value = field.width;
        $('submit').textContent = 'Save observation'; $('cancel').hidden = false; $('name').focus();
      });
      const remove = document.createElement('button'); remove.type = 'button'; remove.textContent = 'Remove';
      remove.setAttribute('aria-label', `Remove ${field.name}`);
      remove.addEventListener('click', () => {
        if (update()) return;
        fields = fields.filter(item => item.name !== field.name); cancel(); persist(); render();
      });
      row.append(details, value, edit, remove); return row;
    }));
  }
  function persist() {
    try {
      if (fields.length) localStorage.setItem(key, JSON.stringify(observationConfig(game, fields)));
      else localStorage.removeItem(key);
      message('Observations saved in this browser. Export a copy to use with Python.');
    } catch (error) { message(`Could not save in this browser: ${error.message}. You can still export the configuration.`); }
  }
  function update() {
    const emu = getEmulator();
    const nextGame = emu?.hasRom ? emu.snapshot().rom : null;
    const nextKey = nextGame ? `tinybird:observations:${getUser()?.id ?? 'guest'}:${nextGame.game_code}:${nextGame.revision}` : null;
    const changed = key !== nextKey;
    if (changed) {
      key = nextKey; game = nextGame; fields = []; cancel(); message('');
      if (key) {
        try {
          const text = localStorage.getItem(key);
          if (text) { const config = validateObservationConfig(JSON.parse(text)); matchObservationGame(config, game); fields = config.fields; }
        } catch (error) { message(`Saved observations could not be opened: ${error.message}`); }
      }
      render();
    }
    $('game').textContent = game ? `${game.game_code} · revision ${game.revision}` : 'Load a game in the test player to create observations.';
    for (const output of root.querySelectorAll('[data-observation-value]')) {
      const field = fields[Number(output.dataset.observationValue)];
      try {
        const bytes = emu.readMemory(field.address, field.width);
        output.textContent = String(bytes.reduce((value, byte, index) => value + byte * 2 ** (8 * index), 0));
      } catch { output.textContent = 'Unavailable'; }
    }
    return changed;
  }
  $('form').addEventListener('submit', event => {
    event.preventDefault();
    try {
      if (update()) throw new Error('The game or account changed. Enter the observation for the current game.');
      if (!game) throw new Error('Load a game first.');
      const raw = $('address').value.trim();
      if (!/^(0x[0-9a-f]+|\d+)$/i.test(raw)) throw new Error('Enter a decimal or hexadecimal memory address.');
      const field = { name: $('name').value.trim(), address: Number(raw), width: Number($('width').value) };
      const next = editing ? fields.map(item => item.name === editing ? field : item) : [...fields, field];
      fields = observationConfig(game, next).fields; cancel(); persist(); render(); update();
    } catch (error) { message(error.message); }
  });
  $('cancel').addEventListener('click', cancel);
  $('export').addEventListener('click', () => {
    try {
      if (update()) throw new Error('The game or account changed. Check the observations before exporting.');
      if (!game) throw new Error('Load a game first.');
      const config = observationConfig(game, fields);
      const url = URL.createObjectURL(new Blob([JSON.stringify(config, null, 2) + '\n'], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url; link.download = `${game.game_code}-r${game.revision}.observations.json`; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      message('Exported. Load this file with the Python client or --observations in tinybird-headless.');
    } catch (error) { message(error.message); }
  });
  $('import').addEventListener('change', async event => {
    try {
      update(); const capturedKey = key; const file = event.target.files[0];
      if (!file) return;
      if (!game) throw new Error('Load the matching game first.');
      if (file.size > 1_048_576) throw new Error('Configuration exceeds 1 MiB.');
      const config = validateObservationConfig(JSON.parse(await file.text()));
      update(); if (capturedKey !== key) throw new Error('Game or account changed during import. Try again.');
      matchObservationGame(config, game); fields = config.fields; cancel(); persist(); render(); update();
    } catch (error) { message(error.message); }
    finally { event.target.value = ''; }
  });
  render(); update();
  return { update, useAddress(address, width) {
    update(); cancel(); root.querySelector('[data-observations]').open = true;
    $('address').value = addressText(address); $('width').value = width;
    root.scrollIntoView({ block: 'nearest' }); $('name').focus();
  } };
}

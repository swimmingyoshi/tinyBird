// One automatic checkpoint per account in this browser. Named vault saves are separate.
export const RECOVERY_INTERVAL = 30_000;
export const recoveryOwner = user => String(user?.id ?? 'guest');
export const observationKey = (owner, game) => `tinybird:observations:${owner}:${game.game_code}:${game.revision}`;

export async function fingerprint(bytes) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(x => x.toString(16).padStart(2, '0')).join('');
}

async function database() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('tinybird-recovery', 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore('checkpoints');
      request.result.createObjectStore('roms');
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () => reject(new Error('Close other tinyBird tabs and try again.'));
  });
}

export async function readRecovery(owner) {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(['checkpoints', 'roms']);
      const checkpoint = tx.objectStore('checkpoints').get(owner);
      const rom = tx.objectStore('roms').get(owner);
      tx.oncomplete = () => resolve(checkpoint.result ? { ...checkpoint.result, rom: rom.result?.bytes } : null);
      tx.onabort = tx.onerror = () => reject(tx.error);
    });
  } finally { db.close(); }
}

export async function writeRecovery(owner, checkpoint, rom) {
  const db = await database();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction(['checkpoints', 'roms'], 'readwrite');
      const checkpoints = tx.objectStore('checkpoints'), roms = tx.objectStore('roms');
      const previous = checkpoints.get(owner);
      previous.onsuccess = () => {
        if (previous.result?.savedAt > checkpoint.savedAt) return;
        if (previous.result?.romHash !== checkpoint.romHash) roms.put({ hash: checkpoint.romHash, bytes: rom }, owner);
        checkpoints.put(checkpoint, owner);
      };
      tx.oncomplete = resolve;
      tx.onabort = tx.onerror = () => reject(tx.error ?? new Error('Browser storage is unavailable.'));
    });
  } finally { db.close(); }
}

export async function validateRecovery(record, bios) {
  if (!record || record.version !== 1 || !(record.rom instanceof Uint8Array) || !(record.state instanceof Uint8Array)) throw new Error('This recovery checkpoint is incomplete or from an unsupported version.');
  if (await fingerprint(record.rom) !== record.romHash) throw new Error('The recovery ROM is damaged or does not match.');
  if (await fingerprint(record.state) !== record.stateHash) throw new Error('The recovery state is damaged.');
  const code = String.fromCharCode(...record.rom.slice(0xac, 0xb0));
  if (code !== record.game.game_code || record.rom[0xbc] !== record.game.revision) throw new Error('The game or revision does not match this checkpoint.');
  if ((bios ? await fingerprint(bios) : null) !== record.biosHash) throw new Error('This checkpoint needs the same BIOS used when it was saved.');
}

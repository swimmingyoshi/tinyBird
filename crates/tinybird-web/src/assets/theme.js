// Appearance: which palette the site wears, and the picture behind it.
//
// Both live on the device rather than the account. A theme is a preference in
// the same class as volume or scanlines, and the background is a file the
// person already has — neither is worth a round trip to a server, and neither
// should wait on being signed in.
//
// The palette is a set of custom properties written onto <html>. Every page
// takes its colours from those, so applying a theme is one attribute and one
// style block: nothing re-renders and no stylesheet is swapped.

/** Themes, in menu order. `id` is what gets stored, so it must not change. */
export const THEMES = [
  {
    id: "default",
    name: "Amber (default)",
    tokens: {
      "--void": "#06090f",
      "--shell": "#101724",
      "--rule": "#24304a",
      "--amber": "#ff9f1c",
      "--teal": "#2ec4b6",
      "--ink": "#f7f3ec",
      "--dim": "#7c8aa3",
      "--play-shell": "#111923",
      "--play-rule": "#26313e",
      "--play-ink": "#edf2f8",
      "--play-dim": "#9ba9bc",
      "--play-accent": "#b9eed4",
      "--play-sunk": "#0b131d",
      "--play-raise": "#1c2935",
      "--play-line": "#35414e",
    },
  },
  {
    id: "midnight",
    name: "Midnight",
    tokens: {
      "--void": "#05070f",
      "--shell": "#0e1424",
      "--rule": "#232f4d",
      "--amber": "#8ab4ff",
      "--teal": "#6ee7ff",
      "--ink": "#eef2ff",
      "--dim": "#8492b4",
      "--play-shell": "#101728",
      "--play-rule": "#263250",
      "--play-ink": "#eaf0ff",
      "--play-dim": "#97a4c4",
      "--play-accent": "#a9c8ff",
      "--play-sunk": "#0a1020",
      "--play-raise": "#1b2540",
      "--play-line": "#33406a",
    },
  },
  {
    id: "gameboy",
    name: "Game Boy",
    tokens: {
      "--void": "#0b1207",
      "--shell": "#14200f",
      "--rule": "#2b3f20",
      "--amber": "#cfe36b",
      "--teal": "#8bc34a",
      "--ink": "#e9f5d0",
      "--dim": "#8ea377",
      "--play-shell": "#16240f",
      "--play-rule": "#2f4423",
      "--play-ink": "#e9f5d0",
      "--play-dim": "#9bb083",
      "--play-accent": "#b8e986",
      "--play-sunk": "#0d1708",
      "--play-raise": "#22331a",
      "--play-line": "#3b5429",
    },
  },
  {
    id: "ember",
    name: "Ember",
    tokens: {
      "--void": "#0d0705",
      "--shell": "#1d100c",
      "--rule": "#40241b",
      "--amber": "#ff8a3d",
      "--teal": "#ffc978",
      "--ink": "#fdeee4",
      "--dim": "#b08a7a",
      "--play-shell": "#20120d",
      "--play-rule": "#46281e",
      "--play-ink": "#fdeee4",
      "--play-dim": "#bb9484",
      "--play-accent": "#ffb37a",
      "--play-sunk": "#140a07",
      "--play-raise": "#2d1a12",
      "--play-line": "#55311f",
    },
  },
  {
    id: "orchid",
    name: "Orchid",
    tokens: {
      "--void": "#0a0713",
      "--shell": "#170f26",
      "--rule": "#33234d",
      "--amber": "#d08cff",
      "--teal": "#7ce0ff",
      "--ink": "#f3ecff",
      "--dim": "#9b8bb8",
      "--play-shell": "#1a1129",
      "--play-rule": "#382654",
      "--play-ink": "#f3ecff",
      "--play-dim": "#a294c0",
      "--play-accent": "#cfa8ff",
      "--play-sunk": "#0d0918",
      "--play-raise": "#261a3b",
      "--play-line": "#443066",
    },
  },
];

export const DEFAULT_THEME = THEMES[0].id;

/** Twelve megabytes: a generous wallpaper, and far short of a video by mistake. */
export const BACKGROUND_LIMIT = 12 * 1024 * 1024;

const KEY_THEME = "tinybird:theme";
const KEY_DIM = "tinybird:bg-dim";
const KEY_BLUR = "tinybird:bg-blur";
const KEY_PANEL = "tinybird:panel-alpha";
const KEY_SCREEN = "tinybird:screen-alpha";
const KEY_READOUTS = "tinybird:readouts";

function read(key, fallback) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    // A private window, or a browser told to keep no site data.
    return fallback;
  }
}

function write(key, value) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // The setting just will not outlive the tab.
  }
}

/** The stored theme, or the default when it names one that no longer exists. */
export function currentTheme() {
  const id = read(KEY_THEME, DEFAULT_THEME);
  return THEMES.some((theme) => theme.id === id) ? id : DEFAULT_THEME;
}

export function backgroundSettings() {
  const dim = Number(read(KEY_DIM, "45"));
  const blur = Number(read(KEY_BLUR, "0"));
  return {
    dim: Number.isFinite(dim) ? Math.min(90, Math.max(0, dim)) : 45,
    blur: Number.isFinite(blur) ? Math.min(24, Math.max(0, blur)) : 0,
  };
}

function clampPercent(raw, fallback) {
  const value = Number(raw);
  return Number.isFinite(value) ? Math.min(100, Math.max(20, value)) : fallback;
}

/**
 * How opaque the panels and the picture are, as percentages.
 *
 * Both default to 100, which is what the stylesheet already does, so a device
 * that has never touched these looks exactly as it did.
 */
export function surfaceSettings() {
  return {
    panel: clampPercent(read(KEY_PANEL, "100"), 100),
    screen: clampPercent(read(KEY_SCREEN, "100"), 100),
  };
}

export function applySurfaces(settings = surfaceSettings()) {
  const root = document.documentElement;
  root.style.setProperty("--panel-alpha", `${settings.panel}%`);
  root.style.setProperty("--screen-alpha", `${settings.screen}%`);
}

export function setSurfaceSettings({ panel, screen }) {
  const root = document.documentElement;
  if (panel !== undefined) {
    write(KEY_PANEL, panel);
    root.style.setProperty("--panel-alpha", `${panel}%`);
  }
  if (screen !== undefined) {
    write(KEY_SCREEN, screen);
    root.style.setProperty("--screen-alpha", `${screen}%`);
  }
}

/** Whether the frame counter and the key hints are being kept out of the way. */
export function readoutsHidden() {
  return read(KEY_READOUTS, "shown") === "hidden";
}

export function applyReadouts(hidden = readoutsHidden()) {
  const root = document.documentElement;
  if (hidden) root.dataset.readouts = "hidden";
  else root.removeAttribute("data-readouts");
}

export function setReadoutsHidden(hidden) {
  write(KEY_READOUTS, hidden ? "hidden" : "shown");
  applyReadouts(hidden);
}

/**
 * Paint a theme onto <html>.
 *
 * The properties are set one at a time rather than through a single cssText
 * write, so that anything else already inline — the background image, the
 * measured deck cap — survives a change of theme.
 */
export function applyTheme(id = currentTheme()) {
  const theme = THEMES.find((entry) => entry.id === id) ?? THEMES[0];
  const root = document.documentElement;
  for (const [name, value] of Object.entries(theme.tokens)) {
    root.style.setProperty(name, value);
  }
  root.dataset.theme = theme.id;
  return theme.id;
}

export function setTheme(id) {
  const applied = applyTheme(id);
  write(KEY_THEME, applied);
  return applied;
}

// --- the background picture ---------------------------------------------
//
// It is kept in IndexedDB, as the blob the file picker handed over.
// localStorage would mean a data URI, which is base64 and so a third larger
// again, against a quota of about five megabytes that the saves and every
// other preference already share: one ordinary wallpaper would fill it and
// start breaking things that matter more than a picture. IndexedDB takes the
// bytes as they are, and is measured in hundreds of megabytes.

const DB_NAME = "tinybird";
const STORE = "appearance";
const BG_KEY = "background";

function openDb() {
  return new Promise((resolve, reject) => {
    let request;
    try {
      request = indexedDB.open(DB_NAME, 1);
    } catch (error) {
      reject(error);
      return;
    }
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () => reject(new Error("blocked"));
  });
}

async function withStore(mode, run) {
  const db = await openDb();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const request = run(tx.objectStore(STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  } finally {
    db.close();
  }
}

export async function loadBackground() {
  try {
    return (await withStore("readonly", (store) => store.get(BG_KEY))) ?? null;
  } catch {
    // No store, or a browser keeping no site data. No picture is a fine answer.
    return null;
  }
}

export async function saveBackground(blob) {
  await withStore("readwrite", (store) => store.put(blob, BG_KEY));
}

export async function clearBackground() {
  try {
    await withStore("readwrite", (store) => store.delete(BG_KEY));
  } catch {
    // Nothing stored, or no store to delete it from.
  }
}

let objectUrl = null;

/**
 * Hang a blob behind the page, or take the picture away when given nothing.
 *
 * The old object URL is revoked on replacement: each one pins its blob in
 * memory until it is, and trying a few wallpapers in a sitting would otherwise
 * keep every one of them alive.
 */
export function paintBackground(blob, settings = backgroundSettings()) {
  const root = document.documentElement;
  if (objectUrl) {
    URL.revokeObjectURL(objectUrl);
    objectUrl = null;
  }
  if (!blob) {
    root.removeAttribute("data-bg");
    root.style.removeProperty("--bg-image");
    return;
  }
  objectUrl = URL.createObjectURL(blob);
  root.style.setProperty("--bg-image", `url("${objectUrl}")`);
  root.style.setProperty("--bg-dim", String(settings.dim / 100));
  root.style.setProperty("--bg-blur", `${settings.blur}px`);
  root.dataset.bg = "on";
}

export function setBackgroundSettings({ dim, blur }) {
  const root = document.documentElement;
  if (dim !== undefined) {
    write(KEY_DIM, dim);
    root.style.setProperty("--bg-dim", String(dim / 100));
  }
  if (blur !== undefined) {
    write(KEY_BLUR, blur);
    root.style.setProperty("--bg-blur", `${blur}px`);
  }
}

/**
 * Put the stored appearance on the page.
 *
 * Every page calls this as early as it can. The theme lands synchronously, so
 * there is no flash of the previous palette; only the picture, which has to
 * come out of a database, arrives a beat later.
 */
export async function mountTheme() {
  applyTheme();
  applySurfaces();
  applyReadouts();
  const blob = await loadBackground();
  if (blob) paintBackground(blob);
  return blob;
}

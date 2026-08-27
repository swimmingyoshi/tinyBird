// The on-the-wire format for a stored save.
//
// Kept free of any DOM reference so it can be exercised outside a browser;
// `saveformat.test.mjs` does exactly that.
//
// A stored save is a container rather than a bare state:
//
//   0..4   "TBCT"
//   4      format version
//   5      flags, bit 0 = the state that follows is gzipped
//   6..10  thumbnail length, u32 little-endian
//   10..14 battery save length, u32 little-endian   (version 2 and later)
//   then   thumbnail (PNG)
//   then   battery save
//   then   the save state
//
// The screenshot lives *inside* the save so the two can never orphan or
// disagree: one object to upload, prune, and delete. Reading a thumbnail back
// costs one ranged request for the first few kilobytes rather than the whole
// five megabytes, which is what makes showing them in a list affordable.
//
// The battery save rides along for a different reason. A save state is a
// photograph of the whole machine; a battery save is what the cartridge itself
// keeps, and it is what the game writes when the player picks "save" from a
// menu. They are not interchangeable — restoring a state without the battery
// leaves the game believing in a save file that is not there — so a stored save
// carries both.

/**
 * Container magic.
 *
 * Deliberately NOT "TBSV": that is the magic the emulator core already writes
 * at the head of every save state, and reusing it made a raw state look like a
 * container whose thumbnail length was really the state's version field.
 */
export const CONTAINER_MAGIC = "TBCT";
/** Magic the core writes at the start of a save state. Must never be ours. */
export const STATE_MAGIC = "TBSV";
export const CONTAINER_VERSION = 2;
/** Header size by version: version 1 had no battery-save length. */
const HEADER_BYTES_V1 = 10;
export const CONTAINER_HEADER_BYTES = 14;
const FLAG_GZIPPED = 1;
/** Gzip magic number, for saves stored before the container existed. */
const GZIP_MAGIC = [0x1f, 0x8b];
/** Enough to cover the header and any 240x160 PNG we produce. */
export const THUMBNAIL_PREFIX_BYTES = 32 * 1024;
/** A thumbnail larger than this means the header was misread. */
const MAX_THUMBNAIL_BYTES = 1024 * 1024;
/** The largest GBA backup chip is 128 KB; more than this means a misread. */
const MAX_BATTERY_BYTES = 1024 * 1024;

function hasMagic(bytes, magic) {
  if (bytes.byteLength < magic.length) return false;
  for (let i = 0; i < magic.length; i += 1) {
    if (bytes[i] !== magic.charCodeAt(i)) return false;
  }
  return true;
}

export async function gzip(bytes) {
  if (typeof CompressionStream === "undefined") return { bytes, gzipped: false };
  const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream("gzip"));
  return { bytes: new Uint8Array(await new Response(stream).arrayBuffer()), gzipped: true };
}

export async function gunzip(bytes) {
  if (typeof DecompressionStream === "undefined") {
    throw new Error("this browser cannot decompress saves");
  }
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
  return await new Response(stream).arrayBuffer();
}

/** Wrap a state, its screenshot, and the cartridge save into one blob. */
export function packSave(stateBytes, gzipped, thumbnail, battery) {
  const thumb = thumbnail ?? new Uint8Array(0);
  const batt = battery ?? new Uint8Array(0);
  const out = new Uint8Array(
    CONTAINER_HEADER_BYTES + thumb.byteLength + batt.byteLength + stateBytes.byteLength,
  );
  const view = new DataView(out.buffer);

  for (let i = 0; i < 4; i += 1) out[i] = CONTAINER_MAGIC.charCodeAt(i);
  out[4] = CONTAINER_VERSION;
  out[5] = gzipped ? FLAG_GZIPPED : 0;
  view.setUint32(6, thumb.byteLength, true);
  view.setUint32(10, batt.byteLength, true);
  out.set(thumb, CONTAINER_HEADER_BYTES);
  out.set(batt, CONTAINER_HEADER_BYTES + thumb.byteLength);
  out.set(stateBytes, CONTAINER_HEADER_BYTES + thumb.byteLength + batt.byteLength);
  return out;
}

/**
 * Read the container header, or null if `buffer` is not one.
 *
 * The version and length are validated, not just the magic: a four-byte match
 * is weak evidence, and misreading a save state as a container corrupts it
 * silently rather than failing loudly.
 */
export function readContainerHeader(buffer) {
  const bytes = new Uint8Array(buffer);
  if (bytes.byteLength < HEADER_BYTES_V1) return null;
  if (!hasMagic(bytes, CONTAINER_MAGIC)) return null;

  const version = bytes[4];
  if (version < 1 || version > CONTAINER_VERSION) return null;

  const headerBytes = version === 1 ? HEADER_BYTES_V1 : CONTAINER_HEADER_BYTES;
  if (bytes.byteLength < headerBytes) return null;

  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const thumbnailLength = view.getUint32(6, true);
  if (thumbnailLength > MAX_THUMBNAIL_BYTES) return null;

  // Version 1 predates battery saves; it simply has none.
  const batteryLength = version === 1 ? 0 : view.getUint32(10, true);
  if (batteryLength > MAX_BATTERY_BYTES) return null;

  return {
    version,
    headerBytes,
    gzipped: (bytes[5] & FLAG_GZIPPED) !== 0,
    thumbnailLength,
    batteryLength,
  };
}

/**
 * Recover a save state from whatever form it was stored in.
 *
 * Handles the container, a bare gzip blob, and a raw state, so a file from any
 * earlier version — or straight from the desktop app — still loads.
 */
export async function unpackSave(buffer) {
  const header = readContainerHeader(buffer);
  if (header) {
    const start = header.headerBytes + header.thumbnailLength + header.batteryLength;
    const state = new Uint8Array(buffer).subarray(start);
    return header.gzipped ? await gunzip(state) : state.slice().buffer;
  }

  const bytes = new Uint8Array(buffer);
  if (bytes[0] === GZIP_MAGIC[0] && bytes[1] === GZIP_MAGIC[1]) return await gunzip(bytes);
  return buffer;
}

/** The cartridge save stored alongside the state, or null if there is none. */
export function readBattery(buffer) {
  const header = readContainerHeader(buffer);
  if (!header || header.batteryLength === 0) return null;

  const start = header.headerBytes + header.thumbnailLength;
  const end = start + header.batteryLength;
  if (buffer.byteLength < end) return null;
  return new Uint8Array(buffer).slice(start, end);
}

/** Extract the thumbnail from a container prefix, or null if there is none. */
export function readThumbnail(buffer) {
  const header = readContainerHeader(buffer);
  if (!header || header.thumbnailLength === 0) return null;

  const end = header.headerBytes + header.thumbnailLength;
  // A short read means the prefix did not cover the whole image; better no
  // thumbnail than a truncated one the decoder will reject noisily.
  if (buffer.byteLength < end) return null;
  return new Uint8Array(buffer).slice(header.headerBytes, end);
}

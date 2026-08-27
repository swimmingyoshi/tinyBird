// Tests for the stored-save container.
//
//   node --test crates/tinybird-web/src/assets/saveformat.test.mjs
//
// This exists because the first version of the container reused "TBSV" as its
// magic — the same four bytes the emulator core writes at the head of every
// save state. Every raw state was therefore misread as a container, its version
// field interpreted as a thumbnail length, and the state silently truncated.

import { strict as assert } from "node:assert";
import test from "node:test";

import {
  CONTAINER_HEADER_BYTES,
  CONTAINER_MAGIC,
  STATE_MAGIC,
  gzip,
  packSave,
  readBattery,
  readContainerHeader,
  readThumbnail,
  unpackSave,
} from "./saveformat.js";

/**
 * A save state as the core writes it: "TBSV" + version u32 LE + payload.
 *
 * The version is a parameter because it is what sits where a container keeps
 * its own version byte, and the collision is only visible at some values.
 */
function fakeState(payload = "the-emulator-state", stateVersion = 3) {
  const body = new TextEncoder().encode(payload);
  const out = new Uint8Array(8 + body.length);
  for (let i = 0; i < 4; i += 1) out[i] = STATE_MAGIC.charCodeAt(i);
  new DataView(out.buffer).setUint32(4, stateVersion, true);
  out.set(body, 8);
  return out;
}

const fakeThumbnail = () => new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3, 4, 5]);

const bufferOf = (bytes) =>
  bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);

test("the container magic is not the save state magic", () => {
  // The bug this whole file exists for.
  assert.notEqual(CONTAINER_MAGIC, STATE_MAGIC);
});

test("a raw save state passes through untouched", async () => {
  // Every savestate version, including 1 — where the state's version field
  // lines up with the container's, so only the magic tells them apart.
  for (const version of [1, 2, 3, 4]) {
    const state = fakeState("the-emulator-state", version);
    const out = new Uint8Array(await unpackSave(bufferOf(state)));
    assert.deepEqual([...out], [...state], `savestate version ${version}`);
  }
});

test("a raw save state is not mistaken for a container", () => {
  for (const version of [1, 2, 3, 4]) {
    const state = bufferOf(fakeState("the-emulator-state", version));
    assert.equal(readContainerHeader(state), null, `savestate version ${version}`);
    assert.equal(readThumbnail(state), null, `savestate version ${version}`);
  }
});

test("a save state that reads like a plausible header survives", () => {
  // The shape that actually broke: savestate version 1, then a bincode payload
  // whose leading bytes are small integers. Under the old colliding magic this
  // parsed as a container with a zero-length thumbnail, and unpacking silently
  // sliced the first two payload bytes off the state.
  const state = new Uint8Array([
    ...[...STATE_MAGIC].map((c) => c.charCodeAt(0)),
    1, 0, 0, 0, // savestate version 1, u32 LE
    0, 0, 7, 7, 7, 7, // payload: leading zeroes, as bincode tends to produce
  ]);
  assert.equal(readContainerHeader(bufferOf(state)), null);
});

test("an uncompressed container round-trips", async () => {
  const state = fakeState();
  const packed = packSave(state, false, fakeThumbnail());
  const out = new Uint8Array(await unpackSave(bufferOf(packed)));
  assert.deepEqual([...out], [...state]);
});

test("a compressed container round-trips", async () => {
  const state = fakeState("a".repeat(5000));
  const compressed = await gzip(state);
  assert.ok(compressed.gzipped, "Node should provide CompressionStream");

  const packed = packSave(compressed.bytes, true, fakeThumbnail());
  assert.ok(packed.byteLength < state.byteLength, "compression should shrink it");

  const out = new Uint8Array(await unpackSave(bufferOf(packed)));
  assert.deepEqual([...out], [...state]);
});

test("a bare gzip blob still loads", async () => {
  // Saves written before the container existed.
  const state = fakeState("legacy");
  const compressed = await gzip(state);
  const out = new Uint8Array(await unpackSave(bufferOf(compressed.bytes)));
  assert.deepEqual([...out], [...state]);
});

test("the thumbnail comes back byte for byte", () => {
  const thumbnail = fakeThumbnail();
  const packed = packSave(fakeState(), false, thumbnail);
  assert.deepEqual([...readThumbnail(bufferOf(packed))], [...thumbnail]);
});

test("a container without a thumbnail reports none", () => {
  const packed = packSave(fakeState(), false, null);
  assert.equal(readThumbnail(bufferOf(packed)), null);
});

test("a truncated prefix yields no thumbnail rather than a broken one", () => {
  const packed = packSave(fakeState(), false, fakeThumbnail());
  const prefix = packed.slice(0, CONTAINER_HEADER_BYTES + 3);
  assert.equal(readThumbnail(bufferOf(prefix)), null);
});

test("a wrong container version is rejected", () => {
  const packed = packSave(fakeState(), false, fakeThumbnail());
  packed[4] = 99;
  assert.equal(readContainerHeader(bufferOf(packed)), null);
});

test("an absurd thumbnail length is rejected", () => {
  // What a misread header looks like; better to ignore than to slice wildly.
  const packed = packSave(fakeState(), false, fakeThumbnail());
  new DataView(packed.buffer).setUint32(6, 0xffffffff, true);
  assert.equal(readContainerHeader(bufferOf(packed)), null);
});

test("short buffers do not throw", async () => {
  for (const length of [0, 1, 4, 9]) {
    const bytes = new Uint8Array(length);
    assert.equal(readContainerHeader(bufferOf(bytes)), null);
    assert.equal(readThumbnail(bufferOf(bytes)), null);
    await unpackSave(bufferOf(bytes));
  }
});

// --- battery saves --------------------------------------------------------

const fakeBattery = () => new Uint8Array(Array.from({ length: 64 }, (_, i) => i));

test("the cartridge save round-trips beside the state", async () => {
  const state = fakeState();
  const battery = fakeBattery();
  const packed = packSave(state, false, fakeThumbnail(), battery);

  assert.deepEqual([...readBattery(bufferOf(packed))], [...battery]);
  assert.deepEqual([...readThumbnail(bufferOf(packed))], [...fakeThumbnail()]);
  const out = new Uint8Array(await unpackSave(bufferOf(packed)));
  assert.deepEqual([...out], [...state], "the state must survive too");
});

test("a compressed container carries the cartridge save uncompressed", async () => {
  // The thumbnail and battery sit ahead of the gzip stream so a ranged read can
  // reach them without inflating five megabytes.
  const state = fakeState("b".repeat(4000));
  const battery = fakeBattery();
  const compressed = await gzip(state);
  const packed = packSave(compressed.bytes, compressed.gzipped, fakeThumbnail(), battery);

  assert.deepEqual([...readBattery(bufferOf(packed))], [...battery]);
  const out = new Uint8Array(await unpackSave(bufferOf(packed)));
  assert.deepEqual([...out], [...state]);
});

test("a save with no cartridge backup reports no battery", () => {
  const packed = packSave(fakeState(), false, fakeThumbnail(), null);
  assert.equal(readBattery(bufferOf(packed)), null);
});

/// Saves written before the battery field existed must keep working.
test("a version 1 container still loads", async () => {
  const state = fakeState();
  const thumbnail = fakeThumbnail();
  const v1 = new Uint8Array(10 + thumbnail.byteLength + state.byteLength);
  for (let i = 0; i < 4; i += 1) v1[i] = CONTAINER_MAGIC.charCodeAt(i);
  v1[4] = 1; // version
  v1[5] = 0; // not gzipped
  new DataView(v1.buffer).setUint32(6, thumbnail.byteLength, true);
  v1.set(thumbnail, 10);
  v1.set(state, 10 + thumbnail.byteLength);

  const header = readContainerHeader(bufferOf(v1));
  assert.equal(header.version, 1);
  assert.equal(header.headerBytes, 10);
  assert.equal(header.batteryLength, 0);
  assert.deepEqual([...readThumbnail(bufferOf(v1))], [...thumbnail]);
  assert.equal(readBattery(bufferOf(v1)), null);
  assert.deepEqual([...new Uint8Array(await unpackSave(bufferOf(v1)))], [...state]);
});

test("a version from the future is refused rather than misread", () => {
  const packed = packSave(fakeState(), false, fakeThumbnail(), fakeBattery());
  packed[4] = 99;
  assert.equal(readContainerHeader(bufferOf(packed)), null);
});

test("an absurd battery length is rejected", () => {
  const packed = packSave(fakeState(), false, fakeThumbnail(), fakeBattery());
  new DataView(packed.buffer).setUint32(10, 0xffffffff, true);
  assert.equal(readContainerHeader(bufferOf(packed)), null);
});

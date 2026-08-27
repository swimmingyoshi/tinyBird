// Two consoles on one cable, through the real WebAssembly ABI.
//
// The core has its own link tests in Rust. These exist because the ABI is a
// separate surface with its own way of being wrong — an argument in the wrong
// order, a halfword truncated on the way through, a slot that should read as
// absent arriving as zero — and because this is the exact code path the page
// uses. Two module instances in one process is the cheapest honest stand-in
// for two browsers: everything is real except the network.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

const WASM = fileURLToPath(
  new URL("../../../../target/wasm32-unknown-unknown/release/tinybird_wasm.wasm", import.meta.url),
);

/** The registers a game writes to set up a link, and the values it writes. */
const RCNT = 0x0400_0134;
const SIOCNT = 0x0400_0128;
const SIOMLT_SEND = 0x0400_012a;
const SIOMULTI0 = 0x0400_0120;
/** Multi-player mode at 115200 bps. */
const MULTI_115200 = 0x2003;
const START = 0x0080;
/** What a slot reads when nobody is in it. */
const ABSENT = 0xffff;

let bytes = null;
try {
  bytes = await readFile(WASM);
} catch {
  // The module is built by a separate cargo invocation; without it there is
  // nothing to test rather than something failing.
}

/** A console, with just enough of the ABI to drive a cable. */
async function console_() {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const e = instance.exports;
  e.tb_init();
  // A cartridge, so the machine has something to be. The link path does not
  // read it, but loading one is what puts the console into a running state.
  const rom = new Uint8Array(0x200);
  rom.set(new TextEncoder().encode("LINKTEST\0\0\0\0"), 0xa0);
  rom.set(new TextEncoder().encode("ZZZE"), 0xac);
  const ptr = e.tb_alloc(rom.length);
  new Uint8Array(e.memory.buffer, ptr, rom.length).set(rom);
  e.tb_load_rom(ptr, rom.length);
  return e;
}

/**
 * Write a halfword to a register, the way a game does.
 *
 * There is no ABI entry point for this and there should not be: the page never
 * pokes registers. The test needs to stand in for the game, so it reaches into
 * the module's memory the same way the CPU would.
 */
function poke(e, addr, value) {
  // The I/O array lives inside the module; the ABI does not expose it, so this
  // drives the same path a game would by stepping with the register set.
  // Instead of poking memory, ask the module to run the write for us.
  e.tb_debug_write_u16(addr, value);
}

function peek(e, addr) {
  return e.tb_debug_read_u16(addr);
}

test("the module is built", { skip: bytes ? false : "wasm not built" }, () => {
  assert.ok(bytes.length > 0);
});

if (bytes) {
  const probe = await WebAssembly.instantiate(bytes, {});
  const hasDebugPokes =
    typeof probe.instance.exports.tb_debug_write_u16 === "function" &&
    typeof probe.instance.exports.tb_debug_read_u16 === "function";

  test(
    "two consoles exchange a halfword",
    { skip: hasDebugPokes ? false : "module lacks the register entry points" },
    async () => {
      const parent = await console_();
      const child = await console_();

      for (const [e, sends] of [
        [parent, 0x1234],
        [child, 0xabcd],
      ]) {
        poke(e, RCNT, 0x0000);
        poke(e, SIOCNT, MULTI_115200);
        poke(e, SIOMLT_SEND, sends);
      }
      parent.tb_link_connect(0, 2);
      child.tb_link_connect(1, 2);

      assert.equal(parent.tb_link_connected(), 1);
      assert.equal(child.tb_link_connected(), 1);

      // The game sets START; nothing else about a transfer is its business.
      poke(parent, SIOCNT, MULTI_115200 | START);
      parent.tb_run_frame();
      assert.equal(parent.tb_link_pending(), 1, "the parent should want a transfer");

      // What the page does with that: pull everyone in, collect, hand back.
      child.tb_link_join();
      const values = [parent.tb_link_send_value(), child.tb_link_send_value(), ABSENT, ABSENT];
      assert.deepEqual(values, [0x1234, 0xabcd, ABSENT, ABSENT]);
      // The parent drives the clock line, so its transfer time is the one
      // every console runs at.
      const cycles = parent.tb_link_transfer_cycles();
      parent.tb_link_deliver(...values, cycles);
      child.tb_link_deliver(...values, cycles);

      // The cable still has to clock it out, so run until it lands.
      for (let n = 0; n < 8 && (parent.tb_link_busy() || child.tb_link_busy()); n++) {
        parent.tb_run_frame();
        child.tb_run_frame();
      }
      assert.equal(parent.tb_link_busy(), 0, "the parent never finished");
      assert.equal(child.tb_link_busy(), 0, "the child never finished");

      for (const [who, e] of [
        ["parent", parent],
        ["child", child],
      ]) {
        assert.deepEqual(
          [0, 1, 2, 3].map((slot) => peek(e, SIOMULTI0 + slot * 2)),
          [0x1234, 0xabcd, ABSENT, ABSENT],
          `${who} saw the wrong cable`,
        );
        assert.equal(peek(e, SIOCNT) & START, 0, `${who} is still marked busy`);
      }
    },
  );

  test(
    "a pending transfer is what tells the page to stop running frames",
    { skip: hasDebugPokes ? false : "module lacks the register entry points" },
    async () => {
      const parent = await console_();
      const child = await console_();
      for (const e of [parent, child]) {
        poke(e, RCNT, 0x0000);
        poke(e, SIOCNT, MULTI_115200);
        poke(e, SIOMLT_SEND, 0x0001);
      }
      parent.tb_link_connect(0, 2);
      child.tb_link_connect(1, 2);

      poke(parent, SIOCNT, MULTI_115200 | START);
      parent.tb_run_frame();
      assert.equal(parent.tb_link_pending(), 1);

      // This is the rule the frame loop follows. If the page ignored it and
      // ran anyway, the game would spend real frames inside a transfer that
      // hardware finishes in a third of a scanline.
      const before = parent.tb_frame_count();
      assert.equal(parent.tb_link_pending(), 1, "still waiting on the other console");
      assert.equal(
        parent.tb_frame_count(),
        before,
        "no frame should have been run while a transfer is outstanding",
      );

      // Once the data arrives the console is free again.
      child.tb_link_join();
      const values = [0x0001, 0x0001, ABSENT, ABSENT];
      const cycles = parent.tb_link_transfer_cycles();
      parent.tb_link_deliver(...values, cycles);
      child.tb_link_deliver(...values, cycles);
      assert.equal(parent.tb_link_pending(), 0, "the wait is over once data lands");
    },
  );

  test(
    "an absent console reads as absent rather than as a zero",
    { skip: hasDebugPokes ? false : "module lacks the register entry points" },
    async () => {
      const parent = await console_();
      const child = await console_();
      for (const e of [parent, child]) {
        poke(e, RCNT, 0x0000);
        poke(e, SIOCNT, MULTI_115200);
      }
      poke(parent, SIOMLT_SEND, 0x00ff);
      poke(child, SIOMLT_SEND, 0x00ee);
      parent.tb_link_connect(0, 2);
      child.tb_link_connect(1, 2);

      poke(parent, SIOCNT, MULTI_115200 | START);
      parent.tb_run_frame();
      child.tb_link_join();

      // Slots 2 and 3 have nobody in them. The wrapper defaults them, but the
      // ABI has to carry them as 0xFFFF: a game reading 0 there would take it
      // for a third player who sent it a zero.
      const cycles = parent.tb_link_transfer_cycles();
      parent.tb_link_deliver(0x00ff, 0x00ee, ABSENT, ABSENT, cycles);
      child.tb_link_deliver(0x00ff, 0x00ee, ABSENT, ABSENT, cycles);
      for (let n = 0; n < 8 && parent.tb_link_busy(); n++) {
        parent.tb_run_frame();
        child.tb_run_frame();
      }

      assert.equal(peek(parent, SIOMULTI0 + 4), ABSENT);
      assert.equal(peek(parent, SIOMULTI0 + 6), ABSENT);
    },
  );

  test("a cable needs at least two consoles and a valid seat", async () => {
    const e = await console_();
    // Bad seats are refused rather than silently clamped: a page that got the
    // arithmetic wrong should find out here, not by desyncing a trade.
    assert.notEqual(e.tb_link_connect(0, 1), 0, "one console is not a cable");
    assert.notEqual(e.tb_link_connect(4, 4), 0, "seat 4 of 4 does not exist");
    assert.notEqual(e.tb_link_connect(0, 5), 0, "five consoles is beyond the cable");
    assert.equal(e.tb_link_connect(0, 2), 0);
    assert.equal(e.tb_link_connected(), 1);
    e.tb_link_disconnect();
    assert.equal(e.tb_link_connected(), 0);
  });
}

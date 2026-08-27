// Measure the host cost of one and two browser emulator instances.
//
// Run after the release WASM build:
//   node tests/benchmark_wasm_instances.mjs

import { readFile } from "node:fs/promises";
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";

import { TinyBird } from "../crates/tinybird-web/src/assets/tinybird.js";

const ROOT = new URL("../", import.meta.url);
const WASM = new URL("../target/wasm32-unknown-unknown/release/tinybird_wasm.wasm", import.meta.url);
const ROM = new URL("../roms/pokemon_fire_red.gba", import.meta.url);
const STATE = new URL(`../TradeTest${workerData?.seat ?? 1}.state`, import.meta.url);
const FRAMES = Number(process.env.TINYBIRD_BENCH_FRAMES ?? 600);

async function emulator() {
  const [wasm, rom, state] = await Promise.all([readFile(WASM), readFile(ROM), readFile(STATE)]);
  const { instance } = await WebAssembly.instantiate(wasm, {});
  const emu = new TinyBird(instance.exports);
  emu.loadRom(rom);
  emu.loadState(state);
  return emu;
}

async function run(seat = 1) {
  const emu = await emulator(seat);
  // Warm caches and the JIT before timing.
  emu.runFrames(30);
  const start = performance.now();
  emu.runFrames(FRAMES);
  const elapsed = performance.now() - start;
  return { elapsed, fps: (FRAMES * 1000) / elapsed, wasmMemoryMiB: emu.memoryBytes / 1048576 };
}

if (!isMainThread) {
  parentPort.postMessage(await run(workerData.seat));
} else {
  const oneStartMemory = process.memoryUsage().rss;
  const one = await run(1);
  const oneMemory = process.memoryUsage().rss;

  const workerStartMemory = process.memoryUsage().rss;
  const started = performance.now();
  const workers = [1, 2].map(
    (seat) =>
      new Promise((resolve, reject) => {
        const worker = new Worker(import.meta.filename, { workerData: { seat } });
        worker.once("message", resolve);
        worker.once("error", reject);
      }),
  );
  const pair = await Promise.all(workers);
  const pairElapsed = performance.now() - started;
  const pairMemory = process.memoryUsage().rss;

  process.stdout.write(
    `${JSON.stringify(
      {
        machine: { logicalCpus: (await import("node:os")).cpus().length },
        framesPerInstance: FRAMES,
        one: { ...one, rssIncreaseMiB: (oneMemory - oneStartMemory) / 1048576 },
        pairWorkers: {
          instances: pair,
          wallElapsedIncludingStartup: pairElapsed,
          aggregateFps: pair.reduce((sum, result) => sum + result.fps, 0),
          rssIncreaseMiB: (pairMemory - workerStartMemory) / 1048576,
        },
      },
      null,
      2,
    )}\n`,
  );
}

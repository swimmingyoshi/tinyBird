import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { schedule } from './pacing.js';

const source = await readFile(new URL('./play.js', import.meta.url), 'utf8');
const functions = source.slice(source.indexOf('function setFastForward(on)'), source.indexOf('el.ff.addEventListener'));
test('paced fast-forward batches due frames and submits audio at the requested rate', () => {
  const calls = [];
  const emu = { frameCount: 0,
    runAudioFrames(n) { calls.push(n); this.frameCount += n; },
    runFrame() { throw new Error('fast-forward must batch'); },
    takeAudio() { return new Float32Array(16); },
  };
  const context = vm.createContext({ emu, schedule, frameClock: 100, fastFrameMode: 'fast', fastAudio: true,
    audio: { ready: true, push(samples, speed) { calls.push(speed); } } });
  vm.runInContext(source.slice(source.indexOf('function runPaced(now, speed)'), source.indexOf('/** Run frames for a slice of wall time')), context);
  assert.equal(vm.runInContext('runPaced(117, 4)', context), 4);
  assert.deepEqual(calls, [4, 4]);
});
test('smooth mode yields after a slow frame and fast audio can be disabled', () => {
  let now = 0;
  const rates = [];
  const emu = { frameCount: 0,
    runFrame() { this.frameCount++; now += 8; },
    runAudioFrames() { throw new Error('smooth must not batch'); },
    takeAudio() { return new Float32Array(16); },
  };
  const context = vm.createContext({ emu, schedule, frameClock: 100,
    fastFrameMode: 'smooth', fastAudio: false, performance: { now: () => now },
    audio: { ready: true, push(samples, speed) { rates.push(speed); } } });
  vm.runInContext(source.slice(source.indexOf('function runPaced(now, speed)'), source.indexOf('/** Run frames for a slice of wall time')), context);
  assert.equal(vm.runInContext('runPaced(117, 4)', context), 1);
  assert.equal(context.frameClock, 0);
  assert.deepEqual(rates, []);
  context.fastAudio = true;
  vm.runInContext('runPaced(134, 4)', context);
  assert.deepEqual(rates, [4]);
});
test('switching fast forward resets the clock, flushes audio and reports its state', () => {
  const pressed = [];
  let flushed = 0;
  const context = vm.createContext({
    el: { ff: { setAttribute(name, value) { pressed.push([name, value]); } } },
    fastForward: false, frameClock: 10,
    audio: { flush() { flushed++; } },
  });
  vm.runInContext(functions, context);
  vm.runInContext('setFastForward(true)', context);
  assert.equal(context.fastForward, true);
  assert.equal(context.frameClock, 0);
  assert.equal(flushed, 1);
  assert.deepEqual(pressed, [['aria-pressed', 'true']]);
  context.frameClock = 5;
  vm.runInContext('setFastForward(true)', context);
  assert.equal(context.frameClock, 5, 'asking for the current state changes nothing');
  vm.runInContext('setFastForward(false)', context);
  assert.equal(context.fastForward, false);
  assert.deepEqual(pressed.at(-1), ['aria-pressed', 'false']);
});

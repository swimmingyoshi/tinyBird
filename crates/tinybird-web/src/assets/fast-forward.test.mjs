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
  const context = vm.createContext({ emu, schedule, frameClock: 100, lastFinished: 0, fastFrameMode: 'fast', fastAudio: true,
    audio: { ready: true, push(samples, speed) { calls.push(speed); } } });
  vm.runInContext(source.slice(source.indexOf('function runPaced(now, speed)'), source.indexOf('/** How many of the frames just asked')), context);
  vm.runInContext('runPaced(117, 4)', context);
  assert.deepEqual(calls, [4, 4]);
  assert.equal(context.lastFinished, 4);
});
test('smooth mode yields after a slow frame and fast audio can be disabled', () => {
  let now = 0;
  const rates = [];
  const emu = { frameCount: 0,
    runFrame() { this.frameCount++; now += 8; },
    runAudioFrames() { throw new Error('smooth must not batch'); },
    takeAudio() { return new Float32Array(16); },
  };
  const context = vm.createContext({ emu, schedule, frameClock: 100, lastFinished: 0,
    fastFrameMode: 'smooth', fastAudio: false, performance: { now: () => now },
    audio: { ready: true, push(samples, speed) { rates.push(speed); } } });
  vm.runInContext(source.slice(source.indexOf('function runPaced(now, speed)'), source.indexOf('/** How many of the frames just asked')), context);
  assert.equal(vm.runInContext('runPaced(117, 4)', context), 1);
  assert.equal(context.frameClock, 0);
  assert.deepEqual(rates, []);
  context.fastAudio = true;
  vm.runInContext('runPaced(134, 4)', context);
  assert.deepEqual(rates, [4]);
});
test('lobby cable cancels latched speed and blocks held inputs until disconnected', () => {
  const context = vm.createContext({
    session: null, emu: { linkConnected: false }, lobby: { connected: true },
    el: { link: { checked: false }, ff: { setAttribute() {} }, optSpeed: {} },
    fastForward: false, ffLatched: true, frameClock: 10,
    audio: { flush() {} },
  });
  vm.runInContext(functions, context);
  vm.runInContext('setFastForward(true)', context);
  assert.equal(context.fastForward, true, 'lobby without cable allows solo speed');
  context.el.link.checked = true;
  vm.runInContext('syncSpeedControls(); setFastForward(true)', context);
  assert.equal(context.fastForward, false);
  assert.equal(context.ffLatched, false);
  assert.equal(context.el.ff.disabled, true);
  assert.equal(context.el.optSpeed.disabled, true);
  assert.equal(context.frameClock, 0);
  context.lobby.connected = false;
  vm.runInContext('syncSpeedControls(); setFastForward(true)', context);
  assert.equal(context.fastForward, true);
  context.session = {};
  vm.runInContext('syncSpeedControls(); setFastForward(true)', context);
  assert.equal(context.fastForward, false, 'active session also blocks speed during teardown');
});

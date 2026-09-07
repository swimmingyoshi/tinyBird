import assert from 'node:assert/strict';
import test from 'node:test';
import { AudioSink } from './tinybird.js';

test('fast audio schedules compressed duration and flush cancels queued sources', async () => {
  const sources = [];
  const context = {
    state: 'running', currentTime: 0, destination: {},
    createGain: () => ({ gain: {}, connect() {} }),
    createBuffer: (_, frames) => ({ getChannelData: () => new Float32Array(frames) }),
    createBufferSource() {
      const source = { playbackRate: {}, connect() {}, disconnect() {},
        start(time) { this.time = time; }, stop() { this.stopped = true; } };
      sources.push(source);
      return source;
    }, close() {},
  };
  globalThis.window = { AudioContext: function () { return context; } };
  try {
    const sink = new AudioSink(1000);
    await sink.resume();
    sink.push(new Float32Array(200), 4);
    sink.push(new Float32Array(200), 4);
    assert.equal(sources[0].playbackRate.value, 4);
    assert.ok(Math.abs(sources[1].time - sources[0].time - 0.025) < 1e-9);
    sink.flush();
    assert.ok(sources.every(source => source.stopped));
    sink.push(new Float32Array(200));
    assert.equal(sources[2].playbackRate.value, 1);
    assert.equal(sources[2].time, 0.05);
    sink.close();
  } finally { delete globalThis.window; }
});

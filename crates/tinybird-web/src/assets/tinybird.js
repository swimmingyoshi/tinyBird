// The only file that knows how the WebAssembly module lays out its memory.
//
// tinybird-wasm exports a plain C ABI rather than using wasm-bindgen, so the
// whole build is `cargo build --target wasm32-unknown-unknown`. The trade is
// that pointer and length handling lives here; everything above this file works
// with ordinary JavaScript values.

/** Button bit positions. Must match BUTTON_BITS in crates/tinybird-wasm/src/lib.rs. */
export const BUTTON = Object.freeze({
  A: 1 << 0,
  B: 1 << 1,
  SELECT: 1 << 2,
  START: 1 << 3,
  RIGHT: 1 << 4,
  LEFT: 1 << 5,
  UP: 1 << 6,
  DOWN: 1 << 7,
  R: 1 << 8,
  L: 1 << 9,
});

const OK = 0;

/** Thrown when the module reports a failure, with the numeric code attached. */
export class EmulatorError extends Error {
  constructor(message, code) {
    super(code === undefined ? message : `${message} (code ${code})`);
    this.name = "EmulatorError";
    this.code = code;
  }
}

export class TinyBird {
  #exports;
  #memory;

  constructor(exports) {
    this.#exports = exports;
    this.#memory = exports.memory;
    exports.tb_init();
    this.width = exports.tb_screen_width();
    this.height = exports.tb_screen_height();
  }

  /** Bytes reserved by this emulator instance in WebAssembly linear memory. */
  get memoryBytes() {
    return this.#memory.buffer.byteLength;
  }

  /** Fetch, compile, and initialise the emulator module. */
  static async load(url = "/tinybird.wasm") {
    const response = await fetch(url);
    if (!response.ok) {
      // The server answers with a JSON hint when the module has not been built.
      let detail = `${response.status} ${response.statusText}`;
      try {
        const body = await response.json();
        if (body.error) detail = body.error;
      } catch {
        /* not JSON; the status line is the best we have */
      }
      throw new EmulatorError(`Could not load the emulator: ${detail}`);
    }

    // instantiateStreaming needs the right MIME type; fall back if a proxy
    // rewrites it, which is common behind corporate middleboxes.
    let instance;
    try {
      ({ instance } = await WebAssembly.instantiateStreaming(response.clone(), {}));
    } catch {
      const bytes = await response.arrayBuffer();
      ({ instance } = await WebAssembly.instantiate(bytes, {}));
    }
    return new TinyBird(instance.exports);
  }

  // --- memory helpers ---------------------------------------------------

  // Every view is created on demand. Growing the module's memory detaches the
  // old ArrayBuffer, so a cached view would silently read zeroes.
  #bytes(ptr, len) {
    return new Uint8Array(this.#memory.buffer, ptr, len);
  }

  /** Copy `data` into the module, run `fn(ptr, len)`, then free. */
  #withBytes(data, fn) {
    const len = data.byteLength;
    if (len === 0) throw new EmulatorError("Refusing to load an empty file.");
    const ptr = this.#exports.tb_alloc(len);
    if (ptr === 0) throw new EmulatorError("The emulator is out of memory.");
    try {
      this.#bytes(ptr, len).set(new Uint8Array(data));
      return fn(ptr, len);
    } finally {
      this.#exports.tb_free(ptr, len);
    }
  }

  #check(code, what) {
    if (code !== OK) throw new EmulatorError(what, code);
    return code;
  }

  // --- loading ----------------------------------------------------------

  loadRom(data) {
    this.#check(
      this.#withBytes(data, (ptr, len) => this.#exports.tb_load_rom(ptr, len)),
      "The ROM could not be loaded.",
    );
  }

  loadBios(data) {
    this.#check(
      this.#withBytes(data, (ptr, len) => this.#exports.tb_load_bios(ptr, len)),
      "The BIOS could not be loaded.",
    );
  }

  loadSave(data) {
    this.#check(
      this.#withBytes(data, (ptr, len) => this.#exports.tb_load_save(ptr, len)),
      "The save file could not be loaded.",
    );
  }

  // --- running ----------------------------------------------------------

  setButtons(bits) {
    this.#exports.tb_set_buttons(bits & 0x3ff);
  }

  /** Run one video frame. Invalidates the previous frame and audio buffers. */
  runFrame() {
    this.#exports.tb_run_frame();
  }

  /** Run `count` frames, presenting only the last. Used for fast-forward. */
  runFrames(count) {
    this.#exports.tb_run_frames(count);
  }

  reset() {
    this.#exports.tb_reset();
  }

  setPaused(paused) {
    this.#exports.tb_set_paused(paused ? 1 : 0);
  }

  setColorCorrection(enabled) {
    this.#exports.tb_set_color_correction(enabled ? 1 : 0);
  }

  get running() {
    return this.#exports.tb_is_running() === 1;
  }

  get frameCount() {
    return this.#exports.tb_frame_count();
  }

  get cycleCount() {
    return this.#exports.tb_cycle_count();
  }

  // --- reading ----------------------------------------------------------

  /**
   * RGBA pixels for the last frame, as a view into module memory.
   *
   * Valid only until the next call in. Pass it straight to `ImageData` or copy
   * it; do not hold on to it.
   */
  frameView() {
    const ptr = this.#exports.tb_frame_ptr();
    if (ptr === 0) return null;
    return new Uint8ClampedArray(
      this.#memory.buffer,
      ptr,
      this.#exports.tb_frame_len(),
    );
  }

  /** Interleaved stereo samples produced by the last frame, copied out. */
  takeAudio() {
    const len = this.#exports.tb_audio_len();
    if (len === 0) return null;
    const ptr = this.#exports.tb_audio_ptr();
    // Copied because the caller keeps it until the audio clock catches up.
    return new Float32Array(this.#memory.buffer, ptr, len).slice();
  }

  get sampleRate() {
    return this.#exports.tb_audio_sample_rate();
  }

  /** The live addon snapshot: the same data the desktop app reports. */
  snapshot() {
    if (this.#exports.tb_refresh_snapshot() !== OK) return null;
    const len = this.#exports.tb_snapshot_len();
    if (len === 0) return null;
    const json = new TextDecoder().decode(
      this.#bytes(this.#exports.tb_snapshot_ptr(), len),
    );
    try {
      return JSON.parse(json);
    } catch {
      return null;
    }
  }

  // --- save states ------------------------------------------------------

  /** Serialize the current state. Returns a copy, safe to keep. */
  saveState() {
    this.#check(this.#exports.tb_save_state(), "The state could not be saved.");
    const len = this.#exports.tb_state_len();
    return this.#bytes(this.#exports.tb_state_ptr(), len).slice();
  }

  loadState(data) {
    this.#check(
      this.#withBytes(data, (ptr, len) => this.#exports.tb_load_state(ptr, len)),
      "That save state could not be loaded.",
    );
  }

  // --- battery saves ----------------------------------------------------
  //
  // A save state is a photograph of the whole machine; a battery save is what
  // the cartridge keeps, and it is what the game writes when the player picks
  // "save" from a menu. Persisting only the former loses progress the game
  // believes it has already committed.

  /** Whether the cartridge has a backup chip. */
  get hasBattery() {
    return this.#exports.tb_has_battery() !== 0;
  }

  /**
   * Whether the game wrote to the backup chip since this was last read.
   *
   * Reading clears the flag, so persist whenever it comes back true rather
   * than writing megabytes on a timer.
   */
  takeBatteryDirty() {
    return this.#exports.tb_battery_dirty() !== 0;
  }

  /** The cartridge's backup memory. Returns a copy, safe to keep. */
  batterySave() {
    this.#check(this.#exports.tb_battery_save(), "The save could not be read.");
    return this.#bytes(this.#exports.tb_battery_ptr(), this.#exports.tb_battery_len()).slice();
  }

  /**
   * Run part of a frame. Returns whether the frame finished.
   *
   * Used while a link cable is attached, so the page can return to the event
   * loop and answer the other console between slices rather than making it
   * wait for a whole frame.
   */
  runSlice(steps) {
    return this.#exports.tb_run_slice(steps) !== 0;
  }

  // --- diagnostics ------------------------------------------------------

  /** Read a halfword from the machine, as the CPU would see it. */
  debugRead(addr) {
    return this.#exports.tb_debug_read_u16(addr);
  }

  /** Write a halfword to the machine, as the CPU would. */
  debugWrite(addr, value) {
    this.#exports.tb_debug_write_u16(addr, value);
  }

  // --- the link cable ---------------------------------------------------
  //
  // The core carries no network. It says a transfer wants to happen and takes
  // the answer back; moving halfwords between consoles is this page's job.

  /**
   * Plug into a cable as `id`, one of `players` consoles.
   *
   * Console 0 is the parent and is the only one that may start a transfer.
   */
  linkConnect(id, players) {
    this.#check(
      this.#exports.tb_link_connect(id, players),
      "That is not a cable this console can be plugged into.",
    );
  }

  /** Unplug. Any transfer in flight is abandoned. */
  linkDisconnect() {
    this.#exports.tb_link_disconnect();
  }

  /** Whether a cable is attached. */
  get linkConnected() {
    return this.#exports.tb_link_connected() !== 0;
  }

  /**
   * Whether a transfer is waiting on the other consoles.
   *
   * While this is true the emulator must not be run. Emulated time may not
   * advance across a network round trip, or the game sees a transfer that took
   * longer than any cable could and abandons the link.
   */
  get linkPending() {
    return this.#exports.tb_link_pending() !== 0;
  }

  /** Whether a transfer has begun and not yet landed. */
  get linkBusy() {
    return this.#exports.tb_link_busy() !== 0;
  }

  /** Take part in a transfer the parent began. */
  linkJoin() {
    return this.#exports.tb_link_join() !== 0;
  }

  /**
   * Give up on a transfer nobody is going to answer.
   *
   * A console waiting for data does not run, which is what keeps it in step
   * with the others. Without a way out, one lost message stops the game for
   * good instead of for a moment.
   */
  linkAbandon() {
    return this.#exports.tb_link_abandon() !== 0;
  }

  /** What this console is putting on the wire. */
  get linkSendValue() {
    return this.#exports.tb_link_send_value();
  }

  /**
   * How long a transfer takes on this cable, in cycles.
   *
   * Meaningful on the parent, which drives the clock line. The parent sends
   * this along with the data so every console runs the cable at the same rate.
   */
  get linkTransferCycles() {
    return this.#exports.tb_link_transfer_cycles();
  }

  /**
   * Hand back what every console sent, parent first.
   *
   * A slot with nobody in it must be `0xFFFF`, which is what the hardware
   * reads for an absent player; zero would tell the game it was sent a zero.
   *
   * `cycles` comes from the parent. A child must never time the cable from its
   * own baud setting — Pokémon leaves a child at 9600 while the parent moves
   * to 115200, so the child would hold the wire twelve times too long, still
   * be busy when the next transfer arrived, and miss it.
   */
  linkDeliver(values, cycles) {
    return (
      this.#exports.tb_link_deliver(
        values[0] ?? 0xffff,
        values[1] ?? 0xffff,
        values[2] ?? 0xffff,
        values[3] ?? 0xffff,
        cycles,
      ) === 0
    );
  }
}

/**
 * Plays interleaved stereo samples from the emulator.
 *
 * Frames arrive in ~16ms bursts, which is too irregular to hand straight to the
 * output. Each burst is scheduled as its own buffer at a running start time so
 * playback stays continuous; when the clock falls behind (a background tab, a
 * slow frame) the queue is dropped rather than allowed to drift further and
 * further behind the picture.
 */
export class AudioSink {
  #context = null;
  #gain = null;
  #nextStart = 0;
  #sampleRate;

  constructor(sampleRate) {
    this.#sampleRate = sampleRate || 32768;
    this.volume = 0.7;
  }

  /** Must be called from a user gesture; browsers block audio otherwise. */
  async resume() {
    if (!this.#context) {
      this.#context = new (window.AudioContext || window.webkitAudioContext)();
      this.#gain = this.#context.createGain();
      this.#gain.connect(this.#context.destination);
    }
    if (this.#context.state === "suspended") await this.#context.resume();
    this.#gain.gain.value = this.volume;
    return this.#context.state === "running";
  }

  get ready() {
    return this.#context !== null && this.#context.state === "running";
  }

  /**
   * The APU's rate is only known once a ROM is loaded, and it is not the
   * 32768 Hz you might assume — the core outputs 65536 Hz. Playing at the
   * wrong rate pitches everything.
   */
  setSampleRate(rate) {
    if (rate > 0) this.#sampleRate = rate;
  }

  setVolume(value) {
    this.volume = Math.min(1, Math.max(0, value));
    if (this.#gain) this.#gain.gain.value = this.volume;
  }

  /** Queue one frame of interleaved stereo samples. */
  push(samples) {
    if (!this.ready || !samples || samples.length === 0) return;

    const frames = samples.length >> 1;
    if (frames === 0) return;

    const buffer = this.#context.createBuffer(2, frames, this.#sampleRate);
    const left = buffer.getChannelData(0);
    const right = buffer.getChannelData(1);
    for (let i = 0; i < frames; i += 1) {
      left[i] = samples[i * 2];
      right[i] = samples[i * 2 + 1];
    }

    const source = this.#context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.#gain);

    const now = this.#context.currentTime;
    // A little lead keeps small scheduling jitter from producing gaps.
    if (this.#nextStart < now + 0.02) this.#nextStart = now + 0.05;
    // If we have drifted more than a quarter second ahead the picture is behind
    // the sound; resync rather than accumulate latency.
    if (this.#nextStart > now + 0.25) this.#nextStart = now + 0.05;

    source.start(this.#nextStart);
    this.#nextStart += frames / this.#sampleRate;
  }

  close() {
    if (this.#context) this.#context.close();
    this.#context = null;
    this.#gain = null;
    this.#nextStart = 0;
  }
}

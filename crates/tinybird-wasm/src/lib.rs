//! tinyBird as a WebAssembly module, exposed through a plain C ABI.
//!
//! # Why no `wasm-bindgen`
//!
//! The interface a browser frontend actually needs is small: hand over some
//! bytes, run a frame, read a framebuffer. That is expressible with raw
//! pointers and integers, which means the whole build is
//!
//! ```text
//! rustup target add wasm32-unknown-unknown
//! cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release
//! ```
//!
//! with no `wasm-bindgen`, no `wasm-pack`, and no npm. Anyone cloning the
//! repository can build the web frontend with the Rust toolchain they already
//! have. The cost is that JavaScript reads the module's linear memory directly;
//! [`js/tinybird.js`](../../../crates/tinybird-web/src/assets/tinybird.js) is
//! the only place that has to know about it.
//!
//! # Memory model
//!
//! Buffers the host needs to read (framebuffer, audio, snapshot JSON, save
//! state) live inside the emulator struct and are exposed as pointer/length
//! pairs. They stay valid until the next call that rewrites them, which the
//! docs on each function state. JavaScript must copy out of a view before
//! calling back in, because any allocation can grow — and therefore detach —
//! the `WebAssembly.Memory` buffer.
//!
//! # Threading
//!
//! `wasm32-unknown-unknown` is single-threaded and the emulator is a process
//! global. Every entry point runs to completion before returning to JavaScript,
//! so there is no re-entrancy.

use std::ptr::addr_of_mut;

// `Bus` is needed for the register read and write below; the rest of this
// module works through `Gba` and never touches memory directly.
use tinybird_core::bus::Bus;
use tinybird_core::{Gba, GbaButton, GbaState};
use tinybird_addons::ManifestAddon;
use tinybird_games::{capture_stream_snapshot, snapshot_to_json};

/// GBA screen width in pixels.
pub const SCREEN_WIDTH: usize = 240;
/// GBA screen height in pixels.
pub const SCREEN_HEIGHT: usize = 160;
/// Bytes per pixel in the RGBA output handed to the canvas.
const BYTES_PER_PIXEL: usize = 4;

/// Number of distinct RGB555 colors the GBA can produce.
const RGB555_COLOR_COUNT: usize = 1 << 15;

/// Status codes returned by fallible entry points.
pub const TB_OK: i32 = 0;
pub const TB_ERR_NOT_INITIALISED: i32 = -1;
pub const TB_ERR_BAD_ARGUMENT: i32 = -2;
pub const TB_ERR_FAILED: i32 = -3;

struct Emulator {
    gba: Box<Gba>,
    /// RGBA8888 pixels, ready for `ImageData`.
    frame: Vec<u8>,
    /// Interleaved stereo samples in -1.0..=1.0, ready for an `AudioBuffer`.
    audio: Vec<f32>,
    /// The live addon snapshot as JSON, refreshed by [`tb_refresh_snapshot`].
    snapshot: String,
    /// Scratch for [`tb_save_state`].
    state: Vec<u8>,
    /// Scratch for [`tb_battery_save`].
    battery: Vec<u8>,
    color_correction: bool,
    lookup_plain: Vec<u32>,
    lookup_corrected: Vec<u32>,
}

impl Emulator {
    fn new() -> Self {
        Self {
            gba: Box::new(Gba::new()),
            frame: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL],
            audio: Vec::new(),
            snapshot: String::new(),
            state: Vec::new(),
            battery: Vec::new(),
            color_correction: false,
            lookup_plain: build_rgb555_lookup(false),
            lookup_corrected: build_rgb555_lookup(true),
        }
    }

    /// Convert the PPU framebuffer into the RGBA bytes a canvas expects.
    fn blit(&mut self) {
        let lookup = if self.color_correction {
            &self.lookup_corrected
        } else {
            &self.lookup_plain
        };
        let pixels = self.gba.ppu.get_framebuffer().as_slice();

        for (index, pixel) in pixels.iter().enumerate() {
            let rgb = lookup[pixel.color.to_rgb555() as usize];
            let out = index * BYTES_PER_PIXEL;
            self.frame[out] = (rgb >> 16) as u8;
            self.frame[out + 1] = (rgb >> 8) as u8;
            self.frame[out + 2] = rgb as u8;
            self.frame[out + 3] = 0xFF;
        }
    }

    /// Move one frame of audio out of the APU into the host-visible buffer.
    fn drain_audio(&mut self) {
        let samples = self.gba.apu.drain_samples();
        self.audio.clear();
        self.audio.reserve(samples.len());
        self.audio
            .extend(samples.iter().map(|sample| *sample as f32 / 32768.0));
    }
}

/// The single emulator instance.
///
/// A process global rather than a handle passed back and forth: `wasm32` has no
/// threads, and it keeps the JavaScript side from having to track a pointer.
static mut EMULATOR: Option<Emulator> = None;

/// Access the emulator, or `None` before [`tb_init`].
///
/// # Safety
///
/// Single-threaded by construction (see the module docs). The returned borrow
/// must not outlive the entry point that took it, which is why every caller
/// here is a leaf function.
#[allow(static_mut_refs)]
unsafe fn emulator() -> Option<&'static mut Emulator> {
    (*addr_of_mut!(EMULATOR)).as_mut()
}

fn build_rgb555_lookup(color_correction: bool) -> Vec<u32> {
    (0..RGB555_COLOR_COUNT)
        .map(|rgb555| {
            let r = (rgb555 as u32) & 0x1F;
            let g = ((rgb555 as u32) >> 5) & 0x1F;
            let b = ((rgb555 as u32) >> 10) & 0x1F;
            // Widen 5-bit channels to 8 by replicating the high bits.
            let r = (r << 3) | (r >> 2);
            let g = (g << 3) | (g >> 2);
            let b = (b << 3) | (b >> 2);

            if color_correction {
                // Same LCD approximation the desktop frontend uses, so a
                // screenshot from either build matches.
                let r2 = ((r * 26 + g * 4 + b * 2) / 32).min(255);
                let g2 = ((g * 24 + b * 8) / 32).min(255);
                let b2 = ((r * 6 + g * 4 + b * 22) / 32).min(255);
                (r2 << 16) | (g2 << 8) | b2
            } else {
                (r << 16) | (g << 8) | b
            }
        })
        .collect()
}

// ---------------------------------------------------------------- allocation

/// Allocate `len` bytes inside the module for the host to write into.
///
/// Pair every call with [`tb_free`] using the same length.
#[no_mangle]
pub extern "C" fn tb_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Release a buffer from [`tb_alloc`].
///
/// # Safety
///
/// `ptr` must have come from [`tb_alloc`] with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn tb_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    drop(Vec::from_raw_parts(ptr, 0, len));
}

// -------------------------------------------------------------------- set-up

/// Create the emulator. Safe to call again; it resets everything.
#[no_mangle]
pub extern "C" fn tb_init() {
    unsafe {
        EMULATOR = Some(Emulator::new());
    }
}

/// Screen dimensions, so the host does not hardcode them.
#[no_mangle]
pub extern "C" fn tb_screen_width() -> usize {
    SCREEN_WIDTH
}

#[no_mangle]
pub extern "C" fn tb_screen_height() -> usize {
    SCREEN_HEIGHT
}

/// Load a BIOS image. Optional: the core falls back to high-level emulation.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn tb_load_bios(ptr: *const u8, len: usize) -> i32 {
    let Some(emu) = emulator() else {
        return TB_ERR_NOT_INITIALISED;
    };
    if ptr.is_null() || len == 0 {
        return TB_ERR_BAD_ARGUMENT;
    }
    emu.gba
        .load_bios(std::slice::from_raw_parts(ptr, len).to_vec());
    TB_OK
}

/// Load a ROM and start running.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn tb_load_rom(ptr: *const u8, len: usize) -> i32 {
    let Some(emu) = emulator() else {
        return TB_ERR_NOT_INITIALISED;
    };
    if ptr.is_null() || len == 0 {
        return TB_ERR_BAD_ARGUMENT;
    }
    emu.gba
        .load_rom(std::slice::from_raw_parts(ptr, len).to_vec());
    emu.gba.start();
    TB_OK
}

/// Install an addon manifest, as JSON.
///
/// The browser has no filesystem, so manifests arrive the way everything else
/// does: the page fetches them and hands the bytes over. Call before the first
/// snapshot — the registry is built on first use and cannot be changed after.
///
/// Returns the number of manifests now installed, or a negative error code.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes of UTF-8.
#[no_mangle]
pub unsafe extern "C" fn tb_install_manifests(ptr: *const u8, len: usize) -> i32 {
    if ptr.is_null() || len == 0 {
        return TB_ERR_BAD_ARGUMENT;
    }

    let Ok(json) = std::str::from_utf8(std::slice::from_raw_parts(ptr, len)) else {
        return TB_ERR_BAD_ARGUMENT;
    };

    // A JSON array, so one call installs the lot: the registry can only be set
    // once, and a per-file entry point would make the second file an error.
    let Ok(manifests) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return TB_ERR_BAD_ARGUMENT;
    };

    let mut addons = Vec::with_capacity(manifests.len());
    for manifest in &manifests {
        let Ok(addon) = ManifestAddon::parse(&manifest.to_string()) else {
            // One bad manifest costs that manifest, not the page.
            continue;
        };
        addons.push(addon);
    }

    match tinybird_games::install_manifests(addons) {
        Ok(count) => count as i32,
        Err(_) => TB_ERR_BAD_ARGUMENT,
    }
}

/// Restore battery-backed cartridge save data.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn tb_load_save(ptr: *const u8, len: usize) -> i32 {
    let Some(emu) = emulator() else {
        return TB_ERR_NOT_INITIALISED;
    };
    if ptr.is_null() || len == 0 {
        return TB_ERR_BAD_ARGUMENT;
    }
    emu.gba.load_save_data(std::slice::from_raw_parts(ptr, len));
    TB_OK
}

#[no_mangle]
pub extern "C" fn tb_reset() -> i32 {
    unsafe {
        match emulator() {
            Some(emu) => {
                emu.gba.reset();
                TB_OK
            }
            None => TB_ERR_NOT_INITIALISED,
        }
    }
}

/// 1 while a ROM is loaded and running.
#[no_mangle]
pub extern "C" fn tb_is_running() -> i32 {
    unsafe {
        match emulator() {
            Some(emu) => i32::from(emu.gba.state == GbaState::Running),
            None => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_set_paused(paused: i32) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        if paused != 0 {
            emu.gba.pause();
        } else {
            emu.gba.start();
        }
        TB_OK
    }
}

/// Toggle the LCD color-correction lookup. Takes effect on the next frame.
#[no_mangle]
pub extern "C" fn tb_set_color_correction(enabled: i32) -> i32 {
    unsafe {
        match emulator() {
            Some(emu) => {
                emu.color_correction = enabled != 0;
                TB_OK
            }
            None => TB_ERR_NOT_INITIALISED,
        }
    }
}

// ------------------------------------------------------------------- running

/// Button bits, matching `GbaButton` so the host can send one integer.
const BUTTON_BITS: [(u16, GbaButton); 10] = [
    (1 << 0, GbaButton::A),
    (1 << 1, GbaButton::B),
    (1 << 2, GbaButton::SELECT),
    (1 << 3, GbaButton::START),
    (1 << 4, GbaButton::RIGHT),
    (1 << 5, GbaButton::LEFT),
    (1 << 6, GbaButton::UP),
    (1 << 7, GbaButton::DOWN),
    (1 << 8, GbaButton::R),
    (1 << 9, GbaButton::L),
];

/// Set the full button state as a bitmask. See [`BUTTON_BITS`] for the layout.
#[no_mangle]
pub extern "C" fn tb_set_buttons(bits: u16) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        let mut buttons = GbaButton::empty();
        for (mask, button) in BUTTON_BITS {
            if bits & mask != 0 {
                buttons |= button;
            }
        }
        emu.gba.input.set_buttons(buttons);
        TB_OK
    }
}

/// Run one video frame, then refresh the framebuffer and audio buffers.
///
/// Invalidates anything previously returned by [`tb_frame_ptr`] and
/// [`tb_audio_ptr`].
#[no_mangle]
pub extern "C" fn tb_run_frame() -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        if emu.gba.state != GbaState::Running {
            // Still refresh the frame so a paused screen keeps painting.
            emu.blit();
            emu.audio.clear();
            return TB_OK;
        }
        emu.gba.run_frame();
        emu.blit();
        emu.drain_audio();
        TB_OK
    }
}

/// Run part of a frame, stopping after at most `max_steps` instructions.
///
/// Returns 1 if the frame finished, 0 if there is more of it to run.
///
/// This exists so a linked console can answer the other one promptly.
/// JavaScript is single-threaded, so a message that arrives while a whole
/// frame is being run waits for the end of it — about fourteen milliseconds on
/// this emulator. A game trading Pokémon asks for a transfer every couple of
/// milliseconds, so the reply was always late. Running a frame in slices and
/// yielding between them lets the socket be serviced in the gaps.
///
/// Presenting and audio happen only when the frame actually finishes; a
/// half-drawn frame is not worth showing.
#[no_mangle]
pub extern "C" fn tb_run_slice(max_steps: u32) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return 0;
        };
        if emu.gba.state != GbaState::Running {
            emu.blit();
            emu.audio.clear();
            return 1;
        }
        match emu.gba.run_frame_with_budget(u64::from(max_steps.max(1))) {
            Some(_) => {
                emu.blit();
                emu.drain_audio();
                1
            }
            None => 0,
        }
    }
}

/// Run `frames` frames, presenting only the last. Used for fast-forward.
#[no_mangle]
pub extern "C" fn tb_run_frames(frames: u32) -> i32 {
    let frames = frames.max(1);
    for _ in 0..frames.saturating_sub(1) {
        unsafe {
            let Some(emu) = emulator() else {
                return TB_ERR_NOT_INITIALISED;
            };
            if emu.gba.state != GbaState::Running {
                break;
            }
            emu.gba.run_frame();
            // Audio still has to be drained or the APU buffer would fill.
            emu.gba.apu.drain_samples();
        }
    }
    tb_run_frame()
}

// ------------------------------------------------------------------- reading

/// Pointer to RGBA8888 pixels, `tb_frame_len()` bytes, top row first.
#[no_mangle]
pub extern "C" fn tb_frame_ptr() -> *const u8 {
    unsafe {
        match emulator() {
            Some(emu) => emu.frame.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_frame_len() -> usize {
    SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL
}

/// Pointer to interleaved stereo `f32` samples produced by the last frame.
#[no_mangle]
pub extern "C" fn tb_audio_ptr() -> *const f32 {
    unsafe {
        match emulator() {
            Some(emu) => emu.audio.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

/// Number of `f32` samples available, counting both channels.
#[no_mangle]
pub extern "C" fn tb_audio_len() -> usize {
    unsafe { emulator().map_or(0, |emu| emu.audio.len()) }
}

#[no_mangle]
pub extern "C" fn tb_audio_sample_rate() -> u32 {
    unsafe { emulator().map_or(0, |emu| emu.gba.apu.output_sample_rate()) }
}

/// Frames emulated since reset, for a frame-accurate host clock.
///
/// Returned as `f64` because that is the only numeric type the raw
/// WebAssembly ABI and JavaScript agree on for values past 2^32.
#[no_mangle]
pub extern "C" fn tb_frame_count() -> f64 {
    unsafe { emulator().map_or(0.0, |emu| emu.gba.frame_count as f64) }
}

/// CPU cycles emulated since reset.
///
/// Like [`tb_frame_count`], this is an `f64` so JavaScript receives the full
/// counter rather than a wrapping 32-bit value. It remains exact for years of
/// continuous emulation.
#[no_mangle]
pub extern "C" fn tb_cycle_count() -> f64 {
    unsafe { emulator().map_or(0.0, |emu| emu.gba.total_cycles as f64) }
}

// ------------------------------------------------------------------- addons

/// Re-read the live addon snapshot. Invalidates [`tb_snapshot_ptr`].
///
/// This is the same registry the desktop app uses, so the browser reports
/// identical game data.
#[no_mangle]
pub extern "C" fn tb_refresh_snapshot() -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        let snapshot = capture_stream_snapshot(Some(emu.gba.as_ref()));
        match snapshot_to_json(&snapshot) {
            Ok(json) => {
                emu.snapshot = json;
                TB_OK
            }
            Err(_) => TB_ERR_FAILED,
        }
    }
}

/// Pointer to the UTF-8 snapshot JSON from the last [`tb_refresh_snapshot`].
#[no_mangle]
pub extern "C" fn tb_snapshot_ptr() -> *const u8 {
    unsafe {
        match emulator() {
            Some(emu) => emu.snapshot.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_snapshot_len() -> usize {
    unsafe { emulator().map_or(0, |emu| emu.snapshot.len()) }
}

// -------------------------------------------------------------- save states

/// Serialize a save state. Invalidates [`tb_state_ptr`].
// ------------------------------------------------------------ battery saves
//
// A save state is a photograph of the whole machine; a battery save is what the
// cartridge itself keeps, and it is what the game writes when the player picks
// "save" in a menu. They are not interchangeable, and a browser that persists
// only the former loses progress the game believes it has already written.

/// Whether the cartridge has a backup chip at all.
#[no_mangle]
pub extern "C" fn tb_has_battery() -> i32 {
    unsafe {
        match emulator() {
            Some(emu) => i32::from(emu.gba.has_persistent_save()),
            None => 0,
        }
    }
}

/// Whether the game has written to the backup chip since this was last asked.
///
/// Consuming the flag is what lets the host persist only when something
/// actually changed, rather than writing several megabytes every frame.
#[no_mangle]
pub extern "C" fn tb_battery_dirty() -> i32 {
    unsafe {
        match emulator() {
            Some(emu) => i32::from(emu.gba.take_save_dirty()),
            None => 0,
        }
    }
}

/// Copy the cartridge's backup memory into a buffer the host can read.
#[no_mangle]
pub extern "C" fn tb_battery_save() -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        emu.battery = emu.gba.save_data();
        TB_OK
    }
}

#[no_mangle]
pub extern "C" fn tb_battery_ptr() -> *const u8 {
    unsafe {
        match emulator() {
            Some(emu) => emu.battery.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_battery_len() -> usize {
    unsafe {
        match emulator() {
            Some(emu) => emu.battery.len(),
            None => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_save_state() -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        match emu.gba.save_state_bytes() {
            Ok(bytes) => {
                emu.state = bytes;
                TB_OK
            }
            Err(_) => TB_ERR_FAILED,
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_state_ptr() -> *const u8 {
    unsafe {
        match emulator() {
            Some(emu) => emu.state.as_ptr(),
            None => std::ptr::null(),
        }
    }
}

#[no_mangle]
pub extern "C" fn tb_state_len() -> usize {
    unsafe { emulator().map_or(0, |emu| emu.state.len()) }
}

/// Restore a save state previously produced by [`tb_save_state`].
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn tb_load_state(ptr: *const u8, len: usize) -> i32 {
    let Some(emu) = emulator() else {
        return TB_ERR_NOT_INITIALISED;
    };
    if ptr.is_null() || len == 0 {
        return TB_ERR_BAD_ARGUMENT;
    }
    match emu
        .gba
        .load_state_bytes(std::slice::from_raw_parts(ptr, len))
    {
        Ok(()) => {
            emu.gba.start();
            TB_OK
        }
        Err(_) => TB_ERR_FAILED,
    }
}

// --- Diagnostics -----------------------------------------------------------
//
// Reading and writing hardware registers from outside the machine. The page
// does not use these and should not: it drives the console through the entry
// points above, which is what a player does. They exist so the ABI itself can
// be tested — a link transfer starts with a register write, and without a way
// to make one there is no way to exercise this surface at all — and because a
// register read-out is the first thing wanted when a game misbehaves.

/// Read a halfword from the machine, as the CPU would see it.
#[no_mangle]
pub extern "C" fn tb_debug_read_u16(addr: u32) -> u32 {
    unsafe { emulator().map_or(0, |emu| u32::from(emu.gba.bus.read_u16(addr))) }
}

/// Write a halfword to the machine, as the CPU would.
///
/// This goes through the ordinary bus path, so registers that are read-only,
/// write-to-clear, or watched by a subsystem behave exactly as they do for a
/// game. It is a way in, not a way around.
#[no_mangle]
pub extern "C" fn tb_debug_write_u16(addr: u32, value: u32) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        emu.gba.bus.write_u16(addr, value as u16);
        TB_OK
    }
}

// --- The link cable --------------------------------------------------------
//
// The core does not know what a network is. It says a transfer wants to happen
// and accepts the answer; these entry points are how the page carries halfwords
// between consoles that are not in the same process.
//
// The sequence, from the page's point of view:
//
//   parent: tb_link_pending() goes non-zero
//           -> tell the room a transfer is starting
//   child:  tb_link_join(), then send tb_link_send_value()
//   parent: collect all four, tb_link_deliver(...), broadcast the same four
//   child:  tb_link_deliver(...) with what the parent sent

/// Plug this console into a cable as `id`, one of `players` consoles.
///
/// Console 0 is the parent: the only one that may start a transfer. Calling
/// this again with different numbers re-seats the cable, which is what happens
/// when somebody joins or leaves a room.
#[no_mangle]
pub extern "C" fn tb_link_connect(id: u32, players: u32) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        if players < 2 || players > 4 || id >= players {
            return TB_ERR_BAD_ARGUMENT;
        }
        emu.gba.link_connect(id as u8, players as u8);
        TB_OK
    }
}

/// Unplug the cable.
#[no_mangle]
pub extern "C" fn tb_link_disconnect() -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        emu.gba.link_disconnect();
        TB_OK
    }
}

/// Whether a cable is attached.
#[no_mangle]
pub extern "C" fn tb_link_connected() -> i32 {
    unsafe { emulator().map_or(0, |emu| i32::from(emu.gba.sio.connected())) }
}

/// Whether a transfer is waiting on the other consoles.
///
/// This is the signal the page acts on, and it is also the signal to stop
/// running frames: emulated time must not advance while the cable is waiting
/// on a network, or the game sees a transfer that took far longer than any
/// cable could and gives up on it.
#[no_mangle]
pub extern "C" fn tb_link_pending() -> i32 {
    unsafe { emulator().map_or(0, |emu| i32::from(emu.gba.link_transfer_pending())) }
}

/// Whether a transfer has begun and not yet landed.
#[no_mangle]
pub extern "C" fn tb_link_busy() -> i32 {
    unsafe { emulator().map_or(0, |emu| i32::from(emu.gba.sio.busy())) }
}

/// Take part in a transfer the parent has begun. Returns 1 if this console
/// joined, 0 if it had nothing to join.
#[no_mangle]
pub extern "C" fn tb_link_join() -> i32 {
    unsafe { emulator().map_or(0, |emu| i32::from(emu.gba.link_join())) }
}

/// Give up on a transfer nobody is going to answer, unfreezing the console.
///
/// Returns 1 if there was one to give up on.
#[no_mangle]
pub extern "C" fn tb_link_abandon() -> i32 {
    unsafe { emulator().map_or(0, |emu| i32::from(emu.gba.link_abandon())) }
}

/// What this console is putting on the wire.
#[no_mangle]
pub extern "C" fn tb_link_send_value() -> u32 {
    unsafe { emulator().map_or(0, |emu| u32::from(emu.gba.link_send_value())) }
}

/// How long a transfer takes on this console's cable, in cycles.
///
/// Read on the parent and sent to everyone with the data: the parent drives
/// the clock line, so its baud rate is the cable's.
#[no_mangle]
pub extern "C" fn tb_link_transfer_cycles() -> u32 {
    unsafe { emulator().map_or(0, |emu| emu.gba.link_transfer_cycles()) }
}

/// Hand back what every console sent, parent first.
///
/// A slot for a console that is not on the cable should be `0xFFFF`, which is
/// what the hardware reads for an absent player. Passing zero would tell the
/// game somebody sent it a zero.
///
/// `cycles` is the parent's transfer time, from [`tb_link_transfer_cycles`].
/// A child must not use its own: Pokémon runs the parent at 115200 while a
/// child is still set to 9600, and a child timing itself would hold the cable
/// twelve times too long and miss the next transfer entirely.
#[no_mangle]
pub extern "C" fn tb_link_deliver(v0: u32, v1: u32, v2: u32, v3: u32, cycles: u32) -> i32 {
    unsafe {
        let Some(emu) = emulator() else {
            return TB_ERR_NOT_INITIALISED;
        };
        emu.gba
            .link_deliver([v0 as u16, v1 as u16, v2 as u16, v3 as u16], cycles);
        TB_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// The emulator is a process global because `wasm32` is single-threaded.
    /// The host test harness is not, so tests take this lock to keep that
    /// invariant true here too.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn exclusive() -> MutexGuard<'static, ()> {
        // A panicking test poisons the lock; the state is rebuilt by `tb_init`
        // in each test anyway, so recovering is correct rather than masking.
        TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// A minimal ROM: the header fields the identity reader looks at.
    fn fake_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x200];
        rom[0xA0..0xAC].copy_from_slice(b"TESTGAME\0\0\0\0");
        rom[0xAC..0xB0].copy_from_slice(b"ZZZE");
        rom[0xB0..0xB2].copy_from_slice(b"01");
        rom
    }

    #[test]
    fn entry_points_report_an_error_before_init() {
        let _guard = exclusive();
        // The host may call in any order; nothing may dereference a null global.
        unsafe {
            EMULATOR = None;
        }
        assert_eq!(tb_reset(), TB_ERR_NOT_INITIALISED);
        assert_eq!(tb_run_frame(), TB_ERR_NOT_INITIALISED);
        assert_eq!(tb_set_buttons(0xFFFF), TB_ERR_NOT_INITIALISED);
        assert_eq!(tb_refresh_snapshot(), TB_ERR_NOT_INITIALISED);
        assert_eq!(tb_frame_ptr(), std::ptr::null());
        assert_eq!(tb_audio_len(), 0);
        assert_eq!(tb_is_running(), 0);
    }

    #[test]
    fn alloc_and_free_round_trip() {
        let _guard = exclusive();
        let ptr = tb_alloc(64);
        assert!(!ptr.is_null());
        unsafe {
            std::ptr::write_bytes(ptr, 0xAB, 64);
            assert_eq!(*ptr, 0xAB);
            tb_free(ptr, 64);
        }
    }

    #[test]
    fn alloc_of_zero_is_null_and_free_of_null_is_a_no_op() {
        let _guard = exclusive();
        assert_eq!(tb_alloc(0), std::ptr::null_mut());
        unsafe {
            tb_free(std::ptr::null_mut(), 0);
            tb_free(std::ptr::null_mut(), 16);
        }
    }

    #[test]
    fn loading_a_rom_starts_the_emulator() {
        let _guard = exclusive();
        tb_init();
        let rom = fake_rom();
        assert_eq!(unsafe { tb_load_rom(rom.as_ptr(), rom.len()) }, TB_OK);
        assert_eq!(tb_is_running(), 1);
    }

    #[test]
    fn null_or_empty_buffers_are_rejected() {
        let _guard = exclusive();
        tb_init();
        unsafe {
            assert_eq!(tb_load_rom(std::ptr::null(), 16), TB_ERR_BAD_ARGUMENT);
            assert_eq!(tb_load_rom([0u8; 4].as_ptr(), 0), TB_ERR_BAD_ARGUMENT);
            assert_eq!(tb_load_bios(std::ptr::null(), 16), TB_ERR_BAD_ARGUMENT);
            assert_eq!(tb_load_state(std::ptr::null(), 16), TB_ERR_BAD_ARGUMENT);
        }
    }

    #[test]
    fn a_frame_fills_the_rgba_buffer_with_opaque_pixels() {
        let _guard = exclusive();
        tb_init();
        let rom = fake_rom();
        unsafe { tb_load_rom(rom.as_ptr(), rom.len()) };
        assert_eq!(tb_run_frame(), TB_OK);

        assert_eq!(tb_frame_len(), SCREEN_WIDTH * SCREEN_HEIGHT * 4);
        let frame = unsafe { std::slice::from_raw_parts(tb_frame_ptr(), tb_frame_len()) };
        // Every fourth byte is alpha; a canvas needs them all opaque or the
        // picture silently renders blank.
        assert!(frame.chunks_exact(4).all(|px| px[3] == 0xFF));
    }

    #[test]
    fn pausing_stops_the_frame_counter_but_still_paints() {
        let _guard = exclusive();
        tb_init();
        let rom = fake_rom();
        unsafe { tb_load_rom(rom.as_ptr(), rom.len()) };
        tb_run_frame();
        let running_count = tb_frame_count();

        assert_eq!(tb_set_paused(1), TB_OK);
        assert_eq!(tb_run_frame(), TB_OK);
        assert_eq!(tb_frame_count(), running_count, "paused must not advance");
        assert_eq!(tb_is_running(), 0);

        tb_set_paused(0);
        tb_run_frame();
        assert!(tb_frame_count() > running_count);
    }

    #[test]
    fn button_bits_map_onto_every_gba_button() {
        let _guard = exclusive();
        // A gap here would silently drop an input in the browser.
        let mut seen = GbaButton::empty();
        for (_, button) in BUTTON_BITS {
            seen |= button;
        }
        assert_eq!(seen, GbaButton::all());
    }

    #[test]
    fn button_bits_are_unique() {
        let _guard = exclusive();
        let mut mask = 0u16;
        for (bit, _) in BUTTON_BITS {
            assert_eq!(mask & bit, 0, "bit {bit:#x} is used twice");
            mask |= bit;
        }
    }

    #[test]
    fn the_snapshot_is_valid_json_describing_the_cartridge() {
        let _guard = exclusive();
        tb_init();
        let rom = fake_rom();
        unsafe { tb_load_rom(rom.as_ptr(), rom.len()) };
        assert_eq!(tb_refresh_snapshot(), TB_OK);

        let json = unsafe {
            std::str::from_utf8(std::slice::from_raw_parts(
                tb_snapshot_ptr(),
                tb_snapshot_len(),
            ))
            .expect("snapshot must be UTF-8")
        };
        assert!(json.contains("\"schema_version\""));
        // The cartridge fallback claims every ROM, so an unknown game still
        // reports something in the browser.
        assert!(json.contains("\"cartridge\""), "got: {json}");
    }

    #[test]
    fn a_save_state_round_trips() {
        let _guard = exclusive();
        tb_init();
        let rom = fake_rom();
        unsafe { tb_load_rom(rom.as_ptr(), rom.len()) };
        tb_run_frame();

        assert_eq!(tb_save_state(), TB_OK);
        let saved = unsafe { std::slice::from_raw_parts(tb_state_ptr(), tb_state_len()) }.to_vec();
        assert!(!saved.is_empty());

        tb_run_frame();
        let advanced = tb_frame_count();

        assert_eq!(unsafe { tb_load_state(saved.as_ptr(), saved.len()) }, TB_OK);
        assert!(
            tb_frame_count() < advanced,
            "loading a state must rewind the frame counter"
        );
    }

    #[test]
    fn loading_a_corrupt_state_fails_without_panicking() {
        let _guard = exclusive();
        tb_init();
        let garbage = [0xFFu8; 64];
        assert_eq!(
            unsafe { tb_load_state(garbage.as_ptr(), garbage.len()) },
            TB_ERR_FAILED
        );
    }

    #[test]
    fn color_correction_changes_the_lookup_not_the_buffer_size() {
        let _guard = exclusive();
        tb_init();
        assert_eq!(tb_set_color_correction(1), TB_OK);
        let emu = unsafe { emulator() }.expect("initialised");
        assert!(emu.color_correction);
        assert_eq!(emu.lookup_corrected.len(), RGB555_COLOR_COUNT);
        // The correction matrix rows sum to 32, so white stays white; a
        // saturated primary is where it actually does work.
        const WHITE: usize = 0x7FFF;
        const PURE_RED: usize = 0x001F;
        assert_eq!(emu.lookup_plain[WHITE], emu.lookup_corrected[WHITE]);
        assert_ne!(
            emu.lookup_plain[PURE_RED], emu.lookup_corrected[PURE_RED],
            "correction must desaturate a primary"
        );
    }

    #[test]
    fn the_screen_dimensions_match_the_hardware() {
        let _guard = exclusive();
        assert_eq!(tb_screen_width(), 240);
        assert_eq!(tb_screen_height(), 160);
    }
}

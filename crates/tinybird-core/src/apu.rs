//! Audio Processing Unit (APU) for the GBA
//!
//! The GBA has 6 sound channels:
//! - Channels 1-2: Square wave (legacy Game Boy)
//! - Channel 3: Wave (legacy Game Boy)
//! - Channel 4: Noise (legacy Game Boy)
//! - Channels A-B: DMA/FIFO (GBA-specific)

use serde::{Deserialize, Serialize};

/// GBA CPU frequency (16.78 MHz)
const CPU_FREQ: u32 = 16_777_216;

/// Audio sample rate (default/native GBA PWM rate)
pub const SAMPLE_RATE: u32 = 32768;

/// Audio buffer size
pub const BUFFER_SIZE: usize = 1024;

/// Hard cap on buffered output samples (interleaved stereo).
///
/// One second at [`SAMPLE_RATE`]. A frontend drains every frame, so reaching
/// this means nothing is consuming audio — a headless run, or a frontend with
/// no audio device. Without a cap the buffer grows for as long as emulation
/// runs, and because it is part of the savestate, a long headless session
/// produced savestates of a hundred megabytes and could later hand the audio
/// backend a batch large enough to overflow its own bookkeeping.
///
/// Audio older than a second is useless to play, so the oldest is dropped.
pub const MAX_BUFFERED_SAMPLES: usize = SAMPLE_RATE as usize * 2;

/// How much to discard at once when the cap is hit, so a host that never
/// drains costs one memmove per quarter-second rather than one per sample.
const SAMPLE_DISCARD_CHUNK: usize = MAX_BUFFERED_SAMPLES / 4;

/// Frame sequencer frequency (512 Hz)
const FRAME_SEQ_FREQ: u32 = 512;

/// CPU cycles per frame sequencer step
const CYCLES_PER_FRAME_SEQ: u32 = CPU_FREQ / FRAME_SEQ_FREQ;

/// Duty cycle waveforms: each has 8 steps
/// Index by duty_cycle (0-3), then by position (0-7)
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

// ---------------------------------------------------------------------------
// Register addresses
// ---------------------------------------------------------------------------

/// SOUND1CNT_L - Channel 1 sweep
const SOUND1CNT_L: u32 = 0x0400_0060;
/// SOUND1CNT_H - Channel 1 duty/length/envelope
const SOUND1CNT_H: u32 = 0x0400_0062;
/// SOUND1CNT_X - Channel 1 frequency/control
const SOUND1CNT_X: u32 = 0x0400_0064;
/// SOUND2CNT_L - Channel 2 duty/length/envelope
const SOUND2CNT_L: u32 = 0x0400_0068;
/// SOUND2CNT_H - Channel 2 frequency/control
const SOUND2CNT_H: u32 = 0x0400_006C;
/// SOUND3CNT_L - Channel 3 stop/wave RAM select
const SOUND3CNT_L: u32 = 0x0400_0070;
/// SOUND3CNT_H - Channel 3 length/volume
const SOUND3CNT_H: u32 = 0x0400_0072;
/// SOUND3CNT_X - Channel 3 frequency/control
const SOUND3CNT_X: u32 = 0x0400_0074;
/// SOUND4CNT_L - Channel 4 length/envelope
const SOUND4CNT_L: u32 = 0x0400_0078;
/// SOUND4CNT_H - Channel 4 frequency/control
const SOUND4CNT_H: u32 = 0x0400_007C;
/// SOUNDCNT_L - PSG master volume/enable
const SOUNDCNT_L: u32 = 0x0400_0080;
/// SOUNDCNT_H - DMA sound control
const SOUNDCNT_H: u32 = 0x0400_0082;
/// SOUNDCNT_X - Master enable + channel status
const SOUNDCNT_X: u32 = 0x0400_0084;
/// SOUNDBIAS
const SOUNDBIAS: u32 = 0x0400_0088;
/// Wave RAM start
const WAVE_RAM_START: u32 = 0x0400_0090;
/// Wave RAM end (inclusive)
const WAVE_RAM_END: u32 = 0x0400_009F;
/// FIFO_A
const FIFO_A: u32 = 0x0400_00A0;
/// FIFO_B
const FIFO_B: u32 = 0x0400_00A4;

// ---------------------------------------------------------------------------
// Square wave channel (channels 1 & 2)
// ---------------------------------------------------------------------------

/// Square wave channel (channels 1 & 2)
#[derive(Clone, Serialize, Deserialize)]
pub struct SquareChannel {
    /// Whether the channel is currently active
    pub enabled: bool,
    /// Length counter (6-bit, 0-63)
    pub length_counter: u8,
    /// Duty cycle selector: 0-3 (12.5%, 25%, 50%, 75%)
    pub duty_cycle: u8,
    /// Envelope initial volume (0-15)
    pub envelope_volume: u8,
    /// Envelope direction: true = increase, false = decrease
    pub envelope_dir: bool,
    /// Envelope period (0-7; 0 disables)
    pub envelope_period: u8,
    /// Frequency value (11-bit)
    pub frequency: u16,
    /// Whether the length counter is enabled
    pub length_enable: bool,
    /// Sweep period (channel 1 only; 0-7)
    pub sweep_period: u8,
    /// Sweep direction: false = increase, true = decrease
    pub sweep_dir: bool,
    /// Sweep shift (0-7)
    pub sweep_shift: u8,

    // Internal state
    timer: u32,
    duty_pos: u8,
    current_volume: u8,
    envelope_timer: u8,
    sweep_timer: u8,
    shadow_freq: u16,
}

impl SquareChannel {
    /// Create a new square channel with default (zeroed) state.
    pub fn new() -> Self {
        Self {
            enabled: false,
            length_counter: 0,
            duty_cycle: 0,
            envelope_volume: 0,
            envelope_dir: false,
            envelope_period: 0,
            frequency: 0,
            length_enable: false,
            sweep_period: 0,
            sweep_dir: false,
            sweep_shift: 0,
            timer: 0,
            duty_pos: 0,
            current_volume: 0,
            envelope_timer: 0,
            sweep_timer: 0,
            shadow_freq: 0,
        }
    }

    /// Reset channel to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Advance the channel timer by `cycles` CPU cycles.
    pub fn tick(&mut self, cycles: u32) {
        if !self.enabled {
            return;
        }
        let period = (2048 - self.frequency as u32) * 16;
        if period == 0 {
            return;
        }
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            self.duty_pos = (self.duty_pos + 1) & 7;
        }
    }

    /// Get the current output sample (signed 16-bit range).
    pub fn sample(&self) -> i16 {
        if !self.enabled {
            return 0;
        }
        let duty = self.duty_cycle.min(3) as usize;
        let high = DUTY_TABLE[duty][self.duty_pos as usize] != 0;
        // GBA square wave outputs 0..volume (unsigned 4-bit); centered representation is
        // +(volume/2) for high and -((volume+1)/2) for low, matching the wave channel's range.
        let v = self.current_volume as i16;
        if high {
            v / 2
        } else {
            -((v + 1) / 2)
        }
    }

    /// Trigger (restart) the channel.
    pub fn trigger(&mut self) {
        self.enabled = true;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer = 0;
        self.current_volume = self.envelope_volume;
        self.envelope_timer = self.envelope_period;
        self.shadow_freq = self.frequency;
        self.sweep_timer = if self.sweep_period > 0 {
            self.sweep_period
        } else {
            8
        };
        // Disable channel if DAC is off (volume 0, decreasing envelope)
        if self.envelope_volume == 0 && !self.envelope_dir {
            self.enabled = false;
        }
    }

    /// Clock the length counter (called at 256 Hz — frame sequencer steps 0,2,4,6).
    pub fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    /// Clock the envelope (called at 64 Hz — frame sequencer step 7).
    pub fn clock_envelope(&mut self) {
        if self.envelope_period == 0 {
            return;
        }
        if self.envelope_timer > 0 {
            self.envelope_timer -= 1;
        }
        if self.envelope_timer == 0 {
            self.envelope_timer = self.envelope_period;
            if self.envelope_dir && self.current_volume < 15 {
                self.current_volume += 1;
            } else if !self.envelope_dir && self.current_volume > 0 {
                self.current_volume -= 1;
            }
        }
    }

    /// Clock the frequency sweep (channel 1 only; called at 128 Hz — steps 2,6).
    pub fn clock_sweep(&mut self) {
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer == 0 {
            self.sweep_timer = if self.sweep_period > 0 {
                self.sweep_period
            } else {
                8
            };
            if self.sweep_period > 0 {
                let new_freq = self.calculate_sweep();
                if new_freq <= 2047 && self.sweep_shift > 0 {
                    self.shadow_freq = new_freq;
                    self.frequency = new_freq;
                    // Overflow check again
                    if self.calculate_sweep() > 2047 {
                        self.enabled = false;
                    }
                }
                if new_freq > 2047 {
                    self.enabled = false;
                }
            }
        }
    }

    fn calculate_sweep(&self) -> u16 {
        let delta = self.shadow_freq >> self.sweep_shift;
        if self.sweep_dir {
            self.shadow_freq.wrapping_sub(delta)
        } else {
            self.shadow_freq.wrapping_add(delta)
        }
    }
}

impl Default for SquareChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Wave channel (channel 3)
// ---------------------------------------------------------------------------

/// Wave channel (channel 3)
#[derive(Clone, Serialize, Deserialize)]
pub struct WaveChannel {
    /// Whether the channel is currently active
    pub enabled: bool,
    /// DAC power
    pub dac_enabled: bool,
    /// Length counter (0-255)
    pub length_counter: u16,
    /// Volume shift: 0=mute, 1=100%, 2=50%, 3=25%
    pub volume: u8,
    /// Frequency value (11-bit)
    pub frequency: u16,
    /// Whether the length counter is enabled
    pub length_enable: bool,
    /// Wave RAM: two 16-byte banks holding 64 4-bit samples total
    pub wave_ram: [u8; 32],
    /// Wave RAM dimension: false = 1 bank / 32 samples, true = 2 banks / 64 samples
    pub two_banks: bool,
    /// Software-selected playback bank from SOUND3CNT_L bit 6.
    pub bank_select: u8,

    // Internal
    timer: u32,
    position: u8,
    sample_buffer: u8,
    playback_bank: u8,
}

impl WaveChannel {
    /// Create a new wave channel.
    pub fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            length_counter: 0,
            volume: 0,
            frequency: 0,
            length_enable: false,
            wave_ram: [0; 32],
            two_banks: false,
            bank_select: 0,
            timer: 0,
            position: 0,
            sample_buffer: 0,
            playback_bank: 0,
        }
    }

    /// Reset channel to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Advance the channel timer.
    pub fn tick(&mut self, cycles: u32) {
        if !self.enabled {
            return;
        }
        let period = (2048 - self.frequency as u32) * 8;
        if period == 0 {
            return;
        }
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            let sample_count = if self.two_banks { 64 } else { 32 };
            self.position = (self.position + 1) % sample_count;
            self.sample_buffer = self.read_wave_sample(self.position);
        }
    }

    /// Get the current output sample.
    pub fn sample(&self) -> i16 {
        if !self.enabled || !self.dac_enabled {
            return 0;
        }
        // sample_buffer is 0-15 (4-bit unsigned); center to signed then apply volume.
        let centered = self.sample_buffer as i16 - 8; // -8..+7
        match self.volume {
            0 => 0,
            1 => centered,           // 100%
            2 => centered / 2,       // 50%
            3 => centered / 4,       // 25%
            4 => (centered * 3) / 4, // GBA force-volume bit: 75%
            _ => 0,
        }
    }

    /// Trigger (restart) the channel.
    pub fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 256;
        }
        self.timer = 0;
        self.position = 0;
        self.playback_bank = self.bank_select & 1;
        self.sample_buffer = self.read_wave_sample(0);
    }

    /// Clock the length counter.
    pub fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    fn playback_bank_for_position(&self, sample_index: u8) -> u8 {
        if self.two_banks && sample_index >= 32 {
            self.playback_bank ^ 1
        } else {
            self.playback_bank
        }
    }

    fn read_wave_sample(&self, sample_index: u8) -> u8 {
        let bank = self.playback_bank_for_position(sample_index) as usize;
        let nibble = (sample_index as usize) & 31;
        let byte_index = bank * 16 + nibble / 2;
        let byte = self.wave_ram[byte_index];
        if (nibble & 1) == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        }
    }

    fn accessible_bank(&self) -> usize {
        if self.enabled || self.dac_enabled {
            (self.bank_select ^ 1) as usize
        } else {
            1
        }
    }

    fn read_wave_ram_byte(&self, offset: usize) -> u8 {
        self.wave_ram[self.accessible_bank() * 16 + offset]
    }

    fn write_wave_ram_byte(&mut self, offset: usize, value: u8) {
        let index = self.accessible_bank() * 16 + offset;
        self.wave_ram[index] = value;
        if !self.enabled {
            self.sample_buffer = self.read_wave_sample(self.position);
        }
    }
}

impl Default for WaveChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Noise channel (channel 4)
// ---------------------------------------------------------------------------

/// Noise channel (channel 4)
#[derive(Clone, Serialize, Deserialize)]
pub struct NoiseChannel {
    /// Whether the channel is currently active
    pub enabled: bool,
    /// Length counter (6-bit, 0-63)
    pub length_counter: u8,
    /// Envelope initial volume (0-15)
    pub envelope_volume: u8,
    /// Envelope direction: true = increase
    pub envelope_dir: bool,
    /// Envelope period (0-7)
    pub envelope_period: u8,
    /// Clock shift (0-15)
    pub clock_shift: u8,
    /// LFSR width mode: false = 15-bit, true = 7-bit
    pub width_mode: bool,
    /// Divisor code (0-7)
    pub divisor_code: u8,
    /// Whether the length counter is enabled
    pub length_enable: bool,

    // Internal
    timer: u32,
    current_volume: u8,
    envelope_timer: u8,
    lfsr: u16,
}

/// Divisor values for noise channel, indexed by divisor_code
const DIVISOR_TABLE: [u32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

impl NoiseChannel {
    /// Create a new noise channel.
    pub fn new() -> Self {
        Self {
            enabled: false,
            length_counter: 0,
            envelope_volume: 0,
            envelope_dir: false,
            envelope_period: 0,
            clock_shift: 0,
            width_mode: false,
            divisor_code: 0,
            length_enable: false,
            timer: 0,
            current_volume: 0,
            envelope_timer: 0,
            lfsr: 0x7FFF,
        }
    }

    /// Reset channel to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Advance the channel timer.
    pub fn tick(&mut self, cycles: u32) {
        if !self.enabled {
            return;
        }
        let period = DIVISOR_TABLE[self.divisor_code.min(7) as usize] << self.clock_shift;
        if period == 0 {
            return;
        }
        self.timer += cycles;
        while self.timer >= period {
            self.timer -= period;
            self.clock_lfsr();
        }
    }

    /// Clock the LFSR once.
    fn clock_lfsr(&mut self) {
        let bit0 = self.lfsr & 1;
        let bit1 = (self.lfsr >> 1) & 1;
        let new_bit = bit0 ^ bit1;
        self.lfsr >>= 1;
        self.lfsr |= new_bit << 14; // bit 14 for 15-bit LFSR
        if self.width_mode {
            // Also set bit 6 for 7-bit mode
            self.lfsr = (self.lfsr & !(1 << 6)) | (new_bit << 6);
        }
    }

    /// Get the current output sample.
    pub fn sample(&self) -> i16 {
        if !self.enabled {
            return 0;
        }
        // Output is inverted bit 0 of LFSR
        let high = (self.lfsr & 1) == 0;
        // GBA noise outputs 0..volume (unsigned 4-bit); use same centering as square channel.
        let v = self.current_volume as i16;
        if high {
            v / 2
        } else {
            -((v + 1) / 2)
        }
    }

    /// Trigger (restart) the channel.
    pub fn trigger(&mut self) {
        self.enabled = true;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer = 0;
        self.current_volume = self.envelope_volume;
        self.envelope_timer = self.envelope_period;
        self.lfsr = 0x7FFF;
        if self.envelope_volume == 0 && !self.envelope_dir {
            self.enabled = false;
        }
    }

    /// Clock the length counter.
    pub fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    /// Clock the envelope.
    pub fn clock_envelope(&mut self) {
        if self.envelope_period == 0 {
            return;
        }
        if self.envelope_timer > 0 {
            self.envelope_timer -= 1;
        }
        if self.envelope_timer == 0 {
            self.envelope_timer = self.envelope_period;
            if self.envelope_dir && self.current_volume < 15 {
                self.current_volume += 1;
            } else if !self.envelope_dir && self.current_volume > 0 {
                self.current_volume -= 1;
            }
        }
    }
}

impl Default for NoiseChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// FIFO channel (channels A & B)
// ---------------------------------------------------------------------------

/// DMA Sound FIFO channel (channels A & B)
#[derive(Clone, Serialize, Deserialize)]
pub struct FifoChannel {
    /// Whether the channel is enabled
    pub enabled: bool,
    /// Volume: true = 100%, false = 50%
    pub volume_full: bool,
    /// Output to right speaker
    pub enable_right: bool,
    /// Output to left speaker
    pub enable_left: bool,
    /// Which timer triggers playback (0 or 1)
    pub timer_select: u8,
    fifo: [i8; 32],
    read_pos: usize,
    write_pos: usize,
    count: usize,
    current_sample: i8,
}

impl FifoChannel {
    /// Create a new FIFO channel.
    pub fn new() -> Self {
        Self {
            enabled: false,
            volume_full: true,
            enable_right: false,
            enable_left: false,
            timer_select: 0,
            fifo: [0; 32],
            read_pos: 0,
            write_pos: 0,
            count: 0,
            current_sample: 0,
        }
    }

    /// Reset channel to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Write bytes into the FIFO buffer.
    pub fn write(&mut self, data: &[u8]) {
        for &byte in data {
            if self.count < 32 {
                self.fifo[self.write_pos] = byte as i8;
                self.write_pos = (self.write_pos + 1) % 32;
                self.count += 1;
            }
        }
    }

    /// Read the next sample from the FIFO (called on timer overflow).
    /// Returns true if a DMA refill should be requested (count <= 16).
    pub fn pop(&mut self) -> bool {
        if self.count > 0 {
            self.current_sample = self.fifo[self.read_pos];
            self.read_pos = (self.read_pos + 1) % 32;
            self.count -= 1;
        }
        self.count <= 16
    }

    /// Get the current output sample.
    pub fn sample(&self) -> i16 {
        if !self.enabled {
            return 0;
        }
        let s = self.current_sample as i16;
        if self.volume_full {
            s
        } else {
            s / 2
        }
    }

    /// Clear the FIFO buffer.
    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.count = 0;
        self.current_sample = 0;
    }

    /// Number of samples currently in the FIFO.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the FIFO is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Default for FifoChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Main APU
// ---------------------------------------------------------------------------

/// Main Audio Processing Unit.
#[derive(Clone, Serialize, Deserialize)]
pub struct Apu {
    /// Square wave channel 1 (with sweep)
    pub square1: SquareChannel,
    /// Square wave channel 2
    pub square2: SquareChannel,
    /// Wave channel
    pub wave: WaveChannel,
    /// Noise channel
    pub noise: NoiseChannel,
    /// DMA FIFO channel A
    pub fifo_a: FifoChannel,
    /// DMA FIFO channel B
    pub fifo_b: FifoChannel,
    /// Master volume left (0-7)
    pub volume_left: u8,
    /// Master volume right (0-7)
    pub volume_right: u8,
    /// PSG channel enable flags (left/right for each channel)
    pub psg_enable: u8,
    /// PSG master ratio from SOUNDCNT_H bits 0-1.
    pub psg_ratio: u8,
    /// Sound master enable
    pub master_enable: bool,
    /// SOUNDBIAS register
    pub sound_bias: u16,
    /// Output audio buffer (interleaved stereo i16 samples)
    pub sample_buffer: Vec<i16>,

    /// Frame sequencer step (0-7)
    frame_sequencer_step: u8,
    /// Cycle counter for frame sequencer
    frame_sequencer_cycles: u32,
    /// Cycle counter for sample generation
    sample_counter: u32,
    /// Cycles per sample (CPU_FREQ / SAMPLE_RATE)
    cycles_per_sample: u32,
    /// Low-pass filter state (fixed-point, 8 fractional bits)
    lp_left: i32,
    lp_right: i32,
}

impl Apu {
    /// Create a new APU with default state.
    pub fn new() -> Self {
        Self {
            square1: SquareChannel::new(),
            square2: SquareChannel::new(),
            wave: WaveChannel::new(),
            noise: NoiseChannel::new(),
            fifo_a: FifoChannel::new(),
            fifo_b: FifoChannel::new(),
            volume_left: 7,
            volume_right: 7,
            psg_enable: 0,
            psg_ratio: 0,
            master_enable: false,
            sound_bias: 0x0200,
            sample_buffer: Vec::with_capacity(BUFFER_SIZE * 2),
            frame_sequencer_step: 0,
            frame_sequencer_cycles: 0,
            sample_counter: 0,
            cycles_per_sample: CPU_FREQ / SAMPLE_RATE,
            lp_left: 0,
            lp_right: 0,
        }
    }

    /// Get the host PCM sample rate used for audio mixing/output.
    ///
    /// The hardware's SOUNDBIAS high bits control the PWM carrier, but generating
    /// host samples at that carrier rate is unnecessarily expensive and caused a
    /// noticeable emulation slowdown in real games. We keep host mixing fixed at
    /// the hardware-selected PWM rate so the frontend can resample from the
    /// same cadence the mixer is actually generating.
    pub fn output_sample_rate(&self) -> u32 {
        SAMPLE_RATE << self.soundbias_resolution()
    }

    fn update_sample_timing(&mut self) {
        self.cycles_per_sample = 0x200 >> self.soundbias_resolution();
    }

    fn soundbias_resolution(&self) -> u32 {
        ((self.sound_bias >> 14) & 0x3) as u32
    }

    /// Reset the APU to initial state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Advance the APU by `cycles` CPU cycles, generating samples into the buffer.
    pub fn tick(&mut self, cycles: u32) {
        if !self.master_enable {
            // Still need to advance sample counter to produce silence
            self.sample_counter += cycles;
            while self.sample_counter >= self.cycles_per_sample {
                self.sample_counter -= self.cycles_per_sample;
                self.push_stereo(0, 0);
            }
            return;
        }

        // Tick channels
        self.square1.tick(cycles);
        self.square2.tick(cycles);
        self.wave.tick(cycles);
        self.noise.tick(cycles);

        // Frame sequencer
        self.frame_sequencer_cycles += cycles;
        while self.frame_sequencer_cycles >= CYCLES_PER_FRAME_SEQ {
            self.frame_sequencer_cycles -= CYCLES_PER_FRAME_SEQ;
            self.clock_frame_sequencer();
        }

        // Sample generation
        self.sample_counter += cycles;
        while self.sample_counter >= self.cycles_per_sample {
            self.sample_counter -= self.cycles_per_sample;
            self.generate_sample();
        }
    }

    fn clock_frame_sequencer(&mut self) {
        match self.frame_sequencer_step {
            0 => {
                self.square1.clock_length();
                self.square2.clock_length();
                self.wave.clock_length();
                self.noise.clock_length();
            }
            2 => {
                self.square1.clock_length();
                self.square2.clock_length();
                self.wave.clock_length();
                self.noise.clock_length();
                self.square1.clock_sweep();
            }
            4 => {
                self.square1.clock_length();
                self.square2.clock_length();
                self.wave.clock_length();
                self.noise.clock_length();
            }
            6 => {
                self.square1.clock_length();
                self.square2.clock_length();
                self.wave.clock_length();
                self.noise.clock_length();
                self.square1.clock_sweep();
            }
            7 => {
                self.square1.clock_envelope();
                self.square2.clock_envelope();
                self.noise.clock_envelope();
            }
            _ => {}
        }
        self.frame_sequencer_step = (self.frame_sequencer_step + 1) & 7;
    }

    fn generate_sample(&mut self) {
        let sq1 = self.square1.sample();
        let sq2 = self.square2.sample();
        let wav = self.wave.sample();
        let noi = self.noise.sample();

        let psg = self.psg_enable;
        let mut left: i32 = 0;
        let mut right: i32 = 0;

        // PSG channel mixing based on psg_enable byte:
        // Bit 0-3: right enable for ch1-4
        // Bit 4-7: left enable for ch1-4
        if psg & 0x01 != 0 {
            right += sq1 as i32;
        }
        if psg & 0x02 != 0 {
            right += sq2 as i32;
        }
        if psg & 0x04 != 0 {
            right += wav as i32;
        }
        if psg & 0x08 != 0 {
            right += noi as i32;
        }
        if psg & 0x10 != 0 {
            left += sq1 as i32;
        }
        if psg & 0x20 != 0 {
            left += sq2 as i32;
        }
        if psg & 0x40 != 0 {
            left += wav as i32;
        }
        if psg & 0x80 != 0 {
            left += noi as i32;
        }

        // Apply master volume (0-7 → multiply by (volume+1))
        left = left * (self.volume_left as i32 + 1) / 8;
        right = right * (self.volume_right as i32 + 1) / 8;

        // SOUNDCNT_H bits 0-1 select the PSG contribution ratio:
        // 0=25%, 1=50%, 2/3=100%.
        let psg_numer = match self.psg_ratio & 0x03 {
            0 => 1,
            1 => 2,
            _ => 4,
        };
        left = left * psg_numer / 4;
        right = right * psg_numer / 4;
        // GBATEK documents that each PSG channel spans one quarter of the final
        // signed range, so scale the mixed PSG sum into hardware-like units
        // before applying SOUNDBIAS clipping.
        left *= 16;
        right *= 16;

        // Mix in FIFO channels
        // Each FIFO can span the full signed mixer range at 100% volume.
        let fifo_a = self.fifo_a.sample() as i32 * 4;
        let fifo_b = self.fifo_b.sample() as i32 * 4;

        if self.fifo_a.enable_left {
            left += fifo_a;
        }
        if self.fifo_a.enable_right {
            right += fifo_a;
        }
        if self.fifo_b.enable_left {
            left += fifo_b;
        }
        if self.fifo_b.enable_right {
            right += fifo_b;
        }

        // The GBA mixer produces a signed signal around the SOUNDBIAS midpoint,
        // then clips into the unsigned 10-bit DAC range before output.
        let bias = (self.sound_bias & 0x03FF) as i32;
        let clamp_mixer = |sample: i32| {
            let unsigned = (sample + bias).clamp(0, 0x03FF);
            unsigned - bias
        };
        let host_scale = 64;
        let left = clamp_mixer(left) * host_scale;
        let right = clamp_mixer(right) * host_scale;

        // One-pole low-pass filter to smooth square-wave harshness.
        // y[n] = (1-k)*y[n-1] + k*x[n], k=220/256 → cutoff ≈ 10 kHz at 32768 Hz.
        const K: i32 = 220;
        self.lp_left = (self.lp_left * (256 - K) + left * K) / 256;
        self.lp_right = (self.lp_right * (256 - K) + right * K) / 256;

        self.push_stereo(self.lp_left as i16, self.lp_right as i16);
    }

    /// Append one stereo frame, discarding the oldest audio at the cap.
    ///
    /// Always writes both channels together so the interleaved buffer can never
    /// end up with an odd length, which would swap left and right for every
    /// sample after it.
    fn push_stereo(&mut self, left: i16, right: i16) {
        if self.sample_buffer.len() >= MAX_BUFFERED_SAMPLES {
            let discard = SAMPLE_DISCARD_CHUNK.min(self.sample_buffer.len());
            // Keep the discard even so the stereo pairing survives.
            let discard = discard - (discard % 2);
            self.sample_buffer.drain(..discard);
        }
        self.sample_buffer.push(left);
        self.sample_buffer.push(right);
    }

    /// Take all buffered audio samples for output (interleaved stereo).
    pub fn drain_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.sample_buffer)
    }

    /// Push data into a FIFO channel. `channel`: 0 = FIFO A, 1 = FIFO B.
    pub fn fifo_write(&mut self, channel: usize, data: &[u8]) {
        match channel {
            0 => self.fifo_a.write(data),
            1 => self.fifo_b.write(data),
            _ => {}
        }
    }

    /// Called when timer 0 or 1 overflows. Advances the associated FIFO playback.
    /// Returns true if a DMA refill is needed for the respective FIFO.
    pub fn on_timer_overflow(&mut self, timer: u8) -> (bool, bool) {
        let mut need_refill_a = false;
        let mut need_refill_b = false;

        if self.fifo_a.enabled && self.fifo_a.timer_select == timer {
            need_refill_a = self.fifo_a.pop();
        }
        if self.fifo_b.enabled && self.fifo_b.timer_select == timer {
            need_refill_b = self.fifo_b.pop();
        }

        (need_refill_a, need_refill_b)
    }

    /// Read a sound register (address range 0x04000060 - 0x040000A7).
    pub fn read_register(&self, addr: u32) -> u8 {
        match addr {
            // SOUND1CNT_L - Sweep (channel 1)
            SOUND1CNT_L => {
                (self.square1.sweep_period << 4)
                    | if self.square1.sweep_dir { 0x08 } else { 0 }
                    | self.square1.sweep_shift
            }
            0x0400_0061 => 0, // upper byte unused

            // SOUND1CNT_H - Duty/Length/Envelope
            SOUND1CNT_H => self.square1.duty_cycle << 6, // length is write-only
            0x0400_0063 => {
                (self.square1.envelope_volume << 4)
                    | if self.square1.envelope_dir { 0x08 } else { 0 }
                    | self.square1.envelope_period
            }

            // SOUND1CNT_X - Frequency/Control
            SOUND1CNT_X => 0, // frequency is write-only
            0x0400_0065 => {
                if self.square1.length_enable {
                    0x40
                } else {
                    0
                }
            }

            // SOUND2CNT_L - Duty/Length/Envelope
            SOUND2CNT_L => self.square2.duty_cycle << 6,
            0x0400_0069 => {
                (self.square2.envelope_volume << 4)
                    | if self.square2.envelope_dir { 0x08 } else { 0 }
                    | self.square2.envelope_period
            }

            // SOUND2CNT_H - Frequency/Control
            SOUND2CNT_H => 0,
            0x0400_006D => {
                if self.square2.length_enable {
                    0x40
                } else {
                    0
                }
            }

            // SOUND3CNT_L
            SOUND3CNT_L => {
                ((self.wave.two_banks as u8) << 5)
                    | ((self.wave.bank_select & 1) << 6)
                    | if self.wave.dac_enabled { 0x80 } else { 0 }
            }
            0x0400_0071 => 0,

            // SOUND3CNT_H
            SOUND3CNT_H => 0, // length write-only
            0x0400_0073 => {
                if self.wave.volume == 4 {
                    0x80
                } else {
                    (self.wave.volume & 0x03) << 5
                }
            }

            // SOUND3CNT_X
            SOUND3CNT_X => 0,
            0x0400_0075 => {
                if self.wave.length_enable {
                    0x40
                } else {
                    0
                }
            }

            // SOUND4CNT_L
            SOUND4CNT_L => 0, // length write-only
            0x0400_0079 => {
                (self.noise.envelope_volume << 4)
                    | if self.noise.envelope_dir { 0x08 } else { 0 }
                    | self.noise.envelope_period
            }

            // SOUND4CNT_H
            SOUND4CNT_H => {
                (self.noise.clock_shift << 4)
                    | if self.noise.width_mode { 0x08 } else { 0 }
                    | self.noise.divisor_code
            }
            0x0400_007D => {
                if self.noise.length_enable {
                    0x40
                } else {
                    0
                }
            }

            // SOUNDCNT_L - PSG master volume/enable
            SOUNDCNT_L => (self.volume_right & 7) | ((self.volume_left & 7) << 4),
            0x0400_0081 => self.psg_enable,

            // SOUNDCNT_H - DMA sound control
            SOUNDCNT_H => {
                let mut val: u8 = self.psg_ratio & 0x03;
                if self.fifo_a.volume_full {
                    val |= 0x04;
                }
                if self.fifo_b.volume_full {
                    val |= 0x08;
                }
                val
            }
            0x0400_0083 => {
                let mut val: u8 = 0;
                if self.fifo_a.enable_right {
                    val |= 0x01;
                }
                if self.fifo_a.enable_left {
                    val |= 0x02;
                }
                if self.fifo_a.timer_select != 0 {
                    val |= 0x04;
                }
                if self.fifo_b.enable_right {
                    val |= 0x10;
                }
                if self.fifo_b.enable_left {
                    val |= 0x20;
                }
                if self.fifo_b.timer_select != 0 {
                    val |= 0x40;
                }
                val
            }

            // SOUNDCNT_X - Master enable + status
            SOUNDCNT_X => {
                let mut val: u8 = 0;
                if self.square1.enabled {
                    val |= 0x01;
                }
                if self.square2.enabled {
                    val |= 0x02;
                }
                if self.wave.enabled {
                    val |= 0x04;
                }
                if self.noise.enabled {
                    val |= 0x08;
                }
                if self.master_enable {
                    val |= 0x80;
                }
                val
            }
            0x0400_0085 => 0,

            // SOUNDBIAS
            SOUNDBIAS => (self.sound_bias & 0xFF) as u8,
            0x0400_0089 => ((self.sound_bias >> 8) & 0xFF) as u8,

            // Wave RAM
            WAVE_RAM_START..=WAVE_RAM_END => {
                let offset = (addr - WAVE_RAM_START) as usize;
                self.wave.read_wave_ram_byte(offset)
            }

            // FIFO registers are write-only
            FIFO_A..=0x0400_00A3 => 0,
            FIFO_B..=0x0400_00A7 => 0,

            _ => 0,
        }
    }

    /// Write a sound register (address range 0x04000060 - 0x040000A7).
    pub fn write_register(&mut self, addr: u32, value: u8) {
        if !self.master_enable && addr != SOUNDCNT_X && addr != 0x0400_0085 {
            // When master is disabled, only SOUNDCNT_X and SOUNDBIAS are writable
            if !(SOUNDCNT_H..=0x0400_0083).contains(&addr)
                && !(SOUNDBIAS..=0x0400_0089).contains(&addr)
            {
                return;
            }
        }

        match addr {
            // SOUND1CNT_L - Sweep
            SOUND1CNT_L => {
                self.square1.sweep_period = (value >> 4) & 7;
                self.square1.sweep_dir = value & 0x08 != 0;
                self.square1.sweep_shift = value & 7;
            }
            0x0400_0061 => {} // unused upper byte

            // SOUND1CNT_H - Duty/Length/Envelope
            SOUND1CNT_H => {
                self.square1.duty_cycle = (value >> 6) & 3;
                self.square1.length_counter = 64 - (value & 0x3F);
            }
            0x0400_0063 => {
                self.square1.envelope_volume = (value >> 4) & 0x0F;
                self.square1.envelope_dir = value & 0x08 != 0;
                self.square1.envelope_period = value & 7;
            }

            // SOUND1CNT_X - Frequency/Control
            SOUND1CNT_X => {
                self.square1.frequency = (self.square1.frequency & 0x0700) | value as u16;
            }
            0x0400_0065 => {
                self.square1.frequency =
                    (self.square1.frequency & 0x00FF) | (((value & 7) as u16) << 8);
                self.square1.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.square1.trigger();
                }
            }

            // SOUND2CNT_L - Duty/Length/Envelope
            SOUND2CNT_L => {
                self.square2.duty_cycle = (value >> 6) & 3;
                self.square2.length_counter = 64 - (value & 0x3F);
            }
            0x0400_0069 => {
                self.square2.envelope_volume = (value >> 4) & 0x0F;
                self.square2.envelope_dir = value & 0x08 != 0;
                self.square2.envelope_period = value & 7;
            }

            // SOUND2CNT_H - Frequency/Control
            SOUND2CNT_H => {
                self.square2.frequency = (self.square2.frequency & 0x0700) | value as u16;
            }
            0x0400_006D => {
                self.square2.frequency =
                    (self.square2.frequency & 0x00FF) | (((value & 7) as u16) << 8);
                self.square2.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.square2.trigger();
                }
            }

            // SOUND3CNT_L
            SOUND3CNT_L => {
                self.wave.two_banks = value & 0x20 != 0;
                self.wave.bank_select = (value >> 6) & 1;
                self.wave.dac_enabled = value & 0x80 != 0;
                if !self.wave.dac_enabled {
                    self.wave.enabled = false;
                } else {
                    self.wave.playback_bank = self.wave.bank_select;
                    if !self.wave.two_banks {
                        self.wave.position &= 31;
                    }
                    self.wave.sample_buffer = self.wave.read_wave_sample(self.wave.position);
                }
            }
            0x0400_0071 => {}

            // SOUND3CNT_H
            SOUND3CNT_H => {
                self.wave.length_counter = (256 - value as u16) & 0xFF;
            }
            0x0400_0073 => {
                self.wave.volume = if value & 0x80 != 0 {
                    4
                } else {
                    (value >> 5) & 3
                };
            }

            // SOUND3CNT_X
            SOUND3CNT_X => {
                self.wave.frequency = (self.wave.frequency & 0x0700) | value as u16;
            }
            0x0400_0075 => {
                self.wave.frequency = (self.wave.frequency & 0x00FF) | (((value & 7) as u16) << 8);
                self.wave.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.wave.trigger();
                }
            }

            // SOUND4CNT_L
            SOUND4CNT_L => {
                self.noise.length_counter = 64 - (value & 0x3F);
            }
            0x0400_0079 => {
                self.noise.envelope_volume = (value >> 4) & 0x0F;
                self.noise.envelope_dir = value & 0x08 != 0;
                self.noise.envelope_period = value & 7;
            }

            // SOUND4CNT_H
            SOUND4CNT_H => {
                self.noise.clock_shift = (value >> 4) & 0x0F;
                self.noise.width_mode = value & 0x08 != 0;
                self.noise.divisor_code = value & 7;
            }
            0x0400_007D => {
                self.noise.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    self.noise.trigger();
                }
            }

            // SOUNDCNT_L
            SOUNDCNT_L => {
                self.volume_right = value & 7;
                self.volume_left = (value >> 4) & 7;
            }
            0x0400_0081 => {
                self.psg_enable = value;
            }

            // SOUNDCNT_H
            SOUNDCNT_H => {
                self.psg_ratio = value & 0x03;
                self.fifo_a.volume_full = value & 0x04 != 0;
                self.fifo_b.volume_full = value & 0x08 != 0;
            }
            0x0400_0083 => {
                self.fifo_a.enable_right = value & 0x01 != 0;
                self.fifo_a.enable_left = value & 0x02 != 0;
                self.fifo_a.enabled = self.fifo_a.enable_left || self.fifo_a.enable_right;
                self.fifo_a.timer_select = if value & 0x04 != 0 { 1 } else { 0 };
                if value & 0x08 != 0 {
                    self.fifo_a.clear();
                }
                self.fifo_b.enable_right = value & 0x10 != 0;
                self.fifo_b.enable_left = value & 0x20 != 0;
                self.fifo_b.enabled = self.fifo_b.enable_left || self.fifo_b.enable_right;
                self.fifo_b.timer_select = if value & 0x40 != 0 { 1 } else { 0 };
                if value & 0x80 != 0 {
                    self.fifo_b.clear();
                }
            }

            // SOUNDCNT_X - Master enable
            SOUNDCNT_X => {
                let was_enabled = self.master_enable;
                self.master_enable = value & 0x80 != 0;
                if was_enabled && !self.master_enable {
                    // Turning off: reset all channels
                    self.square1.reset();
                    self.square2.reset();
                    self.wave.reset();
                    self.noise.reset();
                    self.fifo_a.reset();
                    self.fifo_b.reset();
                    self.volume_left = 0;
                    self.volume_right = 0;
                    self.psg_enable = 0;
                    self.psg_ratio = 0;
                }
            }
            0x0400_0085 => {} // read-only upper byte

            // SOUNDBIAS
            SOUNDBIAS => {
                self.sound_bias = (self.sound_bias & 0xFF00) | value as u16;
                self.update_sample_timing();
            }
            0x0400_0089 => {
                self.sound_bias = (self.sound_bias & 0x00FF) | ((value as u16) << 8);
                self.update_sample_timing();
            }

            // Wave RAM
            WAVE_RAM_START..=WAVE_RAM_END => {
                let offset = (addr - WAVE_RAM_START) as usize;
                self.wave.write_wave_ram_byte(offset, value);
            }

            // FIFO_A (write 4 bytes)
            FIFO_A..=0x0400_00A3 => {
                self.fifo_a.write(&[value]);
            }

            // FIFO_B (write 4 bytes)
            FIFO_B..=0x0400_00A7 => {
                self.fifo_b.write(&[value]);
            }

            _ => {}
        }
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LegacyWaveChannelV2 {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub length_counter: u16,
    pub volume: u8,
    pub frequency: u16,
    pub length_enable: bool,
    pub wave_ram: [u8; 16],
    timer: u32,
    position: u8,
    sample_buffer: u8,
}

impl From<&WaveChannel> for LegacyWaveChannelV2 {
    fn from(channel: &WaveChannel) -> Self {
        let mut wave_ram = [0u8; 16];
        wave_ram.copy_from_slice(&channel.wave_ram[..16]);
        Self {
            enabled: channel.enabled,
            dac_enabled: channel.dac_enabled,
            length_counter: channel.length_counter,
            volume: channel.volume.min(3),
            frequency: channel.frequency,
            length_enable: channel.length_enable,
            wave_ram,
            timer: channel.timer,
            position: channel.position & 31,
            sample_buffer: channel.sample_buffer,
        }
    }
}

impl From<LegacyWaveChannelV2> for WaveChannel {
    fn from(channel: LegacyWaveChannelV2) -> Self {
        let mut wave_ram = [0u8; 32];
        wave_ram[..16].copy_from_slice(&channel.wave_ram);
        wave_ram[16..].copy_from_slice(&channel.wave_ram);
        Self {
            enabled: channel.enabled,
            dac_enabled: channel.dac_enabled,
            length_counter: channel.length_counter,
            volume: channel.volume,
            frequency: channel.frequency,
            length_enable: channel.length_enable,
            wave_ram,
            two_banks: false,
            bank_select: 0,
            timer: channel.timer,
            position: channel.position,
            sample_buffer: channel.sample_buffer,
            playback_bank: 0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct LegacyApuV2 {
    pub square1: SquareChannel,
    pub square2: SquareChannel,
    pub wave: LegacyWaveChannelV2,
    pub noise: NoiseChannel,
    pub fifo_a: FifoChannel,
    pub fifo_b: FifoChannel,
    pub volume_left: u8,
    pub volume_right: u8,
    pub psg_enable: u8,
    pub psg_ratio: u8,
    pub master_enable: bool,
    pub sound_bias: u16,
    pub sample_buffer: Vec<i16>,
    frame_sequencer_step: u8,
    frame_sequencer_cycles: u32,
    sample_counter: u32,
    cycles_per_sample: u32,
    lp_left: i32,
    lp_right: i32,
}

impl From<&Apu> for LegacyApuV2 {
    fn from(apu: &Apu) -> Self {
        Self {
            square1: apu.square1.clone(),
            square2: apu.square2.clone(),
            wave: LegacyWaveChannelV2::from(&apu.wave),
            noise: apu.noise.clone(),
            fifo_a: apu.fifo_a.clone(),
            fifo_b: apu.fifo_b.clone(),
            volume_left: apu.volume_left,
            volume_right: apu.volume_right,
            psg_enable: apu.psg_enable,
            psg_ratio: apu.psg_ratio,
            master_enable: apu.master_enable,
            sound_bias: apu.sound_bias,
            sample_buffer: apu.sample_buffer.clone(),
            frame_sequencer_step: apu.frame_sequencer_step,
            frame_sequencer_cycles: apu.frame_sequencer_cycles,
            sample_counter: apu.sample_counter,
            cycles_per_sample: apu.cycles_per_sample,
            lp_left: apu.lp_left,
            lp_right: apu.lp_right,
        }
    }
}

impl From<LegacyApuV2> for Apu {
    fn from(apu: LegacyApuV2) -> Self {
        Self {
            square1: apu.square1,
            square2: apu.square2,
            wave: apu.wave.into(),
            noise: apu.noise,
            fifo_a: apu.fifo_a,
            fifo_b: apu.fifo_b,
            volume_left: apu.volume_left,
            volume_right: apu.volume_right,
            psg_enable: apu.psg_enable,
            psg_ratio: apu.psg_ratio,
            master_enable: apu.master_enable,
            sound_bias: apu.sound_bias,
            sample_buffer: apu.sample_buffer,
            frame_sequencer_step: apu.frame_sequencer_step,
            frame_sequencer_cycles: apu.frame_sequencer_cycles,
            sample_counter: apu.sample_counter,
            cycles_per_sample: apu.cycles_per_sample,
            lp_left: apu.lp_left,
            lp_right: apu.lp_right,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_samples_are_capped_when_nothing_drains() {
        // A headless run never drains. Before the cap the buffer grew for the
        // whole session and was serialized into savestates, producing files of
        // a hundred megabytes.
        let mut apu = Apu::new();
        for _ in 0..MAX_BUFFERED_SAMPLES * 2 {
            apu.push_stereo(1, -1);
        }
        assert!(
            apu.sample_buffer.len() <= MAX_BUFFERED_SAMPLES,
            "buffer grew to {} past the {MAX_BUFFERED_SAMPLES} cap",
            apu.sample_buffer.len()
        );
    }

    #[test]
    fn the_buffer_stays_stereo_aligned_across_the_cap() {
        // An odd length would swap left and right for every later sample.
        let mut apu = Apu::new();
        // Values stay inside i16 so the test exercises the buffer, not overflow.
        for index in 0..MAX_BUFFERED_SAMPLES + SAMPLE_DISCARD_CHUNK + 7 {
            let value = (index % 1000) as i16;
            apu.push_stereo(value, -value);
        }
        assert!(apu.sample_buffer.len().is_multiple_of(2));
        for pair in apu.sample_buffer.chunks_exact(2) {
            assert_eq!(pair[0], -pair[1], "a left/right pair was split");
        }
    }

    #[test]
    fn the_cap_keeps_the_newest_audio() {
        let mut apu = Apu::new();
        let total = MAX_BUFFERED_SAMPLES + SAMPLE_DISCARD_CHUNK;
        for index in 0..total {
            let value = (index % 1000) as i16;
            apu.push_stereo(value, value);
        }
        let last = *apu.sample_buffer.last().expect("buffer should not be empty");
        assert_eq!(
            last,
            ((total - 1) % 1000) as i16,
            "the most recent sample must survive"
        );
    }

    #[test]
    fn draining_empties_the_buffer() {
        let mut apu = Apu::new();
        apu.push_stereo(5, 6);
        assert_eq!(apu.drain_samples(), vec![5, 6]);
        assert!(apu.sample_buffer.is_empty());
        assert!(apu.drain_samples().is_empty());
    }

    #[test]
    fn test_square_channel_new_and_reset() {
        let mut ch = SquareChannel::new();
        assert!(!ch.enabled);
        assert_eq!(ch.frequency, 0);
        assert_eq!(ch.duty_cycle, 0);
        assert_eq!(ch.current_volume, 0);
        ch.enabled = true;
        ch.frequency = 1000;
        ch.reset();
        assert!(!ch.enabled);
        assert_eq!(ch.frequency, 0);
    }

    #[test]
    fn test_square_channel_trigger() {
        let mut ch = SquareChannel::new();
        ch.envelope_volume = 10;
        ch.envelope_dir = true;
        ch.envelope_period = 3;
        ch.trigger();
        assert!(ch.enabled);
        assert_eq!(ch.current_volume, 10);
        assert_eq!(ch.length_counter, 64);
    }

    #[test]
    fn test_square_channel_trigger_dac_off() {
        let mut ch = SquareChannel::new();
        ch.envelope_volume = 0;
        ch.envelope_dir = false;
        ch.trigger();
        // DAC is off when volume=0 and direction=decrease
        assert!(!ch.enabled);
    }

    #[test]
    fn test_square_duty_cycle_output() {
        // 50% duty (duty_cycle=2) should produce: [1,0,0,0,0,1,1,1]
        let mut ch = SquareChannel::new();
        ch.enabled = true;
        ch.duty_cycle = 2;
        ch.current_volume = 15;

        let expected = [true, false, false, false, false, true, true, true];
        for (i, &expect_high) in expected.iter().enumerate() {
            ch.duty_pos = i as u8;
            let s = ch.sample();
            if expect_high {
                assert!(s > 0, "duty pos {} should be high, got {}", i, s);
            } else {
                assert!(s < 0, "duty pos {} should be low, got {}", i, s);
            }
        }
    }

    #[test]
    fn test_square_duty_12_5_percent() {
        // 12.5% duty (duty_cycle=0): [0,0,0,0,0,0,0,1]
        let mut ch = SquareChannel::new();
        ch.enabled = true;
        ch.duty_cycle = 0;
        ch.current_volume = 8;

        let mut high_count = 0;
        for pos in 0..8 {
            ch.duty_pos = pos;
            if ch.sample() > 0 {
                high_count += 1;
            }
        }
        assert_eq!(high_count, 1, "12.5% duty should have 1 high out of 8");
    }

    #[test]
    fn test_square_length_counter() {
        let mut ch = SquareChannel::new();
        ch.enabled = true;
        ch.length_enable = true;
        ch.length_counter = 3;
        ch.clock_length();
        assert_eq!(ch.length_counter, 2);
        assert!(ch.enabled);
        ch.clock_length();
        ch.clock_length();
        assert_eq!(ch.length_counter, 0);
        assert!(!ch.enabled);
    }

    #[test]
    fn test_square_envelope() {
        let mut ch = SquareChannel::new();
        ch.envelope_volume = 5;
        ch.envelope_dir = true;
        ch.envelope_period = 1;
        ch.trigger();
        assert_eq!(ch.current_volume, 5);
        // First clock sets timer to 0, then reloads and increments
        ch.clock_envelope();
        assert_eq!(ch.current_volume, 6);
    }

    #[test]
    fn test_wave_channel_new_and_reset() {
        let mut ch = WaveChannel::new();
        assert!(!ch.enabled);
        assert_eq!(ch.wave_ram, [0; 32]);
        ch.enabled = true;
        ch.wave_ram[0] = 0xFF;
        ch.reset();
        assert!(!ch.enabled);
        assert_eq!(ch.wave_ram[0], 0);
    }

    #[test]
    fn test_wave_channel_sample_muted() {
        let ch = WaveChannel::new();
        assert_eq!(ch.sample(), 0);
    }

    #[test]
    fn test_wave_channel_force_volume_75_percent() {
        let mut ch = WaveChannel::new();
        ch.enabled = true;
        ch.dac_enabled = true;
        ch.sample_buffer = 15;
        ch.volume = 4;

        assert_eq!(ch.sample(), 5);
    }

    #[test]
    fn test_wave_channel_trigger_primes_first_sample() {
        let mut ch = WaveChannel::new();
        ch.dac_enabled = true;
        ch.wave_ram[0] = 0xF1;

        ch.trigger();

        assert!(ch.enabled);
        assert_eq!(ch.sample_buffer, 0x0F);
    }

    #[test]
    fn test_wave_channel_reads_and_writes_inactive_bank() {
        let mut ch = WaveChannel::new();
        ch.bank_select = 0;
        ch.dac_enabled = true;

        ch.write_wave_ram_byte(0, 0xAB);

        assert_eq!(ch.wave_ram[0], 0x00);
        assert_eq!(ch.wave_ram[16], 0xAB);
        assert_eq!(ch.read_wave_ram_byte(0), 0xAB);
    }

    #[test]
    fn test_wave_channel_two_bank_playback_flips_after_32_samples() {
        let mut ch = WaveChannel::new();
        ch.dac_enabled = true;
        ch.two_banks = true;
        ch.bank_select = 0;
        ch.wave_ram[0] = 0xF0;
        ch.wave_ram[16] = 0x10;
        ch.trigger();

        assert_eq!(ch.sample_buffer, 0x0F);
        ch.position = 31;
        ch.tick((2048 - ch.frequency as u32) * 8);
        assert_eq!(ch.position, 32);
        assert_eq!(ch.sample_buffer, 0x01);
    }

    #[test]
    fn test_noise_channel_new_and_reset() {
        let mut ch = NoiseChannel::new();
        assert!(!ch.enabled);
        assert_eq!(ch.lfsr, 0x7FFF);
        ch.enabled = true;
        ch.lfsr = 0;
        ch.reset();
        assert!(!ch.enabled);
        assert_eq!(ch.lfsr, 0x7FFF);
    }

    #[test]
    fn test_noise_lfsr_15bit() {
        let mut ch = NoiseChannel::new();
        ch.enabled = true;
        ch.envelope_volume = 15;
        ch.width_mode = false;
        ch.trigger();

        // LFSR starts at 0x7FFF. Clock it a few times and verify it changes.
        let initial = ch.lfsr;
        ch.clock_lfsr();
        // bit0=1, bit1=1, xor=0, so lfsr = 0x3FFF
        assert_eq!(ch.lfsr, 0x3FFF);
        ch.clock_lfsr();
        // bit0=1, bit1=1, xor=0, so lfsr = 0x1FFF
        assert_eq!(ch.lfsr, 0x1FFF);
        assert_ne!(ch.lfsr, initial);
    }

    #[test]
    fn test_noise_lfsr_7bit() {
        let mut ch = NoiseChannel::new();
        ch.width_mode = true;
        ch.trigger();

        let initial = ch.lfsr;
        ch.clock_lfsr();
        // In 7-bit mode, bit 6 is also set from XOR result
        // Initial: 0x7FFF. bit0=1, bit1=1, xor=0.
        // lfsr >> 1 = 0x3FFF, | (0 << 14) = 0x3FFF
        // Clear bit 6, set to 0: 0x3FFF & ~0x40 = 0x3FBF
        assert_eq!(ch.lfsr, 0x3FBF);
        assert_ne!(ch.lfsr, initial);
    }

    #[test]
    fn test_fifo_push_pop() {
        let mut fifo = FifoChannel::new();
        assert!(fifo.is_empty());
        assert_eq!(fifo.len(), 0);

        fifo.write(&[10, 20, 30, 40]);
        assert_eq!(fifo.len(), 4);
        assert!(!fifo.is_empty());

        fifo.pop();
        assert_eq!(fifo.current_sample, 10);
        assert_eq!(fifo.len(), 3);

        fifo.pop();
        assert_eq!(fifo.current_sample, 20);
        assert_eq!(fifo.len(), 2);
    }

    #[test]
    fn test_fifo_overflow() {
        let mut fifo = FifoChannel::new();
        // Fill 32 bytes
        for i in 0..32u8 {
            fifo.write(&[i]);
        }
        assert_eq!(fifo.len(), 32);
        // Writing more should be dropped
        fifo.write(&[99]);
        assert_eq!(fifo.len(), 32);
    }

    #[test]
    fn test_fifo_clear() {
        let mut fifo = FifoChannel::new();
        fifo.write(&[1, 2, 3, 4]);
        fifo.clear();
        assert!(fifo.is_empty());
        assert_eq!(fifo.current_sample, 0);
    }

    #[test]
    fn test_fifo_dma_refill_request() {
        let mut fifo = FifoChannel::new();
        // Write exactly 17 bytes
        for i in 0..17u8 {
            fifo.write(&[i]);
        }
        assert_eq!(fifo.len(), 17);
        // Pop one: count goes to 16, which triggers refill request
        let needs_refill = fifo.pop();
        assert!(needs_refill, "should request DMA refill when count <= 16");
    }

    #[test]
    fn test_fifo_sample_volume() {
        let mut fifo = FifoChannel::new();
        fifo.enabled = true;
        fifo.volume_full = true;
        fifo.write(&[64]); // 64 as i8 = 64
        fifo.pop();
        let full = fifo.sample();

        fifo.volume_full = false;
        let half = fifo.sample();
        assert_eq!(half, full / 2);
    }

    #[test]
    fn test_apu_new_and_reset() {
        let mut apu = Apu::new();
        assert!(!apu.master_enable);
        assert_eq!(apu.sound_bias, 0x0200);
        assert!(apu.sample_buffer.is_empty());
        apu.master_enable = true;
        apu.reset();
        assert!(!apu.master_enable);
    }

    #[test]
    fn test_apu_drain_samples() {
        let mut apu = Apu::new();
        apu.sample_buffer.push(100);
        apu.sample_buffer.push(200);
        let samples = apu.drain_samples();
        assert_eq!(samples, vec![100, 200]);
        assert!(apu.sample_buffer.is_empty());
    }

    #[test]
    fn test_apu_tick_generates_silence_when_disabled() {
        let mut apu = Apu::new();
        apu.master_enable = false;
        // Tick enough cycles to generate at least one sample
        apu.tick(apu.cycles_per_sample + 1);
        let samples = apu.drain_samples();
        assert!(!samples.is_empty());
        for &s in &samples {
            assert_eq!(s, 0, "disabled APU should produce silence");
        }
    }

    #[test]
    fn test_apu_tick_generates_centered_silence_when_enabled_but_idle() {
        let mut apu = Apu::new();
        apu.master_enable = true;
        apu.tick(apu.cycles_per_sample + 1);
        let samples = apu.drain_samples();
        assert!(!samples.is_empty());
        for &s in &samples {
            assert_eq!(s, 0, "idle enabled APU should stay centered at zero");
        }
    }

    #[test]
    fn test_apu_fifo_write_and_timer_overflow() {
        let mut apu = Apu::new();
        apu.fifo_a.enabled = true;
        apu.fifo_a.timer_select = 0;
        apu.fifo_write(0, &[42, 43, 44, 45]);
        assert_eq!(apu.fifo_a.len(), 4);

        let (needs_a, _) = apu.on_timer_overflow(0);
        assert!(needs_a); // count is now 3, which is <= 16
        assert_eq!(apu.fifo_a.current_sample, 42);
    }

    #[test]
    fn test_apu_register_soundcnt_x() {
        let mut apu = Apu::new();
        // Enable master
        apu.write_register(SOUNDCNT_X, 0x80);
        assert!(apu.master_enable);
        assert_eq!(apu.read_register(SOUNDCNT_X) & 0x80, 0x80);

        // Disable master
        apu.write_register(SOUNDCNT_X, 0x00);
        assert!(!apu.master_enable);
    }

    #[test]
    fn test_apu_register_wave_ram() {
        let mut apu = Apu::new();
        apu.master_enable = true;
        for i in 0..16u8 {
            apu.write_register(WAVE_RAM_START + i as u32, i * 0x11);
        }
        for i in 0..16u8 {
            assert_eq!(apu.read_register(WAVE_RAM_START + i as u32), i * 0x11);
        }
    }

    #[test]
    fn test_apu_register_soundcnt_h_is_writable_while_master_disabled() {
        let mut apu = Apu::new();
        assert!(!apu.master_enable);

        apu.write_register(SOUNDCNT_H, 0x09);
        apu.write_register(0x0400_0083, 0x33);

        assert_eq!(apu.psg_ratio, 1);
        assert!(apu.fifo_b.volume_full);
        assert!(apu.fifo_a.enable_left);
        assert!(apu.fifo_b.enable_left);
    }

    #[test]
    fn test_apu_wave_force_volume_register_round_trip() {
        let mut apu = Apu::new();
        apu.master_enable = true;

        apu.write_register(0x0400_0073, 0x80);

        assert_eq!(apu.wave.volume, 4);
        assert_eq!(apu.read_register(0x0400_0073), 0x80);
    }

    #[test]
    fn test_apu_register_soundcnt_l() {
        let mut apu = Apu::new();
        apu.master_enable = true;
        apu.write_register(SOUNDCNT_L, 0x37); // left=3, right=7
        assert_eq!(apu.volume_right, 7);
        assert_eq!(apu.volume_left, 3);
        assert_eq!(apu.read_register(SOUNDCNT_L), 0x37);

        apu.write_register(0x0400_0081, 0xFF); // all channels enabled L+R
        assert_eq!(apu.psg_enable, 0xFF);
    }

    #[test]
    fn test_apu_register_soundcnt_h_enables_fifo_channels_and_ratio() {
        let mut apu = Apu::new();
        apu.master_enable = true;

        apu.write_register(SOUNDCNT_H, 0x01); // PSG ratio 50%
        apu.write_register(0x0400_0083, 0x33); // FIFO A L+R, FIFO B L+R, timer 0

        assert_eq!(apu.psg_ratio, 1);
        assert!(apu.fifo_a.enabled);
        assert!(apu.fifo_b.enabled);
        assert!(apu.fifo_a.enable_left);
        assert!(apu.fifo_a.enable_right);
        assert!(apu.fifo_b.enable_left);
        assert!(apu.fifo_b.enable_right);
    }

    #[test]
    fn test_apu_soundbias_updates_output_rate() {
        let mut apu = Apu::new();

        assert_eq!(apu.output_sample_rate(), SAMPLE_RATE);
        assert_eq!(apu.cycles_per_sample, CPU_FREQ / SAMPLE_RATE);

        apu.write_register(0x0400_0089, 0x42);
        assert_eq!(apu.sound_bias, 0x4200);
        assert_eq!(apu.output_sample_rate(), SAMPLE_RATE << 1);
        assert_eq!(apu.cycles_per_sample, CPU_FREQ / (SAMPLE_RATE << 1));

        apu.write_register(0x0400_0089, 0x82);
        assert_eq!(apu.output_sample_rate(), SAMPLE_RATE << 2);
        assert_eq!(apu.cycles_per_sample, CPU_FREQ / (SAMPLE_RATE << 2));

        apu.write_register(0x0400_0089, 0xC2);
        assert_eq!(apu.output_sample_rate(), SAMPLE_RATE << 3);
        assert_eq!(apu.cycles_per_sample, CPU_FREQ / (SAMPLE_RATE << 3));
    }
}

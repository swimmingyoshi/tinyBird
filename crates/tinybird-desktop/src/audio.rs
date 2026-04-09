//! Audio output handler using cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tinybird_core::apu::SAMPLE_RATE as DEFAULT_GBA_SAMPLE_RATE;

const MAX_BUFFERED_FRAMES: usize = 262_144;
const MIN_PRIME_FRAMES: usize = 512;

struct SharedAudioState {
    samples: VecDeque<i16>,
    current_left: i16,
    current_right: i16,
    next_left: i16,
    next_right: i16,
    source_rate: u32,
    resample_phase: u32,
    primed: bool,
    starved: bool,
    volume: f32,
}

impl Default for SharedAudioState {
    fn default() -> Self {
        Self {
            samples: VecDeque::new(),
            current_left: 0,
            current_right: 0,
            next_left: 0,
            next_right: 0,
            source_rate: DEFAULT_GBA_SAMPLE_RATE,
            resample_phase: 0,
            primed: false,
            starved: false,
            volume: 1.0,
        }
    }
}

impl SharedAudioState {
    fn pop_stereo_frame(&mut self) -> Option<(i16, i16)> {
        if self.samples.len() < 2 {
            return None;
        }

        Some((
            self.samples.pop_front().unwrap_or(0),
            self.samples.pop_front().unwrap_or(0),
        ))
    }

    fn try_prime(&mut self) {
        if self.primed || self.samples.len() < MIN_PRIME_FRAMES * 2 {
            return;
        }

        let Some((current_left, current_right)) = self.pop_stereo_frame() else {
            return;
        };
        let (next_left, next_right) = self
            .pop_stereo_frame()
            .unwrap_or((current_left, current_right));
        self.current_left = current_left;
        self.current_right = current_right;
        self.next_left = next_left;
        self.next_right = next_right;
        self.resample_phase = 0;
        self.primed = true;
        self.starved = false;
    }

    fn next_stereo_frame(&mut self, output_rate: u32) -> (i16, i16) {
        self.try_prime();

        if !self.primed {
            return (0, 0);
        }

        let denom = output_rate.max(1) as i64;
        let frac = self.resample_phase.min(output_rate) as i64;
        let inv_frac = denom - frac;
        let left =
            ((self.current_left as i64 * inv_frac) + (self.next_left as i64 * frac)) / denom;
        let right =
            ((self.current_right as i64 * inv_frac) + (self.next_right as i64 * frac)) / denom;

        self.resample_phase = self
            .resample_phase
            .saturating_add(self.source_rate.max(1));
        while self.resample_phase >= output_rate {
            self.resample_phase -= output_rate;
            self.current_left = self.next_left;
            self.current_right = self.next_right;

            let Some((next_left, next_right)) = self.pop_stereo_frame() else {
                // Hold the last sample and fade gently instead of snapping to zero,
                // which produces loud clicks when the producer briefly runs dry.
                self.next_left = self.current_left;
                self.next_right = self.current_right;
                self.starved = true;
                break;
            };
            self.next_left = next_left;
            self.next_right = next_right;
            self.starved = false;
        }

        if self.starved {
            self.current_left = ((self.current_left as i32 * 15) / 16) as i16;
            self.current_right = ((self.current_right as i32 * 15) / 16) as i16;
            self.next_left = self.current_left;
            self.next_right = self.current_right;
        }

        let vol = self.volume;
        let left_out = (left as f32 * vol) as i16;
        let right_out = (right as f32 * vol) as i16;
        (left_out, right_out)
    }
}

/// Handles audio output to the system audio device.
pub struct AudioHandler {
    _stream: Stream,
    shared_state: Arc<Mutex<SharedAudioState>>,
}

impl AudioHandler {
    /// Create a new audio handler, initializing the audio output stream.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No audio output device found")?;

        let config = device.default_output_config()?;
        let stream_config = StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let output_rate = stream_config.sample_rate.0;
        let channels = stream_config.channels as usize;

        let shared_state = Arc::new(Mutex::new(SharedAudioState::default()));
        let callback_state = Arc::clone(&shared_state);

        let stream = match config.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |data: &mut [f32], _| write_output_f32(data, channels, output_rate, &callback_state),
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _| write_output_i16(data, channels, output_rate, &callback_state),
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::U16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [u16], _| write_output_u16(data, channels, output_rate, &callback_state),
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            fmt => return Err(format!("Unsupported sample format: {:?}", fmt).into()),
        };

        stream.play()?;

        Ok(Self {
            _stream: stream,
            shared_state,
        })
    }

    /// Set playback volume in range [0.0, 1.0].
    pub fn set_volume(&self, volume: f32) {
        if let Ok(mut state) = self.shared_state.lock() {
            state.volume = volume.clamp(0.0, 1.0);
        }
    }

    /// Push interleaved stereo i16 samples into the playback buffer.
    pub fn push_samples(&self, samples: &[i16], source_rate: u32) {
        if let Ok(mut state) = self.shared_state.lock() {
            state.source_rate = source_rate.max(1);
            let max_samples = MAX_BUFFERED_FRAMES * 2;
            let incoming = samples.len();
            let overflow = state
                .samples
                .len()
                .saturating_add(incoming)
                .saturating_sub(max_samples);
            let overflow = (overflow + 1) & !1;
            if overflow > 0 {
                state.samples.drain(..overflow);
            }
            state.samples.extend(samples.iter().copied());
        }
    }
}

fn write_output_f32(
    data: &mut [f32],
    channels: usize,
    output_rate: u32,
    state: &Arc<Mutex<SharedAudioState>>,
) {
    let mut state = state.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let (left, right) = state.next_stereo_frame(output_rate);
        write_frame(frame, left, right, |sample| sample as f32 / 32768.0);
    }
}

fn write_output_i16(
    data: &mut [i16],
    channels: usize,
    output_rate: u32,
    state: &Arc<Mutex<SharedAudioState>>,
) {
    let mut state = state.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let (left, right) = state.next_stereo_frame(output_rate);
        write_frame(frame, left, right, |sample| sample);
    }
}

fn write_output_u16(
    data: &mut [u16],
    channels: usize,
    output_rate: u32,
    state: &Arc<Mutex<SharedAudioState>>,
) {
    let mut state = state.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        let (left, right) = state.next_stereo_frame(output_rate);
        write_frame(frame, left, right, |sample| (sample as i32 + 32768) as u16);
    }
}

fn write_frame<T, F>(frame: &mut [T], left: i16, right: i16, mut convert: F)
where
    F: FnMut(i16) -> T,
{
    match frame {
        [] => {}
        [mono] => {
            let mixed = ((left as i32 + right as i32) / 2) as i16;
            *mono = convert(mixed);
        }
        _ => {
            frame[0] = convert(left);
            frame[1] = convert(right);
            for (idx, sample) in frame.iter_mut().enumerate().skip(2) {
                let value = if idx % 2 == 0 { left } else { right };
                *sample = convert(value);
            }
        }
    }
}

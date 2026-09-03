//! Audio output handler using cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use tinybird_core::apu::SAMPLE_RATE as DEFAULT_GBA_SAMPLE_RATE;

// Keep the host queue intentionally small so video doesn't feel detached from
// audio if the emulator briefly runs ahead of real time.
const MAX_BUFFERED_MILLIS: u32 = 125;
const MIN_PRIME_MILLIS: u32 = 16;

fn frames_for_millis(sample_rate: u32, millis: u32) -> usize {
    let frames = (sample_rate.max(1) as u64 * millis as u64).div_ceil(1000);
    frames.max(1) as usize
}

fn lock_shared_state(state: &Mutex<SharedAudioState>) -> MutexGuard<'_, SharedAudioState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn try_lock_shared_state(
    state: &Arc<Mutex<SharedAudioState>>,
) -> Option<MutexGuard<'_, SharedAudioState>> {
    match state.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        // Brief silence is safer here than blocking the realtime callback.
        Err(TryLockError::WouldBlock) => None,
    }
}

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
        let min_prime_frames = frames_for_millis(self.source_rate, MIN_PRIME_MILLIS);
        if self.primed || self.samples.len() < min_prime_frames * 2 {
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
        let left = ((self.current_left as i64 * inv_frac) + (self.next_left as i64 * frac)) / denom;
        let right =
            ((self.current_right as i64 * inv_frac) + (self.next_right as i64 * frac)) / denom;

        self.resample_phase = self.resample_phase.saturating_add(self.source_rate.max(1));
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
                move |data: &mut [f32], _| {
                    write_output_f32(data, channels, output_rate, &callback_state)
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _| {
                    write_output_i16(data, channels, output_rate, &callback_state)
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::U16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [u16], _| {
                    write_output_u16(data, channels, output_rate, &callback_state)
                },
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
        let mut state = lock_shared_state(self.shared_state.as_ref());
        state.volume = volume.clamp(0.0, 1.0);
    }

    /// Return the approximate playback backlog in source milliseconds.
    pub fn buffered_millis(&self) -> u32 {
        let state = lock_shared_state(self.shared_state.as_ref());
        let frames = state.samples.len() / 2;
        ((frames as u64) * 1000 / state.source_rate.max(1) as u64) as u32
    }

    /// Drop any queued samples and reset the resampler state.
    pub fn clear(&self) {
        let mut state = lock_shared_state(self.shared_state.as_ref());
        state.samples.clear();
        state.current_left = 0;
        state.current_right = 0;
        state.next_left = 0;
        state.next_right = 0;
        state.resample_phase = 0;
        state.primed = false;
        state.starved = false;
    }

    /// Push interleaved stereo i16 samples into the playback buffer.
    pub fn push_samples(&self, samples: &[i16], source_rate: u32) {
        let mut state = lock_shared_state(self.shared_state.as_ref());
        state.source_rate = source_rate.max(1);
        let max_samples = (frames_for_millis(state.source_rate, MAX_BUFFERED_MILLIS) * 2).max(2);
        append_bounded(&mut state.samples, samples, max_samples);
    }
}

/// Append `samples` to `buffer`, dropping the oldest audio to stay within
/// `max_samples`.
///
/// Split out from `AudioHandler` so it can be tested without an audio device.
///
/// The size checks are not decoration: a batch larger than the whole buffer is
/// reachable in practice — loading a savestate that carries a backlog of
/// undrained audio hands the backend millions of samples at once — and the
/// previous arithmetic computed a drain range past the end of the buffer and
/// panicked.
fn append_bounded(buffer: &mut VecDeque<i16>, samples: &[i16], max_samples: usize) {
    if samples.len() >= max_samples {
        // Keep the newest audio, starting on an even index so the kept run
        // begins on a left sample, and trimming any trailing half-frame so the
        // buffer length stays even.
        let keep_from = samples.len() - max_samples;
        let keep_from = keep_from + (keep_from % 2);
        let mut tail = &samples[keep_from..];
        if !tail.len().is_multiple_of(2) {
            tail = &tail[..tail.len() - 1];
        }
        buffer.clear();
        buffer.extend(tail.iter().copied());
        return;
    }

    let overflow = buffer
        .len()
        .saturating_add(samples.len())
        .saturating_sub(max_samples);
    // Round up to a whole stereo frame, and never drain more than is there.
    let overflow = ((overflow + 1) & !1).min(buffer.len());
    if overflow > 0 {
        buffer.drain(..overflow);
    }
    buffer.extend(samples.iter().copied());
}

fn write_output_f32(
    data: &mut [f32],
    channels: usize,
    output_rate: u32,
    state: &Arc<Mutex<SharedAudioState>>,
) {
    let Some(mut state) = try_lock_shared_state(state) else {
        data.fill(0.0);
        return;
    };
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
    let Some(mut state) = try_lock_shared_state(state) else {
        data.fill(0);
        return;
    };
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
    let Some(mut state) = try_lock_shared_state(state) else {
        data.fill(32768);
        return;
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_of(values: &[i16]) -> VecDeque<i16> {
        values.iter().copied().collect()
    }

    fn contents(buffer: &VecDeque<i16>) -> Vec<i16> {
        buffer.iter().copied().collect()
    }

    #[test]
    fn a_batch_that_fits_is_appended_whole() {
        let mut buffer = buffer_of(&[1, 2]);
        append_bounded(&mut buffer, &[3, 4], 8);
        assert_eq!(contents(&buffer), vec![1, 2, 3, 4]);
    }

    #[test]
    fn the_oldest_audio_is_dropped_at_the_limit() {
        let mut buffer = buffer_of(&[1, 2, 3, 4]);
        append_bounded(&mut buffer, &[5, 6], 4);
        assert_eq!(contents(&buffer), vec![3, 4, 5, 6]);
    }

    #[test]
    fn an_oversized_batch_replaces_the_buffer_instead_of_panicking() {
        // This is the savestate case: the APU handed over far more audio than
        // the output buffer can hold. The previous code drained past the end.
        let mut buffer = buffer_of(&[1, 2]);
        let huge: Vec<i16> = (0..1000).collect();
        append_bounded(&mut buffer, &huge, 4);

        assert_eq!(buffer.len(), 4);
        assert_eq!(
            contents(&buffer),
            vec![996, 997, 998, 999],
            "keeps the newest audio"
        );
    }

    #[test]
    fn an_oversized_batch_stays_stereo_aligned() {
        // An odd keep-point would swap left and right for the whole batch.
        let mut buffer = VecDeque::new();
        // An odd-length batch is not something the APU produces, but the audio
        // path must not corrupt channel order if it ever sees one.
        let huge: Vec<i16> = (0..1001).collect();
        append_bounded(&mut buffer, &huge, 4);

        assert!(
            buffer.len().is_multiple_of(2),
            "an odd buffer swaps left and right for everything after it"
        );
        assert_eq!(buffer[0] % 2, 0, "batch must start on a left sample");
    }

    #[test]
    fn a_batch_exactly_the_limit_is_kept_entirely() {
        let mut buffer = buffer_of(&[9, 9]);
        append_bounded(&mut buffer, &[1, 2, 3, 4], 4);
        assert_eq!(contents(&buffer), vec![1, 2, 3, 4]);
    }

    #[test]
    fn an_empty_batch_leaves_the_buffer_alone() {
        let mut buffer = buffer_of(&[1, 2]);
        append_bounded(&mut buffer, &[], 8);
        assert_eq!(contents(&buffer), vec![1, 2]);
    }

    #[test]
    fn the_buffer_never_exceeds_the_limit_across_many_pushes() {
        let mut buffer = VecDeque::new();
        for round in 0..50i16 {
            append_bounded(&mut buffer, &[round, round], 10);
            assert!(buffer.len() <= 10, "overshot at round {round}");
        }
    }
}

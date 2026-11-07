//! Sample processing: bounded queues, format conversion, resampling, mixing
//! and voice activity detection. Pure logic, no devices.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{CHANNELS, FRAME_LEN, FRAME_SAMPLES, SAMPLE_RATE};

/// Samples shared between a device callback and the voice task. Bounded: when
/// full, the oldest samples are dropped (a short glitch instead of growing
/// latency or memory).
#[derive(Clone)]
pub struct SampleQueue {
    inner: Arc<Mutex<VecDeque<f32>>>,
    capacity: usize,
}

impl SampleQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn push(&self, samples: &[f32]) {
        let Ok(mut q) = self.inner.lock() else { return };
        // Only the newest `capacity` samples can be kept.
        let samples = &samples[samples.len().saturating_sub(self.capacity)..];
        let overflow = (q.len() + samples.len()).saturating_sub(self.capacity);
        let excess = overflow.min(q.len());
        q.drain(..excess);
        q.extend(samples);
    }

    /// Fills `out` with queued samples, then silence if the queue runs dry.
    /// Returns how many samples came from the queue.
    pub fn pop_into(&self, out: &mut [f32]) -> usize {
        let Ok(mut q) = self.inner.lock() else {
            out.fill(0.0);
            return 0;
        };
        let n = out.len().min(q.len());
        for (slot, sample) in out.iter_mut().zip(q.drain(..n)) {
            *slot = sample;
        }
        out[n..].fill(0.0);
        n
    }

    /// Takes everything queued.
    pub fn drain(&self) -> Vec<f32> {
        self.inner
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Streaming linear resampler for one channel. Good enough for voice and
/// cheap; the state carries over between chunks.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    pos: f64,
    prev: f32,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            step: from_rate.max(1) as f64 / to_rate.max(1) as f64,
            pos: 0.0,
            prev: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        // Position 0 is the last sample of the previous chunk (`prev`),
        // positions 1.. are this chunk.
        let prev = self.prev;
        let at = |i: usize| if i == 0 { prev } else { input[i - 1] };
        let mut pos = self.pos;
        while pos < input.len() as f64 {
            let i = pos.floor() as usize;
            let frac = (pos - i as f64) as f32;
            out.push(at(i) + (at(i + 1) - at(i)) * frac);
            pos += self.step;
        }
        self.pos = pos - input.len() as f64;
        self.prev = input[input.len() - 1];
    }
}

/// Microphone samples (device rate, any channel count) to 48 kHz mono.
pub struct CaptureConverter {
    channels: usize,
    resampler: Resampler,
    mono: Vec<f32>,
}

impl CaptureConverter {
    pub fn new(device_rate: u32, device_channels: usize) -> Self {
        Self {
            channels: device_channels.max(1),
            resampler: Resampler::new(device_rate, SAMPLE_RATE),
            mono: Vec::new(),
        }
    }

    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        self.mono.clear();
        self.mono.extend(
            interleaved
                .chunks_exact(self.channels)
                .map(|f| f.iter().sum::<f32>() / self.channels as f32),
        );
        self.resampler.process(&self.mono, out);
    }
}

/// 48 kHz stereo frames to the speakers' rate and channel count.
pub struct PlaybackConverter {
    channels: usize,
    left: Resampler,
    right: Resampler,
    l: Vec<f32>,
    r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

impl PlaybackConverter {
    pub fn new(device_rate: u32, device_channels: usize) -> Self {
        Self {
            channels: device_channels.max(1),
            left: Resampler::new(SAMPLE_RATE, device_rate),
            right: Resampler::new(SAMPLE_RATE, device_rate),
            l: Vec::new(),
            r: Vec::new(),
            out_l: Vec::new(),
            out_r: Vec::new(),
        }
    }

    pub fn process(&mut self, stereo: &[f32], out: &mut Vec<f32>) {
        self.l.clear();
        self.r.clear();
        for f in stereo.chunks_exact(CHANNELS) {
            self.l.push(f[0]);
            self.r.push(f[1]);
        }
        self.out_l.clear();
        self.out_r.clear();
        self.left.process(&self.l, &mut self.out_l);
        self.right.process(&self.r, &mut self.out_r);
        for (l, r) in self.out_l.iter().zip(&self.out_r) {
            match self.channels {
                1 => out.push((l + r) / 2.0),
                n => {
                    out.push(*l);
                    out.push(*r);
                    // Extra channels (surround) stay silent.
                    out.extend(std::iter::repeat_n(0.0, n - 2));
                }
            }
        }
    }
}

/// Mono to interleaved stereo.
pub fn mono_to_stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|s| [*s, *s]).collect()
}

/// Frames waiting to be played for one speaker.
struct Source {
    frames: VecDeque<Vec<f32>>,
    /// Playback starts (or restarts after running dry) once a couple of
    /// frames are queued, to absorb network jitter.
    playing: bool,
    last_push: Instant,
}

/// Frames buffered per speaker before playback starts (40 ms).
const PREBUFFER: usize = 2;
/// Most frames kept per speaker (200 ms): beyond this the oldest are dropped.
const MAX_QUEUED: usize = 10;
/// Most speakers mixed at once.
const MAX_SOURCES: usize = 64;
const SOURCE_IDLE: Duration = Duration::from_secs(5);

/// Mixes the decoded audio of all speakers, 20 ms at a time.
#[derive(Default)]
pub struct Mixer {
    sources: HashMap<u32, Source>,
}

impl Mixer {
    /// Adds a decoded frame for a speaker (by SSRC). Frames that are not
    /// exactly 20 ms of stereo audio are ignored.
    pub fn push(&mut self, ssrc: u32, frame: Vec<f32>) {
        if frame.len() != FRAME_LEN {
            return;
        }
        if !self.sources.contains_key(&ssrc) && self.sources.len() >= MAX_SOURCES {
            return;
        }
        let source = self.sources.entry(ssrc).or_insert_with(|| Source {
            frames: VecDeque::new(),
            playing: false,
            last_push: Instant::now(),
        });
        if source.frames.len() >= MAX_QUEUED {
            source.frames.pop_front();
        }
        source.frames.push_back(frame);
        source.last_push = Instant::now();
    }

    pub fn remove(&mut self, ssrc: u32) {
        self.sources.remove(&ssrc);
    }

    /// The next 20 ms of mixed audio, or `None` if nobody is playing.
    pub fn mix(&mut self) -> Option<Vec<f32>> {
        let now = Instant::now();
        self.sources
            .retain(|_, s| !s.frames.is_empty() || now.duration_since(s.last_push) < SOURCE_IDLE);
        let mut out: Option<Vec<f32>> = None;
        for source in self.sources.values_mut() {
            if !source.playing && source.frames.len() >= PREBUFFER {
                source.playing = true;
            }
            if !source.playing {
                continue;
            }
            let Some(frame) = source.frames.pop_front() else {
                source.playing = false;
                continue;
            };
            let mix = out.get_or_insert_with(|| vec![0.0; FRAME_LEN]);
            for (m, s) in mix.iter_mut().zip(&frame) {
                *m += s;
            }
        }
        if let Some(mix) = &mut out {
            for s in mix.iter_mut() {
                *s = s.clamp(-1.0, 1.0);
            }
        }
        out
    }
}

/// Simple voice activity detection on 20 ms frames: level above a threshold,
/// held for a short while so words are not cut between syllables.
pub struct VoiceActivity {
    threshold: f32,
    hangover: u32,
    remaining: u32,
}

impl VoiceActivity {
    /// `threshold` is an RMS level (0..1); `hangover` in frames.
    pub fn new(threshold: f32, hangover: u32) -> Self {
        Self {
            threshold,
            hangover,
            remaining: 0,
        }
    }

    pub fn update(&mut self, frame: &[f32]) -> bool {
        if rms(frame) >= self.threshold {
            self.remaining = self.hangover;
            true
        } else if self.remaining > 0 {
            self.remaining -= 1;
            true
        } else {
            false
        }
    }
}

pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Synthetic "voice" for the demo: a soft hum with a syllable rhythm, so the
/// fake participants are audible without any recording.
pub struct Hum {
    freq: f32,
    t: u64,
}

impl Hum {
    pub fn new(freq: f32) -> Self {
        Self { freq, t: 0 }
    }

    /// The next 20 ms, interleaved stereo.
    pub fn next_frame(&mut self) -> Vec<f32> {
        let mut out = Vec::with_capacity(FRAME_LEN);
        for _ in 0..FRAME_SAMPLES {
            let t = self.t as f32 / SAMPLE_RATE as f32;
            self.t += 1;
            // Pitch glides a little and the level follows ~4 syllables/s.
            let pitch = self.freq * (1.0 + 0.05 * (t * 1.3 * std::f32::consts::TAU).sin());
            let envelope = (t * 4.0 * std::f32::consts::PI).sin().abs().powf(1.5);
            let s = 0.12 * envelope * (t * pitch * std::f32::consts::TAU).sin();
            out.push(s);
            out.push(s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_bounded_and_fills_with_silence() {
        let q = SampleQueue::new(4);
        q.push(&[1.0, 2.0, 3.0]);
        q.push(&[4.0, 5.0, 6.0]);
        assert_eq!(q.len(), 4);
        let mut out = [9.0; 6];
        assert_eq!(q.pop_into(&mut out), 4);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0, 0.0, 0.0]);
        q.push(&[1.0; 10]);
        assert_eq!(q.drain().len(), 4);
        assert!(q.is_empty());
    }

    #[test]
    fn resampling_keeps_duration() {
        for (from, to) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (48_000, 48_000),
            (16_000, 48_000),
        ] {
            let mut r = Resampler::new(from, to);
            let mut out = Vec::new();
            // One second in irregular chunks.
            let input: Vec<f32> = (0..from).map(|i| (i as f32 * 0.01).sin()).collect();
            for chunk in input.chunks(441) {
                r.process(chunk, &mut out);
            }
            let diff = (out.len() as i64 - to as i64).abs();
            assert!(diff <= 2, "{from}->{to}: {} samples", out.len());
        }
    }

    #[test]
    fn channel_conversion() {
        let mut capture = CaptureConverter::new(48_000, 2);
        let mut mono = Vec::new();
        capture.process(&[0.2, 0.4, 0.6, 0.8], &mut mono);
        assert_eq!(mono.len(), 2);
        assert!((mono[1] - 0.3).abs() < 1e-6);

        let mut playback = PlaybackConverter::new(48_000, 6);
        let mut out = Vec::new();
        playback.process(&[0.5, -0.5, 0.5, -0.5], &mut out);
        assert_eq!(out.len(), 12);
        assert_eq!(&out[6..12], &[0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(mono_to_stereo(&[0.1, 0.2]), vec![0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn mixer_prebuffers_mixes_and_clamps() {
        let mut m = Mixer::default();
        m.push(1, vec![0.7; FRAME_LEN]);
        // One frame is not enough to start.
        assert!(m.mix().is_none());
        m.push(1, vec![0.7; FRAME_LEN]);
        m.push(2, vec![0.7; FRAME_LEN]);
        m.push(2, vec![0.7; FRAME_LEN]);
        let out = m.mix().unwrap();
        assert!(out.iter().all(|s| *s == 1.0), "clamped");
        // Wrong frame sizes are ignored.
        m.push(3, vec![0.1; 10]);
        assert!(!m.sources.contains_key(&3));
        // Too many queued frames: only the newest are kept.
        for _ in 0..50 {
            m.push(4, vec![0.0; FRAME_LEN]);
        }
        assert_eq!(m.sources[&4].frames.len(), MAX_QUEUED);
    }

    #[test]
    fn voice_activity_with_hangover() {
        let mut vad = VoiceActivity::new(0.02, 2);
        let loud = vec![0.1; FRAME_LEN];
        let quiet = vec![0.001; FRAME_LEN];
        assert!(!vad.update(&quiet));
        assert!(vad.update(&loud));
        assert!(vad.update(&quiet));
        assert!(vad.update(&quiet));
        assert!(!vad.update(&quiet));
    }

    #[test]
    fn hum_is_audible_but_soft() {
        let mut hum = Hum::new(200.0);
        let level = rms(&(0..10).flat_map(|_| hum.next_frame()).collect::<Vec<_>>());
        assert!(level > 0.01 && level < 0.2, "{level}");
    }
}

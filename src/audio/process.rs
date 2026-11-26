//! Microphone clean-up before sending: echo, noise and level.
//!
//! Echo cancellation, automatic gain and the "Standard" noise suppression are
//! `sonora`, a pure-Rust port of WebRTC's audio processing (the one browsers
//! and Discord's own client build on). Two stronger noise suppressors can be
//! chosen instead of the standard one: `RNNoise` (`nnnoiseless`, a small
//! neural network) and `DPDFNet` (a larger one, its model downloaded once;
//! see `dpdfnet.rs`). They run after the echo canceller.
//!
//! Everything is 48 kHz mono, 10 ms at a time. The echo canceller needs to
//! hear what the speakers play: [`Processor::render`] gets the mix right
//! before it is queued for the speakers, [`Processor::capture`] cleans the
//! microphone frames before voice detection and encoding.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use serde::{Deserialize, Serialize};
use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::dpdfnet::{self, DpdfNet};
use super::{CHANNELS, SAMPLE_RATE};

/// Samples in one 10 ms chunk at 48 kHz (what the processing works on).
pub const CHUNK: usize = SAMPLE_RATE as usize / 100;
// DPDFNet takes exactly one chunk per step.
const _: () = assert!(CHUNK == dpdfnet::HOP);

/// Above this level the soft limiter bends the peaks instead of cutting them.
const KNEE: f32 = 0.8;

/// Peaks above [`KNEE`] are rounded off towards 1.0 instead of cut flat
/// (cutting sounds harsh, "robotic"); below it nothing changes.
pub fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let room = 1.0 - KNEE;
    (KNEE + room * ((a - KNEE) / room).tanh()).copysign(x)
}

/// Which noise suppression cleans the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Noise {
    Off,
    /// WebRTC's: steady noise (fan, hiss), almost no CPU.
    #[default]
    Standard,
    /// `RNNoise`: also keyboard and sudden noise, light.
    RnNoise,
    /// `DPDFNet` (`DeepFilterNet2` with dual-path blocks), its 16 kHz model:
    /// the strongest, no sound above 8 kHz in the cleaned voice; downloaded
    /// once when chosen. Saved as `dpdf_net_light`, its name when the 48 kHz
    /// model (`dpdf_net`, removed in 0.20.0) was there too.
    #[serde(rename = "dpdf_net_light")]
    DpdfNet,
}

impl Noise {
    pub const ALL: [Self; 4] = [Self::Off, Self::Standard, Self::RnNoise, Self::DpdfNet];

    /// It needs the `DPDFNet` model.
    pub const fn is_dpdfnet(self) -> bool {
        matches!(self, Self::DpdfNet)
    }
}

/// Read tolerantly: the old switch (`true`/`false`) means Standard/Off; a
/// missing or unknown value means Standard, never a weaker one by surprise.
impl<'de> Deserialize<'de> for Noise {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match serde_json::Value::deserialize(d)? {
            serde_json::Value::Bool(false) => Self::Off,
            serde_json::Value::String(s) => match s.as_str() {
                "off" => Self::Off,
                "rn_noise" => Self::RnNoise,
                // DeepFilterNet was removed in 0.19.0: the nearest choice
                // that needs no download.
                "deep_filter" => Self::RnNoise,
                // The 48 kHz model was removed in 0.20.0: the 16 kHz one.
                "dpdf_net" | "dpdf_net_light" => Self::DpdfNet,
                _ => Self::Standard,
            },
            _ => Self::Standard,
        })
    }
}

/// What is turned on in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Choices {
    pub echo: bool,
    pub noise: Noise,
    pub gain: bool,
    /// `DPDFNet` stays on even when the computer cannot keep up with it
    /// (otherwise `RNNoise` takes over for the rest of the call).
    pub keep_model: bool,
}

impl Choices {
    pub fn any(self) -> bool {
        self.echo || self.noise != Noise::Off || self.gain
    }
}

/// Microphone frames in one check of `DPDFNet`'s speed (2 s).
const SLOW_WINDOW: u32 = 100;
/// Too slow: on average more than this per 20 ms frame (60 % of the time
/// there is). On the owner's PC it takes 4.5 ms; with the computer busy it
/// went to 17–19 ms, and frames were dropped.
const SLOW_FRAME_US: u64 = 12_000;
/// Checks in a row that must be too slow before switching (6 s): a short
/// spike (a program starting) does not switch.
const SLOW_WINDOWS: u32 = 3;

/// Watches how long `DPDFNet` takes, frame by frame.
#[derive(Default)]
struct SpeedWatch {
    frames: u32,
    total_us: u64,
    slow_windows: u32,
}

impl SpeedWatch {
    /// One frame took `us`; `true` once it has been too slow for long
    /// enough.
    fn frame(&mut self, us: u64) -> bool {
        self.frames += 1;
        self.total_us = self.total_us.saturating_add(us);
        if self.frames < SLOW_WINDOW {
            return false;
        }
        let average = self.total_us / u64::from(self.frames);
        self.frames = 0;
        self.total_us = 0;
        if average > SLOW_FRAME_US {
            self.slow_windows += 1;
            tracing::debug!("noise suppression: DPDFNet slow, {average} µs a frame");
        } else {
            self.slow_windows = 0;
        }
        self.slow_windows >= SLOW_WINDOWS
    }
}

/// The `DPDFNet` model, read (and checked) and prepared on its own thread.
enum Dpdf {
    Loading(Receiver<Result<Box<DpdfNet>, String>>),
    Ready(Box<DpdfNet>),
}

/// The neural noise suppressor chosen, if any.
enum Neural {
    None,
    RnNoise(Box<nnnoiseless::DenoiseState<'static>>),
    Dpdf(Dpdf),
}

/// A downloaded `DPDFNet` model, checked, ready to run.
fn load_dpdfnet() -> Result<Box<DpdfNet>, String> {
    let bytes = dpdfnet::read().map_err(|e| format!("{e:#}"))?;
    std::panic::catch_unwind(|| DpdfNet::new(&bytes).map_err(|e| format!("{e:#}")))
        .map_err(|_| "the model could not be loaded".to_string())?
        .map(Box::new)
}

/// Runs `load` on a thread of its own; its result arrives on the receiver.
fn load_in_background<T: Send + 'static>(
    name: &str,
    load: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name(name.into()).spawn({
        let tx = tx.clone();
        move || {
            let _ = tx.send(load());
        }
    });
    if let Err(e) = spawned {
        let _ = tx.send(Err(e.to_string()));
    }
    rx
}

/// The sonora configuration for these choices: WebRTC's own noise
/// suppression only for Standard (or as the fallback).
fn config(choices: Choices, standard_noise: bool) -> Config {
    Config {
        high_pass_filter: Some(HighPassFilter::default()),
        echo_canceller: choices.echo.then(EchoCanceller::default),
        noise_suppression: standard_noise.then(|| NoiseSuppression {
            level: NoiseSuppressionLevel::High,
            ..NoiseSuppression::default()
        }),
        gain_controller2: choices.gain.then(|| GainController2 {
            adaptive_digital: Some(AdaptiveDigital::default()),
            ..GainController2::default()
        }),
        ..Config::default()
    }
}

/// The processing of one call. Its state (echo model, noise estimate, gain)
/// adapts over a few seconds, so it lives as long as the microphone is on.
pub struct Processor {
    apm: AudioProcessing,
    choices: Choices,
    neural: Neural,
    /// Why the chosen suppressor could not start (taken once, to tell the
    /// user); Standard runs instead.
    failure: Option<String>,
    /// How fast `DPDFNet` runs here.
    speed: SpeedWatch,
    /// `DPDFNet` was too slow and `RNNoise` took over (taken once, to tell
    /// the user).
    switched: bool,
    /// Delay between rendering and capturing, for the echo canceller (ms).
    delay_ms: i32,
    input: [f32; CHUNK],
    output: [f32; CHUNK],
    /// Mono render samples waiting for a full chunk.
    render: Vec<f32>,
}

impl Processor {
    /// `delay_ms`: rough time between a sample being handed to the speakers
    /// and its echo reaching [`Processor::capture`]; the echo canceller
    /// refines it on its own.
    pub fn new(choices: Choices, delay_ms: i32) -> Self {
        let mono = StreamConfig::new(SAMPLE_RATE, 1);
        // DPDFNet's model takes a moment to load: Standard cleans until then
        // (never no filter).
        let standard = matches!(choices.noise, Noise::Standard) || choices.noise.is_dpdfnet();
        let apm = AudioProcessing::builder()
            .config(config(choices, standard))
            .capture_config(mono)
            .render_config(mono)
            .build();
        let neural = match choices.noise {
            Noise::RnNoise => Neural::RnNoise(nnnoiseless::DenoiseState::new()),
            Noise::DpdfNet => Neural::Dpdf(Dpdf::Loading(load_in_background("dpdfnet-load", load_dpdfnet))),
            Noise::Off | Noise::Standard => Neural::None,
        };
        Self {
            apm,
            choices,
            neural,
            failure: None,
            speed: SpeedWatch::default(),
            switched: false,
            delay_ms: delay_ms.clamp(0, 500),
            input: [0.0; CHUNK],
            output: [0.0; CHUNK],
            render: Vec::with_capacity(CHUNK * 4),
        }
    }

    pub fn choices(&self) -> Choices {
        self.choices
    }

    /// Why the chosen noise suppression could not start, once: Standard
    /// runs instead.
    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// Whether `DPDFNet` was too slow and `RNNoise` took over, once.
    pub fn take_switch(&mut self) -> bool {
        std::mem::take(&mut self.switched)
    }

    /// `DPDFNet` cannot keep up: `RNNoise` for the rest of the call, which
    /// needs almost no CPU and still removes sudden noise.
    fn switch_to_rnnoise(&mut self) {
        tracing::warn!("noise suppression: DPDFNet too slow on this computer, RNNoise instead");
        self.neural = Neural::RnNoise(nnnoiseless::DenoiseState::new());
        self.apm.apply_config(config(self.choices, false));
        self.switched = true;
    }

    /// The suppressor failed: WebRTC's Standard instead, never no filter.
    fn fall_back(&mut self, why: String) {
        tracing::warn!("noise suppression: falling back to Standard: {why}");
        self.neural = Neural::None;
        self.apm.apply_config(config(self.choices, true));
        self.failure = Some(why);
    }

    /// The `DPDFNet` model, once loaded.
    fn poll_model(&mut self) {
        let Neural::Dpdf(Dpdf::Loading(rx)) = &self.neural else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(model)) => {
                tracing::info!("noise suppression: DPDFNet ready");
                self.neural = Neural::Dpdf(Dpdf::Ready(model));
                // The model takes over from Standard.
                self.apm.apply_config(config(self.choices, false));
            }
            Ok(Err(e)) => self.fall_back(e),
            Err(TryRecvError::Disconnected) => self.fall_back("the model loader stopped".into()),
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Cleans microphone audio (48 kHz mono) in place. Its length must be a
    /// multiple of [`CHUNK`] (a 20 ms frame is two chunks); a rest is left
    /// as it is.
    pub fn capture(&mut self, frame: &mut [f32]) {
        self.poll_model();
        let start = matches!(self.neural, Neural::Dpdf(Dpdf::Ready(_))).then(std::time::Instant::now);
        for chunk in frame.as_chunks_mut::<CHUNK>().0 {
            self.input.copy_from_slice(chunk);
            if self.choices.echo {
                // Must be given for every chunk while the echo canceller runs.
                let _ = self.apm.set_stream_delay_ms(self.delay_ms);
            }
            let ok = self
                .apm
                .process_capture_f32(&[&self.input], &mut [&mut self.output])
                .is_ok();
            if ok {
                self.input.copy_from_slice(&self.output);
            }
            let failed = match &mut self.neural {
                Neural::None => None,
                Neural::RnNoise(state) => {
                    // RNNoise works in the 16-bit range.
                    for s in &mut self.input {
                        *s *= 32767.0;
                    }
                    state.process_frame(&mut self.output, &self.input);
                    for s in &mut self.output {
                        *s /= 32767.0;
                    }
                    self.input.copy_from_slice(&self.output);
                    None
                }
                Neural::Dpdf(Dpdf::Ready(model)) => match model.process(&mut self.input) {
                    Ok(()) => None,
                    Err(e) => Some(format!("{e:#}")),
                },
                // Until the model is loaded (a fraction of a second) the
                // echo canceller and gain still run.
                Neural::Dpdf(Dpdf::Loading(_)) => None,
            };
            if let Some(why) = failed {
                self.fall_back(why);
            }
            for (s, o) in chunk.iter_mut().zip(self.input.iter()) {
                *s = if o.is_finite() { soft_limit(*o) } else { 0.0 };
            }
        }
        // Still DPDFNet (it did not fail during this frame).
        if let Some(start) = start.filter(|_| matches!(self.neural, Neural::Dpdf(Dpdf::Ready(_)))) {
            // Per 20 ms of audio, whatever the frame's length.
            let chunks = (frame.len() / CHUNK).max(1) as u64;
            let us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
            if self.slow_frame(us.saturating_mul(2) / chunks) {
                self.switch_to_rnnoise();
            }
        }
    }

    /// A 20 ms frame of `DPDFNet` took `us`: `true` if it is time to switch
    /// (never with [`Choices::keep_model`]).
    fn slow_frame(&mut self, us: u64) -> bool {
        self.speed.frame(us) && !self.choices.keep_model
    }

    /// What the speakers are about to play (48 kHz interleaved stereo): the
    /// reference the echo canceller removes from the microphone. Bounded: at
    /// most a few chunks wait for a full one.
    pub fn render(&mut self, stereo: &[f32]) {
        if !self.choices.echo {
            return;
        }
        self.render.extend(
            stereo
                .as_chunks::<CHANNELS>()
                .0
                .iter()
                .map(|f| f.iter().sum::<f32>() / CHANNELS as f32),
        );
        let mut done = 0;
        while self.render.len() - done >= CHUNK {
            self.input.copy_from_slice(&self.render[done..done + CHUNK]);
            let _ = self
                .apm
                .process_render_f32(&[&self.input], &mut [&mut self.output]);
            done += CHUNK;
        }
        self.render.drain(..done);
        let excess = self.render.len().saturating_sub(CHUNK * 4);
        self.render.drain(..excess);
    }
}

/// Work for the processing thread, in order (the echo reference and the
/// microphone stay in step).
enum Job {
    Capture(Vec<f32>),
    Render(Vec<f32>),
}

/// How long the processing takes, for the log.
#[derive(Default)]
pub struct Timings {
    /// Microphone frames processed, and their total and longest time (µs).
    frames: std::sync::atomic::AtomicU64,
    total_us: std::sync::atomic::AtomicU64,
    max_us: std::sync::atomic::AtomicU64,
}

impl Timings {
    /// (frames, average ms, longest ms) since the last call.
    pub fn take(&self) -> (u64, f32, f32) {
        use std::sync::atomic::Ordering::Relaxed;
        let frames = self.frames.swap(0, Relaxed);
        let total = self.total_us.swap(0, Relaxed);
        let max = self.max_us.swap(0, Relaxed);
        let avg = if frames == 0 {
            0.0
        } else {
            total as f32 / frames as f32 / 1000.0
        };
        (frames, avg, max as f32 / 1000.0)
    }
}

/// Frames waiting for the processing thread, at most (200 ms): beyond that
/// it cannot keep up and new frames are dropped rather than piling delay.
const MAX_QUEUED: usize = 10;

/// The microphone clean-up on a thread of its own, so the voice
/// connection's timer and the other connections (a Go Live stream) are not
/// slowed by it, and it is not slowed by them.
pub struct Worker {
    jobs: std::sync::mpsc::SyncSender<Job>,
    done: tokio::sync::mpsc::Receiver<Vec<f32>>,
    failure: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// `DPDFNet` was too slow and `RNNoise` took over.
    switched: std::sync::Arc<std::sync::atomic::AtomicBool>,
    timings: std::sync::Arc<Timings>,
    choices: Choices,
    /// Frames handed over and not back yet.
    pending: usize,
}

impl Worker {
    /// Starts the thread; `None` if the system refuses one (the microphone
    /// then goes unprocessed, and the caller says so).
    pub fn start(choices: Choices, delay_ms: i32) -> Option<Self> {
        let (jobs, rx) = std::sync::mpsc::sync_channel::<Job>(MAX_QUEUED * 3);
        let (done_tx, done) = tokio::sync::mpsc::channel(MAX_QUEUED * 2);
        let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
        let timings = std::sync::Arc::new(Timings::default());
        let switched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (fail, times, switch) = (failure.clone(), timings.clone(), switched.clone());
        std::thread::Builder::new()
            .name("mic-processing".into())
            .spawn(move || {
                use std::sync::atomic::Ordering::Relaxed;
                let mut p = Processor::new(choices, delay_ms);
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Render(stereo) => p.render(&stereo),
                        Job::Capture(mut frame) => {
                            let start = std::time::Instant::now();
                            p.capture(&mut frame);
                            let us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
                            times.frames.fetch_add(1, Relaxed);
                            times.total_us.fetch_add(us, Relaxed);
                            times.max_us.fetch_max(us, Relaxed);
                            if let Some(why) = p.take_failure() {
                                if let Ok(mut f) = fail.lock() {
                                    *f = Some(why);
                                }
                            }
                            if p.take_switch() {
                                switch.store(true, Relaxed);
                            }
                            if done_tx.blocking_send(frame).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .ok()?;
        Some(Self {
            jobs,
            done,
            failure,
            switched,
            timings,
            choices,
            pending: 0,
        })
    }

    pub fn choices(&self) -> Choices {
        self.choices
    }

    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    /// Why the chosen noise suppression could not start, once.
    pub fn take_failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|mut f| f.take())
    }

    /// Whether `DPDFNet` was too slow and `RNNoise` took over, once.
    pub fn take_switch(&self) -> bool {
        self.switched.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// The speakers' mix, as the echo reference; dropped if the thread is
    /// behind (the echo canceller copes with a missing piece).
    pub fn render(&self, stereo: &[f32]) {
        if self.choices.echo {
            let _ = self.jobs.try_send(Job::Render(stereo.to_vec()));
        }
    }

    /// Hands over a microphone frame; `false` if the thread is too far
    /// behind and the frame was dropped.
    pub fn capture(&mut self, frame: Vec<f32>) -> bool {
        if self.pending >= MAX_QUEUED {
            return false;
        }
        match self.jobs.try_send(Job::Capture(frame)) {
            Ok(()) => {
                self.pending += 1;
                true
            }
            Err(_) => false,
        }
    }

    /// The processed frames: waits up to `wait` for the ones handed over
    /// (processing takes about a millisecond), and takes any that came
    /// back late. In order.
    pub async fn processed(&mut self, wait: std::time::Duration) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        if wait.is_zero() {
            // No waiting at all: what is ready now.
            while self.pending > 0 {
                match self.done.try_recv() {
                    Ok(frame) => {
                        self.pending -= 1;
                        out.push(frame);
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.pending = 0;
                        break;
                    }
                }
            }
            return out;
        }
        let deadline = tokio::time::Instant::now() + wait;
        while self.pending > 0 {
            match tokio::time::timeout_at(deadline, self.done.recv()).await {
                Ok(Some(frame)) => {
                    self.pending -= 1;
                    out.push(frame);
                }
                // Late: they come with the next tick.
                Err(_) => break,
                // The thread is gone.
                Ok(None) => {
                    self.pending = 0;
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::rms;
    use crate::audio::FRAME_SAMPLES;

    /// A 440 Hz tone, `n` samples from sample `from`.
    fn tone(from: usize, n: usize, level: f32) -> Vec<f32> {
        (from..from + n)
            .map(|i| level * (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin())
            .collect()
    }

    /// Repeatable white noise.
    fn noise(n: usize, level: f32, seed: &mut u32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 17;
                *seed ^= *seed << 5;
                level * ((*seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn noise_is_turned_down() {
        let mut p = Processor::new(
            Choices {
                noise: Noise::Standard,
                ..Choices::default()
            },
            0,
        );
        let mut seed = 7;
        let mut last = Vec::new();
        // Two seconds: the noise estimate settles.
        for _ in 0..100 {
            let mut frame = noise(FRAME_SAMPLES, 0.05, &mut seed);
            p.capture(&mut frame);
            last = frame;
        }
        let before = rms(&noise(FRAME_SAMPLES, 0.05, &mut seed));
        assert!(rms(&last) < before * 0.5, "{} vs {}", rms(&last), before);
    }

    #[test]
    fn the_echo_of_the_speakers_is_removed() {
        let mut p = Processor::new(
            Choices {
                echo: true,
                ..Choices::default()
            },
            20,
        );
        let mut level = 1.0;
        // Four seconds of the speakers' tone coming back in the microphone,
        // 20 ms later and quieter.
        for k in 0..200 {
            let played = tone(k * FRAME_SAMPLES, FRAME_SAMPLES, 0.5);
            let stereo: Vec<f32> = played.iter().flat_map(|s| [*s, *s]).collect();
            p.render(&stereo);
            let mut mic = if k == 0 {
                vec![0.0; FRAME_SAMPLES]
            } else {
                tone((k - 1) * FRAME_SAMPLES, FRAME_SAMPLES, 0.25)
            };
            p.capture(&mut mic);
            level = rms(&mic);
        }
        assert!(level < 0.25 * 0.707 * 0.5, "echo left: {level}");
    }

    /// Noise in, cleaned out for `seconds`; the level of the last frame
    /// against the noise's.
    fn noise_after(p: &mut Processor, seconds: usize) -> (f32, f32) {
        let mut seed = 11;
        let mut last = Vec::new();
        for _ in 0..seconds * 50 {
            let mut frame = noise(FRAME_SAMPLES, 0.05, &mut seed);
            p.capture(&mut frame);
            assert!(frame.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            last = frame;
        }
        (rms(&last), rms(&noise(FRAME_SAMPLES, 0.05, &mut seed)))
    }

    #[test]
    fn rnnoise_turns_noise_down() {
        let mut p = Processor::new(
            Choices {
                noise: Noise::RnNoise,
                ..Choices::default()
            },
            0,
        );
        let (after, before) = noise_after(&mut p, 2);
        assert!(after < before * 0.5, "{after} vs {before}");
        assert_eq!(p.take_failure(), None);
    }

    #[test]
    fn a_failing_suppressor_falls_back_to_standard() {
        let mut p = Processor::new(
            Choices {
                noise: Noise::DpdfNet,
                ..Choices::default()
            },
            0,
        );
        let (tx, rx) = mpsc::channel();
        tx.send(Err("broken".to_string())).unwrap();
        p.neural = Neural::Dpdf(Dpdf::Loading(rx));
        let mut frame = vec![0.0; FRAME_SAMPLES];
        p.capture(&mut frame);
        assert!(matches!(p.neural, Neural::None));
        assert_eq!(p.take_failure().as_deref(), Some("broken"));
        assert_eq!(p.take_failure(), None, "told once");
        assert!(
            p.apm.config().noise_suppression.is_some(),
            "Standard runs instead"
        );
    }

    #[test]
    fn only_a_lasting_slowness_counts() {
        let mut w = SpeedWatch::default();
        // Fast: never.
        assert!(!(0..1000).any(|_| w.frame(4_500)));
        // A spike of 2 s, then fast again: no.
        assert!(!(0..SLOW_WINDOW).any(|_| w.frame(18_000)));
        assert!(!(0..SLOW_WINDOW).any(|_| w.frame(4_000)));
        assert_eq!(w.slow_windows, 0);
        // Slow for 6 s in a row: yes, at the end of the third check.
        let n = SLOW_WINDOW * SLOW_WINDOWS;
        let switch_at = (1..=n).find(|_| w.frame(18_000));
        assert_eq!(switch_at, Some(n));
        // Absurd times do not overflow.
        let mut w = SpeedWatch::default();
        assert!(!w.frame(u64::MAX));
        assert!(!w.frame(u64::MAX));
    }

    #[test]
    fn too_slow_switches_to_rnnoise_unless_kept() {
        let slow = |keep_model: bool| {
            let mut p = Processor::new(
                Choices {
                    noise: Noise::DpdfNet,
                    keep_model,
                    ..Choices::default()
                },
                0,
            );
            (0..SLOW_WINDOW * SLOW_WINDOWS).any(|_| p.slow_frame(18_000))
        };
        assert!(slow(false));
        assert!(!slow(true));

        let mut p = Processor::new(
            Choices {
                noise: Noise::DpdfNet,
                ..Choices::default()
            },
            0,
        );
        p.switch_to_rnnoise();
        assert!(matches!(p.neural, Neural::RnNoise(_)));
        assert!(p.take_switch());
        assert!(!p.take_switch(), "told once");
        // RNNoise keeps cleaning, and no failure is reported.
        let (after, before) = noise_after(&mut p, 2);
        assert!(after < before * 0.5, "{after} vs {before}");
        assert_eq!(p.take_failure(), None);
    }

    #[test]
    fn digital_silence_stays_silent() {
        for noise in Noise::ALL {
            let mut p = Processor::new(
                Choices {
                    noise,
                    ..Choices::default()
                },
                0,
            );
            for _ in 0..20 {
                let mut frame = vec![0.0; FRAME_SAMPLES];
                p.capture(&mut frame);
                assert!(frame.iter().all(|s| s.abs() < 1e-3), "{noise:?}");
            }
        }
    }

    #[test]
    fn the_old_switch_and_unknown_values_are_read() {
        let read = |json: &str| serde_json::from_str::<Noise>(json).unwrap();
        assert_eq!(read("true"), Noise::Standard);
        assert_eq!(read("false"), Noise::Off);
        assert_eq!(read(r#""rn_noise""#), Noise::RnNoise);
        assert_eq!(read(r#""deep_filter""#), Noise::RnNoise, "removed in 0.19.0");
        assert_eq!(
            read(r#""dpdf_net""#),
            Noise::DpdfNet,
            "48 kHz model removed in 0.20.0"
        );
        assert_eq!(read(r#""dpdf_net_light""#), Noise::DpdfNet);
        assert_eq!(
            serde_json::to_string(&Noise::DpdfNet).unwrap(),
            r#""dpdf_net_light""#
        );
        assert_eq!(read(r#""off""#), Noise::Off);
        assert_eq!(read(r#""krisp""#), Noise::Standard);
        assert_eq!(read("3"), Noise::Standard);
        for n in Noise::ALL {
            assert_eq!(read(&serde_json::to_string(&n).unwrap()), n);
        }
    }

    #[test]
    fn peaks_are_rounded_not_cut() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.8), -0.8);
        for x in [0.9_f32, 1.0, 1.5, 10.0] {
            let y = soft_limit(x);
            assert!(y > KNEE && y <= 1.0, "{x} -> {y}");
            assert_eq!(soft_limit(-x), -y);
        }
        assert!(soft_limit(0.9) < soft_limit(1.0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_worker_processes_in_order_on_its_own_thread() {
        let mut w = Worker::start(
            Choices {
                noise: Noise::RnNoise,
                echo: true,
                ..Choices::default()
            },
            20,
        )
        .unwrap();
        let mut seed = 3;
        let mut got = 0;
        for _ in 0..50 {
            w.render(&vec![0.0; FRAME_SAMPLES * CHANNELS]);
            assert!(w.capture(noise(FRAME_SAMPLES, 0.05, &mut seed)));
            for f in w.processed(std::time::Duration::from_millis(200)).await {
                assert_eq!(f.len(), FRAME_SAMPLES);
                got += 1;
            }
        }
        assert_eq!(got, 50);
        let (frames, avg, max) = w.timings().take();
        assert_eq!(frames, 50);
        assert!(avg > 0.0 && max >= avg);
        assert_eq!(w.timings().take().0, 0, "taken");
        // Too many at once: the rest is dropped, nothing piles up.
        let accepted = (0..30).filter(|_| w.capture(vec![0.0; FRAME_SAMPLES])).count();
        assert_eq!(accepted, MAX_QUEUED);
        let mut back = 0;
        while back < accepted {
            back += w.processed(std::time::Duration::from_millis(500)).await.len();
        }
        assert_eq!(w.take_failure(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_waiting_only_ready_frames_come_and_the_rest_later() {
        let mut w = Worker::start(Choices::default(), 0).unwrap();
        let start = std::time::Instant::now();
        assert!(w.capture(vec![0.0; FRAME_SAMPLES]));
        // At once: maybe nothing yet, never a wait.
        let now = w.processed(std::time::Duration::ZERO).await.len();
        assert!(start.elapsed() < std::time::Duration::from_millis(5));
        let mut got = now;
        while got < 1 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            got += w.processed(std::time::Duration::ZERO).await.len();
        }
        assert_eq!(got, 1);
    }

    #[test]
    fn nothing_on_keeps_the_voice() {
        let nothing = Choices {
            noise: Noise::Off,
            ..Choices::default()
        };
        assert!(!nothing.any());
        let mut p = Processor::new(nothing, 0);
        let mut frame = tone(0, FRAME_SAMPLES, 0.3);
        let before = rms(&frame);
        for _ in 0..10 {
            frame = tone(0, FRAME_SAMPLES, 0.3);
            p.capture(&mut frame);
        }
        assert!((rms(&frame) - before).abs() < before * 0.2);
        // Odd lengths do not panic; the rest is left alone.
        let mut odd = vec![0.1; CHUNK + 7];
        p.capture(&mut odd);
        assert_eq!(odd[CHUNK..], [0.1; 7]);
        p.render(&[0.0; 3]);
    }
}

//! Microphone clean-up before sending: echo, noise and level.
//!
//! Echo cancellation, automatic gain and the "Standard" noise suppression are
//! `sonora`, a pure-Rust port of WebRTC's audio processing (the one browsers
//! and Discord's own client build on). Two stronger noise suppressors can be
//! chosen instead of the standard one: RNNoise (`nnnoiseless`, a small
//! neural network) and DeepFilterNet3 (`deep-filter-rt`, a larger one, close
//! to Krisp; about 30 ms more delay). They run after the echo canceller.
//!
//! Everything is 48 kHz mono, 10 ms at a time. The echo canceller needs to
//! hear what the speakers play: [`Processor::render`] gets the mix right
//! before it is queued for the speakers, [`Processor::capture`] cleans the
//! microphone frames before voice detection and encoding.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use deep_filter_rt::tract::{DfParams, DfTract, RuntimeParams};
use serde::{Deserialize, Serialize};
use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::{CHANNELS, SAMPLE_RATE};

/// Samples in one 10 ms chunk at 48 kHz (what the processing works on).
pub const CHUNK: usize = SAMPLE_RATE as usize / 100;

/// Which noise suppression cleans the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Noise {
    Off,
    /// WebRTC's: steady noise (fan, hiss), almost no CPU.
    #[default]
    Standard,
    /// RNNoise: also keyboard and sudden noise, light.
    RnNoise,
    /// DeepFilterNet3: the strongest; more CPU, 30 ms more delay.
    DeepFilter,
}

impl Noise {
    pub const ALL: [Self; 4] = [Self::Off, Self::Standard, Self::RnNoise, Self::DeepFilter];
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
                "deep_filter" => Self::DeepFilter,
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
}

impl Choices {
    pub fn any(self) -> bool {
        self.echo || self.noise != Noise::Off || self.gain
    }
}

/// DeepFilterNet's model, loaded on its own thread (about 85 ms, more in
/// debug builds) so the voice connection never waits for it.
enum Deep {
    Loading(Receiver<Result<Box<DfTract>, String>>),
    Ready(Box<DfTract>),
}

/// The neural noise suppressor chosen, if any.
enum Neural {
    None,
    RnNoise(Box<nnnoiseless::DenoiseState<'static>>),
    Deep(Deep),
}

/// DeepFilterNet's model with Rico's settings. Never panics: a failure (the
/// crate's own loader panics on a broken embedded model) is an error.
fn load_deep_filter() -> Result<Box<DfTract>, String> {
    std::panic::catch_unwind(|| {
        let params = RuntimeParams::default_with_ch(1);
        DfTract::new(DfParams::default(), &params).map_err(|e| format!("{e:#}"))
    })
    .map_err(|_| "the model could not be loaded".to_string())?
    .map(Box::new)
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
        let apm = AudioProcessing::builder()
            .config(config(choices, choices.noise == Noise::Standard))
            .capture_config(mono)
            .render_config(mono)
            .build();
        let neural = match choices.noise {
            Noise::RnNoise => Neural::RnNoise(nnnoiseless::DenoiseState::new()),
            Noise::DeepFilter => {
                let (tx, rx) = mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("deepfilter-load".into())
                    .spawn(move || {
                        let _ = tx.send(load_deep_filter());
                    });
                match spawned {
                    Ok(_) => Neural::Deep(Deep::Loading(rx)),
                    Err(e) => {
                        let (tx, rx) = mpsc::channel();
                        let _ = tx.send(Err(e.to_string()));
                        Neural::Deep(Deep::Loading(rx))
                    }
                }
            }
            Noise::Off | Noise::Standard => Neural::None,
        };
        Self {
            apm,
            choices,
            neural,
            failure: None,
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

    /// The suppressor failed: WebRTC's Standard instead, never no filter.
    fn fall_back(&mut self, why: String) {
        tracing::warn!("noise suppression: falling back to Standard: {why}");
        self.neural = Neural::None;
        self.apm.apply_config(config(self.choices, true));
        self.failure = Some(why);
    }

    /// The DeepFilterNet model, once loaded.
    fn poll_deep(&mut self) {
        let Neural::Deep(Deep::Loading(rx)) = &self.neural else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(df)) => {
                tracing::info!("noise suppression: DeepFilterNet ready");
                self.neural = Neural::Deep(Deep::Ready(df));
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
        self.poll_deep();
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
                Neural::Deep(Deep::Ready(df)) => {
                    let noisy = ndarray::ArrayView2::from_shape((1, CHUNK), &self.input[..]);
                    let clean = ndarray::ArrayViewMut2::from_shape((1, CHUNK), &mut self.output[..]);
                    match (noisy, clean) {
                        (Ok(noisy), Ok(clean)) => match df.process(noisy, clean) {
                            Ok(_) => {
                                self.input.copy_from_slice(&self.output);
                                None
                            }
                            Err(e) => Some(format!("{e:#}")),
                        },
                        _ => Some("bad frame shape".to_string()),
                    }
                }
                // Until the model is loaded (a fraction of a second) the
                // echo canceller and gain still run.
                Neural::Deep(Deep::Loading(_)) => None,
            };
            if let Some(why) = failed {
                self.fall_back(why);
            }
            for (s, o) in chunk.iter_mut().zip(self.input.iter()) {
                *s = if o.is_finite() { o.clamp(-1.0, 1.0) } else { 0.0 };
            }
        }
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
    fn deep_filter_loads_in_the_background_and_turns_noise_down() {
        let mut p = Processor::new(
            Choices {
                noise: Noise::DeepFilter,
                echo: true,
                ..Choices::default()
            },
            20,
        );
        // Frames go on while the model loads (debug builds: a few seconds).
        let mut frame = vec![0.0; FRAME_SAMPLES];
        let start = std::time::Instant::now();
        while matches!(p.neural, Neural::Deep(Deep::Loading(_))) {
            assert!(start.elapsed().as_secs() < 120, "model not loaded");
            p.capture(&mut frame);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(matches!(p.neural, Neural::Deep(Deep::Ready(_))));
        let (after, before) = noise_after(&mut p, 2);
        assert!(after < before * 0.5, "{after} vs {before}");
        assert_eq!(p.take_failure(), None);
    }

    #[test]
    fn a_failing_suppressor_falls_back_to_standard() {
        let mut p = Processor::new(
            Choices {
                noise: Noise::DeepFilter,
                ..Choices::default()
            },
            0,
        );
        let (tx, rx) = mpsc::channel();
        tx.send(Err("broken".to_string())).unwrap();
        p.neural = Neural::Deep(Deep::Loading(rx));
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
        assert_eq!(read(r#""deep_filter""#), Noise::DeepFilter);
        assert_eq!(read(r#""off""#), Noise::Off);
        assert_eq!(read(r#""krisp""#), Noise::Standard);
        assert_eq!(read("3"), Noise::Standard);
        for n in Noise::ALL {
            assert_eq!(read(&serde_json::to_string(&n).unwrap()), n);
        }
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

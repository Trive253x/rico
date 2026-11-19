//! Microphone clean-up before sending: echo, noise and level.
//!
//! Done by `sonora`, a pure-Rust port of WebRTC's audio processing (the one
//! browsers and Discord's own client build on): echo cancellation, noise
//! suppression and automatic gain.
//!
//! Everything is 48 kHz mono, 10 ms at a time. The echo canceller needs to
//! hear what the speakers play: [`Processor::render`] gets the mix right
//! before it is queued for the speakers, [`Processor::capture`] cleans the
//! microphone frames before voice detection and encoding.

use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::{CHANNELS, SAMPLE_RATE};

/// Samples in one 10 ms chunk at 48 kHz (what the processing works on).
pub const CHUNK: usize = SAMPLE_RATE as usize / 100;

/// What is turned on in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Choices {
    pub echo: bool,
    pub noise: bool,
    pub gain: bool,
}

impl Choices {
    pub fn any(self) -> bool {
        self.echo || self.noise || self.gain
    }
}

/// The processing of one call. Its state (echo model, noise estimate, gain)
/// adapts over a few seconds, so it lives as long as the microphone is on.
pub struct Processor {
    apm: AudioProcessing,
    choices: Choices,
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
        let config = Config {
            high_pass_filter: Some(HighPassFilter::default()),
            echo_canceller: choices.echo.then(EchoCanceller::default),
            noise_suppression: choices.noise.then(|| NoiseSuppression {
                level: NoiseSuppressionLevel::High,
                ..NoiseSuppression::default()
            }),
            gain_controller2: choices.gain.then(|| GainController2 {
                adaptive_digital: Some(AdaptiveDigital::default()),
                ..GainController2::default()
            }),
            ..Config::default()
        };
        let mono = StreamConfig::new(SAMPLE_RATE, 1);
        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(mono)
            .render_config(mono)
            .build();
        Self {
            apm,
            choices,
            delay_ms: delay_ms.clamp(0, 500),
            input: [0.0; CHUNK],
            output: [0.0; CHUNK],
            render: Vec::with_capacity(CHUNK * 4),
        }
    }

    pub fn choices(&self) -> Choices {
        self.choices
    }

    /// Cleans microphone audio (48 kHz mono) in place. Its length must be a
    /// multiple of [`CHUNK`] (a 20 ms frame is two chunks); a rest is left
    /// as it is.
    pub fn capture(&mut self, frame: &mut [f32]) {
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
                for (s, o) in chunk.iter_mut().zip(self.output.iter()) {
                    *s = o.clamp(-1.0, 1.0);
                }
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
                noise: true,
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

    #[test]
    fn nothing_on_keeps_the_voice() {
        let mut p = Processor::new(Choices::default(), 0);
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

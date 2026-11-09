//! Voice audio: Opus codec, sample processing and audio devices.
//!
//! Discord voice is Opus at 48 kHz, stereo, 20 ms per packet. Inside ferrocord
//! audio is `f32` samples in that format; conversion to and from the devices'
//! own rate and channel count happens here, outside the device callbacks, so
//! that the callbacks only copy samples.

pub mod codec;
pub mod device;
pub mod dsp;
pub mod jitter;

/// Sample rate used by Discord voice.
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels of the Opus stream (stereo).
pub const CHANNELS: usize = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME_SAMPLES: usize = 960;
/// Interleaved samples in one 20 ms stereo frame.
pub const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS;

/// Audio choices from the settings (see `settings`): devices, volumes,
/// microphone sensitivity. Applied by the voice connection, also mid-call.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Microphone by name; `None`: the system default.
    pub input_device: Option<String>,
    /// Speakers by name; `None`: the system default.
    pub output_device: Option<String>,
    /// Gain on the microphone, 0.0 to [`MAX_GAIN`].
    pub input_volume: f32,
    /// Gain on everything played, 0.0 to [`MAX_GAIN`].
    pub output_volume: f32,
    /// The microphone sends when the level is above [`DEFAULT_SENSITIVITY_DB`]
    /// (`true`) or above `sensitivity_db`.
    pub auto_sensitivity: bool,
    /// Level (dBFS, -80 to 0) above which the microphone sends.
    pub sensitivity_db: f32,
}

/// Highest input or output gain (200%).
pub const MAX_GAIN: f32 = 2.0;
/// Microphone level above which voice is sent, by default (-40 dBFS).
pub const DEFAULT_SENSITIVITY_DB: f32 = -40.0;

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            auto_sensitivity: true,
            sensitivity_db: DEFAULT_SENSITIVITY_DB,
        }
    }
}

impl AudioSettings {
    /// Values from a file brought back into range.
    pub fn sanitized(mut self) -> Self {
        let gain = |g: f32| {
            if g.is_finite() {
                g.clamp(0.0, MAX_GAIN)
            } else {
                1.0
            }
        };
        self.input_volume = gain(self.input_volume);
        self.output_volume = gain(self.output_volume);
        self.sensitivity_db = if self.sensitivity_db.is_finite() {
            self.sensitivity_db.clamp(-80.0, 0.0)
        } else {
            DEFAULT_SENSITIVITY_DB
        };
        for name in [&mut self.input_device, &mut self.output_device] {
            if name
                .as_ref()
                .is_some_and(|n| n.trim().is_empty() || n.len() > 256)
            {
                *name = None;
            }
        }
        self
    }

    /// RMS level (0..1) above which the microphone sends.
    pub fn vad_threshold(&self) -> f32 {
        let db = if self.auto_sensitivity {
            DEFAULT_SENSITIVITY_DB
        } else {
            self.sensitivity_db
        };
        10f32.powf(db / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_brought_into_range() {
        let s = AudioSettings {
            input_volume: 9.0,
            output_volume: f32::NAN,
            sensitivity_db: -200.0,
            input_device: Some("  ".into()),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(
            (s.input_volume, s.output_volume, s.sensitivity_db),
            (2.0, 1.0, -80.0)
        );
        assert_eq!(s.input_device, None);
        assert!((AudioSettings::default().vad_threshold() - 0.01).abs() < 1e-6);
    }
}

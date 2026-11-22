//! Voice audio: Opus codec, sample processing and audio devices.
//!
//! Discord voice is Opus at 48 kHz, stereo, 20 ms per packet. Inside Rico
//! audio is `f32` samples in that format; conversion to and from the devices'
//! own rate and channel count happens here, outside the device callbacks, so
//! that the callbacks only copy samples.

pub mod codec;
pub mod device;
pub mod dpdfnet;
pub mod dsp;
pub mod hotkey;
pub mod jitter;
pub mod process;

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
    /// Volume of single people in calls (user ID → 0.0 to [`MAX_GAIN`]),
    /// only for us. Missing: 100%.
    pub user_volumes: std::collections::BTreeMap<String, f32>,
    /// People we do not hear in calls (only for us).
    pub muted_users: std::collections::BTreeSet<String>,
    /// Which noise suppression cleans the microphone before sending (the
    /// old `true`/`false` switch is read as Standard/Off).
    pub noise_suppression: process::Noise,
    /// What the speakers play is removed from the microphone (for calls
    /// without headphones).
    pub echo_cancellation: bool,
    /// The microphone level is evened out on its own.
    pub auto_gain: bool,
    /// The microphone sends only while [`AudioSettings::push_to_talk_key`]
    /// is held, instead of when it hears voice.
    pub push_to_talk: bool,
    /// The push-to-talk key, as `[modifier+]…Code` with W3C key codes
    /// (`F13`, `ctrl+Backquote`, `shift+KeyV`); see [`hotkey`].
    pub push_to_talk_key: Option<String>,
}

/// Highest input or output gain (200%).
pub const MAX_GAIN: f32 = 2.0;
/// Microphone level above which voice is sent, by default (-40 dBFS).
pub const DEFAULT_SENSITIVITY_DB: f32 = -40.0;
/// At most this many people with their own volume or muted.
pub const MAX_USER_SETTINGS: usize = 1000;

/// A Discord ID (a number), the only thing kept about a person.
fn is_user_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            auto_sensitivity: true,
            sensitivity_db: DEFAULT_SENSITIVITY_DB,
            user_volumes: std::collections::BTreeMap::default(),
            muted_users: std::collections::BTreeSet::default(),
            noise_suppression: process::Noise::Standard,
            echo_cancellation: true,
            auto_gain: false,
            push_to_talk: false,
            push_to_talk_key: None,
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
        // Only valid IDs, volumes in range, 100% is not kept, a bounded number.
        self.user_volumes.retain(|id, v| is_user_id(id) && v.is_finite());
        for v in self.user_volumes.values_mut() {
            *v = ((*v * 20.0).round() / 20.0).clamp(0.0, MAX_GAIN);
        }
        self.user_volumes.retain(|_, v| *v != 1.0);
        while self.user_volumes.len() > MAX_USER_SETTINGS {
            self.user_volumes.pop_last();
        }
        self.muted_users.retain(|id| is_user_id(id));
        while self.muted_users.len() > MAX_USER_SETTINGS {
            self.muted_users.pop_last();
        }
        self.push_to_talk_key = self
            .push_to_talk_key
            .as_deref()
            .and_then(hotkey::Hotkey::parse)
            .map(|k| k.to_string());
        // Push-to-talk without a key would never send: voice activity then.
        if self.push_to_talk_key.is_none() {
            self.push_to_talk = false;
        }
        self
    }

    /// What the microphone clean-up does (see [`process`]).
    pub fn processing(&self) -> process::Choices {
        process::Choices {
            echo: self.echo_cancellation,
            noise: self.noise_suppression,
            gain: self.auto_gain,
        }
    }

    /// Gain on what `user_id` says: 0 if muted by us, else their volume.
    pub fn user_gain(&self, user_id: &str) -> f32 {
        if self.muted_users.contains(user_id) {
            0.0
        } else {
            self.user_volumes.get(user_id).copied().unwrap_or(1.0)
        }
    }

    /// The volume chosen for `user_id` (1.0 if none).
    pub fn user_volume(&self, user_id: &str) -> f32 {
        self.user_volumes.get(user_id).copied().unwrap_or(1.0)
    }

    /// Sets the volume of `user_id` (0.0 to [`MAX_GAIN`]).
    pub fn set_user_volume(&mut self, user_id: &str, volume: f32) {
        if !is_user_id(user_id) || !volume.is_finite() {
            return;
        }
        let v = ((volume * 20.0).round() / 20.0).clamp(0.0, MAX_GAIN);
        if v == 1.0 {
            self.user_volumes.remove(user_id);
        } else if self.user_volumes.len() < MAX_USER_SETTINGS || self.user_volumes.contains_key(user_id) {
            self.user_volumes.insert(user_id.to_string(), v);
        }
    }

    /// Mutes or unmutes `user_id` for us.
    pub fn set_user_muted(&mut self, user_id: &str, muted: bool) {
        if !is_user_id(user_id) {
            return;
        }
        if !muted {
            self.muted_users.remove(user_id);
        } else if self.muted_users.len() < MAX_USER_SETTINGS {
            self.muted_users.insert(user_id.to_string());
        }
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

    #[test]
    fn push_to_talk_needs_a_valid_key() {
        let s: AudioSettings =
            serde_json::from_str(r#"{"push_to_talk": true, "push_to_talk_key": "Escape"}"#).unwrap();
        let s = s.sanitized();
        assert!(!s.push_to_talk);
        assert_eq!(s.push_to_talk_key, None);
        let s: AudioSettings =
            serde_json::from_str(r#"{"push_to_talk": true, "push_to_talk_key": "Shift+F13"}"#).unwrap();
        let s = s.sanitized();
        assert!(s.push_to_talk);
        assert_eq!(s.push_to_talk_key.as_deref(), Some("shift+F13"));
        // Older settings files: the clean-up defaults.
        let s: AudioSettings = serde_json::from_str("{}").unwrap();
        assert!(s.echo_cancellation && !s.auto_gain && !s.push_to_talk);
        assert_eq!(s.noise_suppression, process::Noise::Standard);
        // The 0.19.0-rc switch.
        let s: AudioSettings = serde_json::from_str(r#"{"noise_suppression": false}"#).unwrap();
        assert_eq!(s.noise_suppression, process::Noise::Off);
    }

    #[test]
    fn volumes_of_single_people() {
        let mut s = AudioSettings::default();
        assert_eq!(s.user_gain("901"), 1.0);
        s.set_user_volume("901", 1.5);
        s.set_user_volume("902", 9.0);
        s.set_user_volume("not an id", 0.5);
        assert_eq!(s.user_gain("901"), 1.5);
        assert_eq!(s.user_gain("902"), MAX_GAIN);
        assert_eq!(s.user_volumes.len(), 2);
        // Muting keeps the volume for later.
        s.set_user_muted("901", true);
        assert_eq!(s.user_gain("901"), 0.0);
        s.set_user_muted("901", false);
        assert_eq!(s.user_gain("901"), 1.5);
        // Back to 100%: nothing kept.
        s.set_user_volume("901", 1.0);
        assert!(!s.user_volumes.contains_key("901"));
        // From a file: bad IDs and values are dropped or brought into range.
        let s: AudioSettings = serde_json::from_str(
            r#"{"user_volumes": {"903": 7, "x": 0.5, "904": 1.0}, "muted_users": ["905", "../x"]}"#,
        )
        .unwrap();
        let s = s.sanitized();
        assert_eq!(s.user_volumes.len(), 1);
        assert_eq!(s.user_gain("903"), MAX_GAIN);
        assert_eq!(s.muted_users.len(), 1);
    }
}

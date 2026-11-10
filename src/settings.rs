//! Client settings, kept in `settings.json` in the configuration folder.
//!
//! Only things ferrocord itself does are here (appearance, animations,
//! images, audio devices and levels): nothing is sent to Discord. A missing
//! or broken file gives the defaults; values out of range are brought back.

use serde::{Deserialize, Serialize};

use crate::audio::AudioSettings;

/// Colour theme of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    /// The original dark theme ("One Dark").
    #[default]
    Dark,
    Light,
    /// Black backgrounds, for OLED screens.
    Amoled,
    /// Light or dark like the operating system, following its changes.
    System,
    Purple,
    /// Warm light theme (light brown).
    Latte,
    Blue,
    Red,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 8] = [
        ThemeChoice::Dark,
        ThemeChoice::Light,
        ThemeChoice::Amoled,
        ThemeChoice::System,
        ThemeChoice::Purple,
        ThemeChoice::Latte,
        ThemeChoice::Blue,
        ThemeChoice::Red,
    ];

    pub fn label(self) -> &'static str {
        use crate::i18n::t;
        match self {
            ThemeChoice::Dark => t("Dark", "Scuro"),
            ThemeChoice::Light => t("Light", "Chiaro"),
            ThemeChoice::Amoled => t("Black (AMOLED)", "Nero (AMOLED)"),
            ThemeChoice::System => t("Like the system", "Come il sistema"),
            ThemeChoice::Purple => t("Purple", "Viola"),
            ThemeChoice::Latte => t("Light brown", "Marrone chiaro"),
            ThemeChoice::Blue => t("Blue", "Blu"),
            ThemeChoice::Red => t("Red", "Rosso"),
        }
    }
}

/// Smallest and largest interface zoom.
pub const MIN_ZOOM: f32 = 0.8;
pub const MAX_ZOOM: f32 = 1.5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Interface zoom, [`MIN_ZOOM`] to [`MAX_ZOOM`] (on top of the system's
    /// display scaling).
    pub zoom: f32,
    /// Compact messages: one line per message, no avatars.
    pub compact: bool,
    /// Times as 14:05 (`true`) or 2:05 PM.
    pub clock_24h: bool,
    /// GIFs and animated emojis play by themselves; otherwise only while the
    /// mouse is over them.
    pub autoplay: bool,
    /// Images and GIFs in messages are downloaded and shown; otherwise only
    /// their name.
    pub show_images: bool,
    /// Reactions under the messages.
    pub show_reactions: bool,
    /// Previews in the GIF picker. They come from Klipy, which then sees
    /// the IP address: off until the user turns them on.
    pub gif_picker_previews: bool,
    /// The members of the server on the right of its channels.
    pub member_list: bool,
    pub audio: AudioSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            zoom: 1.0,
            compact: false,
            clock_24h: true,
            autoplay: true,
            show_images: true,
            show_reactions: true,
            gif_picker_previews: false,
            member_list: true,
            audio: AudioSettings::default(),
        }
    }
}

impl Settings {
    /// Values from a file brought back into range.
    pub fn sanitized(mut self) -> Self {
        self.zoom = if self.zoom.is_finite() {
            ((self.zoom * 20.0).round() / 20.0).clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            1.0
        };
        self.audio = self.audio.sanitized();
        self
    }

    /// Reads settings saved by [`Settings::to_json`]; anything unreadable
    /// gives the defaults.
    pub fn from_json(text: &str) -> Self {
        match serde_json::from_str::<Settings>(text) {
            Ok(s) => s.sanitized(),
            Err(e) => {
                tracing::warn!("settings: unreadable file, using the defaults: {e}");
                Settings::default()
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Time of a message, as chosen.
    pub fn time_format(&self) -> &'static str {
        if self.clock_24h {
            "%H:%M"
        } else {
            "%-I:%M %p"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let s = Settings {
            theme: ThemeChoice::Latte,
            zoom: 1.25,
            compact: true,
            autoplay: false,
            ..Default::default()
        };
        assert_eq!(Settings::from_json(&s.to_json()), s);
        // Missing fields take the default, broken files give the defaults.
        assert_eq!(
            Settings::from_json(r#"{"theme":"blue"}"#).theme,
            ThemeChoice::Blue
        );
        assert_eq!(Settings::from_json(r#"{"theme":"blue"}"#).zoom, 1.0);
        assert_eq!(Settings::from_json("not json"), Settings::default());
        assert_eq!(Settings::from_json(r#"{"theme":"unknown"}"#), Settings::default());
    }

    #[test]
    fn values_are_brought_into_range() {
        let s = Settings::from_json(r#"{"zoom": 9, "audio": {"input_volume": -1}}"#);
        assert_eq!(s.zoom, MAX_ZOOM);
        assert_eq!(s.audio.input_volume, 0.0);
        assert_eq!(Settings::from_json(r#"{"zoom": 0.1}"#).zoom, MIN_ZOOM);
    }

    #[test]
    fn clock_formats() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(14, 5, 0)
            .unwrap();
        let mut s = Settings::default();
        assert_eq!(t.format(s.time_format()).to_string(), "14:05");
        s.clock_24h = false;
        assert_eq!(t.format(s.time_format()).to_string(), "2:05 PM");
    }
}

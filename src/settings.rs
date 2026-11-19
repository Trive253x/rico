//! Client settings, kept in `settings.json` in the configuration folder.
//!
//! Only things Rico itself does are here (appearance, animations,
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
    /// Dark green with a sage accent.
    Forest,
    /// Dark petrol with a turquoise accent.
    Ocean,
    /// Warm brown with an amber accent.
    Sunset,
    /// Neutral grey with a single lime accent.
    Graphite,
    /// Soft purple with rose and gold (after the Rosé Pine theme).
    RosePine,
    /// Warm cream (after Solarized Light, darkened for legible text).
    SolarizedLight,
    /// Light green-white with a dark green accent.
    Mint,
}

impl ThemeChoice {
    pub const ALL: [Self; 15] = [
        Self::Dark,
        Self::Light,
        Self::Amoled,
        Self::System,
        Self::Purple,
        Self::Latte,
        Self::Blue,
        Self::Red,
        Self::Forest,
        Self::Ocean,
        Self::Sunset,
        Self::Graphite,
        Self::RosePine,
        Self::SolarizedLight,
        Self::Mint,
    ];

    pub fn label(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Self::Dark => t("Dark", "Scuro"),
            Self::Light => t("Light", "Chiaro"),
            Self::Amoled => t("Black (AMOLED)", "Nero (AMOLED)"),
            Self::System => t("Like the system", "Come il sistema"),
            Self::Purple => t("Purple", "Viola"),
            Self::Latte => t("Light brown", "Marrone chiaro"),
            Self::Blue => t("Blue", "Blu"),
            Self::Red => t("Red", "Rosso"),
            Self::Forest => t("Forest", "Foresta"),
            Self::Ocean => t("Ocean", "Oceano"),
            Self::Sunset => t("Sunset", "Tramonto"),
            Self::Graphite => t("Graphite", "Grafite"),
            Self::RosePine => t("Rosé Pine", "Rosé Pine"),
            Self::SolarizedLight => t("Solarized light", "Solarized chiaro"),
            Self::Mint => t("Mint", "Menta"),
        }
    }
}

/// How the member list on the right shows people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStyle {
    /// Groups by role, 32 px avatars (as before).
    #[default]
    Classic,
    /// Short rows with small avatars, a search field and the counts.
    Compact,
    /// One card per person: status ring, custom status or what they are doing.
    Rich,
    /// Who is in voice first, then who is active here, online, offline.
    Activity,
}

impl MemberStyle {
    pub const ALL: [Self; 4] = [Self::Classic, Self::Compact, Self::Rich, Self::Activity];

    pub fn label(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Self::Classic => t("Classic", "Classico"),
            Self::Compact => t("Compact", "Compatto"),
            Self::Rich => t("Rich", "Ricco"),
            Self::Activity => t("By activity", "Per attività"),
        }
    }

    pub fn description(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Self::Classic => t(
                "Groups by role, as in Discord.",
                "Gruppi per ruolo, come su Discord.",
            ),
            Self::Compact => t(
                "Short rows and a search field: about twice as many people on screen.",
                "Righe basse e un campo di ricerca: circa il doppio delle persone a schermo.",
            ),
            Self::Rich => t(
                "Bigger cards with what each person is doing: in voice, streaming, typing, custom status.",
                "Schede più grandi con cosa fa ognuno: in voce, in streaming, sta scrivendo, stato personalizzato.",
            ),
            Self::Activity => t(
                "Who is in voice first, then who wrote here recently, then online and offline.",
                "Prima chi è in voce, poi chi ha scritto qui da poco, poi online e offline.",
            ),
        }
    }
}

// A value written by a newer version becomes the default instead of making
// the whole file unreadable.
impl<'de> Deserialize<'de> for MemberStyle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "compact" => Self::Compact,
            "rich" => Self::Rich,
            "activity" => Self::Activity,
            _ => Self::Classic,
        })
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
    /// How that list shows people.
    pub member_style: MemberStyle,
    /// Check GitHub once a day for a newer Rico (on by default, the owner's
    /// choice; GitHub then sees the IP address once a day).
    pub check_updates: bool,
    /// Also offer candidates (pre-releases): off by default.
    pub update_candidates: bool,
    pub audio: AudioSettings,
    /// Our Go Live stream: quality, and whether the sound goes too.
    pub golive_quality: crate::video::encoder::Quality,
    pub golive_sound: bool,
    /// Windows notifications for direct messages and mentions (as the
    /// account's notification settings say).
    pub notifications: bool,
    /// The message text in the notification (off: only who and where).
    pub notification_text: bool,
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
            member_style: MemberStyle::Classic,
            check_updates: true,
            update_candidates: false,
            audio: AudioSettings::default(),
            golive_quality: crate::video::encoder::Quality::default(),
            golive_sound: true,
            notifications: true,
            notification_text: true,
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
    /// gives the defaults. A UTF-8 byte order mark (added by some Windows
    /// editors and by PowerShell 5) is skipped.
    pub fn from_json(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        match serde_json::from_str::<Self>(text) {
            Ok(s) => s.sanitized(),
            Err(e) => {
                tracing::warn!("settings: unreadable file, using the defaults: {e}");
                Self::default()
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
        assert_eq!(
            Settings::from_json(r#"{"theme":"rose_pine"}"#).theme,
            ThemeChoice::RosePine
        );
        assert_eq!(
            Settings::from_json(r#"{"theme":"solarized_light"}"#).theme,
            ThemeChoice::SolarizedLight
        );
        assert_eq!(Settings::from_json("not json"), Settings::default());
        // A byte order mark in front (Notepad, PowerShell 5) is skipped.
        let s = Settings::from_json("\u{feff}{\"theme\":\"rose_pine\",\"update_candidates\":true}");
        assert_eq!(s.theme, ThemeChoice::RosePine);
        assert!(s.update_candidates);
        assert_eq!(Settings::from_json(r#"{"theme":"unknown"}"#), Settings::default());
    }

    #[test]
    fn member_style_round_trip_and_unknown_values() {
        for style in MemberStyle::ALL {
            let s = Settings {
                member_style: style,
                ..Default::default()
            };
            assert_eq!(Settings::from_json(&s.to_json()).member_style, style);
        }
        // A style from a newer version: the default, the rest of the file kept.
        let s = Settings::from_json(r#"{"member_style":"hologram","zoom":1.25}"#);
        assert_eq!(s.member_style, MemberStyle::Classic);
        assert_eq!(s.zoom, 1.25);
        assert_eq!(Settings::from_json("{}").member_style, MemberStyle::Classic);
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

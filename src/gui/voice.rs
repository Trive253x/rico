//! Voice chat state in the GUI and the panel above the user bar.

use std::collections::HashSet;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, horizontal_space, row, text, tooltip};
use iced::{Element, Length};

use super::style::*;
use super::{dot, Message};
use crate::discord::voice::{Encryption, VoiceHandle, VoiceServer, VoiceStats};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Request sent to the gateway, waiting for the voice server.
    Joining,
    Connecting,
    Connected(&'static str),
    Reconnecting,
    Failed(String),
}

pub struct VoiceUi {
    pub guild_id: String,
    pub channel_id: String,
    pub status: Status,
    /// With `--demo`: local fake voice server.
    pub demo: bool,
    /// From our `VOICE_STATE_UPDATE`.
    pub session_id: Option<String>,
    /// From `VOICE_SERVER_UPDATE`: (endpoint, token).
    pub server: Option<(String, String)>,
    /// Changes on every join: restarts the connection.
    pub attempt: u64,
    pub handle: Option<VoiceHandle>,
    pub speaking: HashSet<String>,
    pub stats: VoiceStats,
    pub encryption: Encryption,
    /// The microphone is being captured (confirmed by the connection).
    pub mic_on: bool,
    pub deafened: bool,
    /// Last problem with an audio device.
    pub audio_error: Option<String>,
    /// Discord confirmed we are in this channel (our voice state arrived).
    pub confirmed: bool,
}

impl VoiceUi {
    pub fn new(guild_id: String, channel_id: String, demo: bool, attempt: u64) -> Self {
        Self {
            guild_id,
            channel_id,
            status: if demo { Status::Connecting } else { Status::Joining },
            demo,
            session_id: None,
            server: None,
            attempt,
            handle: None,
            speaking: HashSet::new(),
            stats: VoiceStats::default(),
            encryption: Encryption::Pending,
            mic_on: false,
            deafened: false,
            audio_error: None,
            confirmed: false,
        }
    }

    /// Everything needed to connect to the real voice server, once it is all there.
    pub fn server_info(&self, user_id: &str) -> Option<VoiceServer> {
        if self.demo || matches!(self.status, Status::Failed(_)) {
            return None;
        }
        let (endpoint, token) = self.server.clone()?;
        Some(VoiceServer {
            endpoint,
            token,
            guild_id: self.guild_id.clone(),
            channel_id: self.channel_id.clone(),
            session_id: self.session_id.clone()?,
            user_id: user_id.to_string(),
        })
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.status, Status::Failed(_))
    }

    fn e2ee_active(&self) -> bool {
        matches!(self.encryption, Encryption::Active { .. })
    }
}

/// Splits a long numeric code in groups of five digits.
fn grouped(code: &str) -> String {
    let digits: Vec<char> = code.chars().filter(char::is_ascii_digit).collect();
    digits
        .chunks(5)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Panel at the bottom of the channel list: state, channel, encryption,
/// microphone, deafen and leave.
pub fn panel<'a>(v: &'a VoiceUi, channel_name: String) -> Element<'a, Message> {
    let (color, label) = match &v.status {
        Status::Joining => (YELLOW, "Waiting for the voice server…"),
        Status::Connecting => (YELLOW, "Connecting to voice…"),
        Status::Reconnecting => (YELLOW, "Reconnecting…"),
        Status::Connected(_) if v.e2ee_active() => (GREEN, "Voice connected"),
        Status::Connected(_) => (YELLOW, "Setting up end-to-end encryption…"),
        Status::Failed(_) => (RED, "Voice unavailable"),
    };
    let mut details = format!("🔊 {channel_name}");
    if let (Status::Connected(_), Some(ping)) = (&v.status, v.stats.ping_ms) {
        details.push_str(&format!(" · {ping} ms"));
    }
    if v.demo {
        details.push_str(" · demo");
    }
    let mut col = column![
        row![
            dot(color, 8.0),
            text(label).size(12).font(SEMIBOLD).color(color),
            horizontal_space(),
            button(text("Leave").size(12))
                .on_press(Message::LeaveVoice)
                .padding([3, 10])
                .style(ghost),
        ]
        .spacing(6)
        .align_y(Vertical::Center),
        text(details).size(11).color(MUTED).shaping(Shaping::Advanced),
    ]
    .spacing(4);

    if let Status::Failed(reason) = &v.status {
        col = col.push(
            text(reason.clone())
                .size(11)
                .color(MUTED)
                .shaping(Shaping::Advanced),
        );
    } else if matches!(v.status, Status::Connected(_)) {
        if let Encryption::Active { privacy_code } = &v.encryption {
            let lock = text(format!("🔒 End-to-end encrypted · {}", grouped(privacy_code)))
                .size(10)
                .color(GREEN)
                .shaping(Shaping::Advanced);
            col = col.push(tooltip(
                lock,
                container(
                    text(
                        "Voice privacy code: the same for everyone in this call. \
                         Compare it with the others to check that nobody is in between.",
                    )
                    .size(11),
                )
                .padding(6)
                .max_width(260)
                .style(super::style::tooltip),
                tooltip::Position::Top,
            ));
        }
        // Microphone and deafen: the state is always written out in full.
        let mic_label = if v.mic_on {
            "🎙 Microphone ON"
        } else {
            "🎙 Microphone off"
        };
        let mic = button(text(mic_label).size(11).shaping(Shaping::Advanced))
            .on_press(Message::ToggleMicrophone)
            .padding([3, 8])
            .style(if v.mic_on { danger } else { ghost });
        let deaf_label = if v.deafened {
            "🔇 Deafened"
        } else {
            "🔈 Sound on"
        };
        let deaf = button(text(deaf_label).size(11).shaping(Shaping::Advanced))
            .on_press(Message::ToggleDeafen)
            .padding([3, 8])
            .style(ghost);
        col = col.push(row![mic, deaf].spacing(6));
        if let Some(err) = &v.audio_error {
            col = col.push(text(err.clone()).size(10).color(RED).shaping(Shaping::Advanced));
        }
        col = col.push(
            text(format!(
                "Audio: {} received, {} sent · {}",
                v.stats.decoded,
                v.stats.sent,
                match &v.status {
                    Status::Connected(mode) => mode,
                    _ => "",
                }
            ))
            .size(10)
            .color(MUTED),
        );
    }
    container(col)
        .padding([8, 12])
        .width(Length::Fill)
        .style(fill(TITLE))
        .into()
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn codes_are_grouped_by_five() {
        assert_eq!(grouped("123451234512345"), "12345 12345 12345");
        assert_eq!(grouped("12 345"), "12345");
    }
}

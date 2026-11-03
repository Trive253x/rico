//! Stato della chat vocale nella GUI e pannello sopra l'utente.

use std::collections::HashSet;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, horizontal_space, row, text};
use iced::{Element, Length};

use super::style::*;
use super::{dot, Message};
use crate::discord::voice::{VoiceHandle, VoiceServer, VoiceStats};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Richiesta mandata al gateway, in attesa del server vocale.
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
    /// Con `--demo`: server vocale finto in locale.
    pub demo: bool,
    /// Dal nostro `VOICE_STATE_UPDATE`.
    pub session_id: Option<String>,
    /// Dal `VOICE_SERVER_UPDATE`: (endpoint, token).
    pub server: Option<(String, String)>,
    /// Cambia a ogni ingresso: riavvia la connessione.
    pub attempt: u64,
    pub handle: Option<VoiceHandle>,
    pub speaking: HashSet<String>,
    pub stats: VoiceStats,
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
        }
    }

    /// Dati per collegarsi al server vocale vero, quando ci sono tutti.
    pub fn server_info(&self, user_id: &str) -> Option<VoiceServer> {
        if self.demo || matches!(self.status, Status::Failed(_)) {
            return None;
        }
        let (endpoint, token) = self.server.clone()?;
        Some(VoiceServer {
            endpoint,
            token,
            guild_id: self.guild_id.clone(),
            session_id: self.session_id.clone()?,
            user_id: user_id.to_string(),
        })
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.status, Status::Failed(_))
    }
}

/// Pannello in fondo alla lista dei canali: stato, canale, ping, "Esci".
pub fn panel<'a>(v: &'a VoiceUi, channel_name: String) -> Element<'a, Message> {
    let (color, label) = match &v.status {
        Status::Joining => (YELLOW, "In attesa del server vocale…".to_string()),
        Status::Connecting => (YELLOW, "Connessione alla voce…".to_string()),
        Status::Reconnecting => (YELLOW, "Riconnessione…".to_string()),
        Status::Connected(_) => (GREEN, "Connesso alla voce".to_string()),
        Status::Failed(_) => (RED, "Voce non disponibile".to_string()),
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
            button(text("Esci").size(12))
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
    } else if let Status::Connected(mode) = &v.status {
        col = col.push(
            text(format!(
                "Audio: {} ricevuti, {} inviati · {}",
                v.stats.received, v.stats.sent, mode
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

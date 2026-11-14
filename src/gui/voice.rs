//! Voice chat state in the GUI and the panel above the user bar.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::LazyLock;

use iced::alignment::Vertical;
use iced::widget::svg;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, horizontal_space, row, text, tooltip};
use iced::{Element, Length};

use super::icons::{self, icon};
use super::style::*;
use super::{dot, Message};
use crate::discord::voice::{Encryption, VoiceHandle, VoiceServer, VoiceStats};
use crate::i18n::t;

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
            stream: None,
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
/// microphone, deafen, our stream and leave.
/// `can_speak`: the channel's permissions let us talk; otherwise the
/// microphone cannot be turned on (only off, if it was on). `can_stream`:
/// we may Go Live here. `golive`: our stream, if on ("Live" stays visible
/// for as long as something is captured).
pub fn panel<'a>(
    v: &'a VoiceUi,
    channel_name: String,
    can_speak: bool,
    can_stream: bool,
    golive: Option<&'a super::golive::GoLiveUi>,
) -> Element<'a, Message> {
    let (color, label) = match &v.status {
        Status::Joining => (
            pal::yellow(),
            t("Waiting for the voice server…", "In attesa del server vocale…"),
        ),
        Status::Connecting => (pal::yellow(), t("Connecting to voice…", "Connessione alla voce…")),
        Status::Reconnecting => (pal::yellow(), t("Reconnecting…", "Riconnessione…")),
        Status::Connected(_) if v.e2ee_active() => (pal::green(), t("Voice connected", "Connesso alla voce")),
        Status::Connected(_) => (
            pal::yellow(),
            t(
                "Setting up end-to-end encryption…",
                "Attivazione della crittografia end-to-end…",
            ),
        ),
        Status::Failed(_) => (pal::red(), t("Voice unavailable", "Voce non disponibile")),
    };
    let mut details = format!("🔊 {channel_name}");
    if let (Status::Connected(_), Some(ping)) = (&v.status, v.stats.ping_ms) {
        let _ = write!(details, " · {ping} ms");
    }
    if v.demo {
        details.push_str(" · demo");
    }
    // Own row for the buttons: the sidebar is narrow and the texts above vary.
    let connected = matches!(v.status, Status::Connected(_));
    let mut controls = row![].spacing(8).align_y(Vertical::Center);
    if connected {
        controls = controls.push(mic_button(v, can_speak, 32.0));
        controls = controls.push(sound_button(v, 32.0));
        controls = controls.push(stream_button(golive.is_some(), can_stream, 32.0));
    }
    controls = controls.push(horizontal_space());
    controls = controls.push(hang_up_button(32.0));
    let mut col = column![
        row![
            dot(color, 8.0),
            column![
                text(label).size(12).font(SEMIBOLD).color(color),
                text(details)
                    .size(11)
                    .color(pal::muted())
                    .shaping(Shaping::Advanced),
            ]
            .spacing(1)
            .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Vertical::Center),
        controls,
    ]
    .spacing(6);

    if let Some(g) = golive {
        col = col.push(super::golive::live_panel(g));
    }
    if let Status::Failed(reason) = &v.status {
        col = col.push(
            text(reason.clone())
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        );
    } else if matches!(v.status, Status::Connected(_)) {
        if let Encryption::Active { privacy_code } = &v.encryption {
            let lock = text(tf!(
                "🔒 End-to-end encrypted · {}",
                "🔒 Crittografia end-to-end · {}",
                grouped(privacy_code)
            ))
            .size(10)
            .color(pal::green())
            .shaping(Shaping::Advanced);
            col = col.push(tooltip(
                lock,
                container(
                    text(t(
                        "Voice privacy code: the same for everyone in this call. \
                         Compare it with the others to check that nobody is in between.",
                        "Codice di privacy della chiamata: è uguale per tutti i partecipanti. \
                         Confrontalo con gli altri per verificare che nessuno si sia messo in mezzo.",
                    ))
                    .size(11),
                )
                .padding(6)
                .max_width(260)
                .style(super::style::tooltip),
                tooltip::Position::Top,
            ));
        }
        if let Some(err) = &v.audio_error {
            col = col.push(
                text(err.clone())
                    .size(10)
                    .color(pal::red())
                    .shaping(Shaping::Advanced),
            );
        }
        col = col.push(
            text(tf!(
                "Audio: {} received, {} sent · {}",
                "Audio: {} ricevuti, {} inviati · {}",
                v.stats.decoded,
                v.stats.sent,
                match &v.status {
                    Status::Connected(mode) => mode,
                    _ => "",
                }
            ))
            .size(10)
            .color(pal::muted()),
        );
    }
    container(col)
        .padding([8, 12])
        .width(Length::Fill)
        .style(fill(pal::title()))
        .into()
}

/// Round icon button with a tooltip that says the state and what a click does.
pub fn control_button<'a>(
    image: &LazyLock<svg::Handle>,
    kind: Control,
    tip: String,
    on_press: Option<Message>,
    size: f32,
) -> Element<'a, Message> {
    let enabled = on_press.is_some();
    let b = button(
        container(icon(image, size * 0.55, control_icon(kind, enabled)))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(size)
    .height(size)
    .padding(0)
    .on_press_maybe(on_press)
    .style(control(kind));
    tooltip(
        b,
        container(text(tip).size(11).shaping(Shaping::Advanced))
            .padding([4, 8])
            .style(super::style::tooltip),
        tooltip::Position::Top,
    )
    .gap(4)
    .into()
}

/// Microphone on/off. It only turns on with a click, and only where the
/// channel lets us speak (turning it off is always possible).
pub fn mic_button<'a>(v: &VoiceUi, can_speak: bool, size: f32) -> Element<'a, Message> {
    let (image, kind, tip) = if v.mic_on {
        (
            &icons::MIC,
            Control::On,
            t(
                "Microphone on: click to mute",
                "Microfono acceso: clic per spegnerlo",
            ),
        )
    } else if !can_speak {
        (
            &icons::MIC_OFF,
            Control::Off,
            t(
                "You cannot speak in this channel",
                "In questo canale non puoi parlare",
            ),
        )
    } else {
        (
            &icons::MIC_OFF,
            Control::Off,
            t(
                "Microphone off: click to speak",
                "Microfono spento: clic per parlare",
            ),
        )
    };
    let press = (can_speak || v.mic_on).then_some(Message::ToggleMicrophone);
    control_button(image, kind, tip.into(), press, size)
}

/// Sound on/off (deafen): off also turns the microphone off.
pub fn sound_button<'a>(v: &VoiceUi, size: f32) -> Element<'a, Message> {
    let (image, kind, tip) = if v.deafened {
        (
            &icons::HEADPHONES_OFF,
            Control::Off,
            t(
                "Sound off: you hear nobody. Click to hear again",
                "Audio disattivato: non senti nessuno. Clic per riattivarlo",
            ),
        )
    } else {
        (
            &icons::HEADPHONES,
            Control::On,
            t(
                "Sound on: click to hear nobody (also turns the microphone off)",
                "Audio attivo: clic per non sentire nessuno (spegne anche il microfono)",
            ),
        )
    };
    control_button(image, kind, tip.into(), Some(Message::ToggleDeafen), size)
}

/// Our Go Live stream: opens the choice of what to stream, or stops it.
pub fn stream_button<'a>(live: bool, can_stream: bool, size: f32) -> Element<'a, Message> {
    let (kind, tip, press) = if live {
        (
            Control::Leave,
            t(
                "You are live: click to stop",
                "Sei in diretta: clic per interrompere",
            ),
            Some(Message::StopGoLive),
        )
    } else if can_stream {
        (
            Control::On,
            t(
                "Stream your screen or a window",
                "Trasmetti lo schermo o una finestra",
            ),
            Some(Message::OpenGoLive),
        )
    } else {
        (
            Control::Off,
            t(
                "You cannot stream in this channel",
                "In questo canale non puoi trasmettere",
            ),
            None,
        )
    };
    control_button(&icons::SCREEN, kind, tip.into(), press, size)
}

/// Leaves the voice channel.
pub fn hang_up_button<'a>(size: f32) -> Element<'a, Message> {
    control_button(
        &icons::HANG_UP,
        Control::Leave,
        t("Leave the voice channel", "Esci dal canale vocale").into(),
        Some(Message::LeaveVoice),
        size,
    )
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

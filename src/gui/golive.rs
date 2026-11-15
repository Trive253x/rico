//! Our own Go Live stream: the window to choose what to stream, the state of
//! the stream and its connection, and the "Live" line in the call panel.
//!
//! Starting:
//! 1. the user picks a screen or a window (and quality and sound) and
//!    clicks "Go Live": only then does the capture start
//!    ([`crate::capture`]);
//! 2. op 18 on the gateway; Discord answers with `STREAM_CREATE` (op 22
//!    then says the stream is not paused) and `STREAM_SERVER_UPDATE`;
//! 3. a stream connection to that server sends the frames
//!    ([`crate::discord::voice::GoLiveFeed`]), end-to-end encrypted.
//!
//! Stopping (button, leaving the call, Discord ending it, the window being
//! closed) sends op 19, closes the connection and drops the capture. In the
//! demo, the stream goes to the local fake voice server.

use std::sync::Arc;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{
    button, checkbox, column, container, mouse_area, opaque, pick_list, row, scrollable, space, text,
};
use iced::{Element, Length};

use super::style::*;
use super::watch::{FrameSlot, VideoView};
use super::Message;
use crate::capture::{self, Running, Source, SourceKind};
use crate::discord::voice::{Encryption, GoLiveFeed, VoiceHandle, VoiceServer, VoiceStats};
use crate::i18n::t;
use crate::video::encoder::Quality;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Op 18 sent, waiting for Discord.
    Starting,
    Connecting,
    Connected,
}

pub struct GoLiveUi {
    pub guild_id: String,
    pub channel_id: String,
    /// `guild:{server}:{channel}:{us}`.
    pub key: String,
    pub status: Status,
    pub source_name: String,
    pub quality: Quality,
    pub rtc_server_id: Option<String>,
    /// (endpoint, token) from `STREAM_SERVER_UPDATE`.
    pub server: Option<(String, String)>,
    /// Changes when a new server arrives: restarts the connection.
    pub attempt: u64,
    pub handle: Option<VoiceHandle>,
    pub stats: VoiceStats,
    pub encryption: Encryption,
    /// What we send, drawn small in the call panel.
    pub preview: Arc<FrameSlot>,
    pub demo: bool,
    /// The sound could not be captured (the picture goes on).
    pub sound_error: Option<String>,
    /// The capture: dropping it stops it.
    pub running: Running,
}

impl GoLiveUi {
    pub fn feed(&self) -> GoLiveFeed {
        GoLiveFeed {
            outbox: self.running.outbox(),
            sound: self.running.sound.clone(),
            quality: self.quality,
        }
    }

    /// Everything needed for the stream connection, once it is all there.
    pub fn server_info(&self, user_id: &str, session_id: Option<&str>) -> Option<VoiceServer> {
        if self.demo {
            return None;
        }
        let (endpoint, token) = self.server.clone()?;
        Some(VoiceServer {
            endpoint,
            token,
            guild_id: self.guild_id.clone(),
            channel_id: self.channel_id.clone(),
            session_id: session_id?.to_string(),
            user_id: user_id.to_string(),
            stream: Some(self.rtc_server_id.clone()?),
        })
    }

    pub fn encrypted(&self) -> bool {
        matches!(self.encryption, Encryption::Active { .. })
    }
}

/// The window to choose what to stream.
#[derive(Default)]
pub struct Picker {
    /// `None` while the list is being made.
    pub sources: Option<Vec<Source>>,
    pub selected: Option<usize>,
    pub error: Option<String>,
}

impl Picker {
    /// The list arrived: the test picture (demo) or the main screen (or the
    /// first source) is selected.
    pub fn set_sources(&mut self, sources: Vec<Source>) {
        self.selected = sources
            .iter()
            .position(|s| s.kind == SourceKind::TestPattern)
            .or_else(|| sources.iter().position(|s| s.primary))
            .or_else(|| (!sources.is_empty()).then_some(0));
        self.sources = Some(sources);
    }

    pub fn source(&self) -> Option<&Source> {
        self.sources.as_ref()?.get(self.selected?)
    }
}

fn source_row(i: usize, s: &Source, selected: bool) -> Element<'_, Message> {
    let kind = match s.kind {
        SourceKind::Screen => "🖥",
        SourceKind::Window => "🗔",
        SourceKind::TestPattern => "🎨",
    };
    let mut name = s.name.clone();
    if s.primary {
        name.push_str(t(" (main)", " (principale)"));
    }
    button(
        row![
            text(kind).size(14).shaping(Shaping::Advanced),
            text(name)
                .size(13)
                .shaping(Shaping::Advanced)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .width(Length::Fill)
    .padding([6, 10])
    .style(list_item(selected))
    .on_press(Message::GoLiveSelect(i))
    .into()
}

pub fn picker_view<'a>(p: &'a Picker, quality: Quality, sound: bool) -> Element<'a, Message> {
    let title = text(t("Stream your screen", "Trasmetti lo schermo"))
        .size(17)
        .font(BOLD);
    let list: Element<'a, Message> = match &p.sources {
        None => text(t(
            "Looking for screens and windows…",
            "Ricerca di schermi e finestre…",
        ))
        .size(13)
        .color(pal::muted())
        .into(),
        Some(sources) if sources.is_empty() => text(t(
            "Nothing to stream was found on this system.",
            "Su questo sistema non c'è niente da trasmettere.",
        ))
        .size(13)
        .color(pal::muted())
        .into(),
        Some(sources) => {
            let mut screens = column![].spacing(2);
            let mut windows = column![].spacing(2);
            for (i, s) in sources.iter().enumerate() {
                let r = source_row(i, s, p.selected == Some(i));
                if s.kind == SourceKind::Window {
                    windows = windows.push(r);
                } else {
                    screens = screens.push(r);
                }
            }
            let heading = |s: &'static str| text(s).size(11).font(BOLD).color(pal::muted());
            let mut col = column![heading(t("SCREENS", "SCHERMI")), screens].spacing(6);
            if sources.iter().any(|s| s.kind == SourceKind::Window) {
                col = col.push(heading(t("WINDOWS", "FINESTRE"))).push(windows);
            }
            scrollable(col.padding([0, 8]))
                .direction(super::thin())
                .height(260)
                .into()
        }
    };
    let window_selected = p.source().is_some_and(|s| s.kind == SourceKind::Window);
    let sound_label = if window_selected {
        t("Stream the program's sound", "Trasmetti l'audio del programma")
    } else {
        t(
            "Stream the PC's sound (not ferrocord's)",
            "Trasmetti l'audio del PC (non quello di ferrocord)",
        )
    };
    let quality_row = row![
        text(t("Quality", "Qualità")).size(13),
        space::horizontal(),
        pick_list(&Quality::ALL[..], Some(quality), Message::GoLiveQuality)
            .text_size(13)
            .padding([4, 8]),
    ]
    .align_y(Vertical::Center);
    let note = text(t(
        "720p at 30 fps is what Discord allows without Nitro. While you stream, \
         \"Live\" stays in the call panel: Windows does not draw its yellow border.",
        "720p a 30 fps è quanto Discord permette senza Nitro. Durante la diretta \
         nel pannello della chiamata resta \"In diretta\": Windows non disegna il bordo giallo.",
    ))
    .size(11)
    .color(pal::muted());
    let start = button(text(t("Go Live", "Vai in diretta")).size(13).font(SEMIBOLD))
        .on_press_maybe(p.source().is_some().then_some(Message::StartGoLive))
        .padding([6, 14])
        .style(primary);
    let cancel = button(text(t("Cancel (Esc)", "Annulla (Esc)")).size(13))
        .on_press(Message::CloseGoLive)
        .padding([6, 12])
        .style(ghost);
    let mut body = column![
        title,
        list,
        quality_row,
        checkbox(sound)
            .label(sound_label)
            .on_toggle(Message::GoLiveSound)
            .text_size(13),
        note,
    ]
    .spacing(12)
    .width(420);
    if let Some(e) = &p.error {
        body = body.push(
            text(e.clone())
                .size(12)
                .color(pal::red())
                .shaping(Shaping::Advanced),
        );
    }
    body = body.push(row![start, space::horizontal(), cancel].spacing(8));
    let card = container(body).padding(16).style(outlined);
    let card = mouse_area(card).on_press(Message::Noop);
    opaque(
        mouse_area(
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .style(backdrop),
        )
        .on_press(Message::CloseGoLive),
    )
}

impl std::fmt::Display for Quality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// "Live" in the call panel: always there while something is captured, with
/// the state of the connection, a small preview and the button to stop.
pub fn live_panel(g: &GoLiveUi) -> Element<'_, Message> {
    let badge = container(
        text(t("LIVE", "IN DIRETTA"))
            .size(10)
            .font(BOLD)
            .color(pal::bg())
            .wrapping(text::Wrapping::None),
    )
    .padding([2, 6])
    .style(rounded(pal::red(), 4.0));
    let state = match g.status {
        Status::Starting => t("Waiting for Discord…", "In attesa di Discord…").to_string(),
        Status::Connecting => t("Connecting the stream…", "Connessione della diretta…").to_string(),
        Status::Connected if !g.encrypted() => t(
            "Setting up end-to-end encryption…",
            "Attivazione della crittografia end-to-end…",
        )
        .to_string(),
        Status::Connected => tf!(
            "🔒 End-to-end encrypted · {} frames",
            "🔒 Crittografia end-to-end · {} fotogrammi",
            g.stats.video_sent
        ),
    };
    // "Stop" in both languages, on one line whatever the width of the column.
    let stop = button(
        text("Stop")
            .size(11)
            .font(SEMIBOLD)
            .wrapping(text::Wrapping::None),
    )
    .on_press(Message::StopGoLive)
    .padding([3, 8])
    .style(primary);
    let mut col = column![
        row![
            badge,
            // The name takes what is left and is cut, never the button.
            container(
                // No-break spaces: the name stays on one line and is cut at
                // the edge instead of going to the next line.
                text(g.source_name.replace(' ', "\u{a0}"))
                    .size(11)
                    .shaping(Shaping::Advanced)
                    .wrapping(text::Wrapping::None),
            )
            .width(Length::Fill)
            .height(16)
            .clip(true),
            stop,
        ]
        .spacing(6)
        .align_y(Vertical::Center),
        text(state)
            .size(10)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
        container(VideoView::new(g.preview.clone()))
            .width(Length::Fill)
            .height(108)
            .style(rounded(pal::bg(), 4.0)),
    ]
    .spacing(4);
    if let Some(e) = &g.sound_error {
        col = col.push(
            text(tf!("No sound: {e}", "Niente audio: {e}"))
                .size(10)
                .color(pal::red())
                .shaping(Shaping::Advanced),
        );
    }
    if g.demo {
        col = col.push(
            text(t(
                "Demo: the stream goes to the local test server, nobody sees it.",
                "Demo: la diretta va al server di prova locale, nessuno la vede.",
            ))
            .size(10)
            .color(pal::muted()),
        );
    }
    container(col).padding([6, 0]).into()
}

/// The sources to offer: the system's, and in the demo the test picture first.
pub fn sources(demo: bool) -> Vec<Source> {
    let mut list = capture::sources();
    if demo {
        list.insert(0, Source::test_pattern());
    }
    list
}

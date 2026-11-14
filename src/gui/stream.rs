//! Go Live streams: the preview card, opened from the LIVE badge next to
//! whoever is streaming in a voice channel, and the small preview shown
//! while the mouse is over them (like Discord's).
//!
//! The preview is a still image (about 512×288) that Discord updates every
//! few minutes while the stream is on. It is asked only when the card is
//! opened or refreshed, or when the mouse goes over someone streaming (at
//! most once a minute per stream), never in the background, and comes from
//! Discord's CDN like avatars.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, horizontal_space, mouse_area, opaque, row, text};
use iced::{ContentFit, Element, Length};

use super::style::*;
use super::{Message, Preview};
use crate::i18n::t;
use crate::store::Store;

/// Size the preview is decoded at (Discord makes them about this big).
pub const PREVIEW_PX: (u32, u32) = (512, 288);
/// The preview can be asked again after this long (Discord updates it only
/// every few minutes, and the endpoint is rate limited).
pub const REFRESH_AFTER: Duration = Duration::from_secs(15);

/// The small preview on hover is asked again only after this long.
pub const HOVER_REFRESH: Duration = Duration::from_secs(60);
/// At most this many small previews are kept (one per stream).
pub const MAX_HOVER_PREVIEWS: usize = 32;

const WIDTH: f32 = 432.0;
const HOVER_IMAGE: (f32, f32) = (256.0, 144.0);
const IMAGE: (f32, f32) = (400.0, 225.0);

/// Whose stream, where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRef {
    pub guild_id: String,
    pub channel_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone)]
pub enum PreviewState {
    Loading,
    /// Address of the image (Discord's CDN).
    Ready(String),
    /// Discord has no preview yet (a new stream, or not shared).
    Unavailable,
    Failed(String),
}

pub struct StreamView {
    pub stream: StreamRef,
    pub state: PreviewState,
    /// When the preview was last asked.
    pub asked: Instant,
}

impl StreamView {
    pub fn new(stream: StreamRef) -> Self {
        Self {
            stream,
            state: PreviewState::Loading,
            asked: Instant::now(),
        }
    }

    pub fn image_url(&self) -> Option<&str> {
        match &self.state {
            PreviewState::Ready(url) => Some(url),
            _ => None,
        }
    }

    pub fn can_refresh(&self) -> bool {
        !matches!(self.state, PreviewState::Loading) && self.asked.elapsed() >= REFRESH_AFTER
    }
}

/// Small preview of a stream, asked when the mouse goes over its streamer.
#[derive(Debug, Clone)]
pub struct HoverPreview {
    pub state: PreviewState,
    pub asked: Instant,
}

impl HoverPreview {
    /// To be asked (again): never asked, or asked long enough ago and not
    /// still on its way.
    pub fn stale(p: Option<&Self>) -> bool {
        p.is_none_or(|p| !matches!(p.state, PreviewState::Loading) && p.asked.elapsed() >= HOVER_REFRESH)
    }

    pub fn image_url(&self) -> Option<&str> {
        match &self.state {
            PreviewState::Ready(url) => Some(url),
            _ => None,
        }
    }
}

/// The small card shown while the mouse is over someone streaming: the
/// preview, who and where, and what a click does.
pub fn hover_card<'a>(
    store: &Store,
    s: &StreamRef,
    preview: Option<&HoverPreview>,
    images: &'a HashMap<String, Preview>,
    in_channel: bool,
) -> Element<'a, Message> {
    let name = store.users.get(&s.user_id).map_or_else(
        || t("someone", "qualcuno").into(),
        |u| store.name_in(Some(&s.guild_id), u),
    );
    let note = |m: &str| -> Element<'a, Message> {
        container(
            text(m.to_string())
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        )
        .width(HOVER_IMAGE.0)
        .height(HOVER_IMAGE.1)
        .padding(10)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style(rounded(pal::bg(), 6.0))
        .into()
    };
    let picture: Element<'a, Message> = match preview.map(|p| &p.state) {
        None | Some(PreviewState::Loading) => note(t("Loading the preview…", "Caricamento dell'anteprima…")),
        Some(PreviewState::Unavailable) => note(t("No preview yet", "Ancora nessuna anteprima")),
        Some(PreviewState::Failed(_)) => note(t("Preview not available", "Anteprima non disponibile")),
        Some(PreviewState::Ready(url)) => match images.get(url) {
            Some(Preview::Ready(h)) => img::Image::new(h.clone())
                .width(HOVER_IMAGE.0)
                .height(HOVER_IMAGE.1)
                .content_fit(ContentFit::Cover)
                .into(),
            Some(Preview::Failed) => note(t("Preview not available", "Anteprima non disponibile")),
            _ => note(t("Loading the preview…", "Caricamento dell'anteprima…")),
        },
    };
    let action = if !store.can_connect(&s.channel_id) {
        t(
            "You cannot join this voice channel",
            "Non puoi entrare in questo canale vocale",
        )
    } else if in_channel {
        t("Click to watch", "Clic per guardare")
    } else {
        t(
            "Click to join the channel and watch",
            "Clic per entrare nel canale e guardare",
        )
    };
    container(
        column![
            picture,
            row![
                container(text("LIVE").size(9).font(BOLD).color(pal::bg()))
                    .padding([1, 5])
                    .style(rounded(pal::red(), 4.0)),
                text(name).size(13).font(SEMIBOLD).shaping(Shaping::Advanced),
            ]
            .spacing(6)
            .align_y(Vertical::Center),
            text(action).size(11).color(pal::muted()),
        ]
        .spacing(6),
    )
    .padding(8)
    .style(super::style::tooltip)
    .into()
}

/// The red LIVE badge; a click opens the preview.
pub fn live_badge<'a>(on_press: Message) -> Element<'a, Message> {
    button(
        container(text("LIVE").size(9).font(BOLD).color(pal::bg()))
            .padding([1, 5])
            .style(rounded(pal::red(), 4.0)),
    )
    .padding(0)
    .style(bare)
    .on_press(on_press)
    .into()
}

/// The card over a dark background; a click outside closes it.
pub fn view<'a>(
    v: &'a StreamView,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    in_channel: bool,
) -> Element<'a, Message> {
    let s = &v.stream;
    let name = store.users.get(&s.user_id).map_or_else(
        || t("someone", "qualcuno").into(),
        |u| store.name_in(Some(&s.guild_id), u),
    );
    let channel = store
        .channel_names
        .get(&s.channel_id)
        .cloned()
        .unwrap_or_else(|| t("voice channel", "canale vocale").into());
    let live = store.streaming.contains(&s.user_id)
        && store
            .voice
            .get(&s.user_id)
            .is_some_and(|(_, c)| *c == s.channel_id);

    let title = row![
        container(text("LIVE").size(11).font(BOLD).color(pal::bg()))
            .padding([2, 6])
            .style(rounded(if live { pal::red() } else { pal::muted() }, 4.0)),
        text(tf!("{name} is streaming", "{name} è in diretta"))
            .size(17)
            .font(BOLD)
            .shaping(Shaping::Advanced),
    ]
    .spacing(10)
    .align_y(Vertical::Center);
    let place = text(tf!("in 🔊 {channel}", "in 🔊 {channel}"))
        .size(12)
        .color(pal::muted())
        .shaping(Shaping::Advanced);

    let message = |m: &str| -> Element<'a, Message> {
        container(
            text(m.to_string())
                .size(13)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        )
        .width(IMAGE.0)
        .height(IMAGE.1)
        .padding(16)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style(rounded(pal::panel(), 8.0))
        .into()
    };
    let picture: Element<'a, Message> = if live {
        match &v.state {
            PreviewState::Loading => message(t("Loading the preview…", "Caricamento dell'anteprima…")),
            PreviewState::Unavailable => message(t(
                "No preview yet: Discord makes one a few minutes after the stream starts.",
                "Ancora nessuna anteprima: Discord la crea qualche minuto dopo l'inizio della diretta.",
            )),
            PreviewState::Failed(e) => message(&tf!(
                "Preview not available ({e}).",
                "Anteprima non disponibile ({e})."
            )),
            PreviewState::Ready(url) => match images.get(url) {
                Some(Preview::Ready(h)) => img::Image::new(h.clone())
                    .width(IMAGE.0)
                    .height(IMAGE.1)
                    .content_fit(ContentFit::Contain)
                    .into(),
                Some(Preview::Failed) => message(t(
                    "The image could not be loaded.",
                    "Impossibile caricare l'immagine.",
                )),
                _ => message(t("Loading the preview…", "Caricamento dell'anteprima…")),
            },
        }
    } else {
        message(t("The stream has ended.", "La diretta è finita."))
    };

    let note = text(t(
        "Still image updated by Discord every few minutes.",
        "Immagine fissa, aggiornata da Discord ogni pochi minuti.",
    ))
    .size(11)
    .color(pal::muted());

    let refresh = button(text(t("Refresh", "Aggiorna")).size(12))
        .on_press_maybe((live && v.can_refresh()).then_some(Message::RefreshStreamPreview))
        .padding([4, 10])
        .style(ghost);
    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseStreamPreview)
        .padding([4, 10])
        .style(ghost);
    // Discord lets you watch only from inside the voice channel (`in_channel`:
    // we are already there): the button says so when it would join it.
    let can_join = store.can_connect(&s.channel_id);
    let watch_label = if in_channel {
        t("Watch", "Guarda")
    } else {
        t("Join the channel and watch", "Entra nel canale e guarda")
    };
    let watch = button(text(watch_label).size(12).font(SEMIBOLD))
        .on_press_maybe((live && can_join).then_some(Message::WatchStream))
        .padding([4, 12])
        .style(primary);

    let body = column![
        column![title, place].spacing(4),
        picture,
        note,
        row![watch, refresh, horizontal_space(), close].spacing(8),
    ]
    .spacing(12)
    .width(IMAGE.0);

    let card = container(body).padding(16).width(WIDTH).style(outlined);
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
        .on_press(Message::CloseStreamPreview),
    )
}

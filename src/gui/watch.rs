//! Watching a Go Live stream.
//!
//! Discord only lets you watch a stream from inside its voice channel, so
//! "Watch" first joins it (microphone off), then:
//!
//! 1. op 20 on the gateway with the stream key;
//! 2. `STREAM_CREATE` (the stream's voice server ID) and
//!    `STREAM_SERVER_UPDATE` (endpoint and token);
//! 3. a second voice connection to that server with video
//!    ([`crate::discord::voice::run_with_video`]); the decoded pictures land
//!    in a [`FrameSlot`] that the [`VideoView`] widget draws.
//!
//! Stopping sends op 19 and closes the second connection; we stay in the
//! voice channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::advanced::image as core_image;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{renderer, Clipboard, Shell, Widget};
use iced::alignment::Vertical;
use iced::widget::image::{self as img, FilterMethod};
use iced::widget::text::Shaping;
use iced::widget::{
    column, container, mouse_area, opaque, row, rule, scrollable, slider, space, stack, text,
};
use iced::{border, mouse, window, Border, ContentFit, Element, Event, Length, Rectangle, Rotation, Size};

use super::icons::{self, icon};
use super::stream::{self as stream_ui, StreamRef};
use super::style::*;
use super::voice::{self as voice_ui, control_button, VoiceUi};
use super::{avatar_with, Message, Preview};
use crate::discord::voice::{VoiceHandle, VoiceServer, VoiceStats, MAX_VOLUME};
use crate::i18n::t;
use crate::store::Store;
use crate::video::decoder::Sink;
use crate::video::Picture;

/// How often the video widget looks for a new picture (about 30 per second).
const POLL: Duration = Duration::from_millis(33);
/// Without a new picture for this many looks (about a second: a still
/// screen share, a paused stream) the widget looks less often, so the
/// window is not redrawn 30 times a second for nothing...
const IDLE_POLLS: u32 = 30;
/// ...but every this often, back to [`POLL`] as soon as a picture comes.
const SLOW_POLL: Duration = Duration::from_millis(100);
/// While the controls show, the mouse is listened to again this long after
/// its last move (not at every move: each one rebuilds the window).
const MOVE_RESAMPLE: Duration = Duration::from_millis(1000);
/// Two clicks on the video closer than this: full screen.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Width of the people column next to the stream.
const MEMBERS_WIDTH: f32 = 240.0;
/// Without moving the mouse for this long, the controls and the cards over
/// the video go away (they come back with the mouse).
pub const OVERLAY_IDLE: Duration = Duration::from_millis(2500);
/// The mini player: the stream in a small window over the rest of Rico.
const MINI_WIDTH: f32 = 400.0;
const MINI_HEIGHT: f32 = 225.0;
/// The other streams of the channel shown over the video (the rest: "+N").
pub const MAX_OTHER_STREAMS: usize = 4;
/// Their small preview.
const OTHER_PREVIEW: (f32, f32) = (192.0, 108.0);

/// The other streams in the watched stream's voice channel, in the order of
/// its people.
pub fn other_streams(w: &WatchUi, store: &Store) -> Vec<StreamRef> {
    let s = &w.stream;
    store
        .voice_members(&s.channel_id)
        .into_iter()
        .filter(|(user, _)| *user != s.user_id && store.streaming.contains(user))
        .map(|(user_id, _)| StreamRef {
            guild_id: s.guild_id.clone(),
            channel_id: s.channel_id.clone(),
            user_id,
        })
        .collect()
}

/// The latest decoded picture, shared between the decoder thread and the widget.
#[derive(Default)]
pub struct FrameSlot {
    picture: Mutex<Option<(img::Handle, u32, u32)>>,
    /// Changes at every new picture.
    seq: AtomicU64,
}

impl FrameSlot {
    /// Where the decoder thread puts its pictures.
    pub fn sink(self: &Arc<Self>) -> Sink {
        let slot = Arc::downgrade(self);
        Box::new(move |p: Picture| {
            let Some(slot) = slot.upgrade() else { return };
            let handle = img::Handle::from_rgba(p.width, p.height, p.rgba);
            if let Ok(mut current) = slot.picture.lock() {
                *current = Some((handle, p.width, p.height));
            }
            slot.seq.fetch_add(1, Ordering::Release);
        })
    }

    fn latest(&self) -> Option<(img::Handle, u32, u32)> {
        self.picture.lock().ok().and_then(|p| p.clone())
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        self.latest().map(|(_, w, h)| (w, h))
    }

    pub fn has_picture(&self) -> bool {
        self.seq.load(Ordering::Acquire) > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Joining the voice channel first.
    JoiningVoice,
    /// Op 20 sent, waiting for Discord.
    Asking,
    Connecting,
    Watching,
    Ended(String),
}

pub struct WatchUi {
    pub stream: StreamRef,
    pub key: String,
    pub status: Status,
    /// Op 20 already sent.
    pub asked: bool,
    pub rtc_server_id: Option<String>,
    /// (endpoint, token) from `STREAM_SERVER_UPDATE`.
    pub server: Option<(String, String)>,
    /// Changes when a new server arrives: restarts the connection.
    pub attempt: u64,
    pub handle: Option<VoiceHandle>,
    pub stats: VoiceStats,
    pub frames: Arc<FrameSlot>,
    pub demo: bool,
    /// Volume of the stream's sound, 0.0 to [`MAX_VOLUME`].
    pub volume: f32,
    /// The stream's sound is muted (the volume is kept for when it comes back).
    pub muted: bool,
    /// The mouse is over the controls: they stay.
    pub controls_shown: bool,
    /// Last click on the video, to tell a double click.
    pub last_click: Option<Instant>,
    /// The cards and controls over the video are shown (the mouse moved
    /// lately).
    pub overlays: bool,
    /// When the mouse last moved over the video.
    pub last_move: Option<Instant>,
    /// Shown as the mini player, in a corner over the rest of Rico.
    pub minimized: bool,
    /// The mouse's moves over the video are listened to (see
    /// [`MOVE_RESAMPLE`]).
    pub track_moves: bool,
}

impl WatchUi {
    pub fn new(stream: StreamRef, key: String, demo: bool, volume: f32) -> Self {
        Self {
            stream,
            key,
            status: Status::JoiningVoice,
            asked: false,
            rtc_server_id: None,
            server: None,
            attempt: 0,
            handle: None,
            stats: VoiceStats::default(),
            frames: Arc::new(FrameSlot::default()),
            demo,
            volume: volume.clamp(0.0, MAX_VOLUME),
            muted: false,
            controls_shown: false,
            last_click: None,
            overlays: true,
            last_move: Some(Instant::now()),
            minimized: false,
            track_moves: false,
        }
    }

    /// The cards and controls over the video are on screen: while the mouse
    /// moves or is over the controls, and always until the picture comes.
    pub fn overlays_visible(&self) -> bool {
        self.overlays || self.controls_shown || self.status != Status::Watching || !self.frames.has_picture()
    }

    /// The mouse moved over the video: the controls come back.
    pub fn mouse_moved(&mut self, now: Instant) {
        self.overlays = true;
        self.last_move = Some(now);
        self.track_moves = false;
    }

    /// Checked by a timer while the controls are shown: they go away after
    /// [`OVERLAY_IDLE`] without moving the mouse (not while it is over them).
    pub fn hide_if_idle(&mut self, now: Instant) {
        let still = self.last_move.map_or(Duration::MAX, |t| now.duration_since(t));
        if still >= MOVE_RESAMPLE {
            self.track_moves = true;
        }
        if !self.controls_shown && still >= OVERLAY_IDLE {
            self.overlays = false;
            self.track_moves = true;
        }
    }

    /// The timer that hides the controls is needed: they are shown, the
    /// mouse is not over them, and the stream is not in the mini player.
    pub fn needs_idle_timer(&self) -> bool {
        !self.minimized && self.overlays && !self.controls_shown
    }

    /// The volume the stream connection plays at.
    pub fn effective_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }

    /// Sends the volume to the stream connection (if it is there yet).
    pub fn apply_volume(&self) {
        if let Some(h) = &self.handle {
            h.set_volume(self.effective_volume());
        }
    }

    /// Changes the volume by `steps` of 10%, unmuting.
    pub fn step_volume(&mut self, steps: i32) {
        self.set_volume(self.volume + steps as f32 * 0.1);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = ((volume * 20.0).round() / 20.0).clamp(0.0, MAX_VOLUME);
        self.muted = false;
        self.apply_volume();
    }

    /// Everything needed for the stream connection, once it is all there.
    pub fn server_info(&self, user_id: &str, session_id: Option<&str>) -> Option<VoiceServer> {
        if matches!(self.status, Status::Ended(_)) || self.demo {
            return None;
        }
        let (endpoint, token) = self.server.clone()?;
        Some(VoiceServer {
            endpoint,
            token,
            guild_id: self.stream.guild_id.clone(),
            channel_id: self.stream.channel_id.clone(),
            session_id: session_id?.to_string(),
            user_id: user_id.to_string(),
            stream: Some(self.rtc_server_id.clone()?),
        })
    }
}

/// How the stream window is laid out.
#[derive(Debug, Clone, Copy)]
pub struct WindowLayout<'a> {
    /// The window is full screen (the same view, without the window frame).
    pub fullscreen: bool,
    /// The people of the channel on the left.
    pub members: bool,
    /// Mutes of people, only for us (marked in the list; their volume is in
    /// their right-click menu).
    pub audio: &'a crate::audio::AudioSettings,
}

/// The stream window, "theatre" style: the video fills the window; over it,
/// who is streaming (top left), the channel's people and how the stream is
/// going (top right) and the controls floating at the bottom (microphone,
/// sound, stream volume, people, mini player, full screen, stop watching,
/// leave the channel). The cards and controls go away when the mouse stays
/// still and come back when it moves. The people of the channel can also
/// be shown in a column on the left.
/// `voice`: our voice channel connection; `can_speak`: we may talk there.
pub fn view<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    voice: Option<&'a VoiceUi>,
    can_speak: bool,
    images: &'a HashMap<String, Preview>,
    hover: &'a HashMap<String, stream_ui::HoverPreview>,
    layout: WindowLayout<'a>,
) -> Element<'a, Message> {
    let video = mouse_area(
        container(VideoView::new(w.frames.clone()))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(iced::Color::BLACK)),
    )
    .on_press(Message::VideoPressed);
    // Moves are listened to only when they change something (see
    // `track_moves`): otherwise each one would rebuild the window.
    let video = if w.track_moves {
        video.on_move(|_| Message::WatchMouseMoved)
    } else {
        video
    };

    let mut stage = stack![video];
    if w.overlays_visible() {
        let controls = mouse_area(controls(w, voice, can_speak, layout.fullscreen, layout.members))
            .on_enter(Message::WatchControls(true))
            .on_exit(Message::WatchControls(false));
        let mut right = column![channel_card(w, store, images)]
            .spacing(10)
            .align_x(iced::alignment::Horizontal::Right);
        if let Some(others) = others_strip(w, store, images, hover) {
            right = right.push(others);
        }
        stage = stage.push(
            column![
                row![who_card(w, store, images), space::horizontal(), right].align_y(Vertical::Top),
                space::vertical(),
                row![space::horizontal(), controls, space::horizontal()],
            ]
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fill),
        );
    }

    let mut page = row![].height(Length::Fill);
    if layout.members {
        page = page.push(members(w, store, voice, images, hover, layout.audio));
    }
    page = page.push(container(stage).width(Length::Fill).height(Length::Fill));
    opaque(
        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(iced::Color::BLACK)),
    )
}

/// The name of who is streaming in this server.
fn streamer_name(w: &WatchUi, store: &Store) -> String {
    let s = &w.stream;
    store.users.get(&s.user_id).map_or_else(
        || t("someone", "qualcuno").into(),
        |u| store.name_in(Some(&s.guild_id), u),
    )
}

fn channel_name(w: &WatchUi, store: &Store) -> String {
    store
        .channel_names
        .get(&w.stream.channel_id)
        .cloned()
        .unwrap_or_default()
}

/// What the stream is doing, for the cards: the connection steps, then the
/// picture size and counts.
fn state_line(w: &WatchUi) -> String {
    match &w.status {
        Status::JoiningVoice => t("Joining the voice channel…", "Ingresso nel canale vocale…").to_string(),
        Status::Asking => t(
            "Asking Discord for the stream…",
            "Richiesta della diretta a Discord…",
        )
        .to_string(),
        Status::Connecting => t("Connecting to the stream…", "Connessione alla diretta…").to_string(),
        Status::Watching if !w.frames.has_picture() => t(
            "Waiting for the first picture…",
            "In attesa della prima immagine…",
        )
        .to_string(),
        Status::Watching => match w.frames.size() {
            Some((width, height)) => tf!(
                "{width}×{height} · {} pictures · {} dropped",
                "{width}×{height} · {} immagini · {} scartate",
                w.stats.video_frames,
                w.stats.video_dropped
            ),
            None => String::new(),
        },
        Status::Ended(reason) => reason.clone(),
    }
}

fn live_badge<'a>(w: &WatchUi, size: u32) -> Element<'a, Message> {
    container(text("LIVE").size(size).font(BOLD).color(pal::bg()))
        .padding([1, 5])
        .style(rounded(
            if w.status == Status::Watching {
                pal::red()
            } else {
                pal::muted()
            },
            4.0,
        ))
        .into()
}

/// Card over the video: who is streaming and where.
fn who_card<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let s = &w.stream;
    let name = streamer_name(w, store);
    let image = store
        .avatar_url(Some(&s.guild_id), &s.user_id, super::AVATAR_PX)
        .and_then(|a| images.get(&a));
    container(
        row![
            avatar_with(&name, &s.user_id, 34.0, image),
            column![
                row![
                    text(name.clone()).size(15).font(BOLD).shaping(Shaping::Advanced),
                    live_badge(w, 10)
                ]
                .spacing(8)
                .align_y(Vertical::Center),
                text(tf!("in 🔊 {}", "in 🔊 {}", channel_name(w, store)))
                    .size(12)
                    .color(pal::muted())
                    .shaping(Shaping::Advanced),
            ]
            .spacing(2),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding([8, 14])
    .style(rounded(overlay_bg(), 10.0))
    .into()
}

/// Card over the video: the channel's people (small faces) and how the
/// stream is going.
fn channel_card<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let s = &w.stream;
    let people = store.voice_members(&s.channel_id);
    let mut faces = row![].spacing(-6.0_f32);
    for (user_id, name) in people.iter().take(5) {
        let image = store
            .avatar_url(Some(&s.guild_id), user_id, super::AVATAR_PX)
            .and_then(|a| images.get(&a));
        faces = faces.push(avatar_with(name, user_id, 24.0, image));
    }
    container(
        row![
            faces,
            text(tf!("{} in the channel", "{} nel canale", people.len()))
                .size(12)
                .shaping(Shaping::Advanced),
            container(rule::vertical(1)).height(16),
            text(state_line(w))
                .size(12)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding([8, 12])
    .style(rounded(overlay_bg(), 10.0))
    .into()
}

/// The other streams of the channel, over the right of the video: their
/// preview (Discord's still picture, refreshed every minute), LIVE and
/// name; a click watches that one instead. `None` with no other stream.
fn others_strip<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    hover: &'a HashMap<String, stream_ui::HoverPreview>,
) -> Option<Element<'a, Message>> {
    let others = other_streams(w, store);
    if others.is_empty() {
        return None;
    }
    let mut col = column![text(tf!(
        "Other streams in the channel ({})",
        "Altre dirette nel canale ({})",
        others.len()
    ))
    .size(11)
    .font(BOLD)
    .color(pal::muted())]
    .spacing(8)
    .align_x(iced::alignment::Horizontal::Right);
    for other in others.iter().take(MAX_OTHER_STREAMS) {
        let name = store.users.get(&other.user_id).map_or_else(
            || t("someone", "qualcuno").into(),
            |u| store.name_in(Some(&other.guild_id), u),
        );
        let picture: Element<'a, Message> =
            match crate::media::stream_key(&other.guild_id, &other.channel_id, &other.user_id)
                .and_then(|k| hover.get(&k))
                .and_then(stream_ui::HoverPreview::image_url)
                .and_then(|url| images.get(url))
            {
                Some(Preview::Ready(h)) => img::Image::new(h.clone())
                    .width(OTHER_PREVIEW.0)
                    .height(OTHER_PREVIEW.1)
                    .content_fit(ContentFit::Cover)
                    .into(),
                _ => container(text(t("Preview…", "Anteprima…")).size(11).color(pal::muted()))
                    .width(OTHER_PREVIEW.0)
                    .height(OTHER_PREVIEW.1)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(Vertical::Center)
                    .style(rounded(pal::bg(), 6.0))
                    .into(),
            };
        let card = column![
            picture,
            row![
                container(text("LIVE").size(9).font(BOLD).color(pal::bg()))
                    .padding([1, 5])
                    .style(rounded(pal::red(), 4.0)),
                text(name)
                    .size(12)
                    .font(SEMIBOLD)
                    .shaping(Shaping::Advanced)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(6)
            .align_y(Vertical::Center),
        ]
        .spacing(6);
        col = col.push(
            iced::widget::button(card)
                .padding(6)
                .on_press(Message::WatchStreamOf(other.clone()))
                .style(|_, status| iced::widget::button::Style {
                    background: Some(
                        match status {
                            iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
                                pal::hover()
                            }
                            _ => iced::Color::TRANSPARENT,
                        }
                        .into(),
                    ),
                    border: Border {
                        radius: border::radius(10.0),
                        ..Default::default()
                    },
                    text_color: pal::text(),
                    ..Default::default()
                }),
        );
    }
    if others.len() > MAX_OTHER_STREAMS {
        col = col.push(
            text(tf!(
                "+{} more (people button)",
                "+{} altre (pulsante persone)",
                others.len() - MAX_OTHER_STREAMS
            ))
            .size(11)
            .color(pal::muted()),
        );
    }
    Some(
        container(col)
            .padding(8)
            .style(rounded(overlay_bg(), 10.0))
            .into(),
    )
}

/// The background of the cards over the video: the panel colour, a little
/// see-through.
fn overlay_bg() -> iced::Color {
    iced::Color {
        a: 0.9,
        ..pal::panel()
    }
}

/// The mini player: the stream in a small window in the bottom-right corner,
/// over the rest of Rico (chat, channels), so watching goes on while doing
/// something else. A click on the video makes it big again; × stops watching.
pub fn mini_view<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    voice: Option<&'a VoiceUi>,
    can_speak: bool,
) -> Element<'a, Message> {
    const SMALL: f32 = 32.0;
    let corner = row![
        control_button(
            &icons::FULLSCREEN,
            Control::On,
            t("Make it big again", "Ingrandisci").into(),
            Some(Message::ExpandStream),
            SMALL,
        ),
        control_button(
            &icons::CLOSE,
            Control::On,
            t("Stop watching", "Smetti di guardare").into(),
            Some(Message::StopWatching),
            SMALL,
        ),
    ]
    .spacing(6);
    let video = stack![
        mouse_area(
            container(VideoView::new(w.frames.clone()))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(fill(iced::Color::BLACK)),
        )
        .on_press(Message::ExpandStream),
        column![row![space::horizontal(), corner]].padding(8),
    ];
    let mut bar = row![
        live_badge(w, 9),
        text(streamer_name(w, store))
            .size(13)
            .font(SEMIBOLD)
            .shaping(Shaping::Advanced)
            .wrapping(text::Wrapping::None),
        text(tf!("in {}", "in {}", channel_name(w, store)))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced)
            .wrapping(text::Wrapping::None),
        space::horizontal(),
    ]
    .spacing(8)
    .align_y(Vertical::Center);
    // Other streams in the channel: a hint, a click opens the big view
    // where they are.
    let others = other_streams(w, store).len();
    if others > 0 {
        bar = bar.push(
            iced::widget::button(
                text(tf!("+{others} live", "+{others} live"))
                    .size(11)
                    .font(SEMIBOLD),
            )
            .padding([3, 8])
            .on_press(Message::ExpandStream)
            .style(ghost),
        );
    }
    bar = bar.push(stream_sound_button(w, voice, SMALL));
    if let Some(v) = voice {
        bar = bar.push(voice_ui::mic_button(v, can_speak, SMALL));
    }
    let player = column![
        container(video).width(MINI_WIDTH).height(MINI_HEIGHT),
        container(bar).padding([8, 10]).width(MINI_WIDTH),
    ];
    let framed = container(player)
        .width(MINI_WIDTH)
        .clip(true)
        .style(|_| container::Style {
            background: Some(pal::panel().into()),
            border: Border {
                color: pal::border(),
                width: 1.0,
                radius: border::radius(12.0),
            },
            ..Default::default()
        });
    // In the corner, above the message field and the status bar; the rest
    // of the window stays usable.
    column![space::vertical(), row![space::horizontal(), opaque(framed)]]
        .padding(iced::Padding {
            top: 0.0,
            right: 20.0,
            bottom: 100.0,
            left: 0.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The people of the stream's voice channel: avatar (green ring while
/// speaking), name, muted microphone or sound, LIVE for who is streaming (the
/// mouse over them shows their small preview, a click watches them instead),
/// and their menu on a right click (their volume for us is there).
fn members<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    voice: Option<&'a VoiceUi>,
    images: &'a HashMap<String, Preview>,
    hover: &'a HashMap<String, stream_ui::HoverPreview>,
    audio: &'a crate::audio::AudioSettings,
) -> Element<'a, Message> {
    let s = &w.stream;
    let muted_for_us = &audio.muted_users;
    let people = store.voice_members(&s.channel_id);
    let channel = store
        .channel_names
        .get(&s.channel_id)
        .cloned()
        .unwrap_or_default();
    let header = column![
        text(t("IN THE CHANNEL", "NEL CANALE"))
            .size(11)
            .font(BOLD)
            .color(pal::muted()),
        row![
            text(format!("🔊 {channel}"))
                .size(14)
                .font(SEMIBOLD)
                .shaping(Shaping::Advanced),
            space::horizontal(),
            text(people.len().to_string()).size(12).color(pal::muted()),
        ]
        .align_y(Vertical::Center),
    ]
    .spacing(4);

    let speaking = |id: &str| voice.is_some_and(|v| v.channel_id == s.channel_id && v.speaking.contains(id));
    let mut list = column![].spacing(2);
    for (user_id, name) in people {
        let talking = speaking(&user_id);
        let image = store
            .avatar_url(Some(&s.guild_id), &user_id, super::AVATAR_PX)
            .and_then(|a| images.get(&a));
        let ring = if talking {
            pal::green()
        } else {
            iced::Color::TRANSPARENT
        };
        let face = container(avatar_with(&name, &user_id, 30.0, image))
            .padding(2)
            .style(move |_| container::Style {
                border: Border {
                    color: ring,
                    width: 2.0,
                    radius: border::radius(17.0),
                },
                ..Default::default()
            });
        let label = text(name)
            .size(13)
            .font(if talking { SEMIBOLD } else { iced::Font::DEFAULT })
            .color(if talking { pal::text() } else { pal::muted() })
            .shaping(Shaping::Advanced)
            .wrapping(text::Wrapping::None);
        let mut line = row![face, container(label).width(Length::Fill).clip(true)]
            .spacing(8)
            .align_y(Vertical::Center);
        let flags = store.voice_status.get(&user_id).copied().unwrap_or_default();
        if let Some(m) = super::voice::marks(flags, muted_for_us.contains(&user_id), 15.0) {
            line = line.push(m);
        }
        let watched = user_id == s.user_id;
        let other = (!watched && store.streaming.contains(&user_id)).then(|| StreamRef {
            guild_id: s.guild_id.clone(),
            channel_id: s.channel_id.clone(),
            user_id: user_id.clone(),
        });
        if watched {
            line = line.push(
                container(
                    text(t("WATCHING", "IN VISIONE"))
                        .size(9)
                        .font(BOLD)
                        .color(pal::bg()),
                )
                .padding([1, 5])
                .style(rounded(pal::accent(), 4.0)),
            );
        } else if let Some(other) = &other {
            line = line.push(stream_ui::live_badge(Message::WatchStreamOf(other.clone())));
        }
        // Someone else streaming: small preview on hover, a click watches.
        let entry: Element<'a, Message> = if let Some(other) = other {
            let preview = crate::media::stream_key(&other.guild_id, &other.channel_id, &other.user_id)
                .and_then(|k| hover.get(&k));
            let card = stream_ui::hover_card(store, &other, preview, images, true);
            iced::widget::tooltip(
                mouse_area(
                    iced::widget::button(line)
                        .width(Length::Fill)
                        .padding([4, 8])
                        .style(list_item(false))
                        .on_press(Message::WatchStreamOf(other.clone())),
                )
                .on_enter(Message::HoverStream(other)),
                card,
                iced::widget::tooltip::Position::Right,
            )
            .gap(8)
            .into()
        } else {
            let row_box = container(line).width(Length::Fill).padding([4, 8]);
            if watched {
                row_box.style(rounded(pal::selected(), 6.0)).into()
            } else {
                row_box.into()
            }
        };
        let menu_user = user_id.clone();
        let menu_guild = s.guild_id.clone();
        let entry = super::context::RightClick::new(entry, move |at| Message::OpenUserMenu {
            user_id: menu_user.clone(),
            guild_id: Some(menu_guild.clone()),
            at,
        });
        list = list.push(entry);
    }

    container(column![header, scrollable(list).height(Length::Fill)].spacing(12))
        .padding([14, 10])
        .width(MEMBERS_WIDTH)
        .height(Length::Fill)
        .style(fill(pal::panel()))
        .into()
}

const CONTROL: f32 = 40.0;

/// The stream's sound: on or muted (off when the whole call's sound is off).
fn stream_sound_button<'a>(w: &WatchUi, voice: Option<&VoiceUi>, size: f32) -> Element<'a, Message> {
    let deafened = voice.is_some_and(|v| v.deafened);
    let (image, kind, tip) = if w.muted || deafened || w.volume == 0.0 {
        (
            &icons::VOLUME_OFF,
            Control::Off,
            if deafened {
                t(
                    "Sound is off for the whole call",
                    "L'audio è disattivato per tutta la chiamata",
                )
            } else {
                t(
                    "Stream muted: click to hear it",
                    "Diretta muta: clic per sentirla",
                )
            },
        )
    } else {
        (
            if w.volume < 0.5 {
                &icons::VOLUME_LOW
            } else {
                &icons::VOLUME
            },
            Control::On,
            t("Click to mute the stream", "Clic per silenziare la diretta"),
        )
    };
    control_button(
        image,
        kind,
        tip.into(),
        (!deafened).then_some(Message::ToggleStreamSound),
        size,
    )
}

/// The controls floating over the bottom of the video.
fn controls<'a>(
    w: &'a WatchUi,
    voice: Option<&'a VoiceUi>,
    can_speak: bool,
    fullscreen: bool,
    members_shown: bool,
) -> Element<'a, Message> {
    let separator = || container(rule::vertical(1)).height(28);
    let mut bar = row![].spacing(8).align_y(Vertical::Center);
    if let Some(v) = voice {
        bar = bar.push(voice_ui::mic_button(v, can_speak, CONTROL));
        bar = bar.push(voice_ui::sound_button(v, CONTROL));
        bar = bar.push(separator());
    }

    // Stream volume: speaker (mute), slider, percentage.
    let percent = (w.volume * 100.0).round() as u32;
    let shown = if w.muted { 0.0 } else { w.volume * 100.0 };
    let volume_slider = slider(0.0..=MAX_VOLUME * 100.0, shown, Message::StreamVolume)
        .step(5.0_f32)
        .width(120);
    let level = text(if w.muted {
        t("muted", "muto").to_string()
    } else {
        format!("{percent}%")
    })
    .size(12)
    .width(40)
    .color(if percent > 100 && !w.muted {
        pal::yellow()
    } else {
        pal::muted()
    });
    bar = bar.push(stream_sound_button(w, voice, CONTROL));
    bar = bar.push(tooltip_hint(
        volume_slider.into(),
        t(
            "Stream volume (↑ ↓ to change it, up to 200%)",
            "Volume della diretta (↑ ↓ per cambiarlo, fino al 200%)",
        ),
    ));
    bar = bar.push(level);
    bar = bar.push(separator());

    bar = bar.push(control_button(
        &icons::PEOPLE,
        Control::On,
        if members_shown {
            t("Hide the people of the channel", "Nascondi le persone del canale")
        } else {
            t("Show the people of the channel", "Mostra le persone del canale")
        }
        .into(),
        Some(Message::ToggleWatchMembers),
        CONTROL,
    ));
    bar = bar.push(control_button(
        &icons::MINI_PLAYER,
        Control::On,
        t(
            "Mini player: keep watching in a corner (Esc)",
            "Mini player: continua a guardare in un angolo (Esc)",
        )
        .into(),
        Some(Message::MinimizeStream),
        CONTROL,
    ));
    bar = bar.push(control_button(
        if fullscreen {
            &icons::FULLSCREEN_EXIT
        } else {
            &icons::FULLSCREEN
        },
        Control::On,
        if fullscreen {
            t(
                "Leave full screen (F11 or Esc)",
                "Esci dallo schermo intero (F11 o Esc)",
            )
        } else {
            t(
                "Full screen (F11 or double click)",
                "Schermo intero (F11 o doppio clic)",
            )
        }
        .into(),
        Some(Message::ToggleFullscreen),
        CONTROL,
    ));
    // Stop watching: the main way out, with its words.
    let stop = iced::widget::button(
        row![
            icon(&icons::EXIT, 18.0, pal::bg()),
            text(t("Stop watching", "Smetti di guardare"))
                .size(13)
                .font(SEMIBOLD),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding([10, 16])
    .on_press(Message::StopWatching)
    .style(control(Control::Leave));
    bar = bar.push(stop);
    if voice.is_some() {
        bar = bar.push(voice_ui::hang_up_button(CONTROL));
    }

    container(bar)
        .padding([8, 10])
        .style(|_| container::Style {
            background: Some(overlay_bg().into()),
            border: Border {
                color: pal::border(),
                width: 1.0,
                radius: border::radius(28.0),
            },
            ..Default::default()
        })
        .into()
}

fn tooltip_hint<'a>(content: Element<'a, Message>, tip: &str) -> Element<'a, Message> {
    iced::widget::tooltip(
        content,
        container(text(tip.to_string()).size(11).shaping(Shaping::Advanced))
            .padding([4, 8])
            .style(super::style::tooltip),
        iced::widget::tooltip::Position::Top,
    )
    .gap(6)
    .into()
}

/// Draws the latest picture of a [`FrameSlot`], fitted to its space. While
/// on screen it looks for a new picture about 30 times a second (a redraw,
/// never a rebuild of the interface).
pub struct VideoView {
    frames: Arc<FrameSlot>,
}

impl VideoView {
    pub fn new(frames: Arc<FrameSlot>) -> Self {
        Self { frames }
    }
}

/// How many times the video's state was rebuilt while showing a stream.
static VIEW_RESETS: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct ViewState {
    seq: u64,
    /// Looks in a row without a new picture.
    idle_polls: u32,
    picture: Option<img::Handle>,
    /// Keeps the shown picture in the GPU memory: with it iced draws the
    /// picture in the next frame for sure.
    allocation: Option<core_image::Allocation>,
}

/// When to look for a picture again: often while pictures come, less
/// after about a second without one.
pub(super) fn next_poll(idle_polls: u32) -> Duration {
    if idle_polls >= IDLE_POLLS {
        SLOW_POLL
    } else {
        POLL
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for VideoView
where
    Renderer: core_image::Renderer<Handle = img::Handle>,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ViewState>()
    }

    fn state(&self) -> tree::State {
        // A new state while a stream is already showing means iced rebuilt
        // this part of the window: counted in the log (powers of two only).
        if self.frames.has_picture() {
            let n = VIEW_RESETS.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                tracing::info!("stream video: view rebuilt {n} times");
            }
        }
        // Starts from the latest picture: never black while it waits for the next.
        let picture = self.frames.latest().map(|(h, _, _)| h);
        tree::State::new(ViewState {
            seq: self.frames.seq.load(Ordering::Acquire),
            idle_polls: 0,
            picture,
            allocation: None,
        })
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let now = *now;
        let state = tree.state.downcast_mut::<ViewState>();
        let seq = self.frames.seq.load(Ordering::Acquire);
        if seq == state.seq {
            state.idle_polls = state.idle_polls.saturating_add(1);
        } else {
            state.seq = seq;
            state.idle_polls = 0;
            if let Some((handle, _, _)) = self.frames.latest() {
                // iced 0.14 uploads new images in the background and draws
                // nothing until they are ready: with a new picture every
                // frame the video went black now and then. The picture is
                // uploaded here (a few MB, about a millisecond) and kept; if
                // that fails the previous one stays on screen.
                let started = std::time::Instant::now();
                match renderer.load_image(&handle) {
                    Ok(allocation) => {
                        state.picture = Some(handle);
                        state.allocation = Some(allocation);
                    }
                    Err(e) => tracing::debug!("stream video: picture not uploaded: {e}"),
                }
                let took = started.elapsed();
                if took > Duration::from_millis(8) {
                    tracing::debug!("stream video: upload took {} ms", took.as_millis());
                }
            }
        }
        shell.request_redraw_at(window::RedrawRequest::At(now + next_poll(state.idle_polls)));
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<ViewState>();
        // Without a picture of its own (just rebuilt), the latest one.
        let latest;
        let handle = match &state.picture {
            Some(h) => h,
            None => match self.frames.latest() {
                Some((h, _, _)) => {
                    latest = h;
                    &latest
                }
                None => return,
            },
        };
        img::draw(
            renderer,
            layout,
            handle,
            None,
            border::Radius::default(),
            ContentFit::Contain,
            FilterMethod::Linear,
            Rotation::default(),
            1.0,
            1.0,
        );
    }
}

impl<'a, Message: 'a, Theme: 'a, Renderer> From<VideoView> for Element<'a, Message, Theme, Renderer>
where
    Renderer: core_image::Renderer<Handle = img::Handle> + 'a,
{
    fn from(widget: VideoView) -> Self {
        Element::new(widget)
    }
}

/// Moving test picture for the demo (no network, no decoder): a bar that
/// slides over a gradient, 30 times a second, while the slot is alive.
pub fn demo_feed(frames: &Arc<FrameSlot>) {
    let sink_slot = Arc::downgrade(frames);
    let _ = std::thread::Builder::new()
        .name("demo-video".into())
        .spawn(move || {
            let (w, h) = (640u32, 360u32);
            let mut n = 0u32;
            loop {
                let Some(slot) = sink_slot.upgrade() else { return };
                let mut sink = slot.sink();
                drop(slot);
                let x0 = (n * 8) % w;
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let bar = x >= x0 && x < x0 + 60;
                        let (r, g, b) = if bar {
                            (222, 165, 96)
                        } else {
                            ((x * 120 / w) as u8 + 30, (y * 90 / h) as u8 + 40, 110)
                        };
                        rgba.extend_from_slice(&[r, g, b, 255]);
                    }
                }
                sink(Picture {
                    width: w,
                    height: h,
                    rgba,
                });
                n = n.wrapping_add(1);
                std::thread::sleep(POLL);
            }
        });
}

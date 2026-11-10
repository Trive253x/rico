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
    column, container, horizontal_space, mouse_area, opaque, row, scrollable, slider, stack, text,
    vertical_rule, vertical_space, Space,
};
use iced::{
    border, event, mouse, window, Border, ContentFit, Element, Event, Length, Rectangle, Rotation, Size,
};

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
/// Two clicks on the video closer than this: full screen.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Width of the people column next to the stream.
const MEMBERS_WIDTH: f32 = 240.0;
/// Height of the area at the bottom of the full-screen video that shows the
/// controls when the mouse is over it.
const CONTROLS_AREA: f32 = 96.0;

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
    /// Full screen: the mouse is over the controls' area, so they show.
    pub controls_shown: bool,
    /// Last click on the video, to tell a double click.
    pub last_click: Option<Instant>,
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
        }
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
pub struct WindowLayout {
    /// Full screen: only the video, controls when the mouse goes to the bottom.
    pub fullscreen: bool,
    /// The people of the channel on the left.
    pub members: bool,
}

/// Stream window, like Discord's: the people of the voice channel on the
/// left (who speaks, who is muted, who is streaming), who is streaming on
/// top, the video, the controls below (microphone, sound, stream volume,
/// full screen, stop watching, leave the channel). In full screen only the
/// video, with the controls when the mouse is at the bottom.
/// `voice`: our voice channel connection; `can_speak`: we may talk there.
pub fn view<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    voice: Option<&'a VoiceUi>,
    can_speak: bool,
    images: &'a HashMap<String, Preview>,
    layout: WindowLayout,
) -> Element<'a, Message> {
    let video = mouse_area(
        container(VideoView::new(w.frames.clone()))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(iced::Color::BLACK)),
    )
    .on_press(Message::VideoPressed);

    if layout.fullscreen {
        // Controls only while the mouse is over the bottom of the screen.
        let bottom: Element<'a, Message> = if w.controls_shown {
            container(controls(w, voice, can_speak, true))
                .padding(16)
                .width(Length::Fill)
                .into()
        } else {
            Space::new(Length::Fill, CONTROLS_AREA).into()
        };
        return opaque(
            container(stack![
                video,
                column![
                    vertical_space(),
                    mouse_area(bottom)
                        .on_enter(Message::WatchControls(true))
                        .on_exit(Message::WatchControls(false)),
                ]
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(iced::Color::BLACK)),
        );
    }

    let main = column![
        top_bar(w, store, layout.members),
        container(video).width(Length::Fill).height(Length::Fill),
        controls(w, voice, can_speak, false),
    ]
    .spacing(10)
    .padding(14)
    .width(Length::Fill)
    .height(Length::Fill);

    let mut page = row![].height(Length::Fill);
    if layout.members {
        page = page.push(members(w, store, voice, images));
    }
    page = page.push(main);
    opaque(
        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(pal::bg())),
    )
}

/// Who is streaming, where, and how the stream is going.
fn top_bar<'a>(w: &'a WatchUi, store: &'a Store, members_shown: bool) -> Element<'a, Message> {
    let s = &w.stream;
    let name = store
        .users
        .get(&s.user_id)
        .map(|u| store.name_in(Some(&s.guild_id), u))
        .unwrap_or_else(|| t("someone", "qualcuno").into());
    let channel = store
        .channel_names
        .get(&s.channel_id)
        .cloned()
        .unwrap_or_default();

    let state = match &w.status {
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
    };

    let people = control_button(
        &icons::PEOPLE,
        Control::On,
        if members_shown {
            t("Hide the people of the channel", "Nascondi le persone del canale")
        } else {
            t("Show the people of the channel", "Mostra le persone del canale")
        }
        .into(),
        Some(Message::ToggleWatchMembers),
        32.0,
    );

    row![
        people,
        container(text("LIVE").size(11).font(BOLD).color(pal::bg()))
            .padding([2, 6])
            .style(rounded(
                if w.status == Status::Watching {
                    pal::red()
                } else {
                    pal::muted()
                },
                4.0
            )),
        text(name).size(15).font(BOLD).shaping(Shaping::Advanced),
        text(tf!("in 🔊 {channel}", "in 🔊 {channel}"))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
        horizontal_space(),
        text(state)
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
    ]
    .spacing(12)
    .align_y(Vertical::Center)
    .into()
}

/// The people of the stream's voice channel: avatar (green ring while
/// speaking), name, muted microphone or sound, LIVE for who is streaming (a
/// click watches them instead).
fn members<'a>(
    w: &'a WatchUi,
    store: &'a Store,
    voice: Option<&'a VoiceUi>,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let s = &w.stream;
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
            horizontal_space(),
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
        let (muted, deafened) = store.voice_status.get(&user_id).copied().unwrap_or_default();
        if muted {
            line = line.push(icon(&icons::MIC_OFF, 15.0, pal::red()));
        }
        if deafened {
            line = line.push(icon(&icons::HEADPHONES_OFF, 15.0, pal::red()));
        }
        let watched = user_id == s.user_id;
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
        } else if store.streaming.contains(&user_id) {
            line = line.push(stream_ui::live_badge(Message::WatchStreamOf(StreamRef {
                guild_id: s.guild_id.clone(),
                channel_id: s.channel_id.clone(),
                user_id: user_id.clone(),
            })));
        }
        let row_box = container(line).width(Length::Fill).padding([4, 8]);
        list = list.push(if watched {
            row_box.style(rounded(pal::selected(), 6.0))
        } else {
            row_box
        });
    }

    container(column![header, scrollable(list).height(Length::Fill)].spacing(12))
        .padding([14, 10])
        .width(MEMBERS_WIDTH)
        .height(Length::Fill)
        .style(fill(pal::panel()))
        .into()
}

const CONTROL: f32 = 40.0;

/// The bar under the video.
fn controls<'a>(
    w: &'a WatchUi,
    voice: Option<&'a VoiceUi>,
    can_speak: bool,
    fullscreen: bool,
) -> Element<'a, Message> {
    let mut bar = row![].spacing(10).align_y(Vertical::Center);
    if let Some(v) = voice {
        bar = bar.push(voice_ui::mic_button(v, can_speak, CONTROL));
        bar = bar.push(voice_ui::sound_button(v, CONTROL));
        bar = bar.push(container(vertical_rule(1)).height(24));
    }

    // Stream volume: speaker (mute), slider, percentage.
    let deafened = voice.is_some_and(|v| v.deafened);
    let percent = (w.volume * 100.0).round() as u32;
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
    let speaker = control_button(
        image,
        kind,
        tip.into(),
        (!deafened).then_some(Message::ToggleStreamSound),
        CONTROL,
    );
    let shown = if w.muted { 0.0 } else { w.volume * 100.0 };
    let volume_slider = slider(0.0..=MAX_VOLUME * 100.0, shown, Message::StreamVolume)
        .step(5.0_f32)
        .width(160);
    let level = text(if w.muted {
        t("muted", "muto").to_string()
    } else {
        format!("{percent}%")
    })
    .size(12)
    .width(44)
    .color(if percent > 100 && !w.muted {
        pal::yellow()
    } else {
        pal::muted()
    });
    bar = bar.push(speaker);
    bar = bar.push(tooltip_hint(
        volume_slider.into(),
        t(
            "Stream volume (↑ ↓ to change it, up to 200%)",
            "Volume della diretta (↑ ↓ per cambiarlo, fino al 200%)",
        ),
    ));
    bar = bar.push(level);

    bar = bar.push(horizontal_space());
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
            text(t("Stop watching (Esc)", "Esci dalla diretta (Esc)"))
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
        .padding([8, 12])
        .width(Length::Fill)
        .style(rounded(pal::panel(), 12.0))
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

#[derive(Default)]
struct ViewState {
    seq: u64,
    picture: Option<img::Handle>,
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
        tree::State::new(ViewState::default())
    }

    fn layout(&self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return event::Status::Ignored;
        };
        let state = tree.state.downcast_mut::<ViewState>();
        let seq = self.frames.seq.load(Ordering::Acquire);
        if seq != state.seq {
            state.seq = seq;
            state.picture = self.frames.latest().map(|(h, _, _)| h);
        }
        shell.request_redraw(window::RedrawRequest::At(now + POLL));
        event::Status::Ignored
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
        let Some(handle) = &state.picture else { return };
        img::draw(
            renderer,
            layout,
            handle,
            ContentFit::Contain,
            FilterMethod::Linear,
            Rotation::default(),
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

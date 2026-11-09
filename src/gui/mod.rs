//! Graphical interface (iced), with the layout of the "iced in gpui style" mock-up.
//!
//! Elm architecture: the state is [`Gui`], every event (click, key, network
//! answer, gateway event) is a [`Message`] applied by `update`, and `view`
//! rebuilds the interface from the state. Gateway and QR login run as
//! subscriptions: iced starts and stops them by itself when the state changes.

mod anim;
mod folders;
mod icons;
mod picker;
mod profile;
mod settings;
mod stream;
mod style;
mod voice;
mod watch;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate};
use futures_util::{SinkExt, Stream};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, opaque, qr_code, rich_text, row, scrollable,
    span, stack, text, text_input, tooltip, Column, Row, Space,
};
use iced::{event, keyboard, mouse, window, ContentFit, Element, Length, Padding, Size, Subscription, Task};

use crate::audio::AudioSettings;
use crate::discord::gateway::{self, GatewayEvent, GatewayHandle};
use crate::discord::models::{
    kind, Attachment, GifResult, Message as DMessage, User, UserProfile, VoiceState,
};
use crate::discord::remote_auth::{self, AuthEvent};
use crate::discord::rest::{self, Rest, Unauthorized};
use crate::discord::voice::{self as voice_net, VoiceEvent};
use crate::i18n::{self, t};
use crate::settings::{Settings, ThemeChoice};
use crate::store::{first_selectable, ChannelRow, Store};
use crate::text::{initials, segments, Ctx, Segment, SegmentKind};
use crate::{config, media, permissions, Options};
use anim::{animated, Animation};
use picker::{Gifs, Picker};
use style::*;

/// Most previews kept in memory (the oldest are dropped). They stay
/// compressed (a few tens of KB each): iced decodes only those actually
/// drawn.
const MAX_PREVIEWS: usize = 400;
/// Preview downloads running at the same time.
const MAX_DOWNLOADS: usize = 6;
/// Images of the open channel (newest first) whose previews are downloaded.
/// Must stay below `MAX_PREVIEWS`, otherwise the cache would drop previews
/// that would be requested again right away.
const MAX_IMAGES_PER_CHANNEL: usize = 300;
/// Small images (server emojis, avatars, server icons, banners) kept in
/// memory and downloaded together.
const MAX_SMALL_IMAGES: usize = 800;
/// Profiles kept for the session (they are asked again after that).
const MAX_PROFILES: usize = 200;
/// Size at which avatars and server icons are downloaded and cut (pixels):
/// enough for high density screens at the sizes they are shown.
const AVATAR_PX: u32 = 64;
const ICON_PX: u32 = 96;
const MAX_SMALL_DOWNLOADS: usize = 8;
/// Pictures of private conversations downloaded for their list.
const MAX_DM_PICTURES: usize = 100;
/// Size of those pictures next to the conversation's name.
const DM_AVATAR: f32 = 20.0;
/// Space between the avatar and the text of a message.
const AUTHOR_GAP: f32 = 8.0;
/// Space on the sides of the author's name, highlighted on hover.
const NAME_PAD: f32 = 4.0;
/// Width of the time column in compact mode.
const COMPACT_TIME: f32 = 58.0;
/// Emojis considered per channel (from the latest messages).
const MAX_EMOJIS_PER_CHANNEL: usize = 300;

/// Distance of the emoji and GIF picker from the bottom of the window: just
/// above the message field.
const PICKER_BOTTOM: f32 = 96.0;
/// Consecutive messages by the same author within this interval are grouped.
const GROUP_GAP_SECS: i64 = 7 * 60;

pub fn run(opts: Options) -> iced::Result {
    iced::application(Gui::title, Gui::update, Gui::view)
        .subscription(Gui::subscription)
        .theme(|_| style::theme())
        .scale_factor(|g| g.settings.zoom as f64)
        .settings(iced::Settings {
            default_text_size: iced::Pixels(14.0),
            antialiasing: true,
            ..Default::default()
        })
        .window(window::Settings {
            // Must fit a 1280×800 logical screen (e.g. 2560×1600 at 200%)
            // together with the title bar and the task bar.
            size: Size::new(1180.0, 700.0),
            min_size: Some(Size::new(800.0, 500.0)),
            position: window::Position::Centered,
            // Title bar and task bar (the .exe gets it from build.rs).
            icon: window::icon::from_file_data(include_bytes!("../../assets/icon.png"), None).ok(),
            ..Default::default()
        })
        .run_with(move || Gui::new(opts))
}

/// The operating system uses a dark theme (light only if it says so).
fn system_is_dark() -> bool {
    !matches!(dark_light::detect(), dark_light::Mode::Light)
}

fn messages_id() -> scrollable::Id {
    scrollable::Id::new("messages")
}

fn composer_id() -> text_input::Id {
    text_input::Id::new("composer")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTab {
    Qr,
    Token,
}

pub enum QrView {
    Working(String),
    Ready(qr_code::Data),
    Scanned(String),
    Failed(String),
}

pub struct Login {
    tab: LoginTab,
    qr: QrView,
    /// Changes on every new attempt: restarts the QR subscription.
    attempt: u64,
    token: String,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
}

impl Default for Login {
    fn default() -> Self {
        Self {
            tab: LoginTab::Qr,
            qr: QrView::Working(t("Starting…", "Avvio…").into()),
            attempt: 0,
            token: String::new(),
            busy: false,
            error: None,
            notice: None,
        }
    }
}

/// State of the preview of an attached image.
pub enum Preview {
    Loading,
    Ready(img::Handle),
    /// Animated GIF, already decoded and scaled to the preview size.
    Animated(Animation),
    Failed,
}

/// Result of a preview download.
#[derive(Debug, Clone)]
pub enum Loaded {
    /// Image to decode (iced does it when drawing it).
    Image(Vec<u8>),
    Animated(Animation),
    /// Already decoded and cut to shape (avatars, icons, banners).
    Pixels {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Saving the original from the full-screen view.
pub enum SaveState {
    Idle,
    Saving,
    Saved(PathBuf),
    Failed(String),
}

/// Image open in full screen.
pub struct ImageViewer {
    attachment: Attachment,
    /// Address of the large version; `None` if it cannot be downloaded.
    url: Option<String>,
    image: Preview,
    /// Preview already downloaded, shown while the large version arrives.
    preview: Option<img::Handle>,
    save: SaveState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Online,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    LoginTab(LoginTab),
    RetryQr,
    Auth(AuthEvent),
    TokenInput(String),
    SubmitToken,
    TokenChecked(Result<(String, User), String>),
    Gateway(GatewayEvent),
    SelectGuild(Option<usize>),
    OpenChannel(String),
    History {
        channel_id: String,
        older: bool,
        result: Result<Vec<DMessage>, String>,
    },
    ComposerChanged(String),
    Send,
    Sent {
        content: String,
        result: Result<Box<DMessage>, String>,
    },
    Scrolled(scrollable::Viewport),
    HoverStart(String),
    HoverEnd(String),
    PreviewLoaded {
        url: String,
        result: Result<Loaded, String>,
    },
    OpenViewer(Box<Attachment>),
    ViewerLoaded {
        url: String,
        result: Result<Vec<u8>, String>,
    },
    CloseViewer,
    SaveAttachment,
    Saved(Result<PathBuf, String>),
    ShowInFolder(PathBuf),
    /// Click that must do nothing (e.g. on the image: it does not close the view).
    Noop,
    /// GIF and emoji buttons: open the picker (or close it if already on that tab).
    TogglePicker(picker::Tab),
    PickerTab(picker::Tab),
    ClosePicker,
    EmojiQuery(String),
    EmojiSection(picker::Section),
    PickEmoji(&'static str),
    GifQuery(String),
    /// The search starts if nothing else was typed meanwhile.
    GifSearch(u64),
    GifResults {
        seq: u64,
        result: Result<Vec<GifResult>, String>,
    },
    PickGif(String),
    /// Server emoji: `:name:` is written and becomes the emoji when sent.
    PickServerEmoji(String),
    SmallImageLoaded {
        url: String,
        result: Result<Loaded, String>,
    },
    /// To send commands to the gateway (arrives when it starts).
    GatewayLink(GatewayHandle),
    /// Click on a voice channel.
    JoinVoice(String),
    LeaveVoice,
    ToggleMicrophone,
    /// Switches the interface language (English ↔ Italian) and saves it.
    ToggleLanguage,
    OpenSettings,
    /// Account settings read from Discord (base64 protobuf).
    SettingsLoaded(Result<String, String>),
    /// Opens or closes a server folder in the column.
    ToggleFolder(i64),
    OpenFolderEditor,
    CloseFolderEditor,
    FolderEdit(folders::Edit),
    SaveFolders,
    /// Answer to saving the folders: the settings after it, and whether
    /// Discord refused because they had changed meanwhile.
    FoldersSaved(Result<(String, bool), String>),
    CloseSettings,
    SettingsSection(settings::Section),
    Setting(settings::Change),
    /// Writes the settings (after a slider is released).
    SaveSettings,
    RefreshAudioDevices,
    /// Every few seconds with the "like the system" theme: is it dark?
    CheckSystemTheme,
    ToggleDeafen,
    Voice(VoiceEvent),
    /// Click on an avatar or a name: opens the profile (with the server part
    /// if it was in a server).
    OpenProfile {
        user_id: String,
        guild_id: Option<String>,
    },
    ProfileLoaded {
        user_id: String,
        guild_id: Option<String>,
        result: Result<Box<UserProfile>, String>,
    },
    ProfileTab(profile::Tab),
    CloseProfile,
    /// Click on a LIVE badge: preview of that stream.
    OpenStreamPreview(stream::StreamRef),
    StreamPreviewLoaded {
        stream: stream::StreamRef,
        result: Result<Option<String>, String>,
    },
    RefreshStreamPreview,
    CloseStreamPreview,
    /// "Watch" in the stream card: joins the voice channel if needed, then watches.
    WatchStream,
    StopWatching,
    /// Stream volume from the slider, in percent.
    StreamVolume(f32),
    /// Stream volume up or down by 10% steps (arrow keys).
    StreamVolumeStep(i32),
    /// Mutes or unmutes the stream's sound.
    ToggleStreamSound,
    /// Events of the stream connection.
    WatchVoice(VoiceEvent),
    Logout,
    Tick,
    /// Pixels per logical pixel of the window (for the GIF decoding size).
    ScaleFactor(f32),
    /// The window opened or changed size: its scale factor is asked again.
    WindowSeen(window::Id),
}

pub struct Gui {
    /// Screen scale factor; until it is known, 2 (never blurry).
    scale: f32,
    demo: bool,
    screen: Screen,
    login: Login,
    token: Option<String>,
    /// Changes on every login: restarts the gateway subscription.
    session: u64,
    rest: Option<Rest>,
    conn: Conn,
    store: Store,
    /// Selected server (`None` = direct messages).
    guild: Option<usize>,
    composer: String,
    hovered: Option<String>,
    status: Option<(String, bool)>,
    /// Previews of attached images, by downloaded address.
    previews: HashMap<String, Preview>,
    /// Arrival order of the previews, to drop the oldest.
    preview_order: VecDeque<String>,
    downloads: usize,
    viewer: Option<ImageViewer>,
    picker: Option<Picker>,
    /// Recently used emojis (newest first), for this session only.
    recent_emojis: Vec<&'static str>,
    /// Server emoji images, by CDN address.
    small_images: HashMap<String, Preview>,
    small_order: VecDeque<String>,
    small_downloads: usize,
    gateway_link: Option<GatewayHandle>,
    voice: Option<voice::VoiceUi>,
    voice_attempt: u64,
    /// `session_id` from our last own voice state: Discord does not send it
    /// again when we ask for the channel we are already in.
    voice_session: Option<String>,
    /// Open profile card.
    profile: Option<profile::ProfileView>,
    /// Card with the preview of a Go Live stream.
    stream_view: Option<stream::StreamView>,
    /// The Go Live stream being watched.
    watch: Option<watch::WatchUi>,
    watch_attempt: u64,
    /// Last stream volume chosen: the next stream starts with it.
    stream_volume: f32,
    settings: Settings,
    /// Server folders open in the column.
    open_folders: std::collections::HashSet<i64>,
    /// The folder editor, when open.
    folder_editor: Option<folders::Editor>,
    /// The settings window, when open.
    settings_panel: Option<settings::Panel>,
    /// The operating system uses a dark theme (for "like the system").
    system_dark: bool,
    /// Profiles already downloaded in this session, by (user, server).
    profile_cache: HashMap<(String, Option<String>), Box<UserProfile>>,
}

impl Gui {
    fn blank() -> Self {
        Self {
            scale: 2.0,
            demo: false,
            screen: Screen::Login,
            login: Login::default(),
            token: None,
            session: 0,
            rest: None,
            conn: Conn::Connecting,
            store: Store::default(),
            guild: None,
            composer: String::new(),
            hovered: None,
            status: None,
            previews: HashMap::new(),
            preview_order: VecDeque::new(),
            downloads: 0,
            viewer: None,
            picker: None,
            recent_emojis: Vec::new(),
            small_images: HashMap::new(),
            small_order: VecDeque::new(),
            small_downloads: 0,
            gateway_link: None,
            voice: None,
            voice_attempt: 0,
            voice_session: None,
            profile: None,
            stream_view: None,
            watch: None,
            watch_attempt: 0,
            stream_volume: 1.0,
            settings: Settings::default(),
            open_folders: std::collections::HashSet::new(),
            folder_editor: None,
            settings_panel: None,
            system_dark: true,
            profile_cache: HashMap::new(),
        }
    }

    pub fn new(opts: Options) -> (Self, Task<Message>) {
        let mut gui = Self::blank();
        if opts.demo_login {
            gui.demo = true;
            if let Ok(data) = qr_code::Data::new("https://discord.com/ra/demo-ferrocord-0000000000000000") {
                gui.login.qr = QrView::Ready(data);
            }
            return (gui, Task::none());
        }
        if opts.demo {
            gui.demo = true;
            gui.screen = Screen::Main;
            gui.conn = Conn::Online;
            gui.store = Store::demo();
            gui.store.fake_typing("1001", "901", "Giulia");
            gui.guild = Some(0);
            gui.status = Some((
                t(
                    "Demo mode: no connection to Discord",
                    "Modalità demo: nessuna connessione a Discord",
                )
                .into(),
                false,
            ));
            let task = gui.open_channel("1001".into());
            return (gui, task);
        }
        gui.settings = config::load_settings();
        gui.apply_look();
        let task = match config::load_token() {
            // With a saved token it starts right away: if it is invalid the gateway will say so.
            Some(token) => gui.start_session(token),
            None => Task::none(),
        };
        (gui, task)
    }

    /// Reads the account settings (for the server folders).
    fn fetch_account_settings(&self) -> Task<Message> {
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { rest.settings_proto().await.map_err(|e| format!("{e:#}")) },
            Message::SettingsLoaded,
        )
    }

    /// Sends the edited folders to Discord (only the folders field, with the
    /// settings version: a change made meanwhile elsewhere is not overwritten).
    fn save_folders(&mut self) -> Task<Message> {
        let Some(e) = self.folder_editor.as_mut().filter(|e| e.changed() && !e.saving) else {
            return Task::none();
        };
        if self.demo {
            self.store.folders = Some(e.draft.clone());
            self.folder_editor = None;
            return Task::none();
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        e.saving = true;
        e.error = None;
        let body = crate::discord::folders::encode_folders_update(&e.draft);
        let version = self.store.settings_version;
        Task::perform(
            async move {
                rest.update_settings_proto(&body, version)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::FoldersSaved,
        )
    }

    /// Theme and animations as in the settings.
    fn apply_look(&mut self) {
        if self.settings.theme == ThemeChoice::System {
            self.system_dark = system_is_dark();
        }
        style::apply(self.settings.theme, self.system_dark);
        anim::set_autoplay(self.settings.autoplay);
    }

    fn save_settings(&mut self) {
        // The demo never writes settings (it also runs in the tests).
        if self.demo {
            return;
        }
        if let Err(e) = config::save_settings(&self.settings) {
            self.status = Some((
                tf!("Settings not saved: {e:#}", "Impostazioni non salvate: {e:#}"),
                true,
            ));
        }
    }

    fn title(&self) -> String {
        match &self.store.current {
            Some(c) if self.screen == Screen::Main => format!("{} — ferrocord", self.store.title(c)),
            _ => "ferrocord".to_string(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            window::open_events().map(Message::WindowSeen),
            window::resize_events().map(|(id, _)| Message::WindowSeen(id)),
        ];
        if !self.demo {
            match &self.token {
                Some(token) => subs.push(Subscription::run_with_id(
                    ("gateway", self.session),
                    gateway_stream(token.clone()),
                )),
                None if self.screen == Screen::Login => {
                    subs.push(Subscription::run_with_id(("qr", self.login.attempt), qr_stream()))
                }
                None => {}
            }
        }
        // Esc closes the profile card.
        // Esc stops watching a stream, the arrows change its volume.
        if self.watch.is_some() && self.settings_panel.is_none() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(named),
                    ..
                }) => match named {
                    keyboard::key::Named::Escape => Some(Message::StopWatching),
                    keyboard::key::Named::ArrowUp => Some(Message::StreamVolumeStep(1)),
                    keyboard::key::Named::ArrowDown => Some(Message::StreamVolumeStep(-1)),
                    _ => None,
                },
                _ => None,
            }));
        }
        if self.folder_editor.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseFolderEditor),
                _ => None,
            }));
        }
        // Ctrl+, opens the settings; Esc closes them.
        if self.settings_panel.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseSettings),
                _ => None,
            }));
        } else if self.screen == Screen::Main {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Character(c),
                    modifiers,
                    ..
                }) if c.as_str() == "," && modifiers.command() => Some(Message::OpenSettings),
                _ => None,
            }));
        }
        // "Like the system": look at the system theme now and then.
        if self.settings.theme == ThemeChoice::System {
            subs.push(iced::time::every(Duration::from_secs(5)).map(|_| Message::CheckSystemTheme));
        }
        // The stream connection, once Discord said where.
        if let Some(w) = &self.watch {
            let session = self.voice.as_ref().and_then(|v| v.session_id.as_deref());
            if let Some(server) = w.server_info(self.store.me_id().unwrap_or_default(), session) {
                subs.push(Subscription::run_with_id(
                    ("watch", w.attempt, server.endpoint.clone()),
                    watch_stream(server, w.frames.sink(), self.settings.audio.clone()),
                ));
            }
        }
        // Esc closes the stream preview.
        if self.stream_view.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseStreamPreview),
                _ => None,
            }));
        }
        if self.profile.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseProfile),
                _ => None,
            }));
        }
        // Esc closes the full-screen view (even if the text field has focus).
        if self.viewer.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseViewer),
                _ => None,
            }));
        }
        // Voice connection: in the demo towards the local fake server.
        if let Some(v) = self.voice.as_ref().filter(|v| v.is_live()) {
            if v.demo {
                let others: Vec<String> = self
                    .store
                    .voice_members(&v.channel_id)
                    .into_iter()
                    .map(|(id, _)| id)
                    .filter(|id| Some(id.as_str()) != self.store.me_id())
                    .collect();
                let me = self.store.me_id().unwrap_or("0").to_string();
                subs.push(Subscription::run_with_id(
                    ("voice-demo", v.attempt),
                    voice_demo_stream(
                        v.guild_id.clone(),
                        v.channel_id.clone(),
                        me,
                        others,
                        self.settings.audio.clone(),
                    ),
                ));
            } else if let Some(server) = v.server_info(self.store.me_id().unwrap_or_default()) {
                subs.push(Subscription::run_with_id(
                    ("voice", v.attempt, server.endpoint.clone()),
                    voice_stream(server, self.settings.audio.clone()),
                ));
            }
        }
        // Esc also closes the emoji and GIF picker.
        if self.picker.is_some() && self.viewer.is_none() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ClosePicker),
                _ => None,
            }));
        }
        // Only needed to make "is typing…" disappear when it expires.
        if self.store.has_typing() {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    // ------------------------------------------------------------ session

    fn start_session(&mut self, token: String) -> Task<Message> {
        match Rest::new(&token) {
            Ok(r) => self.rest = Some(r),
            Err(e) => {
                self.login.error = Some(tf!("Unusable token: {e:#}", "Token non utilizzabile: {e:#}"));
                return Task::none();
            }
        }
        self.token = Some(token);
        self.session += 1;
        self.conn = Conn::Connecting;
        self.screen = Screen::Main;
        Task::none()
    }

    fn finish_login(&mut self, token: String, user: Option<User>) -> Task<Message> {
        if let Err(e) = config::save_token(&token) {
            self.status = Some((tf!("Token not saved: {e:#}", "Token non salvato: {e:#}"), true));
        }
        let task = self.start_session(token);
        if let Some(u) = user {
            self.store.me = Some(u);
        }
        task
    }

    fn logout(&mut self, notice: &str) {
        config::delete_token();
        self.leave_voice();
        let session = self.session + 1;
        let attempt = self.login.attempt + 1;
        let voice_attempt = self.voice_attempt;
        *self = Self::blank();
        self.voice_attempt = voice_attempt;
        self.session = session;
        self.login.attempt = attempt;
        self.login.notice = Some(notice.to_string());
    }

    // ------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Frames of GIFs no longer on screen are dropped (at most once a second).
        anim::sweep();
        match message {
            Message::LoginTab(tab) => self.login.tab = tab,
            Message::RetryQr => {
                self.login.attempt += 1;
                self.login.qr = QrView::Working(t("Starting…", "Avvio…").into());
            }
            Message::Auth(ev) => return self.on_auth(ev),
            Message::TokenInput(s) => {
                self.login.token = s;
                self.login.error = None;
            }
            Message::SubmitToken => return self.submit_token(),
            Message::TokenChecked(result) => {
                self.login.busy = false;
                match result {
                    Ok((token, user)) => {
                        self.login.token.clear();
                        return self.finish_login(token, Some(user));
                    }
                    Err(e) => self.login.error = Some(e),
                }
            }
            Message::Gateway(ev) => return self.on_gateway(ev),
            Message::SelectGuild(guild) => {
                self.guild = guild;
                let rows = self.store.channel_rows(guild);
                if let Some(ChannelRow::Channel {
                    id, selectable: true, ..
                }) = rows.get(first_selectable(&rows))
                {
                    return self.open_channel(id.clone());
                }
            }
            Message::OpenChannel(id) => return self.open_channel(id),
            Message::History {
                channel_id,
                older,
                result,
            } => match result {
                Ok(batch) => {
                    self.store.apply_history(&channel_id, older, batch);
                    return self.request_previews();
                }
                Err(e) => {
                    self.store.history_failed(&channel_id);
                    self.status = Some((
                        tf!("History unavailable: {e}", "Cronologia non disponibile: {e}"),
                        true,
                    ));
                }
            },
            Message::ComposerChanged(s) => self.composer = s,
            Message::Send => return self.send(),
            Message::Sent { content, result } => match result {
                Ok(m) => {
                    if self.store.insert_message(*m) {
                        return self.request_previews();
                    }
                }
                Err(e) => {
                    if self.composer.is_empty() {
                        self.composer = content;
                    }
                    self.status = Some((tf!("Sending failed: {e}", "Invio non riuscito: {e}"), true));
                }
            },
            Message::Scrolled(viewport) => {
                // With the view anchored at the bottom the offset is measured from
                // the bottom: `absolute_offset_reversed` is the distance from the top.
                let taller = viewport.content_bounds().height > viewport.bounds().height;
                if taller && viewport.absolute_offset_reversed().y < 80.0 {
                    if let Some(channel_id) = self.store.current.clone() {
                        if let Some(before) = self.store.older_cursor(&channel_id) {
                            return self.fetch_history(channel_id, Some(before));
                        }
                    }
                }
            }
            Message::HoverStart(id) => self.hovered = Some(id),
            Message::HoverEnd(id) => {
                if self.hovered.as_deref() == Some(id.as_str()) {
                    self.hovered = None;
                }
            }
            Message::PreviewLoaded { url, result } => {
                self.downloads = self.downloads.saturating_sub(1);
                let state = match result {
                    Ok(Loaded::Image(bytes)) => {
                        tracing::debug!("preview downloaded ({} KB)", bytes.len() / 1024);
                        Preview::Ready(img::Handle::from_bytes(bytes))
                    }
                    Ok(Loaded::Animated(animation)) => {
                        tracing::debug!("animated GIF downloaded ({} KB)", animation.source_len() / 1024);
                        Preview::Animated(animation)
                    }
                    Ok(Loaded::Pixels { width, height, rgba }) => {
                        Preview::Ready(img::Handle::from_rgba(width, height, rgba))
                    }
                    Err(e) => {
                        tracing::debug!("preview not downloaded: {e}");
                        Preview::Failed
                    }
                };
                // It may have been dropped meanwhile (logout, full cache).
                if let Some(slot) = self.previews.get_mut(&url) {
                    *slot = state;
                }
                return self.request_previews();
            }
            Message::OpenViewer(attachment) => return self.open_viewer(*attachment),
            Message::ViewerLoaded { url, result } => {
                if let Some(v) = self
                    .viewer
                    .as_mut()
                    .filter(|v| v.url.as_deref() == Some(url.as_str()))
                {
                    v.image = match result {
                        Ok(bytes) => {
                            tracing::debug!("full image downloaded ({} KB)", bytes.len() / 1024);
                            Preview::Ready(img::Handle::from_bytes(bytes))
                        }
                        Err(e) => {
                            tracing::debug!("full image not downloaded: {e}");
                            Preview::Failed
                        }
                    };
                }
            }
            Message::CloseViewer => self.viewer = None,
            Message::SaveAttachment => return self.save_attachment(),
            Message::Saved(result) => {
                let state = match result {
                    Ok(path) => {
                        tracing::info!("attachment saved in {}", path.display());
                        self.status = Some((tf!("Saved in {}", "Salvato in {}", path.display()), false));
                        SaveState::Saved(path)
                    }
                    Err(e) => {
                        self.status =
                            Some((tf!("Saving failed: {e}", "Salvataggio non riuscito: {e}"), true));
                        SaveState::Failed(e)
                    }
                };
                if let Some(v) = self.viewer.as_mut() {
                    v.save = state;
                }
            }
            Message::ShowInFolder(path) => show_in_folder(&path),
            Message::Noop => {}
            Message::Logout => self.logout(t("You signed out.", "Sei uscito dall'account.")),
            Message::GatewayLink(handle) => self.gateway_link = Some(handle),
            Message::JoinVoice(channel_id) => self.join_voice(channel_id),
            Message::LeaveVoice => self.leave_voice(),
            Message::ToggleMicrophone => {
                // Only an explicit click turns the microphone on, and only
                // where the channel lets us speak (turning it off is always fine).
                let can_speak = self
                    .voice
                    .as_ref()
                    .is_some_and(|v| self.store.can(&v.channel_id, permissions::SPEAK));
                if let Some(v) = &mut self.voice {
                    if let Some(h) = &v.handle {
                        if v.mic_on || can_speak {
                            h.set_microphone(!v.mic_on);
                        }
                    }
                    v.audio_error = None;
                }
            }
            Message::OpenProfile { user_id, guild_id } => return self.open_profile(user_id, guild_id),
            Message::ProfileLoaded {
                user_id,
                guild_id,
                result,
            } => {
                if let Ok(p) = &result {
                    self.store.apply_profile(guild_id.as_deref(), p);
                    if self.profile_cache.len() >= MAX_PROFILES {
                        self.profile_cache.clear();
                    }
                    self.profile_cache
                        .insert((user_id.clone(), guild_id.clone()), p.clone());
                }
                if let Some(view) = self
                    .profile
                    .as_mut()
                    .filter(|v| v.user_id == user_id && v.guild_id == guild_id)
                {
                    view.data = match result {
                        Ok(p) => profile::Data::Ready(p),
                        Err(e) => profile::Data::Failed(e),
                    };
                }
                return self.request_small_images();
            }
            Message::ProfileTab(tab) => {
                if let Some(v) = &mut self.profile {
                    v.tab = tab;
                }
                return self.request_small_images();
            }
            Message::CloseProfile => self.profile = None,
            Message::OpenStreamPreview(s) => {
                self.profile = None;
                self.stream_view = Some(stream::StreamView::new(s));
                return self.fetch_stream_preview();
            }
            Message::RefreshStreamPreview => {
                if self.stream_view.as_ref().is_some_and(|v| v.can_refresh()) {
                    if let Some(v) = &mut self.stream_view {
                        v.state = stream::PreviewState::Loading;
                        v.asked = std::time::Instant::now();
                    }
                    return self.fetch_stream_preview();
                }
            }
            Message::StreamPreviewLoaded { stream, result } => {
                if let Some(v) = self.stream_view.as_mut().filter(|v| v.stream == stream) {
                    v.state = match result {
                        Ok(Some(url)) => stream::PreviewState::Ready(url),
                        Ok(None) => stream::PreviewState::Unavailable,
                        Err(e) => stream::PreviewState::Failed(e),
                    };
                }
                return self.request_small_images();
            }
            Message::CloseStreamPreview => self.stream_view = None,
            Message::WatchStream => return self.start_watching(),
            Message::StopWatching => self.stop_watching(),
            Message::StreamVolume(percent) => {
                if let Some(w) = &mut self.watch {
                    w.set_volume(percent / 100.0);
                    self.stream_volume = w.volume;
                }
            }
            Message::StreamVolumeStep(steps) => {
                if let Some(w) = &mut self.watch {
                    w.step_volume(steps);
                    self.stream_volume = w.volume;
                }
            }
            Message::ToggleStreamSound => {
                if let Some(w) = &mut self.watch {
                    w.muted = !w.muted;
                    w.apply_volume();
                }
            }
            Message::WatchVoice(ev) => self.on_watch_voice(ev),
            Message::SettingsLoaded(result) => {
                match result.map(|b64| crate::discord::folders::decode_settings(&b64)) {
                    Ok(Ok(snap)) => self.store.apply_settings(snap),
                    Ok(Err(e)) => tracing::warn!("account settings unreadable: {e:#}"),
                    Err(e) => tracing::warn!("account settings unavailable: {e}"),
                }
            }
            Message::ToggleFolder(id) => {
                if !self.open_folders.remove(&id) {
                    self.open_folders.insert(id);
                }
            }
            Message::OpenFolderEditor => {
                if self.screen == Screen::Main && self.folder_editor.is_none() {
                    self.picker = None;
                    self.folder_editor = Some(folders::Editor::new(&self.store));
                }
            }
            Message::CloseFolderEditor => {
                if !self.folder_editor.as_ref().is_some_and(|e| e.saving) {
                    self.folder_editor = None;
                }
            }
            Message::FolderEdit(edit) => {
                if let Some(e) = self.folder_editor.as_mut().filter(|e| !e.saving) {
                    e.apply(edit);
                }
            }
            Message::SaveFolders => return self.save_folders(),
            Message::FoldersSaved(result) => {
                let Some(e) = self.folder_editor.as_mut() else {
                    return Task::none();
                };
                e.saving = false;
                match result {
                    Ok((b64, out_of_date)) => {
                        if let Ok(snap) = crate::discord::folders::decode_settings(&b64) {
                            self.store.apply_settings(snap);
                        }
                        if out_of_date {
                            // Changed meanwhile elsewhere: start again from Discord's.
                            let mut fresh = folders::Editor::new(&self.store);
                            fresh.error = Some(
                                t(
                                    "The folders were changed meanwhile on another device: here they are \
                                     as they are now, make your changes again.",
                                    "Le cartelle sono state cambiate nel frattempo da un altro dispositivo: \
                                     eccole come sono ora, rifai le modifiche.",
                                )
                                .into(),
                            );
                            self.folder_editor = Some(fresh);
                        } else {
                            self.folder_editor = None;
                        }
                    }
                    Err(err) => e.error = Some(tf!("Not saved: {err}", "Non salvate: {err}")),
                }
            }
            Message::OpenSettings => {
                if self.screen == Screen::Main && self.settings_panel.is_none() {
                    self.picker = None;
                    self.settings_panel = Some(settings::Panel::new());
                }
            }
            Message::CloseSettings => self.settings_panel = None,
            Message::SettingsSection(section) => {
                if let Some(p) = &mut self.settings_panel {
                    p.section = section;
                }
            }
            Message::Setting(change) => {
                let drag = change.is_drag();
                let audio = change.is_audio();
                let images_before = self.settings.show_images;
                settings::apply(&mut self.settings, change);
                self.apply_look();
                if audio {
                    for h in [
                        self.voice.as_ref().and_then(|v| v.handle.as_ref()),
                        self.watch.as_ref().and_then(|w| w.handle.as_ref()),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        h.set_audio(self.settings.audio.clone());
                    }
                }
                if !drag {
                    self.save_settings();
                }
                if self.settings.show_images && !images_before {
                    return self.request_previews();
                }
            }
            Message::SaveSettings => self.save_settings(),
            Message::RefreshAudioDevices => {
                if let Some(p) = &mut self.settings_panel {
                    p.refresh_devices();
                }
            }
            Message::CheckSystemTheme => {
                let dark = system_is_dark();
                if dark != self.system_dark {
                    self.system_dark = dark;
                    self.apply_look();
                }
            }
            Message::ToggleLanguage => {
                let next = match i18n::lang() {
                    i18n::Lang::En => i18n::Lang::It,
                    i18n::Lang::It => i18n::Lang::En,
                };
                i18n::set_lang(next);
                // The demo never writes settings (it also runs in the tests).
                if !self.demo {
                    if let Err(e) = config::save_language(next) {
                        self.status = Some((
                            tf!("Language not saved: {e:#}", "Lingua non salvata: {e:#}"),
                            true,
                        ));
                    }
                }
            }
            Message::ToggleDeafen => {
                if let Some(v) = &mut self.voice {
                    v.deafened = !v.deafened;
                    if let Some(h) = &v.handle {
                        h.set_deafened(v.deafened);
                    }
                    // The stream's sound follows.
                    if let Some(h) = self.watch.as_ref().and_then(|w| w.handle.as_ref()) {
                        h.set_deafened(v.deafened);
                    }
                }
            }
            Message::Voice(ev) => self.on_voice(ev),
            Message::TogglePicker(tab) => return self.toggle_picker(tab),
            Message::PickerTab(tab) => {
                if let Some(p) = self.picker.as_mut() {
                    p.tab = tab;
                }
                return Task::batch([
                    text_input::focus(picker::search_id()),
                    self.load_gifs(),
                    self.request_small_images(),
                ]);
            }
            Message::ClosePicker => {
                self.close_picker();
                return text_input::focus(composer_id());
            }
            Message::EmojiQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.emoji_query = q;
                }
            }
            Message::EmojiSection(section) => {
                if let Some(p) = self.picker.as_mut() {
                    p.section = section;
                    p.emoji_query.clear();
                }
            }
            Message::PickServerEmoji(name) => {
                self.composer.push_str(&format!(":{name}:"));
                self.close_picker();
                return Task::batch([
                    text_input::focus(composer_id()),
                    text_input::move_cursor_to_end(composer_id()),
                ]);
            }
            Message::SmallImageLoaded { url, result } => {
                self.small_downloads = self.small_downloads.saturating_sub(1);
                let state = match result {
                    Ok(Loaded::Image(bytes)) => Preview::Ready(img::Handle::from_bytes(bytes)),
                    Ok(Loaded::Animated(a)) => Preview::Animated(a),
                    Ok(Loaded::Pixels { width, height, rgba }) => {
                        Preview::Ready(img::Handle::from_rgba(width, height, rgba))
                    }
                    Err(e) => {
                        tracing::debug!("small image not downloaded: {e}");
                        Preview::Failed
                    }
                };
                if let Some(slot) = self.small_images.get_mut(&url) {
                    *slot = state;
                }
                return self.request_small_images();
            }
            Message::PickEmoji(emoji) => {
                self.composer.push_str(emoji);
                self.recent_emojis.retain(|e| *e != emoji);
                self.recent_emojis.insert(0, emoji);
                self.recent_emojis.truncate(picker::MAX_RECENT);
                self.close_picker();
                return Task::batch([
                    text_input::focus(composer_id()),
                    text_input::move_cursor_to_end(composer_id()),
                ]);
            }
            Message::GifQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.gif_query = q;
                    p.seq += 1;
                    let seq = p.seq;
                    // Search only when typing pauses for a moment
                    // (the timer is created inside the task, i.e. in iced's runtime).
                    let pause = async { tokio::time::sleep(Duration::from_millis(350)).await };
                    return Task::perform(pause, move |_| Message::GifSearch(seq));
                }
            }
            Message::GifSearch(seq) => return self.search_gifs(seq),
            Message::GifResults { seq, result } => {
                let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
                    return Task::none();
                };
                p.searching = false;
                let old = std::mem::replace(
                    &mut p.gifs,
                    match result {
                        Ok(list) => Gifs::Ready(list),
                        Err(e) => Gifs::Failed(e),
                    },
                );
                tracing::debug!("GIF: {} results", p.shown_gifs().len());
                self.forget_gif_previews(old);
                return self.request_previews();
            }
            Message::PickGif(url) => {
                self.close_picker();
                return Task::batch([self.send_text(url), text_input::focus(composer_id())]);
            }
            Message::Tick => {}
            Message::ScaleFactor(f) => {
                tracing::debug!("window scale factor: {f}");
                self.scale = f.clamp(1.0, 4.0);
            }
            // When it opens, and when it moves to a screen with another scale.
            Message::WindowSeen(id) => return window::get_scale_factor(id).map(Message::ScaleFactor),
        }
        Task::none()
    }

    fn on_auth(&mut self, ev: AuthEvent) -> Task<Message> {
        if self.screen != Screen::Login {
            return Task::none();
        }
        match ev {
            AuthEvent::Progress(s) => self.login.qr = QrView::Working(s),
            AuthEvent::Qr(url) => {
                self.login.qr = match qr_code::Data::new(url) {
                    Ok(data) => QrView::Ready(data),
                    Err(_) => QrView::Failed(
                        t(
                            "Could not generate the QR code",
                            "Impossibile generare il codice QR",
                        )
                        .into(),
                    ),
                }
            }
            AuthEvent::Scanned(user) => self.login.qr = QrView::Scanned(user),
            AuthEvent::Token(token) => return self.finish_login(token, None),
            AuthEvent::Failed(e) => self.login.qr = QrView::Failed(e),
        }
        Task::none()
    }

    fn submit_token(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let token = match rest::clean_token(&self.login.token) {
            Ok(t) => t,
            Err(e) => {
                self.login.error = Some(e);
                return Task::none();
            }
        };
        let client = match Rest::new(&token) {
            Ok(r) => r,
            Err(e) => {
                self.login.error = Some(tf!("Invalid token: {e:#}", "Token non valido: {e:#}"));
                return Task::none();
            }
        };
        self.login.busy = true;
        self.login.error = None;
        Task::perform(
            async move {
                match client.me().await {
                    Ok(user) => Ok((token, user)),
                    Err(e) if e.is::<Unauthorized>() => {
                        Err(t("Invalid or expired token.", "Token non valido o scaduto.").to_string())
                    }
                    Err(e) => Err(tf!("Verification failed: {e:#}", "Verifica non riuscita: {e:#}")),
                }
            },
            Message::TokenChecked,
        )
    }

    fn on_gateway(&mut self, ev: GatewayEvent) -> Task<Message> {
        match ev {
            GatewayEvent::Connecting => self.conn = Conn::Connecting,
            GatewayEvent::Disconnected(e) => self.conn = Conn::Error(e),
            GatewayEvent::Ready(r) => {
                self.conn = Conn::Online;
                let reload = self.store.apply_ready(*r);
                tracing::debug!("permissions: {} channels hidden", self.store.hidden_channels());
                if self.guild.is_some_and(|g| g >= self.store.guilds.len()) {
                    self.guild = None;
                }
                let icons = self.request_small_images();
                let settings = self.fetch_account_settings();
                if let Some(c) = reload {
                    return Task::batch([icons, settings, self.fetch_history(c, None)]);
                }
                return Task::batch([icons, settings]);
            }
            GatewayEvent::Resumed => self.conn = Conn::Online,
            GatewayEvent::SettingsProto(u) => {
                match crate::discord::folders::decode_settings(&u.settings.proto) {
                    Ok(snap) => self.store.apply_settings(snap),
                    Err(e) => tracing::warn!("settings update ignored: {e:#}"),
                }
            }
            GatewayEvent::GuildCreate(g) => {
                self.store.upsert_guild(*g);
                return self.request_small_images();
            }
            GatewayEvent::GuildEmojis(u) => self.store.guild_emojis(u),
            GatewayEvent::GuildDelete(g) => {
                if !g.unavailable {
                    let open = self.guild_id();
                    self.store.remove_guild(&g.id);
                    self.guild = open.and_then(|id| self.store.guilds.iter().position(|x| x.id == id));
                }
                return self.after_permission_change();
            }
            GatewayEvent::ChannelUpdate(c) => {
                self.store.upsert_channel(*c);
                return self.after_permission_change();
            }
            GatewayEvent::ChannelDelete(c) => {
                self.store.delete_channel(&c);
                return self.after_permission_change();
            }
            GatewayEvent::RoleUpdate(r) => {
                let r = *r;
                self.store.upsert_role(&r.guild_id, r.role);
                return self.after_permission_change();
            }
            GatewayEvent::RoleDelete(r) => {
                self.store.delete_role(&r.guild_id, &r.role_id);
                return self.after_permission_change();
            }
            GatewayEvent::MemberUpdate(m) => {
                self.store.update_member(*m);
                return self.after_permission_change();
            }
            GatewayEvent::VoiceState(v) => {
                self.on_voice_state(v);
                return self.request_small_images();
            }
            GatewayEvent::StreamCreate(c) => {
                if let Some(w) = self.watch.as_mut().filter(|w| w.key == c.stream_key) {
                    w.rtc_server_id = Some(c.rtc_server_id);
                    w.status = watch::Status::Connecting;
                }
            }
            GatewayEvent::StreamServer(u) => {
                if let Some(w) = self.watch.as_mut().filter(|w| w.key == u.stream_key) {
                    // Without an endpoint Discord is still choosing the server.
                    if let Some(endpoint) = u.endpoint {
                        w.server = Some((endpoint, u.token));
                        self.watch_attempt += 1;
                        w.attempt = self.watch_attempt;
                        w.status = watch::Status::Connecting;
                    }
                }
            }
            GatewayEvent::StreamDelete(d) => {
                if let Some(w) = self.watch.as_mut().filter(|w| w.key == d.stream_key) {
                    w.server = None;
                    w.handle = None;
                    w.status = watch::Status::Ended(stream_end_reason(d.reason.as_deref()));
                }
            }
            GatewayEvent::VoiceServer(u) => {
                if let Some(v) = self
                    .voice
                    .as_mut()
                    .filter(|v| u.guild_id.as_deref() == Some(v.guild_id.as_str()))
                {
                    // Without an endpoint Discord is still choosing the server: it will come.
                    if let Some(endpoint) = u.endpoint {
                        v.server = Some((endpoint, u.token));
                        v.status = voice::Status::Connecting;
                    }
                }
            }
            GatewayEvent::MessageCreate(m) => {
                if self.store.insert_message(*m) {
                    return self.request_previews();
                }
            }
            GatewayEvent::MessageUpdate(u) => {
                self.store.update_message(*u);
                return self.request_previews();
            }
            GatewayEvent::MessageDelete(d) => self.store.delete_message(&d),
            GatewayEvent::TypingStart(t) => self.store.typing_start(t),
            GatewayEvent::ReactionAdd(r) => self.store.reaction_add(&r),
            GatewayEvent::ReactionRemove(r) => self.store.reaction_remove(&r),
            GatewayEvent::AuthFailed => self.logout(t(
                "Session invalid or expired: sign in again.",
                "Sessione non valida o scaduta: accedi di nuovo.",
            )),
            GatewayEvent::Fatal(e) => {
                self.conn = Conn::Error(e.clone());
                self.status = Some((e, true));
            }
        }
        Task::none()
    }

    /// ID of the server shown in the left column (`None` = direct messages).
    fn guild_id(&self) -> Option<String> {
        self.guild
            .and_then(|i| self.store.guilds.get(i))
            .map(|g| g.id.clone())
    }

    /// Channels, roles or our member changed: what we were looking at may
    /// no longer be visible, and the microphone may no longer be allowed.
    fn after_permission_change(&mut self) -> Task<Message> {
        if let Some(v) = &self.voice {
            if v.mic_on && !self.store.can(&v.channel_id, permissions::SPEAK) {
                if let Some(h) = &v.handle {
                    h.set_microphone(false);
                }
            }
        }
        if self.store.close_if_hidden() {
            self.close_picker();
            let rows = self.store.channel_rows(self.guild);
            if let Some(ChannelRow::Channel {
                id, selectable: true, ..
            }) = rows.get(first_selectable(&rows))
            {
                return self.open_channel(id.clone());
            }
        }
        Task::none()
    }

    fn join_voice(&mut self, channel_id: String) {
        if !self.store.can_connect(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to join this voice channel.",
                    "Non hai il permesso di entrare in questo canale vocale.",
                )
                .into(),
                true,
            ));
            return;
        }
        if self
            .voice
            .as_ref()
            .is_some_and(|v| v.channel_id == channel_id && v.is_live())
        {
            return;
        }
        let Some(guild_id) = self.store.channel_guild.get(&channel_id).cloned() else {
            return;
        };
        if self.demo || self.gateway_link.is_none() {
            self.leave_voice();
        } else {
            // Switching channel: the new voice state moves us. Sending "leave"
            // first would make its late update close the new connection.
            self.stop_voice_connection();
        }
        self.voice_attempt += 1;
        if !self.demo {
            let Some(link) = &self.gateway_link else {
                self.status = Some((
                    t(
                        "Not connected to Discord: try again shortly.",
                        "Non connesso a Discord: riprova tra poco.",
                    )
                    .into(),
                    true,
                ));
                return;
            };
            link.voice_state(Some(guild_id.clone()), Some(channel_id.clone()));
        }
        tracing::info!("voice: joining a voice channel");
        let mut v = voice::VoiceUi::new(guild_id, channel_id, self.demo, self.voice_attempt);
        // Already in that channel for Discord: our state will not be sent again.
        let me = self.store.me_id().map(str::to_string);
        let already_there = me.and_then(|me| self.store.voice.get(&me).cloned())
            == Some((v.guild_id.clone(), v.channel_id.clone()));
        if !self.demo && already_there {
            v.session_id = self.voice_session.clone();
            v.confirmed = v.session_id.is_some();
        }
        self.voice = Some(v);
    }

    /// Opens a profile card; the data is downloaded only now (or reused
    /// from earlier in this session).
    /// Asks Discord where the preview of the open stream card is.
    fn fetch_stream_preview(&mut self) -> Task<Message> {
        let Some(v) = &self.stream_view else {
            return Task::none();
        };
        let s = v.stream.clone();
        if self.demo {
            // The demo preview is drawn locally.
            let url = format!("demo://stream/{}", s.user_id);
            if let Some(v) = &mut self.stream_view {
                v.state = stream::PreviewState::Ready(url.clone());
            }
            let (w, h) = stream::PREVIEW_PX;
            self.remember_small_image(url, demo_shaped(w, h, media::Shape::Plain));
            return Task::none();
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = rest
                    .stream_preview(&s.guild_id, &s.channel_id, &s.user_id)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (s, result)
            },
            |(stream, result)| Message::StreamPreviewLoaded { stream, result },
        )
    }

    fn open_profile(&mut self, user_id: String, guild_id: Option<String>) -> Task<Message> {
        let mut view = profile::ProfileView::new(user_id.clone(), guild_id.clone());
        let key = (user_id.clone(), guild_id.clone());
        if let Some(p) = self.profile_cache.get(&key) {
            view.data = profile::Data::Ready(p.clone());
            self.profile = Some(view);
            return self.request_small_images();
        }
        if self.demo {
            view.data = match profile::demo_profile(&self.store, &user_id, guild_id.as_deref()) {
                Some(p) => profile::Data::Ready(Box::new(p)),
                None => profile::Data::Failed(t("unknown user", "utente sconosciuto").into()),
            };
            self.profile = Some(view);
            return self.request_small_images();
        }
        self.profile = Some(view);
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        let images = self.request_small_images();
        let fetch = Task::perform(
            async move {
                let result = rest
                    .profile(&user_id, guild_id.as_deref())
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                (user_id, guild_id, result)
            },
            |(user_id, guild_id, result)| Message::ProfileLoaded {
                user_id,
                guild_id,
                result,
            },
        );
        Task::batch([images, fetch])
    }

    /// Closes our voice connection without telling Discord we left.
    fn stop_voice_connection(&mut self) {
        if let Some(handle) = self.voice.take().and_then(|v| v.handle) {
            handle.leave();
        }
    }

    /// "Watch" in the stream card. Discord only lets you watch from inside
    /// the voice channel: we join it first if needed (microphone off).
    fn start_watching(&mut self) -> Task<Message> {
        let Some(s) = self.stream_view.as_ref().map(|v| v.stream.clone()) else {
            return Task::none();
        };
        if !self.store.can_connect(&s.channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to join this voice channel.",
                    "Non hai il permesso di entrare in questo canale vocale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        let Some(key) = self.store.stream_key(&s.user_id) else {
            return Task::none();
        };
        self.stop_watching();
        self.stream_view = None;
        // Into the channel first: switching channel stops any other stream.
        if !self.voice.as_ref().is_some_and(|v| v.channel_id == s.channel_id) {
            self.join_voice(s.channel_id.clone());
        }
        let mut w = watch::WatchUi::new(s, key, self.demo, self.stream_volume);
        if self.demo {
            // No Discord in the demo: a moving test picture.
            w.status = watch::Status::Watching;
            watch::demo_feed(&w.frames);
        }
        self.watch = Some(w);
        self.ask_to_watch();
        Task::none()
    }

    /// Op 20, as soon as we are connected to the stream's voice channel.
    fn ask_to_watch(&mut self) {
        let Some(w) = self.watch.as_mut().filter(|w| !w.asked && !w.demo) else {
            return;
        };
        let connected = self.voice.as_ref().is_some_and(|v| {
            v.channel_id == w.stream.channel_id && matches!(v.status, voice::Status::Connected(_))
        });
        let Some(link) = self.gateway_link.as_ref().filter(|_| connected) else {
            return;
        };
        link.watch_stream(w.key.clone());
        w.asked = true;
        w.status = watch::Status::Asking;
    }

    /// Stops watching (op 19) and closes the stream connection; we stay in
    /// the voice channel.
    fn stop_watching(&mut self) {
        let Some(w) = self.watch.take() else { return };
        if let Some(h) = &w.handle {
            h.leave();
        }
        if let (true, Some(link)) = (w.asked, &self.gateway_link) {
            link.stop_stream(w.key.clone());
        }
    }

    fn on_watch_voice(&mut self, ev: VoiceEvent) {
        let Some(w) = self.watch.as_mut() else { return };
        match ev {
            VoiceEvent::Handle(h) => {
                if self.voice.as_ref().is_some_and(|v| v.deafened) {
                    h.set_deafened(true);
                }
                w.handle = Some(h);
                w.apply_volume();
            }
            VoiceEvent::Connected { .. } => w.status = watch::Status::Watching,
            VoiceEvent::Reconnecting(_) => w.status = watch::Status::Connecting,
            VoiceEvent::Stats(s) => w.stats = s,
            VoiceEvent::Closed(None) => {
                w.status = watch::Status::Ended(
                    t(
                        "The stream connection closed.",
                        "La connessione alla diretta si è chiusa.",
                    )
                    .into(),
                )
            }
            VoiceEvent::Closed(Some(reason)) => w.status = watch::Status::Ended(reason),
            VoiceEvent::AudioError(e) => self.status = Some((e, true)),
            _ => {}
        }
    }

    fn leave_voice(&mut self) {
        self.stop_watching();
        let Some(v) = self.voice.take() else { return };
        if let Some(handle) = &v.handle {
            handle.leave();
        }
        if v.demo {
            if let Some(me) = self.store.me_id().map(str::to_string) {
                self.store.voice.remove(&me);
            }
        } else if let Some(link) = &self.gateway_link {
            link.voice_state(Some(v.guild_id), None);
        }
    }

    /// Our voice state carries the `session_id` for the voice server; if
    /// Discord removes us from the channel, it closes here too.
    fn on_voice_state(&mut self, state: VoiceState) {
        self.store.apply_voice_state(&state, None);
        if self.store.me_id() != Some(state.user_id.as_str()) {
            return;
        }
        self.voice_session = state.channel_id.is_some().then(|| state.session_id.clone());
        let Some(v) = self.voice.as_mut().filter(|v| !v.demo) else {
            return;
        };
        match state.channel_id {
            Some(channel) => {
                if channel != v.channel_id {
                    // Moved by someone else: start again in the new channel.
                    v.channel_id = channel;
                    v.server = None;
                    self.voice_attempt += 1;
                    v.attempt = self.voice_attempt;
                }
                v.session_id = Some(state.session_id);
                v.confirmed = true;
            }
            // A "left" that arrives before Discord confirmed this join belongs
            // to the previous channel: ignored.
            None if v.confirmed => self.voice = None,
            None => {}
        }
    }

    fn on_voice(&mut self, ev: VoiceEvent) {
        let me = self.store.me_id().map(str::to_string);
        let Some(v) = self.voice.as_mut() else { return };
        match ev {
            VoiceEvent::Handle(h) => v.handle = Some(h),
            VoiceEvent::Connecting => {
                if v.status != voice::Status::Joining {
                    v.status = voice::Status::Connecting;
                }
            }
            VoiceEvent::Connected { mode } => {
                v.status = voice::Status::Connected(mode);
                if v.demo {
                    if let Some(me) = me {
                        self.store
                            .voice
                            .insert(me, (v.guild_id.clone(), v.channel_id.clone()));
                    }
                }
                // A stream was waiting for us to be in its channel.
                self.ask_to_watch();
            }
            VoiceEvent::Speaking { user_id, speaking } => {
                if speaking {
                    v.speaking.insert(user_id);
                } else {
                    v.speaking.remove(&user_id);
                }
            }
            VoiceEvent::Stats(stats) => v.stats = stats,
            VoiceEvent::Encryption(e) => v.encryption = e,
            VoiceEvent::Microphone(on) => {
                v.mic_on = on;
                if on {
                    v.deafened = false;
                }
            }
            VoiceEvent::AudioError(e) => v.audio_error = Some(e),
            VoiceEvent::Reconnecting(_) => v.status = voice::Status::Reconnecting,
            VoiceEvent::Closed(None) => self.voice = None,
            VoiceEvent::Closed(Some(reason)) => {
                v.status = voice::Status::Failed(reason);
                v.speaking.clear();
                v.handle = None;
                v.mic_on = false;
                // On Discord we would still be in the channel: leave.
                if v.demo {
                    if let Some(me) = me {
                        self.store.voice.remove(&me);
                    }
                } else if let Some(link) = &self.gateway_link {
                    link.voice_state(Some(v.guild_id.clone()), None);
                }
            }
        }
    }

    fn open_channel(&mut self, id: String) -> Task<Message> {
        self.hovered = None;
        let needs_history = self.store.open(&id);
        let mut tasks = vec![
            // With the view anchored at the bottom, offset 0 = latest messages.
            scrollable::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 }),
            text_input::focus(composer_id()),
        ];
        if needs_history {
            tasks.push(self.fetch_history(id, None));
        }
        tasks.push(self.request_previews());
        Task::batch(tasks)
    }

    /// Starts downloading the previews still missing among the loaded
    /// messages of the open channel, newest first (at most `MAX_DOWNLOADS` at
    /// a time; the others start as the running ones finish). Includes the
    /// older messages loaded by scrolling up.
    fn request_previews(&mut self) -> Task<Message> {
        // (address, size to decode animated GIFs at). The picker's GIFs come
        // first, if it is open: they are the ones being looked at.
        let mut wanted: Vec<(String, (u32, u32))> = self
            .picker
            .as_ref()
            .map(|p| {
                p.shown_gifs()
                    .iter()
                    .map(|g| {
                        let (w, h) = picker::gif_size(g);
                        (g.src.clone(), ((w * self.scale) as u32, (h * self.scale) as u32))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let channel = self
            .store
            .current
            .as_ref()
            .and_then(|c| self.store.messages.get(c));
        // Images in the chat only if the settings want them.
        let channel = channel.filter(|_| self.settings.show_images);
        wanted.extend(match channel {
            Some(list) => list
                .iter()
                .rev()
                .flat_map(|m| {
                    let attachments = m.attachments.iter().filter(|a| a.is_image()).filter_map(|a| {
                        let size = decode_size(a.width.unwrap_or(1), a.height.unwrap_or(1), self.scale);
                        preview_key(a).map(|k| (k, size))
                    });
                    let embeds = m.embeds.iter().filter_map(media::embed_preview).filter_map(|e| {
                        let size = decode_size(e.width, e.height, self.scale);
                        embed_source(&e, &self.previews).map(|k| (k.to_string(), size))
                    });
                    attachments.chain(embeds).collect::<Vec<_>>()
                })
                .take(MAX_IMAGES_PER_CHANNEL)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        });
        let mut tasks = Vec::new();
        for (url, (w, h)) in wanted {
            if self.previews.contains_key(&url) {
                continue;
            }
            if url == crate::demo::DEMO_IMAGE_URL {
                let (w, h) = (960, 600);
                let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
                self.remember_preview(url, Preview::Ready(handle));
                continue;
            }
            if url == crate::demo::DEMO_GIF_URL {
                let state = match anim::demo_animation(320, 200) {
                    Some(a) => Preview::Animated(a),
                    None => Preview::Failed,
                };
                self.remember_preview(url, state);
                continue;
            }
            if self.downloads >= MAX_DOWNLOADS {
                break;
            }
            self.downloads += 1;
            self.remember_preview(url.clone(), Preview::Loading);
            let key = url.clone();
            tasks.push(Task::perform(
                async move { load_preview(url, w, h).await.map_err(|e| format!("{e:#}")) },
                move |result| Message::PreviewLoaded {
                    url: key.clone(),
                    result,
                },
            ));
        }
        if !tasks.is_empty() {
            tracing::debug!("previews: {} started, {} running", tasks.len(), self.downloads);
        }
        tasks.push(self.request_small_images());
        Task::batch(tasks)
    }

    /// Small images that should be on screen, most important first.
    fn wanted_small_images(&self) -> Vec<(String, SmallKind)> {
        let mut wanted: Vec<(String, SmallKind)> = Vec::new();
        let as_avatar = |url: String| (url, SmallKind::Shaped(media::Shape::Circle, AVATAR_PX, AVATAR_PX));
        // Server icons: always visible in the bar on the left.
        for g in &self.store.guilds {
            if let Some(url) = media::guild_icon_url(&g.id, g.icon(), ICON_PX) {
                wanted.push((url, SmallKind::Shaped(media::Shape::Rounded, ICON_PX, ICON_PX)));
            }
        }
        // The open stream preview.
        if let Some(url) = self.stream_view.as_ref().and_then(|v| v.image_url()) {
            let (w, h) = stream::PREVIEW_PX;
            wanted.push((url.to_string(), SmallKind::Shaped(media::Shape::Plain, w, h)));
        }
        // The open profile.
        if let Some(p) = &self.profile {
            for (url, kind) in p.images(&self.store) {
                wanted.push((url, kind));
            }
        }
        if let Some(me) = self.store.me_id() {
            if let Some(url) = self.store.avatar_url(None, me, AVATAR_PX) {
                wanted.push(as_avatar(url));
            }
        }
        // Pictures of the private conversations, when their list is shown
        // (the most recent first).
        if self.guild.is_none() {
            for c in self.store.dms.iter().take(MAX_DM_PICTURES) {
                if let (_, Some(url)) = self.store.dm_picture(c, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
        }
        let channel = self.store.current.clone();
        let guild = channel
            .as_ref()
            .and_then(|c| self.store.channel_guild.get(c))
            .cloned();
        // Who is in the voice channels of the open server.
        if let Some(g) = &guild {
            for (user, (vg, _)) in &self.store.voice {
                if vg == g {
                    if let Some(url) = self.store.avatar_url(Some(g), user, AVATAR_PX) {
                        wanted.push(as_avatar(url));
                    }
                }
            }
        }
        let Some(channel_id) = channel else { return wanted };
        // Authors of the latest messages.
        if let Some(list) = self.store.messages.get(&channel_id) {
            let mut seen = std::collections::HashSet::new();
            for m in list.iter().rev().take(MAX_IMAGES_PER_CHANNEL) {
                if seen.insert(m.author.id.as_str()) {
                    if let Some(url) = self.store.avatar_url(guild.as_deref(), &m.author.id, AVATAR_PX) {
                        wanted.push(as_avatar(url));
                    }
                }
            }
        }
        // Server emojis: those of the picker first, if it is open, then
        // those in the channel's messages and reactions.
        let mut emojis = 0;
        if self.picker.as_ref().is_some_and(|p| p.tab == picker::Tab::Emoji) {
            for e in self.store.usable_emojis(&channel_id) {
                wanted.push((
                    media::emoji_url(&e.id, e.animated),
                    SmallKind::Emoji { animated: e.animated },
                ));
            }
        }
        if let Some(list) = self.store.messages.get(&channel_id) {
            for m in list.iter().rev() {
                for e in crate::text::custom_emojis(&m.content) {
                    wanted.push((
                        media::emoji_url(&e.id, e.animated),
                        SmallKind::Emoji { animated: e.animated },
                    ));
                    emojis += 1;
                }
                for r in &m.reactions {
                    if let Some(id) = &r.emoji.id {
                        let animated = r.emoji.animated;
                        wanted.push((media::emoji_url(id, animated), SmallKind::Emoji { animated }));
                        emojis += 1;
                    }
                }
                if emojis >= MAX_EMOJIS_PER_CHANNEL {
                    break;
                }
            }
        }
        wanted
    }

    /// Downloads the small images still missing (server icons, avatars, the
    /// open profile, server emojis), a few at a time.
    fn request_small_images(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for (url, kind) in self.wanted_small_images() {
            if self.small_images.contains_key(&url) {
                continue;
            }
            if self.demo {
                let image = match kind {
                    SmallKind::Emoji { animated } => demo_emoji(&url, animated),
                    SmallKind::Shaped(shape, w, h) => demo_shaped(w, h, shape),
                };
                self.remember_small_image(url, image);
                continue;
            }
            if self.small_downloads >= MAX_SMALL_DOWNLOADS {
                break;
            }
            self.small_downloads += 1;
            self.remember_small_image(url.clone(), Preview::Loading);
            let key = url.clone();
            tasks.push(Task::perform(
                async move {
                    match kind {
                        SmallKind::Emoji { .. } => load_preview(url, 48, 48).await,
                        SmallKind::Shaped(shape, w, h) => load_shaped(url, w, h, shape).await,
                    }
                    .map_err(|e| format!("{e:#}"))
                },
                move |result| Message::SmallImageLoaded {
                    url: key.clone(),
                    result,
                },
            ));
        }
        Task::batch(tasks)
    }

    fn remember_small_image(&mut self, url: String, state: Preview) {
        self.small_images.insert(url.clone(), state);
        self.small_order.push_back(url);
        let mut checked = 0;
        while self.small_images.len() > MAX_SMALL_IMAGES && checked < self.small_order.len() {
            let Some(old) = self.small_order.pop_front() else {
                break;
            };
            if matches!(self.small_images.get(&old), Some(Preview::Loading)) {
                self.small_order.push_back(old);
                checked += 1;
            } else {
                self.small_images.remove(&old);
            }
        }
    }

    fn toggle_picker(&mut self, tab: picker::Tab) -> Task<Message> {
        match self.picker.as_mut() {
            Some(p) if p.tab == tab => {
                self.close_picker();
                return text_input::focus(composer_id());
            }
            Some(p) => p.tab = tab,
            None => {
                let has_server = self
                    .store
                    .current
                    .as_ref()
                    .is_some_and(|c| !self.store.usable_emojis(c).is_empty());
                self.picker = Some(Picker::new(tab, !self.recent_emojis.is_empty(), has_server));
            }
        }
        Task::batch([
            text_input::focus(picker::search_id()),
            self.load_gifs(),
            self.request_small_images(),
        ])
    }

    /// The first time the GIF tab opens, it loads the trending ones.
    fn load_gifs(&mut self) -> Task<Message> {
        match self.picker.as_mut() {
            Some(p) if p.tab == picker::Tab::Gif && matches!(p.gifs, Gifs::NotLoaded) => {
                p.gifs = Gifs::Loading;
                let seq = p.seq;
                self.search_gifs(seq)
            }
            _ => Task::none(),
        }
    }

    fn search_gifs(&mut self, seq: u64) -> Task<Message> {
        let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
            return Task::none();
        };
        p.searching = true;
        if self.demo {
            let result = Ok(crate::demo::gif_results(&p.gif_query));
            return self.update(Message::GifResults { seq, result });
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let query = p.gif_query.clone();
        Task::perform(
            async move { client.gifs(&query).await.map_err(|e| format!("{e:#}")) },
            move |result| Message::GifResults { seq, result },
        )
    }

    fn close_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            self.forget_gif_previews(p.gifs);
        }
    }

    /// The previews of the picker's GIFs are no longer needed: freed right away
    /// (they are already decoded animations and take a lot of memory).
    fn forget_gif_previews(&mut self, gifs: Gifs) {
        let Gifs::Ready(list) = gifs else {
            return;
        };
        for g in list {
            if g.src != crate::demo::DEMO_GIF_URL {
                self.previews.remove(&g.src);
            }
        }
        let previews = &self.previews;
        self.preview_order.retain(|url| previews.contains_key(url));
    }

    fn open_viewer(&mut self, attachment: Attachment) -> Task<Message> {
        let preview = preview_key(&attachment).and_then(|k| match self.previews.get(&k) {
            Some(Preview::Ready(h)) => Some(h.clone()),
            _ => None,
        });
        if attachment.url == crate::demo::DEMO_IMAGE_URL {
            let (w, h) = (1920, 1200);
            let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
            self.viewer = Some(ImageViewer {
                attachment,
                url: None,
                image: Preview::Ready(handle),
                preview,
                save: SaveState::Idle,
            });
            return Task::none();
        }
        let url = media::viewer_url(&attachment);
        self.viewer = Some(ImageViewer {
            attachment,
            url: url.clone(),
            image: if url.is_some() {
                Preview::Loading
            } else {
                Preview::Failed
            },
            preview,
            save: SaveState::Idle,
        });
        let Some(url) = url else {
            return Task::none();
        };
        let key = url.clone();
        Task::perform(
            async move {
                media::fetch(&url, media::VIEWER_MAX_BYTES)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::ViewerLoaded {
                url: key.clone(),
                result,
            },
        )
    }

    fn save_attachment(&mut self) -> Task<Message> {
        let demo = self.demo;
        let Some(v) = self.viewer.as_mut() else {
            return Task::none();
        };
        if matches!(v.save, SaveState::Saving) {
            return Task::none();
        }
        if demo || v.attachment.url == crate::demo::DEMO_IMAGE_URL {
            self.status = Some((
                t(
                    "Nothing is downloaded in the demo.",
                    "Nella demo non si scarica nulla.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        // The original is on the CDN; the media proxy is the fallback.
        let url = if !v.attachment.url.is_empty() {
            v.attachment.url.clone()
        } else {
            v.attachment.proxy_url.clone()
        };
        let filename = v.attachment.filename.clone();
        v.save = SaveState::Saving;
        Task::perform(
            async move {
                media::save_to(&url, &media::download_dir(), &filename)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Saved,
        )
    }

    fn view_viewer<'a>(&'a self, v: &'a ImageViewer) -> Element<'a, Message> {
        let a = &v.attachment;
        let mut info = row![
            text(a.filename.clone()).font(SEMIBOLD).shaping(Shaping::Advanced),
            text(human_size(a.size)).size(12).color(pal::muted()),
        ]
        .spacing(10)
        .align_y(Vertical::Center);
        if let (Some(w), Some(h)) = (a.width, a.height) {
            info = info.push(text(format!("{w}×{h}")).size(12).color(pal::muted()));
        }

        let save: Element<'a, Message> = match &v.save {
            SaveState::Idle | SaveState::Failed(_) => button(text(t("Download", "Scarica")).font(SEMIBOLD))
                .on_press(Message::SaveAttachment)
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saving => button(text(t("Saving…", "Salvataggio…")))
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saved(path) => button(text(t("Show in folder", "Mostra nella cartella")))
                .on_press(Message::ShowInFolder(path.clone()))
                .padding([6, 14])
                .style(ghost)
                .into(),
        };
        let close = button(text(t("Close (Esc)", "Chiudi (Esc)")))
            .on_press(Message::CloseViewer)
            .padding([6, 14])
            .style(ghost);
        let top = container(
            row![info, horizontal_space(), save, close]
                .spacing(10)
                .align_y(Vertical::Center),
        )
        .padding([10, 16])
        .width(Length::Fill)
        .style(fill(pal::panel()));

        let mut footer = None;
        let body: Element<'a, Message> = match (&v.image, &v.preview) {
            (Preview::Ready(handle), _) => img::Viewer::new(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .min_scale(0.5)
                .max_scale(8.0)
                .into(),
            (Preview::Loading, Some(small)) => {
                footer = Some(t(
                    "Loading the large version…",
                    "Caricamento della versione grande…",
                ));
                img::Image::new(small.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Contain)
                    .into()
            }
            (Preview::Loading, None) => text(t("Loading…", "Caricamento…")).color(pal::muted()).into(),
            (Preview::Animated(animation), _) => animated(animation)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            (Preview::Failed, _) => text(t(
                "Image unavailable: you can still download it.",
                "Immagine non disponibile: puoi comunque scaricarla.",
            ))
            .color(pal::muted())
            .into(),
        };
        if let SaveState::Failed(e) = &v.save {
            footer = Some(e.as_str());
        }
        // Click on the image: it must not close the view.
        let picture = mouse_area(
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .on_press(Message::Noop);
        let mut panel = column![top, picture].height(Length::Fill);
        let hint = footer.unwrap_or(t(
            "Mouse wheel to zoom, drag to move",
            "Rotellina per lo zoom, trascina per spostare",
        ));
        panel = panel.push(
            container(text(hint).size(12).color(pal::muted()).shaping(Shaping::Advanced))
                .padding([6, 16])
                .width(Length::Fill)
                .style(fill(pal::panel())),
        );

        // The dark background around closes the view; `opaque` blocks clicks
        // towards the chat below.
        opaque(
            mouse_area(
                container(container(panel).style(style::outlined))
                    .padding(32)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(style::backdrop),
            )
            .on_press(Message::CloseViewer),
        )
    }

    fn remember_preview(&mut self, url: String, state: Preview) {
        self.previews.insert(url.clone(), state);
        self.preview_order.push_back(url);
        // Drop the oldest that are done; those downloading stay.
        let mut checked = 0;
        while self.previews.len() > MAX_PREVIEWS && checked < self.preview_order.len() {
            let Some(old) = self.preview_order.pop_front() else {
                break;
            };
            if matches!(self.previews.get(&old), Some(Preview::Loading)) {
                self.preview_order.push_back(old);
                checked += 1;
            } else {
                self.previews.remove(&old);
            }
        }
    }

    fn fetch_history(&mut self, channel_id: String, before: Option<String>) -> Task<Message> {
        if self.demo {
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        if !self.store.begin_loading(&channel_id) {
            return Task::none();
        }
        let older = before.is_some();
        let id = channel_id.clone();
        Task::perform(
            async move {
                client
                    .messages(&channel_id, before.as_deref())
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::History {
                channel_id: id.clone(),
                older,
                result,
            },
        )
    }

    fn send(&mut self) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_send(&channel_id) {
            return Task::none();
        }
        let content = self.composer.trim().to_string();
        if content.is_empty() {
            return Task::none();
        }
        // `:name:` of the channel server's emojis (that server only).
        let content = self.store.prepare_outgoing(&channel_id, &content);
        if content.chars().count() > 2000 {
            self.status = Some((
                t(
                    "The message is longer than 2000 characters.",
                    "Il messaggio supera i 2000 caratteri.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        self.composer.clear();
        self.send_text(content)
    }

    /// Sends a text to the open channel (also the link of a GIF chosen in the picker).
    fn send_text(&mut self, content: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        // Checked again here: the GIF picker sends through this too.
        if !self.store.can_send(&channel_id) {
            self.status = Some((no_send_permission().into(), true));
            return Task::none();
        }
        self.status = None;
        let bottom = scrollable::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 });

        if self.demo {
            let mut msg = crate::demo::local_message(
                &channel_id,
                self.store.me.clone(),
                &content,
                self.store.messages.get(&channel_id),
            );
            // In the demo the GIFs chosen in the picker become an embed right away.
            if let Some(embed) = crate::demo::gif_embed(&content) {
                msg.embeds = vec![embed];
            }
            self.store.insert_message(msg);
            return Task::batch([bottom, self.request_previews()]);
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let text = content.clone();
        let sent = Task::perform(
            async move {
                client
                    .send_message(&channel_id, &text)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::Sent {
                content: content.clone(),
                result,
            },
        );
        Task::batch([sent, bottom])
    }

    // ------------------------------------------------------------ view

    pub fn view(&self) -> Element<'_, Message> {
        let screen = match self.screen {
            Screen::Login => self.view_login(),
            Screen::Main => self.view_main(),
        };
        // Explicit background: the window's does not always follow the theme.
        let base: Element<'_, Message> = container(screen)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(pal::bg()))
            .into();
        // The root is always a `stack` with the chat as first layer, even
        // without the full-screen view: if the structure changed when the view
        // opens or closes, iced would recreate the message list and the
        // scroll position would go back to the bottom.
        let mut layers = stack![base];
        if let Some(p) = self.picker.as_ref().filter(|_| self.screen == Screen::Main) {
            // The panel sits above the message field, on the right; a click
            // outside closes it (`opaque` keeps clicks on the panel from going below).
            let server = self.store.current.as_ref().and_then(|c| {
                let guild = self.store.guild_of(c)?;
                Some(picker::ServerEmojis {
                    name: guild.name(),
                    emojis: self.store.usable_emojis(c),
                    images: &self.small_images,
                })
            });
            let panel = container(opaque(picker::view(
                p,
                &self.previews,
                &self.recent_emojis,
                server,
            )))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: PICKER_BOTTOM,
                left: 0.0,
            });
            layers = layers.push(stack![
                mouse_area(Space::new(Length::Fill, Length::Fill)).on_press(Message::ClosePicker),
                panel
            ]);
        }
        if let Some(v) = self.viewer.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(self.view_viewer(v));
        }
        if let Some(p) = self.profile.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(profile::view(p, &self.store, &self.small_images));
        }
        if let Some(v) = self.stream_view.as_ref().filter(|_| self.screen == Screen::Main) {
            let in_channel = self
                .voice
                .as_ref()
                .is_some_and(|x| x.channel_id == v.stream.channel_id);
            layers = layers.push(stream::view(v, &self.store, &self.small_images, in_channel));
        }
        if let Some(w) = self.watch.as_ref().filter(|_| self.screen == Screen::Main) {
            let voice = self.voice.as_ref();
            let can_speak = voice.is_some_and(|v| self.store.can(&v.channel_id, permissions::SPEAK));
            layers = layers.push(watch::view(w, &self.store, voice, can_speak));
        }
        if let Some(e) = self
            .folder_editor
            .as_ref()
            .filter(|_| self.screen == Screen::Main)
        {
            layers = layers.push(folders::view(e, &self.store, &self.small_images, self.demo));
        }
        if let Some(p) = self
            .settings_panel
            .as_ref()
            .filter(|_| self.screen == Screen::Main)
        {
            layers = layers.push(settings::view(p, &self.settings, &self.store, self.system_dark));
        }
        layers.into()
    }

    fn view_main(&self) -> Element<'_, Message> {
        column![
            self.top_bar(),
            hline(pal::border()),
            row![
                self.server_rail(),
                vline(pal::soft()),
                self.channel_panel(),
                vline(pal::border()),
                self.chat(),
            ]
            .height(Length::Fill),
            hline(pal::border()),
            self.status_bar(),
        ]
        .into()
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let mut crumbs = row![text("ferrocord").font(BOLD)]
            .spacing(10)
            .align_y(Vertical::Center);
        if let Some(c) = &self.store.current {
            let place = match self.store.guild_of(c) {
                Some(g) => g.name().to_string(),
                None => t("Direct messages", "Messaggi diretti").to_string(),
            };
            crumbs = crumbs
                .push(
                    text(place)
                        .size(13)
                        .color(pal::muted())
                        .shaping(Shaping::Advanced),
                )
                .push(text("/").size(13).color(pal::muted()))
                .push(
                    text(self.store.channel_label(c))
                        .size(13)
                        .shaping(Shaping::Advanced),
                );
        }
        let me = self.store.me.as_ref();
        let mut right = row![].spacing(10).align_y(Vertical::Center);
        if let Some(u) = me {
            let image = self
                .store
                .avatar_url(None, &u.id, AVATAR_PX)
                .and_then(|a| self.small_images.get(&a));
            right = right.push(avatar_button(
                u.display_name(),
                &u.id,
                22.0,
                image,
                Message::OpenProfile {
                    user_id: u.id.clone(),
                    guild_id: None,
                },
            ));
        }
        right = right.push(
            tooltip(
                button(text(i18n::lang().code().to_ascii_uppercase()).size(12))
                    .on_press(Message::ToggleLanguage)
                    .padding([3, 8])
                    .style(ghost),
                container(text(t("Language: English", "Lingua: italiano")).size(12))
                    .padding(6)
                    .style(style::tooltip),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
        if !self.demo {
            right = right.push(
                button(text(t("Sign out", "Esci")).size(12))
                    .on_press(Message::Logout)
                    .padding([3, 10])
                    .style(ghost),
            );
        }
        container(row![crumbs, horizontal_space(), right].align_y(Vertical::Center))
            .padding([0, 12])
            .height(36)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(pal::title()))
            .into()
    }

    fn server_rail(&self) -> Element<'_, Message> {
        let mut col = Column::new()
            .spacing(10)
            .padding(Padding {
                top: 12.0,
                right: 0.0,
                bottom: 12.0,
                left: 0.0,
            })
            .align_x(Horizontal::Center)
            .width(Length::Fill);
        col = col.push(rail_button(
            "DM".to_string(),
            t("Direct messages", "Messaggi diretti").to_string(),
            self.guild.is_none(),
            self.store.dms_have_unread(),
            None,
            Message::SelectGuild(None),
        ));
        col = col.push(container(Space::new(24, 1)).style(fill(pal::border())));
        let guild_button = |i: usize| -> Element<'_, Message> {
            let g = &self.store.guilds[i];
            mouse_area(rail_button(
                initials(g.name()),
                g.name().to_string(),
                self.guild == Some(i),
                self.store.guild_has_unread(&g.id),
                media::guild_icon_url(&g.id, g.icon(), ICON_PX).and_then(|u| self.small_images.get(&u)),
                Message::SelectGuild(Some(i)),
            ))
            .on_right_press(Message::OpenFolderEditor)
            .into()
        };
        for entry in self.store.rail() {
            match &entry {
                crate::store::RailEntry::Guild(i) => col = col.push(guild_button(*i)),
                crate::store::RailEntry::Folder { id, .. } => {
                    col = col.push(folders::rail_folder(
                        &self.store,
                        &self.small_images,
                        &entry,
                        self.open_folders.contains(id),
                        self.guild,
                        guild_button,
                    ))
                }
            }
        }
        if !self.store.guilds.is_empty() {
            col = col.push(voice::control_button(
                &icons::FOLDER,
                Control::On,
                t(
                    "Server folders (also right click on a server)",
                    "Cartelle dei server (anche clic destro su un server)",
                )
                .into(),
                Some(Message::OpenFolderEditor),
                32.0,
            ));
        }
        container(scrollable(col).direction(thin()).style(thin_scrollbar))
            .width(64)
            .height(Length::Fill)
            .style(fill(pal::panel()))
            .into()
    }

    fn channel_panel(&self) -> Element<'_, Message> {
        let title = match self.guild.and_then(|i| self.store.guilds.get(i)) {
            Some(g) => g.name().to_string(),
            None => t("Direct messages", "Messaggi diretti").to_string(),
        };
        let header = container(text(title).font(BOLD).shaping(Shaping::Advanced))
            .padding([0, 12])
            .height(40)
            .width(Length::Fill)
            .align_y(Vertical::Center);

        let mut list = Column::new().spacing(1).padding([8, 6]);
        let rows = self.store.channel_rows(self.guild);
        if rows.is_empty() {
            let msg = if self.conn == Conn::Online || self.demo {
                t("No channels", "Nessun canale")
            } else {
                t("Loading…", "Caricamento…")
            };
            list = list.push(container(text(msg).size(13).color(pal::muted())).padding(8));
        }
        for r in rows {
            match r {
                ChannelRow::Category(name) => {
                    list = list.push(
                        container(text(name.to_uppercase()).size(11).font(BOLD).color(pal::muted())).padding(
                            Padding {
                                top: 12.0,
                                right: 4.0,
                                bottom: 4.0,
                                left: 6.0,
                            },
                        ),
                    );
                }
                ChannelRow::Channel {
                    id,
                    name,
                    kind: k,
                    selectable,
                } => {
                    let selected = self.store.current.as_deref() == Some(id.as_str());
                    let unread = self.store.unread.contains(&id);
                    let mut label = text(name.clone()).size(13).shaping(Shaping::Advanced);
                    if unread {
                        label = label.font(BOLD);
                    } else if !selected {
                        label = label.color(pal::muted());
                    }
                    // Private conversations: the other person's avatar (or the
                    // group's picture), small, instead of the "@".
                    let dm = self
                        .guild
                        .is_none()
                        .then(|| self.store.dms.iter().find(|c| c.id == id))
                        .flatten();
                    let is_voice = k == kind::GUILD_VOICE || k == kind::STAGE;
                    // A voice channel we can see but not join: a padlock, as on Discord.
                    let locked = is_voice && !self.store.can_connect(&id);
                    let icon: Element<'_, Message> = match dm {
                        Some(c) => {
                            let (colour_id, url) = self.store.dm_picture(c, AVATAR_PX);
                            let image = url.and_then(|u| self.small_images.get(&u));
                            avatar_with(&name, &colour_id, DM_AVATAR, image)
                        }
                        None => text(if locked { "🔒" } else { channel_icon(k) })
                            .size(13)
                            .color(pal::muted())
                            .width(16)
                            .shaping(Shaping::Advanced)
                            .into(),
                    };
                    let mut line = row![icon, label]
                        .spacing(if dm.is_some() { 8 } else { 4 })
                        .align_y(Vertical::Center);
                    if unread {
                        line = line.push(horizontal_space()).push(dot(pal::accent(), 6.0));
                    }
                    let in_voice = self.voice.as_ref().is_some_and(|v| v.channel_id == id);
                    let press = if selectable {
                        Some(Message::OpenChannel(id.clone()))
                    } else if is_voice && !locked {
                        Some(Message::JoinVoice(id.clone()))
                    } else {
                        None
                    };
                    let item = button(line)
                        .width(Length::Fill)
                        .padding([4, 8])
                        .style(list_item(selected || in_voice))
                        .on_press_maybe(press);
                    list = list.push(item);
                    // Who is in the voice channel, with who is speaking in green.
                    if is_voice {
                        for (user_id, name) in self.store.voice_members(&id) {
                            let speaking = self
                                .voice
                                .as_ref()
                                .is_some_and(|v| v.channel_id == id && v.speaking.contains(&user_id));
                            let mut label = text(name.clone()).size(12).shaping(Shaping::Advanced);
                            label = if speaking {
                                label.color(pal::green()).font(SEMIBOLD)
                            } else {
                                label.color(pal::muted())
                            };
                            let guild = self.store.channel_guild.get(&id).cloned();
                            let image = self
                                .store
                                .avatar_url(guild.as_deref(), &user_id, AVATAR_PX)
                                .and_then(|a| self.small_images.get(&a));
                            let mut member = row![avatar_with(&name, &user_id, 18.0, image), label]
                                .spacing(6)
                                .align_y(Vertical::Center);
                            if speaking {
                                member = member.push(dot(pal::green(), 6.0));
                            }
                            // Streaming (Go Live): the LIVE badge opens the preview.
                            if let (true, Some(g)) = (self.store.streaming.contains(&user_id), &guild) {
                                member = member.push(horizontal_space()).push(stream::live_badge(
                                    Message::OpenStreamPreview(stream::StreamRef {
                                        guild_id: g.clone(),
                                        channel_id: id.clone(),
                                        user_id: user_id.clone(),
                                    }),
                                ));
                            }
                            // Highlighted on hover like the channels; a click opens the profile.
                            list = list.push(
                                container(
                                    button(member)
                                        .width(Length::Fill)
                                        .padding([2, 8])
                                        .style(list_item(false))
                                        .on_press(Message::OpenProfile {
                                            user_id: user_id.clone(),
                                            guild_id: guild,
                                        }),
                                )
                                .padding(Padding {
                                    left: 22.0,
                                    ..Padding::ZERO
                                }),
                            );
                        }
                    }
                }
            }
        }

        let user_panel: Element<'_, Message> = match &self.store.me {
            Some(me) => container(
                row![
                    button(
                        row![
                            avatar_with(
                                me.display_name(),
                                &me.id,
                                28.0,
                                self.store
                                    .avatar_url(None, &me.id, AVATAR_PX)
                                    .and_then(|a| self.small_images.get(&a)),
                            ),
                            column![
                                text(me.display_name().to_string())
                                    .size(13)
                                    .font(SEMIBOLD)
                                    .shaping(Shaping::Advanced),
                                text(match &self.conn {
                                    Conn::Online => "online",
                                    Conn::Connecting => t("connecting…", "connessione…"),
                                    Conn::Error(_) => t("disconnected", "disconnesso"),
                                })
                                .size(11)
                                .color(pal::muted()),
                            ]
                            .spacing(1),
                        ]
                        .spacing(10)
                        .align_y(Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([4, 6])
                    .style(list_item(false))
                    .on_press(Message::OpenProfile {
                        user_id: me.id.clone(),
                        guild_id: None,
                    }),
                    voice::control_button(
                        &icons::GEAR,
                        Control::On,
                        t("Settings (Ctrl + ,)", "Impostazioni (Ctrl + ,)").into(),
                        Some(Message::OpenSettings),
                        30.0,
                    ),
                ]
                .spacing(4)
                .align_y(Vertical::Center),
            )
            .padding([0, 6])
            .height(52)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .into(),
            None => Space::new(Length::Fill, 52).into(),
        };

        let voice_panel: Element<'_, Message> = match &self.voice {
            Some(v) => {
                let name = self
                    .store
                    .channel_names
                    .get(&v.channel_id)
                    .cloned()
                    .unwrap_or_else(|| t("voice channel", "canale vocale").into());
                let can_speak = self.store.can(&v.channel_id, permissions::SPEAK);
                column![voice::panel(v, name, can_speak), hline(pal::soft())].into()
            }
            None => Space::new(0, 0).into(),
        };

        container(column![
            header,
            hline(pal::soft()),
            scrollable(list)
                .direction(thin())
                .style(thin_scrollbar)
                .height(Length::Fill),
            hline(pal::soft()),
            voice_panel,
            user_panel,
        ])
        .width(240)
        .height(Length::Fill)
        .style(fill(pal::panel()))
        .into()
    }

    fn chat(&self) -> Element<'_, Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return container(
                column![
                    text(t("Welcome to ferrocord", "Benvenuto in ferrocord"))
                        .size(18)
                        .font(BOLD),
                    text(t(
                        "Pick a server on the left, then a channel.",
                        "Scegli un server a sinistra, poi un canale."
                    ))
                    .color(pal::muted()),
                ]
                .spacing(8)
                .align_x(Horizontal::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into();
        };

        let topic = match self.store.guild_of(&channel_id) {
            Some(g) => g.name().to_string(),
            None => t("Direct messages", "Messaggi diretti").to_string(),
        };
        let header = container(
            row![
                text(self.store.channel_label(&channel_id))
                    .font(BOLD)
                    .shaping(Shaping::Advanced),
                text(topic).size(13).color(pal::muted()),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
        )
        .padding([0, 16])
        .height(44)
        .width(Length::Fill)
        .align_y(Vertical::Center);

        let body: Element<'_, Message> = match self.store.messages.get(&channel_id) {
            Some(list) if !list.is_empty() => scrollable(self.message_list(list))
                .id(messages_id())
                // `direction` replaces the whole scrollbar, anchoring included:
                // it must be set before `anchor_bottom`.
                .direction(thin())
                .anchor_bottom()
                .on_scroll(Message::Scrolled)
                .style(thin_scrollbar)
                .height(Length::Fill)
                .into(),
            _ => {
                let msg = if self.store.loading.contains(&channel_id) {
                    t("Loading messages…", "Caricamento dei messaggi…")
                } else if !self.store.can(&channel_id, permissions::READ_MESSAGE_HISTORY) {
                    t(
                        "You do not have permission to read the history of this channel: new messages will appear here.",
                        "Non hai il permesso di leggere la cronologia di questo canale: qui compariranno i nuovi messaggi.",
                    )
                } else if !self.store.can_send(&channel_id) {
                    t("No messages.", "Nessun messaggio.")
                } else {
                    t(
                        "No messages: write the first one!",
                        "Nessun messaggio: scrivi il primo!",
                    )
                };
                container(text(msg).color(pal::muted()))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center)
                    .into()
            }
        };

        let placeholder = tf!(
            "Message {}",
            "Scrivi in {}",
            self.store.channel_label(&channel_id)
        );
        // Without the permission to send, a field that cannot be typed in
        // (as on Discord) and no GIF, emoji or send buttons.
        let composer: Element<'_, Message> = if self.store.can_send(&channel_id) {
            row![
                text_input(&placeholder, &self.composer)
                    .id(composer_id())
                    .on_input(Message::ComposerChanged)
                    .on_submit(Message::Send)
                    .padding(11)
                    .style(input),
                self.picker_button(text("GIF").size(13).font(BOLD).into(), picker::Tab::Gif, "GIF"),
                self.picker_button(
                    text("🙂").size(19).shaping(Shaping::Advanced).into(),
                    picker::Tab::Emoji,
                    "Emoji",
                ),
                button(text(t("Send", "Invia")).font(SEMIBOLD))
                    .on_press_maybe((!self.composer.trim().is_empty()).then_some(Message::Send))
                    .padding([10, 16])
                    .style(primary),
            ]
            .spacing(8)
            .align_y(Vertical::Center)
            .into()
        } else {
            text_input(no_send_permission(), "")
                .id(composer_id())
                .padding(11)
                .style(input)
                .into()
        };

        let typing = self.store.typing_in(&channel_id);
        let typing_line = match typing.as_slice() {
            [] => String::new(),
            [one] => tf!("{one} is typing…", "{one} sta scrivendo…"),
            [a, b] => tf!("{a} and {b} are typing…", "{a} e {b} stanno scrivendo…"),
            _ => t("Several people are typing…", "Più persone stanno scrivendo…").to_string(),
        };

        column![
            header,
            hline(pal::soft()),
            body,
            container(composer).padding(Padding {
                top: 8.0,
                right: 16.0,
                bottom: 2.0,
                left: 16.0,
            }),
            container(
                text(typing_line)
                    .size(12)
                    .color(pal::muted())
                    .shaping(Shaping::Advanced)
            )
            .padding([0, 20])
            .height(22),
        ]
        .width(Length::Fill)
        .into()
    }

    fn picker_button<'a>(
        &self,
        content: Element<'a, Message>,
        tab: picker::Tab,
        tip: &'a str,
    ) -> Element<'a, Message> {
        let active = self.picker.as_ref().is_some_and(|p| p.tab == tab);
        tooltip(
            button(container(content).center_x(26).center_y(24))
                .on_press(Message::TogglePicker(tab))
                .padding([8, 8])
                .style(style::tab(active)),
            text(tip).size(12),
            tooltip::Position::Top,
        )
        .style(style::tooltip)
        .into()
    }

    fn message_list<'a>(&'a self, list: &'a [DMessage]) -> Element<'a, Message> {
        let ctx = Ctx {
            users: &self.store.users,
            channels: &self.store.channel_names,
            me: self.store.me_id(),
        };
        let mut col = Column::new().padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 8.0,
            left: 0.0,
        });
        let guild = self
            .store
            .current
            .as_deref()
            .and_then(|c| self.store.channel_guild.get(c))
            .map(String::as_str);
        let mut prev_author: Option<&str> = None;
        let mut prev_time: Option<DateTime<Local>> = None;
        let mut prev_day: Option<NaiveDate> = None;

        for m in list {
            let ts = parse_ts(&m.timestamp);
            let day = ts.map(|t| t.date_naive());
            if let Some(d) = day.filter(|d| Some(*d) != prev_day) {
                col = col.push(day_separator(d));
                prev_author = None;
            }
            let grouped = prev_author == Some(m.author.id.as_str())
                && m.referenced_message.is_none()
                && matches!((prev_time, ts), (Some(a), Some(b)) if (b - a).num_seconds() < GROUP_GAP_SECS);

            let mentions_me = ctx
                .me
                .is_some_and(|me| m.mention_everyone || m.mentions.iter().any(|u| u.id == me));
            let content = message_body(m, &ctx, &self.settings, &self.previews, &self.small_images);
            let time = ts
                .map(|t| t.format(self.settings.time_format()).to_string())
                .unwrap_or_default();

            let inner: Element<'a, Message> = if self.settings.compact {
                // One line: time, name, text; no avatar.
                let name = self.store.name_in(guild, &m.author);
                let open_profile = Message::OpenProfile {
                    user_id: m.author.id.clone(),
                    guild_id: guild.map(str::to_string),
                };
                let line = row![
                    text(time).size(11).color(pal::muted()).width(COMPACT_TIME),
                    button(
                        text(name)
                            .font(SEMIBOLD)
                            .shaping(Shaping::Advanced)
                            .color(user_color(&m.author.id))
                    )
                    .padding([0.0, NAME_PAD])
                    .style(name_link)
                    .on_press(open_profile),
                    content,
                ]
                .spacing(6)
                .align_y(Vertical::Top);
                match &m.referenced_message {
                    Some(r) => column![
                        row![Space::with_width(COMPACT_TIME + 6.0), reply_preview(r, &ctx)],
                        line
                    ]
                    .spacing(2)
                    .into(),
                    None => line.into(),
                }
            } else if grouped {
                // Same left edge as the text of a message with its author.
                row![Space::with_width(32.0 + 4.0 + AUTHOR_GAP + NAME_PAD), content].into()
            } else {
                let name = self.store.name_in(guild, &m.author);
                let open_profile = Message::OpenProfile {
                    user_id: m.author.id.clone(),
                    guild_id: guild.map(str::to_string),
                };
                // The name is a button highlighted on hover (like the channels),
                // with NAME_PAD on the sides: the rest of the column is moved by
                // the same amount so the text stays aligned with the name.
                let indent = |e: Element<'a, Message>| {
                    container(e).padding(Padding {
                        left: NAME_PAD,
                        ..Padding::ZERO
                    })
                };
                let mut right = Column::new().spacing(2);
                if let Some(r) = &m.referenced_message {
                    right = right.push(indent(reply_preview(r, &ctx)));
                }
                right = right
                    .push(
                        row![
                            button(
                                text(name.clone())
                                    .font(SEMIBOLD)
                                    .shaping(Shaping::Advanced)
                                    .color(user_color(&m.author.id))
                            )
                            .padding([1.0, NAME_PAD])
                            .style(name_link)
                            .on_press(open_profile.clone()),
                            text(time).size(11).color(pal::muted()),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .push(indent(content));
                let image = self
                    .store
                    .avatar_url(guild, &m.author.id, AVATAR_PX)
                    .and_then(|a| self.small_images.get(&a));
                let picture = avatar_button(&name, &m.author.id, 32.0, image, open_profile);
                row![picture, right].spacing(AUTHOR_GAP).into()
            };

            let hovered = self.hovered.as_deref() == Some(m.id.as_str());
            let background = if mentions_me {
                Some(pal::mention_bg())
            } else if hovered {
                Some(pal::panel())
            } else {
                None
            };
            let top = if self.settings.compact {
                2.0
            } else if grouped {
                1.0
            } else {
                8.0
            };
            let item = container(inner)
                .width(Length::Fill)
                .padding(Padding {
                    top,
                    right: 12.0,
                    bottom: 2.0,
                    left: 16.0,
                })
                .style(move |_| container::Style {
                    background: background.map(iced::Background::Color),
                    ..Default::default()
                });
            col = col.push(
                mouse_area(item)
                    .on_enter(Message::HoverStart(m.id.clone()))
                    .on_exit(Message::HoverEnd(m.id.clone())),
            );

            prev_author = Some(m.author.id.as_str());
            prev_time = ts;
            prev_day = day;
        }
        col.into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let (color, label) = match &self.conn {
            _ if self.demo => (style::pal::yellow(), "Demo".to_string()),
            Conn::Online => (pal::green(), t("Connected", "Connesso").to_string()),
            Conn::Connecting => (pal::yellow(), t("Connecting…", "Connessione…").to_string()),
            Conn::Error(e) => (
                pal::red(),
                tf!("Disconnected: {e} — retrying…", "Disconnesso: {e} — riprovo…"),
            ),
        };
        let mut left = row![dot(color, 8.0), text(label).size(12).color(pal::muted())]
            .spacing(6)
            .align_y(Vertical::Center);
        if let Some((msg, is_error)) = &self.status {
            left = left.push(text("·").size(12).color(pal::muted())).push(
                text(msg.clone())
                    .size(12)
                    .color(if *is_error { pal::red() } else { pal::accent() }),
            );
        }
        container(left)
            .padding([0, 10])
            .height(24)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(pal::panel()))
            .into()
    }

    fn view_login(&self) -> Element<'_, Message> {
        let tabs = row![
            button(text("QR code").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Qr))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Qr)),
            button(text("Token").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Token))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Token)),
        ]
        .spacing(8);

        let body: Element<'_, Message> = match self.login.tab {
            LoginTab::Qr => match &self.login.qr {
                QrView::Working(msg) => text(msg.clone()).color(pal::muted()).into(),
                QrView::Ready(data) => column![
                    container(qr_code(data).cell_size(5).style(|_| qr_code::Style {
                        cell: iced::Color::BLACK,
                        background: iced::Color::WHITE,
                    }))
                    .padding(12)
                    .style(rounded(iced::Color::WHITE, 10.0)),
                    text(t(
                        "Scan the code with the Discord app on your phone",
                        "Inquadra il codice con l'app Discord sul telefono"
                    )),
                    text(t("Settings → Scan QR Code", "Impostazioni → Scansiona codice QR"))
                        .size(12)
                        .color(pal::muted()),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Scanned(user) => column![
                    text(tf!("✓ Code scanned by {user}", "✓ Codice scansionato da {user}"))
                        .color(pal::green()),
                    text(t(
                        "Confirm the login on your phone…",
                        "Conferma l'accesso sul telefono…"
                    ))
                    .color(pal::muted()),
                ]
                .spacing(8)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Failed(err) => column![
                    text(err.clone()).color(pal::red()),
                    button(text(t("New code", "Nuovo codice")))
                        .on_press(Message::RetryQr)
                        .padding([6, 14])
                        .style(ghost),
                    text(t("Or use the token.", "Oppure usa il token."))
                        .size(12)
                        .color(pal::muted()),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
            },
            LoginTab::Token => {
                let path = config::token_location();
                let mut col = column![
                    text(t(
                        "Paste your account's token.",
                        "Incolla il token del tuo account."
                    )),
                    text_input("Token", &self.login.token)
                        .secure(true)
                        .on_input(Message::TokenInput)
                        .on_submit(Message::SubmitToken)
                        .padding(10)
                        .style(input),
                    button(
                        text(if self.login.busy {
                            t("Checking…", "Verifica…")
                        } else {
                            t("Sign in", "Accedi")
                        })
                        .font(SEMIBOLD)
                    )
                    .on_press_maybe((!self.login.busy).then_some(Message::SubmitToken))
                    .padding([8, 18])
                    .style(primary),
                    text(tf!(
                        "It will be saved in {path}, protected by the system.",
                        "Verrà salvato in {path}, protetto dal sistema."
                    ))
                    .size(11)
                    .color(pal::muted()),
                ]
                .spacing(12)
                .align_x(Horizontal::Center);
                if let Some(e) = &self.login.error {
                    col = col.push(text(e.clone()).color(pal::red()));
                }
                col.into()
            }
        };

        let language = button(text(tf!("Language: {}", "Lingua: {}", i18n::lang().name())).size(12))
            .on_press(Message::ToggleLanguage)
            .padding([3, 10])
            .style(ghost);
        let mut card = column![text("ferrocord").size(28).font(BOLD), tabs]
            .spacing(18)
            .align_x(Horizontal::Center)
            .width(420);
        if let Some(n) = &self.login.notice {
            card = card.push(text(n.clone()).color(pal::yellow()));
        }
        card = card
            .push(container(body).width(Length::Fill).align_x(Horizontal::Center))
            .push(
                text(t(
                    "Unofficial client: using it may violate Discord's Terms of Service.",
                    "Client non ufficiale: usarlo può violare i Termini di Servizio di Discord.",
                ))
                .size(11)
                .color(pal::muted()),
            )
            .push(language);

        container(container(card).padding(28).style(style::card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    }
}

// ---------------------------------------------------------------- subscriptions

/// Gateway as a stream of messages: it stops by itself when iced drops the subscription.
fn gateway_stream(token: String) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<GatewayEvent>();
        let (link, commands) = gateway::commands();
        let _ = output.send(Message::GatewayLink(link)).await;
        let worker = gateway::run(token, tx, commands);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Gateway(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

fn voice_stream(server: voice_net::VoiceServer, audio: AudioSettings) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let worker = voice_net::run(server, voice_net::VoiceOptions { devices: true, audio }, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Voice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Connection to a Go Live stream: audio and video; pictures go to `sink`.
fn watch_stream(
    server: voice_net::VoiceServer,
    sink: crate::video::decoder::Sink,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    // The sink is used once: the subscription with the same ID is not restarted.
    let sink = std::sync::Mutex::new(Some(sink));
    iced::stream::channel(64, move |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let sink = sink.lock().ok().and_then(|mut s| s.take());
        let worker =
            voice_net::run_with_video(server, voice_net::VoiceOptions { devices: true, audio }, sink, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::WatchVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Why Discord ended a stream we watched, in words.
fn stream_end_reason(reason: Option<&str>) -> String {
    match reason {
        Some("stream_full") => t(
            "The stream is full: too many people are watching.",
            "La diretta è piena: la guardano troppe persone.",
        )
        .into(),
        Some("unauthorized") => t(
            "You cannot watch this stream.",
            "Non puoi guardare questa diretta.",
        )
        .into(),
        _ => t("The stream has ended.", "La diretta è finita.").into(),
    }
}

/// Demo: local fake voice server, with the others in the channel talking in
/// turn. The client is the same one used with Discord.
fn voice_demo_stream(
    guild_id: String,
    channel_id: String,
    me: String,
    others: Vec<String>,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output| async move {
        let config = voice_net::mock::MockConfig {
            users: others,
            channel_id: channel_id.parse().unwrap_or(voice_net::mock::CHANNEL_ID),
            ..Default::default()
        };
        let mock = match voice_net::mock::start(config).await {
            Ok(m) => m,
            Err(e) => {
                let _ = output
                    .send(Message::Voice(VoiceEvent::Closed(Some(format!(
                        "demo voice server not started: {e:#}"
                    )))))
                    .await;
                return;
            }
        };
        let server = voice_net::VoiceServer {
            endpoint: mock.endpoint.clone(),
            token: "demo".into(),
            guild_id,
            channel_id,
            session_id: "demo".into(),
            user_id: me,
            stream: None,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        // Speakers play the synthetic voices; the microphone only opens on request.
        let worker = voice_net::run(server, voice_net::VoiceOptions { devices: true, audio }, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Voice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
        drop(mock);
    })
}

fn qr_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(16, |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AuthEvent>();
        let worker = remote_auth::run(tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Auth(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

// ---------------------------------------------------------------- interface pieces

fn thin() -> scrollable::Direction {
    scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(6).scroller_width(6).margin(2))
}

fn hline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new(Length::Fill, 1)).style(fill(color)).into()
}

fn vline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new(1, Length::Fill)).style(fill(color)).into()
}

fn dot<'a>(color: iced::Color, size: f32) -> Element<'a, Message> {
    container(Space::new(size, size))
        .style(rounded(color, size / 2.0))
        .into()
}

/// Avatar: the picture when it has arrived, otherwise the initials on the
/// user's colour.
fn avatar_with<'a>(name: &str, id: &str, size: f32, image: Option<&Preview>) -> Element<'a, Message> {
    if let Some(Preview::Ready(handle)) = image {
        return img::Image::new(handle.clone()).width(size).height(size).into();
    }
    avatar(name, id, size)
}

/// Avatar that opens the profile: a light ring around it on hover.
fn avatar_button<'a>(
    name: &str,
    id: &str,
    size: f32,
    image: Option<&Preview>,
    on_press: Message,
) -> Element<'a, Message> {
    button(avatar_with(name, id, size, image))
        .padding(2)
        .style(style::avatar_ring(size))
        .on_press(on_press)
        .into()
}

fn avatar<'a>(name: &str, id: &str, size: f32) -> Element<'a, Message> {
    container(
        text(initials(name))
            .size(size * 0.42)
            .font(SEMIBOLD)
            .color(pal::bg())
            .shaping(Shaping::Advanced),
    )
    .width(size)
    .height(size)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(rounded(user_color(id), size / 2.0))
    .into()
}

fn rail_button<'a>(
    label: String,
    name: String,
    selected: bool,
    unread: bool,
    image: Option<&'a Preview>,
    on_press: Message,
) -> Element<'a, Message> {
    // Small bar on the left: tall if selected, a dot if there are unread messages.
    let marker_height = if selected {
        22.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    let marker = container(Space::new(3, marker_height)).style(rounded(pal::text(), 2.0));
    // The server icon, already cut to a rounded square; until it arrives (or
    // if the server has none) the initials.
    let icon = match image {
        Some(Preview::Ready(handle)) => button(img::Image::new(handle.clone()).width(40).height(40))
            .width(40)
            .height(40)
            .padding(0)
            .style(style::bare),
        _ => button(
            container(text(label).size(13).font(SEMIBOLD).shaping(Shaping::Advanced))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .width(40)
        .height(40)
        .padding(0)
        .style(server(selected)),
    }
    .on_press(on_press);
    let with_tip = tooltip(
        icon,
        text(name).size(12).shaping(Shaping::Advanced),
        tooltip::Position::Right,
    )
    .padding(6)
    .style(style::tooltip);
    row![marker, Space::with_width(7), with_tip, Space::with_width(10)]
        .align_y(Vertical::Center)
        .into()
}

fn no_send_permission() -> &'static str {
    t(
        "You do not have permission to send messages in this channel.",
        "Non hai il permesso di inviare messaggi in questo canale.",
    )
}

fn channel_icon(k: u8) -> &'static str {
    match k {
        kind::GUILD_VOICE | kind::STAGE => "♪",
        kind::ANNOUNCEMENT => "»",
        kind::DM | kind::GROUP_DM => "@",
        _ => "#",
    }
}

fn parse_ts(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&Local))
}

fn day_separator<'a>(day: NaiveDate) -> Element<'a, Message> {
    use chrono::Datelike;
    let label = format!(
        "{} {} {}",
        day.day(),
        i18n::month(i18n::lang(), day.month()),
        day.year()
    );
    container(
        row![
            hline(pal::soft()),
            text(label).size(12).color(pal::muted()),
            hline(pal::soft())
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding(Padding {
        top: 10.0,
        right: 4.0,
        bottom: 2.0,
        left: 16.0,
    })
    .into()
}

fn reply_preview<'a>(r: &crate::discord::models::MessagePreview, ctx: &Ctx) -> Element<'a, Message> {
    let who = r
        .author
        .as_ref()
        .map(|u| u.display_name())
        .unwrap_or("?")
        .to_string();
    let full = crate::text::render_content(&r.content, ctx).replace('\n', " ");
    let mut content: String = full.chars().take(90).collect();
    if content.len() < full.len() {
        content.push('…');
    }
    row![
        text("↳").size(12).color(pal::muted()).shaping(Shaping::Advanced),
        text(format!("{who}: {content}"))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
    ]
    .spacing(6)
    .into()
}

fn message_body<'a>(
    m: &DMessage,
    ctx: &Ctx,
    prefs: &Settings,
    previews: &'a HashMap<String, Preview>,
    small_images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(6);
    // Without images, embeds are not shown (their link stays in the text).
    let embeds: Vec<media::EmbedPreview> = if prefs.show_images {
        m.embeds.iter().filter_map(media::embed_preview).collect()
    } else {
        Vec::new()
    };
    // As on Discord: if the message is only a GIF link, only the GIF is shown.
    let only_link = embeds
        .iter()
        .any(|e| e.link.as_deref() == Some(m.content.trim()) && !m.content.trim().is_empty());

    if !m.content.is_empty() && !only_link {
        let segs = segments(&m.content, ctx);
        let edited = m.edited_timestamp.is_some();
        if segs.iter().any(|s| s.emoji.is_some()) {
            col = col.push(content_with_emojis(segs, edited, small_images));
        } else {
            let mut spans: Vec<_> = segs
                .into_iter()
                .map(|seg| styled_span(seg.text, seg.kind))
                .collect();
            if edited {
                spans.push(span(t(" (edited)", " (modificato)")).size(11).color(pal::muted()));
            }
            col = col.push(rich_text(spans).size(14));
        }
    } else if m.attachments.is_empty() && embeds.is_empty() {
        let note = if m.kind == 0 {
            t("(empty message)", "(messaggio vuoto)")
        } else {
            t("(system message)", "(messaggio di sistema)")
        };
        col = col.push(text(note).size(13).color(pal::muted()));
    }

    for a in &m.attachments {
        if a.is_image() && prefs.show_images {
            let state = preview_key(a).and_then(|k| previews.get(&k));
            if !matches!(state, Some(Preview::Failed)) {
                col = col.push(image_preview(a, state));
                continue;
            }
        }
        col = col.push(
            container(
                row![
                    text(t("Attachment", "Allegato")).size(11).color(pal::muted()),
                    text(a.filename.clone())
                        .size(13)
                        .color(pal::accent())
                        .shaping(Shaping::Advanced),
                    text(human_size(a.size)).size(11).color(pal::muted()),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            )
            .padding([8, 12])
            .style(outlined),
        );
    }

    for e in &embeds {
        let state = embed_source(e, previews).and_then(|k| previews.get(k));
        let (w, h) = media::display_size(e.width, e.height);
        let caption: Element<'a, Message> = match &e.provider {
            Some(name) => text(name.clone())
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced)
                .into(),
            None => Space::new(0, 0).into(),
        };
        col = col.push(column![media_box(state, w, h, None), caption].spacing(4));
    }

    if !m.reactions.is_empty() && prefs.show_reactions {
        let chips = m.reactions.iter().fold(Row::new().spacing(6), |r, re| {
            let label: Element<'a, Message> = match (&re.emoji.id, &re.emoji.name) {
                (Some(id), name) => row![
                    custom_emoji(
                        small_images,
                        id,
                        name.as_deref().unwrap_or("emoji"),
                        re.emoji.animated,
                        16.0
                    ),
                    text(re.count.to_string()).size(12),
                ]
                .spacing(5)
                .align_y(Vertical::Center)
                .into(),
                _ => text(format!("{} {}", re.emoji.label(), re.count))
                    .size(12)
                    .shaping(Shaping::Advanced)
                    .into(),
            };
            r.push(button(label).padding([2, 10]).style(reaction(re.me)))
        });
        col = col.push(chips);
    }
    col.into()
}

/// Style of a piece of message text.
fn styled_span(t: String, kind: SegmentKind) -> text::Span<'static, Message, iced::Font> {
    match kind {
        SegmentKind::Plain => span(t),
        SegmentKind::Mention(mine) => span(t)
            .font(SEMIBOLD)
            .color(if mine { pal::yellow() } else { pal::accent() })
            .background(pal::chip()),
        SegmentKind::Channel => span(t).color(pal::accent()).background(pal::chip()),
        SegmentKind::Emoji => span(t).color(pal::yellow()),
        SegmentKind::Time => span(t).color(pal::muted()),
    }
}

/// Width of a space between words when the text is split to make room for
/// the emoji images.
const WORD_SPACE: f32 = 4.0;

/// Text with server emojis. If the message is made only of emojis they are
/// shown large, as on Discord; otherwise they sit in the text, which wraps
/// word by word.
fn content_with_emojis<'a>(
    segs: Vec<Segment>,
    edited: bool,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let edited_note = || text(t(" (edited)", " (modificato)")).size(11).color(pal::muted());
    let count = segs.iter().filter(|s| s.emoji.is_some()).count();
    let only_emojis = segs.iter().all(|s| s.emoji.is_some() || s.text.trim().is_empty());
    if only_emojis && count <= 27 {
        let mut r = Row::new().spacing(4).align_y(Vertical::Center);
        for e in segs.iter().filter_map(|s| s.emoji.as_ref()) {
            r = r.push(custom_emoji(images, &e.id, &e.name, e.animated, 44.0));
        }
        if edited {
            r = r.push(edited_note());
        }
        return r.wrap().into();
    }

    let new_line = || Row::new().align_y(Vertical::Center);
    let mut lines = Column::new().spacing(2);
    let mut line = new_line();
    for seg in segs {
        if let Some(e) = &seg.emoji {
            line = line.push(custom_emoji(images, &e.id, &e.name, e.animated, 22.0));
            continue;
        }
        let mut parts = seg.text.split('\n').peekable();
        while let Some(part) = parts.next() {
            for piece in part.split_inclusive(' ') {
                let word = piece.trim_end_matches(' ');
                if !word.is_empty() {
                    line = line.push(rich_text(vec![styled_span(word.to_string(), seg.kind)]).size(14));
                }
                for _ in 0..piece.len() - word.len() {
                    line = line.push(Space::with_width(WORD_SPACE));
                }
            }
            if parts.peek().is_some() {
                lines = lines.push(std::mem::replace(&mut line, new_line()).wrap());
            }
        }
    }
    if edited {
        line = line.push(edited_note());
    }
    lines.push(line.wrap()).into()
}

/// Image of a server emoji; until it arrives it takes up its space, if it
/// cannot be downloaded the name stays.
fn custom_emoji<'a>(
    images: &'a HashMap<String, Preview>,
    id: &str,
    name: &str,
    is_animated: bool,
    size: f32,
) -> Element<'a, Message> {
    match images.get(&media::emoji_url(id, is_animated)) {
        Some(Preview::Ready(handle)) => img::Image::new(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Preview::Animated(a)) => animated(a).width(size).height(size).into(),
        Some(Preview::Failed) => text(format!(":{name}:")).size(14).color(pal::yellow()).into(),
        _ => Space::new(size, size).into(),
    }
}

/// Fake emoji for the demo: a coloured circle (or the demo GIF if animated).
fn demo_emoji(url: &str, is_animated: bool) -> Preview {
    if is_animated {
        return anim::demo_animation(48, 48).map_or(Preview::Failed, Preview::Animated);
    }
    let seed = url
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let colors = [[222, 165, 96], [116, 173, 232], [161, 193, 129], [197, 159, 224]];
    let [r, g, b] = colors[seed as usize % colors.len()];
    let mut px = Vec::with_capacity(48 * 48 * 4);
    for y in 0..48i32 {
        for x in 0..48i32 {
            let (dx, dy) = (x - 24, y - 24);
            let inside = dx * dx + dy * dy < 21 * 21;
            px.extend_from_slice(&[r, g, b, if inside { 255 } else { 0 }]);
        }
    }
    Preview::Ready(img::Handle::from_rgba(48, 48, px))
}

/// Key of the preview in the cache: the address to download.
fn preview_key(a: &crate::discord::models::Attachment) -> Option<String> {
    if a.url == crate::demo::DEMO_IMAGE_URL {
        return Some(a.url.clone());
    }
    media::preview_url(a)
}

/// Size at which to decode a GIF: that of the preview in screen pixels
/// (`scale` = the screen's scale factor), without exceeding the original.
/// More would only cost memory: every frame is kept decoded while visible.
fn decode_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let (w, h) = media::display_size(width, height);
    (
        ((w * scale).ceil() as u32).min(width.max(1)),
        ((h * scale).ceil() as u32).min(height.max(1)),
    )
}

/// First usable address for a link preview: the animated GIF, or the static
/// image if the GIF could not be downloaded.
fn embed_source<'p>(e: &'p media::EmbedPreview, previews: &HashMap<String, Preview>) -> Option<&'p str> {
    e.sources
        .iter()
        .map(String::as_str)
        .find(|url| !matches!(previews.get(*url), Some(Preview::Failed)))
}

/// Downloads a preview; if it is an animated GIF it decodes it on a separate thread.
/// Kind of small image to download.
#[derive(Debug, Clone, Copy)]
enum SmallKind {
    Emoji {
        animated: bool,
    },
    /// Avatar, icon or banner: decoded here, scaled and cut to shape.
    Shaped(media::Shape, u32, u32),
}

/// Downloads an avatar, icon or banner and cuts it to shape on a separate thread.
async fn load_shaped(url: String, w: u32, h: u32, shape: media::Shape) -> anyhow::Result<Loaded> {
    let bytes = media::fetch(&url, media::PREVIEW_MAX_BYTES).await?;
    tokio::task::spawn_blocking(move || {
        let rgba = media::shaped_image(&bytes, w, h, shape)?;
        Ok(Loaded::Pixels {
            width: w,
            height: h,
            rgba,
        })
    })
    .await?
}

/// Fake avatar or icon for the demo: the demo picture cut to shape.
fn demo_shaped(w: u32, h: u32, shape: media::Shape) -> Preview {
    let mut px = media::demo_pixels(w, h);
    media::cut_shape(&mut px, w, h, shape);
    Preview::Ready(img::Handle::from_rgba(w, h, px))
}

async fn load_preview(url: String, w: u32, h: u32) -> anyhow::Result<Loaded> {
    let bytes = media::fetch(&url, media::PREVIEW_MAX_BYTES).await?;
    if !media::is_animatable(&bytes) {
        return Ok(Loaded::Image(bytes));
    }
    // Only the first frame now: the others are decoded when the GIF is on
    // screen (see `anim`).
    tokio::task::spawn_blocking(move || match Animation::from_source(&bytes, w, h) {
        Ok(Some(a)) => Ok(Loaded::Animated(a)),
        Ok(None) => Ok(Loaded::Image(bytes)),
        Err(e) => Err(e),
    })
    .await?
}

/// Image box: animated, static or loading, always the same size (so the
/// chat does not jump when the image arrives).
fn media_box<'a>(
    state: Option<&'a Preview>,
    w: f32,
    h: f32,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let picture: Element<'a, Message> = match state {
        Some(Preview::Ready(handle)) => img::Image::new(handle.clone())
            .width(w)
            .height(h)
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Preview::Animated(animation)) => animated(animation).width(w).height(h).into(),
        _ => {
            return container(
                text(t("Loading preview…", "Caricamento anteprima…"))
                    .size(12)
                    .color(pal::muted()),
            )
            .width(w)
            .height(h)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(outlined)
            .into()
        }
    };
    match on_press {
        Some(message) => mouse_area(picture)
            .on_press(message)
            .interaction(mouse::Interaction::Pointer)
            .into(),
        None => picture,
    }
}

/// Preview of an attached image, with the file name and size below.
fn image_preview<'a>(
    a: &crate::discord::models::Attachment,
    state: Option<&'a Preview>,
) -> Element<'a, Message> {
    let (w, h) = media::display_size(a.width.unwrap_or(1), a.height.unwrap_or(1));
    let picture = media_box(state, w, h, Some(Message::OpenViewer(Box::new(a.clone()))));
    let caption = row![
        text(a.filename.clone())
            .size(11)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
        text(human_size(a.size)).size(11).color(pal::muted()),
    ]
    .spacing(8);
    column![picture, caption].spacing(4).into()
}

/// Opens the folder of the downloaded file in the system file manager.
fn show_in_folder(path: &std::path::Path) {
    use std::process::Command;
    // No shell: the path is passed as an argument.
    let result = if cfg!(windows) {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else {
        Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open the folder: {e}");
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0 => String::new(),
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Gui {
        Gui::new(Options {
            demo: true,
            ..Default::default()
        })
        .0
    }

    #[test]
    fn demo_opens_on_general() {
        let gui = demo();
        assert_eq!(gui.store.current.as_deref(), Some("1001"));
        // The demo image is generated locally, no download.
        assert!(matches!(
            gui.previews.get(crate::demo::DEMO_IMAGE_URL),
            Some(Preview::Ready(_))
        ));
        assert_eq!(gui.downloads, 0);
        assert_eq!(gui.title(), "#general · Rust Community — ferrocord");
        let _ = gui.view();
    }

    #[test]
    fn stream_preview_from_the_live_badge() {
        let mut gui = demo();
        // Giulia is streaming in the demo's voice channel.
        assert_eq!(gui.store.stream_key("901").as_deref(), Some("guild:100:1021:901"));
        assert!(gui.store.stream_key("902").is_none());
        let _ = gui.view();
        let s = stream::StreamRef {
            guild_id: "100".into(),
            channel_id: "1021".into(),
            user_id: "901".into(),
        };
        let _ = gui.update(Message::OpenStreamPreview(s.clone()));
        let url = gui
            .stream_view
            .as_ref()
            .and_then(|v| v.image_url())
            .unwrap()
            .to_string();
        assert!(matches!(gui.small_images.get(&url), Some(Preview::Ready(_))));
        let _ = gui.view();
        // Too soon to ask again.
        assert!(!gui.stream_view.as_ref().unwrap().can_refresh());
        // A late answer for another stream does not touch the card.
        let other = stream::StreamRef {
            user_id: "902".into(),
            ..s.clone()
        };
        let _ = gui.update(Message::StreamPreviewLoaded {
            stream: other,
            result: Ok(None),
        });
        assert!(gui.stream_view.as_ref().unwrap().image_url().is_some());
        // The stream ends while the card is open: no crash, the card says so.
        gui.store.streaming.remove("901");
        let _ = gui.view();
        let _ = gui.update(Message::CloseStreamPreview);
        assert!(gui.stream_view.is_none());
    }

    #[test]
    fn server_folders_in_the_demo() {
        use crate::store::RailEntry;
        let mut gui = demo();
        // Rust Community alone, then the "Hobby" folder with the other two.
        let rail = gui.store.rail();
        assert!(matches!(rail[0], RailEntry::Guild(0)));
        let RailEntry::Folder { id, guilds, name, .. } = &rail[1] else {
            panic!("no folder")
        };
        assert_eq!((name.as_deref(), guilds.as_slice()), (Some("Hobby"), &[1, 2][..]));
        let id = *id;
        let _ = gui.view();
        let _ = gui.update(Message::ToggleFolder(id));
        assert!(gui.open_folders.contains(&id));
        let _ = gui.view();
        // Editor: Rust Community into the folder, rename, save (demo: only here).
        let _ = gui.update(Message::OpenFolderEditor);
        let _ = gui.view();
        let _ = gui.update(Message::FolderEdit(folders::Edit::Move(
            "100".into(),
            folders::Target::Folder(id, "Hobby".into()),
        )));
        let _ = gui.update(Message::FolderEdit(folders::Edit::Rename(id, "Tutto".into())));
        let _ = gui.view();
        let _ = gui.update(Message::SaveFolders);
        assert!(gui.folder_editor.is_none());
        let rail = gui.store.rail();
        assert_eq!(rail.len(), 1);
        assert!(
            matches!(&rail[0], RailEntry::Folder { name, guilds, .. } if name.as_deref() == Some("Tutto") && guilds.len() == 3)
        );
        // A settings update from another device (partial, folders only) wins.
        let mut gf = crate::discord::folders::GuildFolders::default();
        gf.normalize(&gui.store.guild_ids());
        let proto = crate::discord::folders::encode_folders_update(&gf);
        let ev: gateway::SettingsProtoUpdate = serde_json::from_value(serde_json::json!({
            "settings": {"type": 1, "proto": proto}, "partial": true
        }))
        .unwrap();
        let _ = gui.update(Message::Gateway(GatewayEvent::SettingsProto(ev)));
        assert_eq!(gui.store.rail().len(), 3);
    }

    #[test]
    fn settings_window_in_the_demo() {
        let mut gui = demo();
        let _ = gui.update(Message::OpenSettings);
        assert!(gui.settings_panel.is_some());
        for section in [
            settings::Section::Account,
            settings::Section::Appearance,
            settings::Section::Accessibility,
            settings::Section::Voice,
            settings::Section::TextImages,
            settings::Section::Language,
            settings::Section::Shortcuts,
            settings::Section::About,
        ] {
            let _ = gui.update(Message::SettingsSection(section));
            let _ = gui.view();
        }
        // Compact messages, 12-hour clock, no images, no reactions: still drawn.
        for change in [
            settings::Change::Compact(true),
            settings::Change::Clock24h(false),
            settings::Change::ShowImages(false),
            settings::Change::ShowReactions(false),
            settings::Change::Autoplay(false),
            settings::Change::OutputVolume(150.0),
        ] {
            let _ = gui.update(Message::Setting(change));
        }
        assert!(gui.settings.compact && !gui.settings.show_images);
        assert_eq!(gui.settings.audio.output_volume, 1.5);
        let _ = gui.update(Message::CloseSettings);
        assert!(gui.settings_panel.is_none());
        let _ = gui.view();
        // Each theme can be drawn; back to the dark one for the other tests.
        for theme in ThemeChoice::ALL {
            let _ = gui.update(Message::Setting(settings::Change::Theme(theme)));
            let _ = gui.view();
        }
        let _ = gui.update(Message::Setting(settings::Change::Theme(ThemeChoice::Light)));
        assert_eq!(style::pal::bg(), style::colors_of(ThemeChoice::Light, true).bg);
        let _ = gui.update(Message::Setting(settings::Change::Theme(ThemeChoice::Dark)));
        anim::set_autoplay(true);
    }

    #[test]
    fn watching_a_stream_in_the_demo() {
        let mut gui = demo();
        let s = stream::StreamRef {
            guild_id: "100".into(),
            channel_id: "1021".into(),
            user_id: "901".into(),
        };
        let _ = gui.update(Message::OpenStreamPreview(s));
        let _ = gui.update(Message::WatchStream);
        // Watching needs the voice channel: joined, microphone off.
        assert_eq!(gui.voice.as_ref().map(|v| v.channel_id.as_str()), Some("1021"));
        assert!(gui.stream_view.is_none());
        let w = gui.watch.as_ref().expect("watching");
        assert_eq!(w.key, "guild:100:1021:901");
        // The demo's moving picture arrives.
        let mut waited = 0;
        while !w.frames.has_picture() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
            assert!(waited < 300, "no picture");
        }
        assert_eq!(w.frames.size(), Some((640, 360)));
        let _ = gui.view();
        // Volume: slider, arrow steps (up to 200%), mute keeps the level.
        let volume = |gui: &Gui| {
            gui.watch
                .as_ref()
                .map(|w| (w.volume, w.effective_volume()))
                .unwrap()
        };
        let _ = gui.update(Message::StreamVolume(150.0));
        assert_eq!(volume(&gui), (1.5, 1.5));
        let _ = gui.update(Message::StreamVolumeStep(1));
        assert!((volume(&gui).0 - 1.6).abs() < 1e-6);
        for _ in 0..10 {
            let _ = gui.update(Message::StreamVolumeStep(1));
        }
        assert_eq!(volume(&gui), (2.0, 2.0));
        let _ = gui.update(Message::ToggleStreamSound);
        assert_eq!(volume(&gui), (2.0, 0.0));
        let _ = gui.view();
        let _ = gui.update(Message::StreamVolumeStep(-1));
        assert!((volume(&gui).1 - 1.9).abs() < 1e-6, "a change unmutes");
        // Discord ends the stream: the view says so.
        let _ = gui.update(Message::Gateway(GatewayEvent::StreamDelete(
            serde_json::from_str(r#"{"stream_key": "guild:100:1021:901", "reason": "stream_full"}"#).unwrap(),
        )));
        assert!(matches!(
            gui.watch.as_ref().unwrap().status,
            watch::Status::Ended(_)
        ));
        let _ = gui.view();
        // Stopping keeps us in the voice channel.
        let _ = gui.update(Message::StopWatching);
        assert!(gui.watch.is_none());
        assert!(gui.voice.is_some());
        // Leaving the channel also stops watching.
        let _ = gui.update(Message::OpenStreamPreview(stream::StreamRef {
            guild_id: "100".into(),
            channel_id: "1021".into(),
            user_id: "901".into(),
        }));
        let _ = gui.update(Message::WatchStream);
        // The next stream starts at the last volume chosen.
        assert!((gui.watch.as_ref().unwrap().volume - 1.9).abs() < 1e-6);
        let _ = gui.update(Message::LeaveVoice);
        assert!(gui.watch.is_none());
    }

    #[test]
    fn permissions_in_the_interface() {
        let mut gui = demo();
        // Read-only channel: nothing is sent, not even a GIF.
        let _ = gui.update(Message::OpenChannel("1004".into()));
        let before = gui.store.messages.get("1004").map_or(0, Vec::len);
        let _ = gui.update(Message::ComposerChanged("hello".into()));
        let _ = gui.update(Message::Send);
        let _ = gui.send_text("https://tenor.com/view/x-1".into());
        assert_eq!(gui.store.messages.get("1004").map_or(0, Vec::len), before);
        let _ = gui.view();

        // Voice channel we cannot join: no join, an explanation.
        let _ = gui.update(Message::JoinVoice("1023".into()));
        assert!(gui.voice.is_none());
        assert!(gui.status.as_ref().is_some_and(|(_, error)| *error));
        let _ = gui.view();

        // The open channel becomes hidden: the client moves to a visible one.
        let _ = gui.update(Message::OpenChannel("1001".into()));
        let mut ch = gui.store.guilds[0]
            .channels
            .iter()
            .find(|c| c.id == "1001")
            .unwrap()
            .clone();
        ch.permission_overwrites =
            serde_json::from_str(r#"[{"id": "100", "type": 0, "allow": "0", "deny": "1024"}]"#).unwrap();
        let _ = gui.update(Message::Gateway(GatewayEvent::ChannelUpdate(Box::new(ch))));
        assert_ne!(gui.store.current.as_deref(), Some("1001"));
        assert!(gui.store.current.is_some());
        let _ = gui.view();
    }

    #[test]
    fn navigation_and_sending() {
        let mut gui = demo();
        let _ = gui.update(Message::SelectGuild(Some(1)));
        assert_eq!(gui.store.current.as_deref(), Some("2001"));
        let _ = gui.view();

        let _ = gui.update(Message::SelectGuild(None));
        assert_eq!(gui.store.current.as_deref(), Some("5001"));
        let _ = gui.view();

        let _ = gui.update(Message::OpenChannel("1001".into()));
        let _ = gui.update(Message::ComposerChanged("  hello from the GUI  ".into()));
        let _ = gui.update(Message::Send);
        assert_eq!(
            gui.store.messages["1001"].last().unwrap().content,
            "hello from the GUI"
        );
        assert!(gui.composer.is_empty());

        let id = gui.store.messages["1001"][0].id.clone();
        let _ = gui.update(Message::HoverStart(id.clone()));
        assert_eq!(gui.hovered.as_deref(), Some(id.as_str()));
        let _ = gui.view();
        let _ = gui.update(Message::HoverEnd(id));
        assert!(gui.hovered.is_none());
    }

    #[test]
    fn login_screen() {
        let mut gui = Gui::new(Options {
            demo_login: true,
            ..Default::default()
        })
        .0;
        assert_eq!(gui.screen, Screen::Login);
        let _ = gui.view();
        let _ = gui.update(Message::LoginTab(LoginTab::Token));
        let _ = gui.update(Message::TokenInput("Bot abc".into()));
        let _ = gui.update(Message::SubmitToken);
        assert!(gui.login.error.is_some());
        let _ = gui.view();
    }

    #[test]
    fn previews_downloaded_a_few_at_a_time() {
        let mut gui = demo();
        let attachments: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({
                    "id": i.to_string(), "filename": format!("foto{i}.png"), "size": 1000,
                    "url": format!("https://cdn.discordapp.com/attachments/1/{i}/foto.png"),
                    "proxy_url": format!("https://media.discordapp.net/attachments/1/{i}/foto.png"),
                    "content_type": "image/png", "width": 64, "height": 64
                })
            })
            .collect();
        let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
        msg.attachments = serde_json::from_value(serde_json::Value::Array(attachments)).unwrap();
        gui.store.insert_message(msg);
        let _ = gui.request_previews();
        assert_eq!(gui.downloads, MAX_DOWNLOADS);
        let loading = gui
            .previews
            .values()
            .filter(|p| matches!(p, Preview::Loading))
            .count();
        assert_eq!(loading, MAX_DOWNLOADS);
        let _ = gui.view();

        // They start in attachment order: the seventh waits.
        let url = "https://media.discordapp.net/attachments/1/0/foto.png".to_string();
        let seventh = "https://media.discordapp.net/attachments/1/6/foto.png".to_string();
        assert!(matches!(gui.previews.get(&url), Some(Preview::Loading)));
        assert!(!gui.previews.contains_key(&seventh));

        // A failed download starts another one and leaves the file name.
        let _ = gui.update(Message::PreviewLoaded {
            url: url.clone(),
            result: Err("test".into()),
        });
        assert!(matches!(gui.previews.get(&url), Some(Preview::Failed)));
        assert!(matches!(gui.previews.get(&seventh), Some(Preview::Loading)));
        assert_eq!(gui.downloads, MAX_DOWNLOADS);
        let _ = gui.view();
    }

    #[test]
    fn previews_of_old_messages_too() {
        let mut gui = demo();
        // An image followed by many text-only messages (as after loading the
        // older history by scrolling up).
        let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
        msg.attachments = serde_json::from_value(serde_json::json!([{
            "id": "1", "filename": "vecchia.jpg", "size": 1000,
            "url": "https://cdn.discordapp.com/attachments/1/1/vecchia.jpg",
            "proxy_url": "https://media.discordapp.net/attachments/1/1/vecchia.jpg",
            "content_type": "image/jpeg", "width": 64, "height": 64
        }]))
        .unwrap();
        gui.store.insert_message(msg);
        for i in 0..120 {
            let m = crate::demo::local_message(
                "1001",
                None,
                &format!("text {i}"),
                gui.store.messages.get("1001"),
            );
            gui.store.insert_message(m);
        }
        let _ = gui.request_previews();
        let url = "https://media.discordapp.net/attachments/1/1/vecchia.jpg";
        assert!(matches!(gui.previews.get(url), Some(Preview::Loading)));
    }

    #[test]
    fn full_screen_view() {
        let mut gui = demo();
        let att = gui.store.messages["1001"]
            .iter()
            .flat_map(|m| m.attachments.iter())
            .find(|a| a.url == crate::demo::DEMO_IMAGE_URL)
            .unwrap()
            .clone();
        let _ = gui.update(Message::OpenViewer(Box::new(att)));
        assert!(matches!(
            gui.viewer.as_ref().map(|v| &v.image),
            Some(Preview::Ready(_))
        ));
        let _ = gui.view();
        // Nothing is downloaded in the demo.
        let _ = gui.update(Message::SaveAttachment);
        assert!(matches!(
            gui.viewer.as_ref().map(|v| &v.save),
            Some(SaveState::Idle)
        ));
        let _ = gui.update(Message::Noop);
        assert!(gui.viewer.is_some());
        let _ = gui.update(Message::CloseViewer);
        assert!(gui.viewer.is_none());

        // Real attachment: the download of the large version starts.
        let real: Attachment = serde_json::from_value(serde_json::json!({
            "id": "9", "filename": "foto.jpg", "size": 2048,
            "url": "https://cdn.discordapp.com/attachments/1/9/foto.jpg",
            "proxy_url": "https://media.discordapp.net/attachments/1/9/foto.jpg",
            "content_type": "image/jpeg", "width": 4000, "height": 3000
        }))
        .unwrap();
        let _ = gui.update(Message::OpenViewer(Box::new(real)));
        let v = gui.viewer.as_ref().unwrap();
        assert!(matches!(v.image, Preview::Loading));
        let url = v.url.clone().unwrap();
        assert!(url.ends_with("width=2560&height=1920"), "{url}");
        let _ = gui.view();
        let _ = gui.update(Message::ViewerLoaded {
            url,
            result: Err("test".into()),
        });
        assert!(matches!(gui.viewer.as_ref().unwrap().image, Preview::Failed));
        let _ = gui.update(Message::Saved(Err("disk full".into())));
        assert!(matches!(gui.viewer.as_ref().unwrap().save, SaveState::Failed(_)));
        let _ = gui.view();
    }

    #[test]
    fn gifs_decoded_at_the_screen_size() {
        let (w, h) = media::display_size(1000, 400);
        // At scale 1.19 a bit more than the logical size, never the double.
        let (dw, dh) = decode_size(1000, 400, 1.19);
        assert_eq!((dw, dh), ((w * 1.19).ceil() as u32, (h * 1.19).ceil() as u32));
        assert!(dw < (w * 2.0) as u32);
        // Never more than the original.
        assert_eq!(decode_size(100, 40, 3.0), (100, 40));
        // The window tells its scale; out-of-range values are clamped.
        let mut gui = demo();
        let _ = gui.update(Message::ScaleFactor(1.25));
        assert_eq!(gui.scale, 1.25);
        let _ = gui.update(Message::ScaleFactor(0.0));
        assert_eq!(gui.scale, 1.0);
    }

    #[test]
    fn link_gifs_are_animated() {
        let gui = demo();
        // The demo GIF is generated locally and animated.
        match gui.previews.get(crate::demo::DEMO_GIF_URL) {
            Some(Preview::Animated(a)) => assert!(a.frames_in_memory() > 1),
            _ => panic!("demo GIF not animated"),
        }
        let _ = gui.view();
    }

    #[test]
    fn real_gif_falling_back_to_the_static_image() {
        let mut gui = demo();
        let mut msg = crate::demo::local_message(
            "1001",
            None,
            "https://tenor.com/view/gatto-1",
            gui.store.messages.get("1001"),
        );
        msg.embeds = serde_json::from_value(serde_json::json!([{
            "type": "gifv", "url": "https://tenor.com/view/gatto-1", "provider": {"name": "Tenor"},
            "thumbnail": {"url": "https://media.tenor.com/abcAAAAe/gatto.png",
                "proxy_url": "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png",
                "width": 498, "height": 280},
            "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4", "width": 498, "height": 280}
        }]))
        .unwrap();
        gui.store.insert_message(msg);
        let _ = gui.request_previews();
        let gif = "https://media.tenor.com/abcAAAAd/gatto.gif".to_string();
        let still = "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png"
            .to_string();
        assert!(matches!(gui.previews.get(&gif), Some(Preview::Loading)));
        assert!(!gui.previews.contains_key(&still));
        let _ = gui.view();
        // If the GIF does not arrive, Discord's static image is used.
        let _ = gui.update(Message::PreviewLoaded {
            url: gif.clone(),
            result: Err("404".into()),
        });
        assert!(matches!(gui.previews.get(&still), Some(Preview::Loading)));
        let _ = gui.view();
    }

    #[test]
    fn emoji_picker() {
        let mut gui = demo();
        gui.composer = "hi ".into();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert!(gui.picker.is_some());
        let _ = gui.view();
        let _ = gui.update(Message::EmojiQuery("crab".into()));
        let _ = gui.view();
        let _ = gui.update(Message::PickEmoji("🦀"));
        assert_eq!(gui.composer, "hi 🦀");
        assert!(gui.picker.is_none());
        assert_eq!(gui.recent_emojis, vec!["🦀"]);
        // Reopening starts from the recently used; the same button closes it.
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert_eq!(gui.picker.as_ref().unwrap().section, picker::Section::Recent);
        let _ = gui.view();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert!(gui.picker.is_none());
    }

    #[test]
    fn gif_picker() {
        let mut gui = demo();
        let before = gui.store.messages["1001"].len();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
        let p = gui.picker.as_ref().unwrap();
        assert!(!p.shown_gifs().is_empty());
        assert!(matches!(
            gui.previews.get(crate::demo::DEMO_GIF_URL),
            Some(Preview::Animated(_))
        ));
        let _ = gui.view();
        // An old search does not overwrite the new one.
        let _ = gui.update(Message::GifQuery("ferr".into()));
        let _ = gui.update(Message::GifQuery("ferris".into()));
        let _ = gui.update(Message::GifSearch(1));
        assert!(!gui.picker.as_ref().unwrap().searching);
        let _ = gui.update(Message::GifSearch(2));
        let url = gui.picker.as_ref().unwrap().shown_gifs()[0].url.clone();
        // Click on a GIF: the link is sent and the GIF shows up in the chat.
        gui.composer = "draft".into();
        let _ = gui.update(Message::PickGif(url.clone()));
        assert!(gui.picker.is_none());
        assert_eq!(gui.composer, "draft");
        let list = &gui.store.messages["1001"];
        assert_eq!(list.len(), before + 1);
        assert_eq!(list.last().unwrap().content, url);
        assert_eq!(list.last().unwrap().embeds.len(), 1);
        let _ = gui.view();
    }

    #[test]
    fn server_emojis_only_in_their_server() {
        let mut gui = demo();
        // The emojis in the channel's messages already have their image.
        let ferris = media::emoji_url("123456789", false);
        assert!(matches!(gui.small_images.get(&ferris), Some(Preview::Ready(_))));
        // In the picker: those of the server, without the animated ones (no
        // Nitro) and without those reserved to a role.
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert_eq!(gui.picker.as_ref().unwrap().section, picker::Section::Server);
        let usable: Vec<_> = gui
            .store
            .usable_emojis("1001")
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(usable, ["ferris", "cargo", "clippy"]);
        assert!(gui
            .small_images
            .contains_key(&media::emoji_url("123456790", false)));
        let _ = gui.view();
        let _ = gui.update(Message::PickServerEmoji("cargo".into()));
        assert_eq!(gui.composer, ":cargo:");
        gui.composer.push_str(" hi :ferris: :ferris_party:");
        let _ = gui.update(Message::Send);
        let last = gui.store.messages["1001"].last().unwrap();
        assert_eq!(
            last.content,
            "<:cargo:123456790> hi <:ferris:123456789> :ferris_party:"
        );
        let _ = gui.view();

        // In a direct message server emojis are not used.
        let _ = gui.update(Message::OpenChannel("5001".into()));
        assert!(gui.store.usable_emojis("5001").is_empty());
        gui.composer = ":ferris:".into();
        let _ = gui.update(Message::Send);
        assert_eq!(gui.store.messages["5001"].last().unwrap().content, ":ferris:");
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert_ne!(gui.picker.as_ref().unwrap().section, picker::Section::Server);
        let _ = gui.view();
    }

    #[test]
    fn voice_chat_in_the_demo() {
        let mut gui = demo();
        // Giulia and Marco are already in the demo voice channel.
        assert_eq!(gui.store.voice_members("1021").len(), 2);
        let _ = gui.update(Message::JoinVoice("1021".into()));
        let v = gui.voice.as_ref().expect("voice not started");
        assert!(v.demo);
        assert_eq!(v.status, voice::Status::Connecting);
        let _ = gui.view();
        // Connection events (in the demo they come from the fake server).
        let _ = gui.update(Message::Voice(VoiceEvent::Connected {
            mode: "aead_aes256_gcm_rtpsize",
        }));
        let _ = gui.update(Message::Voice(VoiceEvent::Speaking {
            user_id: "901".into(),
            speaking: true,
        }));
        assert_eq!(gui.store.voice_members("1021").len(), 3);
        assert!(gui.voice.as_ref().unwrap().speaking.contains("901"));
        let _ = gui.view();
        let _ = gui.update(Message::LeaveVoice);
        assert!(gui.voice.is_none());
        assert_eq!(gui.store.voice_members("1021").len(), 2);

        // A final error stays visible until leaving.
        let _ = gui.update(Message::JoinVoice("1021".into()));
        let _ = gui.update(Message::Voice(VoiceEvent::Closed(Some("test".into()))));
        assert_eq!(
            gui.voice.as_ref().unwrap().status,
            voice::Status::Failed("test".into())
        );
        let _ = gui.view();
        // In the same channel again: try again.
        let _ = gui.update(Message::JoinVoice("1021".into()));
        assert_eq!(gui.voice.as_ref().unwrap().status, voice::Status::Connecting);
    }

    #[test]
    fn switching_voice_channel_survives_the_late_leave_update() {
        let mut gui = demo();
        gui.demo = false;
        let (link, mut commands) = crate::discord::gateway::commands();
        gui.gateway_link = Some(link);
        gui.store.channel_guild.insert("1022".into(), "100".into());
        let me = gui.store.me_id().unwrap().to_string();
        let state = |channel: Option<&str>| {
            let v: VoiceState = serde_json::from_value(serde_json::json!({
                "guild_id": "100", "channel_id": channel, "user_id": me, "session_id": "s1"
            }))
            .unwrap();
            Message::Gateway(GatewayEvent::VoiceState(v))
        };

        let _ = gui.update(Message::JoinVoice("1021".into()));
        let _ = gui.update(state(Some("1021")));
        assert!(gui.voice.as_ref().unwrap().confirmed);
        // Switch: only "join 1022" goes to Discord, no "leave".
        while commands.try_recv().is_ok() {}
        let _ = gui.update(Message::JoinVoice("1022".into()));
        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert_eq!(sent.len(), 1);
        assert!(format!("{:?}", sent[0]).contains("1022"));
        // A late "left" for the old channel does not close the new connection.
        let _ = gui.update(state(None));
        assert_eq!(gui.voice.as_ref().unwrap().channel_id, "1022");
        let _ = gui.update(state(Some("1022")));
        assert_eq!(gui.voice.as_ref().unwrap().session_id.as_deref(), Some("s1"));
        // Once confirmed, leaving for real closes it.
        let _ = gui.update(state(None));
        assert!(gui.voice.is_none());

        // Asking again for the channel Discord already has us in: the
        // session id is reused, since Discord does not send it again.
        let _ = gui.update(state(Some("1021")));
        let _ = gui.update(Message::JoinVoice("1021".into()));
        assert_eq!(gui.voice.as_ref().unwrap().session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn avatars_icons_and_profiles_in_the_demo() {
        let mut gui = demo();
        // Server icon and avatars are made locally in the demo, no downloads.
        let icon =
            media::guild_icon_url("100", gui.store.guild("100").and_then(|g| g.icon()), ICON_PX).unwrap();
        assert!(matches!(gui.small_images.get(&icon), Some(Preview::Ready(_))));
        let giulia = gui.store.avatar_url(Some("100"), "901", AVATAR_PX).unwrap();
        assert!(matches!(gui.small_images.get(&giulia), Some(Preview::Ready(_))));
        assert_eq!(gui.small_downloads, 0);
        // Server nickname instead of the account name.
        let marco = gui.store.users["902"].clone();
        assert_eq!(gui.store.name_in(Some("100"), &marco), "Marco (ops)");

        // Click on Giulia in the server: the server profile with her roles.
        let _ = gui.update(Message::OpenProfile {
            user_id: "901".into(),
            guild_id: Some("100".into()),
        });
        let p = gui.profile.as_ref().expect("profile not open");
        assert_eq!(p.tab, profile::Tab::Server);
        let profile::Data::Ready(data) = &p.data else {
            panic!("demo profile not ready")
        };
        assert_eq!(data.guild_member.as_ref().unwrap().roles, vec!["5", "7"]);
        let _ = gui.view();
        let _ = gui.update(Message::ProfileTab(profile::Tab::Account));
        let _ = gui.view();
        let _ = gui.update(Message::CloseProfile);
        assert!(gui.profile.is_none());

        // Unknown user: an error in the card, not a crash.
        let _ = gui.update(Message::OpenProfile {
            user_id: "123".into(),
            guild_id: None,
        });
        assert!(matches!(
            gui.profile.as_ref().unwrap().data,
            profile::Data::Failed(_)
        ));
        let _ = gui.view();
        let _ = gui.update(Message::CloseProfile);

        // Direct messages: Giulia's avatar next to her conversation; the
        // group without a picture shows the initials.
        let _ = gui.update(Message::SelectGuild(None));
        let dm = gui.store.dms.iter().find(|c| c.id == "5001").unwrap().clone();
        let (colour, url) = gui.store.dm_picture(&dm, AVATAR_PX);
        assert_eq!(colour, "901");
        assert!(matches!(
            gui.small_images.get(&url.unwrap()),
            Some(Preview::Ready(_))
        ));
        let group = gui.store.dms.iter().find(|c| c.id == "5002").unwrap().clone();
        assert_eq!(
            gui.store.dm_picture(&group, AVATAR_PX),
            ("5002".to_string(), None)
        );
        let _ = gui.view();
    }

    #[test]
    fn microphone_starts_off_and_follows_the_connection() {
        let mut gui = demo();
        let _ = gui.update(Message::JoinVoice("1021".into()));
        let _ = gui.update(Message::Voice(VoiceEvent::Connected {
            mode: "aead_aes256_gcm_rtpsize",
        }));
        let v = gui.voice.as_ref().unwrap();
        assert!(!v.mic_on && !v.deafened);
        assert_eq!(v.encryption, voice_net::Encryption::Pending);
        let _ = gui.update(Message::Voice(VoiceEvent::Encryption(
            voice_net::Encryption::Active {
                privacy_code: "123451234512345123451234512345".into(),
            },
        )));
        // The state shown is the one confirmed by the connection.
        let _ = gui.update(Message::Voice(VoiceEvent::Microphone(true)));
        assert!(gui.voice.as_ref().unwrap().mic_on);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleDeafen);
        assert!(gui.voice.as_ref().unwrap().deafened);
        let _ = gui.update(Message::Voice(VoiceEvent::Microphone(false)));
        assert!(!gui.voice.as_ref().unwrap().mic_on);
        let _ = gui.update(Message::Voice(VoiceEvent::Closed(Some("test".into()))));
        assert!(!gui.voice.as_ref().unwrap().mic_on);
    }

    #[test]
    fn preview_cache_is_bounded() {
        let mut gui = demo();
        for i in 0..(MAX_PREVIEWS + 20) {
            gui.remember_preview(format!("u{i}"), Preview::Failed);
        }
        assert_eq!(gui.previews.len(), MAX_PREVIEWS);
        assert!(!gui.previews.contains_key("u0"));
        assert!(gui.previews.contains_key(&format!("u{}", MAX_PREVIEWS + 19)));
    }

    #[test]
    fn readable_sizes() {
        assert_eq!(human_size(245_760), "240 KB");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(0), "");
    }
}

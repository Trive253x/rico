//! Graphical interface (iced), with the layout of the "iced in gpui style" mock-up.
//!
//! Elm architecture: the state is [`Gui`], every event (click, key, network
//! answer, gateway event) is a [`Message`] applied by `update`, and `view`
//! rebuilds the interface from the state. Gateway and QR login run as
//! subscriptions: iced starts and stops them by itself when the state changes.

mod anim;
mod compose;
mod folders;
mod golive;
mod icons;
mod jump;
mod members;
mod picker;
mod presence;
mod profile;
mod role;
mod settings;
mod stream;
mod style;
mod voice;
mod watch;

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate};
use futures_util::{SinkExt, Stream};
use iced::advanced::subscription;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, mouse_area, opaque, qr_code, rich_text, row, scrollable, space, span, stack,
    text, text_input, tooltip, Column, Row, Space,
};
use iced::{event, keyboard, mouse, window, ContentFit, Element, Length, Padding, Size, Subscription, Task};

use crate::audio::AudioSettings;
use crate::discord::gateway::{self, GatewayEvent, GatewayHandle};
use crate::discord::models::{
    kind, Attachment, Emoji, GifResult, Message as DMessage, MessageDelete, MessagePreview, User,
    UserProfile, VoiceState,
};
use crate::discord::password_login::{self, Answer, TwoFactor};
use crate::discord::remote_auth::{self, AuthEvent};
use crate::discord::rest::{self, ModAction, Rest, Unauthorized};
use crate::discord::voice::{self as voice_net, VoiceEvent};
use crate::i18n::{self, t};
use crate::settings::{Settings, ThemeChoice};
use crate::store::{first_selectable, ChannelRow, Store};
use crate::text::{initials, segments, Ctx, Segment, SegmentKind, Target};
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
/// Avatars of the member list asked at most (its first rows).
const MEMBER_LIST_PICTURES: usize = 120;
const ICON_PX: u32 = 96;
const MAX_SMALL_DOWNLOADS: usize = 8;
/// Pictures of private conversations downloaded for their list.
const MAX_DM_PICTURES: usize = 100;
/// Older pages loaded at most to reach the message a reply answers
/// (50 messages each).
const JUMP_PAGES: u8 = 10;
/// How long the message reached stays highlighted.
const HIGHLIGHT: Duration = Duration::from_millis(1800);

/// A message being looked for in the history of a channel.
#[derive(Debug, Clone)]
struct Jump {
    channel_id: String,
    message_id: String,
    pages_left: u8,
}
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
    iced::application(move || Gui::new(opts), Gui::update, Gui::view)
        .title(Gui::title)
        .subscription(Gui::subscription)
        .theme(|_: &Gui| style::theme())
        .scale_factor(|g: &Gui| g.settings.zoom)
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
        .run()
}

/// The operating system uses a dark theme (light only if it says so).
/// Puts the window in `mode`.
fn window_mode(mode: window::Mode) -> Task<Message> {
    window::latest().and_then(move |id| window::set_mode(id, mode))
}

fn system_is_dark() -> bool {
    // Unknown (or an error reading it): dark, Rico's default.
    !matches!(dark_light::detect(), Ok(dark_light::Mode::Light))
}

/// Asks the system theme off the interface thread (on Linux it is a D-Bus
/// call).
fn check_system_theme() -> Task<Message> {
    Task::perform(
        async { tokio::task::spawn_blocking(system_is_dark).await.unwrap_or(true) },
        Message::SystemTheme,
    )
}

/// Runs a file write on a blocking thread; on failure the status bar says
/// `failed(error)`.
fn write_in_background(
    write: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    failed: fn(String) -> String,
) -> Task<Message> {
    Task::perform(
        async move {
            match tokio::task::spawn_blocking(write).await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(failed(format!("{e:#}"))),
                Err(e) => Some(failed(e.to_string())),
            }
        },
        Message::SaveFailed,
    )
}

fn messages_id() -> iced::widget::Id {
    iced::widget::Id::new("messages")
}

fn composer_id() -> iced::widget::Id {
    iced::widget::Id::new("composer")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTab {
    Qr,
    Password,
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
    email: String,
    /// Kept only until Discord answers, never saved.
    password: String,
    /// Discord asked for the two-factor code.
    two_factor: Option<TwoFactor>,
    code: String,
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
            email: String::new(),
            password: String::new(),
            two_factor: None,
            code: String::new(),
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
    EmailInput(String),
    PasswordInput(String),
    SubmitPassword,
    CodeInput(String),
    SubmitCode,
    CancelTwoFactor,
    PasswordAnswer(Result<Answer, String>),
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
        out: Box<rest::Outgoing>,
        result: Result<Box<DMessage>, String>,
    },
    /// "Reply" on a message.
    StartReply(String),
    CancelReply,
    /// Notify (or not) the author of the message being replied to.
    TogglePingReply,
    /// "Edit" on one of our messages: its text goes in the text field.
    StartEdit(String),
    CancelEdit,
    Edited(Result<Box<DMessage>, String>),
    /// "Delete" on one of our messages: asks for a confirmation.
    AskDelete(String),
    CancelDelete,
    ConfirmDelete,
    Deleted {
        channel_id: String,
        message_id: String,
        result: Result<(), String>,
    },
    /// Esc in the chat: cancels the delete, the edit or the reply.
    ComposerEscape,
    /// A click on a reaction: ours is added (`add`) or removed.
    React {
        message_id: String,
        emoji: Emoji,
        add: bool,
    },
    Reacted(Result<(), String>),
    /// "React": the emoji picker adds a reaction to this message.
    OpenReactionPicker(String),
    /// A file dropped on the window: read and attached to the next message.
    FileDropped(PathBuf),
    /// The attach button: the system's file dialog.
    PickFiles,
    /// Files chosen in the dialog (none if it was closed).
    FilesPicked(Vec<PathBuf>),
    /// Our status menu (from our name at the bottom left).
    OpenStatusMenu,
    CloseStatusMenu,
    SetPresence(crate::discord::folders::Presence),
    StatusEdit(presence::Edit),
    SaveCustomStatus,
    ClearCustomStatus,
    /// Answer to saving the status: the settings after it.
    StatusSaved(Result<(String, bool), String>),
    FileRead(Result<compose::PendingFile, String>),
    RemoveFile(usize),
    ClearFiles,
    Scrolled(scrollable::Viewport),
    /// A click on the line of a reply: go to the message it answers.
    JumpToMessage(String),
    /// Where that message is in the list (offset from the bottom).
    JumpOffset(f32),
    /// The highlight of the message reached ends.
    EndHighlight(String),
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
    /// From the GIF picker: the settings, where its previews are turned on.
    OpenGifSettings,
    Setting(settings::Change),
    /// Writes the settings (after a slider is released).
    SaveSettings,
    RefreshAudioDevices,
    /// Microphones and speakers, listed off the interface thread.
    AudioDevices {
        inputs: Vec<String>,
        outputs: Vec<String>,
    },
    /// A settings file could not be written: the text for the status bar.
    SaveFailed(Option<String>),
    /// The window got the focus: with the "like the system" theme, the
    /// system theme may have changed meanwhile.
    WindowFocused,
    /// The system theme is dark (`true`) or light.
    SystemTheme(bool),
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
    /// Click on a role mention: the card with the role and its members.
    OpenRole {
        guild_id: String,
        role_id: String,
    },
    CloseRole,
    /// "See all" in the role card: the whole list of its loaded members.
    RoleShowAll,
    CloseProfile,
    /// Shows or hides the member list on the right of server channels.
    ToggleMemberList,
    /// Search field of the compact member list.
    MemberFilter(String),
    /// Folds or unfolds the offline people (member list by activity).
    ToggleOfflineMembers,
    /// The next page of the member list.
    MemberListMore,
    /// Someone's volume in calls (percent), only for us.
    UserVolume {
        user_id: String,
        percent: f32,
    },
    /// Hear (or stop hearing) someone in calls, only for us.
    ToggleUserMute(String),
    /// Writes `@someone` in the message being written.
    MentionUser(String),
    CopyUserId(String),
    /// A moderation action on the user of the open card (asked for a
    /// confirmation first when it removes them or times them out).
    Moderate(ModAction),
    ConfirmModeration,
    CancelModeration,
    Moderated {
        user_id: String,
        guild_id: String,
        action: ModAction,
        result: Result<(), String>,
    },
    /// Click on a LIVE badge: preview of that stream.
    OpenStreamPreview(stream::StreamRef),
    StreamPreviewLoaded {
        stream: stream::StreamRef,
        result: Result<Option<String>, String>,
    },
    RefreshStreamPreview,
    CloseStreamPreview,
    /// The mouse went over someone streaming: their small preview.
    HoverStream(stream::StreamRef),
    /// "Watch" in the stream card: joins the voice channel if needed, then watches.
    WatchStream,
    /// Another stream of the same voice channel, from the people list of the
    /// stream window.
    WatchStreamOf(stream::StreamRef),
    StopWatching,
    /// Esc in the stream window: out of full screen first, then stop.
    WatchEscape,
    /// Stream window in full screen (F11, button, double click) or not.
    ToggleFullscreen,
    /// A click on the video (two quick ones: full screen).
    VideoPressed,
    /// The mouse is over the bottom of the video in full screen: controls
    /// shown.
    WatchControls(bool),
    /// Shows or hides the people of the channel in the stream window.
    ToggleWatchMembers,
    /// Stream volume from the slider, in percent.
    StreamVolume(f32),
    /// Stream volume up or down by 10% steps (arrow keys).
    StreamVolumeStep(i32),
    /// Mutes or unmutes the stream's sound.
    ToggleStreamSound,
    /// Events of the stream connection.
    WatchVoice(VoiceEvent),
    /// Our own stream: opens the window to choose what to stream.
    OpenGoLive,
    /// Screens and windows that can be streamed (listed off the interface thread).
    GoLiveSources(Vec<crate::capture::Source>),
    GoLiveSelect(usize),
    GoLiveQuality(crate::video::encoder::Quality),
    GoLiveSound(bool),
    CloseGoLive,
    /// "Go Live": the capture starts, then the stream on Discord.
    StartGoLive,
    StopGoLive,
    /// Events of our stream's connection.
    GoLiveVoice(VoiceEvent),
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
    /// The message being replied to.
    replying: Option<compose::Reply>,
    /// Our message being edited in the text field.
    editing: Option<compose::Editing>,
    /// Our message waiting for the delete confirmation.
    confirm_delete: Option<String>,
    /// Message the emoji picker adds a reaction to (instead of writing).
    reacting: Option<String>,
    /// Files dropped on the window, sent with the next message.
    pending_files: Vec<compose::PendingFile>,
    /// Files being read (they count against the limit of 10).
    reading_files: usize,
    hovered: Option<String>,
    /// A message being looked for in older pages (to go to it).
    jump: Option<Jump>,
    /// The message just reached, highlighted for a moment.
    highlight: Option<String>,
    /// Member list asked on the gateway: (server, channel, pages).
    member_list_sub: Option<(String, String, usize)>,
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
    /// Open role card (from a role mention).
    role_view: Option<role::RoleView>,
    /// Search in the compact member list (only the loaded members).
    member_filter: String,
    /// Offline people unfolded in the member list by activity.
    offline_open: bool,
    /// Card with the preview of a Go Live stream.
    stream_view: Option<stream::StreamView>,
    /// Small previews shown on hover, by stream key.
    hover_previews: HashMap<String, stream::HoverPreview>,
    /// The Go Live stream being watched.
    watch: Option<watch::WatchUi>,
    watch_attempt: u64,
    /// The window to choose what to stream.
    golive_picker: Option<golive::Picker>,
    /// Our own stream, while it is on.
    golive: Option<golive::GoLiveUi>,
    golive_attempt: u64,
    /// Last stream volume chosen: the next stream starts with it.
    stream_volume: f32,
    /// The window is in full screen (only while watching a stream).
    fullscreen: bool,
    /// People of the channel shown next to the stream.
    watch_members: bool,
    settings: Settings,
    /// Server folders open in the column.
    open_folders: std::collections::HashSet<i64>,
    /// The folder editor, when open.
    folder_editor: Option<folders::Editor>,
    /// Our status menu, when open.
    status_menu: Option<presence::Menu>,
    /// The presence last sent on the gateway (op 3), not to send it again.
    sent_presence: Option<SentPresence>,
    /// The file dialog is open (only one at a time).
    picking_files: bool,
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
            replying: None,
            editing: None,
            confirm_delete: None,
            reacting: None,
            pending_files: Vec::new(),
            reading_files: 0,
            hovered: None,
            jump: None,
            highlight: None,
            member_list_sub: None,
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
            role_view: None,
            member_filter: String::new(),
            offline_open: false,
            stream_view: None,
            hover_previews: HashMap::new(),
            watch: None,
            watch_attempt: 0,
            golive_picker: None,
            golive: None,
            golive_attempt: 0,
            stream_volume: 1.0,
            fullscreen: false,
            watch_members: true,
            settings: Settings::default(),
            open_folders: std::collections::HashSet::new(),
            folder_editor: None,
            status_menu: None,
            sent_presence: None,
            picking_files: false,
            settings_panel: None,
            system_dark: true,
            profile_cache: HashMap::new(),
        }
    }

    pub fn new(opts: Options) -> (Self, Task<Message>) {
        let mut gui = Self::blank();
        if opts.demo_login {
            gui.demo = true;
            if let Ok(data) = qr_code::Data::new("https://discord.com/ra/demo-rico-0000000000000000") {
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
        // Before the first frame, so the window does not open in the wrong
        // colours (one quick call at startup).
        if gui.settings.theme == ThemeChoice::System {
            gui.system_dark = system_is_dark();
        }
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
            self.store.set_folders(e.draft.clone());
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
    fn apply_look(&self) {
        style::apply(self.settings.theme, self.system_dark);
        anim::set_autoplay(self.settings.autoplay);
    }

    /// Writes the settings off the interface thread.
    fn save_settings(&self) -> Task<Message> {
        // The demo never writes settings (it also runs in the tests).
        if self.demo {
            return Task::none();
        }
        write_in_background(config::settings_writer(self.settings.clone()), |e| {
            tf!("Settings not saved: {e}", "Impostazioni non salvate: {e}")
        })
    }

    fn title(&self) -> String {
        match &self.store.current {
            Some(c) if self.screen == Screen::Main => format!("{} — Rico", self.store.title(c)),
            _ => "Rico".to_string(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            window::open_events().map(Message::WindowSeen),
            window::resize_events().map(|(id, _)| Message::WindowSeen(id)),
        ];
        if !self.demo {
            match &self.token {
                Some(token) => subs.push(keyed(("gateway", self.session), gateway_stream(token.clone()))),
                None if self.screen == Screen::Login => {
                    subs.push(keyed(("qr", self.login.attempt), qr_stream()));
                }
                None => {}
            }
        }
        // Stream window: Esc leaves full screen or stops watching, F11 full
        // screen, the arrows change the volume.
        if self.watch.is_some() && self.settings_panel.is_none() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(named),
                    ..
                }) => match named {
                    keyboard::key::Named::Escape => Some(Message::WatchEscape),
                    keyboard::key::Named::F11 => Some(Message::ToggleFullscreen),
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
        if self.golive_picker.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseGoLive),
                _ => None,
            }));
        }
        // Our stream's connection: to the local test server in the demo.
        if let Some(g) = &self.golive {
            if g.demo {
                let me = self.store.me_id().unwrap_or("0").to_string();
                subs.push(keyed(
                    ("golive-demo", g.attempt),
                    golive_demo_stream(g.guild_id.clone(), me, g.feed()),
                ));
            } else if let Some(server) = g.server_info(
                self.store.me_id().unwrap_or_default(),
                self.voice_session.as_deref(),
            ) {
                subs.push(keyed(
                    ("golive", g.attempt, server.endpoint.clone()),
                    golive_stream(server, g.feed(), self.settings.audio.clone()),
                ));
            }
        }
        // Ctrl+, opens the settings; Esc closes them (and the status menu).
        if self.status_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseStatusMenu),
                _ => None,
            }));
        } else if self.settings_panel.is_some() {
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
        // "Like the system": the system theme is looked at again when the
        // window gets the focus (it is changed from outside the app), never
        // on a timer.
        if self.settings.theme == ThemeChoice::System {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocused),
                _ => None,
            }));
        }
        // The stream connection, once Discord said where.
        if let Some(w) = &self.watch {
            let session = self.voice.as_ref().and_then(|v| v.session_id.as_deref());
            if let Some(server) = w.server_info(self.store.me_id().unwrap_or_default(), session) {
                subs.push(keyed(
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
        if self.role_view.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseRole),
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
                subs.push(keyed(
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
                subs.push(keyed(
                    ("voice", v.attempt, server.endpoint.clone()),
                    voice_stream(server, self.settings.audio.clone()),
                ));
            }
        }
        // Esc cancels a delete, an edit or a reply (when nothing is open above
        // the chat).
        let composing = self.confirm_delete.is_some() || self.editing.is_some() || self.replying.is_some();
        if composing
            && self.picker.is_none()
            && self.viewer.is_none()
            && self.profile.is_none()
            && self.watch.is_none()
            && self.settings_panel.is_none()
            && self.stream_view.is_none()
            && self.folder_editor.is_none()
        {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ComposerEscape),
                _ => None,
            }));
        }
        // Files dropped on the window go with the next message (only after
        // the user presses Send).
        if self.screen == Screen::Main && self.store.current.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
                _ => None,
            }));
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
        let task = self.handle(message);
        // Full screen belongs to the stream window: back to a window when it
        // closes, whatever closed it (Esc, hang up, the stream ending…).
        if self.fullscreen && self.watch.is_none() {
            self.fullscreen = false;
            return Task::batch([task, window_mode(window::Mode::Windowed)]);
        }
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        // Frames of GIFs no longer on screen are dropped (at most once a second).
        anim::sweep();
        match message {
            Message::LoginTab(tab) => {
                self.login.tab = tab;
                self.login.error = None;
            }
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
            Message::EmailInput(s) => {
                self.login.email = s;
                self.login.error = None;
            }
            Message::PasswordInput(s) => {
                self.login.password = s;
                self.login.error = None;
            }
            Message::SubmitPassword => return self.submit_password(),
            Message::CodeInput(s) => {
                // Six digits: nothing longer is needed.
                self.login.code = s.chars().take(12).collect();
                self.login.error = None;
            }
            Message::SubmitCode => return self.submit_code(),
            Message::CancelTwoFactor => {
                self.login.two_factor = None;
                self.login.code.clear();
                self.login.error = None;
            }
            Message::PasswordAnswer(result) => return self.on_password_answer(result),
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
                    let previews = self.request_previews();
                    return Task::batch([previews, self.continue_jump(&channel_id)]);
                }
                Err(e) => {
                    self.store.history_failed(&channel_id);
                    self.jump = None;
                    self.status = Some((
                        tf!("History unavailable: {e}", "Cronologia non disponibile: {e}"),
                        true,
                    ));
                }
            },
            Message::ComposerChanged(s) => self.composer = s,
            Message::Send => return self.send(),
            Message::Sent { out, result } => match result {
                Ok(m) => {
                    if self.store.insert_message(*m) {
                        return self.request_previews();
                    }
                }
                Err(e) => {
                    // What was sent comes back in the text field, if nothing
                    // else was written meanwhile. Never sent again by itself:
                    // it may have arrived anyway.
                    if self.composer.is_empty() && self.editing.is_none() {
                        self.composer = out.content;
                        if self.pending_files.is_empty() {
                            self.pending_files = out
                                .files
                                .into_iter()
                                .map(|f| compose::PendingFile {
                                    name: f.filename,
                                    bytes: f.bytes,
                                })
                                .collect();
                        }
                    }
                    self.status = Some((tf!("Sending failed: {e}", "Invio non riuscito: {e}"), true));
                }
            },
            Message::StartReply(id) => return self.start_reply(id),
            Message::CancelReply => {
                self.replying = None;
                return iced::widget::operation::focus(composer_id());
            }
            Message::TogglePingReply => {
                if let Some(r) = self.replying.as_mut() {
                    r.ping = !r.ping;
                }
            }
            Message::StartEdit(id) => return self.start_edit(id),
            Message::CancelEdit => {
                self.cancel_edit();
                return iced::widget::operation::focus(composer_id());
            }
            Message::Edited(result) => match result {
                Ok(m) => self.store.replace_message(*m),
                Err(e) => self.status = Some((tf!("Edit not saved: {e}", "Modifica non salvata: {e}"), true)),
            },
            Message::AskDelete(id) => {
                let ok = self
                    .store
                    .current
                    .as_deref()
                    .is_some_and(|c| self.store.can_delete(c, &id));
                self.confirm_delete = ok.then_some(id);
            }
            Message::CancelDelete => self.confirm_delete = None,
            Message::ConfirmDelete => return self.delete_message(),
            Message::Deleted {
                channel_id,
                message_id,
                result,
            } => match result {
                Ok(()) => self.store.delete_message(&MessageDelete {
                    id: message_id,
                    channel_id,
                }),
                Err(e) => {
                    self.status = Some((
                        tf!("Message not deleted: {e}", "Messaggio non eliminato: {e}"),
                        true,
                    ));
                }
            },
            Message::ComposerEscape => {
                if self.confirm_delete.take().is_none() {
                    if self.editing.is_some() {
                        self.cancel_edit();
                    } else {
                        self.replying = None;
                    }
                }
            }
            Message::React {
                message_id,
                emoji,
                add,
            } => return self.react(message_id, emoji, add),
            Message::Reacted(result) => {
                if let Err(e) = result {
                    self.status = Some((tf!("Reaction not saved: {e}", "Reazione non salvata: {e}"), true));
                }
            }
            Message::OpenReactionPicker(id) => {
                let task = if self.picker.as_ref().is_some_and(|p| p.tab == picker::Tab::Emoji) {
                    Task::none()
                } else {
                    self.close_picker();
                    self.toggle_picker(picker::Tab::Emoji)
                };
                self.reacting = Some(id);
                return task;
            }
            Message::FileDropped(path) => return self.file_dropped(path),
            Message::PickFiles => return self.pick_files(),
            Message::FilesPicked(paths) => {
                self.picking_files = false;
                return Task::batch(
                    paths
                        .into_iter()
                        .map(|p| self.file_dropped(p))
                        .collect::<Vec<_>>(),
                );
            }
            Message::OpenStatusMenu => {
                if self.screen == Screen::Main {
                    self.status_menu = Some(presence::Menu::new(self.store.my_status.as_ref(), now_ms()));
                }
            }
            Message::CloseStatusMenu => {
                if !self.status_menu.as_ref().is_some_and(|m| m.saving) {
                    self.status_menu = None;
                }
            }
            Message::StatusEdit(edit) => {
                if let Some(m) = self.status_menu.as_mut() {
                    m.apply(edit);
                }
            }
            Message::SetPresence(p) => {
                let new = self.store.my_status.clone().unwrap_or_default().with_presence(p);
                return self.save_status(new, true);
            }
            Message::SaveCustomStatus => {
                let Some(m) = self.status_menu.as_ref() else {
                    return Task::none();
                };
                let custom = m.custom(chrono::Local::now());
                let new = self
                    .store
                    .my_status
                    .clone()
                    .unwrap_or_default()
                    .with_custom(custom);
                return self.save_status(new, true);
            }
            Message::ClearCustomStatus => {
                let new = self.store.my_status.clone().unwrap_or_default().with_custom(None);
                return self.save_status(new, true);
            }
            Message::StatusSaved(result) => match result {
                Ok((b64, _)) => {
                    if let Ok(snap) = crate::discord::folders::decode_settings(&b64) {
                        self.store.apply_settings(snap);
                    }
                    self.status_menu = None;
                    self.send_presence();
                }
                Err(err) => {
                    let msg = tf!("Status not saved: {err}", "Stato non salvato: {err}");
                    match self.status_menu.as_mut() {
                        Some(m) => {
                            m.saving = false;
                            m.error = Some(msg);
                        }
                        None => self.status = Some((msg, true)),
                    }
                    // Back to what Discord has.
                    return self.fetch_account_settings();
                }
            },
            Message::FileRead(result) => {
                self.reading_files = self.reading_files.saturating_sub(1);
                match result {
                    Ok(f) => match compose::room_for_one_more(&self.pending_files) {
                        Ok(room) if f.bytes.len() as u64 <= room => self.pending_files.push(f),
                        Ok(_) => {
                            self.status = Some((
                                t("The files are bigger than 10 MB.", "I file superano i 10 MB.").into(),
                                true,
                            ));
                        }
                        Err(e) => self.status = Some((e, true)),
                    },
                    Err(e) => self.status = Some((e, true)),
                }
            }
            Message::RemoveFile(i) => {
                if i < self.pending_files.len() {
                    self.pending_files.remove(i);
                }
            }
            Message::ClearFiles => self.pending_files.clear(),
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
            Message::JumpToMessage(id) => return self.jump_to(id),
            Message::JumpOffset(y) => {
                return iced::widget::operation::scroll_to(
                    messages_id(),
                    scrollable::AbsoluteOffset { x: 0.0, y },
                );
            }
            Message::EndHighlight(id) => {
                if self.highlight.as_deref() == Some(id.as_str()) {
                    self.highlight = None;
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
            Message::OpenProfile { user_id, guild_id } => {
                self.role_view = None;
                return self.open_profile(user_id, guild_id);
            }
            Message::OpenRole { guild_id, role_id } => {
                // Only roles of a server we are in (the mention may be forged).
                if self.store.guild(&guild_id).is_some() && media::valid_id(&role_id) {
                    self.profile = None;
                    self.role_view = Some(role::RoleView::new(&self.store, guild_id, role_id));
                    return self.request_small_images();
                }
            }
            Message::CloseRole => self.role_view = None,
            Message::RoleShowAll => {
                if let Some(v) = self.role_view.as_mut() {
                    v.show_all = true;
                }
                return self.request_small_images();
            }
            // Bounded: names are searched only among the loaded members.
            Message::MemberFilter(s) => self.member_filter = s.chars().take(64).collect(),
            Message::ToggleOfflineMembers => self.offline_open = !self.offline_open,
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
            Message::ToggleMemberList => {
                self.settings.member_list = !self.settings.member_list;
                let save = self.save_settings();
                let sub = self.subscribe_member_list(1);
                return Task::batch([save, sub, self.request_small_images()]);
            }
            Message::MemberListMore => {
                let pages = self.member_list_sub.as_ref().map_or(1, |(_, _, p)| p + 1);
                return self.subscribe_member_list(pages);
            }
            Message::UserVolume { user_id, percent } => {
                self.settings.audio.set_user_volume(&user_id, percent / 100.0);
                self.push_audio();
            }
            Message::ToggleUserMute(user_id) => {
                let muted = self.settings.audio.muted_users.contains(&user_id);
                self.settings.audio.set_user_muted(&user_id, !muted);
                self.push_audio();
                return self.save_settings();
            }
            Message::MentionUser(user_id) => {
                if crate::media::valid_id(&user_id) {
                    if !self.composer.is_empty() && !self.composer.ends_with(' ') {
                        self.composer.push(' ');
                    }
                    let _ = write!(self.composer, "<@{user_id}> ");
                    self.profile = None;
                    return iced::widget::operation::focus(composer_id());
                }
            }
            Message::CopyUserId(user_id) => {
                self.status = Some((t("ID copied.", "ID copiato.").into(), false));
                return iced::clipboard::write(user_id);
            }
            Message::Moderate(action) => return self.moderate(action, false),
            Message::ConfirmModeration => {
                if let Some(action) = self.profile.as_mut().and_then(|p| p.pending.take()) {
                    return self.moderate(action, true);
                }
            }
            Message::CancelModeration => {
                if let Some(p) = &mut self.profile {
                    p.pending = None;
                }
            }
            Message::Moderated {
                user_id,
                guild_id,
                action,
                result,
            } => {
                if result.is_ok() {
                    self.store.after_moderation(&guild_id, &user_id, &action);
                }
                if let Some(p) = self
                    .profile
                    .as_mut()
                    .filter(|p| p.user_id == user_id && p.guild_id.as_deref() == Some(guild_id.as_str()))
                {
                    p.busy = false;
                    p.outcome = Some(match result {
                        Ok(()) => Ok(profile::done_label(&action).to_string()),
                        Err(e) => Err(e),
                    });
                }
            }
            Message::OpenStreamPreview(s) => {
                self.profile = None;
                self.stream_view = Some(stream::StreamView::new(s));
                return self.fetch_stream_preview();
            }
            Message::RefreshStreamPreview => {
                if self
                    .stream_view
                    .as_ref()
                    .is_some_and(stream::StreamView::can_refresh)
                {
                    if let Some(v) = &mut self.stream_view {
                        v.state = stream::PreviewState::Loading;
                        v.asked = std::time::Instant::now();
                    }
                    return self.fetch_stream_preview();
                }
            }
            Message::StreamPreviewLoaded { stream, result } => {
                let state = match result {
                    Ok(Some(url)) => stream::PreviewState::Ready(url),
                    Ok(None) => stream::PreviewState::Unavailable,
                    Err(e) => stream::PreviewState::Failed(e),
                };
                if let Some(h) = media::stream_key(&stream.guild_id, &stream.channel_id, &stream.user_id)
                    .and_then(|k| self.hover_previews.get_mut(&k))
                {
                    h.state = state.clone();
                }
                if let Some(v) = self.stream_view.as_mut().filter(|v| v.stream == stream) {
                    v.state = state;
                }
                return self.request_small_images();
            }
            Message::HoverStream(s) => return self.hover_stream(s),
            Message::CloseStreamPreview => self.stream_view = None,
            Message::WatchStream => {
                if let Some(s) = self.stream_view.as_ref().map(|v| v.stream.clone()) {
                    self.stream_view = None;
                    return self.watch_stream(s);
                }
            }
            Message::WatchStreamOf(s) => {
                if self.watch.as_ref().is_some_and(|w| w.stream == s) {
                    return Task::none();
                }
                return self.watch_stream(s);
            }
            Message::StopWatching => self.stop_watching(),
            Message::WatchEscape => {
                if self.fullscreen {
                    return self.set_fullscreen(false);
                }
                self.stop_watching();
            }
            Message::ToggleFullscreen => {
                if self.watch.is_some() {
                    return self.set_fullscreen(!self.fullscreen);
                }
            }
            Message::VideoPressed => {
                let now = std::time::Instant::now();
                if let Some(w) = &mut self.watch {
                    let double = w
                        .last_click
                        .is_some_and(|t| now.duration_since(t) < watch::DOUBLE_CLICK);
                    w.last_click = (!double).then_some(now);
                    if double {
                        return self.set_fullscreen(!self.fullscreen);
                    }
                }
            }
            Message::WatchControls(shown) => {
                if let Some(w) = &mut self.watch {
                    w.controls_shown = shown;
                }
            }
            Message::ToggleWatchMembers => self.watch_members = !self.watch_members,
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
            Message::OpenGoLive => return self.open_golive(),
            Message::GoLiveSources(list) => {
                if let Some(p) = self.golive_picker.as_mut() {
                    p.set_sources(list);
                }
            }
            Message::GoLiveSelect(i) => {
                if let Some(p) = self.golive_picker.as_mut() {
                    p.selected = Some(i);
                    p.error = None;
                }
            }
            Message::GoLiveQuality(q) => {
                return self.update(Message::Setting(settings::Change::GoLiveQuality(q)));
            }
            Message::GoLiveSound(on) => {
                return self.update(Message::Setting(settings::Change::GoLiveSound(on)));
            }
            Message::CloseGoLive => self.golive_picker = None,
            Message::StartGoLive => self.start_golive(),
            Message::StopGoLive => self.stop_golive(None),
            Message::GoLiveVoice(ev) => self.on_golive_voice(ev),
            Message::SettingsLoaded(result) => {
                match result.map(|b64| crate::discord::folders::decode_settings(&b64)) {
                    Ok(Ok(snap)) => {
                        self.store.apply_settings(snap);
                        self.send_presence();
                    }
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
                    return settings::list_devices();
                }
            }
            Message::CloseSettings => self.settings_panel = None,
            Message::OpenGifSettings => {
                let task = self.update(Message::OpenSettings);
                if let Some(p) = &mut self.settings_panel {
                    p.section = settings::Section::TextImages;
                }
                return task;
            }
            Message::SettingsSection(section) => {
                if let Some(p) = &mut self.settings_panel {
                    p.section = section;
                }
            }
            Message::Setting(change) => {
                let drag = change.is_drag();
                let audio = change.is_audio();
                let images_before = (self.settings.show_images, self.settings.gif_picker_previews);
                let theme_before = self.settings.theme;
                settings::apply(&mut self.settings, change);
                self.apply_look();
                let mut tasks = Vec::new();
                if self.settings.theme == ThemeChoice::System && theme_before != ThemeChoice::System {
                    tasks.push(check_system_theme());
                }
                if audio {
                    self.push_audio();
                }
                if !drag {
                    tasks.push(self.save_settings());
                }
                if (self.settings.show_images, self.settings.gif_picker_previews) != images_before {
                    tasks.push(self.request_previews());
                }
                return Task::batch(tasks);
            }
            Message::SaveSettings => return self.save_settings(),
            Message::SaveFailed(error) => {
                if let Some(e) = error {
                    self.status = Some((e, true));
                }
            }
            Message::RefreshAudioDevices => {
                if self.settings_panel.is_some() {
                    return settings::list_devices();
                }
            }
            Message::AudioDevices { inputs, outputs } => {
                if let Some(p) = &mut self.settings_panel {
                    p.set_devices(inputs, outputs);
                }
            }
            Message::WindowFocused => {
                if self.settings.theme == ThemeChoice::System {
                    return check_system_theme();
                }
            }
            Message::SystemTheme(dark) => {
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
                    return write_in_background(config::language_writer(next), |e| {
                        tf!("Language not saved: {e}", "Lingua non salvata: {e}")
                    });
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
                // GIFs are sent, never used as reactions.
                if tab != picker::Tab::Emoji {
                    self.reacting = None;
                }
                return Task::batch([
                    iced::widget::operation::focus(picker::search_id()),
                    self.load_gifs(),
                    self.request_small_images(),
                ]);
            }
            Message::ClosePicker => {
                self.close_picker();
                return iced::widget::operation::focus(composer_id());
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
                if let Some(message_id) = self.reacting.take() {
                    let emoji = self.store.current.as_deref().and_then(|c| {
                        self.store
                            .usable_emojis(c)
                            .into_iter()
                            .find(|e| e.name == name)
                            .map(|e| Emoji {
                                id: Some(e.id.clone()),
                                name: Some(e.name.clone()),
                                animated: e.animated,
                            })
                    });
                    self.close_picker();
                    return match emoji {
                        Some(emoji) => self.react(message_id, emoji, true),
                        None => Task::none(),
                    };
                }
                let _ = write!(self.composer, ":{name}:");
                self.close_picker();
                return Task::batch([
                    iced::widget::operation::focus(composer_id()),
                    iced::widget::operation::move_cursor_to_end(composer_id()),
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
                self.recent_emojis.retain(|e| *e != emoji);
                self.recent_emojis.insert(0, emoji);
                self.recent_emojis.truncate(picker::MAX_RECENT);
                if let Some(message_id) = self.reacting.take() {
                    self.close_picker();
                    let emoji = Emoji {
                        id: None,
                        name: Some(emoji.to_string()),
                        animated: false,
                    };
                    return self.react(message_id, emoji, true);
                }
                self.composer.push_str(emoji);
                self.close_picker();
                return Task::batch([
                    iced::widget::operation::focus(composer_id()),
                    iced::widget::operation::move_cursor_to_end(composer_id()),
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
                    return Task::perform(pause, move |()| Message::GifSearch(seq));
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
                return Task::batch([self.send_text(url), iced::widget::operation::focus(composer_id())]);
            }
            Message::Tick => {}
            Message::ScaleFactor(f) => {
                tracing::debug!("window scale factor: {f}");
                self.scale = f.clamp(1.0, 4.0);
            }
            // When it opens, and when it moves to a screen with another scale.
            Message::WindowSeen(id) => return window::scale_factor(id).map(Message::ScaleFactor),
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

    fn submit_password(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let email = self.login.email.trim().to_string();
        if email.is_empty() || self.login.password.is_empty() {
            self.login.error = Some(
                t(
                    "Write the email and the password.",
                    "Scrivi l'email e la password.",
                )
                .into(),
            );
            return Task::none();
        }
        let password = self.login.password.clone();
        self.login.busy = true;
        self.login.error = None;
        self.login.notice = None;
        Task::perform(
            async move { password_login::login(&email, &password).await },
            Message::PasswordAnswer,
        )
    }

    fn submit_code(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let Some(challenge) = self.login.two_factor.clone() else {
            return Task::none();
        };
        let code = self.login.code.trim().to_string();
        if code.is_empty() {
            return Task::none();
        }
        self.login.busy = true;
        self.login.error = None;
        Task::perform(
            async move { password_login::two_factor(&challenge, &code).await },
            Message::PasswordAnswer,
        )
    }

    fn on_password_answer(&mut self, result: Result<Answer, String>) -> Task<Message> {
        let login = &mut self.login;
        login.busy = false;
        let answer = match result {
            Ok(a) => a,
            // No answer: the fields stay as they are, to try again.
            Err(e) => {
                login.error = Some(e);
                return Task::none();
            }
        };
        // Whatever the answer, the password is no longer needed.
        login.password.clear();
        login.code.clear();
        match answer {
            Answer::Token(token) => {
                login.two_factor = None;
                login.error = None;
                return self.finish_login(token, None);
            }
            Answer::TwoFactor(m) if m.app => {
                login.two_factor = Some(m);
                login.error = None;
            }
            Answer::TwoFactor(_) => {
                login.error = Some(
                    t(
                        "This account confirms logins by SMS or security key, which Rico does not handle: use the QR code.",
                        "Questo account conferma gli accessi con SMS o chiave di sicurezza, che Rico non gestisce: usa il QR code.",
                    )
                    .into(),
                );
            }
            Answer::NewLocation => {
                // Authorized from the email, the login is done again from the start.
                login.two_factor = None;
                login.error = None;
                login.notice = Some(password_login::new_location_notice().into());
            }
            Answer::Captcha => {
                login.two_factor = None;
                login.error = Some(password_login::captcha_notice().into());
            }
            // A wrong code keeps the ticket: the code can be written again.
            Answer::Refused(reason) => login.error = Some(reason),
        }
        Task::none()
    }

    fn on_gateway(&mut self, ev: GatewayEvent) -> Task<Message> {
        match ev {
            GatewayEvent::Connecting => self.conn = Conn::Connecting,
            GatewayEvent::Disconnected(e) => self.conn = Conn::Error(e),
            GatewayEvent::Ready(r) => {
                self.conn = Conn::Online;
                // A new session: the member list must be asked again, and
                // the status sent again once the settings arrive.
                self.member_list_sub = None;
                self.sent_presence = None;
                let reload = self.store.apply_ready(*r);
                tracing::debug!("permissions: {} channels hidden", self.store.hidden_channels());
                if self.guild.is_some_and(|g| g >= self.store.guilds.len()) {
                    self.guild = None;
                }
                let icons = self.request_small_images();
                let settings = self.fetch_account_settings();
                let list = self.subscribe_member_list(1);
                if let Some(c) = reload {
                    return Task::batch([icons, settings, list, self.fetch_history(c, None)]);
                }
                return Task::batch([icons, settings, list]);
            }
            GatewayEvent::Resumed => self.conn = Conn::Online,
            GatewayEvent::MemberList(u) => {
                self.store.apply_member_list(*u);
                return self.request_small_images();
            }
            GatewayEvent::SettingsProto(u) => {
                match crate::discord::folders::decode_settings(&u.settings.proto) {
                    Ok(snap) => {
                        self.store.apply_settings(snap);
                        // Changed from another device: this session follows.
                        self.send_presence();
                    }
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
            GatewayEvent::StreamCreate(c) if self.golive.as_ref().is_some_and(|g| g.key == c.stream_key) => {
                if let Some(g) = self.golive.as_mut() {
                    g.rtc_server_id = Some(c.rtc_server_id);
                    g.status = golive::Status::Connecting;
                    if let Some(link) = &self.gateway_link {
                        link.pause_stream(g.key.clone(), false);
                    }
                }
            }
            GatewayEvent::StreamServer(u) if self.golive.as_ref().is_some_and(|g| g.key == u.stream_key) => {
                if let (Some(g), Some(endpoint)) = (self.golive.as_mut(), u.endpoint) {
                    g.server = Some((endpoint, u.token));
                    self.golive_attempt += 1;
                    g.attempt = self.golive_attempt;
                    g.status = golive::Status::Connecting;
                }
            }
            GatewayEvent::StreamDelete(d) if self.golive.as_ref().is_some_and(|g| g.key == d.stream_key) => {
                let reason = match d.reason.as_deref() {
                    Some("unauthorized") => t(
                        "Discord did not let you stream in this channel.",
                        "Discord non ti ha permesso di trasmettere in questo canale.",
                    ),
                    _ => t("Your stream has ended.", "La tua diretta è finita."),
                };
                self.stop_golive(Some(reason.into()));
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
            GatewayEvent::MessageDelete(d) => {
                self.forget_message(&d.id);
                self.store.delete_message(&d);
            }
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
        // Our stream belongs to the channel we leave.
        self.stop_golive(None);
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
            v.session_id.clone_from(&self.voice_session);
            v.confirmed = v.session_id.is_some();
        }
        self.voice = Some(v);
    }

    /// Opens a profile card; the data is downloaded only now (or reused
    /// from earlier in this session).
    /// Asks Discord where the preview of the open stream card is.
    /// The small preview of a stream under the mouse: asked if never asked
    /// or older than a minute.
    fn hover_stream(&mut self, s: stream::StreamRef) -> Task<Message> {
        let Some(key) = self.store.stream_key(&s.user_id) else {
            return Task::none();
        };
        if media::stream_key(&s.guild_id, &s.channel_id, &s.user_id).as_ref() != Some(&key)
            || !stream::HoverPreview::stale(self.hover_previews.get(&key))
        {
            return Task::none();
        }
        // Only streams still on, and a bounded number of them.
        let store = &self.store;
        self.hover_previews.retain(|k, _| {
            k.rsplit(':')
                .next()
                .and_then(|user| store.stream_key(user))
                .as_ref()
                == Some(k)
        });
        if self.hover_previews.len() >= stream::MAX_HOVER_PREVIEWS {
            return Task::none();
        }
        let mut preview = stream::HoverPreview {
            state: stream::PreviewState::Loading,
            asked: std::time::Instant::now(),
        };
        if self.demo {
            let url = format!("demo://stream/{}", s.user_id);
            let (w, h) = stream::PREVIEW_PX;
            self.remember_small_image(url.clone(), demo_shaped(w, h, media::Shape::Plain));
            preview.state = stream::PreviewState::Ready(url);
            self.hover_previews.insert(key, preview);
            return Task::none();
        }
        self.hover_previews.insert(key, preview);
        self.stream_preview_task(s)
    }

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
        self.stream_preview_task(s)
    }

    /// Asks Discord for the preview of a stream; the answer is
    /// [`Message::StreamPreviewLoaded`].
    fn stream_preview_task(&self, s: stream::StreamRef) -> Task<Message> {
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
    /// Full screen on or off, for the stream window.
    fn set_fullscreen(&mut self, on: bool) -> Task<Message> {
        if self.fullscreen == on {
            return Task::none();
        }
        self.fullscreen = on;
        if let Some(w) = &mut self.watch {
            w.controls_shown = false;
        }
        window_mode(if on {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        })
    }

    fn watch_stream(&mut self, s: stream::StreamRef) -> Task<Message> {
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
        // Into the channel first: switching channel stops any other stream.
        if self.voice.as_ref().is_none_or(|v| v.channel_id != s.channel_id) {
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
        // The avatars of the people next to the stream.
        self.request_previews()
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
            link.stop_stream(w.key);
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
                );
            }
            VoiceEvent::Closed(Some(reason)) => w.status = watch::Status::Ended(reason),
            VoiceEvent::AudioError(e) => self.status = Some((e, true)),
            _ => {}
        }
    }

    /// "Stream" in the call panel: the window to choose what, with the
    /// list made off the interface thread.
    fn open_golive(&mut self) -> Task<Message> {
        let Some(v) = self
            .voice
            .as_ref()
            .filter(|v| matches!(v.status, voice::Status::Connected(_)))
        else {
            return Task::none();
        };
        if !self.store.can(&v.channel_id, permissions::STREAM) {
            self.status = Some((
                t(
                    "You do not have permission to stream in this channel.",
                    "Non hai il permesso di trasmettere in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        self.golive_picker = Some(golive::Picker::default());
        let demo = self.demo;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || golive::sources(demo))
                    .await
                    .unwrap_or_default()
            },
            Message::GoLiveSources,
        )
    }

    /// "Go Live" in the picker: the capture starts, then the stream.
    fn start_golive(&mut self) {
        let Some(p) = self.golive_picker.as_mut() else {
            return;
        };
        let Some(source) = p.source().cloned() else { return };
        let Some(v) = self
            .voice
            .as_ref()
            .filter(|v| matches!(v.status, voice::Status::Connected(_)))
        else {
            p.error = Some(t("Join a voice channel first.", "Prima entra in un canale vocale.").into());
            return;
        };
        let (guild_id, channel_id) = (v.guild_id.clone(), v.channel_id.clone());
        let Some(me) = self.store.me_id().map(str::to_string) else {
            return;
        };
        let Some(key) = media::stream_key(&guild_id, &channel_id, &me) else {
            return;
        };
        if !self.demo && self.gateway_link.is_none() {
            p.error = Some(
                t(
                    "Not connected to Discord: try again shortly.",
                    "Non connesso a Discord: riprova tra poco.",
                )
                .into(),
            );
            return;
        }
        let preview = std::sync::Arc::new(watch::FrameSlot::default());
        let quality = self.settings.golive_quality;
        let options = crate::capture::Options {
            quality,
            sound: self.settings.golive_sound,
            preview: Some(preview.sink()),
        };
        let running = match crate::capture::start(&source, options) {
            Ok(r) => r,
            Err(e) => {
                p.error = Some(tf!("Could not start: {e:#}", "Impossibile avviare: {e:#}"));
                return;
            }
        };
        self.golive_picker = None;
        self.golive_attempt += 1;
        let sound_error = running.sound_error.clone();
        self.golive = Some(golive::GoLiveUi {
            guild_id: guild_id.clone(),
            channel_id: channel_id.clone(),
            key,
            status: if self.demo {
                golive::Status::Connecting
            } else {
                golive::Status::Starting
            },
            source_name: source.name.clone(),
            quality,
            rtc_server_id: None,
            server: None,
            attempt: self.golive_attempt,
            handle: None,
            stats: voice_net::VoiceStats::default(),
            encryption: voice_net::Encryption::Pending,
            preview,
            demo: self.demo,
            sound_error,
            running,
        });
        if self.demo {
            // Our LIVE badge next to us in the channel, as Discord would show it.
            self.store.streaming.insert(me);
        } else if let Some(link) = &self.gateway_link {
            link.start_stream(guild_id, channel_id);
        }
        tracing::info!("golive: started ({:?}, {})", source.kind, quality.label());
    }

    /// Ends our stream (button, leaving the call, Discord, the window
    /// closed): op 19, connection closed, capture stopped. `reason` is shown.
    fn stop_golive(&mut self, reason: Option<String>) {
        let Some(g) = self.golive.take() else { return };
        if let Some(h) = &g.handle {
            h.leave();
        }
        if g.demo {
            if let Some(me) = self.store.me_id().map(str::to_string) {
                self.store.streaming.remove(&me);
            }
        } else if let Some(link) = &self.gateway_link {
            link.stop_stream(g.key.clone());
        }
        tracing::info!("golive: stopped after {} frames", g.stats.video_sent);
        // Dropping `g` stops the capture and releases the screen and the sound.
        drop(g);
        if let Some(r) = reason {
            self.status = Some((r, true));
        }
    }

    fn on_golive_voice(&mut self, ev: VoiceEvent) {
        let Some(g) = self.golive.as_mut() else { return };
        match ev {
            VoiceEvent::Handle(h) => {
                // Our stream's connection plays nothing.
                h.set_deafened(true);
                g.handle = Some(h);
            }
            VoiceEvent::Connected { .. } => g.status = golive::Status::Connected,
            VoiceEvent::Reconnecting(_) => g.status = golive::Status::Connecting,
            VoiceEvent::Encryption(e) => g.encryption = e,
            VoiceEvent::Stats(s) => {
                g.stats = s;
                if g.running.has_ended() {
                    self.stop_golive(Some(
                        t(
                            "The streamed window was closed: stream ended.",
                            "La finestra trasmessa è stata chiusa: diretta finita.",
                        )
                        .into(),
                    ));
                }
            }
            VoiceEvent::Closed(reason) => {
                let reason = reason.map(|r| tf!("Stream ended: {r}", "Diretta finita: {r}"));
                self.stop_golive(
                    reason.or_else(|| Some(t("Your stream has ended.", "La tua diretta è finita.").into())),
                );
            }
            VoiceEvent::AudioError(e) => self.status = Some((e, true)),
            _ => {}
        }
    }

    fn leave_voice(&mut self) {
        self.stop_golive(None);
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
        let moved = state.channel_id.as_ref().is_some_and(|c| *c != v.channel_id)
            || (state.channel_id.is_none() && v.confirmed);
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
        if moved {
            self.stop_golive(Some(
                t(
                    "You left the voice channel: stream ended.",
                    "Sei uscito dal canale vocale: diretta finita.",
                )
                .into(),
            ));
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

    /// The audio settings to the voice connections (call and stream).
    fn push_audio(&self) {
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

    /// Asks the member list of the open server channel (its first `pages`
    /// pages), if it is shown and not asked already.
    fn subscribe_member_list(&mut self, pages: usize) -> Task<Message> {
        let Some(channel) = self.store.current.clone() else {
            return Task::none();
        };
        let Some(guild) = self.store.channel_guild.get(&channel).cloned() else {
            return Task::none();
        };
        if !self.settings.member_list {
            return Task::none();
        }
        let pages = pages.clamp(1, crate::discord::member_list::MAX_PAGES);
        let want = (guild.clone(), channel.clone(), pages);
        if self.member_list_sub.as_ref() == Some(&want) {
            return Task::none();
        }
        if self.demo {
            self.store.demo_member_list(&guild);
            self.member_list_sub = Some(want);
            return self.request_small_images();
        }
        let Some(link) = &self.gateway_link else {
            return Task::none();
        };
        link.member_list(guild, channel, pages);
        self.member_list_sub = Some(want);
        Task::none()
    }

    /// A moderation action from the user card: the ones that remove
    /// someone or time them out wait for a confirmation first.
    fn moderate(&mut self, action: ModAction, confirmed: bool) -> Task<Message> {
        let Some(p) = self.profile.as_mut() else {
            return Task::none();
        };
        let Some(guild_id) = p.guild_id.clone() else {
            return Task::none();
        };
        let user_id = p.user_id.clone();
        if p.busy {
            return Task::none();
        }
        // Only what the permissions and the role hierarchy allow.
        let allowed = self.store.moderation(&guild_id, &user_id);
        let ok = match &action {
            ModAction::Kick => allowed.kick,
            ModAction::Ban => allowed.ban,
            ModAction::Timeout(_) => allowed.timeout,
            ModAction::Mute(_) => allowed.mute,
            ModAction::Deafen(_) => allowed.deafen,
            ModAction::Move(_) | ModAction::Disconnect => allowed.move_members,
        };
        if !ok {
            return Task::none();
        }
        let needs_confirmation = matches!(
            action,
            ModAction::Kick | ModAction::Ban | ModAction::Timeout(Some(_))
        );
        if needs_confirmation && !confirmed {
            p.pending = Some(action);
            p.outcome = None;
            return Task::none();
        }
        p.pending = None;
        p.outcome = None;
        if self.demo {
            // Nothing is sent in the demo: the effect is shown locally.
            return Task::done(Message::Moderated {
                user_id,
                guild_id,
                action,
                result: Ok(()),
            });
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        p.busy = true;
        Task::perform(
            async move {
                let result = rest
                    .moderate(&guild_id, &user_id, &action)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (user_id, guild_id, action, result)
            },
            |(user_id, guild_id, action, result)| Message::Moderated {
                user_id,
                guild_id,
                action,
                result,
            },
        )
    }

    fn open_channel(&mut self, id: String) -> Task<Message> {
        self.hovered = None;
        self.jump = None;
        self.highlight = None;
        // Reply, edit and files belong to the channel that was open: they
        // must never end up in another one.
        if self.store.current.as_deref() != Some(id.as_str()) {
            self.replying = None;
            if self.editing.is_some() {
                self.cancel_edit();
            }
            self.confirm_delete = None;
            self.reacting = None;
            self.pending_files.clear();
        }
        let needs_history = self.store.open(&id);
        let mut tasks = vec![
            // With the view anchored at the bottom, offset 0 = latest messages.
            iced::widget::operation::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 }),
            iced::widget::operation::focus(composer_id()),
        ];
        if needs_history {
            tasks.push(self.fetch_history(id, None));
        }
        tasks.push(self.request_previews());
        tasks.push(self.subscribe_member_list(1));
        Task::batch(tasks)
    }

    /// Starts downloading the previews still missing among the loaded
    /// messages of the open channel, newest first (at most `MAX_DOWNLOADS` at
    /// a time; the others start as the running ones finish). Includes the
    /// older messages loaded by scrolling up.
    fn request_previews(&mut self) -> Task<Message> {
        // (address, size to decode animated GIFs at). The picker's GIFs come
        // first, if it is open: they are the ones being looked at.
        // Only with their previews turned on: they come from Klipy.
        let mut wanted: Vec<(String, (u32, u32))> = self
            .picker
            .as_ref()
            .filter(|_| self.settings.gif_picker_previews)
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
                move |result| Message::PreviewLoaded { url: key, result },
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
        // The open stream preview, and the small ones shown on hover.
        let (pw, ph) = stream::PREVIEW_PX;
        if let Some(url) = self.stream_view.as_ref().and_then(|v| v.image_url()) {
            wanted.push((url.to_string(), SmallKind::Shaped(media::Shape::Plain, pw, ph)));
        }
        for p in self.hover_previews.values() {
            if let Some(url) = p.image_url() {
                wanted.push((url.to_string(), SmallKind::Shaped(media::Shape::Plain, pw, ph)));
            }
        }
        // The faces in the open role card.
        if let Some(v) = &self.role_view {
            for id in v.shown_people() {
                if let Some(url) = self.store.avatar_url(Some(&v.guild_id), id, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
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
        // The people next to the stream being watched.
        if let Some(w) = &self.watch {
            for (user, _) in self.store.voice_members(&w.stream.channel_id) {
                if let Some(url) = self.store.avatar_url(Some(&w.stream.guild_id), &user, AVATAR_PX) {
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
        // The member list on the right (the first ones: the visible ones).
        if let Some(list) = guild
            .as_ref()
            .filter(|_| self.settings.member_list)
            .and_then(|g| self.store.member_lists.get(g))
        {
            for row in list.known().take(MEMBER_LIST_PICTURES) {
                if let crate::discord::member_list::Row::Member { user_id, .. } = row {
                    if let Some(url) = self.store.avatar_url(Some(&list.guild_id), user_id, AVATAR_PX) {
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
                move |result| Message::SmallImageLoaded { url: key, result },
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
        // The composer's buttons write in the text field.
        self.reacting = None;
        match self.picker.as_mut() {
            Some(p) if p.tab == tab => {
                self.close_picker();
                return iced::widget::operation::focus(composer_id());
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
            iced::widget::operation::focus(picker::search_id()),
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
        self.reacting = None;
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
            move |result| Message::ViewerLoaded { url: key, result },
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
        let url = if v.attachment.url.is_empty() {
            v.attachment.proxy_url.clone()
        } else {
            v.attachment.url.clone()
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

    fn view_viewer<'a>(v: &'a ImageViewer) -> Element<'a, Message> {
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
            row![info, space::horizontal(), save, close]
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
        let hint = footer.unwrap_or_else(|| {
            t(
                "Mouse wheel to zoom, drag to move",
                "Rotellina per lo zoom, trascina per spostare",
            )
        });
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

    /// Goes to a message of the open channel: at once if it is loaded,
    /// otherwise after loading older pages (at most [`JUMP_PAGES`]).
    fn jump_to(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !crate::media::valid_id(&message_id) {
            return Task::none();
        }
        let loaded = self
            .store
            .messages
            .get(&channel_id)
            .is_some_and(|list| list.iter().any(|m| m.id == message_id));
        if loaded {
            self.jump = None;
            return self.show_message(message_id);
        }
        self.jump = Some(Jump {
            channel_id: channel_id.clone(),
            message_id,
            pages_left: JUMP_PAGES,
        });
        self.continue_jump(&channel_id)
    }

    /// After a page of history: the message looked for is there, or one
    /// more page is loaded, or the search ends.
    fn continue_jump(&mut self, channel_id: &str) -> Task<Message> {
        let Some(jump) = self.jump.as_mut().filter(|j| j.channel_id == channel_id) else {
            return Task::none();
        };
        let found = self
            .store
            .messages
            .get(channel_id)
            .is_some_and(|list| list.iter().any(|m| m.id == jump.message_id));
        if found {
            let id = jump.message_id.clone();
            self.jump = None;
            return self.show_message(id);
        }
        // The demo has no older pages to load.
        let before = self.store.older_cursor(channel_id).filter(|_| !self.demo);
        match before {
            Some(before) if jump.pages_left > 0 => {
                jump.pages_left -= 1;
                self.fetch_history(channel_id.to_string(), Some(before))
            }
            _ => {
                self.jump = None;
                self.status = Some((
                    t(
                        "The original message is too far back or no longer exists.",
                        "Il messaggio originale è troppo indietro o non esiste più.",
                    )
                    .to_string(),
                    true,
                ));
                Task::none()
            }
        }
    }

    /// Scrolls to a loaded message and highlights it for a moment.
    fn show_message(&mut self, message_id: String) -> Task<Message> {
        self.highlight = Some(message_id.clone());
        let id = message_id.clone();
        Task::batch([
            jump::offset_of(messages_id(), &message_id).map(Message::JumpOffset),
            Task::perform(tokio::time::sleep(HIGHLIGHT), move |()| Message::EndHighlight(id)),
        ])
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
        if self.editing.is_some() {
            return self.save_edit();
        }
        if !self.store.can_send(&channel_id) {
            return Task::none();
        }
        let content = self.composer.trim().to_string();
        if content.is_empty() && self.pending_files.is_empty() {
            return Task::none();
        }
        if self.reading_files > 0 {
            self.status = Some((
                t(
                    "Wait: the files are still being read.",
                    "Attendi: i file sono ancora in lettura.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        // `:name:` of the channel server's emojis (that server only).
        let content = self.store.prepare_outgoing(&channel_id, &content);
        if content.chars().count() > rest::MAX_MESSAGE_CHARS {
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
        let files = std::mem::take(&mut self.pending_files)
            .into_iter()
            .map(|f| rest::Upload {
                filename: f.name,
                bytes: f.bytes,
            })
            .collect();
        self.composer.clear();
        self.send_out(rest::Outgoing {
            content,
            files,
            ..Default::default()
        })
    }

    /// Sends a text to the open channel (also the link of a GIF chosen in the picker).
    fn send_text(&mut self, content: String) -> Task<Message> {
        self.send_out(rest::Outgoing {
            content,
            ..Default::default()
        })
    }

    /// Sends a message to the open channel, as a reply if one was chosen.
    /// The permissions are checked here, whatever button started it.
    fn send_out(&mut self, mut out: rest::Outgoing) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        // Checked again here: the GIF picker sends through this too.
        if !self.store.can_send(&channel_id) {
            self.status = Some((no_send_permission().into(), true));
            return Task::none();
        }
        if !out.files.is_empty() && !self.store.can_attach(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to send files in this channel.",
                    "Non hai il permesso di inviare file in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if let Some(r) = self.replying.take() {
            if r.channel_id == channel_id && self.store.can_reply(&channel_id) {
                out.reply_to = Some(r.message_id);
                out.ping_reply = r.ping;
            }
        }
        self.status = None;
        let bottom =
            iced::widget::operation::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 });

        if self.demo {
            let mut msg = crate::demo::local_message(
                &channel_id,
                self.store.me.clone(),
                &out.content,
                self.store.messages.get(&channel_id),
            );
            // In the demo the GIFs chosen in the picker become an embed right away.
            if let Some(embed) = crate::demo::gif_embed(&out.content) {
                msg.embeds = vec![embed];
            }
            if let Some(reply) = out.reply_to.as_deref() {
                msg.kind = 19;
                msg.referenced_message = self.store.find_message(&channel_id, reply).map(|r| {
                    Box::new(MessagePreview {
                        id: r.id.clone(),
                        author: Some(r.author.clone()),
                        content: r.content.clone(),
                    })
                });
            }
            // Nothing is uploaded: the files are listed with their names.
            msg.attachments = out
                .files
                .iter()
                .map(|f| Attachment {
                    filename: f.filename.clone(),
                    size: f.bytes.len() as u64,
                    ..Default::default()
                })
                .collect();
            self.store.insert_message(msg);
            return Task::batch([bottom, self.request_previews()]);
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let out = Box::new(out);
        let sent = Task::perform(
            async move {
                let result = client
                    .send_message(&channel_id, &out)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                (out, result)
            },
            |(out, result)| Message::Sent { out, result },
        );
        Task::batch([sent, bottom])
    }

    fn start_reply(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_reply(&channel_id) {
            return Task::none();
        }
        let guild = self.store.channel_guild.get(&channel_id).cloned();
        let Some(m) = self.store.find_message(&channel_id, &message_id) else {
            return Task::none();
        };
        let author = self.store.name_in(guild.as_deref(), &m.author);
        // Like Discord: no notification when replying to ourselves.
        let ping = self.store.me_id() != Some(m.author.id.as_str());
        if self.editing.is_some() {
            self.cancel_edit();
        }
        self.confirm_delete = None;
        self.replying = Some(compose::Reply {
            channel_id,
            message_id,
            author,
            ping,
        });
        iced::widget::operation::focus(composer_id())
    }

    fn start_edit(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_edit(&channel_id, &message_id) {
            return Task::none();
        }
        let Some(content) = self
            .store
            .find_message(&channel_id, &message_id)
            .map(|m| m.content.clone())
        else {
            return Task::none();
        };
        let draft = match self.editing.take() {
            Some(e) => e.draft,
            None => std::mem::take(&mut self.composer),
        };
        self.replying = None;
        self.confirm_delete = None;
        self.composer = content;
        self.editing = Some(compose::Editing {
            channel_id,
            message_id,
            draft,
        });
        Task::batch([
            iced::widget::operation::focus(composer_id()),
            iced::widget::operation::move_cursor_to_end(composer_id()),
        ])
    }

    /// Leaves the edit: the text that was being written comes back.
    fn cancel_edit(&mut self) {
        if let Some(e) = self.editing.take() {
            self.composer = e.draft;
        }
    }

    fn save_edit(&mut self) -> Task<Message> {
        let Some(e) = self.editing.clone() else {
            return Task::none();
        };
        if self.store.current.as_deref() != Some(e.channel_id.as_str())
            || !self.store.can_edit(&e.channel_id, &e.message_id)
        {
            self.cancel_edit();
            return Task::none();
        }
        let content = self.composer.trim().to_string();
        if content.is_empty() {
            // As on Discord, an empty edit is a delete: it asks first.
            self.cancel_edit();
            self.confirm_delete = Some(e.message_id);
            return Task::none();
        }
        let content = self.store.prepare_outgoing(&e.channel_id, &content);
        if content.chars().count() > rest::MAX_MESSAGE_CHARS {
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
        self.cancel_edit();
        let unchanged = self
            .store
            .find_message(&e.channel_id, &e.message_id)
            .is_some_and(|m| m.content == content);
        if unchanged {
            return Task::none();
        }
        if self.demo {
            if let Some(mut m) = self.store.find_message(&e.channel_id, &e.message_id).cloned() {
                m.content = content;
                m.edited_timestamp = Some(chrono::Local::now().to_rfc3339());
                self.store.replace_message(m);
            }
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                client
                    .edit_message(&e.channel_id, &e.message_id, &content)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Edited,
        )
    }

    fn delete_message(&mut self) -> Task<Message> {
        let Some(message_id) = self.confirm_delete.take() else {
            return Task::none();
        };
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_delete(&channel_id, &message_id) {
            return Task::none();
        }
        if self.demo {
            self.forget_message(&message_id);
            self.store.delete_message(&MessageDelete {
                id: message_id,
                channel_id,
            });
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = client
                    .delete_message(&channel_id, &message_id)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (channel_id, message_id, result)
            },
            |(channel_id, message_id, result)| Message::Deleted {
                channel_id,
                message_id,
                result,
            },
        )
    }

    /// A message is gone: the reply, the edit or the confirmation about it too.
    fn forget_message(&mut self, message_id: &str) {
        if self.replying.as_ref().is_some_and(|r| r.message_id == message_id) {
            self.replying = None;
        }
        if self.editing.as_ref().is_some_and(|e| e.message_id == message_id) {
            self.cancel_edit();
        }
        if self.confirm_delete.as_deref() == Some(message_id) {
            self.confirm_delete = None;
        }
        if self.reacting.as_deref() == Some(message_id) {
            self.close_picker();
        }
    }

    fn react(&mut self, message_id: String, emoji: Emoji, add: bool) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        let Some(m) = self.store.find_message(&channel_id, &message_id) else {
            return Task::none();
        };
        // Already in the state asked (a double click, or an event that
        // arrived meanwhile): nothing to do.
        let mine = m.reactions.iter().any(|r| r.me && r.emoji.same(&emoji));
        if mine == add {
            return Task::none();
        }
        if add && !self.store.can_react(&channel_id, &message_id, &emoji) {
            self.status = Some((
                t(
                    "You do not have permission to add reactions here.",
                    "Non hai il permesso di aggiungere reazioni qui.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if self.demo {
            self.store.local_reaction(&channel_id, &message_id, &emoji, add);
            return self.request_small_images();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        // The count changes when the gateway event arrives (for everyone the
        // same way), not before: no reaction counted twice.
        Task::perform(
            async move {
                client
                    .react(&channel_id, &message_id, &emoji, add)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Reacted,
        )
    }

    /// The attach button: the system's file dialog, off the interface thread.
    /// The chosen files go through the same checks as dropped ones.
    fn pick_files(&mut self) -> Task<Message> {
        if self.picking_files || self.screen != Screen::Main || self.editing.is_some() {
            return Task::none();
        }
        self.picking_files = true;
        // Starts from the Documents folder (otherwise Windows may open the
        // program's working folder); then it remembers the last one.
        let start =
            directories::UserDirs::new().and_then(|d| d.document_dir().map(std::path::Path::to_path_buf));
        Task::perform(
            async move {
                let mut dialog = rfd::AsyncFileDialog::new().set_title(t("Attach files", "Allega file"));
                if let Some(dir) = start {
                    dialog = dialog.set_directory(dir);
                }
                dialog
                    .pick_files()
                    .await
                    .map(|files| {
                        files
                            .into_iter()
                            .take(rest::MAX_UPLOAD_FILES)
                            .map(|f| f.path().to_path_buf())
                            .collect()
                    })
                    .unwrap_or_default()
            },
            Message::FilesPicked,
        )
    }

    /// Saves our status in the account settings (field 11 only) and shows it
    /// at once; in the demo only here.
    fn save_status(&mut self, new: crate::discord::folders::StatusSettings, close: bool) -> Task<Message> {
        if self.status_menu.as_ref().is_some_and(|m| m.saving) {
            return Task::none();
        }
        if self.demo {
            self.store.my_status = Some(new);
            if close {
                self.status_menu = None;
            }
            return Task::none();
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        if let Some(m) = self.status_menu.as_mut() {
            m.saving = true;
            m.error = None;
        }
        let body = crate::discord::folders::encode_status_update(&new);
        // Shown right away; if Discord refuses, the settings are read again.
        self.store.my_status = Some(new);
        self.send_presence();
        Task::perform(
            async move {
                rest.update_settings_proto(&body, None)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::StatusSaved,
        )
    }

    /// Sends our status on the gateway (op 3) if it changed since the last
    /// time: after connecting, after a change here or on another device.
    fn send_presence(&mut self) {
        let Some(s) = self.store.my_status.as_ref() else {
            return;
        };
        let Some(link) = self.gateway_link.as_ref() else {
            return;
        };
        let custom = s
            .custom
            .as_ref()
            .filter(|c| c.active(now_ms()))
            .map(|c| (c.text.clone(), c.emoji_name.clone(), c.emoji_id));
        let presence = (s.presence().code(), custom);
        if self.sent_presence.as_ref() == Some(&presence) {
            return;
        }
        link.presence(presence.0, presence.1.clone());
        tracing::info!("status sent: {}", presence.0);
        self.sent_presence = Some(presence);
    }

    fn file_dropped(&mut self, path: PathBuf) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if self.screen != Screen::Main || self.editing.is_some() {
            return Task::none();
        }
        if !self.store.can_attach(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to send files in this channel.",
                    "Non hai il permesso di inviare file in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if self.pending_files.len() + self.reading_files >= rest::MAX_UPLOAD_FILES {
            self.status = Some((
                t(
                    "At most 10 files per message.",
                    "Al massimo 10 file per messaggio.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        let room = match compose::room_for_one_more(&self.pending_files) {
            Ok(room) => room,
            Err(e) => {
                self.status = Some((e, true));
                return Task::none();
            }
        };
        self.reading_files += 1;
        Task::perform(compose::read_file(path, room), Message::FileRead)
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
                self.settings.gif_picker_previews.then_some(&self.previews),
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
                mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                    .on_press(Message::ClosePicker),
                panel
            ]);
        }
        if let Some(v) = self.viewer.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(Self::view_viewer(v));
        }
        if let Some(p) = self.profile.as_ref().filter(|_| self.screen == Screen::Main) {
            let can_mention = self
                .store
                .current
                .as_deref()
                .is_some_and(|c| self.store.can_send(c));
            layers = layers.push(profile::view(
                p,
                &self.store,
                &self.small_images,
                profile::Extras {
                    audio: &self.settings.audio,
                    can_mention,
                },
            ));
        }
        if let Some(v) = self.role_view.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(role::view(v, &self.store, &self.small_images));
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
            layers = layers.push(watch::view(
                w,
                &self.store,
                voice,
                can_speak,
                &self.small_images,
                &self.hover_previews,
                watch::WindowLayout {
                    fullscreen: self.fullscreen,
                    members: self.watch_members,
                },
            ));
        }
        if let Some(e) = self
            .folder_editor
            .as_ref()
            .filter(|_| self.screen == Screen::Main)
        {
            layers = layers.push(folders::view(e, &self.store, &self.small_images, self.demo));
        }
        if let (Some(m), Some(me)) = (
            self.status_menu.as_ref().filter(|_| self.screen == Screen::Main),
            self.store.me.as_ref(),
        ) {
            let status = self.store.my_status.as_ref();
            let has_custom = status
                .and_then(|s| s.custom.as_ref())
                .is_some_and(|c| c.active(now_ms()));
            layers = layers.push(presence::view(
                m,
                status
                    .map(crate::discord::folders::StatusSettings::presence)
                    .unwrap_or_default(),
                has_custom,
                &me.id,
            ));
        }
        if let Some(p) = self
            .golive_picker
            .as_ref()
            .filter(|_| self.screen == Screen::Main)
        {
            layers = layers.push(golive::picker_view(
                p,
                self.settings.golive_quality,
                self.settings.golive_sound,
            ));
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
            .push(self.member_column())
            .height(Length::Fill),
            hline(pal::border()),
            self.status_bar(),
        ]
        .into()
    }

    /// The member list on the right, in a server channel when it is shown.
    fn member_column(&self) -> Option<Element<'_, Message>> {
        if !self.settings.member_list {
            return None;
        }
        let channel = self.store.current.as_ref()?;
        let guild = self.store.channel_guild.get(channel)?;
        let pages = self.member_list_sub.as_ref().map_or(1, |(_, _, p)| *p);
        Some(
            row![
                vline(pal::border()),
                members::view(
                    &self.store,
                    guild,
                    &self.small_images,
                    pages,
                    members::Look {
                        style: self.settings.member_style,
                        filter: &self.member_filter,
                        offline_open: self.offline_open,
                        channel_id: self.store.current.as_deref(),
                    },
                )
            ]
            .into(),
        )
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let mut crumbs = row![text("Rico").font(BOLD)]
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
        container(row![crumbs, space::horizontal(), right].align_y(Vertical::Center))
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
        col = col.push(container(Space::new().width(24).height(1)).style(fill(pal::border())));
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
            match entry {
                crate::store::RailEntry::Guild(i) => col = col.push(guild_button(*i)),
                crate::store::RailEntry::Folder { id, .. } => {
                    col = col.push(folders::rail_folder(
                        &self.store,
                        &self.small_images,
                        entry,
                        self.open_folders.contains(id),
                        self.guild,
                        guild_button,
                    ));
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
                        container(
                            text(name.to_uppercase())
                                .size(11)
                                .font(BOLD)
                                .color(pal::muted())
                                .shaping(Shaping::Advanced),
                        )
                        .padding(Padding {
                            top: 12.0,
                            right: 4.0,
                            bottom: 4.0,
                            left: 6.0,
                        }),
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
                        line = line.push(space::horizontal()).push(dot(pal::accent(), 6.0));
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
                            let live = guild
                                .as_ref()
                                .filter(|_| self.store.streaming.contains(&user_id))
                                .map(|g| stream::StreamRef {
                                    guild_id: g.clone(),
                                    channel_id: id.clone(),
                                    user_id: user_id.clone(),
                                });
                            if let Some(s) = &live {
                                member = member
                                    .push(space::horizontal())
                                    .push(stream::live_badge(Message::OpenStreamPreview(s.clone())));
                            }
                            let profile = Message::OpenProfile {
                                user_id: user_id.clone(),
                                guild_id: guild,
                            };
                            // Highlighted on hover like the channels. A click opens
                            // the profile; on someone streaming it watches them, the
                            // mouse over them shows the small preview (like Discord)
                            // and the right click opens the profile.
                            let row_button = button(member)
                                .width(Length::Fill)
                                .padding([2, 8])
                                .style(list_item(false));
                            let entry: Element<'_, Message> = match live {
                                Some(s) => {
                                    let preview = media::stream_key(&s.guild_id, &s.channel_id, &s.user_id)
                                        .and_then(|k| self.hover_previews.get(&k));
                                    let in_channel = self.voice.as_ref().is_some_and(|v| v.channel_id == id);
                                    let card = stream::hover_card(
                                        &self.store,
                                        &s,
                                        preview,
                                        &self.small_images,
                                        in_channel,
                                    );
                                    iced::widget::tooltip(
                                        mouse_area(row_button.on_press(Message::WatchStreamOf(s.clone())))
                                            .on_enter(Message::HoverStream(s))
                                            .on_right_press(profile),
                                        card,
                                        iced::widget::tooltip::Position::Right,
                                    )
                                    .gap(8)
                                    .into()
                                }
                                None => row_button.on_press(profile).into(),
                            };
                            list = list.push(container(entry).padding(Padding {
                                left: 22.0,
                                ..Padding::ZERO
                            }));
                        }
                    }
                }
            }
        }

        // Our status: the dot on the avatar, and under the name the custom
        // status (or the status) once connected.
        let my_status = self.store.my_status.as_ref();
        let my_presence = my_status
            .map(crate::discord::folders::StatusSettings::presence)
            .unwrap_or_default();
        let my_line: String = match &self.conn {
            Conn::Online => my_status
                .and_then(|s| s.custom.as_ref())
                .filter(|c| c.active(now_ms()))
                .map_or_else(
                    || my_presence.label().to_string(),
                    |c| {
                        let text: String = c.text.chars().take(28).collect();
                        if c.text.chars().count() > 28 {
                            format!("{text}…")
                        } else {
                            text
                        }
                    },
                ),
            Conn::Connecting => t("connecting…", "connessione…").into(),
            Conn::Error(_) => t("disconnected", "disconnesso").into(),
        };
        let user_panel: Element<'_, Message> = match &self.store.me {
            Some(me) => container(
                row![
                    button(
                        row![
                            stack![
                                avatar_with(
                                    me.display_name(),
                                    &me.id,
                                    28.0,
                                    self.store
                                        .avatar_url(None, &me.id, AVATAR_PX)
                                        .and_then(|a| self.small_images.get(&a)),
                                ),
                                container(
                                    container(presence::dot(my_presence, 9.0))
                                        .padding(2)
                                        .style(rounded(pal::panel(), 7.0)),
                                )
                                .width(32)
                                .height(32)
                                .align_x(Horizontal::Right)
                                .align_y(Vertical::Bottom),
                            ],
                            column![
                                text(me.display_name().to_string())
                                    .size(13)
                                    .font(SEMIBOLD)
                                    .shaping(Shaping::Advanced),
                                text(my_line)
                                    .size(11)
                                    .color(pal::muted())
                                    .shaping(Shaping::Advanced)
                                    .wrapping(iced::widget::text::Wrapping::None),
                            ]
                            .spacing(1),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([4, 6])
                    .style(list_item(self.status_menu.is_some()))
                    .on_press(Message::OpenStatusMenu),
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
            None => Space::new().width(Length::Fill).height(52).into(),
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
                let can_stream = self.store.can(&v.channel_id, permissions::STREAM);
                column![
                    voice::panel(v, name, can_speak, can_stream, self.golive.as_ref()),
                    hline(pal::soft())
                ]
                .into()
            }
            None => Space::new().width(0).height(0).into(),
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
                    text(t("Welcome to Rico", "Benvenuto in Rico"))
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
        let mut header_row = row![
            text(self.store.channel_label(&channel_id))
                .font(BOLD)
                .shaping(Shaping::Advanced),
            text(topic)
                .size(13)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        ]
        .spacing(12)
        .align_y(Vertical::Center);
        // In a server: the button that shows or hides its members.
        if self.store.channel_guild.contains_key(&channel_id) {
            let shown = self.settings.member_list;
            header_row = header_row.push(space::horizontal()).push(voice::control_button(
                &icons::PEOPLE,
                style::Control::On,
                if shown {
                    t("Hide the member list", "Nascondi l'elenco dei membri")
                } else {
                    t("Show the member list", "Mostra l'elenco dei membri")
                }
                .into(),
                Some(Message::ToggleMemberList),
                30.0,
            ));
        }
        let header = container(header_row)
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
        let can_send = self.store.can_send(&channel_id);
        let mut bars = Column::new().spacing(4);
        if can_send {
            if self.editing.is_some() {
                bars = bars.push(compose::edit_bar());
            } else if let Some(r) = &self.replying {
                bars = bars.push(compose::reply_bar(r));
            }
            if !self.pending_files.is_empty() {
                bars = bars.push(compose::files_row(&self.pending_files));
            }
        }
        let ready =
            !self.composer.trim().is_empty() || (!self.pending_files.is_empty() && self.editing.is_none());
        let send_label = if self.editing.is_some() {
            t("Save", "Salva")
        } else {
            t("Send", "Invia")
        };
        let can_attach = self.editing.is_none() && self.store.can_attach(&channel_id);
        let composer: Element<'_, Message> = if can_send {
            row![
                tooltip(
                    button(
                        container(icons::icon(&icons::ATTACH, 22.0, pal::muted()))
                            .center_x(26)
                            .center_y(24),
                    )
                    .on_press_maybe((can_attach && !self.picking_files).then_some(Message::PickFiles))
                    .padding([8, 8])
                    .style(style::tab(false)),
                    text(t(
                        "Attach files (or drop them here)",
                        "Allega file (o trascinali qui)"
                    ))
                    .size(12),
                    tooltip::Position::Top,
                )
                .style(style::tooltip),
                text_input(&placeholder, &self.composer)
                    .id(composer_id())
                    .on_input(Message::ComposerChanged)
                    .on_submit(Message::Send)
                    .padding(11)
                    .style(input),
                self.picker_button(&icons::GIF, picker::Tab::Gif, "GIF"),
                self.picker_button(&icons::SMILE, picker::Tab::Emoji, "Emoji"),
                button(text(send_label).font(SEMIBOLD))
                    .on_press_maybe(ready.then_some(Message::Send))
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
            container(column![bars, composer].spacing(4)).padding(Padding {
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

    /// GIF and emoji buttons: line icons like the rest of the app.
    fn picker_button<'a>(
        &self,
        handle: &std::sync::LazyLock<iced::widget::svg::Handle>,
        tab: picker::Tab,
        tip: &'a str,
    ) -> Element<'a, Message> {
        let active = self.picker.as_ref().is_some_and(|p| p.tab == tab);
        // On the accent background of an open picker, the dark of the window.
        let color = if active { pal::bg() } else { pal::muted() };
        tooltip(
            button(
                container(icons::icon(handle, 22.0, color))
                    .center_x(26)
                    .center_y(24),
            )
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
        let ctx = self.store.text_ctx(self.store.current.as_deref());
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
        let channel_id = self.store.current.as_deref().unwrap_or_default();
        // Clicking an existing reaction only needs to see the history.
        let toggle_reactions = self.store.can(
            channel_id,
            permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY,
        );

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

            let mentions_me = ctx.mentions_me(m);
            let content = message_body(
                m,
                &ctx,
                &self.settings,
                &self.previews,
                &self.small_images,
                toggle_reactions,
            );
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
                        row![Space::new().width(COMPACT_TIME + 6.0), reply_preview(r, &ctx)],
                        line
                    ]
                    .spacing(2)
                    .into(),
                    None => line.into(),
                }
            } else if grouped {
                // Same left edge as the text of a message with its author.
                row![Space::new().width(32.0 + 4.0 + AUTHOR_GAP + NAME_PAD), content].into()
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
            let background = if self.highlight.as_deref() == Some(m.id.as_str()) {
                Some(pal::selected())
            } else if mentions_me {
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
                .id(jump::container_id(&m.id))
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
            // The actions only over the message under the mouse (or the one
            // waiting for the delete confirmation): computed for one message.
            let confirming = self.confirm_delete.as_deref() == Some(m.id.as_str());
            let actions = (hovered || confirming)
                .then(|| {
                    let allowed = compose::Allowed {
                        react: self.store.can(
                            channel_id,
                            permissions::VIEW_CHANNEL
                                | permissions::READ_MESSAGE_HISTORY
                                | permissions::ADD_REACTIONS,
                        ),
                        reply: self.store.can_reply(channel_id),
                        edit: self.store.can_edit(channel_id, &m.id),
                        delete: self.store.can_delete(channel_id, &m.id),
                    };
                    compose::actions(&m.id, allowed, confirming)
                })
                .flatten();
            // Always a stack, so the structure does not change on hover.
            let overlay: Element<'a, Message> = match actions {
                Some(a) => container(a)
                    .width(Length::Fill)
                    .align_x(Horizontal::Right)
                    .padding(Padding {
                        top: 0.0,
                        right: 16.0,
                        bottom: 0.0,
                        left: 0.0,
                    })
                    .into(),
                None => Space::new().width(0).height(0).into(),
            };
            col = col.push(
                mouse_area(stack![item, overlay])
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
            button(text(t("Email", "Email")).font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Password))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Password)),
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
                    text(t(
                        "Or sign in with email and password, or with the token.",
                        "Oppure accedi con email e password, o con il token."
                    ))
                    .size(12)
                    .color(pal::muted()),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
            },
            LoginTab::Password => self.view_password_login(),
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
        let mut card = column![text("Rico").size(28).font(BOLD), tabs]
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

    fn view_password_login(&self) -> Element<'_, Message> {
        let login = &self.login;
        let sign_in = button(
            text(if login.busy {
                t("Signing in…", "Accesso in corso…")
            } else {
                t("Sign in", "Accedi")
            })
            .font(SEMIBOLD),
        )
        .padding([8, 18])
        .style(primary);
        let mut col = if login.two_factor.is_some() {
            column![
                text(t(
                    "Write the code of your authenticator app.",
                    "Scrivi il codice della tua app di autenticazione."
                )),
                text_input(t("Code", "Codice"), &login.code)
                    .on_input(Message::CodeInput)
                    .on_submit(Message::SubmitCode)
                    .padding(10)
                    .style(input),
                row![
                    sign_in.on_press_maybe((!login.busy).then_some(Message::SubmitCode)),
                    button(text(t("Back", "Indietro")))
                        .on_press(Message::CancelTwoFactor)
                        .padding([8, 14])
                        .style(ghost),
                ]
                .spacing(10),
            ]
        } else {
            column![
                text(t(
                    "Sign in with your Discord account's email (or phone) and password.",
                    "Accedi con l'email (o il telefono) e la password del tuo account Discord."
                )),
                text_input(t("Email or phone", "Email o telefono"), &login.email)
                    .on_input(Message::EmailInput)
                    .on_submit(Message::SubmitPassword)
                    .padding(10)
                    .style(input),
                text_input("Password", &login.password)
                    .secure(true)
                    .on_input(Message::PasswordInput)
                    .on_submit(Message::SubmitPassword)
                    .padding(10)
                    .style(input),
                sign_in.on_press_maybe((!login.busy).then_some(Message::SubmitPassword)),
                text(t(
                    "The password goes only to Discord and is not saved: Rico keeps only the token, protected by the system.",
                    "La password va solo a Discord e non viene salvata: Rico conserva solo il token, protetto dal sistema."
                ))
                .size(11)
                .color(pal::muted()),
            ]
        }
        .spacing(12)
        .align_x(Horizontal::Center);
        if let Some(e) = &login.error {
            col = col.push(text(e.clone()).color(pal::red()));
        }
        col.into()
    }
}

// ---------------------------------------------------------------- subscriptions

/// Where a connection's stream sends its messages to the interface.
type Output = iced::futures::channel::mpsc::Sender<Message>;

/// A subscription known by `id`: kept running while `id` stays the same,
/// restarted when it changes (iced 0.14 only offers this for data that can
/// be hashed, and the connections carry keys and buffers).
fn keyed<I>(id: I, stream: impl Stream<Item = Message> + Send + 'static) -> Subscription<Message>
where
    I: std::hash::Hash + 'static,
{
    struct Keyed<I, S> {
        id: I,
        stream: S,
    }
    impl<I, S> subscription::Recipe for Keyed<I, S>
    where
        I: std::hash::Hash + 'static,
        S: Stream<Item = Message> + Send + 'static,
    {
        type Output = Message;

        fn hash(&self, state: &mut subscription::Hasher) {
            use std::hash::Hash;
            std::any::TypeId::of::<I>().hash(state);
            self.id.hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: subscription::EventStream,
        ) -> futures_util::stream::BoxStream<'static, Message> {
            Box::pin(self.stream)
        }
    }
    subscription::from_recipe(Keyed { id, stream })
}

/// Gateway as a stream of messages: it stops by itself when iced drops the subscription.
fn gateway_stream(token: String) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
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
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let worker = voice_net::run(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            tx,
        );
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
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let sink = sink.lock().ok().and_then(|mut s| s.take());
        let worker = voice_net::run_with_video(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            sink,
            tx,
        );
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

/// Our stream's connection to Discord.
fn golive_stream(
    server: voice_net::VoiceServer,
    feed: voice_net::GoLiveFeed,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        // No devices: this connection plays nothing and has no microphone.
        let options = voice_net::VoiceOptions {
            devices: false,
            audio,
            golive: Some(feed),
        };
        let worker = voice_net::run(server, options, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::GoLiveVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Demo: our stream goes to a local fake voice server, where a fake viewer
/// end-to-end decrypts it (and asks for a keyframe, as viewers do).
fn golive_demo_stream(
    guild_id: String,
    me: String,
    feed: voice_net::GoLiveFeed,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let config = voice_net::mock::MockConfig {
            users: vec!["901".into(), "902".into()],
            ask_keyframe_after: Some(60),
            ..Default::default()
        };
        let mock = match voice_net::mock::start(config).await {
            Ok(m) => m,
            Err(e) => {
                let _ = output
                    .send(Message::GoLiveVoice(VoiceEvent::Closed(Some(format!(
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
            channel_id: voice_net::mock::CHANNEL_ID.to_string(),
            session_id: "demo".into(),
            user_id: me,
            stream: Some((voice_net::mock::CHANNEL_ID + 1).to_string()),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let options = voice_net::VoiceOptions {
            devices: false,
            audio: AudioSettings::default(),
            golive: Some(feed),
        };
        let worker = voice_net::run(server, options, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::GoLiveVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
        drop(mock);
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
    iced::stream::channel(64, move |mut output: Output| async move {
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
        let worker = voice_net::run(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            tx,
        );
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
    iced::stream::channel(16, |mut output: Output| async move {
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
    container(Space::new().width(Length::Fill).height(1))
        .style(fill(color))
        .into()
}

fn vline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(fill(color))
        .into()
}

/// A status as sent on the gateway: the status and the custom status (text,
/// emoji name, emoji id).
type SentPresence = (&'static str, Option<(String, String, u64)>);

/// Now, in Unix ms (for the custom status timers).
fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

fn dot<'a>(color: iced::Color, size: f32) -> Element<'a, Message> {
    container(Space::new().width(size).height(size))
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

fn rail_button(
    label: String,
    name: String,
    selected: bool,
    unread: bool,
    image: Option<&Preview>,
    on_press: Message,
) -> Element<'_, Message> {
    // Small bar on the left: tall if selected, a dot if there are unread messages.
    let marker_height = if selected {
        22.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    let marker = container(Space::new().width(3).height(marker_height)).style(rounded(pal::text(), 2.0));
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
    row![marker, Space::new().width(7), with_tip, Space::new().width(10)]
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
        .map_or("?", super::discord::models::User::display_name)
        .to_string();
    let full = crate::text::render_content(&r.content, ctx).replace('\n', " ");
    let mut content: String = full.chars().take(90).collect();
    if content.len() < full.len() {
        content.push('…');
    }
    let line = row![
        text("↳").size(12).color(pal::muted()).shaping(Shaping::Advanced),
        text(format!("{who}: {content}"))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
    ]
    .spacing(6);
    // A click goes to the message (when Discord gave its id).
    let jump = (!r.id.is_empty()).then(|| Message::JumpToMessage(r.id.clone()));
    button(line)
        .on_press_maybe(jump)
        .padding([0, 4])
        .style(name_link)
        .into()
}

fn message_body<'a>(
    m: &DMessage,
    ctx: &Ctx,
    prefs: &Settings,
    previews: &'a HashMap<String, Preview>,
    small_images: &'a HashMap<String, Preview>,
    toggle_reactions: bool,
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
            col = col.push(content_with_emojis(segs, edited, small_images, ctx.guild_id));
        } else {
            let mut spans: Vec<_> = segs
                .into_iter()
                .map(|seg| {
                    let link = mention_link(seg.target.as_ref(), ctx.guild_id);
                    styled_span(seg.text, seg.kind, link)
                })
                .collect();
            if edited {
                spans.push(span(t(" (edited)", " (modificato)")).size(11).color(pal::muted()));
            }
            col = col.push(rich_text(spans).size(14).on_link_click(|m| m));
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
            None => Space::new().width(0).height(0).into(),
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
            let toggle = toggle_reactions.then(|| Message::React {
                message_id: m.id.clone(),
                emoji: re.emoji.clone(),
                add: !re.me,
            });
            r.push(
                button(label)
                    .on_press_maybe(toggle)
                    .padding([2, 10])
                    .style(reaction(re.me)),
            )
        });
        col = col.push(chips);
    }
    col.into()
}

/// What a click on a mention opens: the person's profile (with the server
/// part in a server), or the role card (roles only exist in servers).
fn mention_link(target: Option<&Target>, guild_id: Option<&str>) -> Option<Message> {
    match target? {
        Target::User(id) => Some(Message::OpenProfile {
            user_id: id.clone(),
            guild_id: guild_id.map(str::to_string),
        }),
        Target::Role(id) => Some(Message::OpenRole {
            guild_id: guild_id?.to_string(),
            role_id: id.clone(),
        }),
    }
}

/// Style of a piece of message text; mentions can be clicked.
fn styled_span(
    t: String,
    kind: SegmentKind,
    link: Option<Message>,
) -> text::Span<'static, Message, iced::Font> {
    let s = styled_text(t, kind);
    match link {
        Some(m) => s.link(m),
        None => s,
    }
}

fn styled_text(t: String, kind: SegmentKind) -> text::Span<'static, Message, iced::Font> {
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
    guild_id: Option<&str>,
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
                    let link = mention_link(seg.target.as_ref(), guild_id);
                    line = line.push(
                        rich_text(vec![styled_span(word.to_string(), seg.kind, link)])
                            .size(14)
                            .on_link_click(|m| m),
                    );
                }
                for _ in 0..piece.len() - word.len() {
                    line = line.push(Space::new().width(WORD_SPACE));
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
        Some(Preview::Failed) => text(format!(":{name}:"))
            .size(14)
            .color(pal::yellow())
            .shaping(Shaping::Advanced)
            .into(),
        _ => Space::new().width(size).height(size).into(),
    }
}

/// Fake emoji for the demo: a coloured circle (or the demo GIF if animated).
fn demo_emoji(url: &str, is_animated: bool) -> Preview {
    if is_animated {
        return anim::demo_animation(48, 48).map_or(Preview::Failed, Preview::Animated);
    }
    let seed = url
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
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
    let started = std::time::Instant::now();
    let bytes = media::fetch(&url, media::PREVIEW_MAX_BYTES).await?;
    let downloaded = started.elapsed();
    if !media::is_animatable(&bytes) {
        return Ok(Loaded::Image(bytes));
    }
    // Only the first frame now: the others are decoded when the GIF is on
    // screen (see `anim`).
    tokio::task::spawn_blocking(move || {
        let first = std::time::Instant::now();
        let result = Animation::from_source(&bytes, w, h);
        tracing::debug!(
            "animated preview: {} KB downloaded in {} ms, first frame in {} ms",
            bytes.len() / 1024,
            downloaded.as_millis(),
            first.elapsed().as_millis()
        );
        match result {
            Ok(Some(a)) => Ok(Loaded::Animated(a)),
            Ok(None) => Ok(Loaded::Image(bytes)),
            Err(e) => Err(e),
        }
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
    fn member_list_and_user_card_in_the_demo() {
        let mut gui = demo();
        // Our own server in the demo: its members on the right.
        let _ = gui.update(Message::OpenChannel("2001".into()));
        let list = gui.store.member_lists.get("200").expect("member list");
        let ids: Vec<&str> = list
            .known()
            .filter_map(|r| match r {
                crate::discord::member_list::Row::Member { user_id, .. } => Some(user_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(ids.contains(&"901") && ids.contains(&"902") && ids.contains(&"900"));
        let _ = gui.view();
        // Hidden and shown again from the channel header (saved).
        let _ = gui.update(Message::ToggleMemberList);
        assert!(!gui.settings.member_list);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleMemberList);
        assert!(gui.settings.member_list);

        // The user card: volume only for us, kept by ID.
        let open = || Message::OpenProfile {
            user_id: "902".into(),
            guild_id: Some("200".into()),
        };
        let _ = gui.update(open());
        let _ = gui.view();
        let _ = gui.update(Message::UserVolume {
            user_id: "902".into(),
            percent: 150.0,
        });
        assert_eq!(gui.settings.audio.user_gain("902"), 1.5);
        let _ = gui.update(Message::ToggleUserMute("902".into()));
        assert_eq!(gui.settings.audio.user_gain("902"), 0.0);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleUserMute("902".into()));
        assert_eq!(gui.settings.audio.user_gain("902"), 1.5);
        // Mention: into the message being written, the card closes.
        gui.composer = "hi".into();
        let _ = gui.update(Message::MentionUser("902".into()));
        assert_eq!(gui.composer, "hi <@902> ");
        assert!(gui.profile.is_none());

        // Moderation in our server: kick and ban wait for a confirmation.
        let _ = gui.update(open());
        assert!(gui.store.moderation("200", "902").kick);
        let _ = gui.update(Message::Moderate(ModAction::Kick));
        assert_eq!(gui.profile.as_ref().unwrap().pending, Some(ModAction::Kick));
        let _ = gui.view();
        let _ = gui.update(Message::CancelModeration);
        assert!(gui.profile.as_ref().unwrap().pending.is_none());
        let _ = gui.update(Message::Moderate(ModAction::Timeout(Some(600))));
        let _ = gui.update(Message::ConfirmModeration);
        assert!(gui.profile.as_ref().unwrap().pending.is_none());
        // The demo answers at once, nothing is sent.
        let _ = gui.update(Message::Moderated {
            user_id: "902".into(),
            guild_id: "200".into(),
            action: ModAction::Timeout(Some(600)),
            result: Ok(()),
        });
        assert!(matches!(gui.profile.as_ref().unwrap().outcome, Some(Ok(_))));
        assert!(gui
            .store
            .member("200", "902")
            .and_then(|m| m.communication_disabled_until.as_deref())
            .is_some_and(|u| crate::permissions::timed_out(Some(u))));
        let _ = gui.view();

        // Not where we have no permission, never on ourselves or the owner.
        assert!(!gui.store.moderation("100", "902").any());
        assert!(!gui.store.moderation("200", "900").any());
        assert!(!gui.store.moderation("100", "901").any());
        let _ = gui.update(Message::CloseProfile);
        let _ = gui.update(Message::OpenProfile {
            user_id: "902".into(),
            guild_id: Some("100".into()),
        });
        let _ = gui.update(Message::Moderate(ModAction::Ban));
        assert!(
            gui.profile.as_ref().unwrap().pending.is_none(),
            "not allowed: nothing"
        );
        let _ = gui.view();
    }

    #[test]
    fn reply_in_the_demo() {
        let mut gui = demo();
        let _ = gui.update(Message::StartReply("60008".into()));
        let r = gui.replying.as_ref().expect("replying");
        assert_eq!(r.author, "Giulia");
        assert!(r.ping);
        let _ = gui.view();
        let _ = gui.update(Message::TogglePingReply);
        let _ = gui.update(Message::ComposerChanged("Thanks!".into()));
        let _ = gui.update(Message::Send);
        assert!(gui.replying.is_none());
        let last = gui.store.messages["1001"].last().unwrap();
        assert_eq!(last.content, "Thanks!");
        assert_eq!(
            last.referenced_message.as_ref().map(|r| r.id.as_str()),
            Some("60008")
        );
        // Esc cancels a reply; another channel drops it.
        let _ = gui.update(Message::StartReply("60008".into()));
        let _ = gui.update(Message::ComposerEscape);
        assert!(gui.replying.is_none());
        let _ = gui.update(Message::StartReply("60008".into()));
        let _ = gui.update(Message::OpenChannel("1003".into()));
        assert!(gui.replying.is_none());
    }

    #[test]
    fn no_reply_where_we_cannot_write() {
        let mut gui = demo();
        let _ = gui.update(Message::OpenChannel("1004".into()));
        let id = gui
            .store
            .messages
            .get("1004")
            .and_then(|l| l.first())
            .map(|m| m.id.clone());
        if let Some(id) = id {
            let _ = gui.update(Message::StartReply(id));
        }
        assert!(gui.replying.is_none());
    }

    #[test]
    fn edit_and_delete_our_messages_in_the_demo() {
        let mut gui = demo();
        let _ = gui.update(Message::ComposerChanged("draft".into()));
        // Someone else's message: nothing happens.
        let _ = gui.update(Message::StartEdit("60008".into()));
        assert!(gui.editing.is_none());
        let _ = gui.update(Message::AskDelete("60008".into()));
        assert!(gui.confirm_delete.is_none());

        let _ = gui.update(Message::StartEdit("60007".into()));
        assert_eq!(gui.composer, "Next step is rendering images");
        let _ = gui.view();
        let _ = gui.update(Message::ComposerChanged("Next step: images".into()));
        let _ = gui.update(Message::Send);
        assert!(gui.editing.is_none());
        // The draft comes back after the edit.
        assert_eq!(gui.composer, "draft");
        let m = gui.store.find_message("1001", "60007").unwrap();
        assert_eq!(m.content, "Next step: images");
        assert!(m.edited_timestamp.is_some());

        // Esc leaves the edit without changes.
        let _ = gui.update(Message::StartEdit("60007".into()));
        let _ = gui.update(Message::ComposerChanged("nope".into()));
        let _ = gui.update(Message::ComposerEscape);
        assert_eq!(gui.composer, "draft");
        assert_eq!(
            gui.store.find_message("1001", "60007").unwrap().content,
            "Next step: images"
        );

        // Delete only after the confirmation.
        let _ = gui.update(Message::AskDelete("60007".into()));
        let _ = gui.view();
        let _ = gui.update(Message::CancelDelete);
        assert!(gui.store.find_message("1001", "60007").is_some());
        let _ = gui.update(Message::AskDelete("60007".into()));
        let _ = gui.update(Message::ConfirmDelete);
        assert!(gui.store.find_message("1001", "60007").is_none());
    }

    #[test]
    fn reactions_in_the_demo() {
        let mut gui = demo();
        let crab = Emoji {
            id: None,
            name: Some("🦀".into()),
            animated: false,
        };
        let mine = |gui: &Gui, e: &Emoji| {
            gui.store
                .find_message("1001", "60008")
                .and_then(|m| m.reactions.iter().find(|r| r.emoji.same(e)))
                .map(|r| (r.count, r.me))
        };
        let _ = gui.update(Message::React {
            message_id: "60008".into(),
            emoji: crab.clone(),
            add: false,
        });
        assert_eq!(mine(&gui, &crab), Some((1, false)));
        // The same click twice changes nothing more.
        let _ = gui.update(Message::React {
            message_id: "60008".into(),
            emoji: crab.clone(),
            add: false,
        });
        assert_eq!(mine(&gui, &crab), Some((1, false)));

        // From the picker: a reaction, not text in the field.
        let _ = gui.update(Message::OpenReactionPicker("60008".into()));
        assert!(gui.picker.is_some());
        let _ = gui.update(Message::PickEmoji("🎉"));
        assert!(gui.composer.is_empty());
        assert!(gui.picker.is_none());
        let party = Emoji {
            id: None,
            name: Some("🎉".into()),
            animated: false,
        };
        assert_eq!(mine(&gui, &party), Some((1, true)));
        // A server emoji too.
        let _ = gui.update(Message::OpenReactionPicker("60008".into()));
        let _ = gui.update(Message::PickServerEmoji("cargo".into()));
        let cargo = Emoji {
            id: Some("123456790".into()),
            name: Some("cargo".into()),
            animated: false,
        };
        assert_eq!(mine(&gui, &cargo), Some((1, true)));
        // The composer's emoji button writes again.
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        let _ = gui.update(Message::PickEmoji("🦀"));
        assert_eq!(gui.composer, "🦀");
    }

    #[test]
    fn dropped_files_are_limited_and_stay_in_their_channel() {
        let dir = std::env::temp_dir().join(format!("rico-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("note.txt");
        std::fs::write(&small, b"hello").unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let f = rt.block_on(compose::read_file(small.clone(), 1024)).unwrap();
        assert_eq!(f.name, "note.txt");
        assert_eq!(&f.bytes[..], b"hello");
        // Bigger than the room left, a folder, a missing file: refused.
        assert!(rt.block_on(compose::read_file(small.clone(), 4)).is_err());
        assert!(rt.block_on(compose::read_file(dir.clone(), 1024)).is_err());
        assert!(rt
            .block_on(compose::read_file(dir.join("missing"), 1024))
            .is_err());

        let mut gui = demo();
        let _ = gui.update(Message::FileRead(Ok(f.clone())));
        assert_eq!(gui.pending_files.len(), 1);
        let _ = gui.view();
        let _ = gui.update(Message::Send);
        assert!(gui.pending_files.is_empty());
        let last = gui.store.messages["1001"].last().unwrap();
        assert_eq!(last.attachments[0].filename, "note.txt");
        assert_eq!(last.attachments[0].size, 5);

        // At most 10 files.
        for _ in 0..rest::MAX_UPLOAD_FILES {
            let _ = gui.update(Message::FileRead(Ok(f.clone())));
        }
        assert_eq!(gui.pending_files.len(), rest::MAX_UPLOAD_FILES);
        let _ = gui.update(Message::FileDropped(small.clone()));
        assert_eq!(gui.reading_files, 0);
        // Another channel: the files are dropped, never sent there.
        let _ = gui.update(Message::OpenChannel("1003".into()));
        assert!(gui.pending_files.is_empty());
        // Where we cannot write, no file is read.
        let _ = gui.update(Message::OpenChannel("1004".into()));
        let _ = gui.update(Message::FileDropped(small));
        assert_eq!(gui.reading_files, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_member_list_style_draws() {
        use crate::settings::MemberStyle;
        let mut gui = demo();
        // A server channel with its member list.
        let _ = gui.update(Message::OpenChannel("1001".into()));
        assert!(gui.store.member_lists.contains_key("100"));
        for style in MemberStyle::ALL {
            let _ = gui.update(Message::Setting(settings::Change::MemberStyle(style)));
            assert_eq!(gui.settings.member_style, style);
            let _ = gui.view();
        }
        // Compact: the search keeps at most 64 characters.
        let _ = gui.update(Message::MemberFilter("giu".into()));
        let _ = gui.view();
        let _ = gui.update(Message::MemberFilter("x".repeat(500)));
        assert_eq!(gui.member_filter.chars().count(), 64);
        // By activity: offline folded and unfolded.
        let _ = gui.update(Message::Setting(settings::Change::MemberStyle(
            MemberStyle::Activity,
        )));
        let _ = gui.update(Message::ToggleOfflineMembers);
        assert!(gui.offline_open);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleOfflineMembers);
        assert!(!gui.offline_open);
        let _ = gui.update(Message::Setting(settings::Change::MemberStyle(
            MemberStyle::Classic,
        )));
    }

    #[test]
    fn clicking_mentions_opens_the_person_or_the_role() {
        let mut gui = demo();
        // A role mention in #general: the role card with its members.
        let link = mention_link(Some(&Target::Role("7".into())), Some("100"));
        assert!(matches!(&link, Some(Message::OpenRole { role_id, .. }) if role_id == "7"));
        let _ = gui.update(link.unwrap());
        let v = gui.role_view.as_ref().expect("role card");
        assert_eq!(
            v.members.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["901"]
        );
        // Giulia is online in the demo's member list.
        assert_eq!(v.online.len(), 1);
        assert!(!v.show_all);
        let _ = gui.view();
        // "See all" opens the whole list.
        let _ = gui.update(Message::RoleShowAll);
        assert!(gui.role_view.as_ref().is_some_and(|v| v.show_all));
        let _ = gui.view();
        // A name in the card opens the profile and closes the card.
        let _ = gui.update(Message::OpenProfile {
            user_id: "901".into(),
            guild_id: Some("100".into()),
        });
        assert!(gui.role_view.is_none());
        assert!(gui.profile.is_some());
        let _ = gui.update(Message::CloseProfile);

        // A user mention opens that person's profile with the server part.
        let link = mention_link(Some(&Target::User("902".into())), Some("100"));
        assert!(matches!(
            &link,
            Some(Message::OpenProfile { user_id, guild_id: Some(g) }) if user_id == "902" && g == "100"
        ));
        // No role card outside servers, nor for servers we are not in.
        assert!(mention_link(Some(&Target::Role("7".into())), None).is_none());
        let _ = gui.update(Message::OpenRole {
            guild_id: "999".into(),
            role_id: "7".into(),
        });
        assert!(gui.role_view.is_none());
        let _ = gui.update(Message::OpenRole {
            guild_id: "100".into(),
            role_id: "../x".into(),
        });
        assert!(gui.role_view.is_none());
        // Esc (CloseRole) closes it.
        let _ = gui.update(Message::OpenRole {
            guild_id: "100".into(),
            role_id: "6".into(),
        });
        let _ = gui.update(Message::CloseRole);
        assert!(gui.role_view.is_none());
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
        assert_eq!(gui.title(), "#general · Rust Community — Rico");
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
            ..s
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
    fn status_menu_in_the_demo() {
        use crate::discord::folders::Presence;
        let mut gui = demo();
        let _ = gui.update(Message::OpenStatusMenu);
        assert!(gui.status_menu.is_some());
        let _ = gui.view();
        let _ = gui.update(Message::SetPresence(Presence::DoNotDisturb));
        let status_of = |g: &Gui| {
            g.store
                .my_status
                .as_ref()
                .map(crate::discord::folders::StatusSettings::presence)
        };
        assert_eq!(status_of(&gui), Some(Presence::DoNotDisturb));
        let _ = gui.update(Message::OpenStatusMenu);
        let _ = gui.update(Message::StatusEdit(presence::Edit::Text(" At work ".into())));
        let _ = gui.update(Message::SaveCustomStatus);
        assert!(gui.status_menu.is_none());
        let custom = |g: &Gui| g.store.my_status.as_ref().and_then(|s| s.custom.clone());
        assert_eq!(custom(&gui).map(|c| c.text), Some("At work".to_string()));
        // The status stays when the custom status changes.
        assert_eq!(status_of(&gui), Some(Presence::DoNotDisturb));
        let _ = gui.view();
        let _ = gui.update(Message::OpenStatusMenu);
        let _ = gui.update(Message::ClearCustomStatus);
        assert_eq!(custom(&gui), None);
        let _ = gui.update(Message::OpenStatusMenu);
        let _ = gui.update(Message::CloseStatusMenu);
        assert!(gui.status_menu.is_none());
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
    fn hovering_someone_streaming_shows_the_small_preview() {
        let mut gui = demo();
        let s = stream::StreamRef {
            guild_id: "100".into(),
            channel_id: "1021".into(),
            user_id: "901".into(),
        };
        let _ = gui.update(Message::HoverStream(s.clone()));
        let p = gui.hover_previews.get("guild:100:1021:901").expect("asked");
        let url = p.image_url().unwrap().to_string();
        assert!(matches!(gui.small_images.get(&url), Some(Preview::Ready(_))));
        let asked = p.asked;
        let _ = gui.view();
        // Hovering again soon after does not ask again.
        let _ = gui.update(Message::HoverStream(s.clone()));
        assert_eq!(gui.hover_previews.get("guild:100:1021:901").unwrap().asked, asked);
        // Someone not streaming, or a made-up stream: nothing is asked.
        let _ = gui.update(Message::HoverStream(stream::StreamRef {
            user_id: "902".into(),
            ..s.clone()
        }));
        let _ = gui.update(Message::HoverStream(stream::StreamRef {
            channel_id: "1023".into(),
            ..s.clone()
        }));
        assert_eq!(gui.hover_previews.len(), 1);
        // An answer from Discord updates the small preview.
        let _ = gui.update(Message::StreamPreviewLoaded {
            stream: s.clone(),
            result: Ok(None),
        });
        assert!(matches!(
            gui.hover_previews.get("guild:100:1021:901").unwrap().state,
            stream::PreviewState::Unavailable
        ));
        // A click on them in the list watches the stream directly.
        let _ = gui.update(Message::WatchStreamOf(s));
        assert!(gui.watch.is_some());
        let _ = gui.view();
    }

    #[test]
    fn stream_window_people_and_full_screen() {
        let mut gui = demo();
        let s = stream::StreamRef {
            guild_id: "100".into(),
            channel_id: "1021".into(),
            user_id: "901".into(),
        };
        let _ = gui.update(Message::OpenStreamPreview(s.clone()));
        let _ = gui.update(Message::WatchStream);
        assert!(gui.watch.is_some());
        // The people of the channel, with their microphone and sound.
        assert!(gui
            .store
            .voice_status
            .get("902")
            .is_some_and(|f| f.muted() && !f.deafened()));
        assert!(gui.store.voice_status.get("901").is_some_and(|f| !f.muted()));
        assert!(gui.watch_members);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleWatchMembers);
        assert!(!gui.watch_members);
        let _ = gui.view();
        let _ = gui.update(Message::ToggleWatchMembers);
        // The stream already watched: nothing restarts.
        let attempt = gui.watch.as_ref().unwrap().attempt;
        let _ = gui.update(Message::WatchStreamOf(s));
        assert_eq!(gui.watch.as_ref().unwrap().attempt, attempt);
        // Full screen: button or F11, then Esc leaves it before stopping.
        let _ = gui.update(Message::ToggleFullscreen);
        assert!(gui.fullscreen);
        let _ = gui.view();
        let _ = gui.update(Message::WatchControls(true));
        assert!(gui.watch.as_ref().unwrap().controls_shown);
        let _ = gui.view();
        let _ = gui.update(Message::WatchEscape);
        assert!(!gui.fullscreen);
        assert!(gui.watch.is_some());
        // Two quick clicks on the video: full screen; one click: nothing.
        let _ = gui.update(Message::VideoPressed);
        assert!(!gui.fullscreen);
        let _ = gui.update(Message::VideoPressed);
        assert!(gui.fullscreen);
        // Whatever closes the stream also leaves full screen.
        let _ = gui.update(Message::LeaveVoice);
        assert!(gui.watch.is_none());
        assert!(!gui.fullscreen);
        // Without a stream, F11 does nothing.
        let _ = gui.update(Message::ToggleFullscreen);
        assert!(!gui.fullscreen);
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
            "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4",
                "proxy_url": "https://images-ext-1.discordapp.net/external/v/https/media.tenor.com/abcAAAPo/gatto.mp4",
                "width": 498, "height": 280}
        }]))
        .unwrap();
        gui.store.insert_message(msg);
        let _ = gui.request_previews();
        // The clip from Discord's proxy first (never Tenor itself).
        let gif = "https://images-ext-1.discordapp.net/external/v/https/media.tenor.com/abcAAAPo/gatto.mp4"
            .to_string();
        let still = "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png"
            .to_string();
        assert!(matches!(gui.previews.get(&gif), Some(Preview::Loading)));
        assert!(!gui.previews.contains_key(&still));
        let _ = gui.view();
        assert!(!gui
            .previews
            .keys()
            .any(|u| u.starts_with("https://media.tenor.com")));
        // If the clip does not arrive, Discord's static image is used.
        let _ = gui.update(Message::PreviewLoaded {
            url: gif,
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
        // Previews are off until turned on (they come from Klipy): titles only.
        assert!(!gui.settings.gif_picker_previews);
        let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
        let p = gui.picker.as_ref().unwrap();
        assert!(!p.shown_gifs().is_empty());
        let _ = gui.view();
        // The line under the titles opens the settings on "Text & images".
        let _ = gui.update(Message::OpenGifSettings);
        assert!(gui.picker.is_none());
        assert_eq!(
            gui.settings_panel.as_ref().map(|p| p.section),
            Some(settings::Section::TextImages)
        );
        let _ = gui.view();
        let _ = gui.update(Message::Setting(settings::Change::GifPickerPreviews(true)));
        assert!(gui.settings.gif_picker_previews);
        let _ = gui.update(Message::CloseSettings);
        let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
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
    fn email_and_password_login_answers() {
        let mut gui = Gui::blank();
        let _ = gui.update(Message::LoginTab(LoginTab::Password));
        let _ = gui.view();
        // Nothing written: nothing sent.
        let _ = gui.update(Message::SubmitPassword);
        assert!(!gui.login.busy && gui.login.error.is_some());
        let _ = gui.update(Message::EmailInput("me@example.com".into()));
        let _ = gui.update(Message::PasswordInput("secret".into()));
        assert!(gui.login.error.is_none());

        // Two-factor code asked: the password is dropped, the ticket kept.
        let two_factor = || Ok(Answer::TwoFactor(TwoFactor::for_tests(true)));
        let _ = gui.update(Message::PasswordAnswer(two_factor()));
        assert!(gui.login.password.is_empty() && gui.login.two_factor.is_some());
        let _ = gui.view();
        let _ = gui.update(Message::CodeInput("123 456 789 012 345".into()));
        assert_eq!(gui.login.code.chars().count(), 12);
        let _ = gui.update(Message::PasswordAnswer(Ok(Answer::Refused("no".into()))));
        assert!(gui.login.two_factor.is_some() && gui.login.code.is_empty());

        // New location: back to the form, with the email kept, to sign in again.
        let _ = gui.update(Message::PasswordAnswer(Ok(Answer::NewLocation)));
        assert!(gui.login.two_factor.is_none());
        assert_eq!(gui.login.email, "me@example.com");
        assert_eq!(
            gui.login.notice.as_deref(),
            Some(password_login::new_location_notice())
        );
        let _ = gui.view();

        // Only SMS or a security key: not handled, said so.
        let _ = gui.update(Message::PasswordInput("secret".into()));
        let _ = gui.update(Message::PasswordAnswer(Ok(Answer::TwoFactor(
            TwoFactor::for_tests(false),
        ))));
        assert!(gui.login.two_factor.is_none() && gui.login.error.is_some());
        assert!(gui.login.password.is_empty());

        // No answer from Discord: the fields stay, to try again.
        let _ = gui.update(Message::PasswordInput("secret".into()));
        let _ = gui.update(Message::PasswordAnswer(Err("offline".into())));
        assert_eq!(gui.login.password, "secret");
        let _ = gui.update(Message::PasswordAnswer(Ok(Answer::Captcha)));
        assert_eq!(gui.login.error.as_deref(), Some(password_login::captcha_notice()));
        assert_eq!(gui.screen, Screen::Login);
    }

    #[test]
    fn our_stream_in_the_demo() {
        let mut gui = demo();
        // Not in a call: nothing opens.
        let _ = gui.update(Message::OpenGoLive);
        assert!(gui.golive_picker.is_none());
        let _ = gui.update(Message::JoinVoice("1021".into()));
        let _ = gui.update(Message::Voice(VoiceEvent::Connected {
            mode: "aead_aes256_gcm_rtpsize",
        }));
        let _ = gui.view();
        let _ = gui.update(Message::OpenGoLive);
        assert!(gui.golive_picker.is_some());
        let _ = gui.view();
        // The list arrives (here, as the task would give it): the test picture first.
        let _ = gui.update(Message::GoLiveSources(golive::sources(true)));
        let picker = gui.golive_picker.as_ref().unwrap();
        assert_eq!(
            picker.source().unwrap().kind,
            crate::capture::SourceKind::TestPattern
        );
        let _ = gui.update(Message::GoLiveQuality(crate::video::encoder::Quality::Sd30));
        let _ = gui.update(Message::GoLiveSound(false));
        assert!(!gui.settings.golive_sound);
        let _ = gui.view();
        let _ = gui.update(Message::StartGoLive);
        assert!(gui.golive_picker.is_none());
        let g = gui.golive.as_ref().expect("stream not started");
        assert!(g.demo && g.running.sound.is_none());
        assert_eq!(g.quality, crate::video::encoder::Quality::Sd30);
        // Our LIVE badge next to us, and "Live" in the call panel.
        let me = gui.store.me_id().unwrap().to_string();
        assert!(gui.store.streaming.contains(&me));
        let _ = gui.update(Message::GoLiveVoice(VoiceEvent::Connected {
            mode: "aead_aes256_gcm_rtpsize",
        }));
        assert_eq!(gui.golive.as_ref().unwrap().status, golive::Status::Connected);
        let _ = gui.view();
        // Stop: the capture is dropped, the badge goes away.
        let _ = gui.update(Message::StopGoLive);
        assert!(gui.golive.is_none());
        assert!(!gui.store.streaming.contains(&me));
        // Leaving the call also ends a stream.
        let _ = gui.update(Message::OpenGoLive);
        let _ = gui.update(Message::GoLiveSources(golive::sources(true)));
        let _ = gui.update(Message::StartGoLive);
        assert!(gui.golive.is_some());
        let _ = gui.update(Message::LeaveVoice);
        assert!(gui.golive.is_none());
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
        gui.store.upsert_channel(
            serde_json::from_value(serde_json::json!({
                "id": "1022", "type": 2, "name": "Second room", "guild_id": "100", "parent_id": "1020"
            }))
            .unwrap(),
        );
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

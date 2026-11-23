//! Graphical interface (iced), with the layout of the "iced in gpui style" mock-up.
//!
//! Elm architecture: the state is [`Gui`], every event (click, key, network
//! answer, gateway event) is a [`Message`] applied by `update`, and `view`
//! rebuilds the interface from the state. Gateway and QR login run as
//! subscriptions: iced starts and stops them by itself when the state changes.

mod anim;
mod calls;
mod compose;
mod context;
mod folders;
mod friends;
mod golive;
mod icons;
mod images;
mod jump;
mod login;
mod members;
mod messages;
mod notify;
mod people;
mod picker;
mod presence;
mod profile;
mod ptt;
mod role;
mod settings;
mod stream;
mod style;
mod updates;
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
use crate::discord::rest::{self, ModAction, Rest};
use crate::discord::voice::{self as voice_net, VoiceEvent};
use crate::i18n::{self, t};
use crate::session::Next;
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
    /// Right click on a message: its menu, at this point of the window.
    OpenMessageMenu(String, iced::Point),
    CloseMessageMenu,
    /// Right click on a person (author, member list, voice channel): their
    /// menu at the cursor, in the server it was opened in.
    OpenUserMenu {
        user_id: String,
        guild_id: Option<String>,
        at: iced::Point,
    },
    CloseUserMenu,
    UserAction(context::UserAction),
    /// The Friends view (direct messages side).
    OpenFriends,
    Friends(friends::FriendsMsg),
    /// "Message" on a friend: their direct messages.
    MessageUser(String),
    /// Time to check GitHub for a newer Rico (30 s after the start, then daily).
    UpdateCheck,
    /// `DPDFNet` chosen: its model is made sure to be on disk first.
    GetDpdfNet,
    DpdfNetReady(Result<(), String>),
    UpdateChecked(Result<(Box<crate::update::Verified>, crate::update::State), String>),
    UpdateProgress(crate::update::Progress),
    /// "Restart now": the downloaded installer starts and Rico closes.
    RestartToUpdate,
    InstallerStarted(Result<(), String>),
    DismissUpdate,
    /// "What's new": the release notes in the browser.
    OpenUpdateNotes,
    /// The small kick / ban / timeout window opened from a person's menu.
    ConfirmModPopup,
    CloseModPopup,
    PickModTimeout(u64),
    /// The direct messages asked from the menu ("Message").
    DmOpened(Result<Box<crate::discord::models::Channel>, String>),
    MessageAction(context::Action),
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
    /// The push-to-talk key held (`true`) or let go, anywhere.
    PushToTalk(bool),
    /// A key pressed or let go in the window: for push-to-talk, or as the
    /// new push-to-talk key while it is being chosen.
    WindowKey {
        code: String,
        modifiers: crate::audio::hotkey::Modifiers,
        down: bool,
    },
    /// Waiting (`true`) or no longer for the push-to-talk key in the settings.
    ChoosePushToTalkKey(bool),
    CloseVerification,
    /// The window got (`true`) or lost the focus.
    Focus(bool),
    /// A click on a notification: open its channel.
    NotificationClicked(String),
    /// Time to tell Discord how far a channel was read.
    AckDue(String),
    OpenServerMenu {
        guild_id: String,
        at: iced::Point,
    },
    CloseServerMenu,
    ServerAction(context::ServerAction),
    ConfirmLeaveServer,
    /// Discord's answer to muting or unmuting a server.
    GuildMuted(Result<Box<crate::discord::models::GuildNotifySettings>, String>),
    /// Discord's answer to leaving a server.
    LeftGuild(Result<(), String>),
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
    /// The right-click menu of a message, when open.
    message_menu: Option<context::Menu>,
    /// The right-click menu of a person, when open.
    user_menu: Option<context::UserMenu>,
    /// The right-click menu of a server icon, when open.
    server_menu: Option<context::ServerMenu>,
    /// The push-to-talk key registered system-wide (during calls only).
    ptt: ptt::Global,
    /// The push-to-talk key is held in the window.
    ptt_window_held: bool,
    /// The card with someone's end-to-end encryption verification code.
    verification: Option<voice::Verification>,
    /// The window has the focus (notifications are not shown for the
    /// channel being looked at).
    focused: bool,
    /// Per channel, the last read position sent to Discord, and when.
    acked: HashMap<String, (String, std::time::Instant)>,
    /// Channels with a read position to send shortly.
    ack_due: std::collections::HashSet<String>,
    /// When the last notifications were shown (to space them out).
    toast_times: std::collections::VecDeque<std::time::Instant>,
    /// The Friends view.
    friends: friends::FriendsUi,
    /// The kick / ban / timeout window opened from that menu.
    mod_popup: Option<context::ModPopup>,
    /// In-app updates: what the updater is doing.
    update: updates::UpdateUi,
    /// The presence last sent on the gateway (op 3), not to send it again.
    sent_presence: Option<SentPresence>,
    /// The file dialog is open (only one at a time).
    picking_files: bool,
    /// The settings window, when open.
    settings_panel: Option<settings::Panel>,
    dpdfnet_download: settings::ModelDownload,
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
            message_menu: None,
            user_menu: None,
            server_menu: None,
            ptt: ptt::Global::default(),
            ptt_window_held: false,
            verification: None,
            focused: true,
            acked: HashMap::new(),
            ack_due: std::collections::HashSet::new(),
            toast_times: std::collections::VecDeque::new(),
            friends: friends::FriendsUi::default(),
            mod_popup: None,
            update: updates::UpdateUi::default(),
            sent_presence: None,
            picking_files: false,
            settings_panel: None,
            dpdfnet_download: settings::ModelDownload::Idle,
            system_dark: true,
            profile_cache: HashMap::new(),
        }
    }

    pub fn new(opts: Options) -> (Self, Task<Message>) {
        let mut gui = Self::blank();
        if opts.updated {
            gui.update.updated_to = Some(crate::update::current_version().to_string());
        }
        // Installers of updates already applied are no longer needed. After
        // an update the installer is still running for a moment (its file
        // in use), so this waits, off the interface thread.
        if !opts.demo && !opts.demo_login && !cfg!(test) {
            if let Some(dir) = crate::update::download_dir() {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(20));
                    let n = crate::update::remove_installers(&dir, &crate::update::current_version());
                    if n > 0 {
                        tracing::info!("update: removed {n} old installer files");
                    }
                });
            }
        }
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
        // The first update check waits: the start stays fast.
        let check = if gui.updates_on() {
            Task::perform(tokio::time::sleep(crate::update::FIRST_CHECK_DELAY), |()| {
                Message::UpdateCheck
            })
        } else {
            Task::none()
        };
        (gui, Task::batch([task, check]))
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

    /// `DPDFNet`'s model: kept if already on disk and genuine, otherwise
    /// downloaded (never in the demo, which makes no outside connection).
    fn get_dpdfnet(&mut self) -> Task<Message> {
        if self.dpdfnet_download == settings::ModelDownload::Running {
            return Task::none();
        }
        self.dpdfnet_download = settings::ModelDownload::Running;
        let demo = self.demo;
        Task::perform(
            async move {
                if demo {
                    return match tokio::task::spawn_blocking(crate::audio::dpdfnet::read_model).await {
                        Ok(Ok(_)) => Ok(()),
                        _ => Err(t("not downloaded in the demo", "non si scarica nella demo").to_string()),
                    };
                }
                crate::audio::dpdfnet::ensure_model()
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::DpdfNetReady,
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
        let waiting = match self.store.total_mentions() {
            0 => String::new(),
            n if n > 99 => "(99+) ".to_string(),
            n => format!("({n}) "),
        };
        match &self.store.current {
            Some(c) if self.screen == Screen::Main => format!("{waiting}{} — Rico", self.store.title(c)),
            _ => format!("{waiting}Rico"),
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
        // Ctrl+, opens the settings; Esc closes them (and the menus).
        if self.message_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseMessageMenu),
                _ => None,
            }));
        } else if self.mod_popup.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseModPopup),
                _ => None,
            }));
        } else if self.server_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseServerMenu),
                _ => None,
            }));
        } else if self.verification.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseVerification),
                _ => None,
            }));
        } else if self.user_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseUserMenu),
                _ => None,
            }));
        } else if self.status_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseStatusMenu),
                _ => None,
            }));
        } else if self.settings_panel.as_ref().is_some_and(|p| !p.choosing_key) {
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
        // Keys for push-to-talk: being chosen in the settings, or held in the
        // window when the system-wide key could not be registered.
        let choosing_key = self.settings_panel.as_ref().is_some_and(|p| p.choosing_key);
        if choosing_key || (self.push_to_talk_key().is_some() && self.ptt.key().is_none()) {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    physical_key,
                    modifiers,
                    repeat: false,
                    ..
                }) => Some(Message::WindowKey {
                    code: ptt::code_of(physical_key)?,
                    modifiers: ptt::modifiers_of(modifiers),
                    down: true,
                }),
                iced::Event::Keyboard(keyboard::Event::KeyReleased {
                    physical_key,
                    modifiers,
                    ..
                }) => Some(Message::WindowKey {
                    code: ptt::code_of(physical_key)?,
                    modifiers: ptt::modifiers_of(modifiers),
                    down: false,
                }),
                _ => None,
            }));
        }
        if self.ptt.key().is_some() {
            subs.push(Subscription::run(ptt::events));
        }
        // The focus of the window: for notifications and read positions, and
        // for "like the system", whose theme is looked at again when the
        // window gets the focus (it is changed from outside the app), never
        // on a timer.
        subs.push(event::listen_with(|event, _status, _window| match event {
            iced::Event::Window(window::Event::Focused) => Some(Message::Focus(true)),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::Focus(false)),
            _ => None,
        }));
        if self.settings.notifications && !self.demo && self.screen == Screen::Main {
            subs.push(Subscription::run(notify::events));
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
        // Then one update check a day (a timer that fires once a day).
        if self.updates_on() {
            subs.push(iced::time::every(crate::update::CHECK_EVERY).map(|_| Message::UpdateCheck));
        }
        Subscription::batch(subs)
    }

    // ------------------------------------------------------------ session

    // ------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_push_to_talk();
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
                self.friends.open = false;
                let rows = self.store.channel_rows(guild);
                if let Some(ChannelRow::Channel {
                    id, selectable: true, ..
                }) = rows.get(first_selectable(&rows))
                {
                    return self.open_channel(id.clone());
                }
                // No conversation yet: the friends.
                if guild.is_none() {
                    return self.open_friends();
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
            Message::OpenMessageMenu(id, at) => {
                self.user_menu = None;
                self.open_message_menu(id, at);
            }
            Message::CloseMessageMenu => self.message_menu = None,
            Message::OpenUserMenu {
                user_id,
                guild_id,
                at,
            } => {
                self.message_menu = None;
                self.open_user_menu(user_id, guild_id, at);
            }
            Message::CloseUserMenu => self.user_menu = None,
            Message::UserAction(action) => return self.user_action(action),
            Message::OpenFriends => return self.open_friends(),
            Message::Friends(m) => return self.friends_update(m),
            Message::MessageUser(id) => return self.message_user(id),
            Message::CloseModPopup => self.mod_popup = None,
            Message::UpdateCheck => return self.check_for_update(),
            Message::GetDpdfNet => return self.get_dpdfnet(),
            Message::DpdfNetReady(result) => match result {
                Ok(()) => {
                    self.dpdfnet_download = settings::ModelDownload::Idle;
                    return self.update(Message::Setting(settings::Change::NoiseSuppression(
                        crate::audio::process::Noise::DpdfNet,
                    )));
                }
                Err(e) => {
                    tracing::warn!("noise suppression: DPDFNet model not available: {e}");
                    self.dpdfnet_download = settings::ModelDownload::Failed(e);
                }
            },
            Message::UpdateChecked(result) => return self.update_checked(result),
            Message::UpdateProgress(p) => self.update_progress(p),
            Message::RestartToUpdate => return self.restart_to_update(),
            Message::InstallerStarted(result) => match result {
                Ok(()) => return iced::exit(),
                Err(e) => {
                    tracing::warn!("update: installer not started: {e}");
                    self.update.phase = updates::Phase::Idle;
                    self.update.last = updates::LastCheck::CannotCheck;
                    self.status = Some((
                        tf!(
                            "The update could not start: {e}",
                            "L'aggiornamento non è partito: {e}"
                        ),
                        true,
                    ));
                }
            },
            Message::DismissUpdate => self.update.dismissed = true,
            Message::OpenUpdateNotes => {
                if let Some(url) = updates::notes_url(&self.update) {
                    open_in_browser(&url);
                }
            }
            Message::PickModTimeout(secs) => {
                if let Some(p) = self
                    .mod_popup
                    .as_mut()
                    .filter(|p| p.step == context::ModStep::Timeout)
                {
                    let secs = secs.clamp(1, rest::MAX_TIMEOUT_SECS);
                    p.step = context::ModStep::Confirm(ModAction::Timeout(Some(secs)));
                }
            }
            Message::ConfirmModPopup => {
                if let Some(p) = self.mod_popup.take() {
                    if let context::ModStep::Confirm(action) = p.step {
                        return self.send_moderation(p.guild_id, p.user_id, action);
                    }
                }
            }
            Message::DmOpened(result) => match result {
                Ok(c) => {
                    let id = c.id.clone();
                    self.store.upsert_channel(*c);
                    if self.store.is_dm(&id) {
                        let task = self.update(Message::SelectGuild(None));
                        return Task::batch([task, self.update(Message::OpenChannel(id))]);
                    }
                }
                Err(e) => {
                    self.status = Some((
                        tf!(
                            "The conversation could not be opened: {e}",
                            "Impossibile aprire la conversazione: {e}"
                        ),
                        true,
                    ));
                }
            },
            Message::MessageAction(action) => return self.message_action(action),
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
                // From the menu (no card open on them): the outcome in the
                // status bar.
                let card_open = self.profile.as_ref().is_some_and(|p| {
                    p.user_id == user_id && p.guild_id.as_deref() == Some(guild_id.as_str())
                });
                if !card_open {
                    let name = self
                        .store
                        .users
                        .get(&user_id)
                        .map_or_else(|| user_id.clone(), |u| self.store.name_in(Some(&guild_id), u));
                    self.status = Some(match &result {
                        Ok(()) => (format!("{name}: {}", profile::done_label(&action)), false),
                        Err(e) => (tf!("{name}: not done: {e}", "{name}: non riuscito: {e}"), true),
                    });
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
            Message::ChoosePushToTalkKey(on) => {
                if let Some(p) = &mut self.settings_panel {
                    p.choosing_key = on;
                }
            }
            Message::WindowKey {
                code,
                modifiers,
                down,
            } => return self.window_key(&code, modifiers, down),
            Message::PushToTalk(held) => {
                if self.push_to_talk_key().is_some() {
                    if let Some(h) = self.voice.as_ref().and_then(|v| v.handle.as_ref()) {
                        h.push_to_talk(held);
                    }
                }
            }
            Message::CloseVerification => self.verification = None,
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
            Message::Focus(focused) => {
                self.focused = focused;
                let mut tasks = Vec::new();
                if focused {
                    if self.settings.theme == ThemeChoice::System {
                        tasks.push(check_system_theme());
                    }
                    // Back to the window: what is on screen is now read.
                    if let Some(c) = self.store.current.clone() {
                        let latest = self.store.latest_message(&c);
                        self.store.mark_read(&c, latest.as_deref());
                        tasks.push(self.ack(c));
                    }
                }
                return Task::batch(tasks);
            }
            Message::NotificationClicked(channel_id) => return self.open_from_notification(channel_id),
            Message::OpenServerMenu { guild_id, at } => {
                self.message_menu = None;
                self.user_menu = None;
                self.open_server_menu(guild_id, at);
            }
            Message::CloseServerMenu => self.server_menu = None,
            Message::ServerAction(action) => return self.server_action(action),
            Message::ConfirmLeaveServer => return self.leave_server(),
            Message::GuildMuted(result) => match result {
                Ok(settings) => self.store.set_notify_settings(*settings),
                Err(e) => self.status = Some((e, true)),
            },
            Message::LeftGuild(result) => {
                if let Err(e) = result {
                    self.status = Some((e, true));
                }
            }
            Message::AckDue(channel_id) => {
                self.ack_due.remove(&channel_id);
                if self.store.current.as_deref() == Some(channel_id.as_str()) && self.focused {
                    return self.ack(channel_id);
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
                let channel_id = m.channel_id.clone();
                self.maybe_notify(&m);
                if self.store.insert_message(*m) {
                    let mut tasks = vec![self.request_previews()];
                    // Seen as it arrives: read.
                    if self.focused {
                        let latest = self.store.latest_message(&channel_id);
                        self.store.mark_read(&channel_id, latest.as_deref());
                        tasks.push(self.ack(channel_id));
                    }
                    return Task::batch(tasks);
                }
            }
            GatewayEvent::MessageAck(a) => self.store.apply_ack(&a),
            GatewayEvent::GuildNotify(g) => self.store.set_notify_settings(*g),
            GatewayEvent::MessageUpdate(u) => {
                self.store.update_message(*u);
                return self.request_previews();
            }
            GatewayEvent::MessageDelete(d) => {
                self.forget_message(&d.id);
                self.store.delete_message(&d);
            }
            GatewayEvent::RelationshipUpsert(r) => {
                self.store.upsert_relationship(*r);
                if self.friends.open {
                    return self.request_small_images();
                }
            }
            GatewayEvent::RelationshipRemove(id) => {
                self.store.remove_relationship(&id);
                if self.friends.confirm_remove.as_deref() == Some(id.as_str()) {
                    self.friends.confirm_remove = None;
                }
            }
            GatewayEvent::Presence(p) => self.store.apply_presence(&p),
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

    /// Shows the Friends view in place of the open conversation.
    fn open_friends(&mut self) -> Task<Message> {
        self.guild = None;
        self.friends.open = true;
        self.friends.confirm_remove = None;
        self.hovered = None;
        self.jump = None;
        self.highlight = None;
        // No conversation is open: new messages count as unread.
        self.store.current = None;
        self.request_small_images()
    }

    fn open_channel(&mut self, id: String) -> Task<Message> {
        self.friends.open = false;
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
        let ack = self.ack(id.clone());
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
        tasks.push(ack);
        Task::batch(tasks)
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
                    muted_for_us: &self.settings.audio.muted_users,
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
        if let Some(menu) = self.message_menu.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(context::view(menu));
        }
        if let Some(menu) = self.user_menu.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(context::user_view(menu));
        }
        if let Some(p) = self.mod_popup.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(context::mod_view(p));
        }
        if let Some(m) = self.server_menu.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(context::server_view(m));
        }
        if let Some(v) = self.verification.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(voice::verification_view(v));
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
            let status = self.update.status_line(self.settings.check_updates);
            layers = layers.push(settings::view(
                p,
                &self.settings,
                &self.store,
                self.system_dark,
                status,
                self.updates_on() && matches!(self.update.phase, updates::Phase::Idle),
                &self.dpdfnet_download,
            ));
        }
        layers.into()
    }

    fn view_main(&self) -> Element<'_, Message> {
        column![self.top_bar(), hline(pal::border())]
            .push(self.update.banner())
            .push(self.main_body())
            .into()
    }

    fn main_body(&self) -> Element<'_, Message> {
        column![
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
                        muted_for_us: &self.settings.audio.muted_users,
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
        if self.friends.open && self.store.current.is_none() {
            crumbs = crumbs
                .push(
                    text(t("Direct messages", "Messaggi diretti"))
                        .size(13)
                        .color(pal::muted()),
                )
                .push(text("/").size(13).color(pal::muted()))
                .push(text(t("Friends", "Amici")).size(13));
        }
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
            self.store.dm_mentions(),
            None,
            Message::SelectGuild(None),
        ));
        col = col.push(container(Space::new().width(24).height(1)).style(fill(pal::border())));
        let guild_button = |i: usize| -> Element<'_, Message> {
            let g = &self.store.guilds[i];
            let guild_id = g.id.clone();
            context::RightClick::new(
                rail_button(
                    initials(g.name()),
                    g.name().to_string(),
                    self.guild == Some(i),
                    self.store.guild_has_unread(&g.id),
                    self.store.guild_mentions(&g.id),
                    media::guild_icon_url(&g.id, g.icon(), ICON_PX).and_then(|u| self.small_images.get(&u)),
                    Message::SelectGuild(Some(i)),
                ),
                move |at| Message::OpenServerMenu {
                    guild_id: guild_id.clone(),
                    at,
                },
            )
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
                    "Server folders (also right click on a folder)",
                    "Cartelle dei server (anche clic destro su una cartella)",
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
        if self.guild.is_none() {
            list = list
                .push(friends::entry(
                    self.friends.open && self.store.current.is_none(),
                    self.store.incoming_requests(),
                ))
                .push(container(Space::new().width(Length::Fill).height(1)).style(fill(pal::soft())));
        }
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
                        None => text(if locked {
                            "🔒"
                        } else {
                            crate::text::channel_symbol(k)
                        })
                        .size(13)
                        .color(pal::muted())
                        .width(16)
                        .shaping(Shaping::Advanced)
                        .into(),
                    };
                    let mut line = row![icon, label]
                        .spacing(if dm.is_some() { 8 } else { 4 })
                        .align_y(Vertical::Center);
                    let mentions = self.store.mentions(&id);
                    if mentions > 0 {
                        line = line.push(space::horizontal()).push(mention_badge(mentions));
                    } else if unread {
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
                            let flags = self.store.voice_status.get(&user_id).copied().unwrap_or_default();
                            let local = self.settings.audio.muted_users.contains(&user_id);
                            if let Some(m) = voice::marks(flags, local, 13.0) {
                                member = member.push(space::horizontal()).push(m);
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
                                if flags.marks().is_empty() && !local {
                                    member = member.push(space::horizontal());
                                }
                                member =
                                    member.push(stream::live_badge(Message::OpenStreamPreview(s.clone())));
                            }
                            let (menu_user, menu_guild) = (user_id.clone(), guild.clone());
                            let profile = Message::OpenProfile {
                                user_id: user_id.clone(),
                                guild_id: guild,
                            };
                            // Highlighted on hover like the channels. A click opens
                            // the profile; on someone streaming it watches them, the
                            // mouse over them shows the small preview (like Discord)
                            // and the right click opens the person's menu.
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
                                            .on_enter(Message::HoverStream(s)),
                                        card,
                                        iced::widget::tooltip::Position::Right,
                                    )
                                    .gap(8)
                                    .into()
                                }
                                None => row_button.on_press(profile).into(),
                            };
                            let entry = context::RightClick::new(entry, move |at| Message::OpenUserMenu {
                                user_id: menu_user.clone(),
                                guild_id: menu_guild.clone(),
                                at,
                            });
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
        if self.friends.open && self.guild.is_none() && self.store.current.is_none() {
            return friends::view(&self.friends, &self.store, &self.small_images);
        }
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
            let ts = crate::text::parse_ts(&m.timestamp);
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
                    person_menu(
                        button(
                            text(name)
                                .font(SEMIBOLD)
                                .shaping(Shaping::Advanced)
                                .color(user_color(&m.author.id))
                        )
                        .padding([0.0, NAME_PAD])
                        .style(name_link)
                        .on_press(open_profile),
                        &m.author.id,
                        guild,
                    ),
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
                            person_menu(
                                button(
                                    text(name.clone())
                                        .font(SEMIBOLD)
                                        .shaping(Shaping::Advanced)
                                        .color(user_color(&m.author.id))
                                )
                                .padding([1.0, NAME_PAD])
                                .style(name_link)
                                .on_press(open_profile.clone()),
                                &m.author.id,
                                guild,
                            ),
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
                let picture = person_menu(
                    avatar_button(&name, &m.author.id, 32.0, image, open_profile),
                    &m.author.id,
                    guild,
                );
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
            let id = m.id.clone();
            col = col.push(context::RightClick::new(
                mouse_area(stack![item, overlay])
                    .on_enter(Message::HoverStart(m.id.clone()))
                    .on_exit(Message::HoverEnd(m.id.clone())),
                move |at| Message::OpenMessageMenu(id.clone(), at),
            ));

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

/// A person's name or avatar: a right click opens their menu.
fn person_menu<'a>(
    content: impl Into<Element<'a, Message>>,
    user_id: &str,
    guild_id: Option<&str>,
) -> Element<'a, Message> {
    let (user_id, guild_id) = (user_id.to_string(), guild_id.map(str::to_string));
    context::RightClick::new(content, move |at| Message::OpenUserMenu {
        user_id: user_id.clone(),
        guild_id: guild_id.clone(),
        at,
    })
    .into()
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

/// The red number of mentions waiting ("99+" above 99).
fn mention_badge<'a>(n: u32) -> Element<'a, Message> {
    let label = if n > 99 { "99+".to_string() } else { n.to_string() };
    container(text(label).size(11).font(BOLD).color(iced::Color::WHITE))
        .padding([0, 5])
        .style(rounded(pal::red(), 8.0))
        .into()
}

/// An icon of the server column with the mentions waiting at its
/// bottom-right corner, as on Discord.
fn with_mentions<'a>(icon: impl Into<Element<'a, Message>>, mentions: u32) -> Element<'a, Message> {
    if mentions == 0 {
        return icon.into();
    }
    iced::widget::stack![
        container(icon).padding(Padding {
            top: 0.0,
            right: 4.0,
            bottom: 2.0,
            left: 0.0
        }),
        container(mention_badge(mentions))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom),
    ]
    .into()
}

#[allow(clippy::too_many_arguments)]
fn rail_button(
    label: String,
    name: String,
    selected: bool,
    unread: bool,
    mentions: u32,
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
        with_mentions(icon, mentions),
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

fn day_separator<'a>(day: NaiveDate) -> Element<'a, Message> {
    let label = crate::text::day_label(day);
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

/// Opens a checked web address (Rico's release notes) in the default
/// browser. No shell: the address is one argument.
fn open_in_browser(url: &str) {
    use std::process::Command;
    let result = if cfg!(windows) {
        Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open the browser: {e}");
    }
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
mod tests;

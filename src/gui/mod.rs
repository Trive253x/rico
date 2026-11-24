//! Graphical interface (iced), with the layout of the "iced in gpui style" mock-up.
//!
//! Elm architecture: the state is [`Gui`], every event (click, key, network
//! answer, gateway event) is a [`Message`] applied by `update`, and `view`
//! rebuilds the interface from the state. Gateway and QR login run as
//! subscriptions: iced starts and stops them by itself when the state changes.

mod anim;
mod calls;
mod chat;
mod compose;
mod context;
mod covered;
mod folders;
mod friends;
mod golive;
mod icons;
mod images;
mod jump;
mod layout;
mod login;
mod members;
mod messages;
mod notify;
mod people;
mod picker;
mod preferences;
mod presence;
mod profile;
mod ptt;
mod role;
mod settings;
mod stream;
mod style;
mod subscriptions;
mod updates;
mod voice;
mod watch;

// Drawing helpers used by other parts of the GUI too.
use self::chat::{custom_emoji, embed_source, media_box, no_send_permission, preview_key};
use self::images::demo_shaped;
use self::layout::with_mentions;

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
    button, container, mouse_area, opaque, qr_code, rich_text, row, scrollable, space, span, stack, text,
    text_input, tooltip, Column, Row, Space,
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
    GetDpdfNet(crate::audio::process::Noise),
    DpdfNetReady(crate::audio::process::Noise, Result<(), String>),
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
    /// The mouse is (or is no longer) over the stream's controls: they stay.
    WatchControls(bool),
    /// The mouse moved over the stream: its controls come back.
    WatchMouseMoved,
    /// Timer while the stream's controls show: hides them when idle.
    WatchIdle,
    /// The stream goes to the mini player, in a corner over the rest.
    MinimizeStream,
    /// The mini player becomes the stream window again.
    ExpandStream,
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
            watch_members: false,
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

    /// The `DPDFNet` model `noise` needs: kept if already on disk and
    /// genuine, otherwise downloaded (never in the demo, which makes no
    /// outside connection). Then the choice is applied.
    fn get_dpdfnet(&mut self, noise: crate::audio::process::Noise) -> Task<Message> {
        let Some(variant) = noise.dpdfnet() else {
            return Task::none();
        };
        if self.dpdfnet_download == settings::ModelDownload::Running {
            return Task::none();
        }
        self.dpdfnet_download = settings::ModelDownload::Running;
        let demo = self.demo;
        Task::perform(
            async move {
                if demo {
                    return match tokio::task::spawn_blocking(move || variant.read()).await {
                        Ok(Ok(_)) => Ok(()),
                        _ => Err(t("not downloaded in the demo", "non si scarica nella demo").to_string()),
                    };
                }
                variant.ensure().await.map_err(|e| format!("{e:#}"))
            },
            move |result| Message::DpdfNetReady(noise, result),
        )
    }

    /// A menu or a small window is open over the rest: Esc is for it.
    pub(super) fn popup_open(&self) -> bool {
        self.message_menu.is_some()
            || self.user_menu.is_some()
            || self.mod_popup.is_some()
            || self.server_menu.is_some()
            || self.verification.is_some()
            || self.status_menu.is_some()
            || self.folder_editor.is_some()
            || self.golive_picker.is_some()
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

    /// Sends each message to the handler of its area (`handle_login` in
    /// login.rs, `handle_messages` in messages.rs, …); the general ones
    /// (gateway, channels, window) are handled here. A variant routed to an
    /// area must have its arm there: the area's `_` arm does nothing.
    fn handle(&mut self, message: Message) -> Task<Message> {
        // Frames of GIFs no longer on screen are dropped (at most once a second).
        anim::sweep();
        match message {
            m @ (Message::LoginTab(..)
            | Message::RetryQr
            | Message::Auth(..)
            | Message::TokenInput(..)
            | Message::SubmitToken
            | Message::TokenChecked(..)
            | Message::EmailInput(..)
            | Message::PasswordInput(..)
            | Message::SubmitPassword
            | Message::CodeInput(..)
            | Message::SubmitCode
            | Message::CancelTwoFactor
            | Message::PasswordAnswer(..)
            | Message::Logout) => return self.handle_login(m),
            m @ (Message::History { .. }
            | Message::ComposerChanged(..)
            | Message::Send
            | Message::Sent { .. }
            | Message::StartReply(..)
            | Message::CancelReply
            | Message::TogglePingReply
            | Message::StartEdit(..)
            | Message::CancelEdit
            | Message::Edited(..)
            | Message::AskDelete(..)
            | Message::CancelDelete
            | Message::ConfirmDelete
            | Message::Deleted { .. }
            | Message::ComposerEscape
            | Message::React { .. }
            | Message::Reacted(..)
            | Message::OpenReactionPicker(..)
            | Message::FileDropped(..)
            | Message::PickFiles
            | Message::OpenMessageMenu(..)
            | Message::CloseMessageMenu
            | Message::MessageAction(..)
            | Message::FilesPicked(..)
            | Message::FileRead(..)
            | Message::RemoveFile(..)
            | Message::ClearFiles
            | Message::Scrolled(..)
            | Message::JumpToMessage(..)
            | Message::JumpOffset(..)
            | Message::EndHighlight(..)
            | Message::HoverStart(..)
            | Message::HoverEnd(..)
            | Message::NotificationClicked(..)
            | Message::AckDue(..)) => return self.handle_messages(m),
            m @ (Message::OpenUserMenu { .. }
            | Message::CloseUserMenu
            | Message::UserAction(..)
            | Message::OpenFriends
            | Message::Friends(..)
            | Message::MessageUser(..)
            | Message::CloseModPopup
            | Message::PickModTimeout(..)
            | Message::ConfirmModPopup
            | Message::DmOpened(..)
            | Message::OpenStatusMenu
            | Message::CloseStatusMenu
            | Message::StatusEdit(..)
            | Message::SetPresence(..)
            | Message::SaveCustomStatus
            | Message::ClearCustomStatus
            | Message::StatusSaved(..)
            | Message::OpenProfile { .. }
            | Message::OpenRole { .. }
            | Message::CloseRole
            | Message::RoleShowAll
            | Message::MemberFilter(..)
            | Message::ToggleOfflineMembers
            | Message::ProfileLoaded { .. }
            | Message::ProfileTab(..)
            | Message::CloseProfile
            | Message::ToggleMemberList
            | Message::MemberListMore
            | Message::MentionUser(..)
            | Message::CopyUserId(..)
            | Message::Moderate(..)
            | Message::ConfirmModeration
            | Message::CancelModeration
            | Message::Moderated { .. }
            | Message::OpenServerMenu { .. }
            | Message::CloseServerMenu
            | Message::ServerAction(..)
            | Message::ConfirmLeaveServer
            | Message::GuildMuted(..)
            | Message::LeftGuild(..)) => return self.handle_people(m),
            m @ (Message::UpdateCheck
            | Message::UpdateChecked(..)
            | Message::UpdateProgress(..)
            | Message::RestartToUpdate
            | Message::InstallerStarted(..)
            | Message::DismissUpdate
            | Message::OpenUpdateNotes) => return self.handle_updates(m),
            m @ (Message::PreviewLoaded { .. }
            | Message::OpenViewer(..)
            | Message::ViewerLoaded { .. }
            | Message::CloseViewer
            | Message::SaveAttachment
            | Message::Saved(..)
            | Message::ShowInFolder(..)
            | Message::TogglePicker(..)
            | Message::PickerTab(..)
            | Message::ClosePicker
            | Message::EmojiQuery(..)
            | Message::EmojiSection(..)
            | Message::PickServerEmoji(..)
            | Message::SmallImageLoaded { .. }
            | Message::PickEmoji(..)
            | Message::GifQuery(..)
            | Message::GifSearch(..)
            | Message::GifResults { .. }
            | Message::PickGif(..)) => return self.handle_images(m),
            m @ (Message::JoinVoice(..)
            | Message::LeaveVoice
            | Message::ToggleMicrophone
            | Message::UserVolume { .. }
            | Message::ToggleUserMute(..)
            | Message::OpenStreamPreview(..)
            | Message::RefreshStreamPreview
            | Message::StreamPreviewLoaded { .. }
            | Message::HoverStream(..)
            | Message::CloseStreamPreview
            | Message::WatchStream
            | Message::WatchStreamOf(..)
            | Message::StopWatching
            | Message::WatchEscape
            | Message::ToggleFullscreen
            | Message::VideoPressed
            | Message::WatchControls(..)
            | Message::WatchMouseMoved
            | Message::WatchIdle
            | Message::MinimizeStream
            | Message::ExpandStream
            | Message::ToggleWatchMembers
            | Message::StreamVolume(..)
            | Message::StreamVolumeStep(..)
            | Message::ToggleStreamSound
            | Message::WatchVoice(..)
            | Message::OpenGoLive
            | Message::GoLiveSources(..)
            | Message::GoLiveSelect(..)
            | Message::GoLiveQuality(..)
            | Message::GoLiveSound(..)
            | Message::CloseGoLive
            | Message::StartGoLive
            | Message::StopGoLive
            | Message::GoLiveVoice(..)
            | Message::PushToTalk(..)
            | Message::CloseVerification
            | Message::ToggleDeafen
            | Message::Voice(..)) => return self.handle_calls(m),
            m @ (Message::GetDpdfNet(_)
            | Message::DpdfNetReady(..)
            | Message::SettingsLoaded(..)
            | Message::ToggleFolder(..)
            | Message::OpenFolderEditor
            | Message::CloseFolderEditor
            | Message::FolderEdit(..)
            | Message::SaveFolders
            | Message::FoldersSaved(..)
            | Message::OpenSettings
            | Message::CloseSettings
            | Message::ChoosePushToTalkKey(..)
            | Message::OpenGifSettings
            | Message::SettingsSection(..)
            | Message::Setting(..)
            | Message::SaveSettings
            | Message::SaveFailed(..)
            | Message::RefreshAudioDevices
            | Message::AudioDevices { .. }
            | Message::SystemTheme(..)
            | Message::ToggleLanguage) => return self.handle_preferences(m),
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
            Message::Noop => {}
            Message::GatewayLink(handle) => self.gateway_link = Some(handle),
            Message::WindowKey {
                code,
                modifiers,
                down,
            } => return self.window_key(&code, modifiers, down),
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
        // While the stream fills the window, the chat under it is neither
        // drawn (it would be drawn at every picture, then covered) nor given
        // events; its widgets and scroll position stay as they are.
        let hidden = self.screen == Screen::Main && self.watch.as_ref().is_some_and(|w| !w.minimized);
        // Explicit background: the window's does not always follow the theme.
        let base: Element<'_, Message> = covered::covered(
            container(screen)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(fill(pal::bg())),
            hidden,
        )
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
        if let Some(w) = self
            .watch
            .as_ref()
            .filter(|w| w.minimized && self.screen == Screen::Main)
        {
            let voice = self.voice.as_ref();
            let can_speak = voice.is_some_and(|v| self.store.can(&v.channel_id, permissions::SPEAK));
            layers = layers.push(watch::mini_view(w, &self.store, voice, can_speak));
        }
        if let Some(w) = self
            .watch
            .as_ref()
            .filter(|w| !w.minimized && self.screen == Screen::Main)
        {
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
                    audio: &self.settings.audio,
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
            layers = layers.push(context::user_view(menu, &self.settings.audio));
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
}

// ---------------------------------------------------------------- subscriptions

/// Where a connection's stream sends its messages to the interface.
type Output = iced::futures::channel::mpsc::Sender<Message>;

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

// ---------------------------------------------------------------- interface pieces

fn thin() -> scrollable::Direction {
    scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(6).scroller_width(6).margin(2))
}

fn hline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
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

/// Width of a space between words when the text is split to make room for
/// the emoji images.
const WORD_SPACE: f32 = 4.0;

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

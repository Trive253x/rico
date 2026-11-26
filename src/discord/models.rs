//! Minimal data models. Every optional field has `#[serde(default)]`:
//! Discord payloads for user accounts change shape often and a missing
//! field must not make deserialization fail.

use std::cmp::Ordering;

use serde::Deserialize;

use crate::permissions::Overwrite;

/// Channel types (`type` field).
pub mod kind {
    pub const GUILD_TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const GUILD_VOICE: u8 = 2;
    pub const GROUP_DM: u8 = 3;
    pub const CATEGORY: u8 = 4;
    pub const ANNOUNCEMENT: u8 = 5;
    pub const STAGE: u8 = 13;
}

/// Compares two snowflakes (numeric IDs as strings) without converting them.
pub fn cmp_snowflake(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    /// Nitro subscription (0 or missing = none). Without Nitro the animated
    /// server emojis cannot be used.
    #[serde(default)]
    pub premium_type: Option<u8>,
    /// Avatar hash (`a_…` if animated); `None` = Discord's default avatar.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Banner hash, only in profiles.
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub accent_color: Option<u32>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.username)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
    /// Some READY versions only send the recipients' IDs.
    #[serde(default)]
    pub recipient_ids: Vec<String>,
    /// Group conversation picture (image hash).
    #[serde(default)]
    pub icon: Option<String>,
    /// Permission overwrites (server channels). An unreadable item is
    /// skipped, it does not throw away the channel.
    #[serde(default, deserialize_with = "de_overwrites")]
    pub permission_overwrites: Vec<Overwrite>,
}

fn de_overwrites<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Overwrite>, D::Error> {
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

impl Channel {
    pub fn is_text(&self) -> bool {
        matches!(
            self.kind,
            kind::GUILD_TEXT | kind::DM | kind::GROUP_DM | kind::ANNOUNCEMENT
        )
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.kind, kind::GUILD_VOICE | kind::STAGE)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildProperties {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
}

/// Server role: name and colour shown in profiles, permissions.
#[derive(Debug, Clone, Deserialize)]
pub struct Role {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// RGB colour, 0 = none.
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub position: i64,
    #[serde(default, deserialize_with = "crate::permissions::de_bits")]
    pub permissions: u64,
}

/// Member of a server: nickname, server avatar and roles. In messages it
/// comes without `user` (the author is the message's).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildMember {
    #[serde(default)]
    pub user: Option<User>,
    /// Some READY versions only send the ID.
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub nick: Option<String>,
    /// Server profile avatar hash.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Server profile banner hash.
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub joined_at: Option<String>,
    /// Timed out until this moment.
    #[serde(default)]
    pub communication_disabled_until: Option<String>,
}

impl GuildMember {
    pub fn id(&self) -> Option<&str> {
        self.user
            .as_ref()
            .map(|u| u.id.as_str())
            .or(self.user_id.as_deref())
    }
}

/// Answer of `GET /users/{id}/profile`.
#[derive(Debug, Clone, Deserialize)]
pub struct UserProfile {
    pub user: User,
    #[serde(default)]
    pub user_profile: Option<ProfileDetails>,
    /// Present when asked with `guild_id` and the user is in that server.
    #[serde(default)]
    pub guild_member: Option<GuildMember>,
    #[serde(default)]
    pub guild_member_profile: Option<ProfileDetails>,
}

/// Text parts of a profile (account or server).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileDetails {
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(default)]
    pub pronouns: Option<String>,
    #[serde(default)]
    pub accent_color: Option<u32>,
    #[serde(default)]
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// With some "capabilities" the name is here instead of in `name`.
    #[serde(default)]
    pub properties: Option<GuildProperties>,
    #[serde(default)]
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub emojis: Vec<GuildEmoji>,
    /// Who is in the voice channels right now.
    #[serde(default)]
    pub voice_states: Vec<VoiceState>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub roles: Vec<Role>,
    /// In the READY only a few members (usually just us).
    #[serde(default)]
    pub members: Vec<GuildMember>,
    #[serde(default)]
    pub owner_id: Option<String>,
    /// What the server notifies by default: 0 every message, 1 only
    /// mentions (`None`: not sent, taken as only mentions).
    #[serde(default)]
    pub default_message_notifications: Option<u8>,
}

/// Voice state of a user (`VOICE_STATE_UPDATE`, or in the READY guilds).
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceState {
    #[serde(default)]
    pub guild_id: Option<String>,
    /// `None`: the user left voice.
    #[serde(default)]
    pub channel_id: Option<String>,
    pub user_id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    /// Muted or deafened by a moderator of the server.
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub deaf: bool,
    /// Sharing the screen or a game (Go Live).
    #[serde(default)]
    pub self_stream: bool,
    #[serde(default)]
    pub member: Option<VoiceMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceMember {
    #[serde(default)]
    pub user: Option<User>,
}

/// Assigned voice server (`VOICE_SERVER_UPDATE`). `endpoint` is `None`
/// while Discord is still choosing the server.
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceServerUpdate {
    pub token: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Custom emoji of a server.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GuildEmoji {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub animated: bool,
    /// `false` if the server lost the boosts that made it available.
    #[serde(default = "yes")]
    pub available: bool,
    /// If not empty, only members with one of these roles can use it.
    #[serde(default)]
    pub roles: Vec<String>,
}

fn yes() -> bool {
    true
}

/// `GUILD_EMOJIS_UPDATE` event: the full, updated list.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildEmojisUpdate {
    pub guild_id: String,
    #[serde(default)]
    pub emojis: Vec<GuildEmoji>,
}

impl Guild {
    pub fn name(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| self.properties.as_ref().and_then(|p| p.name.as_deref()))
            .unwrap_or("…")
    }

    /// Server icon hash, wherever this version of the payload put it.
    pub fn icon(&self) -> Option<&str> {
        self.icon
            .as_deref()
            .or_else(|| self.properties.as_ref().and_then(|p| p.icon.as_deref()))
    }

    pub fn owner_id(&self) -> Option<&str> {
        self.owner_id
            .as_deref()
            .or_else(|| self.properties.as_ref().and_then(|p| p.owner_id.as_deref()))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub url: String,
    /// Copy on Discord's media proxy: accepts `width`/`height` for thumbnails.
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Bit 3 (`IS_SPOILER`): shown only after a click.
    #[serde(default, deserialize_with = "lenient_u64")]
    pub flags: u64,
}

impl Attachment {
    /// Marked as a spoiler (Discord: the flag, or a name starting with
    /// `SPOILER_`).
    pub fn is_spoiler(&self) -> bool {
        self.flags & (1 << 3) != 0 || self.filename.starts_with("SPOILER_")
    }

    /// Static or animated image that can be previewed.
    pub fn is_image(&self) -> bool {
        let by_type = self
            .content_type
            .as_deref()
            .is_some_and(|t| t.starts_with("image/") && t != "image/svg+xml");
        let lower = self.filename.to_ascii_lowercase();
        let by_name = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
            .iter()
            .any(|ext| lower.ends_with(ext));
        // Without dimensions Discord did not recognise it as an image.
        (by_type || by_name) && self.width.unwrap_or(0) > 0 && self.height.unwrap_or(0) > 0
    }
}

/// Reaction emoji: Unicode (`name` is the character) or custom (`id` present).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

impl Emoji {
    /// Text to show: the Unicode character, or `:name:` for custom emojis.
    pub fn label(&self) -> String {
        match (&self.id, &self.name) {
            (Some(_), Some(name)) => format!(":{name}:"),
            (None, Some(name)) => name.clone(),
            _ => "?".to_string(),
        }
    }

    pub fn same(&self, other: &Self) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.name == other.name,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reaction {
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub me: bool,
    #[serde(default)]
    pub emoji: Emoji,
}

/// The message being replied to: only its author and text are needed.
#[derive(Debug, Clone, Deserialize)]
pub struct MessagePreview {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub author: Option<User>,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub mention_everyone: bool,
    /// Roles mentioned with `<@&id>`.
    #[serde(default)]
    pub mention_roles: Vec<String>,
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub referenced_message: Option<Box<MessagePreview>>,
    /// Link previews generated by Discord (Tenor/Giphy GIFs, images…) and
    /// bots' cards. At most [`MAX_EMBEDS`].
    #[serde(default, deserialize_with = "bounded_embeds")]
    pub embeds: Vec<Embed>,
    /// Only from the gateway, in servers: the author as a member.
    #[serde(default)]
    pub member: Option<GuildMember>,
    #[serde(default)]
    pub guild_id: Option<String>,
}

/// Most embeds kept per message (Discord's own limit).
pub const MAX_EMBEDS: usize = 10;
/// Most fields kept per embed (Discord's own limit).
pub const MAX_EMBED_FIELDS: usize = 25;

/// Link preview created by Discord, or a bot's card.
///
/// For GIF links the type is `gifv` (with an MP4 `video` and a static `thumbnail`), for image links
/// `image`; pages are `article` or `link`, videos `video`, bots' cards
/// `rich`. Texts are cut to Discord's limits: the data comes from others.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default, deserialize_with = "short::<_, 32>")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "short::<_, 2048>")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "short::<_, 256>")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "short::<_, 4096>")]
    pub description: Option<String>,
    /// The colour of the bar on the left (`0xRRGGBB`).
    #[serde(default, deserialize_with = "colour")]
    pub color: Option<u32>,
    #[serde(default, deserialize_with = "short::<_, 64>")]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default, deserialize_with = "bounded_fields")]
    pub fields: Vec<EmbedField>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default, deserialize_with = "short::<_, 256>")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedAuthor {
    #[serde(default, deserialize_with = "short::<_, 256>")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedFooter {
    #[serde(default, deserialize_with = "short::<_, 2048>")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default, deserialize_with = "short::<_, 256>")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "short::<_, 1024>")]
    pub value: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub inline: bool,
}

/// A text, cut to `N` characters; anything but a string is no text (a
/// wrong type does not lose the whole message).
fn short<'de, D: serde::Deserializer<'de>, const N: usize>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) if s.chars().count() > N => Some(s.chars().take(N).collect()),
        serde_json::Value::String(s) => Some(s),
        _ => None,
    })
}

/// A colour `0xRRGGBB`; anything else is none.
fn colour<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(serde_json::Value::deserialize(d)?
        .as_u64()
        .filter(|c| *c <= 0xFF_FF_FF)
        .and_then(|c| u32::try_from(c).ok()))
}

fn lenient_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(d)?.as_bool().unwrap_or(false))
}

fn lenient_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(serde_json::Value::deserialize(d)?.as_u64().unwrap_or(0))
}

/// The first [`MAX_EMBED_FIELDS`] fields; one that cannot be read is
/// skipped.
fn bounded_fields<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<EmbedField>, D::Error> {
    Ok(bounded_list(serde_json::Value::deserialize(d)?, MAX_EMBED_FIELDS))
}

/// As [`bounded_embeds`], for an update (missing: unchanged).
fn some_bounded_embeds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<Embed>>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => None,
        v => Some(bounded_list(v, MAX_EMBEDS)),
    })
}

/// The first [`MAX_EMBEDS`] embeds; one that cannot be read is skipped.
fn bounded_embeds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Embed>, D::Error> {
    Ok(bounded_list(serde_json::Value::deserialize(d)?, MAX_EMBEDS))
}

fn bounded_list<T: serde::de::DeserializeOwned>(v: serde_json::Value, max: usize) -> Vec<T> {
    match v {
        serde_json::Value::Array(items) => items
            .into_iter()
            .take(max)
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedMedia {
    #[serde(default)]
    pub url: String,
    /// Copy on Discord's proxy (images-ext-*.discordapp.net).
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// `MESSAGE_UPDATE` may contain only the fields that changed.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageUpdate {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    /// Discord adds link previews with a later `MESSAGE_UPDATE`.
    #[serde(default, deserialize_with = "some_bounded_embeds")]
    pub embeds: Option<Vec<Embed>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
}

/// `TYPING_START`: someone is typing in a channel.
#[derive(Debug, Clone, Deserialize)]
pub struct TypingStart {
    pub channel_id: String,
    pub user_id: String,
    #[serde(default)]
    pub member: Option<Member>,
}

/// `MESSAGE_REACTION_ADD` / `MESSAGE_REACTION_REMOVE`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReactionEvent {
    pub user_id: String,
    pub channel_id: String,
    pub message_id: String,
    #[serde(default)]
    pub emoji: Emoji,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelete {
    pub id: String,
    pub channel_id: String,
}

/// `STREAM_CREATE`: the stream we asked to watch and its voice server.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamCreate {
    pub stream_key: String,
    pub rtc_server_id: String,
    #[serde(default)]
    pub paused: bool,
}

/// `STREAM_SERVER_UPDATE`: where to connect for a stream. `endpoint` is
/// `null` while Discord is still choosing the server.
#[derive(Clone, Deserialize)]
pub struct StreamServerUpdate {
    pub stream_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub token: String,
}

impl std::fmt::Debug for StreamServerUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a credential: never printed.
        f.debug_struct("StreamServerUpdate")
            .field("stream_key", &self.stream_key)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// `STREAM_DELETE`: the stream ended, or we cannot watch it (`reason`, e.g.
/// `stream_full`, `unauthorized`).
#[derive(Debug, Clone, Deserialize)]
pub struct StreamDelete {
    pub stream_key: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
}

/// `GUILD_ROLE_CREATE` and `GUILD_ROLE_UPDATE`.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildRoleUpdate {
    pub guild_id: String,
    pub role: Role,
}

/// `GUILD_ROLE_DELETE`.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildRoleDelete {
    pub guild_id: String,
    pub role_id: String,
}

/// `GUILD_MEMBER_UPDATE`: the member (roles, nickname, timeout) with its server.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildMemberUpdate {
    pub guild_id: String,
    #[serde(flatten)]
    pub member: GuildMember,
}

/// `GUILD_DELETE`: we left the server (or it is temporarily unavailable).
#[derive(Debug, Clone, Deserialize)]
pub struct GuildDelete {
    pub id: String,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ready {
    pub user: User,
    pub session_id: String,
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
    #[serde(default)]
    pub guilds: Vec<Guild>,
    #[serde(default)]
    pub private_channels: Vec<Channel>,
    #[serde(default)]
    pub users: Vec<User>,
    /// Our member in each server, in the order of `guilds`.
    #[serde(default)]
    pub merged_members: Vec<Vec<GuildMember>>,
    /// Friends, friend requests and blocked people.
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    /// The status of our friends (with `capabilities: 0`).
    #[serde(default)]
    pub presences: Vec<PresenceUpdate>,
    /// Where we stopped reading, per channel, and how many mentions wait.
    #[serde(default, deserialize_with = "entries")]
    pub read_state: Vec<ReadState>,
    /// Our notification and mute settings per server (and for DMs).
    #[serde(default, deserialize_with = "entries")]
    pub user_guild_settings: Vec<GuildNotifySettings>,
}

/// A list Discord sends either as it is or as `{"entries": [...]}`
/// (versioned read states and settings); anything else is an empty list.
fn entries<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape<T> {
        List(Vec<T>),
        Wrapped { entries: Vec<T> },
        Other(serde::de::IgnoredAny),
    }
    Ok(match Shape::<T>::deserialize(d)? {
        Shape::List(v) | Shape::Wrapped { entries: v } => v,
        Shape::Other(_) => Vec::new(),
    })
}

/// A Discord ID sent as a string or a number (`0`: none).
fn lenient_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s)
            if !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(s)
        }
        serde_json::Value::Number(n) => n.as_u64().filter(|n| *n > 0).map(|n| n.to_string()),
        _ => None,
    })
}

/// Where we stopped reading a channel (`read_state` in READY).
#[derive(Debug, Clone, Deserialize)]
pub struct ReadState {
    /// The channel (for type 0).
    pub id: String,
    #[serde(default, deserialize_with = "lenient_id")]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub mention_count: u32,
    /// 0: a channel; other types (server events, notification centre)
    /// are ignored.
    #[serde(default)]
    pub read_state_type: u8,
}

/// When a mute ends (`None`: until turned off).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MuteConfig {
    #[serde(default)]
    pub end_time: Option<String>,
}

/// Our notification settings for a channel inside a server.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelNotifySettings {
    pub channel_id: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub mute_config: Option<MuteConfig>,
    /// 0 every message, 1 only mentions, 2 nothing, 3 as the server.
    #[serde(default = "inherit")]
    pub message_notifications: u8,
}

fn inherit() -> u8 {
    3
}

/// Our notification settings for a server (`guild_id: None`: the direct
/// messages). From READY and `USER_GUILD_SETTINGS_UPDATE`.
#[derive(Debug, Clone, Deserialize)]
pub struct GuildNotifySettings {
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub mute_config: Option<MuteConfig>,
    /// 0 every message, 1 only mentions, 2 nothing, 3 the server's default.
    #[serde(default = "inherit")]
    pub message_notifications: u8,
    #[serde(default)]
    pub suppress_everyone: bool,
    #[serde(default)]
    pub suppress_roles: bool,
    #[serde(default)]
    pub channel_overrides: Vec<ChannelNotifySettings>,
}

/// `MESSAGE_ACK`: a channel was read (on this or another device).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageAck {
    pub channel_id: String,
    #[serde(default, deserialize_with = "lenient_id")]
    pub message_id: Option<String>,
    #[serde(default)]
    pub mention_count: u32,
}

/// What a person is to us (`type` of a relationship).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Friend,
    Blocked,
    /// They sent us a friend request.
    Incoming,
    /// We sent them a friend request.
    Outgoing,
}

impl RelationKind {
    /// `None` for kinds Rico does not show (5 "implicit" and anything new).
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Friend),
            2 => Some(Self::Blocked),
            3 => Some(Self::Incoming),
            4 => Some(Self::Outgoing),
            _ => None,
        }
    }
}

/// A relationship, in READY and in `RELATIONSHIP_ADD`, `_UPDATE` and
/// `_REMOVE` (the last two without `user`). Its `id` is the person's.
#[derive(Debug, Clone, Deserialize)]
pub struct Relationship {
    pub id: String,
    /// See [`RelationKind::from_code`]; missing = unknown, never a friend.
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub user: Option<User>,
    /// The nickname we gave them (only we see it).
    #[serde(default)]
    pub nickname: Option<String>,
}

/// Only the ID of a user (presences carry a partial user).
#[derive(Debug, Clone, Deserialize)]
pub struct UserId {
    pub id: String,
}

/// `PRESENCE_UPDATE`, and the presences in READY.
#[derive(Debug, Clone, Deserialize)]
pub struct PresenceUpdate {
    pub user: UserId,
    /// Present for presences inside a server: Rico uses only the others
    /// (friends).
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub activities: Vec<super::member_list::Activity>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn relationships_and_presences_in_ready() {
        let r: Ready = serde_json::from_str(
            r#"{"user": {"id": "1", "username": "me"}, "session_id": "s",
                "relationships": [
                    {"id": "2", "type": 1, "user": {"id": "2", "username": "ada"}, "nickname": null},
                    {"id": "3", "type": 3, "user": {"id": "3", "username": "bob"}},
                    {"id": "4", "type": 5},
                    {"id": "5"}
                ],
                "presences": [{"user": {"id": "2"}, "status": "idle",
                    "activities": [{"type": 4, "name": "Custom Status", "state": "away"}]}]}"#,
        )
        .unwrap();
        let kinds: Vec<_> = r
            .relationships
            .iter()
            .map(|x| RelationKind::from_code(x.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                Some(RelationKind::Friend),
                Some(RelationKind::Incoming),
                None,
                None
            ]
        );
        assert_eq!(r.presences[0].user.id, "2");
        assert_eq!(r.presences[0].status, "idle");
        assert!(r.presences[0].guild_id.is_none());
    }

    #[test]
    fn permissions_in_the_payloads() {
        let g: Guild = serde_json::from_str(
            r#"{"id": "100", "owner_id": "1",
                "roles": [{"id": "100", "name": "@everyone", "permissions": "1024"}],
                "channels": [{"id": "5", "type": 0, "permission_overwrites": [
                    {"id": "100", "type": 0, "allow": "0", "deny": "1024"},
                    {"id": "x", "type": "something new", "allow": "0", "deny": "0"}
                ]}]}"#,
        )
        .unwrap();
        assert_eq!(g.owner_id(), Some("1"));
        assert_eq!(g.roles[0].permissions, crate::permissions::VIEW_CHANNEL);
        // The unknown overwrite is skipped, the channel is kept.
        assert_eq!(g.channels[0].permission_overwrites.len(), 1);
        let u: GuildMemberUpdate = serde_json::from_str(
            r#"{"guild_id": "100", "user": {"id": "1", "username": "me"}, "roles": ["7"],
                "communication_disabled_until": null}"#,
        )
        .unwrap();
        assert_eq!(u.member.id(), Some("1"));
        assert_eq!(u.member.roles, vec!["7"]);
    }

    #[test]
    fn message_with_reply_and_reactions() {
        let json = r#"{
            "id": "5", "channel_id": "1", "content": "ok",
            "author": {"id": "2", "username": "giulia"},
            "referenced_message": {"id": "4", "content": "domanda?", "author": {"id": "3", "username": "marco"}},
            "reactions": [
                {"count": 2, "me": true, "emoji": {"id": null, "name": "👍"}},
                {"count": 1, "me": false, "emoji": {"id": "99", "name": "ferris"}}
            ]
        }"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert_eq!(m.referenced_message.as_ref().unwrap().content, "domanda?");
        assert_eq!(m.reactions[0].emoji.label(), "👍");
        assert_eq!(m.reactions[1].emoji.label(), ":ferris:");
        // A reply to a deleted message has `referenced_message: null`.
        let m: Message = serde_json::from_str(
            r#"{"id":"6","channel_id":"1","author":{"id":"2"},"referenced_message":null}"#,
        )
        .unwrap();
        assert!(m.referenced_message.is_none());
    }

    #[test]
    fn bot_cards_are_read_tolerantly_and_bounded() {
        let long = "x".repeat(5000);
        let fields: Vec<_> = (0..40)
            .map(|i| json!({"name": format!("f{i}"), "value": "v", "inline": true}))
            .collect();
        let embeds: Vec<_> = (0..15)
            .map(|_| {
                json!({
                    "type": "rich", "title": long, "description": long, "color": 5814783,
                    "author": {"name": "Bot"}, "footer": {"text": "foot"}, "fields": fields,
                    "timestamp": "2026-11-26T10:00:00+00:00"
                })
            })
            .collect();
        let m: Message = serde_json::from_value(json!({
            "id": "5", "channel_id": "1", "content": "",
            "author": {"id": "2", "username": "bot"},
            "timestamp": "2026-11-26T10:00:00+00:00",
            "embeds": embeds,
            "attachments": [{"id": "9", "filename": "a.png", "flags": 8}]
        }))
        .unwrap();
        assert_eq!(m.embeds.len(), MAX_EMBEDS);
        let e = &m.embeds[0];
        assert_eq!(e.title.as_ref().map(|t| t.chars().count()), Some(256));
        assert_eq!(e.description.as_ref().map(|t| t.chars().count()), Some(4096));
        assert_eq!(e.fields.len(), MAX_EMBED_FIELDS);
        assert_eq!(e.color, Some(0x58_B9_FF));
        assert!(m.attachments[0].is_spoiler());

        // Wrong types lose only what is wrong, never the message.
        let m: Message = serde_json::from_value(json!({
            "id": "6", "channel_id": "1", "content": "",
            "author": {"id": "2", "username": "bot"},
            "timestamp": "2026-11-26T10:00:00+00:00",
            "embeds": [
                {"type": "rich", "title": 5, "color": -1, "fields": [{"name": null, "value": {}}, 3]},
                "not an embed",
                {"type": "rich", "color": 99999999999u64}
            ],
            "attachments": [{"id": "9", "filename": "SPOILER_a.png", "flags": "x"}]
        }))
        .unwrap();
        assert_eq!(m.embeds.len(), 2);
        assert_eq!(m.embeds[0].title, None);
        assert_eq!(m.embeds[0].color, None);
        assert_eq!(m.embeds[0].fields.len(), 1);
        assert_eq!(m.embeds[1].color, None);
        assert!(m.attachments[0].is_spoiler());
        // An update without embeds, or with `null`, leaves them as they are.
        let u: MessageUpdate =
            serde_json::from_value(json!({"id": "6", "channel_id": "1", "embeds": null})).unwrap();
        assert!(u.embeds.is_none());
    }

    #[test]
    fn tenor_gif_embed() {
        let json = r#"{
            "id": "7", "channel_id": "1", "content": "https://tenor.com/view/gatto-12345",
            "author": {"id": "2", "username": "giulia"},
            "embeds": [{
                "type": "gifv", "url": "https://tenor.com/view/gatto-12345",
                "provider": {"name": "Tenor", "url": "https://tenor.co"},
                "thumbnail": {"url": "https://media.tenor.com/abcAAAAe/gatto.png",
                    "proxy_url": "https://images-ext-1.discordapp.net/external/x/https/media.tenor.com/abcAAAAe/gatto.png",
                    "width": 498, "height": 280, "placeholder": "x", "flags": 0},
                "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4", "width": 498, "height": 280}
            }]
        }"#;
        let m: Message = serde_json::from_str(json).unwrap();
        let e = &m.embeds[0];
        assert_eq!(e.kind.as_deref(), Some("gifv"));
        assert_eq!(
            e.video.as_ref().unwrap().url,
            "https://media.tenor.com/abcAAAPo/gatto.mp4"
        );
        assert_eq!(e.provider.as_ref().unwrap().name.as_deref(), Some("Tenor"));
    }

    #[test]
    fn image_attachments() {
        let img: Attachment = serde_json::from_str(
            r#"{"id":"1","filename":"Foto.JPG","size":10,"url":"https://cdn.discordapp.com/a","proxy_url":"https://media.discordapp.net/a","width":800,"height":600}"#,
        )
        .unwrap();
        assert!(img.is_image());
        let svg: Attachment = serde_json::from_str(
            r#"{"filename":"x.svg","content_type":"image/svg+xml","width":10,"height":10}"#,
        )
        .unwrap();
        assert!(!svg.is_image());
        let zip: Attachment = serde_json::from_str(r#"{"filename":"log.zip","size":5}"#).unwrap();
        assert!(!zip.is_image());
    }

    #[test]
    fn snowflakes_sorted_as_numbers() {
        assert_eq!(cmp_snowflake("99", "100"), Ordering::Less);
        assert_eq!(cmp_snowflake("200", "100"), Ordering::Greater);
    }

    #[test]
    fn ready_in_both_formats() {
        let json = r#"{
            "user": {"id": "1", "username": "tommaso", "global_name": null},
            "session_id": "abc",
            "resume_gateway_url": "wss://gateway-us-east1-b.discord.gg",
            "guilds": [
                {"id": "10", "name": "Classico", "channels": [{"id": "11", "type": 0, "name": "general"}]},
                {"id": "20", "properties": {"name": "Con capabilities"}, "channels": []},
                {"id": "30", "unavailable": true}
            ],
            "private_channels": [
                {"id": "40", "type": 1, "recipient_ids": ["2"], "last_message_id": "500"}
            ],
            "users": [{"id": "2", "username": "giulia", "global_name": "Giulia"}],
            "campo_nuovo_sconosciuto": {"x": 1}
        }"#;
        let r: Ready = serde_json::from_str(json).unwrap();
        assert_eq!(r.guilds[0].name(), "Classico");
        assert_eq!(r.guilds[1].name(), "Con capabilities");
        assert_eq!(r.guilds[2].name(), "…");
        assert_eq!(r.private_channels[0].recipient_ids, vec!["2"]);
        assert_eq!(r.users[0].display_name(), "Giulia");
        assert_eq!(r.user.display_name(), "tommaso");
    }
}

/// Discord GIF search result (`/gifs/search`, `/gifs/trending-gifs`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GifResult {
    /// GIF page: the link that is sent in the message.
    pub url: String,
    /// File in the requested format (`tinygif`: small GIF for the grid).
    pub src: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

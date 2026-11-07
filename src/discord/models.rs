//! Minimal data models. Every optional field has `#[serde(default)]`:
//! Discord payloads for user accounts change shape often and a missing
//! field must not make deserialization fail.

use std::cmp::Ordering;

use serde::Deserialize;

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
}

#[derive(Debug, Clone, Deserialize)]
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
}

impl Attachment {
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

    pub fn same(&self, other: &Emoji) -> bool {
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
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub referenced_message: Option<Box<MessagePreview>>,
    /// Link previews generated by Discord (Tenor/Giphy GIFs, images…).
    #[serde(default)]
    pub embeds: Vec<Embed>,
}

/// Link preview created by Discord. For GIF links the type is `gifv`
/// (with an MP4 `video` and a static `thumbnail`), for image links `image`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default)]
    pub name: Option<String>,
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

/// MESSAGE_UPDATE may contain only the fields that changed.
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
    /// Discord adds link previews with a later MESSAGE_UPDATE.
    #[serde(default)]
    pub embeds: Option<Vec<Embed>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
}

/// TYPING_START: someone is typing in a channel.
#[derive(Debug, Clone, Deserialize)]
pub struct TypingStart {
    pub channel_id: String,
    pub user_id: String,
    #[serde(default)]
    pub member: Option<Member>,
}

/// MESSAGE_REACTION_ADD / MESSAGE_REACTION_REMOVE.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

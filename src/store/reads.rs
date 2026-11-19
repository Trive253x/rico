//! What is read and what notifies: where we stopped reading each channel,
//! the mentions waiting for us, and our notification settings per server
//! and channel (as on the account, so the other devices agree).
//!
//! Data logic only, shared by the GUI and the terminal.

use std::collections::HashMap;

use super::Store;
use crate::discord::models::{cmp_snowflake, GuildNotifySettings, Message, MessageAck, ReadState};
use crate::permissions;

/// Mentions counted per channel at most (the badge shows "99+").
pub const MAX_MENTIONS: u32 = 999;
/// Most channels with a read state or notification settings kept.
const MAX_ENTRIES: usize = 20_000;

/// What a server or channel notifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    All,
    Mentions,
    Nothing,
}

impl Level {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::All),
            1 => Some(Self::Mentions),
            2 => Some(Self::Nothing),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Reads {
    /// Mentions waiting, per channel (in direct messages every message).
    mentions: HashMap<String, u32>,
    /// The last message read, per channel.
    read_upto: HashMap<String, String>,
    /// Our settings per server; the direct messages under "".
    settings: HashMap<String, GuildNotifySettings>,
}

/// Whether a mute is on now: on, and its end (if any) not passed.
fn mute_on(muted: bool, end: Option<&str>) -> bool {
    if !muted {
        return false;
    }
    match end.and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()) {
        Some(end) => end > chrono::Utc::now(),
        None => true,
    }
}

impl Store {
    /// Read states and notification settings from READY; then which
    /// channels have messages we have not read.
    pub(super) fn apply_reads(&mut self, read: Vec<ReadState>, settings: Vec<GuildNotifySettings>) {
        self.reads = Reads::default();
        for r in read
            .into_iter()
            .filter(|r| r.read_state_type == 0)
            .take(MAX_ENTRIES)
        {
            if r.mention_count > 0 {
                self.reads
                    .mentions
                    .insert(r.id.clone(), r.mention_count.min(MAX_MENTIONS));
            }
            if let Some(m) = r.last_message_id {
                self.reads.read_upto.insert(r.id, m);
            }
        }
        for s in settings.into_iter().take(MAX_ENTRIES) {
            self.set_notify_settings(s);
        }
        self.unread.clear();
        let mut unread = Vec::new();
        let channels = self
            .guilds
            .iter()
            .flat_map(|g| g.channels.iter())
            .chain(self.dms.iter());
        for c in channels {
            let Some(last) = c.last_message_id.as_deref() else {
                continue;
            };
            // No read state: never opened anywhere, taken as read (as many
            // old channels would otherwise all look unread).
            let Some(read) = self.reads.read_upto.get(&c.id) else {
                continue;
            };
            if cmp_snowflake(last, read).is_gt() {
                unread.push(c.id.clone());
            }
        }
        for id in unread {
            if self.can(&id, permissions::VIEW_CHANNEL) && !self.is_muted(&id) {
                self.unread.insert(id);
            }
        }
        // Mentions only where we can read.
        let visible: Vec<String> = self
            .reads
            .mentions
            .keys()
            .filter(|c| !self.can(c, permissions::VIEW_CHANNEL))
            .cloned()
            .collect();
        for c in visible {
            self.reads.mentions.remove(&c);
        }
    }

    /// Mentions in the demo.
    pub(super) fn demo_mentions(&mut self, channel_id: &str, n: u32) {
        self.reads
            .mentions
            .insert(channel_id.to_string(), n.min(MAX_MENTIONS));
    }

    /// Our notification settings of a server (or of the DMs) changed.
    pub fn set_notify_settings(&mut self, s: GuildNotifySettings) {
        let key = s.guild_id.clone().unwrap_or_default();
        if self.reads.settings.len() >= MAX_ENTRIES && !self.reads.settings.contains_key(&key) {
            return;
        }
        self.reads.settings.insert(key, s);
    }

    /// Mentions waiting in a channel.
    pub fn mentions(&self, channel_id: &str) -> u32 {
        self.reads.mentions.get(channel_id).copied().unwrap_or(0)
    }

    /// Mentions waiting in a server.
    pub fn guild_mentions(&self, guild_id: &str) -> u32 {
        self.reads
            .mentions
            .iter()
            .filter(|(c, _)| self.channel_guild.get(*c).is_some_and(|g| g == guild_id))
            .map(|(_, n)| *n)
            .sum::<u32>()
            .min(MAX_MENTIONS)
    }

    /// Messages waiting in the direct messages.
    pub fn dm_mentions(&self) -> u32 {
        self.dms
            .iter()
            .map(|c| self.mentions(&c.id))
            .sum::<u32>()
            .min(MAX_MENTIONS)
    }

    /// Everything waiting (for the window title).
    pub fn total_mentions(&self) -> u32 {
        self.reads.mentions.values().sum::<u32>().min(MAX_MENTIONS)
    }

    /// The server is muted (now).
    pub fn guild_muted(&self, guild_id: &str) -> bool {
        self.reads.settings.get(guild_id).is_some_and(|s| {
            mute_on(
                s.muted,
                s.mute_config.as_ref().and_then(|m| m.end_time.as_deref()),
            )
        })
    }

    /// The channel is muted (now), or its server is.
    pub fn is_muted(&self, channel_id: &str) -> bool {
        let guild = self.channel_guild.get(channel_id).cloned().unwrap_or_default();
        let Some(s) = self.reads.settings.get(&guild) else {
            return false;
        };
        if !guild.is_empty() && self.guild_muted(&guild) {
            return true;
        }
        s.channel_overrides
            .iter()
            .find(|o| o.channel_id == channel_id)
            .is_some_and(|o| {
                mute_on(
                    o.muted,
                    o.mute_config.as_ref().and_then(|m| m.end_time.as_deref()),
                )
            })
    }

    /// What a channel notifies: its own setting, else the server's, else the
    /// server's default. Direct messages: every message.
    pub fn notify_level(&self, channel_id: &str) -> Level {
        let Some(guild_id) = self.channel_guild.get(channel_id) else {
            return Level::All;
        };
        let s = self.reads.settings.get(guild_id);
        let channel = s
            .and_then(|s| s.channel_overrides.iter().find(|o| o.channel_id == channel_id))
            .and_then(|o| Level::from_code(o.message_notifications));
        let server = s.and_then(|s| Level::from_code(s.message_notifications));
        let default = self
            .guild(guild_id)
            .and_then(|g| g.default_message_notifications)
            .and_then(Level::from_code)
            .unwrap_or(Level::Mentions);
        channel.or(server).unwrap_or(default)
    }

    /// The message mentions us: by name, by one of our roles, or with
    /// @everyone/@here (unless we turned those off for the server). Every
    /// message in a direct conversation counts.
    pub fn mentions_me(&self, m: &Message) -> bool {
        let Some(me) = self.me_id() else { return false };
        let Some(guild_id) = self.channel_guild.get(&m.channel_id) else {
            return self.is_dm(&m.channel_id);
        };
        if m.mentions.iter().any(|u| u.id == me) {
            return true;
        }
        let s = self.reads.settings.get(guild_id);
        if m.mention_everyone && !s.is_some_and(|s| s.suppress_everyone) {
            return true;
        }
        if !m.mention_roles.is_empty() && !s.is_some_and(|s| s.suppress_roles) {
            if let Some(member) = self.member(guild_id, me) {
                return m.mention_roles.iter().any(|r| member.roles.contains(r));
            }
        }
        false
    }

    /// A new message from someone else, in a channel we are not looking at:
    /// counted if it mentions us.
    pub(super) fn count_mention(&mut self, m: &Message) {
        if self.mentions_me(m) {
            let n = self.reads.mentions.entry(m.channel_id.clone()).or_insert(0);
            *n = (*n + 1).min(MAX_MENTIONS);
        }
    }

    /// Whether a new message should make a notification, by our settings:
    /// not ours, not muted, and what the channel notifies. Whether the
    /// window is looking at it is up to the interface.
    pub fn should_notify(&self, m: &Message) -> bool {
        if self.me_id() == Some(m.author.id.as_str()) || self.is_muted(&m.channel_id) {
            return false;
        }
        if !self.can(&m.channel_id, permissions::VIEW_CHANNEL) {
            return false;
        }
        match self.notify_level(&m.channel_id) {
            Level::All => true,
            Level::Mentions => self.mentions_me(m),
            Level::Nothing => false,
        }
    }

    /// The newest message we know in a channel.
    pub fn latest_message(&self, channel_id: &str) -> Option<String> {
        if let Some(m) = self.messages.get(channel_id).and_then(|l| l.last()) {
            return Some(m.id.clone());
        }
        self.guild_of(channel_id)
            .and_then(|g| g.channels.iter().find(|c| c.id == channel_id))
            .or_else(|| self.dms.iter().find(|c| c.id == channel_id))
            .and_then(|c| c.last_message_id.clone())
    }

    /// The channel was read up to `message_id` (here or elsewhere).
    pub fn mark_read(&mut self, channel_id: &str, message_id: Option<&str>) {
        self.reads.mentions.remove(channel_id);
        self.unread.remove(channel_id);
        if let Some(id) = message_id {
            let newer = self
                .reads
                .read_upto
                .get(channel_id)
                .is_none_or(|old| cmp_snowflake(id, old).is_gt());
            if newer
                && (self.reads.read_upto.len() < MAX_ENTRIES || self.reads.read_upto.contains_key(channel_id))
            {
                self.reads
                    .read_upto
                    .insert(channel_id.to_string(), id.to_string());
            }
        }
    }

    /// The read position of a channel.
    pub fn read_upto(&self, channel_id: &str) -> Option<&str> {
        self.reads.read_upto.get(channel_id).map(String::as_str)
    }

    /// `MESSAGE_ACK`: read on this or another device.
    pub fn apply_ack(&mut self, a: &MessageAck) {
        if a.mention_count == 0 {
            self.mark_read(&a.channel_id, a.message_id.as_deref());
        } else {
            self.reads
                .mentions
                .insert(a.channel_id.clone(), a.mention_count.min(MAX_MENTIONS));
        }
    }

    /// Channels of a server with something unread or a mention, each with
    /// its newest message (for "Mark as read").
    pub fn guild_unread(&self, guild_id: &str) -> Vec<(String, String)> {
        let Some(g) = self.guild(guild_id) else {
            return Vec::new();
        };
        g.channels
            .iter()
            .filter(|c| self.unread.contains(&c.id) || self.mentions(&c.id) > 0)
            .filter_map(|c| Some((c.id.clone(), self.latest_message(&c.id)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::discord::models::Ready;
    use crate::store::Store;

    fn ready(json: serde_json::Value) -> Ready {
        serde_json::from_value(json).unwrap()
    }

    fn base() -> serde_json::Value {
        serde_json::json!({
            "user": {"id": "1", "username": "me"},
            "session_id": "s",
            "guilds": [{
                "id": "10", "name": "S", "owner_id": "1",
                "roles": [{"id": "10", "permissions": "1024"}, {"id": "11", "permissions": "0"}],
                "channels": [
                    {"id": "100", "type": 0, "name": "a", "last_message_id": "500"},
                    {"id": "101", "type": 0, "name": "b", "last_message_id": "300"},
                    {"id": "102", "type": 0, "name": "c", "last_message_id": "900"}
                ]
            }],
            "merged_members": [[{"user": {"id": "1", "username": "me"}, "roles": ["11"]}]],
            "private_channels": [{"id": "200", "type": 1, "last_message_id": "700",
                                  "recipients": [{"id": "2", "username": "x"}]}],
        })
    }

    #[test]
    fn unread_and_mentions_from_ready() {
        let mut r = base();
        // Both shapes Discord uses: a list, or {entries: [...]}.
        r["read_state"] = serde_json::json!({"entries": [
            {"id": "100", "last_message_id": "400", "mention_count": 2},
            {"id": "101", "last_message_id": "300"},
            {"id": "200", "last_message_id": 600, "mention_count": 1},
            {"id": "999", "last_message_id": "1", "read_state_type": 1, "mention_count": 5}
        ], "version": 3});
        r["user_guild_settings"] = serde_json::json!([
            {"guild_id": "10", "channel_overrides": [{"channel_id": "102", "muted": true}]}
        ]);
        let mut s = Store::default();
        s.apply_ready(ready(r));
        assert!(s.unread.contains("100"));
        assert!(!s.unread.contains("101"), "read up to the last message");
        assert!(!s.unread.contains("102"), "no read state, and muted");
        assert!(s.unread.contains("200"));
        assert_eq!(s.mentions("100"), 2);
        assert_eq!(s.guild_mentions("10"), 2);
        assert_eq!(s.dm_mentions(), 1);
        assert_eq!(s.total_mentions(), 3);
        assert!(s.is_muted("102") && !s.is_muted("100"));
        s.mark_read("100", Some("500"));
        assert_eq!(s.mentions("100"), 0);
        assert!(!s.unread.contains("100"));
        assert_eq!(s.read_upto("100"), Some("500"));
        // An older position does not go back.
        s.mark_read("100", Some("450"));
        assert_eq!(s.read_upto("100"), Some("500"));
        // Malformed lists are ignored, not fatal.
        let mut r = base();
        r["read_state"] = serde_json::json!("nonsense");
        r["user_guild_settings"] = serde_json::json!({"entries": 5});
        let mut s = Store::default();
        s.apply_ready(ready(r));
        assert!(s.unread.is_empty());
    }

    fn message(json: serde_json::Value) -> crate::discord::models::Message {
        let mut m =
            serde_json::json!({"id": "1000", "channel_id": "100", "author": {"id": "2", "username": "x"}});
        if let (Some(m), Some(extra)) = (m.as_object_mut(), json.as_object()) {
            m.extend(extra.clone());
        }
        serde_json::from_value(m).unwrap()
    }

    #[test]
    fn who_is_mentioned_and_what_notifies() {
        let mut s = Store::default();
        s.apply_ready(ready(base()));
        let plain = message(serde_json::json!({}));
        let named = message(serde_json::json!({"mentions": [{"id": "1", "username": "me"}]}));
        let everyone = message(serde_json::json!({"mention_everyone": true}));
        let role = message(serde_json::json!({"mention_roles": ["11"]}));
        let other_role = message(serde_json::json!({"mention_roles": ["12"]}));
        let dm = message(serde_json::json!({"channel_id": "200"}));
        let mine = message(serde_json::json!({"author": {"id": "1", "username": "me"}, "channel_id": "200"}));
        for (m, yes) in [
            (&plain, false),
            (&named, true),
            (&everyone, true),
            (&role, true),
            (&other_role, false),
            (&dm, true),
        ] {
            assert_eq!(s.mentions_me(m), yes, "{:?}", m.mentions);
        }
        // Default: only mentions in servers, everything in DMs, never our own.
        assert!(!s.should_notify(&plain) && s.should_notify(&named) && s.should_notify(&dm));
        assert!(!s.should_notify(&mine));
        // @everyone turned off for the server; the server muted until later.
        s.set_notify_settings(
            serde_json::from_value(
                serde_json::json!({"guild_id": "10", "suppress_everyone": true, "message_notifications": 0}),
            )
            .unwrap(),
        );
        assert!(!s.mentions_me(&everyone));
        assert!(s.should_notify(&plain), "every message");
        s.set_notify_settings(
            serde_json::from_value(serde_json::json!({"guild_id": "10", "muted": true,
                "mute_config": {"end_time": "2999-01-01T00:00:00+00:00"}}))
            .unwrap(),
        );
        assert!(s.guild_muted("10") && !s.should_notify(&named));
        // A mute that ended counts as off.
        s.set_notify_settings(
            serde_json::from_value(serde_json::json!({"guild_id": "10", "muted": true,
                "mute_config": {"end_time": "2001-01-01T00:00:00+00:00"}}))
            .unwrap(),
        );
        assert!(!s.guild_muted("10") && s.should_notify(&named));
        // Counted only for messages that mention us.
        s.count_mention(&plain);
        s.count_mention(&named);
        assert_eq!(s.mentions("100"), 1);
        s.apply_ack(
            &serde_json::from_value(serde_json::json!({"channel_id": "100", "message_id": "1000"})).unwrap(),
        );
        assert_eq!(s.mentions("100"), 0);
        assert_eq!(s.guild_unread("10"), Vec::<(String, String)>::new());
    }
}

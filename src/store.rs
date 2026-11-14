//! Session data, shared by the GUI and the terminal: servers, channels,
//! private conversations, loaded messages, unread, who is typing.
//!
//! Data logic only: no network and no interface. Both interfaces call these
//! methods when gateway events or REST API answers arrive.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::discord::folders::{GuildFolders, SettingsSnapshot};
use crate::discord::member_list::{MemberList, MemberListUpdate};
use crate::discord::models::{
    cmp_snowflake, kind, Channel, Emoji, Guild, GuildEmoji, GuildEmojisUpdate, GuildMember,
    GuildMemberUpdate, Message, MessageDelete, MessageUpdate, Reaction, ReactionEvent, Ready, Role,
    TypingStart, User, UserProfile, VoiceState,
};
use crate::discord::rest::HISTORY_PAGE;
use crate::permissions::{self, Base};

/// How long "is typing…" stays visible after the last `TYPING_START`.
pub const TYPING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRow {
    Category(String),
    Channel {
        id: String,
        name: String,
        kind: u8,
        selectable: bool,
    },
}

impl ChannelRow {
    pub fn is_selectable(&self) -> bool {
        matches!(self, Self::Channel { selectable: true, .. })
    }
}

#[derive(Default)]
pub struct Store {
    pub me: Option<User>,
    /// Servers, in Discord's order. Added and removed only through
    /// `upsert_guild` and `remove_guild` (they keep `rail` up to date).
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    pub users: HashMap<String, User>,
    pub channel_names: HashMap<String, String>,
    pub channel_guild: HashMap<String, String>,
    /// Loaded messages per channel, in chronological order.
    pub messages: HashMap<String, Vec<Message>>,
    pub loading: HashSet<String>,
    /// Channels whose whole history we already have.
    pub exhausted: HashSet<String>,
    pub unread: HashSet<String>,
    /// Open channel.
    pub current: Option<String>,
    /// Who is typing, per channel: (user id, name, when).
    typing: HashMap<String, Vec<(String, String, Instant)>>,
    /// Who is in a voice channel: user → (server, channel).
    pub voice: HashMap<String, (String, String)>,
    /// Who is streaming (Go Live) in the voice channel they are in.
    pub streaming: HashSet<String>,
    /// Microphone and sound of who is in a voice channel.
    pub voice_status: HashMap<String, VoiceFlags>,
    /// Member list on the right, per server (only the channel we asked).
    pub member_lists: HashMap<String, MemberList>,
    /// Server profiles we know: (server, user) → nickname, avatar, roles.
    pub members: HashMap<(String, String), GuildMember>,
    /// Server folders from the account settings (`None` until they arrive:
    /// the servers are then shown in their own order). Changed only through
    /// `set_folders`, which keeps `rail` up to date.
    folders: Option<GuildFolders>,
    /// The server column, built when servers or folders change (not at every
    /// redraw).
    rail: Vec<RailEntry>,
    /// Version of the account settings the folders came with.
    pub settings_version: Option<u32>,
}

/// An entry of the server column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailEntry {
    /// A server outside folders (index in `guilds`).
    Guild(usize),
    /// A folder with the servers we can show (indexes in `guilds`).
    Folder {
        id: i64,
        name: Option<String>,
        color: Option<u64>,
        guilds: Vec<usize>,
    },
}

impl Store {
    /// Fake data for the demo mode.
    pub fn demo() -> Self {
        let mut store = Self::default();
        store.apply_ready(crate::demo::ready());
        for (channel_id, msgs) in crate::demo::messages() {
            for m in &msgs {
                store.remember_users(m);
            }
            store.exhausted.insert(channel_id.clone());
            store.messages.insert(channel_id, msgs);
        }
        store.unread.insert(crate::demo::UNREAD_CHANNEL.to_string());
        let mut folders = GuildFolders::default();
        folders.normalize(&store.guild_ids());
        let id = folders.new_folder("200", Some("Hobby".into()), Some(0x9b59b6));
        folders.move_to_folder("300", id);
        store.set_folders(folders);
        store
    }

    pub fn guild_ids(&self) -> Vec<String> {
        self.guilds.iter().map(|g| g.id.clone()).collect()
    }

    /// Account settings arrived (whole, or only the changed fields).
    pub fn apply_settings(&mut self, snap: SettingsSnapshot) {
        if snap.data_version.is_some() {
            self.settings_version = snap.data_version;
        }
        if let Some(mut folders) = snap.folders {
            folders.normalize(&self.guild_ids());
            self.set_folders(folders);
        }
    }

    /// The server folders, if the account settings arrived.
    pub fn folders(&self) -> Option<&GuildFolders> {
        self.folders.as_ref()
    }

    /// New server folders (from Discord, or saved from the editor).
    pub fn set_folders(&mut self, folders: GuildFolders) {
        self.folders = Some(folders);
        self.refresh_rail();
    }

    /// The server column: folders and servers in the account's order. New
    /// servers (not yet in the settings) on top; without settings, the
    /// servers in their own order.
    pub fn rail(&self) -> &[RailEntry] {
        &self.rail
    }

    fn refresh_rail(&mut self) {
        self.rail = self.build_rail();
    }

    fn build_rail(&self) -> Vec<RailEntry> {
        let index: HashMap<&str, usize> = self
            .guilds
            .iter()
            .enumerate()
            .map(|(i, g)| (g.id.as_str(), i))
            .collect();
        let Some(folders) = &self.folders else {
            return (0..self.guilds.len()).map(RailEntry::Guild).collect();
        };
        let mut shown = HashSet::new();
        let mut out = Vec::new();
        for f in &folders.folders {
            let guilds: Vec<usize> = f
                .guild_ids
                .iter()
                .filter_map(|id| index.get(id.as_str()).copied())
                .filter(|i| shown.insert(*i))
                .collect();
            match f.id {
                Some(id) if !guilds.is_empty() => out.push(RailEntry::Folder {
                    id,
                    name: f.name.clone(),
                    color: f.color,
                    guilds,
                }),
                Some(_) => {}
                None => out.extend(guilds.into_iter().map(RailEntry::Guild)),
            }
        }
        let missing: Vec<RailEntry> = (0..self.guilds.len())
            .filter(|i| !shown.contains(i))
            .map(RailEntry::Guild)
            .collect();
        out.splice(0..0, missing);
        out
    }

    pub fn me_id(&self) -> Option<&str> {
        self.me.as_ref().map(|u| u.id.as_str())
    }

    /// New session (READY). Returns the open channel, to reload: after a
    /// READY (not a RESUME) some messages may be missing.
    pub fn apply_ready(&mut self, r: Ready) -> Option<String> {
        for u in r.users {
            self.users.insert(u.id.clone(), u);
        }
        self.users.insert(r.user.id.clone(), r.user.clone());
        self.me = Some(r.user);

        self.guilds.clear();
        self.channel_guild.clear();
        self.channel_names.clear();
        self.voice.clear();
        self.streaming.clear();
        self.voice_status.clear();
        self.member_lists.clear();
        self.members.clear();
        // Our own member in each server comes aligned with `guilds`.
        for (g, members) in r.guilds.iter().zip(r.merged_members) {
            for m in members {
                self.remember_member(&g.id, m);
            }
        }
        for g in r.guilds {
            self.insert_guild(g);
        }
        self.refresh_rail();

        let mut dms = r.private_channels;
        for c in &dms {
            for u in &c.recipients {
                self.users.insert(u.id.clone(), u.clone());
            }
        }
        dms.sort_by(|a, b| {
            cmp_snowflake(
                b.last_message_id.as_deref().unwrap_or("0"),
                a.last_message_id.as_deref().unwrap_or("0"),
            )
        });
        self.dms = dms;

        self.messages.clear();
        self.exhausted.clear();
        self.loading.clear();
        self.typing.clear();
        self.current.clone()
    }

    /// A server arrived or changed (`GUILD_CREATE`, `GUILD_UPDATE`).
    pub fn upsert_guild(&mut self, g: Guild) {
        if self.insert_guild(g) {
            self.refresh_rail();
        }
    }

    /// Stores a server; `true` if it is new (the server column changes).
    fn insert_guild(&mut self, mut g: Guild) -> bool {
        for m in std::mem::take(&mut g.members) {
            if let Some(u) = &m.user {
                self.users.insert(u.id.clone(), u.clone());
            }
            let id = g.id.clone();
            self.remember_member(&id, m);
        }
        self.voice.retain(|_, (guild, _)| *guild != g.id);
        let voice = &self.voice;
        self.streaming.retain(|u| voice.contains_key(u));
        self.voice_status.retain(|u, _| voice.contains_key(u));
        for v in std::mem::take(&mut g.voice_states) {
            self.apply_voice_state(&v, Some(&g.id));
        }
        for c in &mut g.channels {
            c.guild_id = Some(g.id.clone());
            self.channel_guild.insert(c.id.clone(), g.id.clone());
            if let Some(n) = &c.name {
                self.channel_names.insert(c.id.clone(), n.clone());
            }
        }
        if let Some(slot) = self.guilds.iter_mut().find(|x| x.id == g.id) {
            *slot = g;
            false
        } else {
            self.guilds.push(g);
            true
        }
    }

    /// Someone joined, left or moved to another voice channel.
    pub fn apply_voice_state(&mut self, v: &VoiceState, guild_id: Option<&str>) {
        if let Some(user) = v.member.as_ref().and_then(|m| m.user.clone()) {
            self.users.entry(user.id.clone()).or_insert(user);
        }
        if let (Some(guild), Some(channel)) = (v.guild_id.as_deref().or(guild_id), &v.channel_id) {
            self.voice
                .insert(v.user_id.clone(), (guild.to_string(), channel.clone()));
            self.voice_status.insert(
                v.user_id.clone(),
                VoiceFlags {
                    self_mute: v.self_mute,
                    self_deaf: v.self_deaf,
                    mute: v.mute,
                    deaf: v.deaf,
                },
            );
            if v.self_stream {
                self.streaming.insert(v.user_id.clone());
            } else {
                self.streaming.remove(&v.user_id);
            }
        } else {
            self.voice.remove(&v.user_id);
            self.streaming.remove(&v.user_id);
            self.voice_status.remove(&v.user_id);
        }
    }

    /// Stream key of someone streaming (Go Live): `guild:{server}:{channel}:{user}`.
    pub fn stream_key(&self, user_id: &str) -> Option<String> {
        if !self.streaming.contains(user_id) {
            return None;
        }
        let (guild, channel) = self.voice.get(user_id)?;
        crate::media::stream_key(guild, channel, user_id)
    }

    /// Who is in the voice channel, sorted by name: (id, name).
    pub fn voice_members(&self, channel_id: &str) -> Vec<(String, String)> {
        let mut members: Vec<(String, String)> = self
            .voice
            .iter()
            .filter(|(_, (_, c))| c == channel_id)
            .map(|(id, (guild, _))| {
                let name = self.users.get(id).map_or_else(
                    || crate::i18n::t("user", "utente").into(),
                    |u| self.name_in(Some(guild), u),
                );
                (id.clone(), name)
            })
            .collect();
        members.sort_by_key(|(_, name)| name.to_lowercase());
        members
    }

    pub fn remember_users(&mut self, m: &Message) {
        // The author in a message has the current name and avatar.
        self.users.insert(m.author.id.clone(), m.author.clone());
        for u in &m.mentions {
            self.users.entry(u.id.clone()).or_insert_with(|| u.clone());
        }
        if let Some(member) = &m.member {
            let guild = m
                .guild_id
                .clone()
                .or_else(|| self.channel_guild.get(&m.channel_id).cloned());
            if let Some(guild) = guild {
                let mut member = member.clone();
                member.user_id = Some(m.author.id.clone());
                self.remember_member(&guild, member);
            }
        }
    }

    /// Most server profiles kept: beyond this the map starts again.
    const MAX_MEMBERS: usize = 20_000;

    fn remember_member(&mut self, guild_id: &str, mut m: GuildMember) {
        let Some(user_id) = m.id().map(str::to_string) else {
            return;
        };
        // The user is kept in `users`; here only the server part.
        m.user = None;
        if self.members.len() >= Self::MAX_MEMBERS {
            self.members.clear();
        }
        self.members.insert((guild_id.to_string(), user_id), m);
    }

    /// Our profile or someone else's, opened from the interface: updates
    /// the name, avatar and server profile we show everywhere.
    pub fn apply_profile(&mut self, guild_id: Option<&str>, p: &UserProfile) {
        let mut user = p.user.clone();
        // Profile fields (bio, banner) stay in the profile.
        user.banner = None;
        self.users.insert(user.id.clone(), user);
        if let (Some(guild), Some(member)) = (guild_id, &p.guild_member) {
            let mut member = member.clone();
            member.user_id = Some(p.user.id.clone());
            self.remember_member(guild, member);
        }
    }

    pub fn member(&self, guild_id: &str, user_id: &str) -> Option<&GuildMember> {
        self.members.get(&(guild_id.to_string(), user_id.to_string()))
    }

    /// Name to show for a user: the server nickname, if any, otherwise the
    /// account name.
    pub fn name_in(&self, guild_id: Option<&str>, user: &User) -> String {
        guild_id
            .and_then(|g| self.member(g, &user.id))
            .and_then(|m| m.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| user.display_name().to_string())
    }

    /// Avatar address for a user: the server one if set, otherwise the
    /// account one. `None` = show the initials.
    pub fn avatar_url(&self, guild_id: Option<&str>, user_id: &str, size: u32) -> Option<String> {
        let server = guild_id.and_then(|g| {
            let hash = self.member(g, user_id)?.avatar.as_deref();
            crate::media::member_avatar_url(g, user_id, hash, size)
        });
        server.or_else(|| {
            let hash = self.users.get(user_id)?.avatar.as_deref();
            crate::media::avatar_url(user_id, hash, size)
        })
    }

    pub fn guild(&self, guild_id: &str) -> Option<&Guild> {
        self.guilds.iter().find(|g| g.id == guild_id)
    }

    /// Opens a channel; returns `true` if the history has to be downloaded.
    /// Without the permission to read the history only new messages are
    /// shown (as on Discord): nothing is asked.
    pub fn open(&mut self, channel_id: &str) -> bool {
        self.unread.remove(channel_id);
        self.current = Some(channel_id.to_string());
        if !self.can(channel_id, permissions::READ_MESSAGE_HISTORY) {
            self.exhausted.insert(channel_id.to_string());
            return false;
        }
        !self.exhausted.contains(channel_id)
            && self.messages.get(channel_id).is_none_or(std::vec::Vec::is_empty)
    }

    // ------------------------------------------------------------ permissions

    /// Our situation in a server, to compute the permissions of its
    /// channels. `None` without data on the roles or on who we are: then no
    /// permission is granted (missing data never opens anything).
    fn base_of(&self, g: &Guild) -> Option<Base> {
        if g.roles.is_empty() {
            return None;
        }
        let me = self.me_id()?;
        let member = self.member(&g.id, me);
        let roles: Vec<(&str, u64)> = g.roles.iter().map(|r| (r.id.as_str(), r.permissions)).collect();
        Some(Base::new(
            &g.id,
            me,
            g.owner_id() == Some(me),
            &roles,
            member.map(|m| m.roles.as_slice()).unwrap_or_default(),
            permissions::timed_out(member.and_then(|m| m.communication_disabled_until.as_deref())),
        ))
    }

    /// Our permissions in a channel (`permissions::*` bits). Private
    /// conversations we know (no server, so no server permissions):
    /// everything, Discord decides. Server channels without the data to
    /// compute them, and channels we do not know at all: nothing.
    pub fn permissions(&self, channel_id: &str) -> u64 {
        let Some(g) = self.guild_of(channel_id) else {
            return if self.is_dm(channel_id) {
                permissions::ALL
            } else {
                0
            };
        };
        let Some(base) = self.base_of(g) else {
            return 0;
        };
        match g.channels.iter().find(|c| c.id == channel_id) {
            Some(c) => base.in_channel(&c.permission_overwrites),
            None => 0,
        }
    }

    /// We have all the permissions in `bits` in the channel.
    pub fn can(&self, channel_id: &str, bits: u64) -> bool {
        self.permissions(channel_id) & bits == bits
    }

    /// How many server channels are hidden by the permissions (for the log).
    pub fn hidden_channels(&self) -> usize {
        self.guilds
            .iter()
            .filter_map(|g| Some((g, self.base_of(g)?)))
            .map(|(g, base)| {
                g.channels
                    .iter()
                    .filter(|c| c.is_text() || c.is_voice())
                    .filter(|c| base.in_channel(&c.permission_overwrites) & permissions::VIEW_CHANNEL == 0)
                    .count()
            })
            .sum()
    }

    /// We can write in the channel (viewing it and sending messages).
    pub fn can_send(&self, channel_id: &str) -> bool {
        self.can(channel_id, permissions::VIEW_CHANNEL | permissions::SEND_MESSAGES)
    }

    /// We can reply to messages in the channel (sending and reading the
    /// history the replied message is part of).
    pub fn can_reply(&self, channel_id: &str) -> bool {
        self.can(
            channel_id,
            permissions::VIEW_CHANNEL | permissions::SEND_MESSAGES | permissions::READ_MESSAGE_HISTORY,
        )
    }

    /// We can attach files to messages in the channel.
    pub fn can_attach(&self, channel_id: &str) -> bool {
        self.can(
            channel_id,
            permissions::VIEW_CHANNEL | permissions::SEND_MESSAGES | permissions::ATTACH_FILES,
        )
    }

    /// We can react with `emoji` to messages of the channel. A reaction that is
    /// already there only needs the history; a new one also `ADD_REACTIONS`
    /// (as Discord checks).
    pub fn can_react(&self, channel_id: &str, message_id: &str, emoji: &Emoji) -> bool {
        let base = permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY;
        let exists = self
            .find_message(channel_id, message_id)
            .is_some_and(|m| m.reactions.iter().any(|r| r.emoji.same(emoji)));
        if exists {
            self.can(channel_id, base)
        } else {
            self.can(channel_id, base | permissions::ADD_REACTIONS)
        }
    }

    /// A message of ours in the channel we can still edit: written by us, a
    /// normal message or a reply, and we can still write there.
    pub fn can_edit(&self, channel_id: &str, message_id: &str) -> bool {
        self.own_message(channel_id, message_id)
            .is_some_and(|m| matches!(m.kind, 0 | 19))
            && self.can_send(channel_id)
    }

    /// A message of ours we can delete. Messages of others need
    /// `MANAGE_MESSAGES` and are not offered.
    pub fn can_delete(&self, channel_id: &str, message_id: &str) -> bool {
        self.own_message(channel_id, message_id)
            .is_some_and(|m| matches!(m.kind, 0 | 19))
            && self.can(channel_id, permissions::VIEW_CHANNEL)
    }

    fn own_message(&self, channel_id: &str, message_id: &str) -> Option<&Message> {
        let me = self.me_id()?;
        self.find_message(channel_id, message_id)
            .filter(|m| m.author.id == me)
    }

    /// A message of an open channel.
    pub fn find_message(&self, channel_id: &str, message_id: &str) -> Option<&Message> {
        self.messages
            .get(channel_id)
            .and_then(|l| l.iter().rev().find(|m| m.id == message_id))
    }

    /// The whole message after an edit (the answer to our PATCH).
    pub fn replace_message(&mut self, new: Message) {
        self.remember_users(&new);
        if let Some(m) = self.find_message_mut(&new.channel_id, &new.id) {
            *m = new;
        }
    }

    /// Our reaction added or removed without the gateway (demo).
    pub fn local_reaction(&mut self, channel_id: &str, message_id: &str, emoji: &Emoji, add: bool) {
        let Some(me) = self.me_id().map(str::to_string) else {
            return;
        };
        let ev = ReactionEvent {
            user_id: me,
            channel_id: channel_id.to_string(),
            message_id: message_id.to_string(),
            emoji: emoji.clone(),
        };
        if add {
            self.reaction_add(&ev);
        } else {
            self.reaction_remove(&ev);
        }
    }

    /// We can join the voice channel.
    pub fn can_connect(&self, channel_id: &str) -> bool {
        self.can(channel_id, permissions::VIEW_CHANNEL | permissions::CONNECT)
    }

    /// The open channel is no longer visible (permissions changed, channel
    /// or server deleted): it is closed. Returns `true` if it was closed.
    pub fn close_if_hidden(&mut self) -> bool {
        let Some(c) = self.current.clone() else {
            return false;
        };
        let gone = match self.channel_guild.get(&c) {
            Some(_) => !self.can(&c, permissions::VIEW_CHANNEL),
            None => !self.is_dm(&c),
        };
        if gone {
            self.current = None;
            self.messages.remove(&c);
            self.exhausted.remove(&c);
        }
        gone
    }

    // ------------------------------------------------------------ live changes

    /// A channel was created or changed.
    pub fn upsert_channel(&mut self, mut c: Channel) {
        match c.guild_id.clone() {
            Some(gid) => {
                let Some(g) = self.guilds.iter_mut().find(|g| g.id == gid) else {
                    return;
                };
                self.channel_guild.insert(c.id.clone(), gid);
                if let Some(n) = &c.name {
                    self.channel_names.insert(c.id.clone(), n.clone());
                }
                match g.channels.iter_mut().find(|x| x.id == c.id) {
                    Some(slot) => *slot = c,
                    None => g.channels.push(c),
                }
            }
            None if c.kind == kind::DM || c.kind == kind::GROUP_DM => {
                for u in &c.recipients {
                    self.users.insert(u.id.clone(), u.clone());
                }
                match self.dms.iter_mut().find(|x| x.id == c.id) {
                    Some(slot) => {
                        // Updates do not always repeat the recipients.
                        if c.recipients.is_empty() && c.recipient_ids.is_empty() {
                            c.recipients = std::mem::take(&mut slot.recipients);
                            c.recipient_ids = std::mem::take(&mut slot.recipient_ids);
                        }
                        *slot = c;
                    }
                    None => self.dms.insert(0, c),
                }
            }
            None => {}
        }
    }

    pub fn delete_channel(&mut self, c: &Channel) {
        for g in &mut self.guilds {
            g.channels.retain(|x| x.id != c.id);
        }
        self.dms.retain(|x| x.id != c.id);
        self.channel_guild.remove(&c.id);
        self.channel_names.remove(&c.id);
        self.unread.remove(&c.id);
        self.messages.remove(&c.id);
        if self.current.as_deref() == Some(c.id.as_str()) {
            self.current = None;
        }
    }

    pub fn upsert_role(&mut self, guild_id: &str, role: Role) {
        if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
            match g.roles.iter_mut().find(|r| r.id == role.id) {
                Some(slot) => *slot = role,
                None => g.roles.push(role),
            }
        }
    }

    pub fn delete_role(&mut self, guild_id: &str, role_id: &str) {
        if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
            g.roles.retain(|r| r.id != role_id);
        }
    }

    /// Someone's roles, nickname, server avatar or timeout changed.
    pub fn update_member(&mut self, u: GuildMemberUpdate) {
        if let Some(user) = &u.member.user {
            self.users.insert(user.id.clone(), user.clone());
        }
        self.remember_member(&u.guild_id, u.member);
    }

    /// We left a server, or it was deleted.
    pub fn remove_guild(&mut self, guild_id: &str) {
        self.guilds.retain(|g| g.id != guild_id);
        self.refresh_rail();
        let channels: Vec<String> = self
            .channel_guild
            .iter()
            .filter(|(_, g)| *g == guild_id)
            .map(|(c, _)| c.clone())
            .collect();
        for c in &channels {
            self.channel_guild.remove(c);
            self.channel_names.remove(c);
            self.unread.remove(c);
            self.messages.remove(c);
        }
        if self.current.as_ref().is_some_and(|c| channels.contains(c)) {
            self.current = None;
        }
        self.voice.retain(|_, (g, _)| g != guild_id);
        let voice = &self.voice;
        self.streaming.retain(|u| voice.contains_key(u));
        self.voice_status.retain(|u, _| voice.contains_key(u));
        self.members.retain(|(g, _), _| g != guild_id);
        self.member_lists.remove(guild_id);
    }

    /// Rows of a member list from the gateway: only for a server we know.
    pub fn apply_member_list(&mut self, u: MemberListUpdate) {
        let guild_id = u.guild_id.clone();
        if self.guild(&guild_id).is_none() {
            return;
        }
        let mut list = self
            .member_lists
            .remove(&guild_id)
            .unwrap_or_else(|| MemberList::new(&guild_id));
        let mut seen = Vec::new();
        list.apply(u, |user, member| seen.push((user, member)));
        for (user, member) in seen {
            self.users.insert(user.id.clone(), user);
            self.remember_member(&guild_id, member);
        }
        self.member_lists.insert(guild_id, list);
    }

    /// What we may do to `user_id` in a server, from our permissions and
    /// the role hierarchy (as Discord checks them): nothing on ourselves,
    /// on the owner, or without the data to decide.
    pub fn moderation(&self, guild_id: &str, user_id: &str) -> Moderation {
        let mut m = Moderation::default();
        let (Some(g), Some(me)) = (self.guild(guild_id), self.me_id()) else {
            return m;
        };
        if me == user_id || g.owner_id() == Some(user_id) {
            return m;
        }
        let Some(base) = self.base_of(g) else { return m };
        let bits = base.in_guild();
        let owner = g.owner_id() == Some(me);
        // Kick, ban and timeout only on someone below our highest role.
        let above = owner
            || match (
                self.top_role_position(guild_id, me),
                self.top_role_position(guild_id, user_id),
            ) {
                (Some(ours), Some(theirs)) => ours > theirs,
                _ => false,
            };
        m.kick = above && bits & permissions::KICK_MEMBERS != 0;
        m.ban = above && bits & permissions::BAN_MEMBERS != 0;
        m.timeout = above && bits & permissions::MODERATE_MEMBERS != 0;
        // Voice: only while they are in a voice channel of this server,
        // with the permission in that channel.
        if let Some((g_id, channel)) = self.voice.get(user_id) {
            if g_id == guild_id {
                let p = self.permissions(channel);
                m.in_voice = Some(channel.clone());
                m.mute = p & permissions::MUTE_MEMBERS != 0;
                m.deafen = p & permissions::DEAFEN_MEMBERS != 0;
                m.move_members = p & permissions::MOVE_MEMBERS != 0;
            }
        }
        m
    }

    /// Voice channels of a server someone can be moved to: those where we
    /// have `MOVE_MEMBERS` and `CONNECT`.
    pub fn move_targets(&self, guild_id: &str) -> Vec<(String, String)> {
        let Some(g) = self.guild(guild_id) else {
            return Vec::new();
        };
        g.channels
            .iter()
            .filter(|c| c.kind == crate::discord::models::kind::GUILD_VOICE)
            .filter(|c| self.can(&c.id, permissions::MOVE_MEMBERS | permissions::CONNECT))
            .map(|c| (c.id.clone(), c.name.clone().unwrap_or_default()))
            .collect()
    }

    /// Shows at once the effect of a moderation action that went through
    /// (Discord confirms it with its own events later).
    pub fn after_moderation(
        &mut self,
        guild_id: &str,
        user_id: &str,
        action: &crate::discord::rest::ModAction,
    ) {
        use crate::discord::rest::ModAction;
        match action {
            ModAction::Mute(on) => {
                if let Some(f) = self.voice_status.get_mut(user_id) {
                    f.mute = *on;
                }
            }
            ModAction::Deafen(on) => {
                if let Some(f) = self.voice_status.get_mut(user_id) {
                    f.deaf = *on;
                }
            }
            ModAction::Move(channel) => {
                if let Some((g, c)) = self.voice.get_mut(user_id) {
                    if g == guild_id {
                        c.clone_from(channel);
                    }
                }
            }
            ModAction::Disconnect => {
                if self.voice.get(user_id).is_some_and(|(g, _)| g == guild_id) {
                    self.voice.remove(user_id);
                    self.streaming.remove(user_id);
                    self.voice_status.remove(user_id);
                }
            }
            ModAction::Timeout(seconds) => {
                let key = (guild_id.to_string(), user_id.to_string());
                if let Some(m) = self.members.get_mut(&key) {
                    m.communication_disabled_until = seconds.map(|s| {
                        (chrono::Utc::now()
                            + chrono::Duration::seconds(s.min(crate::discord::rest::MAX_TIMEOUT_SECS) as i64))
                        .to_rfc3339()
                    });
                }
            }
            // The member list follows Discord's own update.
            ModAction::Kick | ModAction::Ban => {}
        }
    }

    /// A member list made up from what the demo knows (no network).
    pub fn demo_member_list(&mut self, guild_id: &str) {
        use crate::discord::member_list::{MemberList, Row, Status};
        let mut ids: Vec<String> = self
            .members
            .keys()
            .filter(|(g, _)| g == guild_id)
            .map(|(_, u)| u.clone())
            .collect();
        if let Some(me) = self.me_id() {
            if !ids.iter().any(|i| i == me) {
                ids.push(me.to_string());
            }
        }
        ids.sort();
        let statuses = [
            Status::Online,
            Status::Idle,
            Status::DoNotDisturb,
            Status::Offline,
        ];
        let mut online = Vec::new();
        let mut offline = Vec::new();
        for (i, id) in ids.into_iter().enumerate() {
            let status = statuses[i % statuses.len()];
            let row = Row::Member { user_id: id, status };
            if status == Status::Offline {
                offline.push(row);
            } else {
                online.push(row);
            }
        }
        let mut list = MemberList::new(guild_id);
        list.id = "everyone".into();
        list.member_count = (online.len() + offline.len()) as u64;
        list.online_count = online.len() as u64;
        list.rows.push(Row::Group {
            id: "online".into(),
            count: online.len() as u64,
        });
        list.rows.extend(online);
        if !offline.is_empty() {
            list.rows.push(Row::Group {
                id: "offline".into(),
                count: offline.len() as u64,
            });
            list.rows.extend(offline);
        }
        self.member_lists.insert(guild_id.to_string(), list);
    }

    /// Name of a group of the member list: a role, online or offline.
    pub fn group_name(&self, guild_id: &str, group_id: &str) -> String {
        match group_id {
            "online" => crate::i18n::t("Online", "Online").into(),
            "offline" => crate::i18n::t("Offline", "Offline").into(),
            id => self
                .guild(guild_id)
                .and_then(|g| g.roles.iter().find(|r| r.id == id))
                .map_or_else(|| crate::i18n::t("Members", "Membri").into(), |r| r.name.clone()),
        }
    }

    /// Colour of someone's name in a server: their highest coloured role.
    pub fn role_colour(&self, guild_id: &str, user_id: &str) -> Option<u32> {
        let g = self.guild(guild_id)?;
        let member = self.member(guild_id, user_id)?;
        g.roles
            .iter()
            .filter(|r| r.color != 0 && member.roles.contains(&r.id))
            .max_by_key(|r| r.position)
            .map(|r| r.color)
    }

    /// Position of someone's highest role (0: only @everyone); `None` if
    /// their roles are not known.
    pub fn top_role_position(&self, guild_id: &str, user_id: &str) -> Option<i64> {
        let g = self.guild(guild_id)?;
        let member = self.member(guild_id, user_id)?;
        Some(
            g.roles
                .iter()
                .filter(|r| member.roles.contains(&r.id))
                .map(|r| r.position)
                .max()
                .unwrap_or(0),
        )
    }

    /// Marks the start of a load; `false` if one is already running.
    pub fn begin_loading(&mut self, channel_id: &str) -> bool {
        self.loading.insert(channel_id.to_string())
    }

    /// ID of the oldest loaded message, if there is more history to download.
    pub fn older_cursor(&self, channel_id: &str) -> Option<String> {
        if self.exhausted.contains(channel_id) || self.loading.contains(channel_id) {
            return None;
        }
        self.messages
            .get(channel_id)
            .and_then(|v| v.first())
            .map(|m| m.id.clone())
    }

    /// Applies a page of history (it comes from newest to oldest).
    pub fn apply_history(&mut self, channel_id: &str, older: bool, mut batch: Vec<Message>) {
        self.loading.remove(channel_id);
        if batch.len() < HISTORY_PAGE {
            self.exhausted.insert(channel_id.to_string());
        }
        for m in &batch {
            self.remember_users(m);
        }
        batch.reverse();
        let list = self.messages.entry(channel_id.to_string()).or_default();
        if older {
            // Looked up in a set: the list grows with every older page.
            let known: HashSet<&str> = list.iter().map(|x| x.id.as_str()).collect();
            batch.retain(|m| !known.contains(m.id.as_str()));
            batch.append(list);
        } else {
            // Merges any message that came from the gateway during the load.
            // (A few at most: a linear search is enough.)
            for m in list.drain(..) {
                if !batch.iter().any(|x| x.id == m.id) {
                    batch.push(m);
                }
            }
            batch.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
        *list = batch;
    }

    pub fn history_failed(&mut self, channel_id: &str) {
        self.loading.remove(channel_id);
    }

    /// Inserts a message (from the gateway or the answer to a send), without duplicates.
    /// Returns `true` if it went into the open channel.
    pub fn insert_message(&mut self, m: Message) -> bool {
        self.remember_users(&m);
        let from_me = self.me_id() == Some(m.author.id.as_str());
        let in_current = self.current.as_deref() == Some(m.channel_id.as_str());
        if !in_current && !from_me {
            self.unread.insert(m.channel_id.clone());
        }
        if let Some(list) = self.typing.get_mut(&m.channel_id) {
            list.retain(|(uid, _, _)| uid != &m.author.id);
        }
        self.bump_dm(&m.channel_id, &m.id);

        // If the channel was never opened nothing is kept: when it is opened
        // the whole history is downloaded anyway.
        let Some(list) = self.messages.get_mut(&m.channel_id) else {
            return false;
        };
        if list.iter().rev().take(100).any(|x| x.id == m.id) {
            return false;
        }
        let out_of_order = list.last().is_some_and(|l| cmp_snowflake(&l.id, &m.id).is_gt());
        list.push(m);
        if out_of_order {
            list.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
        in_current
    }

    pub fn update_message(&mut self, u: MessageUpdate) {
        let Some(m) = self.find_message_mut(&u.channel_id, &u.id) else {
            return;
        };
        if let Some(c) = u.content {
            m.content = c;
        }
        if u.edited_timestamp.is_some() {
            m.edited_timestamp = u.edited_timestamp;
        }
        if let Some(a) = u.attachments {
            m.attachments = a;
        }
        if let Some(e) = u.embeds {
            m.embeds = e;
        }
    }

    pub fn delete_message(&mut self, d: &MessageDelete) {
        if let Some(list) = self.messages.get_mut(&d.channel_id) {
            list.retain(|m| m.id != d.id);
        }
    }

    fn find_message_mut(&mut self, channel_id: &str, message_id: &str) -> Option<&mut Message> {
        self.messages
            .get_mut(channel_id)
            .and_then(|l| l.iter_mut().rev().find(|m| m.id == message_id))
    }

    pub fn reaction_add(&mut self, ev: &ReactionEvent) {
        let mine = self.me_id() == Some(ev.user_id.as_str());
        let Some(m) = self.find_message_mut(&ev.channel_id, &ev.message_id) else {
            return;
        };
        match m.reactions.iter_mut().find(|r| r.emoji.same(&ev.emoji)) {
            Some(r) => {
                r.count += 1;
                r.me |= mine;
            }
            None => m.reactions.push(Reaction {
                count: 1,
                me: mine,
                emoji: ev.emoji.clone(),
            }),
        }
    }

    pub fn reaction_remove(&mut self, ev: &ReactionEvent) {
        let mine = self.me_id() == Some(ev.user_id.as_str());
        let Some(m) = self.find_message_mut(&ev.channel_id, &ev.message_id) else {
            return;
        };
        if let Some(r) = m.reactions.iter_mut().find(|r| r.emoji.same(&ev.emoji)) {
            r.count = r.count.saturating_sub(1);
            if mine {
                r.me = false;
            }
        }
        m.reactions.retain(|r| r.count > 0);
    }

    pub fn typing_start(&mut self, t: TypingStart) {
        if self.me_id() == Some(t.user_id.as_str()) {
            return;
        }
        let name = t
            .member
            .as_ref()
            .and_then(|m| {
                m.nick
                    .clone()
                    .or_else(|| m.user.as_ref().map(|u| u.display_name().to_string()))
            })
            .or_else(|| self.users.get(&t.user_id).map(|u| u.display_name().to_string()))
            .unwrap_or_else(|| crate::i18n::t("Someone", "Qualcuno").to_string());
        let list = self.typing.entry(t.channel_id).or_default();
        list.retain(|(uid, _, _)| uid != &t.user_id);
        list.push((t.user_id, name, Instant::now()));
    }

    /// Names of who is typing in the channel (they expire after `TYPING_TIMEOUT`).
    pub fn typing_in(&self, channel_id: &str) -> Vec<&str> {
        self.typing
            .get(channel_id)
            .map(|l| {
                l.iter()
                    .filter(|(_, _, at)| at.elapsed() < TYPING_TIMEOUT)
                    .map(|(_, name, _)| name.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_typing(&self) -> bool {
        self.typing
            .values()
            .any(|l| l.iter().any(|(_, _, at)| at.elapsed() < TYPING_TIMEOUT))
    }

    /// Simulates "is typing…" (used by the demo).
    pub fn fake_typing(&mut self, channel_id: &str, user_id: &str, name: &str) {
        self.typing.entry(channel_id.to_string()).or_default().push((
            user_id.to_string(),
            name.to_string(),
            Instant::now(),
        ));
    }

    /// Moves to the top the private conversation that just received a message.
    fn bump_dm(&mut self, channel_id: &str, message_id: &str) {
        let Some(pos) = self.dms.iter().position(|c| c.id == channel_id) else {
            return;
        };
        self.dms[pos].last_message_id = Some(message_id.to_string());
        if pos > 0 {
            let c = self.dms.remove(pos);
            self.dms.insert(0, c);
        }
    }

    // ------------------------------------------------------------ views on the data

    /// Rows of the channel column: `None` = direct messages, otherwise the server index.
    pub fn channel_rows(&self, guild: Option<usize>) -> Vec<ChannelRow> {
        match guild.and_then(|i| self.guilds.get(i)) {
            None => self
                .dms
                .iter()
                .map(|c| ChannelRow::Channel {
                    id: c.id.clone(),
                    name: self.dm_name(c),
                    kind: c.kind,
                    selectable: true,
                })
                .collect(),
            Some(g) => {
                // Channels we cannot see are not shown (as on Discord). The
                // base permissions are computed once for the whole server.
                let base = self.base_of(g);
                guild_rows(&g.channels, |c| {
                    base.as_ref().is_some_and(|b| {
                        b.in_channel(&c.permission_overwrites) & permissions::VIEW_CHANNEL != 0
                    })
                })
            }
        }
    }

    pub fn dm_name(&self, c: &Channel) -> String {
        if let Some(name) = c.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        let names: Vec<&str> = if c.recipients.is_empty() {
            c.recipient_ids
                .iter()
                .filter_map(|id| self.users.get(id))
                .map(User::display_name)
                .collect()
        } else {
            c.recipients.iter().map(User::display_name).collect()
        };
        if names.is_empty() {
            crate::i18n::t("Conversation", "Conversazione").into()
        } else {
            names.join(", ")
        }
    }

    /// Picture of a private conversation: the ID that gives the initials'
    /// colour, and the address of the picture (`None` = initials). One to
    /// one: the other person's avatar; group: the group's picture.
    pub fn dm_picture(&self, c: &Channel, size: u32) -> (String, Option<String>) {
        if c.kind == kind::GROUP_DM {
            return (
                c.id.clone(),
                crate::media::channel_icon_url(&c.id, c.icon.as_deref(), size),
            );
        }
        let user = c
            .recipients
            .first()
            .map(|u| u.id.as_str())
            .or_else(|| c.recipient_ids.first().map(String::as_str));
        match user {
            Some(u) => (u.to_string(), self.avatar_url(None, u, size)),
            None => (c.id.clone(), None),
        }
    }

    pub fn is_dm(&self, channel_id: &str) -> bool {
        self.dms.iter().any(|c| c.id == channel_id)
    }

    /// Short channel name: `#name` or `@person`.
    pub fn channel_label(&self, channel_id: &str) -> String {
        if let Some(dm) = self.dms.iter().find(|c| c.id == channel_id) {
            return format!("@{}", self.dm_name(dm));
        }
        let name = self
            .channel_names
            .get(channel_id)
            .map_or_else(|| crate::i18n::t("channel", "canale"), String::as_str);
        format!("#{name}")
    }

    pub fn guild_emojis(&mut self, u: GuildEmojisUpdate) {
        if let Some(g) = self.guilds.iter_mut().find(|g| g.id == u.guild_id) {
            g.emojis = u.emojis;
        }
    }

    /// Server emojis usable in the channel: only those of the server it
    /// belongs to (none in direct messages), available, without role
    /// limits, and animated ones only with Nitro.
    pub fn usable_emojis(&self, channel_id: &str) -> Vec<&GuildEmoji> {
        let nitro = self.me.as_ref().and_then(|u| u.premium_type).unwrap_or(0) > 0;
        self.guild_of(channel_id)
            .map(|g| {
                g.emojis
                    .iter()
                    .filter(|e| e.available && e.roles.is_empty() && (nitro || !e.animated))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Text to send: `:name:` becomes the emoji of the channel's server.
    pub fn prepare_outgoing(&self, channel_id: &str, content: &str) -> String {
        let usable: Vec<GuildEmoji> = self.usable_emojis(channel_id).into_iter().cloned().collect();
        crate::text::expand_custom_emojis(content, &usable)
    }

    pub fn guild_of(&self, channel_id: &str) -> Option<&Guild> {
        self.channel_guild
            .get(channel_id)
            .and_then(|gid| self.guilds.iter().find(|g| &g.id == gid))
    }

    /// Channel title: `#name · server` or `@person`.
    pub fn title(&self, channel_id: &str) -> String {
        let label = self.channel_label(channel_id);
        match self.guild_of(channel_id) {
            Some(g) => format!("{label} · {}", g.name()),
            None => label,
        }
    }

    pub fn guild_has_unread(&self, guild_id: &str) -> bool {
        self.unread
            .iter()
            .any(|c| self.channel_guild.get(c).is_some_and(|g| g == guild_id))
    }

    pub fn dms_have_unread(&self) -> bool {
        self.dms.iter().any(|c| self.unread.contains(&c.id))
    }
}

pub fn first_selectable(rows: &[ChannelRow]) -> usize {
    rows.iter().position(ChannelRow::is_selectable).unwrap_or(0)
}

/// Sorts channels like Discord.
///
/// First those without a category, then each category with its channels
/// (text before voice, then by position). `can_view` leaves out the channels we cannot see; a category without
/// visible channels is not shown.
pub fn guild_rows(channels: &[Channel], can_view: impl Fn(&Channel) -> bool) -> Vec<ChannelRow> {
    let visible = |c: &&Channel| (c.is_text() || c.is_voice()) && can_view(c);
    let order = |a: &&Channel, b: &&Channel| {
        (a.is_voice(), a.position.unwrap_or(0))
            .cmp(&(b.is_voice(), b.position.unwrap_or(0)))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    };
    let row = |c: &Channel| ChannelRow::Channel {
        id: c.id.clone(),
        name: c.name.clone().unwrap_or_default(),
        kind: c.kind,
        selectable: c.is_text(),
    };

    let mut rows = Vec::new();
    let mut top: Vec<&Channel> = channels
        .iter()
        .filter(|c| c.parent_id.is_none())
        .filter(visible)
        .collect();
    top.sort_by(order);
    rows.extend(top.into_iter().map(row));

    let mut categories: Vec<&Channel> = channels.iter().filter(|c| c.kind == kind::CATEGORY).collect();
    categories.sort_by(|a, b| {
        a.position
            .unwrap_or(0)
            .cmp(&b.position.unwrap_or(0))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    });
    for cat in categories {
        let mut children: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(cat.id.as_str()))
            .filter(visible)
            .collect();
        if children.is_empty() {
            continue;
        }
        children.sort_by(order);
        rows.push(ChannelRow::Category(cat.name.clone().unwrap_or_default()));
        rows.extend(children.into_iter().map(row));
    }
    rows
}

/// Microphone and sound of someone in voice: off by themselves (`self_…`)
/// or by a moderator of the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceFlags {
    pub self_mute: bool,
    pub self_deaf: bool,
    pub mute: bool,
    pub deaf: bool,
}

impl VoiceFlags {
    pub fn muted(&self) -> bool {
        self.self_mute || self.mute
    }

    pub fn deafened(&self) -> bool {
        self.self_deaf || self.deaf
    }
}

/// What we may do to someone in a server (see [`Store::moderation`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Moderation {
    pub kick: bool,
    pub ban: bool,
    pub timeout: bool,
    /// The voice channel they are in (in this server).
    pub in_voice: Option<String>,
    pub mute: bool,
    pub deafen: bool,
    /// Move to another voice channel, or disconnect.
    pub move_members: bool,
}

impl Moderation {
    pub fn any(&self) -> bool {
        self.kick || self.ban || self.timeout || self.mute || self.deafen || self.move_members
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::models::Emoji;

    #[test]
    fn message_actions_follow_the_permissions() {
        let mut s = Store::demo();
        let crab = Emoji {
            id: None,
            name: Some("🦀".into()),
            animated: false,
        };
        let new_one = Emoji {
            id: None,
            name: Some("🎉".into()),
            animated: false,
        };
        // Our message in #general: edit and delete; Giulia's: neither.
        assert!(s.can_edit("1001", "60007"));
        assert!(s.can_delete("1001", "60007"));
        assert!(!s.can_edit("1001", "60008"));
        assert!(!s.can_delete("1001", "60008"));
        assert!(s.can_reply("1001"));
        assert!(s.can_attach("1001"));
        assert!(s.can_react("1001", "60008", &crab));
        assert!(s.can_react("1001", "60008", &new_one));

        // Without ADD_REACTIONS and ATTACH_FILES: only the existing reactions.
        let deny: crate::permissions::Overwrite = serde_json::from_value(serde_json::json!(
            {"id": "100", "type": 0, "allow": "0", "deny": (permissions::ADD_REACTIONS | permissions::ATTACH_FILES).to_string()}
        ))
        .unwrap();
        let g = s.guilds.iter_mut().find(|g| g.id == "100").unwrap();
        g.channels
            .iter_mut()
            .find(|c| c.id == "1001")
            .unwrap()
            .permission_overwrites
            .push(deny);
        assert!(s.can_react("1001", "60008", &crab));
        assert!(!s.can_react("1001", "60008", &new_one));
        assert!(!s.can_attach("1001"));
        assert!(s.can_reply("1001"));

        // Announcements: no SEND_MESSAGES, so no reply.
        assert!(!s.can_reply("1004"));
        // Unknown channels and messages: nothing allowed.
        assert!(!s.can_reply("999999"));
        assert!(!s.can_attach("999999"));
        assert!(!s.can_edit("1001", "424242"));
        assert!(!s.can_delete("1001", "424242"));
        // Without knowing who we are, no message is ours.
        s.me = None;
        assert!(!s.can_edit("1001", "60007"));
        assert!(!s.can_delete("1001", "60007"));
    }

    #[test]
    fn local_reactions_toggle_ours() {
        let mut s = Store::demo();
        let crab = Emoji {
            id: None,
            name: Some("🦀".into()),
            animated: false,
        };
        let count = |s: &Store| {
            s.find_message("1001", "60008")
                .and_then(|m| m.reactions.iter().find(|r| r.emoji.same(&crab)))
                .map(|r| (r.count, r.me))
        };
        assert_eq!(count(&s), Some((2, true)));
        s.local_reaction("1001", "60008", &crab, false);
        assert_eq!(count(&s), Some((1, false)));
        s.local_reaction("1001", "60008", &crab, true);
        assert_eq!(count(&s), Some((2, true)));
    }

    #[test]
    fn who_is_in_the_voice_channels() {
        let mut store = Store::demo();
        let names: Vec<String> = store.voice_members("1021").into_iter().map(|(_, n)| n).collect();
        // Marco has a nickname in the demo server.
        assert_eq!(names, ["Giulia", "Marco (ops)"]);
        let state = |v: serde_json::Value| -> VoiceState { serde_json::from_value(v).unwrap() };
        // Giulia leaves, someone we did not know arrives.
        store.apply_voice_state(
            &state(serde_json::json!({"guild_id": "100", "channel_id": null, "user_id": "901"})),
            None,
        );
        store.apply_voice_state(
            &state(
                serde_json::json!({"guild_id": "100", "channel_id": "1021", "user_id": "950",
                "member": {"user": {"id": "950", "username": "nuovo"}}}),
            ),
            None,
        );
        let names: Vec<String> = store.voice_members("1021").into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Marco (ops)", "nuovo"]);
        assert!(store.voice_members("1003").is_empty());
    }

    #[test]
    fn server_profiles_from_messages_and_profiles() {
        let mut s = Store::demo();
        let h = "0123456789abcdef0123456789abcdef";
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "9001", "channel_id": "1001", "guild_id": "100", "content": "hi",
            "author": {"id": "901", "username": "giulia", "global_name": "Giulia", "avatar": h},
            "member": {"nick": "Giuly", "avatar": null, "roles": ["5"]}
        }))
        .unwrap();
        s.remember_users(&m);
        let giulia = s.users["901"].clone();
        assert_eq!(s.name_in(Some("100"), &giulia), "Giuly");
        assert_eq!(s.name_in(None, &giulia), "Giulia");
        // No server avatar: the account one.
        assert!(s
            .avatar_url(Some("100"), "901", 64)
            .unwrap()
            .contains("/avatars/901/"));

        let p: UserProfile = serde_json::from_value(serde_json::json!({
            "user": {"id": "901", "username": "giulia", "avatar": h, "banner": h},
            "user_profile": {"bio": "ciao", "pronouns": "she/her"},
            "guild_member": {"nick": "G", "avatar": h, "roles": []}
        }))
        .unwrap();
        s.apply_profile(Some("100"), &p);
        assert!(s
            .avatar_url(Some("100"), "901", 64)
            .unwrap()
            .contains("/guilds/100/users/901/avatars/"));
        assert_eq!(s.name_in(Some("100"), &s.users["901"].clone()), "G");
        // The banner stays in the profile, not in the shared user data.
        assert_eq!(s.users["901"].banner, None);
        // Someone without an avatar: initials.
        assert_eq!(s.avatar_url(None, "unknown", 64), None);
    }

    #[test]
    fn channels_sorted_by_category() {
        let channels: Vec<Channel> = serde_json::from_value(serde_json::json!([
            {"id": "5", "type": 2, "name": "Voce", "position": 0, "parent_id": "1"},
            {"id": "4", "type": 0, "name": "off-topic", "position": 1, "parent_id": "1"},
            {"id": "3", "type": 0, "name": "generale", "position": 0, "parent_id": "1"},
            {"id": "1", "type": 4, "name": "Testo", "position": 0},
            {"id": "2", "type": 0, "name": "regole", "position": 0},
            {"id": "9", "type": 4, "name": "Vuota", "position": 1},
        ]))
        .unwrap();
        let rows = guild_rows(&channels, |_| true);
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ChannelRow::Category(n) => format!("[{n}]"),
                ChannelRow::Channel { name, .. } => name.clone(),
            })
            .collect();
        assert_eq!(names, ["regole", "[Testo]", "generale", "off-topic", "Voce"]);
        assert_eq!(first_selectable(&rows), 0);
        assert!(!rows[4].is_selectable());
    }

    #[test]
    fn channels_follow_the_permissions() {
        let mut s = Store::demo();
        let names = |s: &Store| -> Vec<String> {
            s.channel_rows(Some(0))
                .iter()
                .filter_map(|r| match r {
                    ChannelRow::Channel { name, .. } => Some(name.clone()),
                    ChannelRow::Category(_) => None,
                })
                .collect()
        };
        // The moderators' channel is hidden, the others are there.
        let shown = names(&s);
        assert!(!shown.iter().any(|n| n == "mod-room"), "{shown:?}");
        assert!(shown.iter().any(|n| n == "announcements"));
        assert!(shown.iter().any(|n| n == "Meeting room"));
        // Read-only announcements; a voice channel we cannot join.
        assert!(s.can_send("1001"));
        assert!(!s.can_send("1004"));
        assert!(s.can_connect("1021"));
        assert!(!s.can_connect("1023"));
        // Private conversations: Discord decides.
        assert!(s.can_send("5001"));
        assert!(s.can_send("2001"));
        // A server without data on the roles (not loaded, or broken): no
        // permission and no channel, never everything.
        let roles = std::mem::take(&mut s.guilds[1].roles);
        assert!(!s.can_send("2001"));
        assert!(!s.can(
            "2001",
            permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY
        ));
        assert!(s.channel_rows(Some(1)).is_empty());
        s.guilds[1].roles = roles;
        assert!(s.can_send("2001"));

        // Discord gives us the moderator role: the channel appears.
        s.update_member(
            serde_json::from_value(serde_json::json!({
                "guild_id": "100", "user": {"id": "900", "username": "tommaso"}, "roles": ["7"]
            }))
            .unwrap(),
        );
        assert!(names(&s).iter().any(|n| n == "mod-room"));
        // Then the role loses the permission: hidden again, and closed if open.
        let _ = s.open("1005");
        assert_eq!(s.current.as_deref(), Some("1005"));
        let mut channel = s.guilds[0]
            .channels
            .iter()
            .find(|c| c.id == "1005")
            .unwrap()
            .clone();
        channel.permission_overwrites.retain(|o| o.id != "7");
        s.upsert_channel(channel);
        assert!(!names(&s).iter().any(|n| n == "mod-room"));
        assert!(s.close_if_hidden());
        assert!(s.current.is_none());

        // @everyone loses the permission to send everywhere.
        let mut everyone = s.guilds[0].roles.iter().find(|r| r.id == "100").unwrap().clone();
        everyone.permissions &= !permissions::SEND_MESSAGES;
        s.upsert_role("100", everyone);
        assert!(!s.can_send("1001"));

        // A timeout leaves only reading.
        let until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        s.update_member(
            serde_json::from_value(serde_json::json!({
                "guild_id": "100", "user": {"id": "900", "username": "tommaso"}, "roles": [],
                "communication_disabled_until": until
            }))
            .unwrap(),
        );
        assert!(s.can(
            "1000",
            permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY
        ));
        assert!(!s.can_connect("1021"));

        // Leaving the server takes away its channels.
        s.open("1000");
        s.remove_guild("100");
        assert!(s.current.is_none());
        assert!(s.guild_of("1000").is_none());
    }

    #[test]
    fn server_column_follows_servers_and_folders() {
        let mut s = Store::demo();
        // Rust Community alone, then the "Hobby" folder with the other two.
        assert_eq!(s.rail().len(), 2);
        // A server we left disappears from the column (and from its folder).
        s.remove_guild("300");
        let RailEntry::Folder { guilds, .. } = &s.rail()[1] else {
            panic!("no folder")
        };
        assert_eq!(guilds.as_slice(), &[1][..]);
        // A new server (not yet in the folders) goes on top.
        let g: Guild = serde_json::from_value(serde_json::json!({"id": "400", "name": "New"})).unwrap();
        s.upsert_guild(g.clone());
        assert_eq!(s.rail()[0], RailEntry::Guild(2));
        // The same server again changes nothing.
        let before = s.rail().to_vec();
        s.upsert_guild(g);
        assert_eq!(s.rail(), before.as_slice());
        // Folders from another device replace the column.
        let mut folders = GuildFolders::default();
        folders.normalize(&s.guild_ids());
        s.set_folders(folders);
        assert_eq!(s.rail().len(), 3);
        assert!(s.rail().iter().all(|e| matches!(e, RailEntry::Guild(_))));
    }

    #[test]
    fn moderation_follows_permissions_and_voice() {
        let mut s = Store::demo();
        // Our server (200): we own it.
        let m = s.moderation("200", "902");
        assert!(m.kick && m.ban && m.timeout);
        assert!(m.in_voice.is_none() && !m.mute);
        // In voice there: the voice actions too.
        s.voice.insert("902".into(), ("200".into(), "2001".into()));
        let m = s.moderation("200", "902");
        assert_eq!(m.in_voice.as_deref(), Some("2001"));
        assert!(m.mute && m.deafen && m.move_members);
        s.voice_status.insert("902".into(), VoiceFlags::default());
        s.after_moderation("200", "902", &crate::discord::rest::ModAction::Mute(true));
        assert!(s.voice_status["902"].mute && s.voice_status["902"].muted());
        s.after_moderation("200", "902", &crate::discord::rest::ModAction::Disconnect);
        assert!(!s.voice.contains_key("902"));
        // Server 100: no moderation permission for us.
        assert_eq!(s.moderation("100", "902"), Moderation::default());
        // Without the roles of the server: nothing.
        let roles = std::mem::take(&mut s.guilds[1].roles);
        assert!(!s.moderation("200", "902").any());
        s.guilds[1].roles = roles;
    }

    #[test]
    fn who_is_streaming() {
        let mut s = Store::demo();
        let state = |user: &str, channel: Option<&str>, stream: bool| -> VoiceState {
            serde_json::from_value(serde_json::json!({
                "guild_id": "100", "channel_id": channel, "user_id": user, "self_stream": stream
            }))
            .unwrap()
        };
        s.apply_voice_state(&state("902", Some("1021"), true), None);
        assert_eq!(s.stream_key("902").as_deref(), Some("guild:100:1021:902"));
        // Stops streaming, then leaves: no stream anymore.
        s.apply_voice_state(&state("902", Some("1021"), false), None);
        assert!(s.stream_key("902").is_none());
        s.apply_voice_state(&state("902", Some("1021"), true), None);
        s.apply_voice_state(&state("902", None, false), None);
        assert!(!s.streaming.contains("902"));
    }

    #[test]
    fn no_history_without_permission() {
        let mut s = Store::demo();
        s.messages.remove("1004");
        s.exhausted.remove("1004");
        let mut ch = s.guilds[0]
            .channels
            .iter()
            .find(|c| c.id == "1004")
            .unwrap()
            .clone();
        ch.permission_overwrites[0].deny |= permissions::READ_MESSAGE_HISTORY;
        s.upsert_channel(ch);
        assert!(!s.open("1004"), "the history must not be asked");
        assert!(s.older_cursor("1004").is_none());
    }

    #[test]
    fn messages_without_duplicates_and_unread() {
        let mut store = Store::demo();
        let channel = crate::demo::UNREAD_CHANNEL.to_string();
        let before = store.messages[&channel].len();
        let msg = crate::demo::local_message(&channel, None, "ciao", store.messages.get(&channel));
        assert!(!store.insert_message(msg.clone()));
        store.insert_message(msg);
        assert_eq!(store.messages[&channel].len(), before + 1);
        assert!(store.unread.contains(&channel));
        assert!(!store.open(&channel));
        assert!(!store.unread.contains(&channel));
    }

    #[test]
    fn reactions_from_the_gateway() {
        let mut store = Store::demo();
        let channel = crate::demo::UNREAD_CHANNEL.to_string();
        let id = store.messages[&channel][0].id.clone();
        let ev = |user: &str| ReactionEvent {
            user_id: user.into(),
            channel_id: channel.clone(),
            message_id: id.clone(),
            emoji: Emoji {
                id: None,
                name: Some("👍".into()),
                animated: false,
            },
        };
        store.reaction_add(&ev("901"));
        store.reaction_add(&ev("900"));
        let r = &store.messages[&channel][0].reactions[0];
        assert_eq!((r.count, r.me), (2, true));
        store.reaction_remove(&ev("900"));
        store.reaction_remove(&ev("901"));
        assert!(store.messages[&channel][0].reactions.is_empty());
    }

    #[test]
    fn dm_moves_to_the_top() {
        let mut store = Store::demo();
        let last = store.dms.last().unwrap().id.clone();
        let msg = crate::demo::local_message(&last, None, "ehi", None);
        store.insert_message(msg);
        assert_eq!(store.dms[0].id, last);
    }
}

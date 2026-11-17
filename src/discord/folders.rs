//! Server folders, as Discord keeps them in the account settings.
//!
//! The settings are a protobuf (`PreloadedUserSettings`, base64 over JSON):
//! read with `GET /users/@me/settings-proto/1`, kept in sync by the gateway
//! event `USER_SETTINGS_PROTO_UPDATE`, changed with `PATCH` on the same path.
//! Only two fields are used here:
//!
//! - 1 `versions` → 3 `data_version` (sent back as `required_data_version`,
//!   so a change made meanwhile on another device is not overwritten);
//! - 11 `status` → `StatusSettings { 1 status (StringValue), 2
//!   custom_status, 3 show_current_game, 4 status_expires_at_ms, 5
//!   status_created_at_ms }`, `CustomStatus { 1 text, 2 emoji_id (fixed64),
//!   3 emoji_name, 4 expires_at_ms (fixed64), 5 created_at_ms (fixed64) }`;
//! - 14 `guild_folders` → `GuildFolders { 1 folders, 2 guild_positions }`,
//!   `GuildFolder { 1 guild_ids (fixed64), 2 id (Int64Value), 3 name
//!   (StringValue), 4 color (UInt64Value) }`.
//!
//! A PATCH replaces the whole top-level field it carries: so an update holds
//! all of field 14 (or 11) and nothing else. Fields that Rico does not know
//! are kept as they are and sent back unchanged.
//!
//! Only the minimal protobuf needed is implemented, with limits: this is
//! data from the network.

use anyhow::{bail, Context, Result};
use base64::Engine;

/// Settings blob larger than this is refused (Discord's limit is 5 MB of base64).
const MAX_PROTO: usize = 4 * 1024 * 1024;
/// Most folders and servers per folder accepted.
const MAX_FOLDERS: usize = 1000;
const MAX_GUILDS_PER_FOLDER: usize = 1000;
/// Longest folder name kept (characters).
pub const MAX_NAME: usize = 64;

/// One entry of the server list: a named folder, or (`id` = `None`) a single
/// server outside any folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Folder {
    pub id: Option<i64>,
    pub name: Option<String>,
    /// 0xRRGGBB.
    pub color: Option<u64>,
    pub guild_ids: Vec<String>,
    /// Fields of the folder not known here, kept as received.
    unknown: Vec<u8>,
}

impl Folder {
    /// A real folder (not a single server shown as an entry).
    pub fn is_folder(&self) -> bool {
        self.id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildFolders {
    pub folders: Vec<Folder>,
    /// Deprecated by Discord, kept and sent back as received.
    guild_positions: Vec<u64>,
}

/// What Rico takes from the account settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsSnapshot {
    pub data_version: Option<u32>,
    /// `None` if the settings did not carry the folders (a partial update).
    pub folders: Option<GuildFolders>,
    /// `None` if the settings did not carry the status.
    pub status: Option<StatusSettings>,
}

/// Decodes base64 `PreloadedUserSettings`.
pub fn decode_settings(b64: &str) -> Result<SettingsSnapshot> {
    if b64.len() > MAX_PROTO {
        bail!("settings too large");
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("settings: invalid base64")?;
    let mut snap = SettingsSnapshot::default();
    let mut r = Reader::new(&bytes);
    while let Some((field, value)) = r.next()? {
        match (field, value) {
            (1, Value::Bytes(versions)) => {
                let mut v = Reader::new(versions);
                while let Some((f, val)) = v.next()? {
                    if let (3, Value::Varint(n)) = (f, val) {
                        snap.data_version = u32::try_from(n).ok();
                    }
                }
            }
            (11, Value::Bytes(status)) => snap.status = Some(decode_status(status)?),
            (14, Value::Bytes(folders)) => snap.folders = Some(decode_folders(folders)?),
            _ => {}
        }
    }
    Ok(snap)
}

fn decode_folders(bytes: &[u8]) -> Result<GuildFolders> {
    let mut out = GuildFolders::default();
    let mut r = Reader::new(bytes);
    while let Some((field, value)) = r.next()? {
        match (field, value) {
            (1, Value::Bytes(folder)) => {
                if out.folders.len() >= MAX_FOLDERS {
                    bail!("settings: too many folders");
                }
                out.folders.push(decode_folder(folder)?);
            }
            (2, Value::Fixed64(p)) => out.guild_positions.push(p),
            (2, Value::Bytes(packed)) => {
                for chunk in packed.as_chunks::<8>().0 {
                    out.guild_positions.push(u64::from_le_bytes(*chunk));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn decode_folder(bytes: &[u8]) -> Result<Folder> {
    let mut f = Folder::default();
    let mut r = Reader::new(bytes);
    loop {
        let start = r.pos;
        let Some((field, value)) = r.next()? else { break };
        let push_id = |id: u64, f: &mut Folder| -> Result<()> {
            if f.guild_ids.len() >= MAX_GUILDS_PER_FOLDER {
                bail!("settings: too many servers in a folder");
            }
            f.guild_ids.push(id.to_string());
            Ok(())
        };
        match (field, value) {
            (1, Value::Fixed64(id)) => push_id(id, &mut f)?,
            // Packed repeated fixed64.
            (1, Value::Bytes(packed)) => {
                if packed.len() % 8 != 0 {
                    bail!("settings: broken server list");
                }
                for chunk in packed.as_chunks::<8>().0 {
                    push_id(u64::from_le_bytes(*chunk), &mut f)?;
                }
            }
            (2, Value::Bytes(w)) => f.id = wrapper_varint(w)?.map(|v| v as i64),
            (3, Value::Bytes(w)) => {
                f.name = wrapper_string(w)?.map(|s| s.chars().take(MAX_NAME).collect());
            }
            (4, Value::Bytes(w)) => f.color = wrapper_varint(w)?,
            _ => f.unknown.extend_from_slice(&bytes[start..r.pos]),
        }
    }
    Ok(f)
}

fn wrapper_varint(bytes: &[u8]) -> Result<Option<u64>> {
    let mut r = Reader::new(bytes);
    let mut out = Some(0);
    while let Some((field, value)) = r.next()? {
        if let (1, Value::Varint(v)) = (field, value) {
            out = Some(v);
        }
    }
    Ok(out)
}

fn wrapper_string(bytes: &[u8]) -> Result<Option<String>> {
    let mut r = Reader::new(bytes);
    let mut out = Some(String::new());
    while let Some((field, value)) = r.next()? {
        if let (1, Value::Bytes(s)) = (field, value) {
            out = Some(String::from_utf8_lossy(s).into_owned());
        }
    }
    Ok(out)
}

/// Base64 `PreloadedUserSettings` holding only the folders (field 14): the
/// body of the PATCH.
pub fn encode_folders_update(folders: &GuildFolders) -> String {
    let mut body = Vec::new();
    for f in &folders.folders {
        let mut folder = Vec::new();
        if !f.guild_ids.is_empty() {
            let mut packed = Vec::with_capacity(f.guild_ids.len() * 8);
            for id in f.guild_ids.iter().filter_map(|id| id.parse::<u64>().ok()) {
                packed.extend_from_slice(&id.to_le_bytes());
            }
            put_bytes(&mut folder, 1, &packed);
        }
        if let Some(id) = f.id {
            let mut w = Vec::new();
            put_varint_field(&mut w, 1, id as u64);
            put_bytes(&mut folder, 2, &w);
        }
        if let Some(name) = &f.name {
            let mut w = Vec::new();
            put_bytes(&mut w, 1, name.as_bytes());
            put_bytes(&mut folder, 3, &w);
        }
        if let Some(color) = f.color {
            let mut w = Vec::new();
            put_varint_field(&mut w, 1, color);
            put_bytes(&mut folder, 4, &w);
        }
        folder.extend_from_slice(&f.unknown);
        put_bytes(&mut body, 1, &folder);
    }
    if !folders.guild_positions.is_empty() {
        let mut packed = Vec::new();
        for p in &folders.guild_positions {
            packed.extend_from_slice(&p.to_le_bytes());
        }
        put_bytes(&mut body, 2, &packed);
    }
    let mut settings = Vec::new();
    put_bytes(&mut settings, 14, &body);
    base64::engine::general_purpose::STANDARD.encode(settings)
}

// --- Status ------------------------------------------------------------------

/// Longest custom status kept or sent (Discord's limit).
pub const MAX_CUSTOM_STATUS: usize = 128;

/// Our status, as the other people see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

impl Presence {
    pub const ALL: [Self; 4] = [Self::Online, Self::Idle, Self::DoNotDisturb, Self::Invisible];

    /// The name Discord uses, in the settings and on the gateway.
    pub fn code(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::DoNotDisturb => "dnd",
            Self::Invisible => "invisible",
        }
    }

    /// Unknown names (and none) are "online", as Discord shows them.
    pub fn from_code(code: &str) -> Self {
        match code {
            "idle" => Self::Idle,
            "dnd" => Self::DoNotDisturb,
            "invisible" => Self::Invisible,
            _ => Self::Online,
        }
    }

    pub fn label(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Self::Online => t("Online", "Online"),
            Self::Idle => t("Idle", "Inattivo"),
            Self::DoNotDisturb => t("Do not disturb", "Non disturbare"),
            Self::Invisible => t("Invisible", "Invisibile"),
        }
    }
}

/// The text (and emoji) shown next to our name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomStatus {
    pub text: String,
    /// A Unicode emoji, or the name of a server emoji (with `emoji_id`).
    pub emoji_name: String,
    pub emoji_id: u64,
    /// When it goes away (Unix ms), 0 = never.
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    unknown: Vec<u8>,
}

impl CustomStatus {
    pub fn new(text: &str, expires_at_ms: u64, now_ms: u64) -> Self {
        Self {
            text: clean_status_text(text),
            created_at_ms: now_ms,
            expires_at_ms,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.emoji_name.is_empty()
    }

    /// Still to show at `now_ms`.
    pub fn active(&self, now_ms: u64) -> bool {
        !self.is_empty() && (self.expires_at_ms == 0 || self.expires_at_ms > now_ms)
    }
}

/// A custom status as typed: one line, no control characters, at most
/// [`MAX_CUSTOM_STATUS`] characters.
pub fn clean_status_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_CUSTOM_STATUS)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSettings {
    /// As saved; `None` = never set (online).
    pub status: Option<String>,
    pub custom: Option<CustomStatus>,
    /// Fields not known here (e.g. "show the game you are playing"), kept and
    /// sent back as received.
    unknown: Vec<u8>,
    /// When a timed status ends (fields 4 and 5): dropped when Rico changes
    /// the status, so a new one does not inherit the old timer.
    timer: Vec<u8>,
}

impl StatusSettings {
    pub fn presence(&self) -> Presence {
        Presence::from_code(self.status.as_deref().unwrap_or(""))
    }

    /// The same settings with a new status, chosen by the user.
    #[must_use]
    pub fn with_presence(&self, p: Presence) -> Self {
        Self {
            status: Some(p.code().to_string()),
            timer: Vec::new(),
            ..self.clone()
        }
    }

    /// The same settings with a new custom status (`None` removes it).
    #[must_use]
    pub fn with_custom(&self, custom: Option<CustomStatus>) -> Self {
        Self {
            custom: custom.filter(|c| !c.is_empty()),
            ..self.clone()
        }
    }
}

fn decode_status(bytes: &[u8]) -> Result<StatusSettings> {
    let mut out = StatusSettings::default();
    let mut r = Reader::new(bytes);
    loop {
        let start = r.pos;
        let Some((field, value)) = r.next()? else { break };
        match (field, value) {
            (1, Value::Bytes(w)) => {
                out.status = wrapper_string(w)?.map(|s| s.chars().take(32).collect());
            }
            (2, Value::Bytes(c)) => out.custom = Some(decode_custom_status(c)?),
            (4 | 5, _) => out.timer.extend_from_slice(&bytes[start..r.pos]),
            _ => out.unknown.extend_from_slice(&bytes[start..r.pos]),
        }
    }
    Ok(out)
}

fn decode_custom_status(bytes: &[u8]) -> Result<CustomStatus> {
    let mut c = CustomStatus::default();
    let mut r = Reader::new(bytes);
    loop {
        let start = r.pos;
        let Some((field, value)) = r.next()? else { break };
        match (field, value) {
            (1, Value::Bytes(t)) => c.text = clean_status_text(&String::from_utf8_lossy(t)),
            (2, Value::Fixed64(id)) => c.emoji_id = id,
            (3, Value::Bytes(n)) => c.emoji_name = String::from_utf8_lossy(n).chars().take(64).collect(),
            (4, Value::Fixed64(ms)) => c.expires_at_ms = ms,
            (5, Value::Fixed64(ms)) => c.created_at_ms = ms,
            _ => c.unknown.extend_from_slice(&bytes[start..r.pos]),
        }
    }
    Ok(c)
}

fn put_fixed64(out: &mut Vec<u8>, field: u32, v: u64) {
    put_varint(out, (u64::from(field) << 3) | 1);
    out.extend_from_slice(&v.to_le_bytes());
}

/// Base64 `PreloadedUserSettings` holding only the status (field 11), whole.
pub fn encode_status_update(s: &StatusSettings) -> String {
    let mut body = Vec::new();
    if let Some(status) = &s.status {
        let mut w = Vec::new();
        put_bytes(&mut w, 1, status.as_bytes());
        put_bytes(&mut body, 1, &w);
    }
    if let Some(c) = &s.custom {
        let mut cs = Vec::new();
        if !c.text.is_empty() {
            put_bytes(&mut cs, 1, c.text.as_bytes());
        }
        if c.emoji_id != 0 {
            put_fixed64(&mut cs, 2, c.emoji_id);
        }
        if !c.emoji_name.is_empty() {
            put_bytes(&mut cs, 3, c.emoji_name.as_bytes());
        }
        if c.expires_at_ms != 0 {
            put_fixed64(&mut cs, 4, c.expires_at_ms);
        }
        if c.created_at_ms != 0 {
            put_fixed64(&mut cs, 5, c.created_at_ms);
        }
        cs.extend_from_slice(&c.unknown);
        put_bytes(&mut body, 2, &cs);
    }
    body.extend_from_slice(&s.unknown);
    body.extend_from_slice(&s.timer);
    let mut settings = Vec::new();
    put_bytes(&mut settings, 11, &body);
    base64::engine::general_purpose::STANDARD.encode(settings)
}

// --- Editing -----------------------------------------------------------------

impl GuildFolders {
    /// Every server we are in appears exactly once: new ones (not yet in the
    /// settings) go on top, as Discord does. Servers no longer listed are
    /// kept (they may be only temporarily unavailable) but not shown.
    pub fn normalize(&mut self, known: &[String]) {
        let mut seen = std::collections::HashSet::new();
        for f in &mut self.folders {
            f.guild_ids.retain(|id| seen.insert(id.clone()));
        }
        self.folders.retain(|f| f.is_folder() || !f.guild_ids.is_empty());
        let missing: Vec<Folder> = known
            .iter()
            .filter(|id| !seen.contains(*id))
            .map(|id| Folder {
                guild_ids: vec![id.clone()],
                ..Default::default()
            })
            .collect();
        self.folders.splice(0..0, missing);
    }

    /// Where a server is: (entry, position inside it).
    pub fn find(&self, guild_id: &str) -> Option<(usize, usize)> {
        self.folders
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.guild_ids.iter().position(|g| g == guild_id).map(|j| (i, j)))
    }

    fn take_guild(&mut self, guild_id: &str) -> Option<usize> {
        let (i, j) = self.find(guild_id)?;
        self.folders[i].guild_ids.remove(j);
        if !self.folders[i].is_folder() && self.folders[i].guild_ids.is_empty() {
            self.folders.remove(i);
            return Some(i);
        }
        Some(i + 1)
    }

    /// Puts a server in the folder `folder_id`, at the end.
    pub fn move_to_folder(&mut self, guild_id: &str, folder_id: i64) {
        if self.find(guild_id).map(|(i, _)| self.folders[i].id) == Some(Some(folder_id)) {
            return;
        }
        if !self.folders.iter().any(|f| f.id == Some(folder_id)) {
            return;
        }
        self.take_guild(guild_id);
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == Some(folder_id)) {
            f.guild_ids.push(guild_id.to_string());
        }
        self.drop_empty_folders();
    }

    /// Takes a server out of its folder: it goes right after the folder.
    pub fn remove_from_folder(&mut self, guild_id: &str) {
        let Some((i, _)) = self.find(guild_id) else { return };
        if !self.folders[i].is_folder() {
            return;
        }
        let after = self.take_guild(guild_id).unwrap_or(self.folders.len());
        self.folders.insert(
            after.min(self.folders.len()),
            Folder {
                guild_ids: vec![guild_id.to_string()],
                ..Default::default()
            },
        );
        self.drop_empty_folders();
    }

    /// New folder with one server in it, where the server was. Returns its id.
    pub fn new_folder(&mut self, guild_id: &str, name: Option<String>, color: Option<u64>) -> i64 {
        let id = loop {
            // Discord uses random positive ids that fit in JavaScript numbers.
            let id = (rand::random::<u64>() >> 12) as i64;
            if id > 0 && !self.folders.iter().any(|f| f.id == Some(id)) {
                break id;
            }
        };
        // Where the server was: in place of its single entry, or right after
        // the folder it leaves.
        let at = self.take_guild(guild_id).unwrap_or(0);
        self.folders.insert(
            at.min(self.folders.len()),
            Folder {
                id: Some(id),
                name,
                color,
                guild_ids: vec![guild_id.to_string()],
                unknown: Vec::new(),
            },
        );
        self.drop_empty_folders();
        id
    }

    pub fn rename(&mut self, folder_id: i64, name: &str) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == Some(folder_id)) {
            let name: String = name.trim().chars().take(MAX_NAME).collect();
            f.name = (!name.is_empty()).then_some(name);
        }
    }

    pub fn set_color(&mut self, folder_id: i64, color: Option<u64>) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == Some(folder_id)) {
            f.color = color.map(|c| c & 0xff_ffff);
        }
    }

    /// Removes a folder, leaving its servers where it was.
    pub fn ungroup(&mut self, folder_id: i64) {
        let Some(i) = self.folders.iter().position(|f| f.id == Some(folder_id)) else {
            return;
        };
        let folder = self.folders.remove(i);
        for (k, id) in folder.guild_ids.into_iter().enumerate() {
            self.folders.insert(
                i + k,
                Folder {
                    guild_ids: vec![id],
                    ..Default::default()
                },
            );
        }
    }

    /// Moves an entry (folder or single server) up (`-1`) or down (`1`).
    pub fn move_entry(&mut self, index: usize, by: i32) {
        let to = index as i64 + i64::from(by);
        if index < self.folders.len() && (0..self.folders.len() as i64).contains(&to) {
            self.folders.swap(index, to as usize);
        }
    }

    /// Moves a server up or down inside its folder.
    pub fn move_in_folder(&mut self, guild_id: &str, by: i32) {
        let Some((i, j)) = self.find(guild_id) else { return };
        let ids = &mut self.folders[i].guild_ids;
        let to = j as i64 + i64::from(by);
        if (0..ids.len() as i64).contains(&to) {
            ids.swap(j, to as usize);
        }
    }

    /// Folders left with no server disappear, as on Discord.
    fn drop_empty_folders(&mut self) {
        self.folders.retain(|f| !f.guild_ids.is_empty());
    }
}

// --- Minimal protobuf -------------------------------------------------------

enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut out = 0u64;
        for shift in (0..64).step_by(7) {
            let Some(&b) = self.buf.get(self.pos) else {
                bail!("protobuf: truncated")
            };
            self.pos += 1;
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
        }
        bail!("protobuf: varint too long")
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len());
        let Some(end) = end else {
            bail!("protobuf: truncated")
        };
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn next(&mut self) -> Result<Option<(u32, Value<'a>)>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).context("protobuf: bad field")?;
        let value = match key & 7 {
            0 => Value::Varint(self.varint()?),
            1 => Value::Fixed64(u64::from_le_bytes(self.take(8)?.try_into()?)),
            2 => {
                let len = usize::try_from(self.varint()?)?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Value::Fixed32
            }
            other => bail!("protobuf: unsupported wire type {other}"),
        };
        Ok(Some((field, value)))
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_varint_field(out: &mut Vec<u8>, field: u32, v: u64) {
    put_varint(out, u64::from(field) << 3);
    put_varint(out, v);
}

fn put_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_varint(out, (u64::from(field) << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: Option<i64>, name: Option<&str>, guilds: &[&str]) -> Folder {
        Folder {
            id,
            name: name.map(str::to_string),
            color: id.map(|_| 0x5865f2),
            guild_ids: guilds.iter().map(ToString::to_string).collect(),
            unknown: Vec::new(),
        }
    }

    fn ids(f: &GuildFolders) -> Vec<(Option<i64>, Vec<String>)> {
        f.folders.iter().map(|x| (x.id, x.guild_ids.clone())).collect()
    }

    #[test]
    fn round_trip_with_versions_and_unknown_fields() {
        let mut gf = GuildFolders {
            folders: vec![
                folder(None, None, &["111"]),
                folder(Some(42), Some("Giochi"), &["222", "333"]),
            ],
            guild_positions: vec![7],
        };
        // A field Rico does not know (field 9, varint 5) is kept.
        gf.folders[1].unknown = vec![9 << 3, 5];
        let update = encode_folders_update(&gf);
        let snap = decode_settings(&update).unwrap();
        assert_eq!(snap.folders, Some(gf));
        assert_eq!(snap.data_version, None);

        // Settings with versions (field 1 → data_version 3 = 17) and other fields.
        let mut versions = Vec::new();
        put_varint_field(&mut versions, 3, 17);
        let mut full = Vec::new();
        put_bytes(&mut full, 1, &versions);
        put_bytes(&mut full, 13, b"\x08\x01");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&full);
        let snap = decode_settings(&b64).unwrap();
        assert_eq!(snap.data_version, Some(17));
        assert_eq!(snap.folders, None);
    }

    #[test]
    fn unpacked_ids_and_empty_wrappers() {
        // guild_ids as single fixed64 fields; name wrapper with no value.
        let mut f = Vec::new();
        for id in [5u64, 6] {
            put_varint(&mut f, (1 << 3) | 1);
            f.extend_from_slice(&id.to_le_bytes());
        }
        put_bytes(&mut f, 3, &[]);
        let mut body = Vec::new();
        put_bytes(&mut body, 1, &f);
        let folders = decode_folders(&body).unwrap();
        assert_eq!(folders.folders[0].guild_ids, ["5", "6"]);
        assert_eq!(folders.folders[0].name.as_deref(), Some(""));
    }

    #[test]
    fn broken_data_is_refused_not_a_crash() {
        assert!(decode_settings("not base64!").is_err());
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        // Truncated length, huge length, bad wire type.
        assert!(decode_settings(&b64(&[0x72, 0x05, 0x01])).is_err());
        assert!(decode_settings(&b64(&[0x72, 0xff, 0xff, 0xff, 0xff, 0x0f])).is_err());
        assert!(decode_settings(&b64(&[0x77])).is_err());
        // A folder with too many servers.
        let mut packed = Vec::new();
        for i in 0..=(MAX_GUILDS_PER_FOLDER as u64) {
            packed.extend_from_slice(&i.to_le_bytes());
        }
        let mut f = Vec::new();
        put_bytes(&mut f, 1, &packed);
        let mut body = Vec::new();
        put_bytes(&mut body, 1, &f);
        assert!(decode_folders(&body).is_err());
    }

    #[test]
    fn editing() {
        let mut gf = GuildFolders {
            folders: vec![
                folder(Some(1), Some("A"), &["10", "11"]),
                folder(None, None, &["12"]),
            ],
            guild_positions: Vec::new(),
        };
        // New server on top, duplicates removed.
        gf.folders.push(folder(None, None, &["10"]));
        gf.normalize(&["10".into(), "11".into(), "12".into(), "13".into()]);
        assert_eq!(
            ids(&gf),
            [
                (None, vec!["13".to_string()]),
                (Some(1), vec!["10".into(), "11".into()]),
                (None, vec!["12".into()])
            ]
        );
        gf.move_to_folder("12", 1);
        assert_eq!(gf.folders.len(), 2);
        assert_eq!(gf.folders[1].guild_ids, ["10", "11", "12"]);
        gf.remove_from_folder("10");
        assert_eq!(ids(&gf)[2], (None, vec!["10".to_string()]));
        let new = gf.new_folder("13", Some("B".into()), None);
        assert_eq!(gf.folders[0].id, Some(new));
        gf.rename(new, "  Nuova cartella  ");
        assert_eq!(gf.folders[0].name.as_deref(), Some("Nuova cartella"));
        gf.set_color(new, Some(0x1234_56ff_ffff));
        assert_eq!(gf.folders[0].color, Some(0xff_ffff));
        gf.move_entry(0, 1);
        assert_eq!(gf.folders[1].id, Some(new));
        gf.move_in_folder("12", -1);
        assert_eq!(gf.folders[0].guild_ids, ["12", "11"]);
        gf.ungroup(1);
        assert_eq!(gf.folders.iter().filter(|f| f.is_folder()).count(), 1);
        // A folder that loses its last server disappears.
        gf.move_to_folder("13", new);
        gf.remove_from_folder("13");
        assert!(gf.folders.iter().all(|f| !f.is_folder()));
    }

    #[test]
    fn status_round_trip_keeps_unknown_fields() {
        // status "dnd", custom status with an emoji and a timer, show_current_game
        // (field 3, unknown here) and a status timer (field 4).
        let mut cs = Vec::new();
        put_bytes(&mut cs, 1, b"At work");
        put_bytes(&mut cs, 3, "\u{1f4bb}".as_bytes());
        put_fixed64(&mut cs, 4, 2_000);
        put_fixed64(&mut cs, 5, 1_000);
        put_bytes(&mut cs, 6, b"x");
        let mut body = Vec::new();
        let mut w = Vec::new();
        put_bytes(&mut w, 1, b"dnd");
        put_bytes(&mut body, 1, &w);
        put_bytes(&mut body, 2, &cs);
        put_bytes(&mut body, 3, &[8, 1]);
        put_fixed64(&mut body, 4, 9_999);
        let mut full = Vec::new();
        put_bytes(&mut full, 11, &body);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&full);

        let s = decode_settings(&b64).unwrap().status.unwrap();
        assert_eq!(s.presence(), Presence::DoNotDisturb);
        let c = s.custom.clone().unwrap();
        assert_eq!((c.text.as_str(), c.emoji_name.as_str()), ("At work", "\u{1f4bb}"));
        assert!(c.active(1_500) && !c.active(2_000));
        // Sent back unchanged.
        let again = decode_settings(&encode_status_update(&s))
            .unwrap()
            .status
            .unwrap();
        assert_eq!(again, s);
        // A new status drops the old timer, keeps the rest.
        let idle = s.with_presence(Presence::Idle);
        let back = decode_settings(&encode_status_update(&idle))
            .unwrap()
            .status
            .unwrap();
        assert_eq!(back.presence(), Presence::Idle);
        assert!(back.timer.is_empty());
        assert_eq!(back.unknown, s.unknown);
        assert_eq!(back.custom, s.custom);
        // Removing the custom status.
        let none = idle.with_custom(None);
        let back = decode_settings(&encode_status_update(&none))
            .unwrap()
            .status
            .unwrap();
        assert_eq!(back.custom, None);
    }

    #[test]
    fn status_text_and_codes() {
        assert_eq!(clean_status_text("  a\nb\u{7} "), "a b");
        assert_eq!(
            clean_status_text(&"x".repeat(500)).chars().count(),
            MAX_CUSTOM_STATUS
        );
        assert_eq!(Presence::from_code("weird"), Presence::Online);
        for p in Presence::ALL {
            assert_eq!(Presence::from_code(p.code()), p);
        }
        assert!(!CustomStatus::new("  ", 0, 1).active(1));
        assert!(CustomStatus::new("hi", 0, 1).active(u64::MAX));
    }
}

//! Server folders, as Discord keeps them in the account settings.
//!
//! The settings are a protobuf (`PreloadedUserSettings`, base64 over JSON):
//! read with `GET /users/@me/settings-proto/1`, kept in sync by the gateway
//! event `USER_SETTINGS_PROTO_UPDATE`, changed with `PATCH` on the same path.
//! Only two fields are used here:
//!
//! - 1 `versions` → 3 `data_version` (sent back as `required_data_version`,
//!   so a change made meanwhile on another device is not overwritten);
//! - 14 `guild_folders` → `GuildFolders { 1 folders, 2 guild_positions }`,
//!   `GuildFolder { 1 guild_ids (fixed64), 2 id (Int64Value), 3 name
//!   (StringValue), 4 color (UInt64Value) }`.
//!
//! A PATCH replaces the whole top-level field it carries: so an update holds
//! all of field 14 and nothing else. Fields of a folder that ferrocord does
//! not know are kept as they are and sent back unchanged.
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

/// What ferrocord takes from the account settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsSnapshot {
    pub data_version: Option<u32>,
    /// `None` if the settings did not carry the folders (a partial update).
    pub folders: Option<GuildFolders>,
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
        // A field ferrocord does not know (field 9, varint 5) is kept.
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
}

//! Discord permissions: what we can see and do in each channel of a server.
//!
//! Same algorithm as Discord (developer docs, "Permissions"):
//!
//! 1. the server owner and whoever has `ADMINISTRATOR` can do everything;
//! 2. base permissions = those of `@everyone` (the role with the server's ID)
//!    plus those of our roles;
//! 3. channel overwrites, in this order: `@everyone`, then our roles
//!    together (all denies, then all allows), then our member;
//! 4. while we are timed out only viewing and reading the history remain.
//!
//! The server checks everything again: this only decides what to show and
//! which actions to offer, so that the client does not try what would be
//! refused (and does not show channels Discord would hide).

use serde::{Deserialize, Deserializer};

pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const ADD_REACTIONS: u64 = 1 << 6;
/// Go Live in a voice channel ("Video" in Discord's settings).
pub const STREAM: u64 = 1 << 9;
pub const ADMINISTRATOR: u64 = 1 << 3;
pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const EMBED_LINKS: u64 = 1 << 14;
pub const ATTACH_FILES: u64 = 1 << 15;
pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
pub const USE_EXTERNAL_EMOJIS: u64 = 1 << 18;
pub const CONNECT: u64 = 1 << 20;
pub const SPEAK: u64 = 1 << 21;
pub const MUTE_MEMBERS: u64 = 1 << 22;
pub const DEAFEN_MEMBERS: u64 = 1 << 23;
pub const MOVE_MEMBERS: u64 = 1 << 24;
pub const MODERATE_MEMBERS: u64 = 1 << 40;

/// Every permission.
pub const ALL: u64 = u64::MAX;

/// What remains while timed out.
const TIMED_OUT: u64 = VIEW_CHANNEL | READ_MESSAGE_HISTORY;

/// Who a channel overwrite applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Role,
    Member,
}

/// Permission overwrite of a channel.
#[derive(Debug, Clone, Deserialize)]
pub struct Overwrite {
    pub id: String,
    #[serde(rename = "type", deserialize_with = "de_target")]
    pub target: Target,
    #[serde(default, deserialize_with = "de_bits")]
    pub allow: u64,
    #[serde(default, deserialize_with = "de_bits")]
    pub deny: u64,
}

/// Our situation in a server, enough to compute the permissions of each
/// channel: computed once and reused for all its channels.
#[derive(Debug, Clone)]
pub struct Base {
    guild_id: String,
    user_id: String,
    roles: Vec<String>,
    /// Permissions from the roles (before the channel overwrites).
    bits: u64,
    /// Owner or administrator: overwrites do not matter.
    everything: bool,
    timed_out: bool,
}

impl Base {
    /// `roles`: (role ID, its permissions) of the server; `member_roles`: our
    /// roles. `owner`: we own the server.
    pub fn new(
        guild_id: &str,
        user_id: &str,
        owner: bool,
        roles: &[(&str, u64)],
        member_roles: &[String],
        timed_out: bool,
    ) -> Self {
        let mut bits = roles
            .iter()
            .find(|(id, _)| *id == guild_id)
            .map(|(_, p)| *p)
            .unwrap_or(0);
        for (id, p) in roles {
            if member_roles.iter().any(|r| r == id) {
                bits |= p;
            }
        }
        let everything = owner || bits & ADMINISTRATOR != 0;
        Base {
            guild_id: guild_id.to_string(),
            user_id: user_id.to_string(),
            roles: member_roles.to_vec(),
            bits,
            everything,
            timed_out: timed_out && !everything,
        }
    }

    /// Permissions in the whole server (no channel): what the moderation
    /// actions need. Nothing while timed out.
    pub fn in_guild(&self) -> u64 {
        if self.everything {
            ALL
        } else if self.timed_out {
            0
        } else {
            self.bits
        }
    }

    /// We own the server or are an administrator.
    pub fn is_admin(&self) -> bool {
        self.everything
    }

    /// Permissions in a channel with these overwrites.
    pub fn in_channel(&self, overwrites: &[Overwrite]) -> u64 {
        if self.everything {
            return ALL;
        }
        let mut bits = self.bits;
        if let Some(o) = overwrites
            .iter()
            .find(|o| o.target == Target::Role && o.id == self.guild_id)
        {
            bits = (bits & !o.deny) | o.allow;
        }
        let (mut allow, mut deny) = (0, 0);
        for o in overwrites
            .iter()
            .filter(|o| o.target == Target::Role && self.roles.contains(&o.id))
        {
            allow |= o.allow;
            deny |= o.deny;
        }
        bits = (bits & !deny) | allow;
        if let Some(o) = overwrites
            .iter()
            .find(|o| o.target == Target::Member && o.id == self.user_id)
        {
            bits = (bits & !o.deny) | o.allow;
        }
        if self.timed_out {
            bits &= TIMED_OUT;
        }
        bits
    }
}

/// Permissions arrive as a decimal string (they do not fit in a JSON
/// number for every language); a number is accepted too. Anything else = 0.
pub fn de_bits<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bits {
        Text(String),
        Number(u64),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Bits::deserialize(d)? {
        Bits::Text(s) => s.trim().parse().unwrap_or(0),
        Bits::Number(n) => n,
        Bits::Other(_) => 0,
    })
}

/// `0`/`"role"` = role, `1`/`"member"` = member.
fn de_target<'de, D: Deserializer<'de>>(d: D) -> Result<Target, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u8),
        Text(String),
    }
    match Raw::deserialize(d)? {
        Raw::Number(0) => Ok(Target::Role),
        Raw::Number(1) => Ok(Target::Member),
        Raw::Text(s) if s == "role" => Ok(Target::Role),
        Raw::Text(s) if s == "member" => Ok(Target::Member),
        _ => Err(serde::de::Error::custom("unknown overwrite type")),
    }
}

/// Timed out until this moment (RFC 3339), if it is in the future.
pub fn timed_out(until: Option<&str>) -> bool {
    until
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| t > chrono::Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "100";
    const ME: &str = "1";
    const EVERYONE_DEFAULT: u64 = VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY | CONNECT | SPEAK;

    fn ow(id: &str, target: Target, allow: u64, deny: u64) -> Overwrite {
        Overwrite {
            id: id.into(),
            target,
            allow,
            deny,
        }
    }

    fn base(roles: &[(&str, u64)], mine: &[&str]) -> Base {
        let mine: Vec<String> = mine.iter().map(|s| s.to_string()).collect();
        Base::new(G, ME, false, roles, &mine, false)
    }

    #[test]
    fn everyone_and_roles_add_up() {
        let b = base(&[(G, VIEW_CHANNEL), ("7", SEND_MESSAGES), ("8", CONNECT)], &["7"]);
        let p = b.in_channel(&[]);
        assert_eq!(p, VIEW_CHANNEL | SEND_MESSAGES);
        assert_eq!(p & CONNECT, 0, "role 8 is not ours");
    }

    #[test]
    fn overwrites_in_discord_order() {
        let b = base(&[(G, EVERYONE_DEFAULT), ("7", 0), ("8", 0)], &["7", "8"]);
        // Private channel: @everyone cannot see it, our role can.
        let private = [
            ow(G, Target::Role, 0, VIEW_CHANNEL),
            ow("7", Target::Role, VIEW_CHANNEL, 0),
        ];
        assert_ne!(b.in_channel(&private) & VIEW_CHANNEL, 0);
        // Between roles the allow wins over the deny.
        let mixed = [
            ow("7", Target::Role, 0, SEND_MESSAGES),
            ow("8", Target::Role, SEND_MESSAGES, 0),
        ];
        assert_ne!(b.in_channel(&mixed) & SEND_MESSAGES, 0);
        // The member overwrite comes last and wins over the roles.
        let muted = [
            ow("8", Target::Role, SEND_MESSAGES, 0),
            ow(ME, Target::Member, 0, SEND_MESSAGES),
        ];
        assert_eq!(b.in_channel(&muted) & SEND_MESSAGES, 0);
        // Someone else's overwrites do not count.
        let other = [
            ow("2", Target::Member, 0, VIEW_CHANNEL),
            ow("9", Target::Role, 0, VIEW_CHANNEL),
        ];
        assert_ne!(b.in_channel(&other) & VIEW_CHANNEL, 0);
    }

    #[test]
    fn hidden_channel_for_everyone() {
        let b = base(&[(G, EVERYONE_DEFAULT)], &[]);
        assert_eq!(
            b.in_channel(&[ow(G, Target::Role, 0, VIEW_CHANNEL)]) & VIEW_CHANNEL,
            0
        );
    }

    #[test]
    fn owner_and_administrator_see_everything() {
        let hidden = [ow(G, Target::Role, 0, VIEW_CHANNEL | SEND_MESSAGES)];
        let owner = Base::new(G, ME, true, &[(G, 0)], &[], false);
        assert_eq!(owner.in_channel(&hidden), ALL);
        let admin = base(&[(G, 0), ("7", ADMINISTRATOR)], &["7"]);
        assert_eq!(admin.in_channel(&hidden), ALL);
        // An administrator is not limited by a timeout either.
        let admin = Base::new(G, ME, false, &[(G, 0), ("7", ADMINISTRATOR)], &["7".into()], true);
        assert_eq!(admin.in_channel(&[]), ALL);
    }

    #[test]
    fn timeout_leaves_only_reading() {
        let b = Base::new(G, ME, false, &[(G, EVERYONE_DEFAULT)], &[], true);
        assert_eq!(b.in_channel(&[]), VIEW_CHANNEL | READ_MESSAGE_HISTORY);
        assert!(timed_out(Some("2999-01-01T00:00:00+00:00")));
        assert!(!timed_out(Some("2001-01-01T00:00:00+00:00")));
        assert!(!timed_out(None));
        assert!(!timed_out(Some("tomorrow")));
    }

    #[test]
    fn overwrites_from_json() {
        let o: Vec<Overwrite> = serde_json::from_str(
            r#"[{"id": "100", "type": 0, "allow": "0", "deny": "1024"},
                {"id": "1", "type": "member", "allow": "2048", "deny": "0"},
                {"id": "7", "type": 0, "allow": 3072, "deny": null}]"#,
        )
        .unwrap();
        assert_eq!(o[0].target, Target::Role);
        assert_eq!(o[0].deny, VIEW_CHANNEL);
        assert_eq!(o[1].target, Target::Member);
        assert_eq!(o[1].allow, SEND_MESSAGES);
        assert_eq!(o[2].allow, VIEW_CHANNEL | SEND_MESSAGES);
        assert_eq!(o[2].deny, 0);
    }
}

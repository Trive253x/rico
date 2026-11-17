//! Message text: Discord tags (`<@id>`, `<#id>`, `<:emoji:id>`, `<t:ts>`)
//! become readable text, split into segments to colour.
//! Used by both the GUI and the terminal.

use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::{DateTime, Local};

use crate::discord::models::{GuildEmoji, Role, User};
use crate::i18n::t;

pub struct Ctx<'a> {
    pub users: &'a HashMap<String, User>,
    pub channels: &'a HashMap<String, String>,
    pub me: Option<&'a str>,
    /// Roles of the channel's server (empty in direct messages), for
    /// `<@&id>` mentions.
    pub roles: &'a [Role],
    /// Our roles in that server: a mention of one of them concerns us.
    pub my_roles: &'a [String],
    /// The channel's server (`None` in direct messages).
    pub guild_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Plain,
    /// Mention of a user or a role; `true` if it concerns the client's user.
    Mention(bool),
    Channel,
    Emoji,
    Time,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub kind: SegmentKind,
    /// For custom emojis: which image to show instead of the text.
    pub emoji: Option<CustomEmoji>,
    /// For mentions: who or what is mentioned (a click shows it).
    pub target: Option<Target>,
}

/// What a mention points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    User(String),
    Role(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

impl Ctx<'_> {
    /// The message mentions us: by name, with @everyone/@here, or one of our
    /// roles.
    pub fn mentions_me(&self, m: &crate::discord::models::Message) -> bool {
        self.me.is_some_and(|me| {
            m.mention_everyone
                || m.mentions.iter().any(|u| u.id == me)
                || m.mention_roles.iter().any(|r| self.my_roles.contains(r))
        })
    }
}

/// Text with the tags replaced.
pub fn render_content(s: &str, ctx: &Ctx) -> String {
    segments(s, ctx).into_iter().map(|seg| seg.text).collect()
}

/// Splits the text into segments: plain text and tags made readable.
pub fn segments(s: &str, ctx: &Ctx) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut plain = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        plain.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('>') {
            Some(end) if end <= 80 => {
                match replace_tag(&tail[1..end], ctx) {
                    Some(seg) => {
                        if !plain.is_empty() {
                            out.push(Segment {
                                text: std::mem::take(&mut plain),
                                kind: SegmentKind::Plain,
                                emoji: None,
                                target: None,
                            });
                        }
                        out.push(seg);
                    }
                    None => plain.push_str(&tail[..=end]),
                }
                rest = &tail[end + 1..];
            }
            _ => {
                plain.push('<');
                rest = &tail[1..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Segment {
            text: plain,
            kind: SegmentKind::Plain,
            emoji: None,
            target: None,
        });
    }
    out
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn replace_tag(inner: &str, ctx: &Ctx) -> Option<Segment> {
    let seg = |text: String, kind| {
        Some(Segment {
            text,
            kind,
            emoji: None,
            target: None,
        })
    };
    if let Some(id) = inner.strip_prefix("@&") {
        if !all_digits(id) {
            return None;
        }
        // As Discord: a role that is not (or no longer) in the server.
        let name = ctx
            .roles
            .iter()
            .find(|r| r.id == id)
            .map_or_else(|| t("unknown-role", "ruolo-sconosciuto"), |r| r.name.as_str());
        let mine = ctx.my_roles.iter().any(|r| r == id);
        return Some(Segment {
            text: format!("@{name}"),
            kind: SegmentKind::Mention(mine),
            emoji: None,
            target: Some(Target::Role(id.to_string())),
        });
    }
    if let Some(id) = inner.strip_prefix('@') {
        let id = id.strip_prefix('!').unwrap_or(id);
        if !all_digits(id) {
            return None;
        }
        let name = ctx
            .users
            .get(id)
            .map_or_else(|| t("user", "utente"), User::display_name);
        return Some(Segment {
            text: format!("@{name}"),
            kind: SegmentKind::Mention(ctx.me == Some(id)),
            emoji: None,
            target: Some(Target::User(id.to_string())),
        });
    }
    if let Some(id) = inner.strip_prefix('#') {
        if !all_digits(id) {
            return None;
        }
        let name = ctx
            .channels
            .get(id)
            .map_or_else(|| t("channel", "canale"), String::as_str);
        return seg(format!("#{name}"), SegmentKind::Channel);
    }
    if let Some(rest) = inner.strip_prefix("t:") {
        let secs: i64 = rest.split(':').next()?.parse().ok()?;
        let dt = DateTime::from_timestamp(secs, 0)?.with_timezone(&Local);
        let format = t("%Y-%m-%d %H:%M", "%d/%m/%Y %H:%M");
        return seg(dt.format(format).to_string(), SegmentKind::Time);
    }
    // Custom emojis: <:name:id> or <a:name:id> (animated)
    let animated = inner.starts_with("a:");
    let emoji = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':'))?;
    let (name, id) = emoji.rsplit_once(':')?;
    if all_digits(id) && !name.is_empty() {
        Some(Segment {
            text: format!(":{name}:"),
            kind: SegmentKind::Emoji,
            emoji: Some(CustomEmoji {
                id: id.to_string(),
                name: name.to_string(),
                animated,
            }),
            target: None,
        })
    } else {
        None
    }
}

/// Custom emojis in a text (to download their images).
pub fn custom_emojis(s: &str) -> Vec<CustomEmoji> {
    if !s.contains('<') {
        return Vec::new();
    }
    let (users, channels) = (HashMap::new(), HashMap::new());
    let ctx = Ctx {
        users: &users,
        channels: &channels,
        me: None,
        roles: &[],
        my_roles: &[],
        guild_id: None,
    };
    segments(s, &ctx)
        .into_iter()
        .filter_map(|seg| seg.emoji)
        .collect()
}

/// Expands server emojis before sending, as Discord does.
///
/// `:name:` becomes `<:name:id>` if `name` is one of the given emojis (those of the channel's server). Code between
/// backticks and tags that are already complete stay as they were.
pub fn expand_custom_emojis(content: &str, emojis: &[GuildEmoji]) -> String {
    if emojis.is_empty() || !content.contains(':') {
        return content.to_string();
    }
    let find = |name: &str| {
        emojis
            .iter()
            .find(|e| e.name == name)
            .or_else(|| emojis.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
    };
    let mut out = String::with_capacity(content.len());
    for (i, part) in content.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        // Odd parts are inside backticks: code, left alone.
        if i % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let mut rest = part;
        while let Some(start) = rest.find(':') {
            let (before, tail) = rest.split_at(start);
            out.push_str(before);
            let name_len = tail[1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '~'))
                .unwrap_or(tail.len() - 1);
            let closed = tail[1 + name_len..].starts_with(':');
            let inside_tag = out.ends_with('<') || out.ends_with("<a");
            match find(&tail[1..=name_len]) {
                Some(e) if closed && name_len >= 2 && !inside_tag => {
                    let a = if e.animated { "a" } else { "" };
                    let _ = write!(out, "<{a}:{}:{}>", e.name, e.id);
                    rest = &tail[name_len + 2..];
                }
                _ => {
                    out.push(':');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
    }
    out
}

/// Initials for avatars: "Marco" → "M", "Rust Italia" → "RI".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    let pick = |w: &str| {
        w.chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect::<String>())
    };
    match words.as_slice() {
        [] => "?".into(),
        [one] => pick(one).unwrap_or_else(|| "?".into()),
        [first, second, ..] => format!(
            "{}{}",
            pick(first).unwrap_or_default(),
            pick(second).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_replaced() {
        let mut users = HashMap::new();
        users.insert(
            "1".to_string(),
            serde_json::from_str::<User>(r#"{"id":"1","username":"giulia","global_name":"Giulia"}"#).unwrap(),
        );
        let mut channels = HashMap::new();
        channels.insert("2".to_string(), "generale".to_string());
        let roles: Vec<Role> = serde_json::from_value(serde_json::json!([
            {"id": "9", "name": "moderatori"},
            {"id": "10", "name": "staff"}
        ]))
        .unwrap();
        let my_roles = vec!["10".to_string()];
        let c = Ctx {
            users: &users,
            channels: &channels,
            me: Some("1"),
            roles: &roles,
            my_roles: &my_roles,
            guild_id: Some("100"),
        };

        assert_eq!(render_content("ciao <@1> e <@!1>", &c), "ciao @Giulia e @Giulia");
        assert_eq!(render_content("vai in <#2>", &c), "vai in #generale");
        assert_eq!(
            render_content("bravo <:ferris:123> <a:party:456>", &c),
            "bravo :ferris: :party:"
        );
        assert_eq!(render_content("<@&9> attenzione", &c), "@moderatori attenzione");
        // A role not in the server (deleted, or another server's).
        assert_eq!(render_content("<@&11>", &c), "@unknown-role");
        // Highlighted only if it is one of our roles.
        assert_eq!(segments("<@&9>", &c)[0].kind, SegmentKind::Mention(false));
        assert_eq!(segments("<@&10>", &c)[0].kind, SegmentKind::Mention(true));
        assert_eq!(render_content("a < b e c > d <html>", &c), "a < b e c > d <html>");
        assert_eq!(render_content("<@99>", &c), "@user");

        let segs = segments("ciao <@1>!", &c);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1].kind, SegmentKind::Mention(true));
        assert_eq!(segs[2].text, "!");
        assert_eq!(segs[1].target, Some(Target::User("1".into())));
        assert_eq!(segments("<@&9>", &c)[0].target, Some(Target::Role("9".into())));
        assert_eq!(segments("<#2>", &c)[0].target, None);
    }

    #[test]
    fn custom_emojis() {
        let users = HashMap::new();
        let channels = HashMap::new();
        let c = Ctx {
            users: &users,
            channels: &channels,
            me: None,
            roles: &[],
            my_roles: &[],
            guild_id: None,
        };
        let segs = segments("ok <a:party:456>", &c);
        assert_eq!(
            segs[1].emoji,
            Some(CustomEmoji {
                id: "456".into(),
                name: "party".into(),
                animated: true
            })
        );

        let emoji = |id: &str, name: &str, animated| GuildEmoji {
            id: id.into(),
            name: name.into(),
            animated,
            available: true,
            roles: vec![],
        };
        let server = [emoji("1", "ferris", false), emoji("2", "party_parrot", true)];
        assert_eq!(
            expand_custom_emojis("ciao :ferris: e :party_parrot:!", &server),
            "ciao <:ferris:1> e <a:party_parrot:2>!"
        );
        // Already complete, unknown, inside code or in times: unchanged.
        for same in ["<:ferris:1>", ":gatto:", "`:ferris:`", "ore 10:30:00", "a: b :"] {
            assert_eq!(expand_custom_emojis(same, &server), same);
        }
        assert_eq!(expand_custom_emojis("::ferris::", &server), ":<:ferris:1>:");
        assert_eq!(expand_custom_emojis(":ferris:", &[]), ":ferris:");
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Marco"), "M");
        assert_eq!(initials("Rust Italia"), "RI");
        assert_eq!(initials("Homelab & Networking"), "HN");
        assert_eq!(initials("  "), "?");
    }
}

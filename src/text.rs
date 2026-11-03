//! Testo dei messaggi: i tag di Discord (`<@id>`, `<#id>`, `<:emoji:id>`,
//! `<t:ts>`) diventano testo leggibile, diviso in segmenti da colorare.
//! Usato sia dalla GUI sia dal terminale.

use std::collections::HashMap;

use chrono::{DateTime, Local};

use crate::discord::models::{GuildEmoji, User};

pub struct Ctx<'a> {
    pub users: &'a HashMap<String, User>,
    pub channels: &'a HashMap<String, String>,
    pub me: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Plain,
    /// Menzione di un utente o di un ruolo; `true` se riguarda chi usa il client.
    Mention(bool),
    Channel,
    Emoji,
    Time,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub kind: SegmentKind,
    /// Per le emoji personalizzate: quale immagine mostrare al posto del testo.
    pub emoji: Option<CustomEmoji>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

/// Testo con i tag sostituiti.
pub fn render_content(s: &str, ctx: &Ctx) -> String {
    segments(s, ctx).into_iter().map(|seg| seg.text).collect()
}

/// Divide il testo in segmenti: testo normale e tag resi leggibili.
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
        })
    };
    if let Some(id) = inner.strip_prefix("@&") {
        return if all_digits(id) {
            seg("@ruolo".into(), SegmentKind::Mention(false))
        } else {
            None
        };
    }
    if let Some(id) = inner.strip_prefix('@') {
        let id = id.strip_prefix('!').unwrap_or(id);
        if !all_digits(id) {
            return None;
        }
        let name = ctx.users.get(id).map(User::display_name).unwrap_or("utente");
        return seg(format!("@{name}"), SegmentKind::Mention(ctx.me == Some(id)));
    }
    if let Some(id) = inner.strip_prefix('#') {
        if !all_digits(id) {
            return None;
        }
        let name = ctx.channels.get(id).map(String::as_str).unwrap_or("canale");
        return seg(format!("#{name}"), SegmentKind::Channel);
    }
    if let Some(rest) = inner.strip_prefix("t:") {
        let secs: i64 = rest.split(':').next()?.parse().ok()?;
        let dt = DateTime::from_timestamp(secs, 0)?.with_timezone(&Local);
        return seg(dt.format("%d/%m/%Y %H:%M").to_string(), SegmentKind::Time);
    }
    // Emoji personalizzate: <:nome:id> oppure <a:nome:id> (animate)
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
        })
    } else {
        None
    }
}

/// Emoji personalizzate presenti in un testo (per scaricarne le immagini).
pub fn custom_emojis(s: &str) -> Vec<CustomEmoji> {
    if !s.contains("<") {
        return Vec::new();
    }
    let (users, channels) = (HashMap::new(), HashMap::new());
    let ctx = Ctx {
        users: &users,
        channels: &channels,
        me: None,
    };
    segments(s, &ctx)
        .into_iter()
        .filter_map(|seg| seg.emoji)
        .collect()
}

/// Prima dell'invio, come fa Discord: `:nome:` diventa `<:nome:id>` se `nome`
/// è una delle emoji indicate (quelle del server del canale). Il codice tra
/// backtick e i tag già completi restano com'erano.
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
        // Le parti dispari sono dentro i backtick: codice, non si tocca.
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
            match find(&tail[1..1 + name_len]) {
                Some(e) if closed && name_len >= 2 && !inside_tag => {
                    let a = if e.animated { "a" } else { "" };
                    out.push_str(&format!("<{a}:{}:{}>", e.name, e.id));
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

/// Iniziali per gli avatar: "Marco" → "M", "Rust Italia" → "RI".
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
    fn tag_sostituiti() {
        let mut users = HashMap::new();
        users.insert(
            "1".to_string(),
            serde_json::from_str::<User>(r#"{"id":"1","username":"giulia","global_name":"Giulia"}"#).unwrap(),
        );
        let mut channels = HashMap::new();
        channels.insert("2".to_string(), "generale".to_string());
        let c = Ctx {
            users: &users,
            channels: &channels,
            me: Some("1"),
        };

        assert_eq!(render_content("ciao <@1> e <@!1>", &c), "ciao @Giulia e @Giulia");
        assert_eq!(render_content("vai in <#2>", &c), "vai in #generale");
        assert_eq!(
            render_content("bravo <:ferris:123> <a:party:456>", &c),
            "bravo :ferris: :party:"
        );
        assert_eq!(render_content("<@&9> attenzione", &c), "@ruolo attenzione");
        assert_eq!(render_content("a < b e c > d <html>", &c), "a < b e c > d <html>");
        assert_eq!(render_content("<@99>", &c), "@utente");

        let segs = segments("ciao <@1>!", &c);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1].kind, SegmentKind::Mention(true));
        assert_eq!(segs[2].text, "!");
    }

    #[test]
    fn emoji_personalizzate() {
        let users = HashMap::new();
        let channels = HashMap::new();
        let c = Ctx {
            users: &users,
            channels: &channels,
            me: None,
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
        // Già completa, sconosciuta, nel codice o con orari: non cambia.
        for same in ["<:ferris:1>", ":gatto:", "`:ferris:`", "ore 10:30:00", "a: b :"] {
            assert_eq!(expand_custom_emojis(same, &server), same);
        }
        assert_eq!(expand_custom_emojis("::ferris::", &server), ":<:ferris:1>:");
        assert_eq!(expand_custom_emojis(":ferris:", &[]), ":ferris:");
    }

    #[test]
    fn iniziali() {
        assert_eq!(initials("Marco"), "M");
        assert_eq!(initials("Rust Italia"), "RI");
        assert_eq!(initials("Homelab & Networking"), "HN");
        assert_eq!(initials("  "), "?");
    }
}

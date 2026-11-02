//! Testo dei messaggi: i tag di Discord (`<@id>`, `<#id>`, `<:emoji:id>`,
//! `<t:ts>`) diventano testo leggibile, diviso in segmenti da colorare.
//! Usato sia dalla GUI sia dal terminale.

use std::collections::HashMap;

use chrono::{DateTime, Local};

use crate::discord::models::User;

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
        });
    }
    out
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn replace_tag(inner: &str, ctx: &Ctx) -> Option<Segment> {
    let seg = |text: String, kind| Some(Segment { text, kind });
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
    let emoji = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':'))?;
    let (name, id) = emoji.rsplit_once(':')?;
    if all_digits(id) && !name.is_empty() {
        seg(format!(":{name}:"), SegmentKind::Emoji)
    } else {
        None
    }
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
    fn iniziali() {
        assert_eq!(initials("Marco"), "M");
        assert_eq!(initials("Rust Italia"), "RI");
        assert_eq!(initials("Homelab & Networking"), "HN");
        assert_eq!(initials("  "), "?");
    }
}

//! Turns messages into lines of text ready to draw: grouping by author, day
//! separators, mentions, line wrapping.

use chrono::{DateTime, Datelike, Local, NaiveDate};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{ACCENT, MUTED};
use crate::discord::models::Message;
use crate::text::{render_content, Ctx};

const NAME_COLORS: [Color; 6] = [
    Color::Cyan,
    Color::Green,
    Color::Yellow,
    Color::Magenta,
    Color::LightBlue,
    Color::LightRed,
];
/// Consecutive messages by the same author within this interval are grouped.
const GROUP_GAP_SECS: i64 = 7 * 60;

fn name_color(id: &str) -> Color {
    let h = id
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
    NAME_COLORS[h as usize % NAME_COLORS.len()]
}

fn parse_ts(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&Local))
}

fn day_separator(day: NaiveDate, width: usize) -> Line<'static> {
    let lang = crate::i18n::lang();
    let label = format!(
        " {} {} {} ",
        day.day(),
        crate::i18n::month(lang, day.month()),
        day.year()
    );
    let side = width.saturating_sub(label.width()) / 2;
    let line = format!("{}{}{}", "─".repeat(side), label, "─".repeat(side));
    Line::styled(line, Style::new().fg(MUTED))
}

pub fn message_lines(msgs: &[Message], width: usize, ctx: &Ctx) -> Vec<Line<'static>> {
    let width = width.max(12);
    let body_width = width - 2;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut prev_author: Option<&str> = None;
    let mut prev_time: Option<DateTime<Local>> = None;
    let mut prev_day: Option<NaiveDate> = None;

    for m in msgs {
        let ts = parse_ts(&m.timestamp);
        let day = ts.map(|t| t.date_naive());

        let new_day = day.is_some() && day != prev_day;
        if let Some(d) = day.filter(|_| new_day) {
            if !out.is_empty() {
                out.push(Line::default());
            }
            out.push(day_separator(d, width));
            prev_author = None;
        }

        let grouped = prev_author == Some(m.author.id.as_str())
            && matches!((prev_time, ts), (Some(a), Some(b)) if (b - a).num_seconds() < GROUP_GAP_SECS);
        if !grouped {
            if !out.is_empty() && !new_day {
                out.push(Line::default());
            }
            let time = ts.map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
            let name_style = Style::new()
                .fg(name_color(&m.author.id))
                .add_modifier(Modifier::BOLD);
            out.push(Line::from(vec![
                Span::styled(time, Style::new().fg(MUTED)),
                Span::raw(" "),
                Span::styled(m.author.display_name().to_string(), name_style),
            ]));
        }

        let mentions_me = ctx.mentions_me(m);
        let text_style = if mentions_me {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
        };

        let mut body: Vec<(String, Style)> = Vec::new();
        if let Some(r) = &m.referenced_message {
            let who = r
                .author
                .as_ref()
                .map_or("?", crate::discord::models::User::display_name);
            let quoted = render_content(&r.content, ctx).replace('\n', " ");
            let line = format!("↪ {who}: {quoted}");
            let line: String = line.chars().take(body_width.saturating_sub(1)).collect();
            body.push((line, Style::new().fg(MUTED).add_modifier(Modifier::ITALIC)));
        }
        let text = render_content(&m.content, ctx);
        if !text.is_empty() {
            body.extend(wrap(&text, body_width).into_iter().map(|l| (l, text_style)));
        }
        for a in &m.attachments {
            body.push((format!("📎 {}", a.filename), Style::new().fg(ACCENT)));
        }
        if !m.reactions.is_empty() {
            let line = m
                .reactions
                .iter()
                .map(|r| format!("{} {}", r.emoji.label(), r.count))
                .collect::<Vec<_>>()
                .join("  ");
            body.push((line, Style::new().fg(MUTED)));
        }
        if body.is_empty() {
            let note = if m.kind == 0 {
                crate::i18n::t("(empty message)", "(messaggio vuoto)")
            } else {
                crate::i18n::t("(system message)", "(messaggio di sistema)")
            };
            body.push((
                note.to_string(),
                Style::new().fg(MUTED).add_modifier(Modifier::ITALIC),
            ));
        }

        let last = body.len() - 1;
        for (i, (line, style)) in body.into_iter().enumerate() {
            let mut spans = vec![Span::raw("  "), Span::styled(line, style)];
            if i == last && m.edited_timestamp.is_some() {
                spans.push(Span::styled(
                    crate::i18n::t(" (edited)", " (modificato)"),
                    Style::new().fg(MUTED),
                ));
            }
            out.push(Line::from(spans));
        }

        prev_author = Some(m.author.id.as_str());
        prev_time = ts;
        prev_day = day;
    }
    out
}

/// Word wrapping on `width` columns; words that are too long are split.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let clean: String = text
        .replace('\t', "    ")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();

    let mut lines = Vec::new();
    for para in clean.split('\n') {
        let mut line = String::new();
        let mut used = 0usize;
        let mut empty = true;
        for word in para.split(' ') {
            let w = word.width();
            let needed = if empty { w } else { used + 1 + w };
            if needed <= width {
                if !empty {
                    line.push(' ');
                    used += 1;
                }
                line.push_str(word);
                used += w;
                empty = false;
                continue;
            }
            if !empty {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if w <= width {
                line.push_str(word);
                used = w;
            } else {
                for c in word.chars() {
                    let cw = c.width().unwrap_or(0);
                    if used + cw > width && used > 0 {
                        lines.push(std::mem::take(&mut line));
                        used = 0;
                    }
                    line.push(c);
                    used += cw;
                }
            }
            empty = false;
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_wrapping() {
        assert_eq!(wrap("ciao mondo bello", 10), ["ciao mondo", "bello"]);
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(wrap("riga uno\n\nriga tre", 20), ["riga uno", "", "riga tre"]);
        assert_eq!(wrap("a\tb", 20), ["a    b"]);
    }
}

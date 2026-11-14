//! Drawing the interface. The functions here read the state and produce
//! widgets: no network or domain logic.

mod format;
mod login;

pub use login::qr_lines;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, List, ListItem, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::discord::models::kind;
use crate::i18n::t as tr;
use crate::store::ChannelRow;
use crate::tui::app::{App, ConnState, Focus, Screen};

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

pub fn draw(f: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::Login => login::draw(f, app),
        Screen::Main => draw_main(f, app),
    }
    neutralize_controls(f.buffer_mut());
}

/// Names, titles and texts come from other people: a control character in
/// them (ESC, BEL…) would reach the terminal as a command. Message bodies are
/// already cleaned when wrapped; this is the single last check for
/// everything drawn (lists, titles, status bar), since ratatui keeps
/// zero-width characters inside the cells.
fn neutralize_controls(buf: &mut ratatui::buffer::Buffer) {
    for cell in buf.content.iter_mut() {
        if cell.symbol().chars().any(char::is_control) {
            let clean: String = cell.symbol().chars().filter(|c| !c.is_control()).collect();
            cell.set_symbol(if clean.is_empty() { " " } else { &clean });
        }
    }
}

fn panel<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    let border = if focused { ACCENT } else { MUTED };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(title)
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    }
}

/// Rectangle of height `h` vertically centred in `area`.
fn middle(area: Rect, h: u16) -> Rect {
    let h = h.min(area.height);
    Rect {
        y: area.y + (area.height - h) / 2,
        height: h,
        ..area
    }
}

fn draw_main(f: &mut Frame, app: &mut App) {
    let [body, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let (gw, cw) = if body.width < 100 { (18, 22) } else { (24, 28) };
    let [guilds, channels, chat] = Layout::horizontal([
        Constraint::Length(gw),
        Constraint::Length(cw),
        Constraint::Min(20),
    ])
    .areas(body);

    draw_guilds(f, app, guilds);
    draw_channels(f, app, channels);
    draw_chat(f, app, chat);
    draw_status(f, app, status);
}

fn unread_item(name: String, unread: bool) -> ListItem<'static> {
    let (dot, style) = if unread {
        ("● ", Style::new().add_modifier(Modifier::BOLD))
    } else {
        ("  ", Style::new())
    };
    ListItem::new(Line::from(vec![
        Span::styled(dot, Style::new().fg(ACCENT)),
        Span::styled(name, style),
    ]))
}

fn draw_guilds(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Guilds;
    let mut items = Vec::with_capacity(app.store.guilds.len() + 1);
    items.push(unread_item(
        tr("Direct messages", "Messaggi diretti").into(),
        app.store.dms_have_unread(),
    ));
    for g in &app.store.guilds {
        items.push(unread_item(
            g.name().to_string(),
            app.store.guild_has_unread(&g.id),
        ));
    }
    let list = List::new(items)
        .block(panel(" Server ", focused))
        .highlight_style(highlight(focused));
    app.guild_list.select(Some(app.guild_sel));
    f.render_stateful_widget(list, area, &mut app.guild_list);
}

fn channel_icon(k: u8) -> &'static str {
    match k {
        kind::GUILD_VOICE | kind::STAGE => "♪ ",
        kind::ANNOUNCEMENT => "» ",
        kind::DM | kind::GROUP_DM => "@ ",
        _ => "# ",
    }
}

fn draw_channels(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Channels;
    let title = match app.selected_guild() {
        Some(g) => format!(" {} ", g.name()),
        None => format!(" {} ", tr("Direct messages", "Messaggi diretti")),
    };
    let block = panel(title, focused);
    let rows = app.channel_rows();

    if rows.is_empty() {
        let text = if app.conn == ConnState::Online || app.demo {
            tr("No channels", "Nessun canale").to_string()
        } else {
            tf!("{} loading…", "{} caricamento…", spinner(app.tick))
        };
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(text).style(Style::new().fg(MUTED)).centered(),
            middle(inner, 1),
        );
        return;
    }

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| match row {
            ChannelRow::Category(name) => ListItem::new(Line::styled(
                name.to_uppercase(),
                Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
            )),
            ChannelRow::Channel {
                id,
                name,
                kind,
                selectable,
            } => {
                let mut style = Style::new();
                if !selectable {
                    style = style.fg(MUTED);
                }
                if app.store.unread.contains(id) {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if app.store.current.as_deref() == Some(id.as_str()) {
                    style = style.fg(ACCENT);
                }
                ListItem::new(Line::from(vec![
                    Span::styled(channel_icon(*kind), Style::new().fg(MUTED)),
                    Span::styled(name.clone(), style),
                ]))
            }
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(highlight(focused));
    app.channel_list.select(Some(app.channel_sel.min(rows.len() - 1)));
    f.render_stateful_widget(list, area, &mut app.channel_list);
}

fn badge(f: &mut Frame, area: Rect, text: &str, top: bool) {
    let w = (text.width() as u16).min(area.width);
    let rect = Rect {
        x: area.right().saturating_sub(w),
        y: if top {
            area.y
        } else {
            area.bottom().saturating_sub(1)
        },
        width: w,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(text.to_string()).style(Style::new().fg(Color::Black).bg(ACCENT)),
        rect,
    );
}

fn draw_chat(f: &mut Frame, app: &mut App, area: Rect) {
    let [msgs_area, input_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(area);

    let title = app
        .current_title()
        .map(|t| format!(" {t} "))
        .unwrap_or_else(|| " ferrocord ".to_string());
    let block = panel(title, app.focus == Focus::Messages);
    let inner = block.inner(msgs_area);
    f.render_widget(block, msgs_area);
    app.page = (inner.height as usize).saturating_sub(2).max(1);

    match app.store.current.clone() {
        None => {
            let lines = vec![
                Line::styled(
                    tr("Welcome to ferrocord", "Benvenuto in ferrocord"),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Line::default(),
                Line::styled(
                    tr(
                        "Pick a server with ↑ ↓ and Enter, then a channel.",
                        "Scegli un server con ↑ ↓ e Invio, poi un canale.",
                    ),
                    Style::new().fg(MUTED),
                ),
                Line::styled(
                    tr("Tab moves between panels.", "Tab passa da un pannello all'altro."),
                    Style::new().fg(MUTED),
                ),
            ];
            f.render_widget(Paragraph::new(lines).centered(), middle(inner, 4));
        }
        Some(channel_id) => {
            let loading = app.store.loading.contains(&channel_id);
            let lines = match app.store.messages.get(&channel_id) {
                Some(list) => {
                    let ctx = crate::text::Ctx {
                        users: &app.store.users,
                        channels: &app.store.channel_names,
                        me: app.store.me.as_ref().map(|u| u.id.as_str()),
                    };
                    format::message_lines(list, inner.width as usize, &ctx)
                }
                None => Vec::new(),
            };

            if lines.is_empty() {
                let text = if loading {
                    tf!(
                        "{} loading messages…",
                        "{} caricamento messaggi…",
                        spinner(app.tick)
                    )
                } else {
                    tr(
                        "No messages: write the first one!",
                        "Nessun messaggio: scrivi il primo!",
                    )
                    .to_string()
                };
                f.render_widget(
                    Paragraph::new(text).style(Style::new().fg(MUTED)).centered(),
                    middle(inner, 1),
                );
            } else {
                // View anchored at the bottom: `scroll` counts lines from the bottom.
                let h = inner.height as usize;
                app.max_scroll = lines.len().saturating_sub(h);
                app.scroll = app.scroll.min(app.max_scroll);
                let end = lines.len() - app.scroll;
                let start = end.saturating_sub(h);
                let shown = lines[start..end].to_vec();
                let rect = Rect {
                    y: inner.y + (h - shown.len()) as u16,
                    height: shown.len() as u16,
                    ..inner
                };
                f.render_widget(Paragraph::new(shown), rect);

                if loading && start == 0 {
                    badge(
                        f,
                        inner,
                        &tf!(
                            " {} earlier messages… ",
                            " {} messaggi precedenti… ",
                            spinner(app.tick)
                        ),
                        true,
                    );
                }
                if app.scroll > 0 {
                    badge(
                        f,
                        inner,
                        tr(" ↓ End for the latest ", " ↓ Fine per gli ultimi "),
                        false,
                    );
                }
            }
        }
    }

    // Message field
    let focused = app.focus == Focus::Input;
    let block = panel(tr(" Message ", " Messaggio "), focused);
    let inner = block.inner(input_area);
    f.render_widget(block, input_area);
    if app.input.is_empty() {
        let read_only = app
            .store
            .current
            .as_deref()
            .is_some_and(|c| !app.store.can_send(c));
        let hint = match app.current_title() {
            Some(_) if read_only => tr(
                "You do not have permission to send messages in this channel",
                "Non hai il permesso di inviare messaggi in questo canale",
            )
            .to_string(),
            Some(title) => tf!(
                "Message {}",
                "Scrivi in {}",
                title.split(" · ").next().unwrap_or_default()
            ),
            None => tr("Open a channel to write", "Apri un canale per scrivere").to_string(),
        };
        f.render_widget(Paragraph::new(hint).style(Style::new().fg(MUTED)), inner);
        if focused {
            f.set_cursor_position((inner.x, inner.y));
        }
    } else {
        let (text, cursor_x) = app.input.visible(inner.width as usize, None);
        f.render_widget(Paragraph::new(text), inner);
        if focused {
            f.set_cursor_position((inner.x + cursor_x, inner.y));
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let me = app.store.me.as_ref().map(|u| u.display_name()).unwrap_or("?");
    let (dot, color, text) = match &app.conn {
        _ if app.demo => ("●", Color::Magenta, "demo".to_string()),
        ConnState::Online => ("●", Color::Green, tf!("Connected as {me}", "Connesso come {me}")),
        ConnState::Connecting => (
            spinner(app.tick),
            Color::Yellow,
            tr("Connecting…", "Connessione…").to_string(),
        ),
        ConnState::Error(e) => (
            "●",
            Color::Red,
            tf!("Disconnected: {e} — retrying…", "Disconnesso: {e} — riprovo…"),
        ),
        ConnState::Offline => ("○", MUTED, "Offline".to_string()),
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(dot, Style::new().fg(color)),
        Span::raw(" "),
        Span::raw(text),
    ];
    if let Some((msg, is_error)) = &app.status {
        let style = if *is_error {
            Style::new().fg(Color::Red)
        } else {
            Style::new().fg(ACCENT)
        };
        spans.push(Span::styled("  │  ", Style::new().fg(MUTED)));
        spans.push(Span::styled(msg.clone(), style));
    }
    let left = Line::from(spans);

    let hints = match app.focus {
        Focus::Guilds => tr(
            "↑↓ server · Enter channels · Tab panel · Ctrl+X sign out · Ctrl+C quit",
            "↑↓ server · Invio canali · Tab pannello · Ctrl+X esci dall'account · Ctrl+C chiudi",
        ),
        Focus::Channels => tr(
            "↑↓ channel · Enter open · ← server · Tab panel",
            "↑↓ canale · Invio apri · ← server · Tab pannello",
        ),
        Focus::Messages => tr(
            "↑↓ scroll · PgUp/PgDn page · End latest · Enter write",
            "↑↓ scorri · PgSu/PgGiù pagina · Fine ultimi · Invio scrivi",
        ),
        Focus::Input => tr(
            "Enter send · Esc channels · PgUp/PgDn scroll",
            "Invio invia · Esc canali · PgSu/PgGiù scorri",
        ),
    };
    let free = (area.width as usize).saturating_sub(left.width() + 2);
    f.render_widget(Paragraph::new(left), area);
    if hints.width() <= free {
        f.render_widget(
            Paragraph::new(Line::styled(format!("{hints} "), Style::new().fg(MUTED)).right_aligned()),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::AppEvent;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, Terminal};
    use tokio::sync::mpsc;

    #[test]
    fn control_characters_never_reach_the_terminal() {
        use ratatui::buffer::Buffer;
        use ratatui::widgets::Widget;
        // A name with an escape sequence (window title) and a bell, drawn by
        // a list as the server column does.
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        List::new([ListItem::new(Line::from("a\u{1b}]0;owned\u{7}b"))]).render(area, &mut buf);
        assert!(buf.content.iter().any(|c| c.symbol().contains('\u{1b}')));
        neutralize_controls(&mut buf);
        assert!(!buf
            .content
            .iter()
            .any(|c| c.symbol().chars().any(char::is_control)));
        let text: String = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .replace(' ', "");
        assert_eq!(text, "a]0;ownedb");
    }

    fn screen_text(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content
            .chunks(w as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle(AppEvent::Term(Event::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))));
    }

    #[test]
    fn main_screen() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::demo(tx);
        // Server "Rust Community" → channel #general
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen_text(&mut app, 120, 30);
        println!("{text}");
        assert!(text.contains("Rust Community"));
        assert!(text.contains("#general · Rust Community"));
        assert!(text.contains("@Tommaso I saw your Rust client"));
        assert!(text.contains("CHAT"));
        assert!(text.contains(":ferris:"));
        assert_eq!(app.focus, Focus::Input);

        // A message is written and shows up at the bottom
        for c in "test send".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        let text = screen_text(&mut app, 120, 30);
        assert!(text.contains("test send"));
    }

    #[test]
    fn narrow_screen_does_not_panic() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::demo(tx);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        for (w, h) in [(40, 8), (20, 5), (80, 24), (1, 1)] {
            screen_text(&mut app, w, h);
        }
    }

    #[test]
    fn login_screen() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx, None);
        app.login.qr = crate::tui::app::QrState::Ready(
            qr_lines("https://discord.com/ra/abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ").unwrap(),
        );
        let text = screen_text(&mut app, 100, 40);
        println!("{text}");
        assert!(text.contains("QR code"));
        assert!(text.contains("Scan the code"));
        assert!(text.contains("Terms of Service"));

        press(&mut app, KeyCode::Tab);
        let text = screen_text(&mut app, 100, 40);
        println!("{text}");
        assert!(text.contains("Email or phone"));
        // The password is never shown.
        for c in "me@example.com".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        for c in "hunter2".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let text = screen_text(&mut app, 100, 40);
        assert!(text.contains("me@example.com") && !text.contains("hunter2"));
        assert_eq!(app.login.password.value(), "hunter2");
        for (w, h) in [(40, 8), (20, 5)] {
            screen_text(&mut app, w, h);
        }

        press(&mut app, KeyCode::Tab);
        let text = screen_text(&mut app, 100, 40);
        println!("{text}");
        assert!(text.contains("Paste your account's token"));
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.login.tab, crate::tui::app::LoginTab::Password);
    }
}

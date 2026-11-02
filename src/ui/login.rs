//! Schermata di accesso: QR code (consigliato) oppure token incollato.

use qrcode::{render::unicode::Dense1x2, QrCode};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    Frame,
};

use super::{spinner, ACCENT, MUTED};
use crate::app::{App, LoginTab, QrState};
use crate::config;

/// Il QR viene disegnato con mezzi blocchi Unicode (due moduli per carattere).
pub fn qr_lines(url: &str) -> Option<Vec<String>> {
    let code = QrCode::new(url.as_bytes()).ok()?;
    let text = code.render::<Dense1x2>().quiet_zone(true).build();
    Some(text.lines().map(str::to_string).collect())
}

fn tab(label: &str, active: bool) -> Span<'static> {
    let style = if active {
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(MUTED)
    };
    Span::styled(format!(" {label} "), style)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let st = &app.login;

    // Il QR va mostrato per intero o non è scansionabile.
    let qr_fits = |qr: &[String]| {
        let w = qr.first().map(|l| l.chars().count()).unwrap_or(0) as u16;
        area.height >= qr.len() as u16 + 12 && area.width >= w + 4
    };

    let body_height: u16 = match (&st.tab, &st.qr) {
        (LoginTab::Qr, QrState::Ready(qr)) if qr_fits(qr.as_slice()) => qr.len() as u16 + 3,
        (LoginTab::Qr, _) => 4,
        (LoginTab::Token, _) => 11,
    };
    let card = centered(area, 72, body_height + 9);

    let hints = match st.tab {
        LoginTab::Qr => " Tab usa un token · r nuovo QR · Esc esci ",
        LoginTab::Token => " Tab usa il QR code · Invio accedi · Esc esci ",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(
            Line::styled(
                " ferrocord ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
            .centered(),
        )
        .title_bottom(Line::styled(hints, Style::new().fg(MUTED)).centered());
    let inner = block.inner(card);
    f.render_widget(block, card);

    let [tabs, _, body, _, notice, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            tab("QR code", st.tab == LoginTab::Qr),
            Span::raw("   "),
            tab("Token", st.tab == LoginTab::Token),
        ]))
        .centered(),
        tabs,
    );

    if let Some(n) = &st.notice {
        f.render_widget(
            Paragraph::new(n.as_str())
                .style(Style::new().fg(Color::Yellow))
                .centered(),
            notice,
        );
    }
    f.render_widget(
        Paragraph::new("Client non ufficiale: usarlo può violare i Termini di Servizio di Discord.")
            .style(Style::new().fg(MUTED))
            .wrap(Wrap { trim: true })
            .centered(),
        footer,
    );

    match st.tab {
        LoginTab::Qr => draw_qr(f, app, body, &qr_fits),
        LoginTab::Token => draw_token(f, app, body),
    }
}

fn draw_qr(f: &mut Frame, app: &App, area: Rect, qr_fits: &dyn Fn(&[String]) -> bool) {
    let muted = Style::new().fg(MUTED);
    let lines: Vec<Line> = match &app.login.qr {
        QrState::Working(msg) => vec![
            Line::default(),
            Line::from(format!("{} {msg}", spinner(app.tick))),
        ],
        QrState::Ready(qr) if qr_fits(qr.as_slice()) => {
            // Moduli scuri su sfondo chiaro, indipendentemente dal tema del terminale.
            let qr_style = Style::new().fg(Color::Black).bg(Color::White);
            let mut v: Vec<Line> = qr.iter().map(|l| Line::styled(l.clone(), qr_style)).collect();
            v.push(Line::default());
            v.push(Line::from("Inquadra il codice con l'app Discord sul telefono"));
            v.push(Line::styled("Impostazioni → Scansiona codice QR", muted));
            v
        }
        QrState::Ready(_) => vec![
            Line::styled(
                "Il terminale è troppo piccolo per mostrare il QR.",
                Style::new().fg(Color::Yellow),
            ),
            Line::styled(
                "Ingrandisci la finestra oppure premi Tab per usare il token.",
                muted,
            ),
        ],
        QrState::Scanned(user) => vec![
            Line::default(),
            Line::styled(
                format!("✓ Codice scansionato da {user}"),
                Style::new().fg(Color::Green),
            ),
            Line::from("Conferma l'accesso sul telefono…"),
        ],
        QrState::Failed(err) => vec![
            Line::default(),
            Line::styled(err.clone(), Style::new().fg(Color::Red)),
            Line::styled(
                "Premi r per un nuovo codice, oppure Tab per usare il token.",
                muted,
            ),
        ],
    };
    let paragraph = Paragraph::new(lines).centered();
    // Niente a capo automatico sul QR: le righe fatte solo di spazi (la cornice
    // bianca) verrebbero spezzate in due e il codice si deformerebbe.
    let paragraph = match app.login.qr {
        QrState::Ready(_) => paragraph,
        _ => paragraph.wrap(Wrap { trim: true }),
    };
    f.render_widget(paragraph, area);
}

fn draw_token(f: &mut Frame, app: &App, area: Rect) {
    let st = &app.login;
    let [intro, _, field, _, info, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new("Incolla il token del tuo account e premi Invio.").centered(),
        intro,
    );

    let field = centered(field, 60, 3);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(" token ");
    let inner = block.inner(field);
    f.render_widget(block, field);
    let (masked, cursor_x) = st.token.visible(inner.width as usize, Some('•'));
    f.render_widget(Paragraph::new(masked), inner);
    f.set_cursor_position((inner.x + cursor_x, inner.y));

    let path = config::token_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "?".into());
    f.render_widget(
        Paragraph::new(format!("Verrà salvato in {path}, leggibile solo dal tuo utente."))
            .style(Style::new().fg(MUTED))
            .wrap(Wrap { trim: true })
            .centered(),
        info,
    );

    let line = if st.token_busy {
        Line::from(format!("{} verifica del token…", spinner(app.tick)))
    } else if let Some(e) = &st.token_error {
        Line::styled(e.clone(), Style::new().fg(Color::Red))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line).centered().wrap(Wrap { trim: true }), status);
}

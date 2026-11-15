//! Login screen: QR code (recommended), email and password, or pasted token.

use qrcode::{render::unicode::Dense1x2, QrCode};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    Frame,
};

use super::{spinner, ACCENT, MUTED};
use crate::config;
use crate::i18n::t as tr;
use crate::tui::app::{App, LoginTab, PasswordField, QrState};

/// The QR is drawn with Unicode half blocks (two modules per character).
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

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let st = &app.login;

    // The QR must be shown whole or it cannot be scanned.
    let qr_fits = |qr: &[String]| {
        let w = qr.first().map_or(0, |l| l.chars().count()) as u16;
        area.height >= qr.len() as u16 + 12 && area.width >= w + 4
    };

    let body_height: u16 = match (&st.tab, &st.qr) {
        (LoginTab::Qr, QrState::Ready(qr)) if qr_fits(qr.as_slice()) => qr.len() as u16 + 3,
        (LoginTab::Qr, _) => 4,
        (LoginTab::Password, _) if st.two_factor.is_some() => 9,
        (LoginTab::Password, _) => 14,
        (LoginTab::Token, _) => 11,
    };
    let card = centered(area, 72, body_height + 9);

    let hints = match st.tab {
        LoginTab::Qr => tr(
            " Tab email and password · r new QR · Esc quit ",
            " Tab email e password · r nuovo QR · Esc esci ",
        ),
        LoginTab::Password if st.two_factor.is_some() => tr(
            " ↑ back · Enter sign in · Tab use a token · Esc quit ",
            " ↑ indietro · Invio accedi · Tab usa un token · Esc esci ",
        ),
        LoginTab::Password => tr(
            " ↑↓ field · Enter sign in · Tab use a token · Esc quit ",
            " ↑↓ campo · Invio accedi · Tab usa un token · Esc esci ",
        ),
        LoginTab::Token => tr(
            " Tab use the QR code · Enter sign in · Esc quit ",
            " Tab usa il QR code · Invio accedi · Esc esci ",
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(" Rico ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)).centered())
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
            tab("Email", st.tab == LoginTab::Password),
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
        Paragraph::new(tr(
            "Unofficial client: using it may violate Discord's Terms of Service.",
            "Client non ufficiale: usarlo può violare i Termini di Servizio di Discord.",
        ))
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true })
        .centered(),
        footer,
    );

    match st.tab {
        LoginTab::Qr => draw_qr(f, app, body, &qr_fits),
        LoginTab::Password => draw_password(f, app, body),
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
            // Dark modules on a light background, whatever the terminal theme.
            let qr_style = Style::new().fg(Color::Black).bg(Color::White);
            let mut v: Vec<Line> = qr.iter().map(|l| Line::styled(l.clone(), qr_style)).collect();
            v.push(Line::default());
            v.push(Line::from(tr(
                "Scan the code with the Discord app on your phone",
                "Inquadra il codice con l'app Discord sul telefono",
            )));
            v.push(Line::styled(
                tr("Settings → Scan QR Code", "Impostazioni → Scansiona codice QR"),
                muted,
            ));
            v
        }
        QrState::Ready(_) => vec![
            Line::styled(
                tr(
                    "The terminal is too small to show the QR code.",
                    "Il terminale è troppo piccolo per mostrare il QR.",
                ),
                Style::new().fg(Color::Yellow),
            ),
            Line::styled(
                tr(
                    "Enlarge the window or press Tab for another way to sign in.",
                    "Ingrandisci la finestra oppure premi Tab per accedere in un altro modo.",
                ),
                muted,
            ),
        ],
        QrState::Scanned(user) => vec![
            Line::default(),
            Line::styled(
                tf!("✓ Code scanned by {user}", "✓ Codice scansionato da {user}"),
                Style::new().fg(Color::Green),
            ),
            Line::from(tr(
                "Confirm the login on your phone…",
                "Conferma l'accesso sul telefono…",
            )),
        ],
        QrState::Failed(err) => vec![
            Line::default(),
            Line::styled(err.clone(), Style::new().fg(Color::Red)),
            Line::styled(
                tr(
                    "Press r for a new code, or Tab to sign in with email and password.",
                    "Premi r per un nuovo codice, oppure Tab per accedere con email e password.",
                ),
                muted,
            ),
        ],
    };
    let paragraph = Paragraph::new(lines).centered();
    // No automatic wrapping on the QR: lines made only of spaces (the white
    // border) would be split in two and the code would be distorted.
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
        Paragraph::new(tr(
            "Paste your account's token and press Enter.",
            "Incolla il token del tuo account e premi Invio.",
        ))
        .centered(),
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

    let path = config::token_location();
    f.render_widget(
        Paragraph::new(tf!(
            "It will be saved in {path}, protected by the system.",
            "Verrà salvato in {path}, protetto dal sistema."
        ))
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true })
        .centered(),
        info,
    );

    let line = if st.token_busy {
        Line::from(tf!(
            "{} checking the token…",
            "{} verifica del token…",
            spinner(app.tick)
        ))
    } else if let Some(e) = &st.token_error {
        Line::styled(e.clone(), Style::new().fg(Color::Red))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line).centered().wrap(Wrap { trim: true }), status);
}

/// A one-line text field with a title; `mask` hides what is written.
fn field(
    f: &mut Frame,
    area: Rect,
    title: &str,
    input: &crate::tui::input::TextInput,
    active: bool,
    mask: Option<char>,
) {
    let area = centered(area, 60, 3);
    let colour = if active { ACCENT } else { MUTED };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colour))
        .title(format!(" {title} "));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (shown, cursor_x) = input.visible(inner.width as usize, mask);
    f.render_widget(Paragraph::new(shown), inner);
    if active && inner.width > 0 && inner.height > 0 {
        f.set_cursor_position((inner.x + cursor_x, inner.y));
    }
}

fn status_line(app: &App) -> Line<'static> {
    let st = &app.login;
    if st.password_busy {
        Line::from(tf!("{} signing in…", "{} accesso in corso…", spinner(app.tick)))
    } else if let Some(e) = &st.password_error {
        Line::styled(e.clone(), Style::new().fg(Color::Red))
    } else {
        Line::default()
    }
}

fn draw_password(f: &mut Frame, app: &App, area: Rect) {
    let st = &app.login;
    if st.two_factor.is_some() {
        let [intro, _, code, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(tr(
                "Write the code of your authenticator app and press Enter.",
                "Scrivi il codice della tua app di autenticazione e premi Invio.",
            ))
            .centered(),
            intro,
        );
        field(f, code, tr("code", "codice"), &st.code, true, None);
        f.render_widget(
            Paragraph::new(status_line(app))
                .centered()
                .wrap(Wrap { trim: true }),
            status,
        );
        return;
    }
    let [intro, _, email, password, info, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(tr(
            "Sign in with your Discord account's email (or phone) and password.",
            "Accedi con l'email (o il telefono) e la password del tuo account Discord.",
        ))
        .centered(),
        intro,
    );
    let on_email = st.field == PasswordField::Email;
    field(
        f,
        email,
        tr("Email or phone", "Email o telefono"),
        &st.email,
        on_email,
        None,
    );
    field(f, password, "password", &st.password, !on_email, Some('•'));
    f.render_widget(
        Paragraph::new(tr(
            "The password goes only to Discord and is not saved: Rico keeps only the token.",
            "La password va solo a Discord e non viene salvata: Rico conserva solo il token.",
        ))
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: true })
        .centered(),
        info,
    );
    f.render_widget(
        Paragraph::new(status_line(app))
            .centered()
            .wrap(Wrap { trim: true }),
        status,
    );
}

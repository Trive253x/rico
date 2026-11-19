//! Terminal interface state and handling of every event (keyboard,
//! network). Session data lives in [`Store`], shared with the GUI.
//!
//! Rule: no network operation blocks the interface thread. Requests start as
//! tokio tasks and the result comes back as an `AppEvent`.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

use super::input::TextInput;
use crate::config;
use crate::discord::{
    gateway::{self, GatewayEvent},
    models::{Message, User},
    password_login::{self, Answer, TwoFactor},
    remote_auth::{self, AuthEvent},
    rest::{Rest, Unauthorized},
};
use crate::i18n::t as tr;
use crate::store::{first_selectable, ChannelRow, Store};

#[derive(Debug)]
pub enum AppEvent {
    Term(Event),
    Gateway(GatewayEvent),
    Auth(AuthEvent),
    TokenChecked(Result<(String, User), String>),
    PasswordAnswer(Result<Answer, String>),
    History {
        channel_id: String,
        older: bool,
        result: Result<Vec<Message>, String>,
    },
    Sent {
        content: String,
        result: Result<Box<Message>, String>,
    },
    Tick,
}

impl From<GatewayEvent> for AppEvent {
    fn from(e: GatewayEvent) -> Self {
        Self::Gateway(e)
    }
}

impl From<AuthEvent> for AppEvent {
    fn from(e: AuthEvent) -> Self {
        Self::Auth(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTab {
    Qr,
    Password,
    Token,
}

impl LoginTab {
    fn next(self) -> Self {
        match self {
            Self::Qr => Self::Password,
            Self::Password => Self::Token,
            Self::Token => Self::Qr,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Qr => Self::Token,
            Self::Password => Self::Qr,
            Self::Token => Self::Password,
        }
    }
}

/// The field being written in the email login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordField {
    Email,
    Password,
}

#[derive(Debug, Clone)]
pub enum QrState {
    Working(String),
    /// QR lines already rendered as Unicode characters.
    Ready(Vec<String>),
    Scanned(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct LoginState {
    pub tab: LoginTab,
    pub qr: QrState,
    pub token: TextInput,
    pub token_busy: bool,
    pub token_error: Option<String>,
    pub email: TextInput,
    /// Kept only until Discord answers, never saved.
    pub password: TextInput,
    pub field: PasswordField,
    /// Discord asked for the two-factor code.
    pub two_factor: Option<TwoFactor>,
    pub code: TextInput,
    pub password_busy: bool,
    pub password_error: Option<String>,
    /// Message shown at startup (e.g. "session expired").
    pub notice: Option<String>,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            tab: LoginTab::Qr,
            qr: QrState::Working(tr("Starting…", "Avvio…").into()),
            token: TextInput::default(),
            token_busy: false,
            token_error: None,
            email: TextInput::default(),
            password: TextInput::default(),
            field: PasswordField::Email,
            two_factor: None,
            code: TextInput::default(),
            password_busy: false,
            password_error: None,
            notice: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Guilds,
    Channels,
    Messages,
    Input,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Guilds => Self::Channels,
            Self::Channels => Self::Messages,
            Self::Messages => Self::Input,
            Self::Input => Self::Guilds,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Guilds => Self::Input,
            Self::Channels => Self::Guilds,
            Self::Messages => Self::Channels,
            Self::Input => Self::Messages,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Offline,
    Connecting,
    Online,
    Error(String),
}

pub struct App {
    pub running: bool,
    pub screen: Screen,
    pub login: LoginState,
    pub tick: usize,
    pub demo: bool,

    tx: UnboundedSender<AppEvent>,
    token: Option<String>,
    rest: Option<Rest>,
    gateway: Option<JoinHandle<()>>,
    auth: Option<JoinHandle<()>>,

    pub conn: ConnState,
    pub store: Store,

    pub focus: Focus,
    /// 0 = direct messages, n = `guilds[n - 1]`.
    pub guild_sel: usize,
    pub channel_sel: usize,
    /// Lines scrolled from the bottom (0 = latest messages).
    pub scroll: usize,
    /// Updated on every draw: scroll limit and height of a page.
    pub max_scroll: usize,
    pub page: usize,
    pub input: TextInput,
    /// Message in the status bar (text, is it an error?).
    pub status: Option<(String, bool)>,

    pub guild_list: ListState,
    pub channel_list: ListState,
}

impl App {
    pub fn new(tx: UnboundedSender<AppEvent>, token: Option<String>) -> Self {
        Self {
            running: true,
            screen: if token.is_some() {
                Screen::Main
            } else {
                Screen::Login
            },
            login: LoginState::default(),
            tick: 0,
            demo: false,
            tx,
            token,
            rest: None,
            gateway: None,
            auth: None,
            conn: ConnState::Offline,
            store: Store::default(),
            focus: Focus::Guilds,
            guild_sel: 0,
            channel_sel: 0,
            scroll: 0,
            max_scroll: 0,
            page: 10,
            input: TextInput::default(),
            status: None,
            guild_list: ListState::default(),
            channel_list: ListState::default(),
        }
    }

    /// Interface filled with fake data, no network.
    pub fn demo(tx: UnboundedSender<AppEvent>) -> Self {
        let mut app = Self::new(tx, None);
        app.demo = true;
        app.screen = Screen::Main;
        app.store = Store::demo();
        app.channel_sel = first_selectable(&app.channel_rows());
        app.status = Some((
            tr(
                "Demo mode: no connection to Discord",
                "Modalità demo: nessuna connessione a Discord",
            )
            .into(),
            false,
        ));
        app
    }

    // ---------------------------------------------------------------- lifecycle

    pub fn start(&mut self) {
        if self.demo {
            return;
        }
        match self.token.clone() {
            // With a saved token it starts right away: if it is invalid the gateway will say so.
            Some(t) => self.start_session(t),
            None => self.start_qr(),
        }
    }

    pub fn shutdown(&mut self) {
        for h in [self.gateway.take(), self.auth.take()].into_iter().flatten() {
            h.abort();
        }
    }

    fn start_qr(&mut self) {
        if let Some(h) = self.auth.take() {
            h.abort();
        }
        self.login.qr = QrState::Working(tr("Starting…", "Avvio…").into());
        self.auth = Some(remote_auth::spawn(self.tx.clone()));
    }

    fn start_session(&mut self, token: String) {
        if let Some(h) = self.auth.take() {
            h.abort();
        }
        match Rest::new(&token) {
            Ok(r) => self.rest = Some(r),
            Err(e) => {
                self.set_error(tf!("Unusable token: {e:#}", "Token non utilizzabile: {e:#}"));
                return;
            }
        }
        if let Some(h) = self.gateway.take() {
            h.abort();
        }
        // The terminal does not use voice for now: no commands to the gateway.
        self.gateway = Some(gateway::spawn(token.clone(), self.tx.clone()).0);
        self.token = Some(token);
        self.conn = ConnState::Connecting;
        self.screen = Screen::Main;
    }

    fn finish_login(&mut self, token: String, user: Option<User>) {
        let saved = config::save_token(&token);
        self.start_session(token);
        if let Some(u) = user {
            self.status = Some((
                tf!("Signed in as {}", "Accesso effettuato come {}", u.display_name()),
                false,
            ));
            self.store.me = Some(u);
        }
        if let Err(e) = saved {
            self.set_error(tf!("Token not saved: {e:#}", "Token non salvato: {e:#}"));
        }
    }

    fn logout(&mut self, notice: Option<String>) {
        if let Some(h) = self.gateway.take() {
            h.abort();
        }
        config::delete_token();
        self.token = None;
        self.rest = None;
        self.store = Store::default();
        self.guild_sel = 0;
        self.channel_sel = 0;
        self.scroll = 0;
        self.focus = Focus::Guilds;
        self.input.clear();
        self.conn = ConnState::Offline;
        self.status = None;
        self.login = LoginState {
            notice,
            ..Default::default()
        };
        self.screen = Screen::Login;
        self.start_qr();
    }

    pub fn is_animating(&self) -> bool {
        match self.screen {
            Screen::Login => {
                matches!(self.login.qr, QrState::Working(_))
                    || self.login.token_busy
                    || self.login.password_busy
            }
            Screen::Main => {
                self.conn == ConnState::Connecting
                    || self
                        .store
                        .current
                        .as_ref()
                        .is_some_and(|c| self.store.loading.contains(c))
            }
        }
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), true));
    }

    // ---------------------------------------------------------------- events

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Tick => self.tick = self.tick.wrapping_add(1),
            AppEvent::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            AppEvent::Term(Event::Paste(s)) => self.on_paste(&s),
            AppEvent::Term(_) => {}
            AppEvent::Gateway(g) => self.on_gateway(g),
            AppEvent::Auth(a) => self.on_auth(a),
            AppEvent::TokenChecked(r) => self.on_token_checked(r),
            AppEvent::PasswordAnswer(r) => self.on_password_answer(r),
            AppEvent::History {
                channel_id,
                older,
                result,
            } => match result {
                Ok(batch) => self.store.apply_history(&channel_id, older, batch),
                Err(e) => {
                    self.store.history_failed(&channel_id);
                    self.set_error(tf!("History unavailable: {e}", "Cronologia non disponibile: {e}"));
                }
            },
            AppEvent::Sent { content, result } => match result {
                Ok(m) => self.insert_message(*m),
                Err(e) => {
                    if self.input.is_empty() {
                        self.input.set(&content);
                    }
                    self.set_error(tf!("Sending failed: {e}", "Invio non riuscito: {e}"));
                }
            },
        }
    }

    fn on_paste(&mut self, s: &str) {
        match self.screen {
            Screen::Login if self.login.tab == LoginTab::Token => {
                self.login.token.insert_str(s.trim());
                self.login.token_error = None;
            }
            Screen::Login if self.login.tab == LoginTab::Password => {
                let line = s.lines().next().unwrap_or_default().trim();
                self.password_input().insert_str(line);
                self.login.password_error = None;
            }
            Screen::Main => {
                self.focus = Focus::Input;
                self.input.insert_str(s);
            }
            Screen::Login => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'q')) {
            self.running = false;
            return;
        }
        match self.screen {
            Screen::Login => self.on_key_login(k),
            Screen::Main => self.on_key_main(k, ctrl),
        }
    }

    fn on_key_login(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.running = false,
            KeyCode::Tab => self.login.tab = self.login.tab.next(),
            KeyCode::BackTab => self.login.tab = self.login.tab.prev(),
            _ => match self.login.tab {
                LoginTab::Qr => {
                    if matches!(k.code, KeyCode::Char('r' | 'R')) {
                        self.start_qr();
                    }
                }
                LoginTab::Password => self.on_key_password(k),
                LoginTab::Token => {
                    if k.code == KeyCode::Enter {
                        self.submit_token();
                    } else if self.login.token.handle_key(k) {
                        self.login.token_error = None;
                    }
                }
            },
        }
    }

    /// The input being written in the email login.
    fn password_input(&mut self) -> &mut TextInput {
        let login = &mut self.login;
        match (login.two_factor.is_some(), login.field) {
            (true, _) => &mut login.code,
            (false, PasswordField::Email) => &mut login.email,
            (false, PasswordField::Password) => &mut login.password,
        }
    }

    fn on_key_password(&mut self, k: KeyEvent) {
        if self.login.two_factor.is_some() {
            match k.code {
                KeyCode::Enter => self.submit_code(),
                // Back to email and password.
                KeyCode::Up => {
                    self.login.two_factor = None;
                    self.login.code.clear();
                    self.login.password_error = None;
                }
                _ => {
                    if self.login.code.handle_key(k) {
                        self.login.password_error = None;
                    }
                }
            }
            return;
        }
        match (k.code, self.login.field) {
            (KeyCode::Up, _) => self.login.field = PasswordField::Email,
            (KeyCode::Down, _) | (KeyCode::Enter, PasswordField::Email) => {
                self.login.field = PasswordField::Password;
            }
            (KeyCode::Enter, PasswordField::Password) => self.submit_password(),
            _ => {
                if self.password_input().handle_key(k) {
                    self.login.password_error = None;
                }
            }
        }
    }

    fn submit_password(&mut self) {
        if self.login.password_busy {
            return;
        }
        let email = self.login.email.value().trim().to_string();
        if email.is_empty() || self.login.password.is_empty() {
            self.login.password_error = Some(
                tr(
                    "Write the email and the password.",
                    "Scrivi l'email e la password.",
                )
                .into(),
            );
            return;
        }
        let password = self.login.password.value().to_string();
        self.login.password_busy = true;
        self.login.password_error = None;
        self.login.notice = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = password_login::login(&email, &password).await;
            let _ = tx.send(AppEvent::PasswordAnswer(result));
        });
    }

    fn submit_code(&mut self) {
        if self.login.password_busy {
            return;
        }
        let Some(challenge) = self.login.two_factor.clone() else {
            return;
        };
        let code = self.login.code.value().trim().to_string();
        if code.is_empty() {
            return;
        }
        self.login.password_busy = true;
        self.login.password_error = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = password_login::two_factor(&challenge, &code).await;
            let _ = tx.send(AppEvent::PasswordAnswer(result));
        });
    }

    fn on_password_answer(&mut self, result: Result<Answer, String>) {
        let login = &mut self.login;
        login.password_busy = false;
        let answer = match result {
            Ok(a) => a,
            // No answer: the fields stay as they are, to try again.
            Err(e) => {
                login.password_error = Some(e);
                return;
            }
        };
        // Whatever the answer, the password is no longer needed.
        login.password.clear();
        login.code.clear();
        match answer {
            Answer::Token(token) => {
                login.two_factor = None;
                login.password_error = None;
                self.finish_login(token, None);
            }
            Answer::TwoFactor(m) if m.app => {
                login.two_factor = Some(m);
                login.password_error = None;
            }
            Answer::TwoFactor(_) => {
                login.field = PasswordField::Password;
                login.password_error = Some(
                    tr(
                        "This account confirms logins by SMS or security key, which Rico does not handle: use the QR code.",
                        "Questo account conferma gli accessi con SMS o chiave di sicurezza, che Rico non gestisce: usa il QR code.",
                    )
                    .into(),
                );
            }
            Answer::NewLocation => {
                // Authorized from the email, the login is done again from the start.
                login.two_factor = None;
                login.password_error = None;
                login.field = PasswordField::Password;
                login.notice = Some(password_login::new_location_notice().into());
            }
            Answer::Captcha => {
                login.two_factor = None;
                login.field = PasswordField::Password;
                login.password_error = Some(password_login::captcha_notice().into());
            }
            // A wrong code keeps the ticket: the code can be written again.
            Answer::Refused(reason) => {
                if login.two_factor.is_none() {
                    login.field = PasswordField::Password;
                }
                login.password_error = Some(reason);
            }
        }
    }

    fn submit_token(&mut self) {
        if self.login.token_busy {
            return;
        }
        let token = match crate::discord::rest::clean_token(self.login.token.value()) {
            Ok(t) => t,
            Err(e) => {
                self.login.token_error = Some(e);
                return;
            }
        };
        let rest = match Rest::new(&token) {
            Ok(r) => r,
            Err(e) => {
                self.login.token_error = Some(tf!("Invalid token: {e:#}", "Token non valido: {e:#}"));
                return;
            }
        };
        self.login.token_busy = true;
        self.login.token_error = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match rest.me().await {
                Ok(user) => Ok((token, user)),
                Err(e) if e.is::<Unauthorized>() => {
                    Err(tr("Invalid or expired token.", "Token non valido o scaduto.").to_string())
                }
                Err(e) => Err(tf!("Verification failed: {e:#}", "Verifica non riuscita: {e:#}")),
            };
            let _ = tx.send(AppEvent::TokenChecked(result));
        });
    }

    fn on_token_checked(&mut self, result: Result<(String, User), String>) {
        self.login.token_busy = false;
        match result {
            Ok((token, user)) => {
                self.login.token.clear();
                self.finish_login(token, Some(user));
            }
            Err(e) => self.login.token_error = Some(e),
        }
    }

    fn on_auth(&mut self, ev: AuthEvent) {
        if self.screen != Screen::Login {
            return;
        }
        match ev {
            AuthEvent::Progress(s) => self.login.qr = QrState::Working(s),
            AuthEvent::Qr(url) => {
                self.login.qr = match super::ui::qr_lines(&url) {
                    Some(lines) => QrState::Ready(lines),
                    None => QrState::Failed(
                        tr(
                            "Could not generate the QR code",
                            "Impossibile generare il codice QR",
                        )
                        .into(),
                    ),
                }
            }
            AuthEvent::Scanned(user) => self.login.qr = QrState::Scanned(user),
            AuthEvent::Token(token) => {
                self.auth = None;
                self.finish_login(token, None);
            }
            AuthEvent::Failed(e) => self.login.qr = QrState::Failed(e),
        }
    }

    fn on_key_main(&mut self, k: KeyEvent, ctrl: bool) {
        self.status = None;
        if ctrl && k.code == KeyCode::Char('x') {
            if self.demo {
                self.running = false;
            } else {
                self.logout(Some(tr("You signed out.", "Sei uscito dall'account.").into()));
            }
            return;
        }
        // Keys valid in every panel.
        let page = self.page as isize;
        match k.code {
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.prev(),
            KeyCode::PageUp => self.scroll_by(page),
            KeyCode::PageDown => self.scroll_by(-page),
            _ => self.on_key_panel(k),
        }
    }

    fn on_key_panel(&mut self, k: KeyEvent) {
        match self.focus {
            Focus::Guilds => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.select_guild(self.guild_sel.saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => self.select_guild(self.guild_sel + 1),
                KeyCode::Home => self.select_guild(0),
                KeyCode::End => self.select_guild(self.store.guilds.len()),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Channels,
                _ => {}
            },
            Focus::Channels => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_channel(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_channel(1),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_selected(),
                KeyCode::Left | KeyCode::Esc | KeyCode::Char('h') => self.focus = Focus::Guilds,
                _ => {}
            },
            Focus::Messages => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll_by(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_by(-1),
                KeyCode::Home | KeyCode::Char('g') => self.scroll_by(isize::MAX / 2),
                KeyCode::End | KeyCode::Char('G') => self.scroll = 0,
                KeyCode::Enter | KeyCode::Char('i') => self.focus = Focus::Input,
                KeyCode::Left | KeyCode::Esc | KeyCode::Char('h') => self.focus = Focus::Channels,
                _ => {}
            },
            Focus::Input => match k.code {
                KeyCode::Enter => self.send_input(),
                KeyCode::Esc => self.focus = Focus::Channels,
                _ => {
                    self.input.handle_key(k);
                }
            },
        }
    }

    // ---------------------------------------------------------------- navigation

    /// Index of the selected server (`None` = direct messages).
    fn guild_index(&self) -> Option<usize> {
        self.guild_sel.checked_sub(1)
    }

    pub fn selected_guild(&self) -> Option<&crate::discord::models::Guild> {
        self.guild_index().and_then(|i| self.store.guilds.get(i))
    }

    pub fn channel_rows(&self) -> Vec<ChannelRow> {
        self.store.channel_rows(self.guild_index())
    }

    fn select_guild(&mut self, idx: usize) {
        let idx = idx.min(self.store.guilds.len());
        if idx != self.guild_sel {
            self.guild_sel = idx;
            self.channel_sel = first_selectable(&self.channel_rows());
            self.channel_list = ListState::default();
        }
    }

    /// Selected server and channel by ID: the rows can change under them
    /// (channels hidden or deleted, server left).
    fn selection(&self) -> (Option<String>, Option<String>) {
        let guild = self.selected_guild().map(|g| g.id.clone());
        let channel = match self.channel_rows().get(self.channel_sel) {
            Some(ChannelRow::Channel { id, .. }) => Some(id.clone()),
            _ => None,
        };
        (guild, channel)
    }

    /// After a change of channels or permissions: same server and channel
    /// if still there, otherwise the first visible ones; the open channel is
    /// closed if it is no longer visible.
    fn restore_selection(&mut self, (guild, channel): (Option<String>, Option<String>)) {
        if let Some(gid) = guild {
            self.guild_sel = self
                .store
                .guilds
                .iter()
                .position(|g| g.id == gid)
                .map_or(0, |i| i + 1);
        }
        let rows = self.channel_rows();
        self.channel_sel = channel
            .and_then(|c| {
                rows.iter()
                    .position(|r| matches!(r, ChannelRow::Channel { id, selectable: true, .. } if *id == c))
            })
            .unwrap_or_else(|| first_selectable(&rows));
        self.store.close_if_hidden();
    }

    fn move_channel(&mut self, dir: isize) {
        let rows = self.channel_rows();
        let mut i = self.channel_sel as isize;
        loop {
            i += dir;
            if i < 0 || i as usize >= rows.len() {
                return;
            }
            if rows[i as usize].is_selectable() {
                self.channel_sel = i as usize;
                return;
            }
        }
    }

    fn open_selected(&mut self) {
        if let Some(ChannelRow::Channel {
            id, selectable: true, ..
        }) = self.channel_rows().get(self.channel_sel)
        {
            self.open_channel(id.clone());
        }
    }

    fn open_channel(&mut self, id: String) {
        self.scroll = 0;
        self.focus = Focus::Input;
        if self.store.open(&id) {
            self.fetch_history(id, None);
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let target = (self.scroll as isize).saturating_add(delta).max(0) as usize;
        self.scroll = target.min(self.max_scroll);
        // At the top, the previous page of history is loaded.
        if delta > 0 && self.scroll == self.max_scroll {
            if let Some(channel_id) = self.store.current.clone() {
                if let Some(before) = self.store.older_cursor(&channel_id) {
                    self.fetch_history(channel_id, Some(before));
                }
            }
        }
    }

    pub fn current_title(&self) -> Option<String> {
        self.store.current.as_deref().map(|c| self.store.title(c))
    }

    // ---------------------------------------------------------------- data

    fn fetch_history(&mut self, channel_id: String, before: Option<String>) {
        if self.demo {
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        if !self.store.begin_loading(&channel_id) {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = rest
                .messages(&channel_id, before.as_deref())
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::History {
                channel_id,
                older: before.is_some(),
                result,
            });
        });
    }

    fn send_input(&mut self) {
        let Some(channel_id) = self.store.current.clone() else {
            self.set_error(tr("Open a channel first.", "Apri prima un canale."));
            return;
        };
        let content = self.input.value().trim().to_string();
        if content.is_empty() {
            return;
        }
        if !self.store.can_send(&channel_id) {
            self.set_error(tr(
                "You do not have permission to send messages in this channel.",
                "Non hai il permesso di inviare messaggi in questo canale.",
            ));
            return;
        }
        let content = self.store.prepare_outgoing(&channel_id, &content);
        if content.chars().count() > 2000 {
            self.set_error(tr(
                "The message is longer than 2000 characters.",
                "Il messaggio supera i 2000 caratteri.",
            ));
            return;
        }
        self.input.clear();
        self.scroll = 0;

        if self.demo {
            let msg = crate::demo::local_message(
                &channel_id,
                self.store.me.clone(),
                &content,
                self.store.messages.get(&channel_id),
            );
            self.insert_message(msg);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let out = crate::discord::rest::Outgoing {
                content: content.clone(),
                ..Default::default()
            };
            let result = rest
                .send_message(&channel_id, &out)
                .await
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::Sent { content, result });
        });
    }

    /// Inserts a message keeping the selection if the DM list is reordered.
    fn insert_message(&mut self, m: Message) {
        let selected_dm = match (self.guild_sel, self.channel_rows().get(self.channel_sel)) {
            (0, Some(ChannelRow::Channel { id, .. })) => Some(id.clone()),
            _ => None,
        };
        self.store.insert_message(m);
        if let Some(sel) = selected_dm {
            self.channel_sel = self.store.dms.iter().position(|c| c.id == sel).unwrap_or(0);
        }
    }

    fn on_gateway(&mut self, ev: GatewayEvent) {
        match ev {
            GatewayEvent::Connecting => self.conn = ConnState::Connecting,
            GatewayEvent::Disconnected(e) => self.conn = ConnState::Error(e),
            GatewayEvent::Ready(r) => {
                let reload = self.store.apply_ready(*r);
                self.guild_sel = self.guild_sel.min(self.store.guilds.len());
                let rows = self.channel_rows();
                if !rows.get(self.channel_sel).is_some_and(ChannelRow::is_selectable) {
                    self.channel_sel = first_selectable(&rows);
                }
                if let Some(c) = reload {
                    self.fetch_history(c, None);
                }
                self.conn = ConnState::Online;
            }
            GatewayEvent::Resumed => self.conn = ConnState::Online,
            GatewayEvent::GuildCreate(g) => self.store.upsert_guild(*g),
            GatewayEvent::GuildEmojis(u) => self.store.guild_emojis(u),
            GatewayEvent::GuildDelete(g) => {
                if !g.unavailable {
                    let sel = self.selection();
                    self.store.remove_guild(&g.id);
                    self.restore_selection(sel);
                }
            }
            GatewayEvent::ChannelUpdate(c) => {
                let sel = self.selection();
                self.store.upsert_channel(*c);
                self.restore_selection(sel);
            }
            GatewayEvent::ChannelDelete(c) => {
                let sel = self.selection();
                self.store.delete_channel(&c);
                self.restore_selection(sel);
            }
            GatewayEvent::RoleUpdate(r) => {
                let sel = self.selection();
                let r = *r;
                self.store.upsert_role(&r.guild_id, r.role);
                self.restore_selection(sel);
            }
            GatewayEvent::RoleDelete(r) => {
                let sel = self.selection();
                self.store.delete_role(&r.guild_id, &r.role_id);
                self.restore_selection(sel);
            }
            GatewayEvent::MemberUpdate(m) => {
                let sel = self.selection();
                self.store.update_member(*m);
                self.restore_selection(sel);
            }
            GatewayEvent::VoiceState(v) => self.store.apply_voice_state(&v, None),
            GatewayEvent::RelationshipUpsert(r) => self.store.upsert_relationship(*r),
            GatewayEvent::RelationshipRemove(id) => self.store.remove_relationship(&id),
            GatewayEvent::Presence(p) => self.store.apply_presence(&p),
            GatewayEvent::MessageAck(a) => self.store.apply_ack(&a),
            GatewayEvent::GuildNotify(g) => self.store.set_notify_settings(*g),
            GatewayEvent::VoiceServer(_)
            | GatewayEvent::StreamCreate(_)
            | GatewayEvent::StreamServer(_)
            | GatewayEvent::StreamDelete(_)
            | GatewayEvent::SettingsProto(_)
            | GatewayEvent::MemberList(_) => {}
            GatewayEvent::MessageCreate(m) => self.insert_message(*m),
            GatewayEvent::MessageUpdate(u) => self.store.update_message(*u),
            GatewayEvent::MessageDelete(d) => self.store.delete_message(&d),
            GatewayEvent::TypingStart(t) => self.store.typing_start(t),
            GatewayEvent::ReactionAdd(r) => self.store.reaction_add(&r),
            GatewayEvent::ReactionRemove(r) => self.store.reaction_remove(&r),
            GatewayEvent::AuthFailed => self.logout(Some(
                tr(
                    "Session invalid or expired: sign in again.",
                    "Sessione non valida o scaduta: accedi di nuovo.",
                )
                .into(),
            )),
            GatewayEvent::Fatal(e) => {
                self.conn = ConnState::Error(e.clone());
                self.set_error(e);
            }
        }
    }
}

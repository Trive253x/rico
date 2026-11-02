//! Stato dell'applicazione e gestione di tutti gli eventi (tastiera, rete).
//!
//! Regola: nessuna operazione di rete blocca il thread dell'interfaccia.
//! Le richieste partono come task tokio e il risultato torna come `AppEvent`.

use std::collections::{HashMap, HashSet};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

use crate::config;
use crate::discord::{
    gateway::{self, GatewayEvent},
    models::{cmp_snowflake, kind, Channel, Guild, Message, MessageUpdate, Ready, User},
    remote_auth::{self, AuthEvent},
    rest::{Rest, Unauthorized, HISTORY_PAGE},
};
use crate::input::TextInput;

#[derive(Debug)]
pub enum AppEvent {
    Term(Event),
    Gateway(GatewayEvent),
    Auth(AuthEvent),
    TokenChecked(Result<(String, User), String>),
    History {
        channel_id: String,
        older: bool,
        result: Result<Vec<Message>, String>,
    },
    Sent {
        content: String,
        result: Result<Message, String>,
    },
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginTab {
    Qr,
    Token,
}

#[derive(Debug, Clone)]
pub enum QrState {
    Working(String),
    /// Righe del QR già renderizzate in caratteri Unicode.
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
    /// Messaggio mostrato all'apertura (es. "sessione scaduta").
    pub notice: Option<String>,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            tab: LoginTab::Qr,
            qr: QrState::Working("Avvio…".into()),
            token: TextInput::default(),
            token_busy: false,
            token_error: None,
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
            Focus::Guilds => Focus::Channels,
            Focus::Channels => Focus::Messages,
            Focus::Messages => Focus::Input,
            Focus::Input => Focus::Guilds,
        }
    }

    fn prev(self) -> Self {
        match self {
            Focus::Guilds => Focus::Input,
            Focus::Channels => Focus::Guilds,
            Focus::Messages => Focus::Channels,
            Focus::Input => Focus::Messages,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRow {
    Category(String),
    Channel {
        id: String,
        name: String,
        kind: u8,
        selectable: bool,
    },
}

impl ChannelRow {
    fn is_selectable(&self) -> bool {
        matches!(self, ChannelRow::Channel { selectable: true, .. })
    }
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
    pub me: Option<User>,
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    pub users: HashMap<String, User>,
    pub channel_names: HashMap<String, String>,
    channel_guild: HashMap<String, String>,
    /// Messaggi caricati per canale, in ordine cronologico.
    pub messages: HashMap<String, Vec<Message>>,
    pub loading: HashSet<String>,
    /// Canali di cui abbiamo già tutta la cronologia.
    exhausted: HashSet<String>,
    pub unread: HashSet<String>,

    pub focus: Focus,
    /// 0 = messaggi diretti, n = `guilds[n - 1]`.
    pub guild_sel: usize,
    pub channel_sel: usize,
    pub current: Option<String>,
    /// Righe di scorrimento dal fondo (0 = ultimi messaggi).
    pub scroll: usize,
    /// Aggiornati a ogni disegno: limite di scroll e altezza di una pagina.
    pub max_scroll: usize,
    pub page: usize,
    pub input: TextInput,
    /// Messaggio nella barra di stato (testo, è un errore?).
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
            me: None,
            guilds: Vec::new(),
            dms: Vec::new(),
            users: HashMap::new(),
            channel_names: HashMap::new(),
            channel_guild: HashMap::new(),
            messages: HashMap::new(),
            loading: HashSet::new(),
            exhausted: HashSet::new(),
            unread: HashSet::new(),
            focus: Focus::Guilds,
            guild_sel: 0,
            channel_sel: 0,
            current: None,
            scroll: 0,
            max_scroll: 0,
            page: 10,
            input: TextInput::default(),
            status: None,
            guild_list: ListState::default(),
            channel_list: ListState::default(),
        }
    }

    /// Interfaccia popolata con dati finti, senza rete.
    pub fn demo(tx: UnboundedSender<AppEvent>) -> Self {
        let mut app = Self::new(tx, None);
        app.demo = true;
        app.screen = Screen::Main;
        app.on_ready(crate::demo::ready());
        for (channel_id, msgs) in crate::demo::messages() {
            for m in &msgs {
                app.users.insert(m.author.id.clone(), m.author.clone());
            }
            app.exhausted.insert(channel_id.clone());
            app.messages.insert(channel_id, msgs);
        }
        app.unread.insert(crate::demo::UNREAD_CHANNEL.to_string());
        app.status = Some(("Modalità demo: nessuna connessione a Discord".into(), false));
        app
    }

    // ---------------------------------------------------------------- ciclo di vita

    pub fn start(&mut self) {
        if self.demo {
            return;
        }
        match self.token.clone() {
            // Con un token salvato si parte subito: se non è valido lo dirà il gateway.
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
        self.login.qr = QrState::Working("Avvio…".into());
        self.auth = Some(remote_auth::spawn(self.tx.clone()));
    }

    fn start_session(&mut self, token: String) {
        if let Some(h) = self.auth.take() {
            h.abort();
        }
        match Rest::new(&token) {
            Ok(r) => self.rest = Some(r),
            Err(e) => {
                self.set_error(format!("Token non utilizzabile: {e:#}"));
                return;
            }
        }
        if let Some(h) = self.gateway.take() {
            h.abort();
        }
        self.gateway = Some(gateway::spawn(token.clone(), self.tx.clone()));
        self.token = Some(token);
        self.conn = ConnState::Connecting;
        self.screen = Screen::Main;
    }

    fn finish_login(&mut self, token: String, user: Option<User>) {
        let saved = config::save_token(&token);
        self.start_session(token);
        if let Some(u) = user {
            self.status = Some((format!("Accesso effettuato come {}", u.display_name()), false));
            self.me = Some(u);
        }
        if let Err(e) = saved {
            self.set_error(format!("Token non salvato: {e:#}"));
        }
    }

    fn logout(&mut self, notice: Option<String>) {
        if let Some(h) = self.gateway.take() {
            h.abort();
        }
        config::delete_token();
        self.token = None;
        self.rest = None;
        self.me = None;
        self.guilds.clear();
        self.dms.clear();
        self.users.clear();
        self.channel_names.clear();
        self.channel_guild.clear();
        self.messages.clear();
        self.loading.clear();
        self.exhausted.clear();
        self.unread.clear();
        self.current = None;
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
            Screen::Login => matches!(self.login.qr, QrState::Working(_)) || self.login.token_busy,
            Screen::Main => {
                self.conn == ConnState::Connecting
                    || self.current.as_ref().is_some_and(|c| self.loading.contains(c))
            }
        }
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), true));
    }

    // ---------------------------------------------------------------- eventi

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Tick => self.tick = self.tick.wrapping_add(1),
            AppEvent::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            AppEvent::Term(Event::Paste(s)) => self.on_paste(&s),
            AppEvent::Term(_) => {}
            AppEvent::Gateway(g) => self.on_gateway(g),
            AppEvent::Auth(a) => self.on_auth(a),
            AppEvent::TokenChecked(r) => self.on_token_checked(r),
            AppEvent::History {
                channel_id,
                older,
                result,
            } => self.on_history(channel_id, older, result),
            AppEvent::Sent { content, result } => match result {
                Ok(m) => self.insert_message(m),
                Err(e) => {
                    if self.input.is_empty() {
                        self.input.set(&content);
                    }
                    self.set_error(format!("Invio non riuscito: {e}"));
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
            Screen::Main => {
                self.focus = Focus::Input;
                self.input.insert_str(s);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
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
            KeyCode::Tab | KeyCode::BackTab => {
                self.login.tab = match self.login.tab {
                    LoginTab::Qr => LoginTab::Token,
                    LoginTab::Token => LoginTab::Qr,
                };
            }
            _ => match self.login.tab {
                LoginTab::Qr => {
                    if matches!(k.code, KeyCode::Char('r') | KeyCode::Char('R')) {
                        self.start_qr();
                    }
                }
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

    fn submit_token(&mut self) {
        if self.login.token_busy {
            return;
        }
        let token = self.login.token.value().trim().trim_matches('"').to_string();
        if token.is_empty() {
            self.login.token_error = Some("Incolla prima il token.".into());
            return;
        }
        if token.starts_with("Bot ") {
            self.login.token_error =
                Some("I token dei bot non sono supportati: serve il token di un account utente.".into());
            return;
        }
        let rest = match Rest::new(&token) {
            Ok(r) => r,
            Err(e) => {
                self.login.token_error = Some(format!("Token non valido: {e:#}"));
                return;
            }
        };
        self.login.token_busy = true;
        self.login.token_error = None;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match rest.me().await {
                Ok(user) => Ok((token, user)),
                Err(e) if e.is::<Unauthorized>() => Err("Token non valido o scaduto.".to_string()),
                Err(e) => Err(format!("Verifica non riuscita: {e:#}")),
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
                self.login.qr = match crate::ui::qr_lines(&url) {
                    Some(lines) => QrState::Ready(lines),
                    None => QrState::Failed("Impossibile generare il codice QR".into()),
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
                self.logout(Some("Sei uscito dall'account.".into()));
            }
            return;
        }
        // Tasti validi in ogni pannello.
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
                KeyCode::End => self.select_guild(self.guilds.len()),
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

    // ---------------------------------------------------------------- navigazione

    pub fn selected_guild(&self) -> Option<&Guild> {
        self.guild_sel.checked_sub(1).and_then(|i| self.guilds.get(i))
    }

    fn select_guild(&mut self, idx: usize) {
        let idx = idx.min(self.guilds.len());
        if idx != self.guild_sel {
            self.guild_sel = idx;
            self.channel_sel = first_selectable(&self.channel_rows());
            self.channel_list = ListState::default();
        }
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
        self.unread.remove(&id);
        self.scroll = 0;
        self.focus = Focus::Input;
        let need_history =
            !self.exhausted.contains(&id) && self.messages.get(&id).is_none_or(|v| v.is_empty());
        self.current = Some(id.clone());
        if need_history {
            self.fetch_history(id, None);
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let target = (self.scroll as isize).saturating_add(delta).max(0) as usize;
        self.scroll = target.min(self.max_scroll);
        // Arrivati in cima si carica la pagina di cronologia precedente.
        if delta > 0 && self.scroll == self.max_scroll {
            self.load_older();
        }
    }

    fn load_older(&mut self) {
        let Some(channel_id) = self.current.clone() else {
            return;
        };
        if self.exhausted.contains(&channel_id) {
            return;
        }
        let oldest = self
            .messages
            .get(&channel_id)
            .and_then(|v| v.first())
            .map(|m| m.id.clone());
        if let Some(before) = oldest {
            self.fetch_history(channel_id, Some(before));
        }
    }

    /// Righe della colonna canali per il server selezionato (o per i DM).
    pub fn channel_rows(&self) -> Vec<ChannelRow> {
        match self.selected_guild() {
            None => self
                .dms
                .iter()
                .map(|c| ChannelRow::Channel {
                    id: c.id.clone(),
                    name: self.dm_name(c),
                    kind: c.kind,
                    selectable: true,
                })
                .collect(),
            Some(g) => guild_rows(&g.channels),
        }
    }

    pub fn dm_name(&self, c: &Channel) -> String {
        if let Some(name) = c.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        let names: Vec<&str> = if c.recipients.is_empty() {
            c.recipient_ids
                .iter()
                .filter_map(|id| self.users.get(id))
                .map(User::display_name)
                .collect()
        } else {
            c.recipients.iter().map(User::display_name).collect()
        };
        if names.is_empty() {
            "Conversazione".into()
        } else {
            names.join(", ")
        }
    }

    /// Titolo del canale aperto: `#nome` oppure `@persona`.
    pub fn current_title(&self) -> Option<String> {
        let id = self.current.as_deref()?;
        if let Some(dm) = self.dms.iter().find(|c| c.id == id) {
            return Some(format!("@{}", self.dm_name(dm)));
        }
        let name = self.channel_names.get(id).map(String::as_str).unwrap_or("canale");
        let guild = self
            .channel_guild
            .get(id)
            .and_then(|gid| self.guilds.iter().find(|g| &g.id == gid))
            .map(|g| g.name());
        Some(match guild {
            Some(g) => format!("#{name} · {g}"),
            None => format!("#{name}"),
        })
    }

    pub fn guild_has_unread(&self, guild_id: &str) -> bool {
        self.unread
            .iter()
            .any(|c| self.channel_guild.get(c).is_some_and(|g| g == guild_id))
    }

    pub fn dms_have_unread(&self) -> bool {
        self.dms.iter().any(|c| self.unread.contains(&c.id))
    }

    // ---------------------------------------------------------------- dati

    fn fetch_history(&mut self, channel_id: String, before: Option<String>) {
        if self.demo || self.loading.contains(&channel_id) {
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        self.loading.insert(channel_id.clone());
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

    fn on_history(&mut self, channel_id: String, older: bool, result: Result<Vec<Message>, String>) {
        self.loading.remove(&channel_id);
        let mut batch = match result {
            Ok(b) => b,
            Err(e) => {
                self.set_error(format!("Cronologia non disponibile: {e}"));
                return;
            }
        };
        if batch.len() < HISTORY_PAGE {
            self.exhausted.insert(channel_id.clone());
        }
        for m in &batch {
            self.remember_users(m);
        }
        batch.reverse(); // l'API restituisce dal più recente
        let list = self.messages.entry(channel_id).or_default();
        if older {
            batch.retain(|m| !list.iter().any(|x| x.id == m.id));
            batch.append(list);
        } else {
            // Unisce eventuali messaggi arrivati dal gateway durante il caricamento.
            for m in list.drain(..) {
                if !batch.iter().any(|x| x.id == m.id) {
                    batch.push(m);
                }
            }
            batch.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
        *list = batch;
    }

    fn remember_users(&mut self, m: &Message) {
        self.users
            .entry(m.author.id.clone())
            .or_insert_with(|| m.author.clone());
        for u in &m.mentions {
            self.users.entry(u.id.clone()).or_insert_with(|| u.clone());
        }
    }

    fn send_input(&mut self) {
        let Some(channel_id) = self.current.clone() else {
            self.set_error("Apri prima un canale.");
            return;
        };
        let content = self.input.value().trim().to_string();
        if content.is_empty() {
            return;
        }
        if content.chars().count() > 2000 {
            self.set_error("Il messaggio supera i 2000 caratteri.");
            return;
        }
        self.input.clear();
        self.scroll = 0;

        if self.demo {
            let msg = crate::demo::local_message(
                &channel_id,
                self.me.clone(),
                &content,
                self.messages.get(&channel_id),
            );
            self.insert_message(msg);
            return;
        }
        let Some(rest) = self.rest.clone() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = rest
                .send_message(&channel_id, &content)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::Sent { content, result });
        });
    }

    /// Inserisce un messaggio (dal gateway o dalla risposta all'invio), senza duplicati.
    fn insert_message(&mut self, m: Message) {
        self.remember_users(&m);
        let from_me = self.me.as_ref().is_some_and(|u| u.id == m.author.id);
        if self.current.as_deref() != Some(m.channel_id.as_str()) && !from_me {
            self.unread.insert(m.channel_id.clone());
        }
        self.bump_dm(&m.channel_id, &m.id);

        // Se il canale non è mai stato aperto non conserviamo nulla: all'apertura
        // si scarica comunque la cronologia completa.
        let Some(list) = self.messages.get_mut(&m.channel_id) else {
            return;
        };
        if list.iter().rev().take(100).any(|x| x.id == m.id) {
            return;
        }
        let out_of_order = list.last().is_some_and(|l| cmp_snowflake(&l.id, &m.id).is_gt());
        list.push(m);
        if out_of_order {
            list.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
    }

    fn update_message(&mut self, u: MessageUpdate) {
        let Some(m) = self
            .messages
            .get_mut(&u.channel_id)
            .and_then(|l| l.iter_mut().rev().find(|m| m.id == u.id))
        else {
            return;
        };
        if let Some(c) = u.content {
            m.content = c;
        }
        if u.edited_timestamp.is_some() {
            m.edited_timestamp = u.edited_timestamp;
        }
        if let Some(a) = u.attachments {
            m.attachments = a;
        }
    }

    /// Porta in cima alla lista la conversazione privata che ha appena ricevuto un messaggio.
    fn bump_dm(&mut self, channel_id: &str, message_id: &str) {
        let Some(pos) = self.dms.iter().position(|c| c.id == channel_id) else {
            return;
        };
        self.dms[pos].last_message_id = Some(message_id.to_string());
        if pos == 0 {
            return;
        }
        let selected = match (self.guild_sel, self.channel_rows().get(self.channel_sel)) {
            (0, Some(ChannelRow::Channel { id, .. })) => Some(id.clone()),
            _ => None,
        };
        let c = self.dms.remove(pos);
        self.dms.insert(0, c);
        if let Some(sel) = selected {
            self.channel_sel = self.dms.iter().position(|c| c.id == sel).unwrap_or(0);
        }
    }

    fn upsert_guild(&mut self, mut g: Guild) {
        for c in &mut g.channels {
            c.guild_id = Some(g.id.clone());
            self.channel_guild.insert(c.id.clone(), g.id.clone());
            if let Some(n) = &c.name {
                self.channel_names.insert(c.id.clone(), n.clone());
            }
        }
        match self.guilds.iter_mut().find(|x| x.id == g.id) {
            Some(slot) => *slot = g,
            None => self.guilds.push(g),
        }
    }

    fn on_gateway(&mut self, ev: GatewayEvent) {
        match ev {
            GatewayEvent::Connecting => self.conn = ConnState::Connecting,
            GatewayEvent::Disconnected(e) => self.conn = ConnState::Error(e),
            GatewayEvent::Ready(r) => self.on_ready(*r),
            GatewayEvent::Resumed => self.conn = ConnState::Online,
            GatewayEvent::GuildCreate(g) => self.upsert_guild(*g),
            GatewayEvent::MessageCreate(m) => self.insert_message(*m),
            GatewayEvent::MessageUpdate(u) => self.update_message(*u),
            GatewayEvent::MessageDelete(d) => {
                if let Some(list) = self.messages.get_mut(&d.channel_id) {
                    list.retain(|m| m.id != d.id);
                }
            }
            GatewayEvent::AuthFailed => {
                self.logout(Some("Sessione non valida o scaduta: accedi di nuovo.".into()))
            }
            GatewayEvent::Fatal(e) => {
                self.conn = ConnState::Error(e.clone());
                self.set_error(e);
            }
        }
    }

    fn on_ready(&mut self, r: Ready) {
        for u in r.users {
            self.users.insert(u.id.clone(), u);
        }
        self.users.insert(r.user.id.clone(), r.user.clone());
        self.me = Some(r.user);

        self.guilds.clear();
        self.channel_guild.clear();
        self.channel_names.clear();
        for g in r.guilds {
            self.upsert_guild(g);
        }

        let mut dms = r.private_channels;
        for c in &dms {
            for u in &c.recipients {
                self.users.insert(u.id.clone(), u.clone());
            }
        }
        dms.sort_by(|a, b| {
            cmp_snowflake(
                b.last_message_id.as_deref().unwrap_or("0"),
                a.last_message_id.as_deref().unwrap_or("0"),
            )
        });
        self.dms = dms;

        self.guild_sel = self.guild_sel.min(self.guilds.len());
        let rows = self.channel_rows();
        if !rows.get(self.channel_sel).is_some_and(ChannelRow::is_selectable) {
            self.channel_sel = first_selectable(&rows);
        }

        // Dopo un READY (non un RESUME) possono esserci buchi: si ricarica.
        self.messages.clear();
        self.exhausted.clear();
        self.loading.clear();
        if let Some(c) = self.current.clone() {
            self.fetch_history(c, None);
        }
        self.conn = ConnState::Online;
    }
}

fn first_selectable(rows: &[ChannelRow]) -> usize {
    rows.iter().position(ChannelRow::is_selectable).unwrap_or(0)
}

/// Ordina i canali come Discord: prima quelli senza categoria, poi ogni
/// categoria con i suoi canali (testo prima della voce, poi per posizione).
pub fn guild_rows(channels: &[Channel]) -> Vec<ChannelRow> {
    let visible = |c: &&Channel| c.is_text() || c.is_voice();
    let order = |a: &&Channel, b: &&Channel| {
        (a.is_voice(), a.position.unwrap_or(0))
            .cmp(&(b.is_voice(), b.position.unwrap_or(0)))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    };
    let row = |c: &Channel| ChannelRow::Channel {
        id: c.id.clone(),
        name: c.name.clone().unwrap_or_default(),
        kind: c.kind,
        selectable: c.is_text(),
    };

    let mut rows = Vec::new();
    let mut top: Vec<&Channel> = channels
        .iter()
        .filter(|c| c.parent_id.is_none())
        .filter(visible)
        .collect();
    top.sort_by(order);
    rows.extend(top.into_iter().map(row));

    let mut categories: Vec<&Channel> = channels.iter().filter(|c| c.kind == kind::CATEGORY).collect();
    categories.sort_by(|a, b| {
        a.position
            .unwrap_or(0)
            .cmp(&b.position.unwrap_or(0))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    });
    for cat in categories {
        let mut children: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(cat.id.as_str()))
            .filter(visible)
            .collect();
        if children.is_empty() {
            continue;
        }
        children.sort_by(order);
        rows.push(ChannelRow::Category(cat.name.clone().unwrap_or_default()));
        rows.extend(children.into_iter().map(row));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn canali_ordinati_per_categoria() {
        let channels: Vec<Channel> = serde_json::from_value(serde_json::json!([
            {"id": "5", "type": 2, "name": "Voce", "position": 0, "parent_id": "1"},
            {"id": "4", "type": 0, "name": "off-topic", "position": 1, "parent_id": "1"},
            {"id": "3", "type": 0, "name": "generale", "position": 0, "parent_id": "1"},
            {"id": "1", "type": 4, "name": "Testo", "position": 0},
            {"id": "2", "type": 0, "name": "regole", "position": 0},
            {"id": "9", "type": 4, "name": "Vuota", "position": 1},
        ]))
        .unwrap();
        let rows = guild_rows(&channels);
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ChannelRow::Category(n) => format!("[{n}]"),
                ChannelRow::Channel { name, .. } => name.clone(),
            })
            .collect();
        assert_eq!(names, ["regole", "[Testo]", "generale", "off-topic", "Voce"]);
        assert_eq!(first_selectable(&rows), 0);
        assert!(!rows[4].is_selectable());
    }

    #[test]
    fn messaggi_senza_duplicati_e_non_letti() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::demo(tx);
        let channel = crate::demo::UNREAD_CHANNEL.to_string();
        let before = app.messages[&channel].len();
        let msg = crate::demo::local_message(&channel, None, "ciao", app.messages.get(&channel));
        app.handle(AppEvent::Gateway(GatewayEvent::MessageCreate(Box::new(
            msg.clone(),
        ))));
        app.handle(AppEvent::Gateway(GatewayEvent::MessageCreate(Box::new(msg))));
        assert_eq!(app.messages[&channel].len(), before + 1);
        assert!(app.unread.contains(&channel));
    }
}

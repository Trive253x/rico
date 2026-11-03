//! Interfaccia grafica (iced), con il layout del mock "iced in stile gpui".
//!
//! Architettura Elm: lo stato è [`Gui`], ogni evento (clic, tasto, risposta di
//! rete, evento del gateway) è un [`Message`] applicato da `update`, e `view`
//! ricostruisce l'interfaccia dallo stato. Gateway e login QR girano come
//! sottoscrizioni: iced le avvia e le ferma da solo quando lo stato cambia.

mod anim;
mod picker;
mod style;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate};
use futures_util::{SinkExt, Stream};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, opaque, qr_code, rich_text, row, scrollable,
    span, stack, text, text_input, tooltip, Column, Row, Space,
};
use iced::{event, keyboard, mouse, window, ContentFit, Element, Length, Padding, Size, Subscription, Task};

use crate::discord::gateway::{self, GatewayEvent};
use crate::discord::models::{kind, Attachment, GifResult, Message as DMessage, User};
use crate::discord::remote_auth::{self, AuthEvent};
use crate::discord::rest::{self, Rest, Unauthorized};
use crate::store::{first_selectable, ChannelRow, Store};
use crate::text::{initials, segments, Ctx, SegmentKind};
use crate::{config, media, Options};
use anim::{animated, Animation};
use picker::{Gifs, Picker};
use style::*;

/// Anteprime tenute in memoria al massimo (le più vecchie vengono scartate).
/// Restano compresse (poche decine di KB l'una): iced decodifica solo quelle
/// effettivamente disegnate.
const MAX_PREVIEWS: usize = 400;
/// Download di anteprime in corso contemporaneamente.
const MAX_DOWNLOADS: usize = 6;
/// Immagini del canale aperto (dalle più recenti) di cui si scaricano le
/// anteprime. Deve restare sotto `MAX_PREVIEWS`, altrimenti la cache
/// scarterebbe anteprime che verrebbero subito richieste di nuovo.
const MAX_IMAGES_PER_CHANNEL: usize = 300;

/// Messaggi consecutivi dello stesso autore entro questo intervallo vengono raggruppati.
/// Distanza del selettore di emoji e GIF dal fondo della finestra: appena
/// sopra il campo di scrittura.
const PICKER_BOTTOM: f32 = 96.0;
const GROUP_GAP_SECS: i64 = 7 * 60;
const MONTHS: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];

pub fn run(opts: Options) -> iced::Result {
    iced::application(Gui::title, Gui::update, Gui::view)
        .subscription(Gui::subscription)
        .theme(|_| style::theme())
        .settings(iced::Settings {
            default_text_size: iced::Pixels(14.0),
            antialiasing: true,
            ..Default::default()
        })
        .window(window::Settings {
            // Deve stare in uno schermo 1280×800 logici (es. 2560×1600 al 200%)
            // insieme alla barra del titolo e a quella delle applicazioni.
            size: Size::new(1180.0, 700.0),
            min_size: Some(Size::new(800.0, 500.0)),
            position: window::Position::Centered,
            ..Default::default()
        })
        .run_with(move || Gui::new(opts))
}

fn messages_id() -> scrollable::Id {
    scrollable::Id::new("messaggi")
}

fn composer_id() -> text_input::Id {
    text_input::Id::new("composer")
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

pub enum QrView {
    Working(String),
    Ready(qr_code::Data),
    Scanned(String),
    Failed(String),
}

pub struct Login {
    tab: LoginTab,
    qr: QrView,
    /// Cambia a ogni nuovo tentativo: riavvia la sottoscrizione del QR.
    attempt: u64,
    token: String,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
}

impl Default for Login {
    fn default() -> Self {
        Self {
            tab: LoginTab::Qr,
            qr: QrView::Working("Avvio…".into()),
            attempt: 0,
            token: String::new(),
            busy: false,
            error: None,
            notice: None,
        }
    }
}

/// Stato dell'anteprima di un'immagine allegata.
pub enum Preview {
    Loading,
    Ready(img::Handle),
    /// GIF animata, già decodificata e ridotta alla misura dell'anteprima.
    Animated(Animation),
    Failed,
}

/// Risultato del download di un'anteprima.
#[derive(Debug, Clone)]
pub enum Loaded {
    /// Immagine da decodificare (lo fa iced quando la disegna).
    Image(Vec<u8>),
    Animated(Animation),
}

/// Salvataggio dell'originale dalla vista a schermo intero.
pub enum SaveState {
    Idle,
    Saving,
    Saved(PathBuf),
    Failed(String),
}

/// Immagine aperta a schermo intero.
pub struct ImageViewer {
    attachment: Attachment,
    /// Indirizzo della versione grande; `None` se non si può scaricare.
    url: Option<String>,
    image: Preview,
    /// Anteprima già scaricata, mostrata mentre arriva la versione grande.
    preview: Option<img::Handle>,
    save: SaveState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Online,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    LoginTab(LoginTab),
    RetryQr,
    Auth(AuthEvent),
    TokenInput(String),
    SubmitToken,
    TokenChecked(Result<(String, User), String>),
    Gateway(GatewayEvent),
    SelectGuild(Option<usize>),
    OpenChannel(String),
    History {
        channel_id: String,
        older: bool,
        result: Result<Vec<DMessage>, String>,
    },
    ComposerChanged(String),
    Send,
    Sent {
        content: String,
        result: Result<DMessage, String>,
    },
    Scrolled(scrollable::Viewport),
    HoverStart(String),
    HoverEnd(String),
    PreviewLoaded {
        url: String,
        result: Result<Loaded, String>,
    },
    OpenViewer(Box<Attachment>),
    ViewerLoaded {
        url: String,
        result: Result<Vec<u8>, String>,
    },
    CloseViewer,
    SaveAttachment,
    Saved(Result<PathBuf, String>),
    ShowInFolder(PathBuf),
    /// Clic che non deve fare nulla (es. sull'immagine: non chiude la vista).
    Noop,
    /// Pulsanti GIF ed emoji: aprono il selettore (o lo chiudono se già su quella scheda).
    TogglePicker(picker::Tab),
    PickerTab(picker::Tab),
    ClosePicker,
    EmojiQuery(String),
    EmojiSection(picker::Section),
    PickEmoji(&'static str),
    GifQuery(String),
    /// Parte la ricerca se nel frattempo non si è scritto altro.
    GifSearch(u64),
    GifResults {
        seq: u64,
        result: Result<Vec<GifResult>, String>,
    },
    PickGif(String),
    Logout,
    Tick,
}

pub struct Gui {
    demo: bool,
    screen: Screen,
    login: Login,
    token: Option<String>,
    /// Cambia a ogni accesso: riavvia la sottoscrizione del gateway.
    session: u64,
    rest: Option<Rest>,
    conn: Conn,
    store: Store,
    /// Server selezionato (`None` = messaggi diretti).
    guild: Option<usize>,
    composer: String,
    hovered: Option<String>,
    status: Option<(String, bool)>,
    /// Anteprime delle immagini allegate, per indirizzo scaricato.
    previews: HashMap<String, Preview>,
    /// Ordine di arrivo delle anteprime, per scartare le più vecchie.
    preview_order: VecDeque<String>,
    downloads: usize,
    viewer: Option<ImageViewer>,
    picker: Option<Picker>,
    /// Emoji usate di recente (dalla più recente), solo per questa sessione.
    recent_emojis: Vec<&'static str>,
}

impl Gui {
    fn blank() -> Self {
        Self {
            demo: false,
            screen: Screen::Login,
            login: Login::default(),
            token: None,
            session: 0,
            rest: None,
            conn: Conn::Connecting,
            store: Store::default(),
            guild: None,
            composer: String::new(),
            hovered: None,
            status: None,
            previews: HashMap::new(),
            preview_order: VecDeque::new(),
            downloads: 0,
            viewer: None,
            picker: None,
            recent_emojis: Vec::new(),
        }
    }

    pub fn new(opts: Options) -> (Self, Task<Message>) {
        let mut gui = Self::blank();
        if opts.demo_login {
            gui.demo = true;
            if let Ok(data) = qr_code::Data::new("https://discord.com/ra/demo-ferrocord-0000000000000000") {
                gui.login.qr = QrView::Ready(data);
            }
            return (gui, Task::none());
        }
        if opts.demo {
            gui.demo = true;
            gui.screen = Screen::Main;
            gui.conn = Conn::Online;
            gui.store = Store::demo();
            gui.store.fake_typing("1001", "901", "Giulia");
            gui.guild = Some(0);
            gui.status = Some(("Modalità demo: nessuna connessione a Discord".into(), false));
            let task = gui.open_channel("1001".into());
            return (gui, task);
        }
        let task = match config::load_token() {
            // Con un token salvato si parte subito: se non è valido lo dirà il gateway.
            Some(token) => gui.start_session(token),
            None => Task::none(),
        };
        (gui, task)
    }

    fn title(&self) -> String {
        match &self.store.current {
            Some(c) if self.screen == Screen::Main => format!("{} — ferrocord", self.store.title(c)),
            _ => "ferrocord".to_string(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = Vec::new();
        if !self.demo {
            match &self.token {
                Some(token) => subs.push(Subscription::run_with_id(
                    ("gateway", self.session),
                    gateway_stream(token.clone()),
                )),
                None if self.screen == Screen::Login => {
                    subs.push(Subscription::run_with_id(("qr", self.login.attempt), qr_stream()))
                }
                None => {}
            }
        }
        // Esc chiude la vista a schermo intero (anche se il campo di testo ha il fuoco).
        if self.viewer.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseViewer),
                _ => None,
            }));
        }
        // Esc chiude anche il selettore di emoji e GIF.
        if self.picker.is_some() && self.viewer.is_none() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ClosePicker),
                _ => None,
            }));
        }
        // Serve solo a far sparire "sta scrivendo…" allo scadere.
        if self.store.has_typing() {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    // ------------------------------------------------------------ sessione

    fn start_session(&mut self, token: String) -> Task<Message> {
        match Rest::new(&token) {
            Ok(r) => self.rest = Some(r),
            Err(e) => {
                self.login.error = Some(format!("Token non utilizzabile: {e:#}"));
                return Task::none();
            }
        }
        self.token = Some(token);
        self.session += 1;
        self.conn = Conn::Connecting;
        self.screen = Screen::Main;
        Task::none()
    }

    fn finish_login(&mut self, token: String, user: Option<User>) -> Task<Message> {
        if let Err(e) = config::save_token(&token) {
            self.status = Some((format!("Token non salvato: {e:#}"), true));
        }
        let task = self.start_session(token);
        if let Some(u) = user {
            self.store.me = Some(u);
        }
        task
    }

    fn logout(&mut self, notice: &str) {
        config::delete_token();
        let session = self.session + 1;
        let attempt = self.login.attempt + 1;
        *self = Self::blank();
        self.session = session;
        self.login.attempt = attempt;
        self.login.notice = Some(notice.to_string());
    }

    // ------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LoginTab(tab) => self.login.tab = tab,
            Message::RetryQr => {
                self.login.attempt += 1;
                self.login.qr = QrView::Working("Avvio…".into());
            }
            Message::Auth(ev) => return self.on_auth(ev),
            Message::TokenInput(s) => {
                self.login.token = s;
                self.login.error = None;
            }
            Message::SubmitToken => return self.submit_token(),
            Message::TokenChecked(result) => {
                self.login.busy = false;
                match result {
                    Ok((token, user)) => {
                        self.login.token.clear();
                        return self.finish_login(token, Some(user));
                    }
                    Err(e) => self.login.error = Some(e),
                }
            }
            Message::Gateway(ev) => return self.on_gateway(ev),
            Message::SelectGuild(guild) => {
                self.guild = guild;
                let rows = self.store.channel_rows(guild);
                if let Some(ChannelRow::Channel {
                    id, selectable: true, ..
                }) = rows.get(first_selectable(&rows))
                {
                    return self.open_channel(id.clone());
                }
            }
            Message::OpenChannel(id) => return self.open_channel(id),
            Message::History {
                channel_id,
                older,
                result,
            } => match result {
                Ok(batch) => {
                    self.store.apply_history(&channel_id, older, batch);
                    return self.request_previews();
                }
                Err(e) => {
                    self.store.history_failed(&channel_id);
                    self.status = Some((format!("Cronologia non disponibile: {e}"), true));
                }
            },
            Message::ComposerChanged(s) => self.composer = s,
            Message::Send => return self.send(),
            Message::Sent { content, result } => match result {
                Ok(m) => {
                    if self.store.insert_message(m) {
                        return self.request_previews();
                    }
                }
                Err(e) => {
                    if self.composer.is_empty() {
                        self.composer = content;
                    }
                    self.status = Some((format!("Invio non riuscito: {e}"), true));
                }
            },
            Message::Scrolled(viewport) => {
                // Con la vista ancorata in basso l'offset si misura dal fondo:
                // `absolute_offset_reversed` è la distanza dalla cima.
                let taller = viewport.content_bounds().height > viewport.bounds().height;
                if taller && viewport.absolute_offset_reversed().y < 80.0 {
                    if let Some(channel_id) = self.store.current.clone() {
                        if let Some(before) = self.store.older_cursor(&channel_id) {
                            return self.fetch_history(channel_id, Some(before));
                        }
                    }
                }
            }
            Message::HoverStart(id) => self.hovered = Some(id),
            Message::HoverEnd(id) => {
                if self.hovered.as_deref() == Some(id.as_str()) {
                    self.hovered = None;
                }
            }
            Message::PreviewLoaded { url, result } => {
                self.downloads = self.downloads.saturating_sub(1);
                let state = match result {
                    Ok(Loaded::Image(bytes)) => {
                        tracing::debug!("anteprima scaricata ({} KB)", bytes.len() / 1024);
                        Preview::Ready(img::Handle::from_bytes(bytes))
                    }
                    Ok(Loaded::Animated(animation)) => {
                        tracing::debug!("GIF animata scaricata ({} fotogrammi)", animation.len());
                        Preview::Animated(animation)
                    }
                    Err(e) => {
                        tracing::debug!("anteprima non scaricata: {e}");
                        Preview::Failed
                    }
                };
                // Può essere stata scartata nel frattempo (logout, cache piena).
                if let Some(slot) = self.previews.get_mut(&url) {
                    *slot = state;
                }
                return self.request_previews();
            }
            Message::OpenViewer(attachment) => return self.open_viewer(*attachment),
            Message::ViewerLoaded { url, result } => {
                if let Some(v) = self
                    .viewer
                    .as_mut()
                    .filter(|v| v.url.as_deref() == Some(url.as_str()))
                {
                    v.image = match result {
                        Ok(bytes) => {
                            tracing::debug!("immagine intera scaricata ({} KB)", bytes.len() / 1024);
                            Preview::Ready(img::Handle::from_bytes(bytes))
                        }
                        Err(e) => {
                            tracing::debug!("immagine intera non scaricata: {e}");
                            Preview::Failed
                        }
                    };
                }
            }
            Message::CloseViewer => self.viewer = None,
            Message::SaveAttachment => return self.save_attachment(),
            Message::Saved(result) => {
                let state = match result {
                    Ok(path) => {
                        tracing::info!("allegato salvato in {}", path.display());
                        self.status = Some((format!("Salvato in {}", path.display()), false));
                        SaveState::Saved(path)
                    }
                    Err(e) => {
                        self.status = Some((format!("Salvataggio non riuscito: {e}"), true));
                        SaveState::Failed(e)
                    }
                };
                if let Some(v) = self.viewer.as_mut() {
                    v.save = state;
                }
            }
            Message::ShowInFolder(path) => show_in_folder(&path),
            Message::Noop => {}
            Message::Logout => self.logout("Sei uscito dall'account."),
            Message::TogglePicker(tab) => return self.toggle_picker(tab),
            Message::PickerTab(tab) => {
                if let Some(p) = self.picker.as_mut() {
                    p.tab = tab;
                }
                return Task::batch([text_input::focus(picker::search_id()), self.load_gifs()]);
            }
            Message::ClosePicker => {
                self.close_picker();
                return text_input::focus(composer_id());
            }
            Message::EmojiQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.emoji_query = q;
                }
            }
            Message::EmojiSection(section) => {
                if let Some(p) = self.picker.as_mut() {
                    p.section = section;
                    p.emoji_query.clear();
                }
            }
            Message::PickEmoji(emoji) => {
                self.composer.push_str(emoji);
                self.recent_emojis.retain(|e| *e != emoji);
                self.recent_emojis.insert(0, emoji);
                self.recent_emojis.truncate(picker::MAX_RECENT);
                self.close_picker();
                return Task::batch([
                    text_input::focus(composer_id()),
                    text_input::move_cursor_to_end(composer_id()),
                ]);
            }
            Message::GifQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.gif_query = q;
                    p.seq += 1;
                    let seq = p.seq;
                    // Si cerca solo quando si smette di scrivere per un attimo.
                    return Task::perform(tokio::time::sleep(Duration::from_millis(350)), move |_| {
                        Message::GifSearch(seq)
                    });
                }
            }
            Message::GifSearch(seq) => return self.search_gifs(seq),
            Message::GifResults { seq, result } => {
                let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
                    return Task::none();
                };
                p.searching = false;
                let old = std::mem::replace(
                    &mut p.gifs,
                    match result {
                        Ok(list) => Gifs::Ready(list),
                        Err(e) => Gifs::Failed(e),
                    },
                );
                tracing::debug!("GIF: {} risultati", p.shown_gifs().len());
                self.forget_gif_previews(old);
                return self.request_previews();
            }
            Message::PickGif(url) => {
                self.close_picker();
                return Task::batch([self.send_text(url), text_input::focus(composer_id())]);
            }
            Message::Tick => {}
        }
        Task::none()
    }

    fn on_auth(&mut self, ev: AuthEvent) -> Task<Message> {
        if self.screen != Screen::Login {
            return Task::none();
        }
        match ev {
            AuthEvent::Progress(s) => self.login.qr = QrView::Working(s),
            AuthEvent::Qr(url) => {
                self.login.qr = match qr_code::Data::new(url) {
                    Ok(data) => QrView::Ready(data),
                    Err(_) => QrView::Failed("Impossibile generare il codice QR".into()),
                }
            }
            AuthEvent::Scanned(user) => self.login.qr = QrView::Scanned(user),
            AuthEvent::Token(token) => return self.finish_login(token, None),
            AuthEvent::Failed(e) => self.login.qr = QrView::Failed(e),
        }
        Task::none()
    }

    fn submit_token(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let token = match rest::clean_token(&self.login.token) {
            Ok(t) => t,
            Err(e) => {
                self.login.error = Some(e);
                return Task::none();
            }
        };
        let client = match Rest::new(&token) {
            Ok(r) => r,
            Err(e) => {
                self.login.error = Some(format!("Token non valido: {e:#}"));
                return Task::none();
            }
        };
        self.login.busy = true;
        self.login.error = None;
        Task::perform(
            async move {
                match client.me().await {
                    Ok(user) => Ok((token, user)),
                    Err(e) if e.is::<Unauthorized>() => Err("Token non valido o scaduto.".to_string()),
                    Err(e) => Err(format!("Verifica non riuscita: {e:#}")),
                }
            },
            Message::TokenChecked,
        )
    }

    fn on_gateway(&mut self, ev: GatewayEvent) -> Task<Message> {
        match ev {
            GatewayEvent::Connecting => self.conn = Conn::Connecting,
            GatewayEvent::Disconnected(e) => self.conn = Conn::Error(e),
            GatewayEvent::Ready(r) => {
                self.conn = Conn::Online;
                let reload = self.store.apply_ready(*r);
                if self.guild.is_some_and(|g| g >= self.store.guilds.len()) {
                    self.guild = None;
                }
                if let Some(c) = reload {
                    return self.fetch_history(c, None);
                }
            }
            GatewayEvent::Resumed => self.conn = Conn::Online,
            GatewayEvent::GuildCreate(g) => self.store.upsert_guild(*g),
            GatewayEvent::MessageCreate(m) => {
                if self.store.insert_message(*m) {
                    return self.request_previews();
                }
            }
            GatewayEvent::MessageUpdate(u) => {
                self.store.update_message(*u);
                return self.request_previews();
            }
            GatewayEvent::MessageDelete(d) => self.store.delete_message(&d),
            GatewayEvent::TypingStart(t) => self.store.typing_start(t),
            GatewayEvent::ReactionAdd(r) => self.store.reaction_add(&r),
            GatewayEvent::ReactionRemove(r) => self.store.reaction_remove(&r),
            GatewayEvent::AuthFailed => self.logout("Sessione non valida o scaduta: accedi di nuovo."),
            GatewayEvent::Fatal(e) => {
                self.conn = Conn::Error(e.clone());
                self.status = Some((e, true));
            }
        }
        Task::none()
    }

    fn open_channel(&mut self, id: String) -> Task<Message> {
        self.hovered = None;
        let needs_history = self.store.open(&id);
        let mut tasks = vec![
            // Con la vista ancorata in basso, offset 0 = ultimi messaggi.
            scrollable::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 }),
            text_input::focus(composer_id()),
        ];
        if needs_history {
            tasks.push(self.fetch_history(id, None));
        }
        tasks.push(self.request_previews());
        Task::batch(tasks)
    }

    /// Avvia il download delle anteprime che mancano fra i messaggi caricati
    /// del canale aperto, dai più recenti (al massimo `MAX_DOWNLOADS` alla
    /// volta; le altre partono man mano che quelle in corso finiscono).
    /// Comprende i messaggi più vecchi caricati scorrendo verso l'alto.
    fn request_previews(&mut self) -> Task<Message> {
        // (indirizzo, misura a cui decodificare le GIF animate). Prima le GIF
        // del selettore, se è aperto: sono quelle che si stanno guardando.
        let mut wanted: Vec<(String, (u32, u32))> = self
            .picker
            .as_ref()
            .map(|p| {
                p.shown_gifs()
                    .iter()
                    .map(|g| {
                        let (w, h) = picker::gif_size(g);
                        (g.src.clone(), (w as u32, h as u32))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let channel = self.store.current.as_ref().and_then(|c| self.store.messages.get(c));
        wanted.extend(match channel {
            Some(list) => list
                .iter()
                .rev()
                .flat_map(|m| {
                    let attachments = m.attachments.iter().filter(|a| a.is_image()).filter_map(|a| {
                        let size = decode_size(a.width.unwrap_or(1), a.height.unwrap_or(1));
                        preview_key(a).map(|k| (k, size))
                    });
                    let embeds = m.embeds.iter().filter_map(media::embed_preview).filter_map(|e| {
                        let size = decode_size(e.width, e.height);
                        embed_source(&e, &self.previews).map(|k| (k.to_string(), size))
                    });
                    attachments.chain(embeds).collect::<Vec<_>>()
                })
                .take(MAX_IMAGES_PER_CHANNEL)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        });
        let mut tasks = Vec::new();
        for (url, (w, h)) in wanted {
            if self.previews.contains_key(&url) {
                continue;
            }
            if url == crate::demo::DEMO_IMAGE_URL {
                let (w, h) = (960, 600);
                let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
                self.remember_preview(url, Preview::Ready(handle));
                continue;
            }
            if url == crate::demo::DEMO_GIF_URL {
                let state = match anim::demo_animation(320, 200) {
                    Some(a) => Preview::Animated(a),
                    None => Preview::Failed,
                };
                self.remember_preview(url, state);
                continue;
            }
            if self.downloads >= MAX_DOWNLOADS {
                break;
            }
            self.downloads += 1;
            self.remember_preview(url.clone(), Preview::Loading);
            let key = url.clone();
            tasks.push(Task::perform(
                async move { load_preview(url, w, h).await.map_err(|e| format!("{e:#}")) },
                move |result| Message::PreviewLoaded {
                    url: key.clone(),
                    result,
                },
            ));
        }
        if !tasks.is_empty() {
            tracing::debug!("anteprime: {} avviate, {} in corso", tasks.len(), self.downloads);
        }
        Task::batch(tasks)
    }

    fn toggle_picker(&mut self, tab: picker::Tab) -> Task<Message> {
        match self.picker.as_mut() {
            Some(p) if p.tab == tab => {
                self.close_picker();
                return text_input::focus(composer_id());
            }
            Some(p) => p.tab = tab,
            None => self.picker = Some(Picker::new(tab, !self.recent_emojis.is_empty())),
        }
        Task::batch([text_input::focus(picker::search_id()), self.load_gifs()])
    }

    /// Alla prima apertura della scheda GIF carica quelle di tendenza.
    fn load_gifs(&mut self) -> Task<Message> {
        match self.picker.as_mut() {
            Some(p) if p.tab == picker::Tab::Gif && matches!(p.gifs, Gifs::NotLoaded) => {
                p.gifs = Gifs::Loading;
                let seq = p.seq;
                self.search_gifs(seq)
            }
            _ => Task::none(),
        }
    }

    fn search_gifs(&mut self, seq: u64) -> Task<Message> {
        let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
            return Task::none();
        };
        p.searching = true;
        if self.demo {
            let result = Ok(crate::demo::gif_results(&p.gif_query));
            return self.update(Message::GifResults { seq, result });
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let query = p.gif_query.clone();
        Task::perform(
            async move { client.gifs(&query).await.map_err(|e| format!("{e:#}")) },
            move |result| Message::GifResults { seq, result },
        )
    }

    fn close_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            self.forget_gif_previews(p.gifs);
        }
    }

    /// Le anteprime delle GIF del selettore non servono più: si liberano subito
    /// (sono animazioni già decodificate, occupano parecchia memoria).
    fn forget_gif_previews(&mut self, gifs: Gifs) {
        let Gifs::Ready(list) = gifs else {
            return;
        };
        for g in list {
            if g.src != crate::demo::DEMO_GIF_URL {
                self.previews.remove(&g.src);
            }
        }
        let previews = &self.previews;
        self.preview_order.retain(|url| previews.contains_key(url));
    }

    fn open_viewer(&mut self, attachment: Attachment) -> Task<Message> {
        let preview = preview_key(&attachment).and_then(|k| match self.previews.get(&k) {
            Some(Preview::Ready(h)) => Some(h.clone()),
            _ => None,
        });
        if attachment.url == crate::demo::DEMO_IMAGE_URL {
            let (w, h) = (1920, 1200);
            let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
            self.viewer = Some(ImageViewer {
                attachment,
                url: None,
                image: Preview::Ready(handle),
                preview,
                save: SaveState::Idle,
            });
            return Task::none();
        }
        let url = media::viewer_url(&attachment);
        self.viewer = Some(ImageViewer {
            attachment,
            url: url.clone(),
            image: if url.is_some() {
                Preview::Loading
            } else {
                Preview::Failed
            },
            preview,
            save: SaveState::Idle,
        });
        let Some(url) = url else {
            return Task::none();
        };
        let key = url.clone();
        Task::perform(
            async move {
                media::fetch(&url, media::VIEWER_MAX_BYTES)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::ViewerLoaded {
                url: key.clone(),
                result,
            },
        )
    }

    fn save_attachment(&mut self) -> Task<Message> {
        let demo = self.demo;
        let Some(v) = self.viewer.as_mut() else {
            return Task::none();
        };
        if matches!(v.save, SaveState::Saving) {
            return Task::none();
        }
        if demo || v.attachment.url == crate::demo::DEMO_IMAGE_URL {
            self.status = Some(("Nella demo non si scarica nulla.".into(), false));
            return Task::none();
        }
        // L'originale sta sulla CDN; il media proxy è la riserva.
        let url = if !v.attachment.url.is_empty() {
            v.attachment.url.clone()
        } else {
            v.attachment.proxy_url.clone()
        };
        let filename = v.attachment.filename.clone();
        v.save = SaveState::Saving;
        Task::perform(
            async move {
                media::save_to(&url, &media::download_dir(), &filename)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Saved,
        )
    }

    fn view_viewer<'a>(&'a self, v: &'a ImageViewer) -> Element<'a, Message> {
        let a = &v.attachment;
        let mut info = row![
            text(a.filename.clone()).font(SEMIBOLD).shaping(Shaping::Advanced),
            text(human_size(a.size)).size(12).color(MUTED),
        ]
        .spacing(10)
        .align_y(Vertical::Center);
        if let (Some(w), Some(h)) = (a.width, a.height) {
            info = info.push(text(format!("{w}×{h}")).size(12).color(MUTED));
        }

        let save: Element<'a, Message> = match &v.save {
            SaveState::Idle | SaveState::Failed(_) => button(text("Scarica").font(SEMIBOLD))
                .on_press(Message::SaveAttachment)
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saving => button(text("Salvataggio…"))
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saved(path) => button(text("Mostra nella cartella"))
                .on_press(Message::ShowInFolder(path.clone()))
                .padding([6, 14])
                .style(ghost)
                .into(),
        };
        let close = button(text("Chiudi (Esc)"))
            .on_press(Message::CloseViewer)
            .padding([6, 14])
            .style(ghost);
        let top = container(
            row![info, horizontal_space(), save, close]
                .spacing(10)
                .align_y(Vertical::Center),
        )
        .padding([10, 16])
        .width(Length::Fill)
        .style(fill(PANEL));

        let mut footer = None;
        let body: Element<'a, Message> = match (&v.image, &v.preview) {
            (Preview::Ready(handle), _) => img::Viewer::new(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .min_scale(0.5)
                .max_scale(8.0)
                .into(),
            (Preview::Loading, Some(small)) => {
                footer = Some("Caricamento della versione grande…");
                img::Image::new(small.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Contain)
                    .into()
            }
            (Preview::Loading, None) => text("Caricamento…").color(MUTED).into(),
            (Preview::Animated(animation), _) => animated(animation)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            (Preview::Failed, _) => text("Immagine non disponibile: puoi comunque scaricarla.")
                .color(MUTED)
                .into(),
        };
        if let SaveState::Failed(e) = &v.save {
            footer = Some(e.as_str());
        }
        // Clic sull'immagine: non deve chiudere la vista.
        let picture = mouse_area(
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .on_press(Message::Noop);
        let mut panel = column![top, picture].height(Length::Fill);
        let hint = footer.unwrap_or("Rotellina per lo zoom, trascina per spostare");
        panel = panel.push(
            container(text(hint).size(12).color(MUTED).shaping(Shaping::Advanced))
                .padding([6, 16])
                .width(Length::Fill)
                .style(fill(PANEL)),
        );

        // Lo sfondo scuro intorno chiude la vista; `opaque` blocca i clic
        // verso la chat sottostante.
        opaque(
            mouse_area(
                container(container(panel).style(style::outlined))
                    .padding(32)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(style::backdrop),
            )
            .on_press(Message::CloseViewer),
        )
    }

    fn remember_preview(&mut self, url: String, state: Preview) {
        self.previews.insert(url.clone(), state);
        self.preview_order.push_back(url);
        // Scarta le più vecchie già concluse; quelle in download restano.
        let mut checked = 0;
        while self.previews.len() > MAX_PREVIEWS && checked < self.preview_order.len() {
            let Some(old) = self.preview_order.pop_front() else {
                break;
            };
            if matches!(self.previews.get(&old), Some(Preview::Loading)) {
                self.preview_order.push_back(old);
                checked += 1;
            } else {
                self.previews.remove(&old);
            }
        }
    }

    fn fetch_history(&mut self, channel_id: String, before: Option<String>) -> Task<Message> {
        if self.demo {
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        if !self.store.begin_loading(&channel_id) {
            return Task::none();
        }
        let older = before.is_some();
        let id = channel_id.clone();
        Task::perform(
            async move {
                client
                    .messages(&channel_id, before.as_deref())
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::History {
                channel_id: id.clone(),
                older,
                result,
            },
        )
    }

    fn send(&mut self) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        let content = self.composer.trim().to_string();
        if content.is_empty() {
            return Task::none();
        }
        if content.chars().count() > 2000 {
            self.status = Some(("Il messaggio supera i 2000 caratteri.".into(), true));
            return Task::none();
        }
        self.composer.clear();
        self.send_text(content)
    }

    /// Invia un testo nel canale aperto (anche il link di una GIF scelta nel selettore).
    fn send_text(&mut self, content: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        self.status = None;
        let bottom = scrollable::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 });

        if self.demo {
            let mut msg = crate::demo::local_message(
                &channel_id,
                self.store.me.clone(),
                &content,
                self.store.messages.get(&channel_id),
            );
            // Nella demo le GIF scelte nel selettore diventano subito un embed.
            if let Some(embed) = crate::demo::gif_embed(&content) {
                msg.embeds = vec![embed];
            }
            self.store.insert_message(msg);
            return Task::batch([bottom, self.request_previews()]);
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let text = content.clone();
        let sent = Task::perform(
            async move {
                client
                    .send_message(&channel_id, &text)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::Sent {
                content: content.clone(),
                result,
            },
        );
        Task::batch([sent, bottom])
    }

    // ------------------------------------------------------------ view

    pub fn view(&self) -> Element<'_, Message> {
        let screen = match self.screen {
            Screen::Login => self.view_login(),
            Screen::Main => self.view_main(),
        };
        // Sfondo esplicito: quello della finestra non segue sempre il tema.
        let base: Element<'_, Message> = container(screen)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(fill(BG))
            .into();
        // La radice è sempre uno `stack` con la chat come primo livello, anche
        // senza la vista a schermo intero: se la struttura cambiasse quando la
        // vista si apre o si chiude, iced ricreerebbe la lista dei messaggi e
        // la posizione di scorrimento tornerebbe in fondo.
        let mut layers = stack![base];
        if let Some(p) = self.picker.as_ref().filter(|_| self.screen == Screen::Main) {
            // Il pannello sta sopra il campo di scrittura, a destra; un clic
            // fuori lo chiude (`opaque` impedisce che i clic sul pannello passino sotto).
            let panel = container(opaque(picker::view(p, &self.previews, &self.recent_emojis)))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Right)
                .align_y(Vertical::Bottom)
                .padding(Padding {
                    top: 0.0,
                    right: 16.0,
                    bottom: PICKER_BOTTOM,
                    left: 0.0,
                });
            layers = layers.push(stack![
                mouse_area(Space::new(Length::Fill, Length::Fill)).on_press(Message::ClosePicker),
                panel
            ]);
        }
        if let Some(v) = self.viewer.as_ref().filter(|_| self.screen == Screen::Main) {
            layers = layers.push(self.view_viewer(v));
        }
        layers.into()
    }

    fn view_main(&self) -> Element<'_, Message> {
        column![
            self.top_bar(),
            hline(BORDER),
            row![
                self.server_rail(),
                vline(SOFT),
                self.channel_panel(),
                vline(BORDER),
                self.chat(),
            ]
            .height(Length::Fill),
            hline(BORDER),
            self.status_bar(),
        ]
        .into()
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let mut crumbs = row![text("ferrocord").font(BOLD)]
            .spacing(10)
            .align_y(Vertical::Center);
        if let Some(c) = &self.store.current {
            let place = match self.store.guild_of(c) {
                Some(g) => g.name().to_string(),
                None => "Messaggi diretti".to_string(),
            };
            crumbs = crumbs
                .push(text(place).size(13).color(MUTED).shaping(Shaping::Advanced))
                .push(text("/").size(13).color(MUTED))
                .push(
                    text(self.store.channel_label(c))
                        .size(13)
                        .shaping(Shaping::Advanced),
                );
        }
        let me = self.store.me.as_ref();
        let mut right = row![].spacing(10).align_y(Vertical::Center);
        if let Some(u) = me {
            right = right.push(avatar(u.display_name(), &u.id, 22.0));
        }
        if !self.demo {
            right = right.push(
                button(text("Esci").size(12))
                    .on_press(Message::Logout)
                    .padding([3, 10])
                    .style(ghost),
            );
        }
        container(row![crumbs, horizontal_space(), right].align_y(Vertical::Center))
            .padding([0, 12])
            .height(36)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(TITLE))
            .into()
    }

    fn server_rail(&self) -> Element<'_, Message> {
        let mut col = Column::new()
            .spacing(10)
            .padding(Padding {
                top: 12.0,
                right: 0.0,
                bottom: 12.0,
                left: 0.0,
            })
            .align_x(Horizontal::Center)
            .width(Length::Fill);
        col = col.push(rail_button(
            "DM".to_string(),
            "Messaggi diretti".to_string(),
            self.guild.is_none(),
            self.store.dms_have_unread(),
            Message::SelectGuild(None),
        ));
        col = col.push(container(Space::new(24, 1)).style(fill(BORDER)));
        for (i, g) in self.store.guilds.iter().enumerate() {
            col = col.push(rail_button(
                initials(g.name()),
                g.name().to_string(),
                self.guild == Some(i),
                self.store.guild_has_unread(&g.id),
                Message::SelectGuild(Some(i)),
            ));
        }
        container(scrollable(col).direction(thin()).style(thin_scrollbar))
            .width(64)
            .height(Length::Fill)
            .style(fill(PANEL))
            .into()
    }

    fn channel_panel(&self) -> Element<'_, Message> {
        let title = match self.guild.and_then(|i| self.store.guilds.get(i)) {
            Some(g) => g.name().to_string(),
            None => "Messaggi diretti".to_string(),
        };
        let header = container(text(title).font(BOLD).shaping(Shaping::Advanced))
            .padding([0, 12])
            .height(40)
            .width(Length::Fill)
            .align_y(Vertical::Center);

        let mut list = Column::new().spacing(1).padding([8, 6]);
        let rows = self.store.channel_rows(self.guild);
        if rows.is_empty() {
            let msg = if self.conn == Conn::Online || self.demo {
                "Nessun canale"
            } else {
                "Caricamento…"
            };
            list = list.push(container(text(msg).size(13).color(MUTED)).padding(8));
        }
        for r in rows {
            match r {
                ChannelRow::Category(name) => {
                    list = list.push(
                        container(text(name.to_uppercase()).size(11).font(BOLD).color(MUTED)).padding(
                            Padding {
                                top: 12.0,
                                right: 4.0,
                                bottom: 4.0,
                                left: 6.0,
                            },
                        ),
                    );
                }
                ChannelRow::Channel {
                    id,
                    name,
                    kind: k,
                    selectable,
                } => {
                    let selected = self.store.current.as_deref() == Some(id.as_str());
                    let unread = self.store.unread.contains(&id);
                    let mut label = text(name).size(13).shaping(Shaping::Advanced);
                    if unread {
                        label = label.font(BOLD);
                    } else if !selected {
                        label = label.color(MUTED);
                    }
                    let mut line = row![
                        text(channel_icon(k))
                            .size(13)
                            .color(MUTED)
                            .width(16)
                            .shaping(Shaping::Advanced),
                        label
                    ]
                    .spacing(4)
                    .align_y(Vertical::Center);
                    if unread {
                        line = line.push(horizontal_space()).push(dot(ACCENT, 6.0));
                    }
                    let item = button(line)
                        .width(Length::Fill)
                        .padding([4, 8])
                        .style(list_item(selected))
                        .on_press_maybe(selectable.then(|| Message::OpenChannel(id.clone())));
                    list = list.push(item);
                }
            }
        }

        let user_panel: Element<'_, Message> = match &self.store.me {
            Some(me) => container(
                row![
                    avatar(me.display_name(), &me.id, 28.0),
                    column![
                        text(me.display_name().to_string())
                            .size(13)
                            .font(SEMIBOLD)
                            .shaping(Shaping::Advanced),
                        text(match &self.conn {
                            Conn::Online => "online",
                            Conn::Connecting => "connessione…",
                            Conn::Error(_) => "disconnesso",
                        })
                        .size(11)
                        .color(MUTED),
                    ]
                    .spacing(1),
                ]
                .spacing(10)
                .align_y(Vertical::Center),
            )
            .padding([0, 12])
            .height(52)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .into(),
            None => Space::new(Length::Fill, 52).into(),
        };

        container(column![
            header,
            hline(SOFT),
            scrollable(list)
                .direction(thin())
                .style(thin_scrollbar)
                .height(Length::Fill),
            hline(SOFT),
            user_panel,
        ])
        .width(240)
        .height(Length::Fill)
        .style(fill(PANEL))
        .into()
    }

    fn chat(&self) -> Element<'_, Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return container(
                column![
                    text("Benvenuto in ferrocord").size(18).font(BOLD),
                    text("Scegli un server a sinistra, poi un canale.").color(MUTED),
                ]
                .spacing(8)
                .align_x(Horizontal::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into();
        };

        let topic = match self.store.guild_of(&channel_id) {
            Some(g) => g.name().to_string(),
            None => "Messaggi diretti".to_string(),
        };
        let header = container(
            row![
                text(self.store.channel_label(&channel_id))
                    .font(BOLD)
                    .shaping(Shaping::Advanced),
                text(topic).size(13).color(MUTED),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
        )
        .padding([0, 16])
        .height(44)
        .width(Length::Fill)
        .align_y(Vertical::Center);

        let body: Element<'_, Message> = match self.store.messages.get(&channel_id) {
            Some(list) if !list.is_empty() => scrollable(self.message_list(list))
                .id(messages_id())
                // `direction` sostituisce la scrollbar intera, ancoraggio
                // compreso: va impostata prima di `anchor_bottom`.
                .direction(thin())
                .anchor_bottom()
                .on_scroll(Message::Scrolled)
                .style(thin_scrollbar)
                .height(Length::Fill)
                .into(),
            _ => {
                let msg = if self.store.loading.contains(&channel_id) {
                    "Caricamento dei messaggi…"
                } else {
                    "Nessun messaggio: scrivi il primo!"
                };
                container(text(msg).color(MUTED))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center)
                    .into()
            }
        };

        let placeholder = format!("Scrivi in {}", self.store.channel_label(&channel_id));
        let composer = row![
            text_input(&placeholder, &self.composer)
                .id(composer_id())
                .on_input(Message::ComposerChanged)
                .on_submit(Message::Send)
                .padding(11)
                .style(input),
            self.picker_button(text("GIF").size(13).font(BOLD).into(), picker::Tab::Gif, "GIF"),
            self.picker_button(
                text("🙂").size(19).shaping(Shaping::Advanced).into(),
                picker::Tab::Emoji,
                "Emoji",
            ),
            button(text("Invia").font(SEMIBOLD))
                .on_press_maybe((!self.composer.trim().is_empty()).then_some(Message::Send))
                .padding([10, 16])
                .style(primary),
        ]
        .spacing(8)
        .align_y(Vertical::Center);

        let typing = self.store.typing_in(&channel_id);
        let typing_line = match typing.as_slice() {
            [] => String::new(),
            [one] => format!("{one} sta scrivendo…"),
            [a, b] => format!("{a} e {b} stanno scrivendo…"),
            _ => "Più persone stanno scrivendo…".to_string(),
        };

        column![
            header,
            hline(SOFT),
            body,
            container(composer).padding(Padding {
                top: 8.0,
                right: 16.0,
                bottom: 2.0,
                left: 16.0,
            }),
            container(text(typing_line).size(12).color(MUTED).shaping(Shaping::Advanced))
                .padding([0, 20])
                .height(22),
        ]
        .width(Length::Fill)
        .into()
    }

    fn picker_button<'a>(
        &self,
        content: Element<'a, Message>,
        tab: picker::Tab,
        tip: &'a str,
    ) -> Element<'a, Message> {
        let active = self.picker.as_ref().is_some_and(|p| p.tab == tab);
        tooltip(
            button(container(content).center_x(26).center_y(24))
                .on_press(Message::TogglePicker(tab))
                .padding([8, 8])
                .style(style::tab(active)),
            text(tip).size(12),
            tooltip::Position::Top,
        )
        .style(style::tooltip)
        .into()
    }

    fn message_list<'a>(&'a self, list: &'a [DMessage]) -> Element<'a, Message> {
        let ctx = Ctx {
            users: &self.store.users,
            channels: &self.store.channel_names,
            me: self.store.me_id(),
        };
        let mut col = Column::new().padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 8.0,
            left: 0.0,
        });
        let mut prev_author: Option<&str> = None;
        let mut prev_time: Option<DateTime<Local>> = None;
        let mut prev_day: Option<NaiveDate> = None;

        for m in list {
            let ts = parse_ts(&m.timestamp);
            let day = ts.map(|t| t.date_naive());
            if let Some(d) = day.filter(|d| Some(*d) != prev_day) {
                col = col.push(day_separator(d));
                prev_author = None;
            }
            let grouped = prev_author == Some(m.author.id.as_str())
                && m.referenced_message.is_none()
                && matches!((prev_time, ts), (Some(a), Some(b)) if (b - a).num_seconds() < GROUP_GAP_SECS);

            let mentions_me = ctx
                .me
                .is_some_and(|me| m.mention_everyone || m.mentions.iter().any(|u| u.id == me));
            let content = message_body(m, &ctx, &self.previews);

            let inner: Element<'a, Message> = if grouped {
                row![Space::with_width(44), content].into()
            } else {
                let name = m.author.display_name();
                let time = ts.map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
                let mut right = Column::new().spacing(2);
                if let Some(r) = &m.referenced_message {
                    right = right.push(reply_preview(r, &ctx));
                }
                right = right
                    .push(
                        row![
                            text(name.to_string())
                                .font(SEMIBOLD)
                                .shaping(Shaping::Advanced)
                                .color(user_color(&m.author.id)),
                            text(time).size(11).color(MUTED),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .push(content);
                row![avatar(name, &m.author.id, 32.0), right].spacing(12).into()
            };

            let hovered = self.hovered.as_deref() == Some(m.id.as_str());
            let background = if mentions_me {
                Some(MENTION_BG)
            } else if hovered {
                Some(PANEL)
            } else {
                None
            };
            let top = if grouped { 1.0 } else { 8.0 };
            let item = container(inner)
                .width(Length::Fill)
                .padding(Padding {
                    top,
                    right: 12.0,
                    bottom: 2.0,
                    left: 16.0,
                })
                .style(move |_| container::Style {
                    background: background.map(iced::Background::Color),
                    ..Default::default()
                });
            col = col.push(
                mouse_area(item)
                    .on_enter(Message::HoverStart(m.id.clone()))
                    .on_exit(Message::HoverEnd(m.id.clone())),
            );

            prev_author = Some(m.author.id.as_str());
            prev_time = ts;
            prev_day = day;
        }
        col.into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let (color, label) = match &self.conn {
            _ if self.demo => (style::YELLOW, "Demo".to_string()),
            Conn::Online => (GREEN, "Connesso".to_string()),
            Conn::Connecting => (YELLOW, "Connessione…".to_string()),
            Conn::Error(e) => (RED, format!("Disconnesso: {e} — riprovo…")),
        };
        let mut left = row![dot(color, 8.0), text(label).size(12).color(MUTED)]
            .spacing(6)
            .align_y(Vertical::Center);
        if let Some((msg, is_error)) = &self.status {
            left = left.push(text("·").size(12).color(MUTED)).push(
                text(msg.clone())
                    .size(12)
                    .color(if *is_error { RED } else { ACCENT }),
            );
        }
        container(left)
            .padding([0, 10])
            .height(24)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(PANEL))
            .into()
    }

    fn view_login(&self) -> Element<'_, Message> {
        let tabs = row![
            button(text("QR code").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Qr))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Qr)),
            button(text("Token").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Token))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Token)),
        ]
        .spacing(8);

        let body: Element<'_, Message> = match self.login.tab {
            LoginTab::Qr => match &self.login.qr {
                QrView::Working(msg) => text(msg.clone()).color(MUTED).into(),
                QrView::Ready(data) => column![
                    container(qr_code(data).cell_size(5).style(|_| qr_code::Style {
                        cell: iced::Color::BLACK,
                        background: iced::Color::WHITE,
                    }))
                    .padding(12)
                    .style(rounded(iced::Color::WHITE, 10.0)),
                    text("Inquadra il codice con l'app Discord sul telefono"),
                    text("Impostazioni → Scansiona codice QR").size(12).color(MUTED),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Scanned(user) => column![
                    text(format!("✓ Codice scansionato da {user}")).color(GREEN),
                    text("Conferma l'accesso sul telefono…").color(MUTED),
                ]
                .spacing(8)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Failed(err) => column![
                    text(err.clone()).color(RED),
                    button(text("Nuovo codice"))
                        .on_press(Message::RetryQr)
                        .padding([6, 14])
                        .style(ghost),
                    text("Oppure usa il token.").size(12).color(MUTED),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
            },
            LoginTab::Token => {
                let path = config::token_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "?".into());
                let mut col = column![
                    text("Incolla il token del tuo account."),
                    text_input("Token", &self.login.token)
                        .secure(true)
                        .on_input(Message::TokenInput)
                        .on_submit(Message::SubmitToken)
                        .padding(10)
                        .style(input),
                    button(text(if self.login.busy { "Verifica…" } else { "Accedi" }).font(SEMIBOLD))
                        .on_press_maybe((!self.login.busy).then_some(Message::SubmitToken))
                        .padding([8, 18])
                        .style(primary),
                    text(format!("Verrà salvato in {path}, leggibile solo dal tuo utente."))
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(12)
                .align_x(Horizontal::Center);
                if let Some(e) = &self.login.error {
                    col = col.push(text(e.clone()).color(RED));
                }
                col.into()
            }
        };

        let mut card = column![text("ferrocord").size(28).font(BOLD), tabs]
            .spacing(18)
            .align_x(Horizontal::Center)
            .width(420);
        if let Some(n) = &self.login.notice {
            card = card.push(text(n.clone()).color(YELLOW));
        }
        card = card
            .push(container(body).width(Length::Fill).align_x(Horizontal::Center))
            .push(
                text("Client non ufficiale: usarlo può violare i Termini di Servizio di Discord.")
                    .size(11)
                    .color(MUTED),
            );

        container(container(card).padding(28).style(style::card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    }
}

// ---------------------------------------------------------------- sottoscrizioni

/// Gateway come flusso di messaggi: si ferma da solo quando iced scarta la sottoscrizione.
fn gateway_stream(token: String) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<GatewayEvent>();
        let worker = gateway::run(token, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Gateway(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

fn qr_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(16, |mut output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AuthEvent>();
        let worker = remote_auth::run(tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Auth(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

// ---------------------------------------------------------------- pezzi di interfaccia

fn thin() -> scrollable::Direction {
    scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(6).scroller_width(6).margin(2))
}

fn hline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new(Length::Fill, 1)).style(fill(color)).into()
}

fn vline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new(1, Length::Fill)).style(fill(color)).into()
}

fn dot<'a>(color: iced::Color, size: f32) -> Element<'a, Message> {
    container(Space::new(size, size))
        .style(rounded(color, size / 2.0))
        .into()
}

fn avatar<'a>(name: &str, id: &str, size: f32) -> Element<'a, Message> {
    container(
        text(initials(name))
            .size(size * 0.42)
            .font(SEMIBOLD)
            .color(BG)
            .shaping(Shaping::Advanced),
    )
    .width(size)
    .height(size)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(rounded(user_color(id), size / 2.0))
    .into()
}

fn rail_button<'a>(
    label: String,
    name: String,
    selected: bool,
    unread: bool,
    on_press: Message,
) -> Element<'a, Message> {
    // Barretta a sinistra: alta se selezionato, un puntino se ci sono non letti.
    let marker_height = if selected {
        22.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    let marker = container(Space::new(3, marker_height)).style(rounded(TEXT, 2.0));
    let icon = button(
        container(text(label).size(13).font(SEMIBOLD).shaping(Shaping::Advanced))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center),
    )
    .width(40)
    .height(40)
    .padding(0)
    .style(server(selected))
    .on_press(on_press);
    let with_tip = tooltip(
        icon,
        text(name).size(12).shaping(Shaping::Advanced),
        tooltip::Position::Right,
    )
    .padding(6)
    .style(style::tooltip);
    row![marker, Space::with_width(7), with_tip, Space::with_width(10)]
        .align_y(Vertical::Center)
        .into()
}

fn channel_icon(k: u8) -> &'static str {
    match k {
        kind::GUILD_VOICE | kind::STAGE => "♪",
        kind::ANNOUNCEMENT => "»",
        kind::DM | kind::GROUP_DM => "@",
        _ => "#",
    }
}

fn parse_ts(ts: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&Local))
}

fn day_separator<'a>(day: NaiveDate) -> Element<'a, Message> {
    use chrono::Datelike;
    let label = format!("{} {} {}", day.day(), MONTHS[day.month0() as usize], day.year());
    container(
        row![hline(SOFT), text(label).size(12).color(MUTED), hline(SOFT)]
            .spacing(10)
            .align_y(Vertical::Center),
    )
    .padding(Padding {
        top: 10.0,
        right: 4.0,
        bottom: 2.0,
        left: 16.0,
    })
    .into()
}

fn reply_preview<'a>(r: &crate::discord::models::MessagePreview, ctx: &Ctx) -> Element<'a, Message> {
    let who = r
        .author
        .as_ref()
        .map(|u| u.display_name())
        .unwrap_or("?")
        .to_string();
    let full = crate::text::render_content(&r.content, ctx).replace('\n', " ");
    let mut content: String = full.chars().take(90).collect();
    if content.len() < full.len() {
        content.push('…');
    }
    row![
        text("↳").size(12).color(MUTED).shaping(Shaping::Advanced),
        text(format!("{who}: {content}"))
            .size(12)
            .color(MUTED)
            .shaping(Shaping::Advanced),
    ]
    .spacing(6)
    .into()
}

fn message_body<'a>(m: &DMessage, ctx: &Ctx, previews: &'a HashMap<String, Preview>) -> Element<'a, Message> {
    let mut col = Column::new().spacing(6);
    let embeds: Vec<media::EmbedPreview> = m.embeds.iter().filter_map(media::embed_preview).collect();
    // Come Discord: se il messaggio è solo il link di una GIF, si vede solo la GIF.
    let only_link = embeds
        .iter()
        .any(|e| e.link.as_deref() == Some(m.content.trim()) && !m.content.trim().is_empty());

    if !m.content.is_empty() && !only_link {
        let mut spans: Vec<text::Span<'static, Message, iced::Font>> = segments(&m.content, ctx)
            .into_iter()
            .map(|seg| match seg.kind {
                SegmentKind::Plain => span(seg.text),
                SegmentKind::Mention(mine) => span(seg.text)
                    .font(SEMIBOLD)
                    .color(if mine { YELLOW } else { ACCENT })
                    .background(CHIP),
                SegmentKind::Channel => span(seg.text).color(ACCENT).background(CHIP),
                SegmentKind::Emoji => span(seg.text).color(YELLOW),
                SegmentKind::Time => span(seg.text).color(MUTED),
            })
            .collect();
        if m.edited_timestamp.is_some() {
            spans.push(span(" (modificato)").size(11).color(MUTED));
        }
        col = col.push(rich_text(spans).size(14));
    } else if m.attachments.is_empty() && embeds.is_empty() {
        let note = if m.kind == 0 {
            "(messaggio vuoto)"
        } else {
            "(messaggio di sistema)"
        };
        col = col.push(text(note).size(13).color(MUTED));
    }

    for a in &m.attachments {
        if a.is_image() {
            let state = preview_key(a).and_then(|k| previews.get(&k));
            if !matches!(state, Some(Preview::Failed)) {
                col = col.push(image_preview(a, state));
                continue;
            }
        }
        col = col.push(
            container(
                row![
                    text("Allegato").size(11).color(MUTED),
                    text(a.filename.clone())
                        .size(13)
                        .color(ACCENT)
                        .shaping(Shaping::Advanced),
                    text(human_size(a.size)).size(11).color(MUTED),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            )
            .padding([8, 12])
            .style(outlined),
        );
    }

    for e in &embeds {
        let state = embed_source(e, previews).and_then(|k| previews.get(k));
        let (w, h) = media::display_size(e.width, e.height);
        let caption: Element<'a, Message> = match &e.provider {
            Some(name) => text(name.clone())
                .size(11)
                .color(MUTED)
                .shaping(Shaping::Advanced)
                .into(),
            None => Space::new(0, 0).into(),
        };
        col = col.push(column![media_box(state, w, h, None), caption].spacing(4));
    }

    if !m.reactions.is_empty() {
        let chips = m.reactions.iter().fold(Row::new().spacing(6), |r, re| {
            r.push(
                button(
                    text(format!("{} {}", re.emoji.label(), re.count))
                        .size(12)
                        .shaping(Shaping::Advanced),
                )
                .padding([2, 10])
                .style(reaction(re.me)),
            )
        });
        col = col.push(chips);
    }
    col.into()
}

/// Chiave dell'anteprima nella cache: l'indirizzo da scaricare.
fn preview_key(a: &crate::discord::models::Attachment) -> Option<String> {
    if a.url == crate::demo::DEMO_IMAGE_URL {
        return Some(a.url.clone());
    }
    media::preview_url(a)
}

/// Misura a cui decodificare una GIF: quella dell'anteprima, per lo schermo
/// ad alta densità fino al doppio (senza superare l'originale).
fn decode_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = media::display_size(width, height);
    (
        ((w * 2.0) as u32).min(width.max(1)),
        ((h * 2.0) as u32).min(height.max(1)),
    )
}

/// Primo indirizzo utile per l'anteprima di un link: la GIF animata, oppure
/// l'immagine statica se la GIF non si è potuta scaricare.
fn embed_source<'p>(e: &'p media::EmbedPreview, previews: &HashMap<String, Preview>) -> Option<&'p str> {
    e.sources
        .iter()
        .map(String::as_str)
        .find(|url| !matches!(previews.get(*url), Some(Preview::Failed)))
}

/// Scarica un'anteprima; se è una GIF animata la decodifica in un thread a parte.
async fn load_preview(url: String, w: u32, h: u32) -> anyhow::Result<Loaded> {
    let bytes = media::fetch(&url, media::PREVIEW_MAX_BYTES).await?;
    if !media::is_animatable(&bytes) {
        return Ok(Loaded::Image(bytes));
    }
    tokio::task::spawn_blocking(move || match media::decode_animation(&bytes, w, h) {
        Ok(Some(frames)) => match Animation::new(frames) {
            Some(a) => Ok(Loaded::Animated(a)),
            None => Ok(Loaded::Image(bytes)),
        },
        Ok(None) => Ok(Loaded::Image(bytes)),
        Err(e) => Err(e),
    })
    .await?
}

/// Riquadro dell'immagine: animata, statica o in caricamento, sempre della
/// stessa misura (così la chat non salta quando l'immagine arriva).
fn media_box<'a>(
    state: Option<&'a Preview>,
    w: f32,
    h: f32,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let picture: Element<'a, Message> = match state {
        Some(Preview::Ready(handle)) => img::Image::new(handle.clone())
            .width(w)
            .height(h)
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Preview::Animated(animation)) => animated(animation).width(w).height(h).into(),
        _ => {
            return container(text("Caricamento anteprima…").size(12).color(MUTED))
                .width(w)
                .height(h)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .style(outlined)
                .into()
        }
    };
    match on_press {
        Some(message) => mouse_area(picture)
            .on_press(message)
            .interaction(mouse::Interaction::Pointer)
            .into(),
        None => picture,
    }
}

/// Anteprima di un'immagine allegata, con nome e dimensione del file sotto.
fn image_preview<'a>(
    a: &crate::discord::models::Attachment,
    state: Option<&'a Preview>,
) -> Element<'a, Message> {
    let (w, h) = media::display_size(a.width.unwrap_or(1), a.height.unwrap_or(1));
    let picture = media_box(state, w, h, Some(Message::OpenViewer(Box::new(a.clone()))));
    let caption = row![
        text(a.filename.clone())
            .size(11)
            .color(MUTED)
            .shaping(Shaping::Advanced),
        text(human_size(a.size)).size(11).color(MUTED),
    ]
    .spacing(8);
    column![picture, caption].spacing(4).into()
}

/// Apre la cartella del file scaricato nel gestore file del sistema.
fn show_in_folder(path: &std::path::Path) {
    use std::process::Command;
    // Nessuna shell: il percorso è passato come argomento.
    let result = if cfg!(windows) {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else {
        Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()
    };
    if let Err(e) = result {
        tracing::warn!("impossibile aprire la cartella: {e}");
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0 => String::new(),
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Gui {
        Gui::new(Options {
            demo: true,
            ..Default::default()
        })
        .0
    }

    #[test]
    fn demo_si_apre_su_generale() {
        let gui = demo();
        assert_eq!(gui.store.current.as_deref(), Some("1001"));
        // L'immagine della demo è generata in locale, senza download.
        assert!(matches!(
            gui.previews.get(crate::demo::DEMO_IMAGE_URL),
            Some(Preview::Ready(_))
        ));
        assert_eq!(gui.downloads, 0);
        assert_eq!(gui.title(), "#generale · Rust Italia — ferrocord");
        let _ = gui.view();
    }

    #[test]
    fn navigazione_e_invio() {
        let mut gui = demo();
        let _ = gui.update(Message::SelectGuild(Some(1)));
        assert_eq!(gui.store.current.as_deref(), Some("2001"));
        let _ = gui.view();

        let _ = gui.update(Message::SelectGuild(None));
        assert_eq!(gui.store.current.as_deref(), Some("5001"));
        let _ = gui.view();

        let _ = gui.update(Message::OpenChannel("1001".into()));
        let _ = gui.update(Message::ComposerChanged("  ciao dalla GUI  ".into()));
        let _ = gui.update(Message::Send);
        assert_eq!(
            gui.store.messages["1001"].last().unwrap().content,
            "ciao dalla GUI"
        );
        assert!(gui.composer.is_empty());

        let id = gui.store.messages["1001"][0].id.clone();
        let _ = gui.update(Message::HoverStart(id.clone()));
        assert_eq!(gui.hovered.as_deref(), Some(id.as_str()));
        let _ = gui.view();
        let _ = gui.update(Message::HoverEnd(id));
        assert!(gui.hovered.is_none());
    }

    #[test]
    fn schermata_di_login() {
        let mut gui = Gui::new(Options {
            demo_login: true,
            ..Default::default()
        })
        .0;
        assert_eq!(gui.screen, Screen::Login);
        let _ = gui.view();
        let _ = gui.update(Message::LoginTab(LoginTab::Token));
        let _ = gui.update(Message::TokenInput("Bot abc".into()));
        let _ = gui.update(Message::SubmitToken);
        assert!(gui.login.error.is_some());
        let _ = gui.view();
    }

    #[test]
    fn anteprime_scaricate_poche_alla_volta() {
        let mut gui = demo();
        let attachments: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({
                    "id": i.to_string(), "filename": format!("foto{i}.png"), "size": 1000,
                    "url": format!("https://cdn.discordapp.com/attachments/1/{i}/foto.png"),
                    "proxy_url": format!("https://media.discordapp.net/attachments/1/{i}/foto.png"),
                    "content_type": "image/png", "width": 64, "height": 64
                })
            })
            .collect();
        let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
        msg.attachments = serde_json::from_value(serde_json::Value::Array(attachments)).unwrap();
        gui.store.insert_message(msg);
        let _ = gui.request_previews();
        assert_eq!(gui.downloads, MAX_DOWNLOADS);
        let loading = gui
            .previews
            .values()
            .filter(|p| matches!(p, Preview::Loading))
            .count();
        assert_eq!(loading, MAX_DOWNLOADS);
        let _ = gui.view();

        // Partono in ordine di allegato: il settimo aspetta.
        let url = "https://media.discordapp.net/attachments/1/0/foto.png".to_string();
        let seventh = "https://media.discordapp.net/attachments/1/6/foto.png".to_string();
        assert!(matches!(gui.previews.get(&url), Some(Preview::Loading)));
        assert!(!gui.previews.contains_key(&seventh));

        // Un download fallito ne fa partire un altro e lascia il nome del file.
        let _ = gui.update(Message::PreviewLoaded {
            url: url.clone(),
            result: Err("prova".into()),
        });
        assert!(matches!(gui.previews.get(&url), Some(Preview::Failed)));
        assert!(matches!(gui.previews.get(&seventh), Some(Preview::Loading)));
        assert_eq!(gui.downloads, MAX_DOWNLOADS);
        let _ = gui.view();
    }

    #[test]
    fn anteprime_anche_dei_messaggi_vecchi() {
        let mut gui = demo();
        // Un'immagine seguita da molti messaggi di solo testo (come dopo aver
        // caricato la cronologia più vecchia scorrendo verso l'alto).
        let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
        msg.attachments = serde_json::from_value(serde_json::json!([{
            "id": "1", "filename": "vecchia.jpg", "size": 1000,
            "url": "https://cdn.discordapp.com/attachments/1/1/vecchia.jpg",
            "proxy_url": "https://media.discordapp.net/attachments/1/1/vecchia.jpg",
            "content_type": "image/jpeg", "width": 64, "height": 64
        }]))
        .unwrap();
        gui.store.insert_message(msg);
        for i in 0..120 {
            let m = crate::demo::local_message(
                "1001",
                None,
                &format!("testo {i}"),
                gui.store.messages.get("1001"),
            );
            gui.store.insert_message(m);
        }
        let _ = gui.request_previews();
        let url = "https://media.discordapp.net/attachments/1/1/vecchia.jpg";
        assert!(matches!(gui.previews.get(url), Some(Preview::Loading)));
    }

    #[test]
    fn vista_a_schermo_intero() {
        let mut gui = demo();
        let att = gui.store.messages["1001"]
            .iter()
            .flat_map(|m| m.attachments.iter())
            .find(|a| a.url == crate::demo::DEMO_IMAGE_URL)
            .unwrap()
            .clone();
        let _ = gui.update(Message::OpenViewer(Box::new(att)));
        assert!(matches!(
            gui.viewer.as_ref().map(|v| &v.image),
            Some(Preview::Ready(_))
        ));
        let _ = gui.view();
        // Nella demo non si scarica nulla.
        let _ = gui.update(Message::SaveAttachment);
        assert!(matches!(
            gui.viewer.as_ref().map(|v| &v.save),
            Some(SaveState::Idle)
        ));
        let _ = gui.update(Message::Noop);
        assert!(gui.viewer.is_some());
        let _ = gui.update(Message::CloseViewer);
        assert!(gui.viewer.is_none());

        // Allegato vero: parte il download della versione grande.
        let real: Attachment = serde_json::from_value(serde_json::json!({
            "id": "9", "filename": "foto.jpg", "size": 2048,
            "url": "https://cdn.discordapp.com/attachments/1/9/foto.jpg",
            "proxy_url": "https://media.discordapp.net/attachments/1/9/foto.jpg",
            "content_type": "image/jpeg", "width": 4000, "height": 3000
        }))
        .unwrap();
        let _ = gui.update(Message::OpenViewer(Box::new(real)));
        let v = gui.viewer.as_ref().unwrap();
        assert!(matches!(v.image, Preview::Loading));
        let url = v.url.clone().unwrap();
        assert!(url.ends_with("width=2560&height=1920"), "{url}");
        let _ = gui.view();
        let _ = gui.update(Message::ViewerLoaded {
            url,
            result: Err("prova".into()),
        });
        assert!(matches!(gui.viewer.as_ref().unwrap().image, Preview::Failed));
        let _ = gui.update(Message::Saved(Err("disco pieno".into())));
        assert!(matches!(gui.viewer.as_ref().unwrap().save, SaveState::Failed(_)));
        let _ = gui.view();
    }

    #[test]
    fn gif_dei_link_animate() {
        let gui = demo();
        // La GIF della demo è generata in locale e animata.
        match gui.previews.get(crate::demo::DEMO_GIF_URL) {
            Some(Preview::Animated(a)) => assert!(a.len() > 1),
            _ => panic!("GIF della demo non animata"),
        }
        let _ = gui.view();
    }

    #[test]
    fn gif_vera_con_ripiego_sull_immagine_statica() {
        let mut gui = demo();
        let mut msg = crate::demo::local_message(
            "1001",
            None,
            "https://tenor.com/view/gatto-1",
            gui.store.messages.get("1001"),
        );
        msg.embeds = serde_json::from_value(serde_json::json!([{
            "type": "gifv", "url": "https://tenor.com/view/gatto-1", "provider": {"name": "Tenor"},
            "thumbnail": {"url": "https://media.tenor.com/abcAAAAe/gatto.png",
                "proxy_url": "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png",
                "width": 498, "height": 280},
            "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4", "width": 498, "height": 280}
        }]))
        .unwrap();
        gui.store.insert_message(msg);
        let _ = gui.request_previews();
        let gif = "https://media.tenor.com/abcAAAAd/gatto.gif".to_string();
        let still = "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png"
            .to_string();
        assert!(matches!(gui.previews.get(&gif), Some(Preview::Loading)));
        assert!(!gui.previews.contains_key(&still));
        let _ = gui.view();
        // Se la GIF non arriva si usa l'immagine statica di Discord.
        let _ = gui.update(Message::PreviewLoaded {
            url: gif.clone(),
            result: Err("404".into()),
        });
        assert!(matches!(gui.previews.get(&still), Some(Preview::Loading)));
        let _ = gui.view();
    }

    #[test]
    fn selettore_delle_emoji() {
        let mut gui = demo();
        gui.composer = "ciao ".into();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert!(gui.picker.is_some());
        let _ = gui.view();
        let _ = gui.update(Message::EmojiQuery("crab".into()));
        let _ = gui.view();
        let _ = gui.update(Message::PickEmoji("🦀"));
        assert_eq!(gui.composer, "ciao 🦀");
        assert!(gui.picker.is_none());
        assert_eq!(gui.recent_emojis, vec!["🦀"]);
        // Riaprendo si parte dalle usate di recente; lo stesso pulsante lo richiude.
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert_eq!(gui.picker.as_ref().unwrap().section, picker::Section::Recent);
        let _ = gui.view();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
        assert!(gui.picker.is_none());
    }

    #[test]
    fn selettore_delle_gif() {
        let mut gui = demo();
        let before = gui.store.messages["1001"].len();
        let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
        let p = gui.picker.as_ref().unwrap();
        assert!(!p.shown_gifs().is_empty());
        assert!(matches!(gui.previews.get(crate::demo::DEMO_GIF_URL), Some(Preview::Animated(_))));
        let _ = gui.view();
        // Una ricerca vecchia non sovrascrive quella nuova.
        let _ = gui.update(Message::GifQuery("ferr".into()));
        let _ = gui.update(Message::GifQuery("ferris".into()));
        let _ = gui.update(Message::GifSearch(1));
        assert!(!gui.picker.as_ref().unwrap().searching);
        let _ = gui.update(Message::GifSearch(2));
        let url = gui.picker.as_ref().unwrap().shown_gifs()[0].url.clone();
        // Clic su una GIF: si invia il link e nella chat compare la GIF.
        gui.composer = "bozza".into();
        let _ = gui.update(Message::PickGif(url.clone()));
        assert!(gui.picker.is_none());
        assert_eq!(gui.composer, "bozza");
        let list = &gui.store.messages["1001"];
        assert_eq!(list.len(), before + 1);
        assert_eq!(list.last().unwrap().content, url);
        assert_eq!(list.last().unwrap().embeds.len(), 1);
        let _ = gui.view();
    }

    #[test]
    fn cache_delle_anteprime_limitata() {
        let mut gui = demo();
        for i in 0..(MAX_PREVIEWS + 20) {
            gui.remember_preview(format!("u{i}"), Preview::Failed);
        }
        assert_eq!(gui.previews.len(), MAX_PREVIEWS);
        assert!(!gui.previews.contains_key("u0"));
        assert!(gui.previews.contains_key(&format!("u{}", MAX_PREVIEWS + 19)));
    }

    #[test]
    fn dimensioni_leggibili() {
        assert_eq!(human_size(245_760), "240 KB");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(0), "");
    }
}

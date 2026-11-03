//! Connessione al gateway WebSocket: identify/resume, heartbeat, eventi.
//!
//! Il task gira finché l'app non lo interrompe (`JoinHandle::abort`) e si
//! riconnette da solo con backoff esponenziale, riprendendo la sessione
//! (op 6 RESUME) quando possibile per non perdere eventi.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, value::RawValue};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{interval_at, timeout, Instant, Interval, MissedTickBehavior},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{protocol::CloseFrame, Message as WsMessage},
};
use tracing::{debug, info, warn};

use super::models::{
    Guild, GuildEmojisUpdate, Message, MessageDelete, MessageUpdate, ReactionEvent, Ready, TypingStart,
    VoiceServerUpdate, VoiceState,
};
use super::{send_json, tick_opt, zlib::ZlibStream};

const DEFAULT_URL: &str = "wss://gateway.discord.gg";
const QUERY: &str = "/?v=10&encoding=json&compress=zlib-stream";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Connecting,
    Disconnected(String),
    Ready(Box<Ready>),
    Resumed,
    GuildCreate(Box<Guild>),
    GuildEmojis(GuildEmojisUpdate),
    MessageCreate(Box<Message>),
    MessageUpdate(Box<MessageUpdate>),
    MessageDelete(MessageDelete),
    TypingStart(TypingStart),
    ReactionAdd(ReactionEvent),
    ReactionRemove(ReactionEvent),
    VoiceState(VoiceState),
    VoiceServer(VoiceServerUpdate),
    /// Token rifiutato (close 4004): serve un nuovo login.
    AuthFailed,
    /// Errore non recuperabile: il task si ferma.
    Fatal(String),
}

/// Comandi da mandare sul gateway.
#[derive(Debug)]
pub enum GatewayCommand {
    /// Op 4: entra in un canale vocale (o esce, con `channel_id: None`).
    VoiceState {
        guild_id: Option<String>,
        channel_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },
}

/// Per mandare comandi al gateway; vale anche attraverso le riconnessioni.
#[derive(Debug, Clone)]
pub struct GatewayHandle(UnboundedSender<GatewayCommand>);

impl GatewayHandle {
    pub fn voice_state(&self, guild_id: Option<String>, channel_id: Option<String>) {
        let _ = self.0.send(GatewayCommand::VoiceState {
            guild_id,
            channel_id,
            self_mute: false,
            self_deaf: false,
        });
    }
}

/// Canale dei comandi: l'handle va a chi li manda, il ricevitore a [`run`].
pub fn commands() -> (GatewayHandle, UnboundedReceiver<GatewayCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (GatewayHandle(tx), rx)
}

/// Avvia il gateway su un task tokio; gli eventi arrivano su `tx` convertiti in `E`.
pub fn spawn<E: From<GatewayEvent> + Send + 'static>(
    token: String,
    tx: UnboundedSender<E>,
) -> (JoinHandle<()>, GatewayHandle) {
    let (handle, rx) = commands();
    (tokio::spawn(run(token, tx, rx)), handle)
}

fn command_payload(cmd: &GatewayCommand) -> serde_json::Value {
    match cmd {
        GatewayCommand::VoiceState {
            guild_id,
            channel_id,
            self_mute,
            self_deaf,
        } => json!({
            "op": 4,
            "d": {
                "guild_id": guild_id,
                "channel_id": channel_id,
                "self_mute": self_mute,
                "self_deaf": self_deaf,
                "self_video": false,
            }
        }),
    }
}

/// Prossimo comando; se tutti gli handle sono stati abbandonati non arriva mai.
async fn next_command(cmds: &mut Option<UnboundedReceiver<GatewayCommand>>) -> GatewayCommand {
    loop {
        match cmds {
            Some(rx) => match rx.recv().await {
                Some(cmd) => return cmd,
                None => *cmds = None,
            },
            None => std::future::pending::<()>().await,
        }
    }
}

#[derive(Default)]
struct Session {
    id: Option<String>,
    resume_url: Option<String>,
    seq: Option<u64>,
}

impl Session {
    fn can_resume(&self) -> bool {
        self.id.is_some()
    }
}

enum Outcome {
    Reconnect,
    Stop,
}

fn emit<E: From<GatewayEvent>>(tx: &UnboundedSender<E>, ev: GatewayEvent) {
    let _ = tx.send(ev.into());
}

/// Ciclo del gateway con riconnessione automatica. Termina solo su errori non
/// recuperabili (token rifiutato) o quando il ricevitore di `tx` viene chiuso.
pub async fn run<E: From<GatewayEvent> + Send + 'static>(
    token: String,
    tx: UnboundedSender<E>,
    cmds: UnboundedReceiver<GatewayCommand>,
) {
    let mut cmds = Some(cmds);
    let mut session = Session::default();
    let mut backoff = Duration::from_secs(1);
    loop {
        emit(&tx, GatewayEvent::Connecting);
        let mut healthy = false;
        let result = connection(&token, &mut session, &tx, &mut cmds, &mut healthy).await;
        if healthy {
            backoff = Duration::from_secs(1);
        }
        match result {
            Ok(Outcome::Stop) => return,
            Ok(Outcome::Reconnect) => info!("gateway: riconnessione richiesta dal server"),
            Err(e) => {
                warn!("gateway: {e:#}");
                emit(&tx, GatewayEvent::Disconnected(short_error(&e)));
            }
        }
        if tx.is_closed() {
            return;
        }
        let jitter = Duration::from_millis(rand::random::<u64>() % 1000);
        tokio::time::sleep(backoff + jitter).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn short_error(e: &anyhow::Error) -> String {
    let s = e.to_string();
    match s.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

#[derive(Deserialize)]
struct Payload<'a> {
    op: u8,
    #[serde(borrow, default)]
    d: Option<&'a RawValue>,
    #[serde(default)]
    s: Option<u64>,
    #[serde(borrow, default)]
    t: Option<&'a str>,
}

#[derive(Deserialize)]
struct Hello {
    heartbeat_interval: u64,
}

async fn connection<E: From<GatewayEvent>>(
    token: &str,
    session: &mut Session,
    tx: &UnboundedSender<E>,
    cmds: &mut Option<UnboundedReceiver<GatewayCommand>>,
    healthy: &mut bool,
) -> Result<Outcome> {
    let base = match (&session.resume_url, session.can_resume()) {
        (Some(url), true) => url.clone(),
        _ => DEFAULT_URL.to_string(),
    };
    let url = format!("{}{}", base.trim_end_matches('/'), QUERY);
    debug!("gateway: connessione a {url}");

    let (ws, _) = timeout(CONNECT_TIMEOUT, connect_async(url.as_str()))
        .await
        .context("timeout di connessione")??;
    let (mut sink, mut stream) = ws.split();
    let mut zlib = ZlibStream::new();
    let mut heartbeat: Option<Interval> = None;
    let mut awaiting_ack = false;

    loop {
        tokio::select! {
            _ = tick_opt(&mut heartbeat) => {
                if awaiting_ack {
                    bail!("heartbeat senza risposta (connessione zombie)");
                }
                send_json(&mut sink, &json!({ "op": 1, "d": session.seq })).await?;
                awaiting_ack = true;
            }
            // Solo dopo READY: prima il gateway rifiuterebbe i comandi.
            cmd = next_command(cmds), if *healthy => {
                send_json(&mut sink, &command_payload(&cmd)).await?;
            }
            msg = stream.next() => {
                let msg = msg.ok_or_else(|| anyhow!("connessione chiusa"))??;
                let text = match msg {
                    WsMessage::Binary(bytes) => match zlib.push(&bytes)? {
                        Some(text) => text,
                        None => continue,
                    },
                    WsMessage::Text(text) => text,
                    WsMessage::Close(frame) => return close_outcome(frame, session, tx),
                    _ => continue,
                };

                let p: Payload = serde_json::from_str(&text).context("payload non valido")?;
                if let Some(s) = p.s {
                    session.seq = Some(s);
                }
                match p.op {
                    // Dispatch
                    0 => dispatch(p.t.unwrap_or_default(), p.d, session, tx, healthy)?,
                    // Il server chiede un heartbeat immediato
                    1 => send_json(&mut sink, &json!({ "op": 1, "d": session.seq })).await?,
                    // Reconnect
                    7 => return Ok(Outcome::Reconnect),
                    // Invalid session: d = true se si può riprendere
                    9 => {
                        let resumable = p.d.is_some_and(|d| d.get() == "true");
                        if !resumable {
                            *session = Session::default();
                        }
                        let wait = 1000 + rand::random::<u64>() % 4000;
                        tokio::time::sleep(Duration::from_millis(wait)).await;
                        return Ok(Outcome::Reconnect);
                    }
                    // Hello
                    10 => {
                        let hello: Hello = parse_raw(p.d)?;
                        heartbeat = Some(heartbeat_timer(hello.heartbeat_interval));
                        if session.can_resume() {
                            send_json(&mut sink, &resume_payload(token, session)).await?;
                        } else {
                            send_json(&mut sink, &identify_payload(token)).await?;
                        }
                    }
                    // Heartbeat ACK
                    11 => awaiting_ack = false,
                    _ => {}
                }
            }
        }
    }
}

fn heartbeat_timer(interval_ms: u64) -> Interval {
    let period = Duration::from_millis(interval_ms.max(1000));
    // Il primo heartbeat va inviato dopo `interval * jitter` (da documentazione).
    let first = Duration::from_millis((interval_ms as f64 * rand::random::<f64>()) as u64);
    let mut iv = interval_at(Instant::now() + first, period);
    iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
    iv
}

/// Identify per un account utente: niente `intents` (sono solo per i bot).
/// Le `properties` dichiarano onestamente chi siamo.
fn identify_payload(token: &str) -> serde_json::Value {
    json!({
        "op": 2,
        "d": {
            "token": token,
            "capabilities": 0,
            "properties": {
                "os": std::env::consts::OS,
                "browser": "ferrocord",
                "device": "ferrocord",
            },
            "presence": { "status": "online", "since": 0, "activities": [], "afk": false },
            "compress": false,
        }
    })
}

fn resume_payload(token: &str, session: &Session) -> serde_json::Value {
    json!({
        "op": 6,
        "d": { "token": token, "session_id": session.id, "seq": session.seq }
    })
}

fn parse_raw<T: DeserializeOwned>(d: Option<&RawValue>) -> Result<T> {
    let d = d.context("payload senza dati")?;
    Ok(serde_json::from_str(d.get())?)
}

/// Un singolo evento malformato viene scartato senza far cadere la connessione.
fn parse_event<T: DeserializeOwned>(name: &str, d: Option<&RawValue>) -> Option<T> {
    match parse_raw(d) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!("gateway: evento {name} ignorato: {e:#}");
            None
        }
    }
}

fn dispatch<E: From<GatewayEvent>>(
    name: &str,
    d: Option<&RawValue>,
    session: &mut Session,
    tx: &UnboundedSender<E>,
    healthy: &mut bool,
) -> Result<()> {
    let ev = match name {
        "READY" => {
            // Senza READY l'app non ha dati: qui un errore fa riconnettere.
            let ready: Ready = parse_raw(d).context("READY non valido")?;
            session.id = Some(ready.session_id.clone());
            session.resume_url = ready.resume_gateway_url.clone();
            *healthy = true;
            info!(
                "gateway: pronto ({} server, {} conversazioni)",
                ready.guilds.len(),
                ready.private_channels.len()
            );
            GatewayEvent::Ready(Box::new(ready))
        }
        "RESUMED" => {
            *healthy = true;
            GatewayEvent::Resumed
        }
        "GUILD_CREATE" => match parse_event(name, d) {
            Some(g) => GatewayEvent::GuildCreate(Box::new(g)),
            None => return Ok(()),
        },
        "GUILD_EMOJIS_UPDATE" => match parse_event(name, d) {
            Some(u) => GatewayEvent::GuildEmojis(u),
            None => return Ok(()),
        },
        "VOICE_STATE_UPDATE" => match parse_event(name, d) {
            Some(v) => GatewayEvent::VoiceState(v),
            None => return Ok(()),
        },
        "VOICE_SERVER_UPDATE" => match parse_event(name, d) {
            Some(v) => GatewayEvent::VoiceServer(v),
            None => return Ok(()),
        },
        "MESSAGE_CREATE" => match parse_event(name, d) {
            Some(m) => GatewayEvent::MessageCreate(Box::new(m)),
            None => return Ok(()),
        },
        "MESSAGE_UPDATE" => match parse_event(name, d) {
            Some(m) => GatewayEvent::MessageUpdate(Box::new(m)),
            None => return Ok(()),
        },
        "MESSAGE_DELETE" => match parse_event(name, d) {
            Some(m) => GatewayEvent::MessageDelete(m),
            None => return Ok(()),
        },
        "TYPING_START" => match parse_event(name, d) {
            Some(t) => GatewayEvent::TypingStart(t),
            None => return Ok(()),
        },
        "MESSAGE_REACTION_ADD" => match parse_event(name, d) {
            Some(r) => GatewayEvent::ReactionAdd(r),
            None => return Ok(()),
        },
        "MESSAGE_REACTION_REMOVE" => match parse_event(name, d) {
            Some(r) => GatewayEvent::ReactionRemove(r),
            None => return Ok(()),
        },
        _ => return Ok(()),
    };
    emit(tx, ev);
    Ok(())
}

fn close_outcome<E: From<GatewayEvent>>(
    frame: Option<CloseFrame<'_>>,
    session: &mut Session,
    tx: &UnboundedSender<E>,
) -> Result<Outcome> {
    let code = frame.as_ref().map(|f| u16::from(f.code));
    match code {
        Some(4004) => {
            emit(tx, GatewayEvent::AuthFailed);
            Ok(Outcome::Stop)
        }
        Some(c @ 4010..=4014) => {
            emit(
                tx,
                GatewayEvent::Fatal(format!("il gateway ha rifiutato la connessione (codice {c})")),
            );
            Ok(Outcome::Stop)
        }
        Some(c @ (4007 | 4009)) => {
            *session = Session::default();
            bail!("sessione scaduta (codice {c})")
        }
        Some(c) => bail!("connessione chiusa dal server (codice {c})"),
        None => bail!("connessione chiusa dal server"),
    }
}

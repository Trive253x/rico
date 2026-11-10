//! Connection to the WebSocket gateway: identify/resume, heartbeat, events.
//!
//! The task runs until the app stops it (`JoinHandle::abort`) and reconnects
//! on its own with exponential backoff, resuming the session (op 6 RESUME)
//! when possible so that no event is lost.

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
    Channel, Guild, GuildDelete, GuildEmojisUpdate, GuildMemberUpdate, GuildRoleDelete, GuildRoleUpdate,
    Message, MessageDelete, MessageUpdate, ReactionEvent, Ready, StreamCreate, StreamDelete,
    StreamServerUpdate, TypingStart, VoiceServerUpdate, VoiceState,
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
    /// We left a server (or it became unavailable).
    GuildDelete(GuildDelete),
    /// A channel was created or changed (name, position, permissions).
    ChannelUpdate(Box<Channel>),
    ChannelDelete(Box<Channel>),
    /// A role was created or changed (permissions).
    RoleUpdate(Box<GuildRoleUpdate>),
    RoleDelete(GuildRoleDelete),
    /// A member's roles, nickname or timeout changed.
    MemberUpdate(Box<GuildMemberUpdate>),
    MessageCreate(Box<Message>),
    MessageUpdate(Box<MessageUpdate>),
    MessageDelete(MessageDelete),
    TypingStart(TypingStart),
    ReactionAdd(ReactionEvent),
    ReactionRemove(ReactionEvent),
    VoiceState(VoiceState),
    VoiceServer(VoiceServerUpdate),
    /// A stream we asked to watch: its voice server ID.
    StreamCreate(StreamCreate),
    /// Where to connect to watch it.
    StreamServer(StreamServerUpdate),
    /// The stream ended, or we cannot watch it.
    StreamDelete(StreamDelete),
    /// Rows of the member list of a server channel we subscribed to.
    MemberList(Box<super::member_list::MemberListUpdate>),
    /// Our account settings changed (also from another device): the base64
    /// protobuf, whole or only the changed fields.
    SettingsProto(SettingsProtoUpdate),
    /// Token rejected (close 4004): a new login is needed.
    AuthFailed,
    /// Unrecoverable error: the task stops.
    Fatal(String),
}

/// Commands to send on the gateway.
#[derive(Debug)]
pub enum GatewayCommand {
    /// Op 4: join a voice channel (or leave it, with `channel_id: None`).
    VoiceState {
        guild_id: Option<String>,
        channel_id: Option<String>,
        self_mute: bool,
        self_deaf: bool,
    },
    /// Op 20: watch a Go Live stream (we must be in its voice channel).
    WatchStream { stream_key: String },
    /// Op 19: stop watching (or end our own stream).
    StopStream { stream_key: String },
    /// Op 37: the member list of a channel (the first `pages` pages), as
    /// the official client asks it when a channel is opened.
    MemberList {
        guild_id: String,
        channel_id: String,
        pages: usize,
    },
}

/// To send commands to the gateway; survives reconnections.
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

    pub fn watch_stream(&self, stream_key: String) {
        let _ = self.0.send(GatewayCommand::WatchStream { stream_key });
    }

    pub fn stop_stream(&self, stream_key: String) {
        let _ = self.0.send(GatewayCommand::StopStream { stream_key });
    }

    pub fn member_list(&self, guild_id: String, channel_id: String, pages: usize) {
        let _ = self.0.send(GatewayCommand::MemberList {
            guild_id,
            channel_id,
            pages,
        });
    }
}

/// Command channel: the handle goes to whoever sends them, the receiver to [`run`].
pub fn commands() -> (GatewayHandle, UnboundedReceiver<GatewayCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (GatewayHandle(tx), rx)
}

/// Starts the gateway on a tokio task; events arrive on `tx` converted to `E`.
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
        GatewayCommand::WatchStream { stream_key } => json!({"op": 20, "d": {"stream_key": stream_key}}),
        GatewayCommand::StopStream { stream_key } => json!({"op": 19, "d": {"stream_key": stream_key}}),
        GatewayCommand::MemberList {
            guild_id,
            channel_id,
            pages,
        } => {
            let mut channels = serde_json::Map::new();
            channels.insert(channel_id.clone(), json!(super::member_list::ranges(*pages)));
            let mut subscriptions = serde_json::Map::new();
            subscriptions.insert(
                guild_id.clone(),
                json!({
                    "typing": true,
                    "activities": false,
                    "threads": false,
                    "channels": channels,
                }),
            );
            json!({"op": 37, "d": {"subscriptions": subscriptions}})
        }
    }
}

/// Next command; if every handle has been dropped it never arrives.
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

/// Gateway loop with automatic reconnection. Ends only on unrecoverable
/// errors (token rejected) or when the receiver of `tx` is closed.
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
            Ok(Outcome::Reconnect) => info!("gateway: reconnection requested by the server"),
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
    debug!("gateway: connecting to {url}");

    let (ws, _) = timeout(CONNECT_TIMEOUT, connect_async(url.as_str()))
        .await
        .context("connection timed out")??;
    let (mut sink, mut stream) = ws.split();
    let mut zlib = ZlibStream::new();
    let mut heartbeat: Option<Interval> = None;
    let mut awaiting_ack = false;

    loop {
        tokio::select! {
            _ = tick_opt(&mut heartbeat) => {
                if awaiting_ack {
                    bail!("heartbeat without answer (zombie connection)");
                }
                send_json(&mut sink, &json!({ "op": 1, "d": session.seq })).await?;
                awaiting_ack = true;
            }
            // Only after READY: before that the gateway would reject commands.
            cmd = next_command(cmds), if *healthy => {
                send_json(&mut sink, &command_payload(&cmd)).await?;
            }
            msg = stream.next() => {
                let msg = msg.ok_or_else(|| anyhow!("connection closed"))??;
                let text = match msg {
                    WsMessage::Binary(bytes) => match zlib.push(&bytes)? {
                        Some(text) => text,
                        None => continue,
                    },
                    WsMessage::Text(text) => text,
                    WsMessage::Close(frame) => return close_outcome(frame, session, tx),
                    _ => continue,
                };

                let p: Payload = serde_json::from_str(&text).context("invalid payload")?;
                if let Some(s) = p.s {
                    session.seq = Some(s);
                }
                match p.op {
                    // Dispatch
                    0 => dispatch(p.t.unwrap_or_default(), p.d, session, tx, healthy)?,
                    // The server asks for an immediate heartbeat
                    1 => send_json(&mut sink, &json!({ "op": 1, "d": session.seq })).await?,
                    // Reconnect
                    7 => return Ok(Outcome::Reconnect),
                    // Invalid session: d = true if it can be resumed
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
    // The first heartbeat goes after `interval * jitter` (as documented).
    let first = Duration::from_millis((interval_ms as f64 * rand::random::<f64>()) as u64);
    let mut iv = interval_at(Instant::now() + first, period);
    iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
    iv
}

/// Identify for a user account: no `intents` (they are for bots only).
/// The `properties` say honestly who we are.
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
    let d = d.context("payload without data")?;
    serde_json::from_str(d.get()).map_err(|e| anyhow!(json_error(&e)))
}

/// A JSON error without serde's message, which can quote values of the
/// payload (message text, names): these errors end up in the log.
fn json_error(e: &serde_json::Error) -> String {
    format!(
        "{:?} error at line {}, column {}",
        e.classify(),
        e.line(),
        e.column()
    )
}

/// `USER_SETTINGS_PROTO_UPDATE`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SettingsProtoUpdate {
    pub settings: SettingsProto,
    /// Only the changed top-level fields are in the protobuf.
    #[serde(default)]
    pub partial: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SettingsProto {
    #[serde(rename = "type")]
    pub kind: u8,
    /// Base64 `PreloadedUserSettings`.
    pub proto: String,
}

/// A single malformed event is dropped without bringing the connection down.
fn parse_event<T: DeserializeOwned>(name: &str, d: Option<&RawValue>) -> Option<T> {
    match parse_raw(d) {
        Ok(v) => Some(v),
        Err(e) => {
            warn!("gateway: event {name} ignored: {e:#}");
            None
        }
    }
}

/// The address Discord gives for resuming, only if it is a Discord gateway
/// over TLS: the token is sent there. Anything else resumes from the default
/// address.
fn resume_url(url: &str) -> Option<String> {
    let host = url.trim_end_matches('/').strip_prefix("wss://")?;
    let valid = host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && (host == "gateway.discord.gg" || host.ends_with(".discord.gg"))
        && !host.starts_with('.')
        && !host.contains("..");
    if !valid {
        warn!("gateway: resume address not accepted, the default one is used");
        return None;
    }
    Some(format!("wss://{host}"))
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
            // Without READY the app has no data: here an error makes it reconnect.
            let ready: Ready = parse_raw(d).context("invalid READY")?;
            session.id = Some(ready.session_id.clone());
            session.resume_url = ready.resume_gateway_url.as_deref().and_then(resume_url);
            *healthy = true;
            info!(
                "gateway: ready ({} servers, {} conversations)",
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
        "GUILD_DELETE" => match parse_event(name, d) {
            Some(g) => GatewayEvent::GuildDelete(g),
            None => return Ok(()),
        },
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => match parse_event(name, d) {
            Some(c) => GatewayEvent::ChannelUpdate(Box::new(c)),
            None => return Ok(()),
        },
        "CHANNEL_DELETE" => match parse_event(name, d) {
            Some(c) => GatewayEvent::ChannelDelete(Box::new(c)),
            None => return Ok(()),
        },
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => match parse_event(name, d) {
            Some(r) => GatewayEvent::RoleUpdate(Box::new(r)),
            None => return Ok(()),
        },
        "GUILD_ROLE_DELETE" => match parse_event(name, d) {
            Some(r) => GatewayEvent::RoleDelete(r),
            None => return Ok(()),
        },
        "GUILD_MEMBER_UPDATE" => match parse_event(name, d) {
            Some(m) => GatewayEvent::MemberUpdate(Box::new(m)),
            None => return Ok(()),
        },
        "VOICE_STATE_UPDATE" => match parse_event(name, d) {
            Some(v) => GatewayEvent::VoiceState(v),
            None => return Ok(()),
        },
        "GUILD_MEMBER_LIST_UPDATE" => match parse_event(name, d) {
            Some(u) => GatewayEvent::MemberList(Box::new(u)),
            None => return Ok(()),
        },
        "USER_SETTINGS_PROTO_UPDATE" => match parse_event::<SettingsProtoUpdate>(name, d) {
            // Type 1: the settings with the folders (2 and 3 are others).
            Some(s) if s.settings.kind == 1 => GatewayEvent::SettingsProto(s),
            _ => return Ok(()),
        },
        "STREAM_CREATE" => match parse_event(name, d) {
            Some(s) => GatewayEvent::StreamCreate(s),
            None => return Ok(()),
        },
        "STREAM_SERVER_UPDATE" => match parse_event(name, d) {
            Some(s) => GatewayEvent::StreamServer(s),
            None => return Ok(()),
        },
        "STREAM_DELETE" => match parse_event(name, d) {
            Some(s) => GatewayEvent::StreamDelete(s),
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
                GatewayEvent::Fatal(tf!(
                    "the gateway refused the connection (code {c})",
                    "il gateway ha rifiutato la connessione (codice {c})"
                )),
            );
            Ok(Outcome::Stop)
        }
        Some(c @ (4007 | 4009)) => {
            *session = Session::default();
            bail!("session expired (code {c})")
        }
        Some(c) => bail!("connection closed by the server (code {c})"),
        None => bail!("connection closed by the server"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_list_subscription() {
        let p = command_payload(&GatewayCommand::MemberList {
            guild_id: "100".into(),
            channel_id: "1001".into(),
            pages: 2,
        });
        assert_eq!(p["op"], 37);
        assert_eq!(
            p["d"]["subscriptions"]["100"]["channels"]["1001"],
            serde_json::json!([[0, 99], [100, 199]])
        );
    }

    #[test]
    fn json_errors_do_not_quote_the_payload() {
        let raw = RawValue::from_string(r#"{"id": "secret words"}"#.to_string()).unwrap();
        #[derive(serde::Deserialize, Debug)]
        struct OnlyNumbers {
            #[allow(dead_code)]
            id: u64,
        }
        let e = parse_raw::<OnlyNumbers>(Some(&raw)).unwrap_err().to_string();
        assert!(!e.contains("secret"), "{e}");
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn resume_only_towards_discord_over_tls() {
        assert_eq!(
            resume_url("wss://gateway-us-east1-b.discord.gg").as_deref(),
            Some("wss://gateway-us-east1-b.discord.gg")
        );
        assert_eq!(
            resume_url("wss://gateway.discord.gg/").as_deref(),
            Some("wss://gateway.discord.gg")
        );
        for bad in [
            "ws://gateway-us-east1-b.discord.gg",
            "wss://example.com",
            "wss://discord.gg.example.com",
            "wss://evil.com/.discord.gg",
            "wss://user@gateway.discord.gg",
            "wss://gateway.discord.gg:8443",
            "wss://.discord.gg",
            "wss://a..discord.gg",
            "https://gateway.discord.gg",
            "",
        ] {
            assert_eq!(resume_url(bad), None, "{bad}");
        }
    }
}

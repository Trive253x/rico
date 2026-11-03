//! Connessione alla voce: gateway vocale (v8) e trasporto UDP dell'audio.
//!
//! Per entrare in un canale vocale servono due passi:
//! 1. sul gateway principale si manda l'op 4 (stato vocale) e Discord
//!    risponde con `VOICE_STATE_UPDATE` (la nostra `session_id`) e
//!    `VOICE_SERVER_UPDATE` (server vocale e token): vedi [`VoiceServer`];
//! 2. qui ci si collega al server vocale: identify, IP discovery via UDP,
//!    scelta della cifratura e chiave segreta; poi l'audio viaggia in
//!    pacchetti RTP cifrati (vedi [`crypto`]).
//!
//! Il contenuto dei pacchetti è audio Opus: decodificarlo e registrarlo è
//! compito di un altro modulo. Qui si misurano ping, pacchetti e chi parla.
//!
//! Manca ancora la crittografia end-to-end (DAVE), che Discord richiede dal
//! 2026: dichiariamo `max_dave_protocol_version: 0` e un server vero può
//! rifiutare la connessione (codice 4017), che viene riportata come tale.

pub mod crypto;
pub mod mock;
pub mod rtp;

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{interval, interval_at, timeout, Instant, Interval, MissedTickBehavior};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{protocol::CloseFrame, Message as WsMessage},
};
use tracing::{debug, info, warn};

use super::{send_json, tick_opt, WsSink};
use crypto::{Cipher, Mode};
use rtp::RtpHeader;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Chi non manda audio da più di così ha smesso di parlare.
const SPEAKING_HOLD: Duration = Duration::from_millis(300);
const UDP_KEEPALIVE: Duration = Duration::from_secs(5);
/// Tentativi di riconnessione di fila prima di arrendersi.
const MAX_ATTEMPTS: u32 = 3;

/// Dove e come collegarsi: dati di `VOICE_SERVER_UPDATE` e `VOICE_STATE_UPDATE`.
#[derive(Debug, Clone)]
pub struct VoiceServer {
    /// Host del server vocale (es. `c-mxp08-1234.discord.media`), oppure un
    /// indirizzo `ws://` completo (il server finto della demo).
    pub endpoint: String,
    pub token: String,
    pub guild_id: String,
    pub session_id: String,
    pub user_id: String,
}

impl VoiceServer {
    fn url(&self) -> String {
        let e = self.endpoint.trim_end_matches('/');
        if e.starts_with("ws://") || e.starts_with("wss://") {
            format!("{e}/?v=8")
        } else {
            format!("wss://{e}/?v=8")
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceStats {
    /// Tempo di risposta del server vocale (heartbeat).
    pub ping_ms: Option<u32>,
    /// Pacchetti audio ricevuti e decifrati.
    pub received: u64,
    pub sent: u64,
    /// Pacchetti scartati perché non autentici.
    pub rejected: u64,
}

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Per mandare comandi alla connessione: è sempre il primo evento.
    Handle(VoiceHandle),
    Connecting,
    /// Audio pronto: cifratura concordata.
    Connected { mode: &'static str },
    Speaking { user_id: String, speaking: bool },
    Stats(VoiceStats),
    /// Connessione persa: si riprova da soli.
    Reconnecting(String),
    /// Fine definitiva: `None` se si è usciti, altrimenti il motivo.
    Closed(Option<String>),
}

/// Comandi verso la connessione vocale. Se tutti gli handle vengono
/// abbandonati la connessione si chiude.
#[derive(Debug, Clone)]
pub struct VoiceHandle(UnboundedSender<VoiceCommand>);

impl VoiceHandle {
    /// Un frame Opus da 20 ms da mandare nel canale.
    pub fn send_opus(&self, frame: Vec<u8>) {
        let _ = self.0.send(VoiceCommand::Opus(frame));
    }

    /// Indicatore "sta parlando" visto dagli altri.
    pub fn set_speaking(&self, speaking: bool) {
        let _ = self.0.send(VoiceCommand::Speaking(speaking));
    }

    pub fn leave(&self) {
        let _ = self.0.send(VoiceCommand::Leave);
    }
}

#[derive(Debug)]
enum VoiceCommand {
    Opus(Vec<u8>),
    Speaking(bool),
    Leave,
}

/// Errore dopo il quale riprovare non serve.
#[derive(Debug)]
struct Fatal(String);

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fatal {}

fn emit<E: From<VoiceEvent>>(tx: &UnboundedSender<E>, ev: VoiceEvent) {
    let _ = tx.send(ev.into());
}

/// Collegamento con il server vocale. Sopravvive alle riconnessioni del
/// WebSocket: con un `resume` si continua con lo stesso UDP e la stessa chiave.
#[derive(Default)]
struct Link {
    ssrc: u32,
    udp: Option<UdpSocket>,
    cipher: Option<Cipher>,
    /// Numero dell'ultimo messaggio ricevuto (v8: va confermato negli heartbeat).
    seq_ack: Option<i64>,
    sequence: u16,
    timestamp: u32,
    nonce: u32,
    stats: VoiceStats,
    /// Utente di ogni SSRC (dall'op 5 del server).
    users: HashMap<u32, String>,
    last_heard: HashMap<u32, Instant>,
    speaking: HashMap<u32, bool>,
}

impl Link {
    fn ready(&self) -> bool {
        self.cipher.is_some() && self.udp.is_some()
    }

    async fn send_opus(&mut self, frame: &[u8]) -> Result<()> {
        let (Some(cipher), Some(udp)) = (&self.cipher, &self.udp) else {
            return Ok(());
        };
        let header = RtpHeader {
            sequence: self.sequence,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
        };
        let packet = cipher.encrypt_rtp(&header, frame, self.nonce)?;
        udp.send(&packet).await?;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(rtp::SAMPLES_PER_FRAME);
        self.nonce = self.nonce.wrapping_add(1);
        self.stats.sent += 1;
        Ok(())
    }

    fn on_udp(&mut self, packet: &[u8]) {
        let Some(cipher) = &self.cipher else { return };
        if !rtp::is_audio(packet) {
            return;
        }
        match cipher.decrypt_rtp(packet) {
            Ok((header, audio)) => {
                self.stats.received += 1;
                // I frame di silenzio non contano come parlato.
                if audio != rtp::OPUS_SILENCE {
                    self.last_heard.insert(header.ssrc, Instant::now());
                }
            }
            Err(_) => self.stats.rejected += 1,
        }
    }

    /// Chi ha iniziato o smesso di parlare, dai pacchetti ricevuti.
    fn speaking_changes(&mut self) -> Vec<(String, bool)> {
        let now = Instant::now();
        let mut changes = Vec::new();
        for (ssrc, heard) in &self.last_heard {
            let speaking = now.duration_since(*heard) < SPEAKING_HOLD;
            let Some(user) = self.users.get(ssrc) else { continue };
            if self.speaking.get(ssrc).copied().unwrap_or(false) != speaking {
                self.speaking.insert(*ssrc, speaking);
                changes.push((user.clone(), speaking));
            }
        }
        changes
    }

    fn forget_user(&mut self, user_id: &str) -> bool {
        let ssrcs: Vec<u32> = self
            .users
            .iter()
            .filter(|(_, u)| u.as_str() == user_id)
            .map(|(s, _)| *s)
            .collect();
        let mut was_speaking = false;
        for s in ssrcs {
            self.users.remove(&s);
            self.last_heard.remove(&s);
            was_speaking |= self.speaking.remove(&s).unwrap_or(false);
        }
        was_speaking
    }
}

/// Connessione vocale con riconnessione automatica. Termina quando si esce
/// (comando `leave` o handle abbandonati), su un errore definitivo o quando
/// il ricevitore di `tx` viene chiuso.
pub async fn run<E: From<VoiceEvent> + Send + 'static>(server: VoiceServer, tx: UnboundedSender<E>) {
    let (cmd_tx, mut cmds) = mpsc::unbounded_channel();
    emit(&tx, VoiceEvent::Handle(VoiceHandle(cmd_tx)));
    let mut link = Link::default();
    let mut attempts = 0;
    loop {
        emit(&tx, VoiceEvent::Connecting);
        let was_ready = link.ready();
        match session(&server, &mut link, &mut cmds, &tx).await {
            Ok(()) => {
                info!("voce: uscito dal canale");
                emit(&tx, VoiceEvent::Closed(None));
                return;
            }
            Err(e) => {
                if let Some(Fatal(msg)) = e.downcast_ref::<Fatal>() {
                    warn!("voce: {msg}");
                    emit(&tx, VoiceEvent::Closed(Some(msg.clone())));
                    return;
                }
                // Una connessione che era arrivata all'audio azzera i tentativi.
                if was_ready || link.ready() {
                    attempts = 0;
                }
                attempts += 1;
                warn!("voce: {e:#} (tentativo {attempts})");
                if attempts >= MAX_ATTEMPTS || tx.is_closed() {
                    emit(&tx, VoiceEvent::Closed(Some(format!("{e:#}"))));
                    return;
                }
                emit(&tx, VoiceEvent::Reconnecting(format!("{e:#}")));
                tokio::time::sleep(Duration::from_millis(500 * attempts as u64)).await;
            }
        }
    }
}

#[derive(Deserialize)]
struct Hello {
    heartbeat_interval: f64,
}

#[derive(Deserialize)]
struct Ready {
    ssrc: u32,
    ip: String,
    port: u16,
    #[serde(default)]
    modes: Vec<String>,
}

#[derive(Deserialize)]
struct SessionDescription {
    mode: String,
    secret_key: Vec<u8>,
    #[serde(default)]
    dave_protocol_version: u8,
}

#[derive(Deserialize)]
struct Speaking {
    #[serde(default)]
    user_id: Option<String>,
    ssrc: u32,
}

#[derive(Deserialize)]
struct ClientDisconnect {
    user_id: String,
}

fn parse<T: serde::de::DeserializeOwned>(d: &Value) -> Result<T> {
    Ok(T::deserialize(d)?)
}

async fn recv_udp(udp: Option<&UdpSocket>, buf: &mut [u8]) -> std::io::Result<usize> {
    match udp {
        Some(s) => s.recv(buf).await,
        None => std::future::pending().await,
    }
}

async fn session<E: From<VoiceEvent>>(
    server: &VoiceServer,
    link: &mut Link,
    cmds: &mut UnboundedReceiver<VoiceCommand>,
    tx: &UnboundedSender<E>,
) -> Result<()> {
    let url = server.url();
    debug!("voce: connessione al server vocale");
    let (ws, _) = timeout(CONNECT_TIMEOUT, connect_async(url.as_str()))
        .await
        .context("timeout di connessione al server vocale")??;
    let (mut sink, mut stream) = ws.split();

    let mut heartbeat: Option<Interval> = None;
    let mut pending_beat: Option<(u64, Instant)> = None;
    let mut missed_beats = 0u32;
    let mut every_100ms = interval(Duration::from_millis(100));
    let mut every_second = interval(Duration::from_secs(1));
    let mut keepalive = interval(UDP_KEEPALIVE);
    for iv in [&mut every_100ms, &mut every_second, &mut keepalive] {
        iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    }
    let mut keepalive_count: u32 = 0;
    let mut buf = vec![0u8; 2048];

    loop {
        tokio::select! {
            _ = tick_opt(&mut heartbeat) => {
                if pending_beat.is_some() {
                    missed_beats += 1;
                    if missed_beats >= 3 {
                        bail!("il server vocale non risponde agli heartbeat");
                    }
                }
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or_default();
                send_json(&mut sink, &json!({"op": 3, "d": {"t": nonce, "seq_ack": link.seq_ack}})).await?;
                pending_beat = Some((nonce, Instant::now()));
            }
            msg = stream.next() => {
                let msg = msg.ok_or_else(|| anyhow!("il server vocale ha chiuso la connessione"))??;
                let text = match msg {
                    WsMessage::Text(t) => t,
                    // I messaggi binari sono di DAVE, non ancora gestito.
                    WsMessage::Binary(_) => continue,
                    WsMessage::Close(frame) => return Err(close_error(frame)),
                    _ => continue,
                };
                let v: Value = serde_json::from_str(&text).context("messaggio vocale non valido")?;
                if let Some(seq) = v["seq"].as_i64() {
                    link.seq_ack = Some(seq);
                }
                let d = &v["d"];
                match v["op"].as_u64().unwrap_or(u64::MAX) {
                    // Hello: si parte con gli heartbeat e ci si presenta
                    8 => {
                        let hello: Hello = parse(d)?;
                        let period = Duration::from_millis((hello.heartbeat_interval as u64).max(100));
                        let mut iv = interval_at(Instant::now() + period, period);
                        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
                        heartbeat = Some(iv);
                        if link.ready() {
                            send_json(&mut sink, &json!({"op": 7, "d": {
                                "server_id": server.guild_id,
                                "session_id": server.session_id,
                                "token": server.token,
                                "seq_ack": link.seq_ack,
                            }})).await?;
                        } else {
                            send_json(&mut sink, &json!({"op": 0, "d": {
                                "server_id": server.guild_id,
                                "user_id": server.user_id,
                                "session_id": server.session_id,
                                "token": server.token,
                                "max_dave_protocol_version": 0,
                            }})).await?;
                        }
                    }
                    // Ready: SSRC e server UDP; IP discovery e scelta della cifratura
                    2 => {
                        let ready: Ready = parse(d)?;
                        let mode = Mode::choose(ready.modes.iter().map(String::as_str))
                            .ok_or_else(|| Fatal("nessuna cifratura dell'audio in comune con il server".into()))?;
                        let (udp, address, port) = discover(&ready).await?;
                        link.ssrc = ready.ssrc;
                        link.udp = Some(udp);
                        link.cipher = None;
                        send_json(&mut sink, &json!({"op": 1, "d": {
                            "protocol": "udp",
                            "data": {"address": address, "port": port, "mode": mode.name()},
                        }})).await?;
                    }
                    // Session Description: la chiave dell'audio
                    4 => {
                        let sd: SessionDescription = parse(d)?;
                        let mode = Mode::from_name(&sd.mode)
                            .ok_or_else(|| Fatal(format!("cifratura sconosciuta: {}", sd.mode)))?;
                        link.cipher = Some(Cipher::new(mode, &sd.secret_key)?);
                        if sd.dave_protocol_version > 0 {
                            warn!("voce: il server chiede DAVE v{}, non gestito", sd.dave_protocol_version);
                        }
                        info!("voce: connesso ({})", mode.name());
                        emit(tx, VoiceEvent::Connected { mode: mode.name() });
                    }
                    // Heartbeat ACK: il ping
                    6 => {
                        if let Some((nonce, at)) = pending_beat {
                            if d["t"].as_u64() == Some(nonce) {
                                link.stats.ping_ms = Some(at.elapsed().as_millis() as u32);
                                pending_beat = None;
                                missed_beats = 0;
                            }
                        }
                    }
                    // Resumed: si continua con la chiave di prima
                    9 => {
                        if let Some(cipher) = &link.cipher {
                            emit(tx, VoiceEvent::Connected { mode: cipher.mode().name() });
                        }
                    }
                    // Speaking: a chi appartiene un SSRC
                    5 => {
                        let s: Speaking = parse(d)?;
                        if let Some(user) = s.user_id {
                            link.users.insert(s.ssrc, user);
                        }
                    }
                    // Un utente è uscito dal canale
                    13 => {
                        let c: ClientDisconnect = parse(d)?;
                        if link.forget_user(&c.user_id) {
                            emit(tx, VoiceEvent::Speaking { user_id: c.user_id, speaking: false });
                        }
                    }
                    _ => {}
                }
            }
            r = recv_udp(link.udp.as_ref(), &mut buf) => {
                let n = r.context("ricezione UDP")?;
                link.on_udp(&buf[..n]);
            }
            cmd = cmds.recv() => match cmd {
                Some(VoiceCommand::Opus(frame)) => link.send_opus(&frame).await?,
                Some(VoiceCommand::Speaking(on)) => {
                    if link.ready() {
                        send_json(&mut sink, &json!({"op": 5, "d": {
                            "speaking": u8::from(on), "delay": 0, "ssrc": link.ssrc,
                        }})).await?;
                    }
                }
                // Uscita, oppure nessuno ha più l'handle
                Some(VoiceCommand::Leave) | None => {
                    close(&mut sink).await;
                    return Ok(());
                }
            },
            _ = every_100ms.tick() => {
                for (user_id, speaking) in link.speaking_changes() {
                    emit(tx, VoiceEvent::Speaking { user_id, speaking });
                }
            }
            _ = every_second.tick() => {
                if link.ready() {
                    emit(tx, VoiceEvent::Stats(link.stats));
                }
            }
            _ = keepalive.tick() => {
                // Tiene aperto il NAT anche quando nessuno parla.
                if let Some(udp) = &link.udp {
                    keepalive_count = keepalive_count.wrapping_add(1);
                    let mut packet = [0u8; 8];
                    packet[..4].copy_from_slice(&keepalive_count.to_le_bytes());
                    let _ = udp.send(&packet).await;
                }
            }
        }
        if tx.is_closed() {
            close(&mut sink).await;
            return Ok(());
        }
    }
}

async fn close(sink: &mut WsSink) {
    let _ = sink.send(WsMessage::Close(None)).await;
}

/// Socket UDP verso il server vocale e indirizzo pubblico visto dal server.
async fn discover(ready: &Ready) -> Result<(UdpSocket, String, u16)> {
    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    udp.connect((ready.ip.as_str(), ready.port)).await?;
    udp.send(&rtp::discovery_request(ready.ssrc)).await?;
    let mut buf = [0u8; 128];
    let found = timeout(DISCOVERY_TIMEOUT, async {
        loop {
            let n = udp.recv(&mut buf).await?;
            if let Ok(found) = rtp::parse_discovery(&buf[..n], ready.ssrc) {
                return Ok::<_, anyhow::Error>(found);
            }
        }
    })
    .await
    .context("nessuna risposta all'IP discovery (UDP bloccato?)")??;
    Ok((udp, found.0, found.1))
}

fn close_error(frame: Option<CloseFrame<'_>>) -> anyhow::Error {
    let code = frame.as_ref().map(|f| u16::from(f.code));
    let fatal = |s: &str| anyhow::Error::new(Fatal(s.to_string()));
    match code {
        Some(4004) => fatal("il server vocale ha rifiutato l'autenticazione"),
        Some(4006) => fatal("sessione vocale non più valida"),
        Some(4011) => fatal("server vocale non trovato"),
        Some(4014) => fatal("disconnesso dal canale vocale"),
        Some(4016) => fatal("cifratura dell'audio non accettata dal server"),
        Some(4017) => fatal(
            "Discord richiede la crittografia end-to-end (DAVE), non ancora supportata da ferrocord",
        ),
        Some(4015) => anyhow!("il server vocale si è riavviato"),
        Some(c) => anyhow!("connessione vocale chiusa (codice {c})"),
        None => anyhow!("connessione vocale chiusa"),
    }
}

#[cfg(test)]
mod tests {
    use super::mock::{self, MockConfig};
    use super::*;
    use std::sync::atomic::Ordering;

    fn server(endpoint: &str) -> VoiceServer {
        VoiceServer {
            endpoint: endpoint.to_string(),
            token: "token-finto".into(),
            guild_id: "100".into(),
            session_id: "sessione".into(),
            user_id: "900".into(),
        }
    }

    /// Aspetta il primo evento che soddisfa `f`, entro qualche secondo.
    async fn wait_for(rx: &mut UnboundedReceiver<VoiceEvent>, f: impl Fn(&VoiceEvent) -> bool) -> VoiceEvent {
        timeout(Duration::from_secs(10), async {
            loop {
                let ev = rx.recv().await.expect("canale chiuso");
                if f(&ev) {
                    return ev;
                }
            }
        })
        .await
        .expect("evento non arrivato")
    }

    #[test]
    fn indirizzo_del_server_vocale() {
        assert_eq!(server("c-mxp08.discord.media").url(), "wss://c-mxp08.discord.media/?v=8");
        assert_eq!(server("ws://127.0.0.1:5000/").url(), "ws://127.0.0.1:5000/?v=8");
    }

    #[tokio::test]
    async fn connessione_completa_con_il_server_finto() {
        let mock = mock::start(MockConfig {
            users: vec!["901".into(), "902".into()],
            heartbeat_ms: 100,
            ..Default::default()
        })
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let task = tokio::spawn(run(server(&mock.endpoint), tx));

        let VoiceEvent::Handle(handle) = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Handle(_))).await else {
            unreachable!()
        };
        let connected = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Connected { .. })).await;
        assert!(matches!(connected, VoiceEvent::Connected { mode: "aead_aes256_gcm_rtpsize" }));

        // L'audio degli altri arriva, si decifra e dice chi parla.
        wait_for(&mut rx, |e| {
            matches!(e, VoiceEvent::Speaking { user_id, speaking: true } if user_id == "901")
        })
        .await;
        let VoiceEvent::Stats(stats) = wait_for(&mut rx, |e| {
            matches!(e, VoiceEvent::Stats(s) if s.ping_ms.is_some() && s.received > 0)
        })
        .await
        else {
            unreachable!()
        };
        assert_eq!(stats.rejected, 0);

        // Il nostro audio arriva al server, cifrato con la chiave concordata.
        handle.set_speaking(true);
        for _ in 0..10 {
            handle.send_opus(vec![0x78; 40]);
        }
        timeout(Duration::from_secs(5), async {
            while mock.received.load(Ordering::Relaxed) < 10 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("audio non arrivato al server");

        handle.leave();
        let closed = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(closed, VoiceEvent::Closed(None)));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn seconda_cifratura_se_il_server_offre_solo_quella() {
        let mock = mock::start(MockConfig {
            modes: vec![Mode::XChaCha20Poly1305.name()],
            ..Default::default()
        })
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let task = tokio::spawn(run(server(&mock.endpoint), tx));
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Connected { .. })).await;
        assert!(matches!(ev, VoiceEvent::Connected { mode: "aead_xchacha20_poly1305_rtpsize" }));
        task.abort();
    }

    #[tokio::test]
    async fn rifiuto_senza_dave_e_definitivo() {
        let mock = mock::start(MockConfig {
            close_after_identify: Some(4017),
            ..Default::default()
        })
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let task = tokio::spawn(run(server(&mock.endpoint), tx));
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        let VoiceEvent::Closed(Some(reason)) = ev else {
            panic!("chiusura senza motivo")
        };
        assert!(reason.contains("DAVE"), "{reason}");
        // Nessun nuovo tentativo: il task è finito.
        timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn server_che_cade_si_riprova() {
        let mock = mock::start(MockConfig {
            close_after_identify: Some(4015),
            ..Default::default()
        })
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let task = tokio::spawn(run(server(&mock.endpoint), tx));
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Reconnecting(_))).await;
        // Dopo i tentativi previsti si arrende con il motivo.
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(ev, VoiceEvent::Closed(Some(_))));
        task.await.unwrap();
    }
}

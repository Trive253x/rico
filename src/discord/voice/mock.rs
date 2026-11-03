//! Server vocale finto, solo in locale (127.0.0.1): parla lo stesso protocollo
//! dei server di Discord (gateway vocale v8, IP discovery, RTP cifrato).
//!
//! Serve alla modalità demo e ai test: tutta la connessione vocale si prova
//! senza account e senza rete. Simula alcuni utenti che parlano a turno
//! (pacchetti audio finti, non Opus vero) e conta l'audio che riceve.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::crypto::{Cipher, Mode};
use super::rtp::{self, RtpHeader};

/// SSRC assegnato al client; quelli degli utenti finti partono da 1000.
const CLIENT_SSRC: u32 = 4242;

#[derive(Debug, Clone)]
pub struct MockConfig {
    /// ID degli utenti finti che parlano a turno.
    pub users: Vec<String>,
    pub heartbeat_ms: u64,
    /// Cifrature offerte (nomi del protocollo).
    pub modes: Vec<&'static str>,
    /// Chiude subito dopo l'identify con questo codice (per i test).
    pub close_after_identify: Option<u16>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            users: Vec::new(),
            heartbeat_ms: 5000,
            modes: vec![Mode::XChaCha20Poly1305.name(), Mode::Aes256Gcm.name()],
            close_after_identify: None,
        }
    }
}

/// Server in esecuzione: si ferma quando viene abbandonato.
pub struct MockVoice {
    /// Indirizzo `ws://` da usare come endpoint.
    pub endpoint: String,
    /// Pacchetti audio del client ricevuti e decifrati.
    pub received: Arc<AtomicU64>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for MockVoice {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[derive(Default)]
struct Shared {
    cipher: Option<Arc<Cipher>>,
    client: Option<SocketAddr>,
}

pub async fn start(cfg: MockConfig) -> Result<MockVoice> {
    let tcp = TcpListener::bind("127.0.0.1:0").await?;
    let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let endpoint = format!("ws://{}", tcp.local_addr()?);
    let shared = Arc::new(Mutex::new(Shared::default()));
    let received = Arc::new(AtomicU64::new(0));

    let udp_task = tokio::spawn(udp_loop(udp.clone(), shared.clone(), received.clone()));
    let ws_task = tokio::spawn(async move {
        let mut sessions: Vec<JoinHandle<()>> = Vec::new();
        while let Ok((stream, _)) = tcp.accept().await {
            let (udp, shared, cfg) = (udp.clone(), shared.clone(), cfg.clone());
            sessions.push(tokio::spawn(async move {
                if let Err(e) = handle(stream, udp, shared, cfg).await {
                    tracing::debug!("server vocale finto: {e:#}");
                }
            }));
        }
        for s in sessions {
            s.abort();
        }
    });
    Ok(MockVoice {
        endpoint,
        received,
        tasks: vec![udp_task, ws_task],
    })
}

async fn udp_loop(udp: Arc<UdpSocket>, shared: Arc<Mutex<Shared>>, received: Arc<AtomicU64>) {
    let mut buf = [0u8; 2048];
    while let Ok((n, from)) = udp.recv_from(&mut buf).await {
        let packet = &buf[..n];
        if let Some(ssrc) = rtp::is_discovery_request(packet) {
            shared.lock().unwrap().client = Some(from);
            let reply = rtp::discovery_response(ssrc, &from.ip().to_string(), from.port());
            let _ = udp.send_to(&reply, from).await;
            continue;
        }
        let cipher = shared.lock().unwrap().cipher.clone();
        if let Some(cipher) = cipher {
            if rtp::is_audio(packet) && cipher.decrypt_rtp(packet).is_ok() {
                received.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn handle(
    stream: TcpStream,
    udp: Arc<UdpSocket>,
    shared: Arc<Mutex<Shared>>,
    cfg: MockConfig,
) -> Result<()> {
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let udp_port = udp.local_addr()?.port();
    let mut seq: i64 = 0;
    let mut out = |op: u8, d: Value| {
        seq += 1;
        WsMessage::Text(json!({"op": op, "d": d, "seq": seq}).to_string())
    };
    let mut speakers: Option<JoinHandle<()>> = None;

    ws.send(out(
        8,
        json!({"v": 8, "heartbeat_interval": cfg.heartbeat_ms as f64}),
    ))
    .await?;
    while let Some(msg) = ws.next().await {
        let WsMessage::Text(text) = msg? else { continue };
        let v: Value = serde_json::from_str(&text)?;
        let d = &v["d"];
        match v["op"].as_u64() {
            // Identify
            Some(0) => {
                if let Some(code) = cfg.close_after_identify {
                    ws.close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: "".into(),
                    }))
                    .await?;
                    break;
                }
                ws.send(out(
                    2,
                    json!({"ssrc": CLIENT_SSRC, "ip": "127.0.0.1", "port": udp_port, "modes": cfg.modes}),
                ))
                .await?;
                ws.send(out(11, json!({"user_ids": cfg.users}))).await?;
            }
            // Select Protocol: si genera la chiave e gli utenti iniziano a parlare
            Some(1) => {
                let mode_name = d["data"]["mode"].as_str().unwrap_or_default();
                let Some(mode) = Mode::from_name(mode_name).filter(|m| cfg.modes.contains(&m.name())) else {
                    ws.close(Some(CloseFrame {
                        code: CloseCode::from(4016),
                        reason: "".into(),
                    }))
                    .await?;
                    break;
                };
                let key: [u8; 32] = rand::random();
                let cipher = Arc::new(Cipher::new(mode, &key)?);
                shared.lock().unwrap().cipher = Some(cipher.clone());
                ws.send(out(
                    4,
                    json!({"mode": mode.name(), "secret_key": key.to_vec(), "dave_protocol_version": 0}),
                ))
                .await?;
                for (i, user) in cfg.users.iter().enumerate() {
                    ws.send(out(
                        5,
                        json!({"user_id": user, "ssrc": 1000 + i as u32, "speaking": 1}),
                    ))
                    .await?;
                }
                if let Some(old) = speakers.take() {
                    old.abort();
                }
                speakers = Some(tokio::spawn(talk(
                    udp.clone(),
                    shared.clone(),
                    cipher,
                    cfg.users.len(),
                )));
            }
            // Heartbeat
            Some(3) => ws.send(out(6, json!({"t": d["t"]}))).await?,
            // Resume
            Some(7) => ws.send(out(9, Value::Null)).await?,
            _ => {}
        }
    }
    if let Some(s) = speakers {
        s.abort();
    }
    Ok(())
}

/// Gli utenti finti parlano a turno: 1,2 s di "audio", qualche frame di
/// silenzio, una pausa, poi tocca al successivo.
async fn talk(udp: Arc<UdpSocket>, shared: Arc<Mutex<Shared>>, cipher: Arc<Cipher>, users: usize) {
    if users == 0 {
        return;
    }
    let mut seq = vec![0u16; users];
    let mut nonce = 0u32;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        for (i, s) in seq.iter_mut().enumerate() {
            for frame in 0..65 {
                tick.tick().await;
                let Some(client) = shared.lock().unwrap().client else {
                    continue;
                };
                let audio: &[u8] = if frame < 60 {
                    &[0x78; 40]
                } else {
                    &rtp::OPUS_SILENCE
                };
                let header = RtpHeader {
                    sequence: *s,
                    timestamp: u32::from(*s).wrapping_mul(rtp::SAMPLES_PER_FRAME),
                    ssrc: 1000 + i as u32,
                };
                *s = s.wrapping_add(1);
                nonce = nonce.wrapping_add(1);
                if let Ok(packet) = cipher.encrypt_rtp(&header, audio, nonce) {
                    let _ = udp.send_to(&packet, client).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
}

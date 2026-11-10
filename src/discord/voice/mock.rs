//! Local fake voice server (127.0.0.1 only): speaks the same protocol as
//! Discord's voice servers (voice gateway v8, IP discovery, encrypted RTP and
//! DAVE end-to-end encryption).
//!
//! Used by the demo mode and the tests: the whole voice connection can be
//! exercised without an account and without the network. The fake
//! participants are real DAVE members: the server acts as the MLS external
//! sender, they join the same group as the client and talk in turn with a
//! synthetic voice ([`Hum`]) encoded with Opus and end-to-end encrypted.
//!
//! It also takes a Go Live stream: our video packets are put back together
//! and end-to-end decrypted by a participant (that is what the counters
//! say), and it can ask for a keyframe like a viewer would (RTCP PLI).

use std::collections::VecDeque;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use davey::{DaveSession, MediaType, ProposalsOperationType};
use futures_util::{SinkExt, StreamExt};
use openmls::prelude::tls_codec::{DeserializeBytes, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::WebSocketStream;

use super::crypto::{Cipher, Mode};
use super::dave::op;
use super::rtp::{self, RtpHeader};
use crate::audio::codec::Encoder;
use crate::audio::dsp::Hum;
use crate::video::h264::{self, Depacketizer};

/// Default voice channel (MLS group) of the fake call.
pub const CHANNEL_ID: u64 = 1021;
/// SSRC given to the client; the participants' ones start at 1000.
const CLIENT_SSRC: u32 = 4242;
/// SSRCs for the client's video and its retransmissions.
const CLIENT_VIDEO_SSRC: u32 = CLIENT_SSRC + 1;
const CLIENT_RTX_SSRC: u32 = CLIENT_SSRC + 2;
/// Decrypted video frames kept for the tests.
const KEPT_FRAMES: usize = 60;
const DAVE_VERSION: u16 = 1;

#[derive(Debug, Clone)]
pub struct MockConfig {
    /// IDs of the fake participants (numeric), who talk in turn.
    pub users: Vec<String>,
    pub heartbeat_ms: u64,
    /// Transport encryption modes offered (protocol names).
    pub modes: Vec<&'static str>,
    /// Closes right after identify with this code (for tests).
    pub close_after_identify: Option<u16>,
    /// Offers DAVE; without it the session has no end-to-end encryption.
    pub dave: bool,
    /// The participants join after the client, which then has to commit.
    pub join_after_client: bool,
    /// Voice channel: the MLS group id.
    pub channel_id: u64,
    /// Misbehaving participants: their frames skip end-to-end encryption
    /// (only the transport is encrypted). The client must drop them.
    pub skip_e2ee: bool,
    /// After this many video frames from the client, ask for a keyframe once.
    pub ask_keyframe_after: Option<u64>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            users: Vec::new(),
            heartbeat_ms: 5000,
            modes: vec![Mode::XChaCha20Poly1305.name(), Mode::Aes256Gcm.name()],
            close_after_identify: None,
            dave: true,
            join_after_client: false,
            channel_id: CHANNEL_ID,
            skip_e2ee: false,
            ask_keyframe_after: None,
        }
    }
}

/// Running server: stops when dropped.
pub struct MockVoice {
    /// `ws://` address to use as endpoint.
    pub endpoint: String,
    /// Client audio frames received and end-to-end decrypted by a participant.
    pub received: Arc<AtomicU64>,
    /// The client's video: frames and keyframes end-to-end decrypted, and
    /// the last decrypted frames (Annex B).
    pub video: Arc<VideoCounters>,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Default)]
pub struct VideoCounters {
    pub frames: AtomicU64,
    pub keyframes: AtomicU64,
    /// Retransmissions received.
    pub retransmitted: AtomicU64,
    pub last_frames: Mutex<VecDeque<Vec<u8>>>,
}

impl Drop for MockVoice {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn err<E: Debug>(e: E) -> anyhow::Error {
    anyhow!("{e:?}")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The voice gateway's MLS identity: it signs the add proposals.
struct ExternalSenderKeys {
    group_id: GroupId,
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    package: Vec<u8>,
}

impl ExternalSenderKeys {
    fn new(channel_id: u64) -> Result<Self> {
        let suite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
        let signer = SignatureKeyPair::new(suite.signature_algorithm()).map_err(err)?;
        let sender = ExternalSender::new(
            signer.public().into(),
            BasicCredential::new(vec![0x00, 0x01, 0x01, 0x00]).into(),
        );
        let package = sender.tls_serialize_detached().map_err(err)?;
        Ok(Self {
            group_id: GroupId::from_slice(&channel_id.to_be_bytes()),
            provider: OpenMlsRustCrypto::default(),
            signer,
            package,
        })
    }

    /// Payload of an append proposals message adding the owners of `key_packages`.
    fn propose_add(&self, epoch: GroupEpoch, key_packages: &[Vec<u8>]) -> Result<Vec<u8>> {
        let mut messages = Vec::new();
        for kp in key_packages {
            let kp = KeyPackageIn::tls_deserialize_exact_bytes(kp)
                .map_err(err)?
                .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                .map_err(err)?;
            let message = ExternalProposal::new_add::<OpenMlsRustCrypto>(
                kp,
                self.group_id.clone(),
                epoch,
                &self.signer,
                SenderExtensionIndex::new(0),
            )
            .map_err(err)?;
            messages.extend(message.tls_serialize_detached().map_err(err)?);
        }
        VLBytes::new(messages).tls_serialize_detached().map_err(err)
    }
}

struct Participant {
    id: u64,
    ssrc: u32,
    session: DaveSession,
    encoder: Encoder,
    hum: Hum,
    sequence: u16,
    timestamp: u32,
}

impl Participant {
    fn new(id: u64, index: usize, channel_id: u64, sender: &ExternalSenderKeys) -> Result<Self> {
        let version = NonZeroU16::new(DAVE_VERSION).context("version")?;
        let mut session = DaveSession::new(version, id, channel_id, None).map_err(err)?;
        session.set_external_sender(&sender.package).map_err(err)?;
        Ok(Self {
            id,
            ssrc: 1000 + index as u32,
            session,
            encoder: Encoder::new()?,
            hum: Hum::new(140.0 + 45.0 * index as f32),
            sequence: 0,
            timestamp: 0,
        })
    }
}

/// End-to-end encryption state of the fake participants.
struct E2ee {
    sender: ExternalSenderKeys,
    participants: Vec<Participant>,
    /// Everybody shares the group with the client: the participants talk.
    active: bool,
    next_transition: u16,
}

impl E2ee {
    fn new(ids: &[u64], client: u64, channel_id: u64, join_after_client: bool) -> Result<Self> {
        let sender = ExternalSenderKeys::new(channel_id)?;
        let participants = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Participant::new(*id, i, channel_id, &sender))
            .collect::<Result<Vec<_>>>()?;
        let mut e2ee = Self {
            sender,
            participants,
            active: false,
            next_transition: 1,
        };
        if !join_after_client && e2ee.participants.len() > 1 {
            e2ee.establish(client)?;
        }
        Ok(e2ee)
    }

    fn everyone(&self, client: u64) -> Vec<u64> {
        let mut ids: Vec<u64> = self.participants.iter().map(|p| p.id).collect();
        ids.push(client);
        ids
    }

    fn transition(&mut self) -> u16 {
        let t = self.next_transition;
        self.next_transition = self.next_transition.wrapping_add(1).max(1);
        t
    }

    /// The first participant creates the group with the others (before the
    /// client arrives).
    fn establish(&mut self, client: u64) -> Result<()> {
        let ids = self.everyone(client);
        let kps = self.participants[1..]
            .iter_mut()
            .map(|p| p.session.create_key_package().map_err(err))
            .collect::<Result<Vec<_>>>()?;
        let proposals = self.sender.propose_add(GroupEpoch::from(0), &kps)?;
        let creator = &mut self.participants[0].session;
        let cw = creator
            .process_proposals(ProposalsOperationType::APPEND, &proposals, Some(&ids))
            .map_err(err)?
            .context("no commit")?;
        creator.process_commit(&cw.commit).map_err(err)?;
        let welcome = cw.welcome.context("no welcome")?;
        for p in &mut self.participants[1..] {
            p.session.process_welcome(&welcome, Some(&ids)).map_err(err)?;
        }
        for p in &mut self.participants {
            p.session.prepare_transition(0, DAVE_VERSION).map_err(err)?;
        }
        Ok(())
    }

    /// The client sent its key package: the first participant commits its
    /// addition. Returns the welcome message (op 30) for the client.
    fn add_client(&mut self, client: u64, key_package: &[u8]) -> Result<Vec<u8>> {
        let ids = self.everyone(client);
        let epoch = self.participants[0].session.epoch().context("no group")?;
        let proposals = self.sender.propose_add(epoch, &[key_package.to_vec()])?;
        let mut commit_welcome = None;
        for p in &mut self.participants {
            let cw = p
                .session
                .process_proposals(ProposalsOperationType::APPEND, &proposals, Some(&ids))
                .map_err(err)?;
            commit_welcome.get_or_insert(cw);
        }
        let cw = commit_welcome.flatten().context("no commit")?;
        for p in &mut self.participants {
            p.session.process_commit(&cw.commit).map_err(err)?;
        }
        let tid = self.transition();
        for p in &mut self.participants {
            p.session.prepare_transition(tid, DAVE_VERSION).map_err(err)?;
        }
        let mut out = tid.to_be_bytes().to_vec();
        out.extend(cw.welcome.context("no welcome")?);
        Ok(out)
    }

    /// Proposals (op 27) adding the participants to the client's group.
    fn propose_participants(&mut self) -> Result<Vec<u8>> {
        let kps = self
            .participants
            .iter_mut()
            .map(|p| p.session.create_key_package().map_err(err))
            .collect::<Result<Vec<_>>>()?;
        let mut out = vec![0u8]; // append
        out.extend(self.sender.propose_add(GroupEpoch::from(0), &kps)?);
        Ok(out)
    }

    /// The client committed (op 28): the participants join through the
    /// welcome. Returns the commit announcement (op 29) for the client.
    fn client_committed(&mut self, client: u64, commit_welcome: &[u8]) -> Result<Vec<u8>> {
        let (_, rest) = MlsMessageIn::tls_deserialize_bytes(commit_welcome).map_err(err)?;
        let commit = &commit_welcome[..commit_welcome.len() - rest.len()];
        if rest.is_empty() {
            bail!("commit without welcome");
        }
        let ids = self.everyone(client);
        let tid = self.transition();
        for p in &mut self.participants {
            p.session.process_welcome(rest, Some(&ids)).map_err(err)?;
            p.session.prepare_transition(tid, DAVE_VERSION).map_err(err)?;
        }
        let mut out = tid.to_be_bytes().to_vec();
        out.extend_from_slice(commit);
        Ok(out)
    }

    fn execute(&mut self, tid: u16) -> Result<()> {
        for p in &mut self.participants {
            p.session.execute_transition(tid).map_err(err)?;
        }
        self.active = self.participants.iter().all(|p| p.session.can_encrypt());
        Ok(())
    }
}

#[derive(Default)]
struct Shared {
    cipher: Option<Arc<Cipher>>,
    client: Option<SocketAddr>,
    client_user: u64,
    e2ee: Option<E2ee>,
    skip_e2ee: bool,
    video: Option<Depacketizer>,
    ask_keyframe_after: Option<u64>,
    rtcp_nonce: u32,
}

pub async fn start(cfg: MockConfig) -> Result<MockVoice> {
    let tcp = TcpListener::bind("127.0.0.1:0").await?;
    let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let endpoint = format!("ws://{}", tcp.local_addr()?);
    let shared = Arc::new(Mutex::new(Shared::default()));
    let received = Arc::new(AtomicU64::new(0));
    let video = Arc::new(VideoCounters::default());
    lock(&shared).ask_keyframe_after = cfg.ask_keyframe_after;

    let udp_task = tokio::spawn(udp_loop(
        udp.clone(),
        shared.clone(),
        received.clone(),
        video.clone(),
    ));
    let talk_task = tokio::spawn(talk(udp.clone(), shared.clone()));
    let ws_task = tokio::spawn(async move {
        let mut sessions: Vec<JoinHandle<()>> = Vec::new();
        while let Ok((stream, _)) = tcp.accept().await {
            let (udp, shared, cfg) = (udp.clone(), shared.clone(), cfg.clone());
            sessions.push(tokio::spawn(async move {
                if let Err(e) = handle(stream, udp, shared, cfg).await {
                    tracing::debug!("fake voice server: {e:#}");
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
        video,
        tasks: vec![udp_task, talk_task, ws_task],
    })
}

/// A video packet from the client: when its frame is complete, a
/// participant end-to-end decrypts it. Returns a PLI to send, if it is time
/// to ask for a keyframe.
fn on_client_video(s: &mut Shared, cipher: &Cipher, packet: &[u8], video: &VideoCounters) -> Option<Vec<u8>> {
    let (parsed, payload) = cipher.decrypt_packet(packet).ok()?;
    let (seq, payload) = match parsed.header.ssrc {
        CLIENT_VIDEO_SSRC => (parsed.header.sequence, payload.as_slice()),
        CLIENT_RTX_SSRC => {
            video.retransmitted.fetch_add(1, Ordering::Relaxed);
            let (seq, original) = rtp::rtx_original(&payload)?;
            (seq, original)
        }
        _ => return None,
    };
    let client = s.client_user;
    let frames = s
        .video
        .get_or_insert_with(Depacketizer::default)
        .push(seq, parsed.header.timestamp, parsed.marker, payload)
        .frames;
    let p = s.e2ee.as_mut().and_then(|e| e.participants.first_mut())?;
    for frame in frames {
        let Ok(plain) = p.session.decrypt(client, MediaType::VIDEO, &frame) else {
            continue;
        };
        video.frames.fetch_add(1, Ordering::Relaxed);
        if h264::is_keyframe(&plain) {
            video.keyframes.fetch_add(1, Ordering::Relaxed);
        }
        let mut kept = lock(&video.last_frames);
        if kept.len() >= KEPT_FRAMES {
            kept.pop_front();
        }
        kept.push_back(plain);
    }
    let after = s.ask_keyframe_after?;
    if video.frames.load(Ordering::Relaxed) < after {
        return None;
    }
    s.ask_keyframe_after = None;
    s.rtcp_nonce = s.rtcp_nonce.wrapping_add(1);
    let (clear, body) = rtp::picture_loss(1, CLIENT_VIDEO_SSRC);
    cipher.encrypt_rtcp(&clear, &body, s.rtcp_nonce).ok()
}

async fn udp_loop(
    udp: Arc<UdpSocket>,
    shared: Arc<Mutex<Shared>>,
    received: Arc<AtomicU64>,
    video: Arc<VideoCounters>,
) {
    let mut buf = [0u8; 2048];
    while let Ok((n, from)) = udp.recv_from(&mut buf).await {
        let packet = &buf[..n];
        if let Some(ssrc) = rtp::is_discovery_request(packet) {
            lock(&shared).client = Some(from);
            let reply = rtp::discovery_response(ssrc, &from.ip().to_string(), from.port());
            let _ = udp.send_to(&reply, from).await;
            continue;
        }
        let pli = {
            let mut s = lock(&shared);
            let Some(cipher) = s.cipher.clone() else { continue };
            if rtp::is_media(packet) && !rtp::is_audio(packet) {
                on_client_video(&mut s, &cipher, packet, &video)
            } else {
                None
            }
        };
        if let Some(pli) = pli {
            let _ = udp.send_to(&pli, from).await;
            continue;
        }
        let mut s = lock(&shared);
        let Some(cipher) = s.cipher.clone() else { continue };
        if !rtp::is_audio(packet) {
            continue;
        }
        let Ok((_, payload)) = cipher.decrypt_rtp(packet) else {
            continue;
        };
        if payload == rtp::OPUS_SILENCE {
            continue;
        }
        let client = s.client_user;
        // Counted only if a participant can end-to-end decrypt it.
        if let Some(p) = s.e2ee.as_mut().and_then(|e| e.participants.first_mut()) {
            if p.session.decrypt(client, MediaType::AUDIO, &payload).is_ok() {
                received.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

struct Out {
    seq: i64,
}

impl Out {
    fn json(&mut self, op: u8, d: Value) -> WsMessage {
        self.seq += 1;
        WsMessage::Text(json!({"op": op, "d": d, "seq": self.seq}).to_string())
    }

    fn binary(&mut self, op: u8, payload: &[u8]) -> WsMessage {
        self.seq += 1;
        let mut m = (self.seq as u16).to_be_bytes().to_vec();
        m.push(op);
        m.extend_from_slice(payload);
        WsMessage::Binary(m)
    }
}

async fn close(ws: &mut WebSocketStream<TcpStream>, code: u16) -> Result<()> {
    ws.close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    }))
    .await?;
    Ok(())
}

async fn handle(
    stream: TcpStream,
    udp: Arc<UdpSocket>,
    shared: Arc<Mutex<Shared>>,
    cfg: MockConfig,
) -> Result<()> {
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    let udp_port = udp.local_addr()?.port();
    let mut out = Out { seq: 0 };
    let participants: Vec<u64> = cfg.users.iter().filter_map(|u| u.parse().ok()).collect();
    let announce = |out: &mut Out| -> Vec<WsMessage> {
        let mut msgs = vec![out.json(11, json!({"user_ids": cfg.users}))];
        for (i, user) in cfg.users.iter().enumerate() {
            msgs.push(out.json(
                5,
                json!({"user_id": user, "ssrc": 1000 + i as u32, "speaking": 1}),
            ));
        }
        msgs
    };

    ws.send(out.json(8, json!({"v": 8, "heartbeat_interval": cfg.heartbeat_ms as f64})))
        .await?;
    while let Some(msg) = ws.next().await {
        let mut replies = Vec::new();
        match msg? {
            WsMessage::Text(text) => {
                let v: Value = serde_json::from_str(&text)?;
                let d = &v["d"];
                match v["op"].as_u64() {
                    // Identify
                    Some(0) => {
                        if let Some(code) = cfg.close_after_identify {
                            close(&mut ws, code).await?;
                            break;
                        }
                        let client_user = d["user_id"]
                            .as_str()
                            .and_then(|u| u.parse().ok())
                            .unwrap_or_default();
                        let mut s = lock(&shared);
                        s.client_user = client_user;
                        s.skip_e2ee = cfg.skip_e2ee;
                        drop(s);
                        replies.push(out.json(
                            2,
                            json!({"ssrc": CLIENT_SSRC, "ip": "127.0.0.1", "port": udp_port, "modes": cfg.modes,
                                   "streams": [{"type": "screen", "rid": "100", "ssrc": CLIENT_VIDEO_SSRC,
                                                "rtx_ssrc": CLIENT_RTX_SSRC, "quality": 100, "active": false}]}),
                        ));
                        if !cfg.join_after_client {
                            replies.extend(announce(&mut out));
                        }
                    }
                    // Select Protocol: transport key, then DAVE
                    Some(1) => {
                        let mode_name = d["data"]["mode"].as_str().unwrap_or_default();
                        let Some(mode) = Mode::from_name(mode_name).filter(|m| cfg.modes.contains(&m.name()))
                        else {
                            close(&mut ws, 4016).await?;
                            break;
                        };
                        let key: [u8; 32] = rand::random();
                        let version = if cfg.dave { DAVE_VERSION } else { 0 };
                        replies.push(out.json(
                            4,
                            json!({"mode": mode.name(), "secret_key": key.to_vec(), "dave_protocol_version": version}),
                        ));
                        let mut s = lock(&shared);
                        s.cipher = Some(Arc::new(Cipher::new(mode, &key)?));
                        if cfg.dave {
                            let e2ee = E2ee::new(
                                &participants,
                                s.client_user,
                                cfg.channel_id,
                                cfg.join_after_client,
                            )?;
                            replies.push(out.binary(op::EXTERNAL_SENDER, &e2ee.sender.package));
                            s.e2ee = Some(e2ee);
                        }
                    }
                    // Heartbeat
                    Some(3) => replies.push(out.json(6, json!({"t": d["t"]}))),
                    // Resume
                    Some(7) => replies.push(out.json(9, Value::Null)),
                    // Ready for transition: everyone switches to the new keys
                    Some(23) => {
                        let tid = d["transition_id"].as_u64().unwrap_or_default() as u16;
                        if let Some(e2ee) = lock(&shared).e2ee.as_mut() {
                            e2ee.execute(tid)?;
                        }
                        replies.push(out.json(op::EXECUTE_TRANSITION, json!({"transition_id": tid})));
                    }
                    Some(31) => tracing::debug!("fake voice server: the client rejected a commit or welcome"),
                    _ => {}
                }
            }
            WsMessage::Binary(b) => {
                let Some((&opcode, payload)) = b.split_first() else {
                    continue;
                };
                let mut s = lock(&shared);
                let client = s.client_user;
                let Some(e2ee) = s.e2ee.as_mut() else { continue };
                match opcode {
                    op::KEY_PACKAGE if cfg.join_after_client => {
                        let proposals = e2ee.propose_participants()?;
                        replies.extend(announce(&mut out));
                        replies.push(out.binary(op::PROPOSALS, &proposals));
                    }
                    op::KEY_PACKAGE => {
                        let welcome = e2ee.add_client(client, payload)?;
                        replies.push(out.binary(op::WELCOME, &welcome));
                    }
                    op::COMMIT_WELCOME => {
                        let announce_commit = e2ee.client_committed(client, payload)?;
                        replies.push(out.binary(op::ANNOUNCE_COMMIT, &announce_commit));
                    }
                    _ => {}
                }
            }
            _ => continue,
        }
        for r in replies {
            ws.send(r).await?;
        }
    }
    Ok(())
}

/// One frame from the participant whose turn it is, transport encrypted.
fn next_packet(s: &mut Shared, turn: usize, frame: u32, nonce: u32) -> Option<(Vec<u8>, SocketAddr)> {
    let client = s.client?;
    let cipher = s.cipher.clone()?;
    let skip_e2ee = s.skip_e2ee;
    let e2ee = s.e2ee.as_mut().filter(|e| e.active)?;
    let p = e2ee.participants.get_mut(turn)?;
    let payload = if frame < 60 {
        let opus = p.encoder.encode(&p.hum.next_frame()).ok()?;
        if skip_e2ee {
            opus
        } else {
            p.session.encrypt_opus(&opus).ok()?.into_owned()
        }
    } else {
        rtp::OPUS_SILENCE.to_vec()
    };
    let header = RtpHeader {
        sequence: p.sequence,
        timestamp: p.timestamp,
        ssrc: p.ssrc,
    };
    p.sequence = p.sequence.wrapping_add(1);
    p.timestamp = p.timestamp.wrapping_add(rtp::SAMPLES_PER_FRAME);
    let packet = cipher.encrypt_rtp(&header, &payload, nonce).ok()?;
    Some((packet, client))
}

/// The participants talk in turn: 1.2 s of voice, a few silence frames, a
/// pause, then the next one.
async fn talk(udp: Arc<UdpSocket>, shared: Arc<Mutex<Shared>>) {
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let (mut turn, mut frame, mut nonce) = (0usize, 0u32, 0u32);
    loop {
        tick.tick().await;
        let packet = {
            let mut s = lock(&shared);
            let count = s
                .e2ee
                .as_ref()
                .filter(|e| e.active)
                .map_or(0, |e| e.participants.len());
            if count == 0 {
                continue;
            }
            turn %= count;
            if frame >= 85 {
                frame = 0;
                turn = (turn + 1) % count;
            }
            frame += 1;
            if frame > 65 {
                continue;
            }
            nonce = nonce.wrapping_add(1);
            next_packet(&mut s, turn, frame - 1, nonce)
        };
        if let Some((packet, to)) = packet {
            let _ = udp.send_to(&packet, to).await;
        }
    }
}

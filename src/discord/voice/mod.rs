//! Voice connection: voice gateway (v8), UDP transport, end-to-end
//! encryption (DAVE) and the audio pipeline.
//!
//! Joining a voice channel takes two steps:
//! 1. on the main gateway we send op 4 (voice state) and Discord answers with
//!    `VOICE_STATE_UPDATE` (our `session_id`) and `VOICE_SERVER_UPDATE`
//!    (voice server and token): see [`VoiceServer`];
//! 2. here we connect to the voice server: identify, IP discovery over UDP,
//!    transport encryption mode and key, then the DAVE handshake (MLS group
//!    and per-sender keys). Audio travels as RTP packets: Opus frames
//!    end-to-end encrypted with DAVE, then encrypted again for transport.
//!
//! Audio path:
//! - in: UDP → transport decryption ([`crypto`]) → DAVE decryption
//!   ([`dave`]) → Opus decoding → mixer → speakers;
//! - out (only with the microphone turned on by the user): microphone →
//!   voice detection → Opus → DAVE → transport → UDP.
//!
//! Go Live streams use the same connection towards another voice server
//! ([`VoiceServer::stream`]): it also receives video, which goes RTP →
//! transport decryption → frame put back together ([`crate::video::h264`]) →
//! DAVE → decoder thread ([`crate::video::decoder`]).
//!
//! Our own stream ([`VoiceOptions::golive`]) goes the other way: encoder
//! ([`crate::video::encoder`]) → DAVE → RTP packets ([`crate::video::h264`])
//! → transport → UDP, and the program's sound like a microphone that never
//! mutes. Nothing leaves before end-to-end encryption is ready. Viewers'
//! RTCP feedback asks for keyframes (PLI, FIR) and for lost packets again
//! (NACK, answered on the retransmission SSRC); a sender report every second
//! lets them line up sound and picture.
//!
//! No audio, video, keys or payloads are ever logged.

pub mod crypto;
pub mod dave;
pub mod mock;
pub mod rtp;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{self, Write as _};
use std::sync::Arc;
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
use crate::audio::codec::Encoder;
use crate::audio::device::{Capture, Playback};
use crate::audio::dsp::{self, CaptureConverter, PlaybackConverter, SampleQueue, VoiceActivity};
use crate::audio::jitter::{JitterStats, Mixer};
use crate::audio::process::Processor;
use crate::audio::AudioSettings;
use crate::audio::FRAME_SAMPLES;
use crate::i18n::t;
use crate::video::decoder::{self as video_decoder, VideoDecoder};
use crate::video::encoder::{Outbox, Quality};
use crate::video::h264::{self, Depacketizer};
use crypto::{Cipher, Mode};
use dave::{Dave, Reply};
use rtp::RtpHeader;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// Someone who has not sent audio for this long stopped speaking.
const SPEAKING_HOLD: Duration = Duration::from_millis(300);
const UDP_KEEPALIVE: Duration = Duration::from_secs(5);
/// Reconnection attempts in a row before giving up.
const MAX_ATTEMPTS: u32 = 3;
/// Highest playback volume (200%).
pub const MAX_VOLUME: f32 = 2.0;

/// Audio kept ready in the speakers' queue (see `jitter::DEVICE_AHEAD_MS`).
const PLAYBACK_AHEAD: Duration = Duration::from_millis(crate::audio::jitter::DEVICE_AHEAD_MS);
/// Silence frames sent when we stop talking, as Discord expects.
const TRAILING_SILENCE: usize = 5;
/// Microphone level that counts as speech (RMS) and how long it is held (frames).
const VAD_HANGOVER: u32 = 15;
/// After the push-to-talk key is let go, the end of the word still goes.
const PTT_RELEASE: Duration = Duration::from_millis(200);
/// Rough delay between handing audio to the speakers and its echo reaching
/// the microphone processing: the speakers' queue, plus device latency.
const ECHO_DELAY_MS: i32 = crate::audio::jitter::DEVICE_AHEAD_MS as i32 + 40;
/// Microphone backlog kept between ticks (frames): older audio is dropped
/// rather than sent late.
const MAX_MIC_BACKLOG: usize = 5;
/// Samples accepted through [`VoiceHandle::feed_microphone`] (2 s).
const MAX_FED: usize = 2 * crate::audio::SAMPLE_RATE as usize;
/// Keyframe requests (RTCP PLI) at most this often: each one makes the
/// streamer send a big frame to everyone watching.
const PLI_EVERY: Duration = Duration::from_secs(2);
/// A missing packet is asked again (NACK) at most this many times, this far apart.
const NACK_TRIES: u8 = 3;
const NACK_AGAIN_AFTER: Duration = Duration::from_millis(100);
/// Most packets in one NACK round.
const NACK_MAX: usize = 64;
/// Our video packets kept for retransmission (about a second at 720p).
const SENT_HISTORY: usize = 1024;
/// Packets sent in a row before a short pause, so a keyframe does not
/// leave as one burst that a home router would drop.
const PACING_BURST: usize = 24;
/// Our stream's sound counts as silent after this long below the threshold.
const SOUND_HANGOVER: u32 = 50;
/// Level under which the stream's sound is silence (about -60 dBFS).
const SOUND_THRESHOLD: f32 = 0.001;
/// Most video sources followed on a stream connection.
const MAX_VIDEO_SOURCES: usize = 8;

/// Where and how to connect: from `VOICE_SERVER_UPDATE` and `VOICE_STATE_UPDATE`.
#[derive(Clone)]
pub struct VoiceServer {
    /// Voice server host (e.g. `c-mxp08-1234.discord.media`), or a full
    /// `ws://127.0.0.1:…` address (the local server of the demo).
    pub endpoint: String,
    pub token: String,
    pub guild_id: String,
    pub channel_id: String,
    pub session_id: String,
    pub user_id: String,
    /// Watching a Go Live stream: its `rtc_server_id` (from `STREAM_CREATE`).
    /// The connection then identifies to that server with video, and its
    /// DAVE group is the stream's.
    pub stream: Option<String>,
}

impl fmt::Debug for VoiceServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token and session id are credentials: never printed.
        f.debug_struct("VoiceServer")
            .field("endpoint", &self.endpoint)
            .field("guild_id", &self.guild_id)
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

impl VoiceServer {
    /// WebSocket address of the voice gateway. Only `wss://` towards a
    /// Discord voice server (`*.discord.media`, the only domain Discord uses
    /// for them); unencrypted `ws://` only towards 127.0.0.1 (the demo
    /// server). The voice token goes there.
    fn url(&self) -> Result<String> {
        let e = self.endpoint.trim_end_matches('/');
        let valid_port = |p: &str| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit());
        if let Some(local) = e.strip_prefix("ws://") {
            return match local.strip_prefix("127.0.0.1:") {
                Some(port) if valid_port(port) => Ok(format!("ws://{local}/?v=8")),
                _ => Err(anyhow::Error::new(Fatal(
                    t(
                        "refused an unencrypted voice server address",
                        "indirizzo del server vocale non cifrato: rifiutato",
                    )
                    .into(),
                ))),
            };
        }
        let host = e.strip_prefix("wss://").unwrap_or(e);
        let (name, port) = match host.split_once(':') {
            Some((n, p)) => (n, Some(p)),
            None => (host, None),
        };
        let valid_name = !name.is_empty()
            && name.len() <= 253
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && name.ends_with(".discord.media")
            && !name.starts_with('.')
            && !name.contains("..");
        if !valid_name || port.is_some_and(|p| !valid_port(p)) {
            return Err(anyhow::Error::new(Fatal(
                t(
                    "invalid voice server address",
                    "indirizzo del server vocale non valido",
                )
                .into(),
            )));
        }
        Ok(format!("wss://{host}/?v=8"))
    }

    /// The server's host for the log (never the token): only letters,
    /// digits, `.`, `-` and `:`, at most 80 characters, so a strange address
    /// refused by `url` cannot write anything else in the log.
    fn host_for_log(&self) -> String {
        let e = self.endpoint.trim();
        let e = e
            .strip_prefix("wss://")
            .or_else(|| e.strip_prefix("ws://"))
            .unwrap_or(e);
        let host = e.split(['/', '?', '#', '@']).next().unwrap_or_default();
        let clean: String = host
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
            .take(80)
            .collect();
        if clean.is_empty() {
            "(empty)".into()
        } else if clean.len() < host.len() {
            format!("{clean} (cut)")
        } else {
            clean
        }
    }

    fn ids(&self) -> Result<(u64, u64)> {
        let user = self.user_id.parse().context("invalid user id")?;
        // A stream's DAVE group is identified by its server ID minus one.
        let channel = match &self.stream {
            Some(rtc) => rtc
                .parse::<u64>()
                .ok()
                .and_then(|id| id.checked_sub(1))
                .context("invalid stream server id")?,
            None => self.channel_id.parse().context("invalid channel id")?,
        };
        Ok((user, channel))
    }

    /// `server_id` for identify and resume: the server, or the stream's server.
    fn server_id(&self) -> &str {
        self.stream.as_deref().unwrap_or(&self.guild_id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceStats {
    /// Voice server round trip time (heartbeat).
    pub ping_ms: Option<u32>,
    /// Audio packets received with valid transport encryption.
    pub received: u64,
    pub sent: u64,
    /// Packets dropped because the transport encryption did not verify.
    pub rejected: u64,
    /// Frames end-to-end decrypted and decoded.
    pub decoded: u64,
    /// Frames dropped because they were not valid DAVE frames.
    pub undecryptable: u64,
    /// Video pictures decoded (streams).
    pub video_frames: u64,
    /// Video frames dropped (late, broken, waiting for a keyframe).
    pub video_dropped: u64,
    /// Video packets asked again (NACK) and retransmissions received.
    pub video_nacked: u64,
    pub video_retransmitted: u64,
    /// Our stream: frames sent, packets sent again on request, keyframes
    /// asked for by viewers.
    pub video_sent: u64,
    pub video_resent: u64,
    pub keyframe_requests: u64,
    /// What the jitter buffer did with received audio.
    pub audio: JitterStats,
    /// Times the speakers found their queue short in the middle of sound.
    pub speaker_gaps: u64,
}

/// End-to-end encryption state of the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encryption {
    /// MLS group not ready yet: nothing is sent.
    Pending,
    /// Audio is end-to-end encrypted. The privacy code is the same for
    /// everyone in the call and can be compared out of band.
    Active { privacy_code: String },
}

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// To send commands to the connection: always the first event.
    Handle(VoiceHandle),
    Connecting,
    /// Transport ready (encryption mode agreed).
    Connected {
        mode: &'static str,
    },
    Encryption(Encryption),
    Speaking {
        user_id: String,
        speaking: bool,
    },
    /// The microphone is (or is no longer) captured.
    Microphone(bool),
    /// A device could not be used: the call goes on without it.
    AudioError(String),
    /// The code that verifies one person's end-to-end encryption key, to
    /// compare with what they see (`None`: not in the encrypted call).
    VerificationCode {
        user_id: String,
        code: Option<String>,
    },
    Stats(VoiceStats),
    /// Connection lost: retrying on our own.
    Reconnecting(String),
    /// Final: `None` if we left, otherwise the reason.
    Closed(Option<String>),
}

/// Commands to the voice connection. If every handle is dropped the
/// connection closes.
#[derive(Debug, Clone)]
pub struct VoiceHandle(UnboundedSender<VoiceCommand>);

impl VoiceHandle {
    /// Turns the microphone on (opens the device) or off (releases it).
    pub fn set_microphone(&self, on: bool) {
        let _ = self.0.send(VoiceCommand::Microphone(on));
    }

    /// Deafened: no audio is played and the microphone is turned off.
    pub fn set_deafened(&self, deafened: bool) {
        let _ = self.0.send(VoiceCommand::Deafen(deafened));
    }

    /// Volume of what this connection plays: 1.0 as received, 0.0 silent,
    /// up to [`MAX_VOLUME`] louder.
    pub fn set_volume(&self, volume: f32) {
        let _ = self.0.send(VoiceCommand::Volume(volume));
    }

    /// New audio settings (devices, volumes, sensitivity), applied at once.
    pub fn set_audio(&self, audio: AudioSettings) {
        let _ = self.0.send(VoiceCommand::Audio(Box::new(audio)));
    }

    /// Push-to-talk key held (`true`) or released. Only counts when
    /// push-to-talk is on in the audio settings.
    pub fn push_to_talk(&self, held: bool) {
        let _ = self.0.send(VoiceCommand::PushToTalk(held));
    }

    /// Asks for the code that verifies `user_id`'s end-to-end encryption
    /// key; it comes back as [`VoiceEvent::VerificationCode`].
    pub fn verification_code(&self, user_id: String) {
        let _ = self.0.send(VoiceCommand::VerificationCode(user_id));
    }

    /// Microphone samples from elsewhere (48 kHz mono), used when there is
    /// no audio device, as in tests. Ignored while the microphone is off.
    pub fn feed_microphone(&self, samples: Vec<f32>) {
        let _ = self.0.send(VoiceCommand::Feed(samples));
    }

    pub fn leave(&self) {
        let _ = self.0.send(VoiceCommand::Leave);
    }
}

#[derive(Debug)]
enum VoiceCommand {
    Microphone(bool),
    Deafen(bool),
    Volume(f32),
    Audio(Box<AudioSettings>),
    PushToTalk(bool),
    VerificationCode(String),
    Feed(Vec<f32>),
    Leave,
}

#[derive(Debug, Clone, Default)]
pub struct VoiceOptions {
    /// Use the real microphone and speakers (false in tests).
    pub devices: bool,
    /// Devices, volumes and sensitivity chosen in the settings.
    pub audio: AudioSettings,
    /// Our own Go Live stream (only on a stream connection, `server.stream`).
    pub golive: Option<GoLiveFeed>,
}

/// What our stream sends: encoded frames and, if chosen, the sound.
#[derive(Clone)]
pub struct GoLiveFeed {
    pub outbox: Arc<Outbox>,
    /// 48 kHz interleaved stereo.
    pub sound: Option<SampleQueue>,
    pub quality: Quality,
}

impl fmt::Debug for GoLiveFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoLiveFeed")
            .field("sound", &self.sound.is_some())
            .field("quality", &self.quality)
            .finish_non_exhaustive()
    }
}

/// An error after which retrying is pointless.
#[derive(Debug)]
pub(crate) struct Fatal(pub(crate) String);

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fatal {}

fn emit<E: From<VoiceEvent>>(tx: &UnboundedSender<E>, ev: VoiceEvent) {
    let _ = tx.send(ev.into());
}

/// Link to the voice server. Survives WebSocket reconnections: with a
/// `resume` the same UDP socket, keys and DAVE group are kept.
#[derive(Default)]
struct Link {
    ssrc: u32,
    udp: Option<UdpSocket>,
    cipher: Option<Cipher>,
    dave: Option<Dave>,
    /// External sender received before DAVE started.
    pending_external_sender: Option<Vec<u8>>,
    /// Users in the call, from the gateway (op 11 / op 13).
    users_in_call: HashSet<u64>,
    reported: Option<Encryption>,
    /// Last sequence number received (v8: acknowledged in heartbeats).
    seq_ack: Option<i64>,
    sequence: u16,
    timestamp: u32,
    nonce: u32,
    stats: VoiceStats,
    /// User of each SSRC (from the server's op 5).
    users: HashMap<u32, String>,
    last_heard: HashMap<u32, Instant>,
    speaking: HashMap<u32, bool>,
    /// Video of a Go Live stream (stream connections only).
    video: Option<VideoIn>,
    /// Our own stream's video (when we stream).
    video_out: Option<VideoOut>,
    /// Diagnostics: incoming video headers written to the log so far.
    logged_video_headers: u8,
}

/// Our stream's video: its SSRCs and the packets kept for retransmission.
struct VideoOut {
    outbox: Arc<Outbox>,
    ssrc: u32,
    rtx_ssrc: u32,
    sequence: u16,
    rtx_sequence: u16,
    sent: VecDeque<SentPacket>,
    /// Packets asked again by viewers, waiting to be sent.
    to_resend: VecDeque<u16>,
    /// For the sender reports.
    packets: u32,
    octets: u32,
    last_timestamp: u32,
    /// The stream's sound: above the threshold recently, frames since.
    sound_on: bool,
    quiet_frames: u32,
    /// RTCP from viewers: packets, ones that did not decrypt, and their
    /// types (200–206, for the log).
    rtcp_packets: u64,
    rtcp_rejected: u64,
    rtcp_types: [u64; 7],
    /// Kinds of feedback seen (type, format), and the last receiver report
    /// about our video: highest sequence received, cumulative lost.
    rtcp_kinds: Vec<(u8, u8)>,
    rtcp_report: Option<(u32, i32)>,
}

struct SentPacket {
    sequence: u16,
    timestamp: u32,
    marker: bool,
    payload: Vec<u8>,
}

impl VideoOut {
    fn new(outbox: Arc<Outbox>, ssrc: u32, rtx_ssrc: u32) -> Self {
        Self {
            outbox,
            ssrc,
            rtx_ssrc,
            sequence: rand::random(),
            rtx_sequence: rand::random(),
            sent: VecDeque::new(),
            to_resend: VecDeque::new(),
            packets: 0,
            octets: 0,
            last_timestamp: 0,
            sound_on: false,
            quiet_frames: 0,
            rtcp_packets: 0,
            rtcp_rejected: 0,
            rtcp_types: [0; 7],
            rtcp_kinds: Vec::new(),
            rtcp_report: None,
        }
    }
}

/// Video received on a stream connection.
struct VideoIn {
    decoder: VideoDecoder,
    /// Video SSRC → (user, frame being put back together), from op 12.
    sources: HashMap<u32, (String, Depacketizer)>,
    /// Retransmission SSRC → video SSRC (RFC 4588).
    rtx: HashMap<u32, u32>,
    /// Packets asked again: (video SSRC, sequence) → (when, how many times).
    nacked: HashMap<(u32, u16), (Instant, u8)>,
    last_pli: Option<Instant>,
}

impl VideoIn {
    fn new(decoder: VideoDecoder) -> Self {
        Self {
            decoder,
            sources: HashMap::new(),
            rtx: HashMap::new(),
            nacked: HashMap::new(),
            last_pli: None,
        }
    }
}

impl Link {
    fn ready(&self) -> bool {
        self.cipher.is_some() && self.udp.is_some()
    }

    fn can_send_audio(&self) -> bool {
        self.ready() && self.dave.as_ref().is_some_and(Dave::can_encrypt)
    }

    /// Sends one RTP packet with an (already end-to-end encrypted) payload.
    async fn send_payload(&mut self, payload: &[u8]) -> Result<()> {
        let (Some(cipher), Some(udp)) = (&self.cipher, &self.udp) else {
            return Ok(());
        };
        let header = RtpHeader {
            sequence: self.sequence,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
        };
        let packet = cipher.encrypt_rtp(&header, payload, self.nonce)?;
        udp.send(&packet).await?;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(rtp::SAMPLES_PER_FRAME);
        self.nonce = self.nonce.wrapping_add(1);
        self.stats.sent += 1;
        Ok(())
    }

    /// One of our video packets: header and extension in clear, then the
    /// extension's element and the payload encrypted.
    async fn send_video_packet(&mut self, header: rtp::VideoHeader, payload: &[u8]) -> Result<()> {
        let (Some(cipher), Some(udp)) = (&self.cipher, &self.udp) else {
            return Ok(());
        };
        let mut body = Vec::with_capacity(rtp::PLAYOUT_DELAY.len() + payload.len());
        body.extend_from_slice(&rtp::PLAYOUT_DELAY);
        body.extend_from_slice(payload);
        let packet = cipher.encrypt_rtcp(&header.clear_bytes(), &body, self.nonce)?;
        udp.send(&packet).await?;
        self.nonce = self.nonce.wrapping_add(1);
        Ok(())
    }

    /// Sends the frames our encoder made: end-to-end encrypted, then in
    /// packets. Before DAVE is ready nothing leaves (a keyframe is asked for
    /// when it is).
    async fn send_video(&mut self) -> Result<()> {
        let Some(out) = self.video_out.as_ref() else {
            return Ok(());
        };
        let frames = out.outbox.take();
        for frame in frames {
            let ready = self.ready() && self.dave.as_ref().is_some_and(Dave::can_encrypt);
            let encrypted = match self.dave.as_mut().filter(|_| ready) {
                Some(dave) => dave.encrypt_video(&frame.data).ok(),
                None => None,
            };
            let Some(encrypted) = encrypted else {
                if let Some(out) = &self.video_out {
                    out.outbox.request_keyframe();
                }
                continue;
            };
            let payloads = h264::packetize(&encrypted, h264::MAX_PAYLOAD);
            let last = payloads.len().saturating_sub(1);
            for (i, payload) in payloads.into_iter().enumerate() {
                let Some(out) = self.video_out.as_mut() else {
                    return Ok(());
                };
                let header = rtp::VideoHeader {
                    payload_type: rtp::H264_PAYLOAD_TYPE,
                    marker: i == last,
                    sequence: out.sequence,
                    timestamp: frame.timestamp,
                    ssrc: out.ssrc,
                };
                out.sequence = out.sequence.wrapping_add(1);
                out.packets = out.packets.wrapping_add(1);
                out.octets = out.octets.wrapping_add(payload.len() as u32);
                out.last_timestamp = frame.timestamp;
                self.send_video_packet(header, &payload).await?;
                if let Some(out) = self.video_out.as_mut() {
                    if out.sent.len() >= SENT_HISTORY {
                        out.sent.pop_front();
                    }
                    out.sent.push_back(SentPacket {
                        sequence: header.sequence,
                        timestamp: header.timestamp,
                        marker: header.marker,
                        payload,
                    });
                }
                if i % PACING_BURST == PACING_BURST - 1 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            self.stats.video_sent += 1;
        }
        Ok(())
    }

    /// Sends again, on the retransmission SSRC (RFC 4588), the packets
    /// viewers said they lost and that we still have.
    async fn resend_lost(&mut self) -> Result<()> {
        loop {
            let Some(out) = self.video_out.as_mut() else {
                return Ok(());
            };
            let Some(seq) = out.to_resend.pop_front() else {
                return Ok(());
            };
            let Some(p) = out.sent.iter().find(|p| p.sequence == seq) else {
                continue;
            };
            let mut payload = Vec::with_capacity(p.payload.len() + 2);
            payload.extend_from_slice(&seq.to_be_bytes());
            payload.extend_from_slice(&p.payload);
            let header = rtp::VideoHeader {
                payload_type: rtp::H264_RTX_PAYLOAD_TYPE,
                marker: p.marker,
                sequence: out.rtx_sequence,
                timestamp: p.timestamp,
                ssrc: out.rtx_ssrc,
            };
            out.rtx_sequence = out.rtx_sequence.wrapping_add(1);
            self.send_video_packet(header, &payload).await?;
            self.stats.video_resent += 1;
        }
    }

    /// Viewers' RTCP feedback about our video.
    fn on_rtcp(&mut self, packet: &[u8]) {
        let (Some(cipher), Some(out)) = (&self.cipher, self.video_out.as_mut()) else {
            return;
        };
        out.rtcp_packets += 1;
        if let Some(n) = packet
            .get(1)
            .and_then(|t| out.rtcp_types.get_mut(t.wrapping_sub(200) as usize))
        {
            *n += 1;
        }
        let Ok(plain) = cipher.decrypt_rtcp(packet) else {
            out.rtcp_rejected += 1;
            self.stats.rejected += 1;
            return;
        };
        // Diagnostics: kinds seen, and what receiver reports say of our video.
        let mut rest = plain.as_slice();
        while rest.len() >= 8 && rest[0] >> 6 == 2 {
            let len = 4 * (u16::from_be_bytes([rest[2], rest[3]]) as usize + 1);
            let Some(part) = rest.get(..len) else { break };
            let kind = (part[1], part[0] & 0x1F);
            if out.rtcp_kinds.len() < 16 && !out.rtcp_kinds.contains(&kind) {
                out.rtcp_kinds.push(kind);
            }
            if part[1] == 201 {
                for block in part.get(8..).unwrap_or_default().as_chunks::<24>().0 {
                    let ssrc = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
                    if ssrc == out.ssrc {
                        let lost = i32::from_be_bytes([0, block[5], block[6], block[7]]);
                        let highest = u32::from_be_bytes([block[8], block[9], block[10], block[11]]);
                        out.rtcp_report = Some((highest, lost));
                    }
                }
            }
            rest = &rest[len..];
        }
        for fb in rtp::parse_feedback(&plain) {
            match fb {
                rtp::Feedback::Keyframe { media_ssrc } if media_ssrc == out.ssrc || media_ssrc == 0 => {
                    out.outbox.request_keyframe();
                    self.stats.keyframe_requests += 1;
                }
                rtp::Feedback::Nack { media_ssrc, lost } if media_ssrc == out.ssrc => {
                    for seq in lost {
                        if out.to_resend.len() < NACK_MAX * 4 && !out.to_resend.contains(&seq) {
                            out.to_resend.push_back(seq);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Sender reports for our stream's video and sound.
    async fn sender_reports(&mut self) {
        let (Some(cipher), Some(udp), Some(out)) = (&self.cipher, &self.udp, &self.video_out) else {
            return;
        };
        let ntp = rtp::ntp_now();
        let reports = [
            rtp::sender_report(out.ssrc, ntp, out.last_timestamp, out.packets, out.octets),
            rtp::sender_report(self.ssrc, ntp, self.timestamp, self.stats.sent as u32, 0),
        ];
        for (clear, body) in reports {
            if let Ok(packet) = cipher.encrypt_rtcp(&clear, &body, self.nonce) {
                let _ = udp.send(&packet).await;
                self.nonce = self.nonce.wrapping_add(1);
            }
        }
    }

    fn on_udp(&mut self, packet: &[u8], media: &mut Media) {
        if self.video_out.is_some() && rtp::is_rtcp(packet) {
            self.on_rtcp(packet);
            return;
        }
        let Some(cipher) = &self.cipher else { return };
        if !rtp::is_media(packet) {
            return;
        }
        let Ok((parsed, payload)) = cipher.decrypt_packet(packet) else {
            self.stats.rejected += 1;
            return;
        };
        self.stats.received += 1;
        if parsed.payload_type != rtp::OPUS_PAYLOAD_TYPE {
            // Diagnostics: how a real client writes its video packets (clear
            // header and extension elements, no content), first few only.
            if self.stats.video_frames == 0 && self.logged_video_headers < 3 {
                self.logged_video_headers += 1;
                let clear = hex(&packet[..parsed.clear_len]);
                let ext = hex(&cipher.extension_of(packet).unwrap_or_default());
                debug!(
                    "video in: pt {} marker {} header {clear} extension {ext} payload {} bytes, packet {} bytes",
                    parsed.payload_type,
                    parsed.marker,
                    payload.len(),
                    packet.len()
                );
            }
            self.on_video(&parsed, &payload);
            return;
        }
        let header = parsed.header;
        if payload == rtp::OPUS_SILENCE {
            // Not end-to-end encrypted: it marks the end of a sentence.
            media.on_frame(header.ssrc, header.sequence, &payload);
            return;
        }
        let Some(user_id) = self.users.get(&header.ssrc).and_then(|u| u.parse::<u64>().ok()) else {
            return;
        };
        let Some(dave) = self.dave.as_mut() else { return };
        match dave.decrypt(user_id, &payload) {
            Ok(opus) if opus == rtp::OPUS_SILENCE => {
                media.on_frame(header.ssrc, header.sequence, &opus);
            }
            Ok(opus) => {
                self.last_heard.insert(header.ssrc, Instant::now());
                if media.on_frame(header.ssrc, header.sequence, &opus) {
                    self.stats.decoded += 1;
                }
            }
            Err(_) => self.stats.undecryptable += 1,
        }
    }

    /// A video packet: when its frame is complete, DAVE then the decoder.
    /// Packets of unknown SSRCs (retransmissions included) are ignored.
    fn on_video(&mut self, p: &rtp::Parsed, payload: &[u8]) {
        let Self {
            video, dave, stats, ..
        } = self;
        let Some(video) = video.as_mut() else { return };
        // A retransmission carries the original sequence number first.
        let (ssrc, seq, payload) = if video.sources.contains_key(&p.header.ssrc) {
            (p.header.ssrc, p.header.sequence, payload)
        } else if let Some(&media) = video.rtx.get(&p.header.ssrc) {
            let Some((seq, original)) = rtp::rtx_original(payload) else {
                return; // padding
            };
            stats.video_retransmitted += 1;
            (media, seq, original)
        } else {
            return;
        };
        let Some((user, depack)) = video.sources.get_mut(&ssrc) else {
            return;
        };
        let out = depack.push(seq, p.header.timestamp, p.marker, payload);
        if out.lost {
            video.decoder.frames_lost();
        }
        let (Ok(user_id), Some(dave)) = (user.parse::<u64>(), dave.as_mut()) else {
            return;
        };
        for frame in out.frames {
            if let Ok(plain) = dave.decrypt_video(user_id, &frame) {
                video.decoder.push(plain);
            } else {
                stats.undecryptable += 1;
                video.decoder.frames_lost();
            }
        }
    }

    /// Asks again for the packets still missing (RTCP NACK); the sender's
    /// retransmissions arrive on the RTX SSRC.
    async fn request_missing(&mut self) {
        let Some(video) = self.video.as_mut() else { return };
        let (Some(cipher), Some(udp)) = (&self.cipher, &self.udp) else {
            return;
        };
        let now = Instant::now();
        video
            .nacked
            .retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(2));
        for (&ssrc, (_, depack)) in &video.sources {
            let mut ask = Vec::new();
            for seq in depack.missing(NACK_MAX) {
                let entry = video.nacked.entry((ssrc, seq)).or_insert((now, 0));
                let due = entry.1 == 0 || now.duration_since(entry.0) >= NACK_AGAIN_AFTER;
                if due && entry.1 < NACK_TRIES {
                    *entry = (now, entry.1 + 1);
                    ask.push(seq);
                }
            }
            if ask.is_empty() {
                continue;
            }
            let (clear, body) = rtp::nack(self.ssrc, ssrc, &ask);
            if let Ok(packet) = cipher.encrypt_rtcp(&clear, &body, self.nonce) {
                let _ = udp.send(&packet).await;
                self.nonce = self.nonce.wrapping_add(1);
                self.stats.video_nacked += ask.len() as u64;
            }
        }
    }

    /// Asks the senders for a keyframe while the decoder waits for one.
    async fn request_keyframe(&mut self) {
        let Some(video) = self.video.as_mut() else { return };
        if !video.decoder.needs_keyframe() || video.sources.is_empty() {
            return;
        }
        if video.last_pli.is_some_and(|at| at.elapsed() < PLI_EVERY) {
            return;
        }
        video.last_pli = Some(Instant::now());
        let (Some(cipher), Some(udp)) = (&self.cipher, &self.udp) else {
            return;
        };
        for ssrc in video.sources.keys() {
            let (clear, body) = rtp::picture_loss(self.ssrc, *ssrc);
            if let Ok(packet) = cipher.encrypt_rtcp(&clear, &body, self.nonce) {
                let _ = udp.send(&packet).await;
                self.nonce = self.nonce.wrapping_add(1);
            }
        }
    }

    /// The server's op 12: whose video (and audio) is on which SSRC.
    fn on_video_info(&mut self, info: VideoInfo) {
        let Some(user) = info.user_id else { return };
        if info.audio_ssrc != 0 {
            self.users.insert(info.audio_ssrc, user.clone());
        }
        let Some(video) = self.video.as_mut() else { return };
        let mut ssrcs: Vec<u32> = info.streams.iter().map(|s| s.ssrc).collect();
        ssrcs.push(info.video_ssrc);
        ssrcs.retain(|s| *s != 0);
        if ssrcs.is_empty() {
            // Video stopped.
            video.sources.retain(|_, (u, _)| *u != user);
            let sources = &video.sources;
            video.rtx.retain(|_, media| sources.contains_key(media));
            return;
        }
        for ssrc in ssrcs {
            if video.sources.len() >= MAX_VIDEO_SOURCES && !video.sources.contains_key(&ssrc) {
                break;
            }
            video
                .sources
                .entry(ssrc)
                .or_insert_with(|| (user.clone(), Depacketizer::default()));
        }
        for s in &info.streams {
            if s.rtx_ssrc != 0 && video.sources.contains_key(&s.ssrc) {
                video.rtx.insert(s.rtx_ssrc, s.ssrc);
            }
        }
        debug!("voice: {} video sources", video.sources.len());
    }

    /// Who started or stopped speaking, from the audio received.
    fn speaking_changes(&mut self) -> Vec<(String, bool)> {
        let now = Instant::now();
        let mut changes = Vec::new();
        for (ssrc, heard) in &self.last_heard {
            let speaking = now.duration_since(*heard) < SPEAKING_HOLD;
            let Some(user) = self.users.get(ssrc) else {
                continue;
            };
            if self.speaking.get(ssrc).copied().unwrap_or(false) != speaking {
                self.speaking.insert(*ssrc, speaking);
                changes.push((user.clone(), speaking));
            }
        }
        changes
    }

    fn forget_user(&mut self, user_id: &str, media: &mut Media) -> bool {
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
            media.forget(s);
            was_speaking |= self.speaking.remove(&s).unwrap_or(false);
        }
        if let Some(video) = self.video.as_mut() {
            video.sources.retain(|_, (u, _)| u.as_str() != user_id);
            let sources = &video.sources;
            video.rtx.retain(|_, media| sources.contains_key(media));
        }
        was_speaking
    }

    /// Tells the interface when the end-to-end encryption state changes.
    fn report_encryption<E: From<VoiceEvent>>(&mut self, tx: &UnboundedSender<E>) {
        let state = match &self.dave {
            Some(d) if d.can_encrypt() => Encryption::Active {
                privacy_code: d.privacy_code().unwrap_or_default(),
            },
            _ => Encryption::Pending,
        };
        if self.reported.as_ref() != Some(&state) {
            if matches!(state, Encryption::Active { .. }) {
                info!("voice: end-to-end encryption active");
                // Our stream starts (again) with a keyframe under the new keys.
                if let Some(out) = &self.video_out {
                    out.outbox.request_keyframe();
                }
            }
            self.reported = Some(state.clone());
            emit(tx, VoiceEvent::Encryption(state));
        }
    }
}

/// Audio devices and codec state. Lives for the whole connection; dropping
/// it releases microphone and speakers.
struct Media {
    options: VoiceOptions,
    playback: Option<(Playback, PlaybackConverter)>,
    capture: Option<(Capture, CaptureConverter)>,
    mic_on: bool,
    deafened: bool,
    /// Gain applied to everything played (see [`VoiceHandle::set_volume`]).
    volume: f32,
    /// Samples given through `feed_microphone` (48 kHz mono).
    fed: VecDeque<f32>,
    /// Microphone audio converted to 48 kHz mono, waiting for a full frame.
    mic: Vec<f32>,
    encoder: Option<Encoder>,
    mixer: Mixer,
    /// Who speaks on each SSRC, for their own volume.
    ssrc_users: HashMap<u32, String>,
    vad: VoiceActivity,
    /// Echo cancellation, noise suppression and gain, while the microphone
    /// is on and any of them is chosen.
    processor: Option<Processor>,
    /// Push-to-talk key held now.
    ptt_held: bool,
    /// When the push-to-talk key was let go (the end of a word still goes).
    ptt_released: Option<Instant>,
    transmitting: bool,
    out: Vec<f32>,
}

impl Media {
    fn new(options: VoiceOptions) -> Self {
        let vad = VoiceActivity::new(options.audio.vad_threshold(), VAD_HANGOVER);
        Self {
            options,
            playback: None,
            capture: None,
            mic_on: false,
            deafened: false,
            volume: 1.0,
            ssrc_users: HashMap::new(),
            fed: VecDeque::new(),
            mic: Vec::new(),
            encoder: None,
            mixer: Mixer::default(),
            vad,
            processor: None,
            ptt_held: false,
            ptt_released: None,
            transmitting: false,
            out: Vec::new(),
        }
    }

    fn start_speakers<E: From<VoiceEvent>>(&mut self, tx: &UnboundedSender<E>) {
        if !self.options.devices || self.deafened || self.playback.is_some() {
            return;
        }
        match Playback::start(self.options.audio.output_device.as_deref()) {
            Ok(p) => {
                let conv = PlaybackConverter::new(p.format.rate, p.format.channels);
                self.playback = Some((p, conv));
            }
            Err(e) => emit(
                tx,
                VoiceEvent::AudioError(tf!(
                    "Speakers unavailable: {e:#}",
                    "Altoparlanti non disponibili: {e:#}"
                )),
            ),
        }
    }

    fn set_microphone<E: From<VoiceEvent>>(&mut self, on: bool, tx: &UnboundedSender<E>) {
        if on == self.mic_on {
            return;
        }
        if !on {
            // Dropping the capture stops the stream and releases the device.
            self.capture = None;
            self.processor = None;
            self.mic_on = false;
            self.mic.clear();
            self.fed.clear();
            emit(tx, VoiceEvent::Microphone(false));
            return;
        }
        if self.deafened {
            self.set_deafened(false, tx);
        }
        if self.options.devices {
            match Capture::start(self.options.audio.input_device.as_deref()) {
                Ok(c) => {
                    let conv = CaptureConverter::new(c.format.rate, c.format.channels);
                    self.capture = Some((c, conv));
                }
                Err(e) => {
                    emit(
                        tx,
                        VoiceEvent::AudioError(tf!(
                            "Microphone unavailable: {e:#}",
                            "Microfono non disponibile: {e:#}"
                        )),
                    );
                    emit(tx, VoiceEvent::Microphone(false));
                    return;
                }
            }
        }
        self.mic_on = true;
        self.start_processing();
        emit(tx, VoiceEvent::Microphone(true));
    }

    /// (Re)starts the microphone clean-up with the chosen parts.
    fn start_processing(&mut self) {
        let choices = self.options.audio.processing();
        self.processor = (self.mic_on && choices.any()).then(|| Processor::new(choices, ECHO_DELAY_MS));
    }

    /// The push-to-talk key held or let go.
    fn set_push_to_talk(&mut self, held: bool) {
        if self.ptt_held && !held {
            self.ptt_released = Some(Instant::now());
        }
        self.ptt_held = held;
    }

    /// Whether what the microphone hears now is sent: with push-to-talk, while
    /// the key is held (and a moment after); otherwise when there is voice.
    fn sends(&mut self, frame: &[f32]) -> bool {
        if self.options.audio.push_to_talk {
            // The level still follows, so voice detection is ready if
            // push-to-talk is turned off mid-call.
            let _ = self.vad.update(frame);
            self.ptt_held || self.ptt_released.is_some_and(|t| t.elapsed() < PTT_RELEASE)
        } else {
            self.vad.update(frame)
        }
    }

    fn set_deafened<E: From<VoiceEvent>>(&mut self, deafened: bool, tx: &UnboundedSender<E>) {
        if deafened == self.deafened {
            return;
        }
        self.deafened = deafened;
        if deafened {
            self.playback = None;
            self.mixer.clear();
            self.set_microphone(false, tx);
        } else {
            self.start_speakers(tx);
        }
    }

    fn feed(&mut self, samples: Vec<f32>) {
        if !self.mic_on {
            return;
        }
        self.fed.extend(samples);
        let excess = self.fed.len().saturating_sub(MAX_FED);
        self.fed.drain(..excess);
    }

    /// Queues a received Opus frame (silence frames included) in the
    /// speaker's jitter buffer; it is decoded when its turn comes.
    fn on_frame(&mut self, ssrc: u32, seq: u16, opus: &[u8]) -> bool {
        if self.deafened {
            return false;
        }
        self.mixer.push(ssrc, seq, opus)
    }

    fn forget(&mut self, ssrc: u32) {
        self.mixer.remove(ssrc);
        self.ssrc_users.remove(&ssrc);
    }

    /// `user_id` speaks on `ssrc`: their own volume applies to it.
    fn on_user(&mut self, ssrc: u32, user_id: &str) {
        self.mixer.set_gain(ssrc, self.options.audio.user_gain(user_id));
        if self.ssrc_users.len() < 4096 || self.ssrc_users.contains_key(&ssrc) {
            self.ssrc_users.insert(ssrc, user_id.to_string());
        }
    }

    /// Feeds the speakers. Audio is mixed when the speakers' queue runs low,
    /// so the sound card's clock sets the pace, not our timer: a late tick
    /// does not leave a gap and a fast one does not pile up delay.
    fn play(&mut self) {
        let Some((playback, conv)) = &mut self.playback else {
            return;
        };
        let format = playback.format;
        let ahead =
            (u128::from(format.rate) * format.channels as u128 * PLAYBACK_AHEAD.as_millis() / 1000) as usize;
        // At most 120 ms per call, whatever the device reports.
        for _ in 0..6 {
            if playback.queue.len() >= ahead {
                break;
            }
            let Some(mut mix) = self.mixer.mix() else { break };
            let gain = self.volume * self.options.audio.output_volume;
            if gain != 1.0 {
                for s in &mut mix {
                    *s = (*s * gain).clamp(-1.0, 1.0);
                }
            }
            // What is played now is the echo the microphone may hear.
            if let Some(p) = self.processor.as_mut() {
                p.render(&mix);
            }
            self.out.clear();
            conv.process(&mix, &mut self.out);
            playback.queue.push(&self.out);
        }
    }

    /// Every 20 ms without speakers (tests, no device, sound off): received
    /// audio still goes through the jitter buffer, then is dropped.
    fn mix_without_speakers(&mut self) {
        if self.playback.is_none() {
            let _ = self.mixer.mix();
        }
    }

    fn speaker_gaps(&self) -> u64 {
        self.playback.as_ref().map_or(0, |(p, _)| p.queue.starved())
    }

    /// Every 20 ms: microphone frames ready to send (48 kHz mono).
    fn mic_frames(&mut self) -> Vec<Vec<f32>> {
        if !self.mic_on {
            return Vec::new();
        }
        if let Some((capture, conv)) = &mut self.capture {
            conv.process(&capture.queue.drain(), &mut self.mic);
            let max = MAX_MIC_BACKLOG * FRAME_SAMPLES;
            let excess = self.mic.len().saturating_sub(max);
            self.mic.drain(..excess);
        }
        // Fed samples behave like a microphone: one frame per tick.
        if self.fed.len() >= FRAME_SAMPLES {
            self.mic.extend(self.fed.drain(..FRAME_SAMPLES));
        }
        let gain = self.options.audio.input_volume;
        let mut frames = Vec::new();
        while self.mic.len() >= FRAME_SAMPLES {
            let mut frame: Vec<f32> = self.mic.drain(..FRAME_SAMPLES).collect();
            if gain != 1.0 {
                for s in &mut frame {
                    *s = (*s * gain).clamp(-1.0, 1.0);
                }
            }
            if let Some(p) = self.processor.as_mut() {
                p.capture(&mut frame);
            }
            frames.push(frame);
        }
        frames
    }

    /// New settings: volumes and sensitivity at once; a different device is
    /// reopened if it was in use (the microphone only if it was on).
    fn set_audio<E: From<VoiceEvent>>(&mut self, audio: AudioSettings, tx: &UnboundedSender<E>) {
        let audio = audio.sanitized();
        let old = std::mem::replace(&mut self.options.audio, audio);
        self.vad.set_threshold(self.options.audio.vad_threshold());
        if old.processing() != self.options.audio.processing() {
            self.start_processing();
        }
        if !self.options.audio.push_to_talk {
            self.ptt_held = false;
            self.ptt_released = None;
        }
        for (ssrc, user) in &self.ssrc_users {
            self.mixer.set_gain(*ssrc, self.options.audio.user_gain(user));
        }
        if old.output_device != self.options.audio.output_device && self.playback.is_some() {
            self.playback = None;
            self.start_speakers(tx);
        }
        if old.input_device != self.options.audio.input_device && self.capture.is_some() {
            self.set_microphone(false, tx);
            self.set_microphone(true, tx);
        }
    }
}

/// Voice connection with automatic reconnection. Ends when we leave (`leave`
/// or every handle dropped), on a fatal error, or when `tx` is closed.
pub async fn run<E: From<VoiceEvent> + Send + 'static>(
    server: VoiceServer,
    options: VoiceOptions,
    tx: UnboundedSender<E>,
) {
    run_with_video(server, options, None, tx).await;
}

/// Like [`run`]; for a stream (`server.stream`), the decoded video pictures
/// go to `video`.
pub async fn run_with_video<E: From<VoiceEvent> + Send + 'static>(
    server: VoiceServer,
    options: VoiceOptions,
    video: Option<video_decoder::Sink>,
    tx: UnboundedSender<E>,
) {
    let (cmd_tx, mut cmds) = mpsc::unbounded_channel();
    emit(&tx, VoiceEvent::Handle(VoiceHandle(cmd_tx)));
    let mut link = Link::default();
    if let (Some(_), Some(sink)) = (&server.stream, video) {
        match VideoDecoder::start(sink) {
            Ok(d) => link.video = Some(VideoIn::new(d)),
            Err(e) => emit(
                &tx,
                VoiceEvent::AudioError(tf!("Video unavailable: {e:#}", "Video non disponibile: {e:#}")),
            ),
        }
    }
    let mut media = Media::new(options);
    let mut attempts = 0;
    loop {
        emit(&tx, VoiceEvent::Connecting);
        let was_ready = link.ready();
        match session(&server, &mut link, &mut media, &mut cmds, &tx).await {
            Ok(()) => {
                info!("voice: left the channel");
                emit(&tx, VoiceEvent::Closed(None));
                return;
            }
            Err(e) => {
                if let Some(Fatal(msg)) = e.downcast_ref::<Fatal>() {
                    warn!("voice: {msg}");
                    emit(&tx, VoiceEvent::Closed(Some(msg.clone())));
                    return;
                }
                // A connection that got as far as audio resets the counter.
                if was_ready || link.ready() {
                    attempts = 0;
                }
                attempts += 1;
                warn!("voice: {e:#} (attempt {attempts})");
                if attempts >= MAX_ATTEMPTS || tx.is_closed() {
                    emit(&tx, VoiceEvent::Closed(Some(format!("{e:#}"))));
                    return;
                }
                emit(&tx, VoiceEvent::Reconnecting(format!("{e:#}")));
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempts))).await;
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
    /// The SSRCs given to our video (when we said we send some).
    #[serde(default)]
    streams: Vec<VideoStreamInfo>,
}

#[derive(Deserialize)]
struct SessionDescription {
    mode: String,
    secret_key: Vec<u8>,
    /// Missing means no DAVE: treated as version 0 and refused.
    #[serde(default)]
    dave_protocol_version: u16,
}

#[derive(Deserialize)]
struct Speaking {
    #[serde(default)]
    user_id: Option<String>,
    ssrc: u32,
}

#[derive(Deserialize)]
struct ClientsConnect {
    #[serde(default)]
    user_ids: Vec<String>,
}

#[derive(Deserialize)]
struct ClientDisconnect {
    user_id: String,
}

/// Op 12 from the server: someone's audio and video SSRCs.
#[derive(Deserialize)]
struct VideoInfo {
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    audio_ssrc: u32,
    #[serde(default)]
    video_ssrc: u32,
    #[serde(default)]
    streams: Vec<VideoStreamInfo>,
}

#[derive(Deserialize)]
struct VideoStreamInfo {
    #[serde(default)]
    ssrc: u32,
    #[serde(default)]
    rtx_ssrc: u32,
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

async fn send_replies(sink: &mut WsSink, replies: Vec<Reply>) -> Result<()> {
    for reply in replies {
        match reply {
            Reply::Json(v) => send_json(sink, &v).await?,
            Reply::Binary(op, payload) => {
                let mut message = Vec::with_capacity(payload.len() + 1);
                message.push(op);
                message.extend_from_slice(&payload);
                sink.send(WsMessage::Binary(message.into())).await?;
            }
        }
    }
    Ok(())
}

async fn session<E: From<VoiceEvent>>(
    server: &VoiceServer,
    link: &mut Link,
    media: &mut Media,
    cmds: &mut UnboundedReceiver<VoiceCommand>,
    tx: &UnboundedSender<E>,
) -> Result<()> {
    let url = server
        .url()
        .inspect_err(|_| warn!("voice: refused server {}", server.host_for_log()))?;
    info!("voice: server {}", server.host_for_log());
    let (user_id, channel_id) = server.ids()?;
    debug!("voice: connecting to the voice server");
    let (ws, _) = timeout(CONNECT_TIMEOUT, connect_async(url.as_str()))
        .await
        .context("timed out connecting to the voice server")??;
    let (mut sink, mut stream) = ws.split();

    let mut heartbeat: Option<Interval> = None;
    let mut pending_beat: Option<(u64, Instant)> = None;
    let mut missed_beats = 0u32;
    let mut audio_tick = interval(Duration::from_millis(20));
    let mut play_tick = interval(Duration::from_millis(10));
    play_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut every_100ms = interval(Duration::from_millis(100));
    let mut every_second = interval(Duration::from_secs(1));
    let mut keepalive = interval(UDP_KEEPALIVE);
    audio_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    for iv in [&mut every_100ms, &mut every_second, &mut keepalive] {
        iv.set_missed_tick_behavior(MissedTickBehavior::Skip);
    }
    let mut keepalive_count: u32 = 0;
    let mut seconds: u64 = 0;
    let mut buf = vec![0u8; 2048];

    loop {
        tokio::select! {
            () = tick_opt(&mut heartbeat) => {
                if pending_beat.is_some() {
                    missed_beats += 1;
                    if missed_beats >= 3 {
                        bail!("the voice server stopped answering heartbeats");
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
                let msg = msg.ok_or_else(|| anyhow!("the voice server closed the connection"))??;
                let text = match msg {
                    WsMessage::Text(t) => t,
                    WsMessage::Binary(b) => {
                        let (seq, opcode, payload) = dave::parse_server_binary(&b)?;
                        link.seq_ack = Some(seq.into());
                        if opcode == dave::op::EXTERNAL_SENDER && link.dave.is_none() {
                            link.pending_external_sender = Some(payload.to_vec());
                        } else if let Some(d) = link.dave.as_mut() {
                            let replies = d.on_binary(opcode, payload)?;
                            send_replies(&mut sink, replies).await?;
                            link.report_encryption(tx);
                        }
                        continue;
                    }
                    WsMessage::Close(frame) => return Err(close_error(frame.as_ref())),
                    _ => continue,
                };
                let v: Value = serde_json::from_str(&text).context("invalid voice message")?;
                if let Some(seq) = v["seq"].as_i64() {
                    link.seq_ack = Some(seq);
                }
                let d = &v["d"];
                match v["op"].as_u64().unwrap_or(u64::MAX) {
                    // Hello: start heartbeats and identify (or resume)
                    8 => {
                        let hello: Hello = parse(d)?;
                        let period = Duration::from_millis((hello.heartbeat_interval as u64).max(100));
                        let mut iv = interval_at(Instant::now() + period, period);
                        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
                        heartbeat = Some(iv);
                        if link.ready() {
                            send_json(&mut sink, &json!({"op": 7, "d": {
                                "server_id": server.server_id(),
                                "session_id": server.session_id,
                                "token": server.token,
                                "seq_ack": link.seq_ack,
                            }})).await?;
                        } else {
                            let mut identify = json!({
                                "server_id": server.server_id(),
                                "user_id": server.user_id,
                                "session_id": server.session_id,
                                "token": server.token,
                                "max_dave_protocol_version": dave::MAX_VERSION,
                            });
                            if media.options.golive.is_some() {
                                // Our stream: the screen, one quality.
                                identify["video"] = json!(true);
                                identify["streams"] = json!([{"type": "screen", "rid": "100", "quality": 100}]);
                            } else if server.stream.is_some() {
                                // Receiving a stream: video on, one quality.
                                identify["video"] = json!(true);
                                identify["streams"] = json!([{"type": "video", "rid": "100", "quality": 100}]);
                            }
                            send_json(&mut sink, &json!({"op": 0, "d": identify})).await?;
                        }
                    }
                    // Ready: SSRC and UDP server; IP discovery and transport encryption
                    2 => {
                        let ready: Ready = parse(d)?;
                        let mode = Mode::choose(ready.modes.iter().map(String::as_str))
                            .ok_or_else(|| Fatal(
                                t(
                                    "no audio transport encryption in common with the server",
                                    "nessuna cifratura dell'audio in comune con il server",
                                )
                                .into(),
                            ))?;
                        let (udp, address, port) = discover(&ready).await?;
                        link.ssrc = ready.ssrc;
                        link.udp = Some(udp);
                        link.cipher = None;
                        if let Some(feed) = &media.options.golive {
                            // The server's SSRCs for our video, or the usual ones after ours.
                            let (video, rtx) = ready
                                .streams
                                .iter()
                                .find(|s| s.ssrc != 0)
                                .map_or_else(
                                    || (ready.ssrc.wrapping_add(1), ready.ssrc.wrapping_add(2)),
                                    |s| (s.ssrc, s.rtx_ssrc),
                                );
                            let rtx = if rtx == 0 { video.wrapping_add(1) } else { rtx };
                            link.video_out = Some(VideoOut::new(feed.outbox.clone(), video, rtx));
                        }
                        let mut select = json!({
                            "protocol": "udp",
                            "data": {"address": address, "port": port, "mode": mode.name()},
                        });
                        if server.stream.is_some() {
                            // Only H.264: watching, we only decode (the server
                            // makes the streamer send something we can
                            // decode); streaming, we also encode.
                            let encode = media.options.golive.is_some();
                            select["codecs"] = json!([
                                {"name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS_PAYLOAD_TYPE},
                                {"name": "H264", "type": "video", "priority": 1000,
                                 "payload_type": rtp::H264_PAYLOAD_TYPE, "rtx_payload_type": rtp::H264_RTX_PAYLOAD_TYPE,
                                 "encode": encode, "decode": true},
                            ]);
                        }
                        send_json(&mut sink, &json!({"op": 1, "d": select})).await?;
                    }
                    // Session Description: transport key and DAVE version
                    4 => {
                        let sd: SessionDescription = parse(d)?;
                        let mode = Mode::from_name(&sd.mode)
                            .ok_or_else(|| Fatal(tf!("unknown transport encryption: {}", "cifratura del trasporto sconosciuta: {}", sd.mode)))?;
                        if sd.dave_protocol_version == 0 {
                            bail!(Fatal(
                                t(
                                    "the voice server did not enable end-to-end encryption (DAVE): call refused",
                                    "il server vocale non ha attivato la crittografia end-to-end (DAVE): chiamata rifiutata",
                                )
                                .into()
                            ));
                        }
                        link.cipher = Some(Cipher::new(mode, &sd.secret_key)?);
                        let (mut d, replies) = Dave::new(sd.dave_protocol_version, user_id, channel_id)?;
                        d.users_joined(link.users_in_call.iter().copied());
                        if let Some(ext) = link.pending_external_sender.take() {
                            d.on_binary(dave::op::EXTERNAL_SENDER, &ext)?;
                        }
                        link.dave = Some(d);
                        link.reported = None;
                        send_replies(&mut sink, replies).await?;
                        info!("voice: connected ({}), DAVE v{}", mode.name(), sd.dave_protocol_version);
                        if let (Some(feed), Some(out)) = (&media.options.golive, &link.video_out) {
                            // Our video: its SSRCs and limits, then our sound's SSRC.
                            let (width, height) = feed.quality.max_size();
                            send_json(&mut sink, &json!({"op": 12, "d": {
                                "audio_ssrc": link.ssrc, "video_ssrc": out.ssrc, "rtx_ssrc": out.rtx_ssrc,
                                "streams": [{
                                    "type": "video", "rid": "100", "ssrc": out.ssrc, "rtx_ssrc": out.rtx_ssrc,
                                    "active": true, "quality": 100,
                                    "max_bitrate": feed.quality.bitrate(), "max_framerate": feed.quality.fps(),
                                    "max_resolution": {"type": "fixed", "width": width, "height": height},
                                }],
                            }})).await?;
                            send_json(
                                &mut sink,
                                // 2 = "soundshare": the stream's sound, not a microphone.
                                &json!({"op": 5, "d": {"speaking": 2, "delay": 0, "ssrc": link.ssrc}}),
                            )
                            .await?;
                            out.outbox.request_keyframe();
                        } else if server.stream.is_some() {
                            // We send no video; then "send everything at the best quality".
                            send_json(&mut sink, &json!({"op": 12, "d": {
                                "audio_ssrc": link.ssrc, "video_ssrc": 0, "rtx_ssrc": 0, "streams": [],
                            }})).await?;
                            send_json(&mut sink, &json!({"op": 15, "d": {"any": 100}})).await?;
                        }
                        emit(tx, VoiceEvent::Connected { mode: mode.name() });
                        link.report_encryption(tx);
                        media.start_speakers(tx);
                    }
                    // Heartbeat ACK: the ping
                    6 => {
                        if let Some((nonce, at)) = pending_beat {
                            if d["t"].as_u64() == Some(nonce) {
                                link.stats.ping_ms = Some(at.elapsed().as_millis() as u32);
                                pending_beat = None;
                                missed_beats = 0;
                            }
                        }
                    }
                    // Resumed: same keys as before
                    9 => {
                        if let Some(cipher) = &link.cipher {
                            emit(tx, VoiceEvent::Connected { mode: cipher.mode().name() });
                        }
                    }
                    // Speaking: who owns an SSRC
                    5 => {
                        let s: Speaking = parse(d)?;
                        if let Some(user) = s.user_id {
                            media.on_user(s.ssrc, &user);
                            link.users.insert(s.ssrc, user);
                        }
                    }
                    // Someone's audio and video SSRCs (streams)
                    12 => {
                        let info: VideoInfo = parse(d)?;
                        if let (Some(user), true) = (&info.user_id, info.audio_ssrc != 0) {
                            media.on_user(info.audio_ssrc, user);
                        }
                        link.on_video_info(info);
                    }
                    // Users already in the call or joining it
                    11 => {
                        let c: ClientsConnect = parse(d)?;
                        let ids: Vec<u64> = c.user_ids.iter().filter_map(|u| u.parse().ok()).collect();
                        link.users_in_call.extend(&ids);
                        if let Some(dave) = link.dave.as_mut() {
                            dave.users_joined(ids);
                        }
                    }
                    // Someone left the call
                    13 => {
                        let c: ClientDisconnect = parse(d)?;
                        if let Ok(id) = c.user_id.parse::<u64>() {
                            link.users_in_call.remove(&id);
                            if let Some(dave) = link.dave.as_mut() {
                                dave.user_left(id);
                            }
                        }
                        if link.forget_user(&c.user_id, media) {
                            emit(tx, VoiceEvent::Speaking { user_id: c.user_id, speaking: false });
                        }
                    }
                    // DAVE transitions and epochs
                    op @ (21 | 22 | 24) => {
                        if let Some(dave) = link.dave.as_mut() {
                            let replies = dave.on_json(op as u8, d)?;
                            send_replies(&mut sink, replies).await?;
                            link.report_encryption(tx);
                        }
                    }
                    // Our stream: what else the server says (numbers only:
                    // op 15 is the quality viewers want per SSRC).
                    op if link.video_out.is_some() => {
                        if op == 15 {
                            debug!("golive: server op 15 (media sink wants) {d}");
                        } else {
                            debug!("golive: server op {op}");
                        }
                    }
                    _ => {}
                }
            }
            r = recv_udp(link.udp.as_ref(), &mut buf) => {
                let n = r.context("UDP receive")?;
                link.on_udp(&buf[..n], media);
                link.resend_lost().await?;
            }
            () = outbox_ready(media.options.golive.as_ref()) => link.send_video().await?,
            cmd = cmds.recv() => match cmd {
                Some(VoiceCommand::Microphone(on)) => media.set_microphone(on, tx),
                Some(VoiceCommand::Deafen(on)) => media.set_deafened(on, tx),
                Some(VoiceCommand::Audio(audio)) => media.set_audio(*audio, tx),
                Some(VoiceCommand::Volume(v)) => media.volume = if v.is_finite() { v.clamp(0.0, MAX_VOLUME) } else { 1.0 },
                Some(VoiceCommand::Feed(samples)) => media.feed(samples),
                Some(VoiceCommand::PushToTalk(held)) => media.set_push_to_talk(held),
                Some(VoiceCommand::VerificationCode(user_id)) => {
                    let code = user_id
                        .parse::<u64>()
                        .ok()
                        .filter(|id| link.users_in_call.contains(id))
                        .and_then(|id| link.dave.as_ref().filter(|d| d.can_encrypt())?.verification_code(id));
                    emit(tx, VoiceEvent::VerificationCode { user_id, code });
                }
                // Leaving, or nobody holds a handle any more
                Some(VoiceCommand::Leave) | None => {
                    close(&mut sink).await;
                    return Ok(());
                }
            },
            _ = play_tick.tick() => media.play(),
            _ = audio_tick.tick() => {
                media.mix_without_speakers();
                transmit(link, media, &mut sink, tx, &server.user_id).await?;
                transmit_stream_sound(link, media).await?;
            }
            _ = every_100ms.tick() => {
                for (user_id, speaking) in link.speaking_changes() {
                    emit(tx, VoiceEvent::Speaking { user_id, speaking });
                }
                link.request_missing().await;
                link.request_keyframe().await;
            }
            _ = every_second.tick() => {
                seconds += 1;
                link.sender_reports().await;
                if let Some(out) = &link.video_out {
                    if seconds.is_multiple_of(5) && link.stats.video_sent > 0 {
                        let (copy, convert, encode) = out.outbox.timings_ms();
                        debug!(
                            "golive: RTCP from viewers: {} packets ({} not decrypted), by type 200-206: {:?}, kinds (type, format) {:?}; \
                             video SSRC {}, RTX {}, our last sequence {}, receiver report (highest sequence, lost) {:?}",
                            out.rtcp_packets, out.rtcp_rejected, out.rtcp_types, out.rtcp_kinds, out.ssrc, out.rtx_ssrc,
                            out.sequence.wrapping_sub(1), out.rtcp_report
                        );
                        debug!(
                            "golive: {} frames sent ({} encoded, {} dropped), {} packets sent again, {} keyframes asked; \
                             per picture: copy {copy:.1} ms, convert {convert:.1} ms, encode {encode:.1} ms",
                            link.stats.video_sent,
                            out.outbox.encoded.load(std::sync::atomic::Ordering::Relaxed),
                            out.outbox.dropped.load(std::sync::atomic::Ordering::Relaxed),
                            link.stats.video_resent,
                            link.stats.keyframe_requests
                        );
                    }
                }
                link.stats.audio = media.mixer.stats;
                link.stats.speaker_gaps = media.speaker_gaps();
                if seconds.is_multiple_of(5) && link.stats.decoded > 0 {
                    let (a, s) = (&link.stats.audio, &link.stats);
                    debug!(
                        "audio: {} frames, {} rebuilt (FEC), {} concealed, {} late, {} skipped, {} ran dry, {} speaker gaps",
                        s.decoded, a.recovered, a.concealed, a.late, a.skipped, a.underruns, s.speaker_gaps
                    );
                }
                if let Some(video) = &link.video {
                    let shared = &video.decoder.shared;
                    link.stats.video_frames = shared.decoded.load(std::sync::atomic::Ordering::Relaxed);
                    link.stats.video_dropped = shared.dropped.load(std::sync::atomic::Ordering::Relaxed);
                    if seconds.is_multiple_of(5) {
                        let s = &link.stats;
                        debug!(
                            "video: {} decoded, {} dropped ({} decode errors), {} packets asked again, {} retransmitted",
                            s.video_frames,
                            s.video_dropped,
                            shared.errors.load(std::sync::atomic::Ordering::Relaxed),
                            s.video_nacked,
                            s.video_retransmitted
                        );
                    }
                }
                if link.ready() {
                    emit(tx, VoiceEvent::Stats(link.stats));
                }
            }
            _ = keepalive.tick() => {
                // Keeps the NAT mapping open even when nobody talks.
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

/// Sends our microphone audio, if on, while we speak. Nothing leaves before
/// end-to-end encryption is ready.
async fn transmit<E: From<VoiceEvent>>(
    link: &mut Link,
    media: &mut Media,
    sink: &mut WsSink,
    tx: &UnboundedSender<E>,
    me: &str,
) -> Result<()> {
    let frames = media.mic_frames();
    if let Some(why) = media.processor.as_mut().and_then(Processor::take_failure) {
        emit(
            tx,
            VoiceEvent::AudioError(crate::tf!(
                "The chosen noise suppression could not start ({why}): Standard is on instead.",
                "La soppressione del rumore scelta non è partita ({why}): è attiva la Standard.",
            )),
        );
    }
    let mut speaking = false;
    for mono in &frames {
        speaking = media.sends(mono);
        if !speaking || !link.can_send_audio() {
            continue;
        }
        if !media.transmitting {
            media.transmitting = true;
            send_json(
                sink,
                &json!({"op": 5, "d": {"speaking": 1, "delay": 0, "ssrc": link.ssrc}}),
            )
            .await?;
            emit(
                tx,
                VoiceEvent::Speaking {
                    user_id: me.to_string(),
                    speaking: true,
                },
            );
        }
        if media.encoder.is_none() {
            media.encoder = Some(Encoder::new()?);
        }
        let Some(encoder) = media.encoder.as_mut() else {
            continue;
        };
        let opus = encoder.encode(&dsp::mono_to_stereo(mono))?;
        let Some(dave) = link.dave.as_mut() else { continue };
        // During a key change a frame may not be encryptable: it is dropped,
        // never sent in clear.
        let Ok(e2ee) = dave.encrypt(&opus) else { continue };
        link.send_payload(&e2ee).await?;
    }
    // Stopped talking (or microphone off): a few silence frames, then not speaking.
    let stopped = !media.mic_on || (!frames.is_empty() && !speaking);
    if media.transmitting && stopped {
        media.transmitting = false;
        for _ in 0..TRAILING_SILENCE {
            link.send_payload(&rtp::OPUS_SILENCE).await?;
        }
        send_json(
            sink,
            &json!({"op": 5, "d": {"speaking": 0, "delay": 0, "ssrc": link.ssrc}}),
        )
        .await?;
        emit(
            tx,
            VoiceEvent::Speaking {
                user_id: me.to_string(),
                speaking: false,
            },
        );
    }
    Ok(())
}

/// Our stream's sound, 20 ms at a time: Opus, DAVE, then like our voice
/// (same SSRC). While it is silent only a few silence frames are sent.
async fn transmit_stream_sound(link: &mut Link, media: &mut Media) -> Result<()> {
    let Some(queue) = media.options.golive.as_ref().and_then(|f| f.sound.clone()) else {
        return Ok(());
    };
    let mut pcm = vec![0f32; FRAME_SAMPLES * 2];
    // At most a few frames per tick: a backlog is caught up, never let grow.
    for _ in 0..3 {
        if queue.len() < pcm.len() {
            break;
        }
        queue.pop_into(&mut pcm);
        let Some(out) = link.video_out.as_mut() else {
            return Ok(());
        };
        if dsp::rms(&pcm) >= SOUND_THRESHOLD {
            out.sound_on = true;
            out.quiet_frames = 0;
        } else if out.sound_on {
            out.quiet_frames += 1;
            if out.quiet_frames > SOUND_HANGOVER {
                out.sound_on = false;
                for _ in 0..TRAILING_SILENCE {
                    link.send_payload(&rtp::OPUS_SILENCE).await?;
                }
            }
        }
        let sound_on = link.video_out.as_ref().is_some_and(|o| o.sound_on);
        if !sound_on || !link.can_send_audio() {
            continue;
        }
        if media.encoder.is_none() {
            media.encoder = Some(Encoder::new()?);
        }
        let Some(encoder) = media.encoder.as_mut() else {
            continue;
        };
        let opus = encoder.encode(&pcm)?;
        let Some(dave) = link.dave.as_mut() else { continue };
        let Ok(e2ee) = dave.encrypt(&opus) else { continue };
        link.send_payload(&e2ee).await?;
    }
    // Far behind (the connection stalled): drop the old sound.
    if queue.len() > pcm.len() * 10 {
        let _ = queue.drain();
    }
    Ok(())
}

/// Waits for our encoder's next frame; never ready without a stream.
async fn outbox_ready(feed: Option<&GoLiveFeed>) {
    match feed {
        Some(f) => f.outbox.notify.notified().await,
        None => std::future::pending().await,
    }
}

async fn close(sink: &mut WsSink) {
    let _ = sink.send(WsMessage::Close(None)).await;
}

/// UDP socket towards the voice server and our public address as seen by it.
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
    .context("no answer to IP discovery (UDP blocked?)")??;
    Ok((udp, found.0, found.1))
}

/// Bytes as lowercase hex, for diagnostics of headers (never payloads).
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn close_error(frame: Option<&CloseFrame>) -> anyhow::Error {
    let code = frame.map(|f| u16::from(f.code));
    let fatal = |s: &str| anyhow::Error::new(Fatal(s.to_string()));
    match code {
        Some(4004) => fatal(t(
            "the voice server rejected the authentication",
            "il server vocale ha rifiutato l'autenticazione",
        )),
        Some(4006) => fatal(t(
            "the voice session is no longer valid",
            "la sessione vocale non è più valida",
        )),
        Some(4011) => fatal(t("voice server not found", "server vocale non trovato")),
        Some(4014) => fatal(t(
            "disconnected from the voice channel",
            "disconnesso dal canale vocale",
        )),
        Some(4016) => fatal(t(
            "the server did not accept the audio encryption",
            "il server non ha accettato la cifratura dell'audio",
        )),
        Some(4017) => fatal(t(
            "the voice server requires a DAVE version Rico does not support",
            "il server vocale richiede una versione di DAVE che Rico non supporta",
        )),
        Some(4020) => fatal(t(
            "the voice server rejected a request",
            "il server vocale ha rifiutato una richiesta",
        )),
        Some(4021) => fatal(t(
            "rate limited by the voice server",
            "troppe richieste al server vocale",
        )),
        Some(4022) => fatal(t("the call has ended", "la chiamata è terminata")),
        Some(4015) => anyhow!("the voice server restarted"),
        Some(c) => anyhow!("voice connection closed (code {c})"),
        None => anyhow!("voice connection closed"),
    }
}

#[cfg(test)]
mod tests {
    use super::mock::{self, MockConfig};
    use super::*;
    use std::sync::atomic::Ordering;
    use tokio::task::JoinHandle;

    fn server(endpoint: &str) -> VoiceServer {
        VoiceServer {
            endpoint: endpoint.to_string(),
            token: "fake-token".into(),
            guild_id: "100".into(),
            channel_id: mock::CHANNEL_ID.to_string(),
            session_id: "session".into(),
            user_id: "900".into(),
            stream: None,
        }
    }

    type Started = (JoinHandle<()>, UnboundedReceiver<VoiceEvent>, VoiceHandle);

    /// Starts the connection and keeps the handle: dropping it would count as
    /// leaving the channel.
    async fn start(endpoint: &str) -> Started {
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let task = tokio::spawn(run(server(endpoint), VoiceOptions::default(), tx));
        let Some(VoiceEvent::Handle(handle)) = rx.recv().await else {
            panic!("the first event must be the handle")
        };
        (task, rx, handle)
    }

    /// Waits for the first event matching `f`, within a few seconds.
    async fn wait_for(rx: &mut UnboundedReceiver<VoiceEvent>, f: impl Fn(&VoiceEvent) -> bool) -> VoiceEvent {
        timeout(Duration::from_secs(15), async {
            loop {
                let ev = rx.recv().await.expect("channel closed");
                if f(&ev) {
                    return ev;
                }
            }
        })
        .await
        .expect("event did not arrive")
    }

    fn tone(seconds: f32) -> Vec<f32> {
        let n = (crate::audio::SAMPLE_RATE as f32 * seconds) as usize;
        (0..n)
            .map(|i| {
                0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / crate::audio::SAMPLE_RATE as f32).sin()
            })
            .collect()
    }

    #[tokio::test]
    async fn push_to_talk_sends_only_while_the_key_is_held() {
        let loud = vec![0.3_f32; FRAME_SAMPLES];
        let quiet = vec![0.0_f32; FRAME_SAMPLES];
        let mut media = Media::new(VoiceOptions::default());
        // Voice activity: loud frames go, silence does not (after the hangover).
        assert!(media.sends(&loud));
        let mut audio = AudioSettings {
            push_to_talk: true,
            push_to_talk_key: Some("F13".into()),
            ..AudioSettings::default()
        };
        let (tx, _rx) = mpsc::unbounded_channel::<VoiceEvent>();
        media.set_audio(audio.clone(), &tx);
        assert!(!media.sends(&loud), "loud but the key is not held");
        media.set_push_to_talk(true);
        assert!(media.sends(&quiet), "the key is held: even silence goes");
        media.set_push_to_talk(false);
        assert!(media.sends(&quiet), "the end of the word still goes");
        tokio::time::sleep(PTT_RELEASE + Duration::from_millis(50)).await;
        assert!(!media.sends(&loud));
        // Turned off mid-call: back to voice activity, the held key forgotten.
        media.set_push_to_talk(true);
        audio.push_to_talk = false;
        media.set_audio(audio, &tx);
        assert!(media.sends(&loud));
        assert!(!media.ptt_held);
    }

    #[test]
    fn microphone_processing_follows_the_settings() {
        let (tx, _rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let mut media = Media::new(VoiceOptions::default());
        // Nothing runs while the microphone is off.
        media.start_processing();
        assert!(media.processor.is_none());
        media.set_microphone(true, &tx);
        assert_eq!(
            media.processor.as_ref().map(Processor::choices),
            Some(AudioSettings::default().processing())
        );
        let off = AudioSettings {
            noise_suppression: crate::audio::process::Noise::Off,
            echo_cancellation: false,
            auto_gain: false,
            ..AudioSettings::default()
        };
        media.set_audio(off, &tx);
        assert!(media.processor.is_none());
        media.set_microphone(false, &tx);
        assert!(media.processor.is_none());
    }

    #[test]
    fn voice_server_address() {
        assert_eq!(
            server("c-mxp08.discord.media").url().unwrap(),
            "wss://c-mxp08.discord.media/?v=8"
        );
        assert_eq!(
            server("c-mxp08.discord.media:443").url().unwrap(),
            "wss://c-mxp08.discord.media:443/?v=8"
        );
        assert_eq!(
            server("ws://127.0.0.1:5000/").url().unwrap(),
            "ws://127.0.0.1:5000/?v=8"
        );
        for bad in [
            "ws://example.com:80",
            "ws://127.0.0.1:80/x",
            "evil.com/path?x",
            "",
            "a b",
            "host:99999x",
            "c-mxp08.discord.media:99999x",
            "example.com",
            "discord.media.example.com",
            "evil.com:443",
            ".discord.media",
            "a..discord.media",
            "us-east1.discord.gg",
            "wss://evil.com/.discord.media",
        ] {
            let err = server(bad).url().unwrap_err();
            assert!(err.downcast_ref::<Fatal>().is_some(), "{bad}");
        }
        // The token never shows up in debug output.
        assert!(!format!("{:?}", server("x")).contains("fake-token"));
        // The host for the log: only the host, nothing else of the address.
        assert_eq!(
            server("c-mxp08.discord.media:443").host_for_log(),
            "c-mxp08.discord.media:443"
        );
        assert_eq!(
            server("wss://c-mxp08.discord.media/").host_for_log(),
            "c-mxp08.discord.media"
        );
        assert_eq!(server("evil.com/path?token=x").host_for_log(), "evil.com");
        assert_eq!(server("a b\nfake").host_for_log(), "abfake (cut)");
        assert_eq!(server("").host_for_log(), "(empty)");
        assert_eq!(
            server(&"a".repeat(300)).host_for_log(),
            format!("{} (cut)", "a".repeat(80))
        );
    }

    #[tokio::test]
    async fn end_to_end_encrypted_call_with_the_local_server() {
        let mock = mock::start(MockConfig {
            users: vec!["901".into(), "902".into()],
            heartbeat_ms: 100,
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, handle) = start(&mock.endpoint).await;

        let connected = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Connected { .. })).await;
        assert!(matches!(
            connected,
            VoiceEvent::Connected {
                mode: "aead_aes256_gcm_rtpsize"
            }
        ));
        let VoiceEvent::Encryption(Encryption::Active { privacy_code }) = wait_for(&mut rx, |e| {
            matches!(e, VoiceEvent::Encryption(Encryption::Active { .. }))
        })
        .await
        else {
            unreachable!()
        };
        // 30 digits in groups of 5.
        assert_eq!(
            privacy_code.chars().filter(char::is_ascii_digit).count(),
            30,
            "{privacy_code}"
        );

        // The others' audio arrives, is end-to-end decrypted and decoded.
        wait_for(
            &mut rx,
            |e| matches!(e, VoiceEvent::Speaking { user_id, speaking: true } if user_id == "901"),
        )
        .await;
        let VoiceEvent::Stats(stats) = wait_for(
            &mut rx,
            |e| matches!(e, VoiceEvent::Stats(s) if s.ping_ms.is_some() && s.decoded > 0),
        )
        .await
        else {
            unreachable!()
        };
        assert_eq!((stats.rejected, stats.undecryptable), (0, 0));

        // Nothing is sent while the microphone is off.
        handle.feed_microphone(tone(0.5));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(mock.received.load(Ordering::Relaxed), 0);

        // Microphone on: our audio reaches the others, end-to-end encrypted.
        handle.set_microphone(true);
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Microphone(true))).await;
        handle.feed_microphone(tone(1.0));
        wait_for(
            &mut rx,
            |e| matches!(e, VoiceEvent::Speaking { user_id, speaking: true } if user_id == "900"),
        )
        .await;
        timeout(Duration::from_secs(10), async {
            while mock.received.load(Ordering::Relaxed) < 10 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("our audio did not reach the server");

        // Microphone off: transmission stops.
        handle.set_microphone(false);
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Microphone(false))).await;
        handle.leave();
        let closed = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(closed, VoiceEvent::Closed(None)));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn our_stream_reaches_the_viewers_end_to_end_encrypted() {
        use crate::capture::{self, Options, Source};
        let mock = mock::start(MockConfig {
            users: vec!["901".into(), "902".into()],
            heartbeat_ms: 100,
            ask_keyframe_after: Some(12),
            ..Default::default()
        })
        .await
        .unwrap();
        let running = capture::start(
            &Source::test_pattern(),
            Options {
                quality: Quality::Sd30,
                sound: true,
                preview: None,
            },
        )
        .unwrap();
        let feed = GoLiveFeed {
            outbox: running.outbox(),
            sound: running.sound.clone(),
            quality: Quality::Sd30,
        };
        // A stream connection: its DAVE group is the stream server's ID minus one.
        let mut srv = server(&mock.endpoint);
        srv.stream = Some((mock::CHANNEL_ID + 1).to_string());
        let (tx, mut rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let options = VoiceOptions {
            golive: Some(feed),
            ..Default::default()
        };
        let task = tokio::spawn(run(srv, options, tx));
        let Some(VoiceEvent::Handle(handle)) = rx.recv().await else {
            panic!("the first event must be the handle")
        };
        wait_for(&mut rx, |e| {
            matches!(e, VoiceEvent::Encryption(Encryption::Active { .. }))
        })
        .await;
        // The frames reach a participant, who decrypts them; after its PLI a
        // keyframe comes; the stream's sound arrives too.
        timeout(Duration::from_secs(20), async {
            while mock.video.frames.load(Ordering::Relaxed) < 30
                || mock.video.keyframes.load(Ordering::Relaxed) < 1
                || mock.received.load(Ordering::Relaxed) < 10
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the stream did not reach the server");
        let VoiceEvent::Stats(stats) = wait_for(
            &mut rx,
            |e| matches!(e, VoiceEvent::Stats(s) if s.video_sent > 0 && s.keyframe_requests >= 1),
        )
        .await
        else {
            unreachable!()
        };
        assert_eq!(stats.rejected, 0);
        // What arrived decodes into pictures, from the first keyframe on.
        let frames: Vec<Vec<u8>> = mock.video.last_frames.lock().unwrap().iter().cloned().collect();
        let first_key = frames
            .iter()
            .position(|f| crate::video::h264::is_keyframe(f))
            .expect("a keyframe among the last frames");
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let pictures = frames[first_key..]
            .iter()
            .filter(|f| matches!(decoder.decode(f), Ok(Some(_))))
            .count();
        assert!(pictures >= 5, "{pictures} pictures");
        handle.leave();
        let closed = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(closed, VoiceEvent::Closed(None)));
        task.await.unwrap();
        drop(running);
    }

    #[tokio::test]
    async fn we_commit_when_others_join_after_us() {
        let mock = mock::start(MockConfig {
            users: vec!["901".into()],
            join_after_client: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        wait_for(&mut rx, |e| {
            matches!(e, VoiceEvent::Encryption(Encryption::Active { .. }))
        })
        .await;
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Stats(s) if s.decoded > 0)).await;
        task.abort();
    }

    #[tokio::test]
    async fn frames_without_end_to_end_encryption_are_dropped() {
        let mock = mock::start(MockConfig {
            users: vec!["901".into()],
            skip_e2ee: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        let VoiceEvent::Stats(stats) = wait_for(
            &mut rx,
            |e| matches!(e, VoiceEvent::Stats(s) if s.undecryptable >= 10),
        )
        .await
        else {
            unreachable!()
        };
        // Valid transport encryption is not enough: nothing is played.
        assert_eq!(stats.decoded, 0);
        assert!(stats.received >= 10);
        task.abort();
    }

    #[tokio::test]
    async fn a_call_without_end_to_end_encryption_is_refused() {
        let mock = mock::start(MockConfig {
            dave: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        let VoiceEvent::Closed(Some(reason)) =
            wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await
        else {
            panic!("closed without a reason")
        };
        assert!(reason.contains("end-to-end"), "{reason}");
        timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn without_handles_we_leave_the_channel() {
        let mock = mock::start(MockConfig::default()).await.unwrap();
        let (task, mut rx, handle) = start(&mock.endpoint).await;
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Connected { .. })).await;
        drop(handle);
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(ev, VoiceEvent::Closed(None)));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn second_cipher_when_the_server_only_offers_that() {
        let mock = mock::start(MockConfig {
            modes: vec![Mode::XChaCha20Poly1305.name()],
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Connected { .. })).await;
        assert!(matches!(
            ev,
            VoiceEvent::Connected {
                mode: "aead_xchacha20_poly1305_rtpsize"
            }
        ));
        task.abort();
    }

    #[tokio::test]
    async fn dave_required_close_code_is_final() {
        let mock = mock::start(MockConfig {
            close_after_identify: Some(4017),
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(ev, VoiceEvent::Closed(Some(_))));
        // No new attempts: the task is over.
        timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_crashed_server_is_retried() {
        let mock = mock::start(MockConfig {
            close_after_identify: Some(4015),
            ..Default::default()
        })
        .await
        .unwrap();
        let (task, mut rx, _handle) = start(&mock.endpoint).await;
        wait_for(&mut rx, |e| matches!(e, VoiceEvent::Reconnecting(_))).await;
        // After the allowed attempts it gives up with the reason.
        let ev = wait_for(&mut rx, |e| matches!(e, VoiceEvent::Closed(_))).await;
        assert!(matches!(ev, VoiceEvent::Closed(Some(_))));
        task.await.unwrap();
    }
}

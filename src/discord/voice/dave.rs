//! DAVE: end-to-end encryption of voice frames (protocol v1).
//!
//! The MLS group and the frame encryption are handled by `davey`; this module
//! maps the voice gateway messages (opcodes 21–31) to davey calls and returns
//! the replies to send. Policy, stricter than "transport encryption only":
//!
//! - a session without DAVE, or a downgrade to protocol version 0, is fatal:
//!   the call is ended instead of continuing without end-to-end encryption;
//! - passthrough stays off, so frames that cannot be decrypted are dropped;
//! - our own audio is only sent once the group key is in place
//!   ([`Dave::can_encrypt`]).
//!
//! Message formats: <https://daveprotocol.com> and Discord's voice docs.

use std::borrow::Cow;
use std::collections::HashSet;
use std::num::NonZeroU16;

use anyhow::{anyhow, bail, Context, Result};
use davey::{Codec, DaveSession, MediaType, ProposalsOperationType};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, warn};

use super::Fatal;

/// Highest DAVE protocol version we support.
pub const MAX_VERSION: u16 = davey::DAVE_PROTOCOL_VERSION;

/// Largest DAVE binary message accepted from the server (MLS messages for a
/// full call are a few tens of KB).
pub const MAX_MESSAGE: usize = 1 << 20;

/// A reply for the voice gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Json(Value),
    /// Binary message: opcode and payload.
    Binary(u8, Vec<u8>),
}

pub mod op {
    pub const PREPARE_TRANSITION: u8 = 21;
    pub const EXECUTE_TRANSITION: u8 = 22;
    pub const READY_FOR_TRANSITION: u8 = 23;
    pub const PREPARE_EPOCH: u8 = 24;
    pub const EXTERNAL_SENDER: u8 = 25;
    pub const KEY_PACKAGE: u8 = 26;
    pub const PROPOSALS: u8 = 27;
    pub const COMMIT_WELCOME: u8 = 28;
    pub const ANNOUNCE_COMMIT: u8 = 29;
    pub const WELCOME: u8 = 30;
    pub const INVALID_COMMIT_WELCOME: u8 = 31;
}

fn fatal(msg: &str) -> anyhow::Error {
    anyhow::Error::new(Fatal(msg.to_string()))
}

fn version(v: u16) -> Result<NonZeroU16> {
    if v == 0 {
        return Err(fatal(crate::i18n::t(
            "the voice server asked to drop end-to-end encryption: call ended",
            "il server vocale ha chiesto di togliere la crittografia end-to-end: chiamata chiusa",
        )));
    }
    if v > MAX_VERSION {
        return Err(fatal(&tf!(
            "unsupported DAVE protocol version {v}",
            "versione del protocollo DAVE non supportata: {v}"
        )));
    }
    NonZeroU16::new(v).ok_or_else(|| anyhow!("invalid DAVE version"))
}

pub struct Dave {
    session: DaveSession,
    version: NonZeroU16,
    user_id: u64,
    channel_id: u64,
    /// Users in the call (from the gateway), us included: MLS members must
    /// be among them.
    recognized: HashSet<u64>,
}

#[derive(Deserialize)]
struct Transition {
    transition_id: u16,
    #[serde(default)]
    protocol_version: Option<u16>,
}

#[derive(Deserialize)]
struct Epoch {
    protocol_version: u16,
    epoch: u64,
}

impl Dave {
    /// Starts DAVE with the version chosen by the server (Session
    /// Description). Returns the key package to send.
    pub fn new(protocol_version: u16, user_id: u64, channel_id: u64) -> Result<(Self, Vec<Reply>)> {
        let version = version(protocol_version)?;
        let session = DaveSession::new(version, user_id, channel_id, None)
            .map_err(|e| anyhow!("starting DAVE: {e:?}"))?;
        let mut dave = Self {
            session,
            version,
            user_id,
            channel_id,
            recognized: HashSet::from([user_id]),
        };
        let replies = vec![dave.key_package()?];
        Ok((dave, replies))
    }

    fn key_package(&mut self) -> Result<Reply> {
        let kp = self
            .session
            .create_key_package()
            .map_err(|e| anyhow!("creating the MLS key package: {e:?}"))?;
        Ok(Reply::Binary(op::KEY_PACKAGE, kp))
    }

    /// After an unusable commit or welcome: tell the server, reset the group
    /// and ask to be added again with a fresh key package.
    fn recover(&mut self, transition_id: u16) -> Result<Vec<Reply>> {
        let invalid =
            Reply::Json(json!({"op": op::INVALID_COMMIT_WELCOME, "d": {"transition_id": transition_id}}));
        self.session
            .reinit(self.version, self.user_id, self.channel_id, None)
            .map_err(|e| anyhow!("resetting DAVE: {e:?}"))?;
        Ok(vec![invalid, self.key_package()?])
    }

    pub fn users_joined(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.recognized.extend(ids);
    }

    pub fn user_left(&mut self, id: u64) {
        if id != self.user_id {
            self.recognized.remove(&id);
        }
    }

    fn recognized(&self) -> Vec<u64> {
        self.recognized.iter().copied().collect()
    }

    /// Prepares the transition after a commit or welcome; transition 0 takes
    /// effect at once, the others after the server's "execute".
    fn after_group_change(&mut self, transition_id: u16) -> Result<Vec<Reply>> {
        self.session
            .prepare_transition(transition_id, self.version.get())
            .map_err(|e| anyhow!("preparing the DAVE transition: {e:?}"))?;
        Ok(if transition_id == 0 {
            Vec::new()
        } else {
            vec![Reply::Json(
                json!({"op": op::READY_FOR_TRANSITION, "d": {"transition_id": transition_id}}),
            )]
        })
    }

    /// JSON DAVE messages (21, 22, 24).
    pub fn on_json(&mut self, opcode: u8, d: &Value) -> Result<Vec<Reply>> {
        match opcode {
            op::PREPARE_TRANSITION => {
                let t = Transition::deserialize(d).context("prepare transition")?;
                // Version 0 = going back to transport-only encryption: refused.
                let v = version(t.protocol_version.unwrap_or(0))?;
                self.session
                    .prepare_transition(t.transition_id, v.get())
                    .map_err(|e| anyhow!("preparing the DAVE transition: {e:?}"))?;
                // Transition 0 is a (re)initialization, executed at once
                // (e.g. the sole member reset): nothing to answer.
                if t.transition_id == 0 {
                    return Ok(Vec::new());
                }
                Ok(vec![Reply::Json(
                    json!({"op": op::READY_FOR_TRANSITION, "d": {"transition_id": t.transition_id}}),
                )])
            }
            op::EXECUTE_TRANSITION => {
                let t = Transition::deserialize(d).context("execute transition")?;
                let done = self
                    .session
                    .execute_transition(t.transition_id)
                    .map_err(|e| anyhow!("executing the DAVE transition: {e:?}"))?;
                debug!("DAVE: transition {} executed: {done}", t.transition_id);
                Ok(Vec::new())
            }
            op::PREPARE_EPOCH => {
                let e = Epoch::deserialize(d).context("prepare epoch")?;
                let v = version(e.protocol_version)?;
                if e.epoch == 1 {
                    // A new group is being created: start over and send a new key package.
                    self.version = v;
                    self.session
                        .reinit(v, self.user_id, self.channel_id, None)
                        .map_err(|e| anyhow!("resetting DAVE: {e:?}"))?;
                    return Ok(vec![self.key_package()?]);
                }
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Binary DAVE messages from the server (25, 27, 29, 30), without the
    /// sequence number and opcode.
    pub fn on_binary(&mut self, opcode: u8, payload: &[u8]) -> Result<Vec<Reply>> {
        if payload.len() > MAX_MESSAGE {
            bail!("DAVE message too large ({} bytes)", payload.len());
        }
        match opcode {
            op::EXTERNAL_SENDER => {
                if let Err(e) = self.session.set_external_sender(payload) {
                    // Already in a group: the server only repeats it on upgrades.
                    debug!("DAVE: external sender not applied: {e:?}");
                }
                Ok(Vec::new())
            }
            op::PROPOSALS => {
                let (&kind, proposals) = payload.split_first().context("empty proposals")?;
                let kind = match kind {
                    0 => ProposalsOperationType::APPEND,
                    1 => ProposalsOperationType::REVOKE,
                    k => bail!("unknown proposals operation {k}"),
                };
                let recognized = self.recognized();
                match self.session.process_proposals(kind, proposals, Some(&recognized)) {
                    Ok(Some(cw)) => {
                        let mut out = cw.commit;
                        if let Some(welcome) = cw.welcome {
                            out.extend_from_slice(&welcome);
                        }
                        Ok(vec![Reply::Binary(op::COMMIT_WELCOME, out)])
                    }
                    Ok(None) => Ok(Vec::new()),
                    Err(e) => {
                        warn!("DAVE: proposals rejected: {e:?}");
                        Ok(Vec::new())
                    }
                }
            }
            op::ANNOUNCE_COMMIT => {
                let (tid, commit) = transition_and_body(payload)?;
                match self.session.process_commit(commit) {
                    Ok(true) => self.after_group_change(tid),
                    Ok(false) => Ok(Vec::new()),
                    Err(e) => {
                        warn!("DAVE: commit rejected: {e:?}");
                        self.recover(tid)
                    }
                }
            }
            op::WELCOME => {
                let (tid, welcome) = transition_and_body(payload)?;
                let recognized = self.recognized();
                match self.session.process_welcome(welcome, Some(&recognized)) {
                    Ok(()) => self.after_group_change(tid),
                    Err(e) => {
                        warn!("DAVE: welcome rejected: {e:?}");
                        self.recover(tid)
                    }
                }
            }
            _ => Ok(Vec::new()),
        }
    }

    /// True once our frames can be end-to-end encrypted.
    pub fn can_encrypt(&self) -> bool {
        self.session.can_encrypt()
    }

    /// Voice privacy code: the same for everyone in the call, to compare.
    pub fn privacy_code(&self) -> Option<String> {
        self.session
            .is_ready()
            .then(|| self.session.voice_privacy_code().map(str::to_string))
            .flatten()
    }

    /// Code to verify one participant's identity out of band.
    pub fn verification_code(&self, user_id: u64) -> Option<String> {
        self.session.get_verification_code(user_id).ok()
    }

    /// Encrypts one of our Opus frames.
    pub fn encrypt(&mut self, opus: &[u8]) -> Result<Vec<u8>> {
        match self.session.encrypt_opus(opus) {
            Ok(Cow::Borrowed(b)) => Ok(b.to_vec()),
            Ok(Cow::Owned(v)) => Ok(v),
            Err(e) => bail!("end-to-end encryption failed: {e:?}"),
        }
    }

    /// Encrypts one of our H.264 frames (Annex B, whole): the NAL headers
    /// the packetizer needs stay in clear, the rest is encrypted, the DAVE
    /// trailer goes at the end of the last NAL unit.
    pub fn encrypt_video(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        match self.session.encrypt(MediaType::VIDEO, Codec::H264, frame) {
            Ok(Cow::Borrowed(b)) => Ok(b.to_vec()),
            Ok(Cow::Owned(v)) => Ok(v),
            Err(e) => bail!("end-to-end encryption failed: {e:?}"),
        }
    }

    /// Decrypts a video frame (Annex B, put back together from the RTP
    /// packets) from a participant. As for audio, no plaintext passthrough.
    pub fn decrypt_video(&mut self, user_id: u64, frame: &[u8]) -> Result<Vec<u8>> {
        self.session
            .decrypt(user_id, MediaType::VIDEO, frame)
            .map_err(|e| anyhow!("end-to-end decryption failed: {e:?}"))
    }

    /// Decrypts a frame from a participant. Frames that are not valid DAVE
    /// frames for that user are an error (no plaintext passthrough).
    pub fn decrypt(&mut self, user_id: u64, frame: &[u8]) -> Result<Vec<u8>> {
        self.session
            .decrypt(user_id, MediaType::AUDIO, frame)
            .map_err(|e| anyhow!("end-to-end decryption failed: {e:?}"))
    }
}

/// `transition_id` (2 bytes, big endian) followed by an MLS message.
fn transition_and_body(payload: &[u8]) -> Result<(u16, &[u8])> {
    if payload.len() < 3 {
        bail!("DAVE message too short");
    }
    let (tid, body) = payload.split_at(2);
    Ok((u16::from_be_bytes([tid[0], tid[1]]), body))
}

/// Splits a binary message from the server: sequence number, opcode, payload.
pub fn parse_server_binary(message: &[u8]) -> Result<(u16, u8, &[u8])> {
    if message.len() < 3 {
        bail!("binary voice message too short");
    }
    if message.len() > MAX_MESSAGE + 3 {
        bail!("binary voice message too large");
    }
    Ok((
        u16::from_be_bytes([message[0], message[1]]),
        message[2],
        &message[3..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_zero_or_unknown_is_fatal() {
        for v in [0, MAX_VERSION + 1] {
            let err = Dave::new(v, 1, 2).err().expect("must fail");
            assert!(err.downcast_ref::<Fatal>().is_some(), "{err:#}");
        }
        let (mut dave, replies) = Dave::new(1, 1, 2).unwrap();
        assert!(matches!(&replies[..], [Reply::Binary(op::KEY_PACKAGE, kp)] if !kp.is_empty()));
        assert!(!dave.can_encrypt());
        assert!(dave.privacy_code().is_none());

        // Downgrade announced by the server: the call must end.
        let err = dave
            .on_json(
                op::PREPARE_TRANSITION,
                &json!({"protocol_version": 0, "transition_id": 5}),
            )
            .unwrap_err();
        assert!(err.downcast_ref::<Fatal>().is_some());
        let err = dave
            .on_json(op::PREPARE_EPOCH, &json!({"protocol_version": 0, "epoch": 1}))
            .unwrap_err();
        assert!(err.downcast_ref::<Fatal>().is_some());
    }

    #[test]
    fn sole_member_reset_needs_no_answer() {
        let (mut dave, _) = Dave::new(1, 7, 9).unwrap();
        // Epoch 1: a new group, so a new key package.
        let replies = dave
            .on_json(op::PREPARE_EPOCH, &json!({"protocol_version": 1, "epoch": 1}))
            .unwrap();
        assert!(matches!(&replies[..], [Reply::Binary(op::KEY_PACKAGE, _)]));
        // Transition 0 is executed at once: no "ready" to send.
        let replies = dave
            .on_json(
                op::PREPARE_TRANSITION,
                &json!({"protocol_version": 1, "transition_id": 0}),
            )
            .unwrap();
        assert!(replies.is_empty());
        assert!(!dave.can_encrypt());
        // Any other transition still answers "ready".
        let replies = dave
            .on_json(
                op::PREPARE_TRANSITION,
                &json!({"protocol_version": 1, "transition_id": 4}),
            )
            .unwrap();
        assert!(
            matches!(&replies[..], [Reply::Json(v)] if v["op"] == op::READY_FOR_TRANSITION && v["d"]["transition_id"] == 4)
        );
    }

    #[test]
    fn malformed_messages_are_errors_not_panics() {
        let (mut dave, _) = Dave::new(1, 1, 2).unwrap();
        assert!(dave.on_binary(op::PROPOSALS, &[]).is_err());
        assert!(dave.on_binary(op::PROPOSALS, &[7, 1, 2]).is_err());
        assert!(dave.on_binary(op::ANNOUNCE_COMMIT, &[0]).is_err());
        assert!(dave.on_binary(op::WELCOME, &[0, 1]).is_err());
        assert!(dave
            .on_binary(op::EXTERNAL_SENDER, &[1, 2, 3])
            .unwrap()
            .is_empty());
        // Garbage proposals are logged and ignored; garbage welcomes lead to
        // "invalid commit/welcome" and a new key package.
        assert!(dave
            .on_binary(op::PROPOSALS, &[0, 3, 1, 2, 3])
            .unwrap()
            .is_empty());
        let replies = dave.on_binary(op::WELCOME, &[0, 9, 1, 2, 3]).unwrap();
        assert!(
            matches!(&replies[0], Reply::Json(v) if v["op"] == op::INVALID_COMMIT_WELCOME && v["d"]["transition_id"] == 9)
        );
        assert!(matches!(&replies[1], Reply::Binary(op::KEY_PACKAGE, _)));
        assert!(dave.on_json(op::EXECUTE_TRANSITION, &json!({"nope": 1})).is_err());
        // Nothing to decrypt with, and nothing to encrypt with yet.
        assert!(dave.decrypt(5, &[1, 2, 3]).is_err());
        assert!(dave.encrypt(&[0x78; 20]).is_err());
        assert!(parse_server_binary(&[0, 1]).is_err());
        assert_eq!(parse_server_binary(&[0, 7, 25, 9]).unwrap(), (7, 25, &[9u8][..]));
    }
}

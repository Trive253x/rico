//! Encryption of audio packets (UDP transport).
//!
//! Discord only accepts the "rtpsize" modes: the RTP header stays in clear
//! and is authenticated as additional data (AAD), the rest is encrypted with
//! AEAD. At the end of the packet there are 4 bytes of counter (big endian)
//! from which the nonce is made, padded with zeros to the required length.
//!
//! This is the transport encryption, towards Discord's server. End-to-end
//! encryption (DAVE) is an extra layer, inside the payload.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use anyhow::{bail, Result};
use chacha20poly1305::XChaCha20Poly1305;

use super::rtp::{self, RtpHeader};

/// Length of the authentication tag (the same for both modes).
pub const TAG_LEN: usize = 16;
/// Bytes of the counter at the end of the packet.
pub const NONCE_SUFFIX_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Aes256Gcm,
    XChaCha20Poly1305,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Aes256Gcm => "aead_aes256_gcm_rtpsize",
            Mode::XChaCha20Poly1305 => "aead_xchacha20_poly1305_rtpsize",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Mode::Aes256Gcm, Mode::XChaCha20Poly1305]
            .into_iter()
            .find(|m| m.name() == name)
    }

    /// The mode to ask for among those the server offers: AES-GCM if present
    /// (the one Discord prefers), otherwise XChaCha20.
    pub fn choose<'a>(offered: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let offered: Vec<Mode> = offered.into_iter().filter_map(Mode::from_name).collect();
        [Mode::Aes256Gcm, Mode::XChaCha20Poly1305]
            .into_iter()
            .find(|m| offered.contains(m))
    }

    fn nonce_len(self) -> usize {
        match self {
            Mode::Aes256Gcm => 12,
            Mode::XChaCha20Poly1305 => 24,
        }
    }
}

pub enum Cipher {
    Aes(Box<Aes256Gcm>),
    XChaCha(Box<XChaCha20Poly1305>),
}

impl Cipher {
    pub fn new(mode: Mode, key: &[u8]) -> Result<Self> {
        if key.len() != 32 {
            bail!("{} byte key instead of 32", key.len());
        }
        Ok(match mode {
            Mode::Aes256Gcm => Cipher::Aes(Box::new(Aes256Gcm::new_from_slice(key)?)),
            Mode::XChaCha20Poly1305 => Cipher::XChaCha(Box::new(XChaCha20Poly1305::new_from_slice(key)?)),
        })
    }

    pub fn mode(&self) -> Mode {
        match self {
            Cipher::Aes(_) => Mode::Aes256Gcm,
            Cipher::XChaCha(_) => Mode::XChaCha20Poly1305,
        }
    }

    fn nonce(&self, suffix: [u8; NONCE_SUFFIX_LEN]) -> Vec<u8> {
        let mut nonce = vec![0u8; self.mode().nonce_len()];
        nonce[..NONCE_SUFFIX_LEN].copy_from_slice(&suffix);
        nonce
    }

    fn seal(&self, nonce: &[u8], aad: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
        let payload = Payload { msg, aad };
        let out = match self {
            Cipher::Aes(c) => c.encrypt(nonce.into(), payload),
            Cipher::XChaCha(c) => c.encrypt(nonce.into(), payload),
        };
        out.map_err(|_| anyhow::anyhow!("encryption failed"))
    }

    fn open(&self, nonce: &[u8], aad: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
        let payload = Payload { msg, aad };
        let out = match self {
            Cipher::Aes(c) => c.decrypt(nonce.into(), payload),
            Cipher::XChaCha(c) => c.decrypt(nonce.into(), payload),
        };
        out.map_err(|_| anyhow::anyhow!("packet not authentic"))
    }

    /// Full RTP packet: header in clear, encrypted audio, counter.
    pub fn encrypt_rtp(&self, header: &RtpHeader, audio: &[u8], counter: u32) -> Result<Vec<u8>> {
        let head = header.to_bytes();
        let suffix = counter.to_be_bytes();
        let sealed = self.seal(&self.nonce(suffix), &head, audio)?;
        let mut packet = Vec::with_capacity(head.len() + sealed.len() + NONCE_SUFFIX_LEN);
        packet.extend_from_slice(&head);
        packet.extend_from_slice(&sealed);
        packet.extend_from_slice(&suffix);
        Ok(packet)
    }

    /// Decrypts a received packet and returns header and audio.
    ///
    /// If there is an RTP extension, only its first 4 bytes are in clear
    /// (profile and length): the extension data is encrypted together with
    /// the audio and is removed from the start of the decrypted text.
    pub fn decrypt_rtp(&self, packet: &[u8]) -> Result<(RtpHeader, Vec<u8>)> {
        self.decrypt_packet(packet)
            .map(|(parsed, plain)| (parsed.header, plain))
    }

    /// Like [`decrypt_rtp`](Self::decrypt_rtp), with the whole parsed header
    /// (payload type and marker, for video).
    pub fn decrypt_packet(&self, packet: &[u8]) -> Result<(rtp::Parsed, Vec<u8>)> {
        let parsed = rtp::parse(packet)?;
        let min = parsed.clear_len + TAG_LEN + NONCE_SUFFIX_LEN;
        if packet.len() < min {
            bail!("packet too short ({} bytes)", packet.len());
        }
        let (body, suffix) = packet.split_at(packet.len() - NONCE_SUFFIX_LEN);
        let suffix: [u8; NONCE_SUFFIX_LEN] = suffix.try_into()?;
        let (aad, sealed) = body.split_at(parsed.clear_len);
        let mut plain = self.open(&self.nonce(suffix), aad, sealed)?;
        if parsed.extension_len > plain.len() {
            bail!("RTP extension longer than the packet");
        }
        plain.drain(..parsed.extension_len);
        // RTP padding (P bit, RFC 3550 §5.1): the last byte says how many bytes
        // at the end are padding, itself included. With the "rtpsize" modes it
        // is inside the encrypted part, so it is removed only now.
        if packet[0] & 0x20 != 0 {
            let pad = plain.last().copied().unwrap_or(0) as usize;
            if pad == 0 || pad > plain.len() {
                bail!("invalid RTP padding");
            }
            plain.truncate(plain.len() - pad);
        }
        Ok((parsed, plain))
    }

    /// RTCP packet: the first 8 bytes in clear (authenticated), the rest
    /// encrypted, then the counter.
    pub fn encrypt_rtcp(&self, clear: &[u8], body: &[u8], counter: u32) -> Result<Vec<u8>> {
        let suffix = counter.to_be_bytes();
        let sealed = self.seal(&self.nonce(suffix), clear, body)?;
        let mut packet = Vec::with_capacity(clear.len() + sealed.len() + NONCE_SUFFIX_LEN);
        packet.extend_from_slice(clear);
        packet.extend_from_slice(&sealed);
        packet.extend_from_slice(&suffix);
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> RtpHeader {
        RtpHeader {
            sequence: 7,
            timestamp: 960 * 7,
            ssrc: 4242,
        }
    }

    #[test]
    fn round_trip_in_both_modes() {
        for mode in [Mode::Aes256Gcm, Mode::XChaCha20Poly1305] {
            let cipher = Cipher::new(mode, &[9u8; 32]).unwrap();
            let audio = b"fake opus";
            let packet = cipher.encrypt_rtp(&header(), audio, 3).unwrap();
            assert_eq!(packet.len(), 12 + audio.len() + TAG_LEN + 4);
            assert_eq!(&packet[packet.len() - 4..], &[0, 0, 0, 3]);
            let (h, plain) = cipher.decrypt_rtp(&packet).unwrap();
            assert_eq!(h, header());
            assert_eq!(plain, audio);

            // One bit changed in the header or in the audio: rejected.
            for i in [2, 14] {
                let mut bad = packet.clone();
                bad[i] ^= 1;
                assert!(cipher.decrypt_rtp(&bad).is_err(), "{mode:?} byte {i}");
            }
            // Wrong key: rejected.
            let other = Cipher::new(mode, &[8u8; 32]).unwrap();
            assert!(other.decrypt_rtp(&packet).is_err());
        }
    }

    #[test]
    fn rtp_extension_encrypted_with_the_audio() {
        // As Discord sends them: X=1, extension header in clear (BEDE,
        // 1 word), the 4 extension bytes encrypted before the audio.
        let cipher = Cipher::new(Mode::Aes256Gcm, &[1u8; 32]).unwrap();
        let mut head = header().to_bytes().to_vec();
        head[0] |= 0x10;
        head.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
        let mut plain = vec![0x10, 0xAA, 0, 0];
        plain.extend_from_slice(b"audio");
        let suffix = 5u32.to_be_bytes();
        let sealed = cipher.seal(&cipher.nonce(suffix), &head, &plain).unwrap();
        let packet = [head.as_slice(), &sealed, &suffix].concat();
        let (h, audio) = cipher.decrypt_rtp(&packet).unwrap();
        assert_eq!(h.ssrc, 4242);
        assert_eq!(audio, b"audio");
    }

    #[test]
    fn rtp_padding_is_removed_after_decryption() {
        let cipher = Cipher::new(Mode::Aes256Gcm, &[2u8; 32]).unwrap();
        let mut head = header().to_bytes().to_vec();
        head[0] |= 0x20;
        let seal = |plain: &[u8]| {
            let suffix = 9u32.to_be_bytes();
            let sealed = cipher.seal(&cipher.nonce(suffix), &head, plain).unwrap();
            [head.as_slice(), &sealed, &suffix].concat()
        };
        // Three bytes of padding, the last one being the count.
        let (_, audio) = cipher.decrypt_rtp(&seal(b"frame\0\0\x03")).unwrap();
        assert_eq!(audio, b"frame");
        // A count of zero or longer than the payload is rejected.
        assert!(cipher.decrypt_rtp(&seal(b"frame\0")).is_err());
        assert!(cipher.decrypt_rtp(&seal(b"ab\x09")).is_err());
    }

    #[test]
    fn mode_choice() {
        let both = [
            "aead_xchacha20_poly1305_rtpsize",
            "aead_aes256_gcm_rtpsize",
            "xsalsa20_poly1305",
        ];
        assert_eq!(Mode::choose(both), Some(Mode::Aes256Gcm));
        assert_eq!(
            Mode::choose(["aead_xchacha20_poly1305_rtpsize"]),
            Some(Mode::XChaCha20Poly1305)
        );
        assert_eq!(Mode::choose(["xsalsa20_poly1305"]), None);
        assert!(Cipher::new(Mode::Aes256Gcm, &[0u8; 16]).is_err());
    }
}

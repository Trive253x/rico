//! Cifratura dei pacchetti audio (trasporto UDP).
//!
//! Discord accetta solo le modalità "rtpsize": l'intestazione RTP resta in
//! chiaro ed è autenticata come dato aggiuntivo (AAD), il resto è cifrato con
//! AEAD. In fondo al pacchetto ci sono 4 byte di contatore (big endian) da cui
//! si ricava il nonce, completato con zeri fino alla lunghezza richiesta.
//!
//! Questa è la cifratura del trasporto, verso il server di Discord. La
//! cifratura end-to-end (DAVE) è un livello in più, dentro il payload.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use anyhow::{bail, Result};
use chacha20poly1305::XChaCha20Poly1305;

use super::rtp::{self, RtpHeader};

/// Lunghezza del tag di autenticazione (uguale per le due modalità).
pub const TAG_LEN: usize = 16;
/// Byte del contatore in coda al pacchetto.
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

    /// La modalità da chiedere tra quelle offerte dal server: AES-GCM se c'è
    /// (è quella che Discord preferisce), altrimenti XChaCha20.
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
            bail!("chiave di {} byte invece di 32", key.len());
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
        out.map_err(|_| anyhow::anyhow!("cifratura non riuscita"))
    }

    fn open(&self, nonce: &[u8], aad: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
        let payload = Payload { msg, aad };
        let out = match self {
            Cipher::Aes(c) => c.decrypt(nonce.into(), payload),
            Cipher::XChaCha(c) => c.decrypt(nonce.into(), payload),
        };
        out.map_err(|_| anyhow::anyhow!("pacchetto non autentico"))
    }

    /// Pacchetto RTP completo: intestazione in chiaro, audio cifrato, contatore.
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

    /// Decifra un pacchetto ricevuto e restituisce intestazione e audio.
    ///
    /// Se c'è un'estensione RTP, in chiaro restano solo i suoi primi 4 byte
    /// (profilo e lunghezza): i dati dell'estensione sono cifrati insieme
    /// all'audio e vanno tolti dall'inizio del testo decifrato.
    pub fn decrypt_rtp(&self, packet: &[u8]) -> Result<(RtpHeader, Vec<u8>)> {
        let parsed = rtp::parse(packet)?;
        let min = parsed.clear_len + TAG_LEN + NONCE_SUFFIX_LEN;
        if packet.len() < min {
            bail!("pacchetto troppo corto ({} byte)", packet.len());
        }
        let (body, suffix) = packet.split_at(packet.len() - NONCE_SUFFIX_LEN);
        let suffix: [u8; NONCE_SUFFIX_LEN] = suffix.try_into()?;
        let (aad, sealed) = body.split_at(parsed.clear_len);
        let mut plain = self.open(&self.nonce(suffix), aad, sealed)?;
        if parsed.extension_len > plain.len() {
            bail!("estensione RTP più lunga del pacchetto");
        }
        plain.drain(..parsed.extension_len);
        Ok((parsed.header, plain))
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
    fn andata_e_ritorno_nelle_due_modalita() {
        for mode in [Mode::Aes256Gcm, Mode::XChaCha20Poly1305] {
            let cipher = Cipher::new(mode, &[9u8; 32]).unwrap();
            let audio = b"opus finto";
            let packet = cipher.encrypt_rtp(&header(), audio, 3).unwrap();
            assert_eq!(packet.len(), 12 + audio.len() + TAG_LEN + 4);
            assert_eq!(&packet[packet.len() - 4..], &[0, 0, 0, 3]);
            let (h, plain) = cipher.decrypt_rtp(&packet).unwrap();
            assert_eq!(h, header());
            assert_eq!(plain, audio);

            // Un bit cambiato nell'intestazione o nell'audio: rifiutato.
            for i in [2, 14] {
                let mut bad = packet.clone();
                bad[i] ^= 1;
                assert!(cipher.decrypt_rtp(&bad).is_err(), "{mode:?} byte {i}");
            }
            // Chiave sbagliata: rifiutato.
            let other = Cipher::new(mode, &[8u8; 32]).unwrap();
            assert!(other.decrypt_rtp(&packet).is_err());
        }
    }

    #[test]
    fn estensione_rtp_cifrata_insieme_all_audio() {
        // Come li manda Discord: X=1, intestazione dell'estensione in chiaro
        // (BEDE, 1 parola), i 4 byte dell'estensione cifrati prima dell'audio.
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
    fn scelta_della_modalita() {
        let both = ["aead_xchacha20_poly1305_rtpsize", "aead_aes256_gcm_rtpsize", "xsalsa20_poly1305"];
        assert_eq!(Mode::choose(both), Some(Mode::Aes256Gcm));
        assert_eq!(
            Mode::choose(["aead_xchacha20_poly1305_rtpsize"]),
            Some(Mode::XChaCha20Poly1305)
        );
        assert_eq!(Mode::choose(["xsalsa20_poly1305"]), None);
        assert!(Cipher::new(Mode::Aes256Gcm, &[0u8; 16]).is_err());
    }
}

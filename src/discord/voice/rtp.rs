//! Pacchetti UDP della voce: intestazione RTP e scoperta dell'indirizzo IP.

use anyhow::{bail, Result};

/// Tipo di payload RTP usato da Discord per l'audio Opus.
pub const OPUS_PAYLOAD_TYPE: u8 = 0x78;
/// Campioni per pacchetto: 20 ms a 48 kHz.
pub const SAMPLES_PER_FRAME: u32 = 960;
/// Frame Opus di silenzio: Discord ne vuole alcuni quando si smette di parlare.
pub const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// Lunghezza della richiesta e della risposta di IP discovery.
pub const DISCOVERY_LEN: usize = 74;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    /// Intestazione di 12 byte: versione 2, niente estensioni, payload Opus.
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = 0x80;
        b[1] = OPUS_PAYLOAD_TYPE;
        b[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        b[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        b[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        b
    }
}

/// Intestazione letta da un pacchetto ricevuto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub header: RtpHeader,
    /// Byte in chiaro all'inizio del pacchetto (12 + CSRC + 4 se c'è un'estensione).
    pub clear_len: usize,
    /// Byte dei dati dell'estensione, cifrati all'inizio del payload.
    pub extension_len: usize,
}

/// `true` se il pacchetto è audio RTP (e non RTCP o altro).
pub fn is_audio(packet: &[u8]) -> bool {
    packet.len() >= 12 && packet[0] >> 6 == 2 && packet[1] & 0x7F == OPUS_PAYLOAD_TYPE
}

pub fn parse(packet: &[u8]) -> Result<Parsed> {
    if !is_audio(packet) {
        bail!("non è un pacchetto audio RTP");
    }
    let csrc = (packet[0] & 0x0F) as usize;
    let has_extension = packet[0] & 0x10 != 0;
    let mut clear_len = 12 + 4 * csrc;
    let mut extension_len = 0;
    if has_extension {
        if packet.len() < clear_len + 4 {
            bail!("estensione RTP troncata");
        }
        let words = u16::from_be_bytes([packet[clear_len + 2], packet[clear_len + 3]]);
        extension_len = 4 * words as usize;
        clear_len += 4;
    }
    if packet.len() < clear_len {
        bail!("intestazione RTP troncata");
    }
    Ok(Parsed {
        header: RtpHeader {
            sequence: u16::from_be_bytes([packet[2], packet[3]]),
            timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        },
        clear_len,
        extension_len,
    })
}

/// Richiesta di IP discovery: il server risponde con l'indirizzo e la porta
/// da cui vede arrivare i nostri pacchetti (serve dietro NAT).
pub fn discovery_request(ssrc: u32) -> [u8; DISCOVERY_LEN] {
    let mut b = [0u8; DISCOVERY_LEN];
    b[0..2].copy_from_slice(&1u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    b
}

/// Risposta di IP discovery (usata anche dal server finto della demo).
pub fn discovery_response(ssrc: u32, address: &str, port: u16) -> [u8; DISCOVERY_LEN] {
    let mut b = [0u8; DISCOVERY_LEN];
    b[0..2].copy_from_slice(&2u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    let addr = address.as_bytes();
    let n = addr.len().min(63);
    b[8..8 + n].copy_from_slice(&addr[..n]);
    b[72..74].copy_from_slice(&port.to_be_bytes());
    b
}

/// Legge la risposta di IP discovery: (indirizzo, porta).
pub fn parse_discovery(packet: &[u8], ssrc: u32) -> Result<(String, u16)> {
    if packet.len() < DISCOVERY_LEN || packet[0..2] != 2u16.to_be_bytes() {
        bail!("risposta di IP discovery non valida");
    }
    if packet[4..8] != ssrc.to_be_bytes() {
        bail!("risposta di IP discovery per un altro SSRC");
    }
    let raw = &packet[8..72];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let address = std::str::from_utf8(&raw[..end])?.to_string();
    if address.is_empty() {
        bail!("IP discovery senza indirizzo");
    }
    let port = u16::from_be_bytes([packet[72], packet[73]]);
    Ok((address, port))
}

/// Pacchetto di discovery in arrivo (lo riconosce il server finto).
pub fn is_discovery_request(packet: &[u8]) -> Option<u32> {
    (packet.len() == DISCOVERY_LEN && packet[0..2] == 1u16.to_be_bytes())
        .then(|| u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intestazione_rtp() {
        let h = RtpHeader {
            sequence: 0xABCD,
            timestamp: 0x01020304,
            ssrc: 99,
        };
        let b = h.to_bytes();
        assert_eq!(&b[..4], &[0x80, 0x78, 0xAB, 0xCD]);
        let p = parse(&b).unwrap();
        assert_eq!(p.header, h);
        assert_eq!((p.clear_len, p.extension_len), (12, 0));
        // RTCP (tipo 200) e pacchetti corti non sono audio.
        assert!(!is_audio(&[0x80, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse(&b[..8]).is_err());
    }

    #[test]
    fn intestazione_con_csrc_ed_estensione() {
        let mut b = RtpHeader {
            sequence: 1,
            timestamp: 2,
            ssrc: 3,
        }
        .to_bytes()
        .to_vec();
        b[0] |= 0x10 | 0x01; // estensione e un CSRC
        b.extend_from_slice(&[0, 0, 0, 7]); // CSRC
        b.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x02]); // estensione di 2 parole
        let p = parse(&b).unwrap();
        assert_eq!((p.clear_len, p.extension_len), (20, 8));
        assert!(parse(&b[..17]).is_err());
    }

    #[test]
    fn ip_discovery() {
        let req = discovery_request(42);
        assert_eq!(req.len(), 74);
        assert_eq!(is_discovery_request(&req), Some(42));
        let resp = discovery_response(42, "203.0.113.7", 50004);
        assert_eq!(parse_discovery(&resp, 42).unwrap(), ("203.0.113.7".to_string(), 50004));
        assert!(parse_discovery(&resp, 43).is_err());
        assert!(parse_discovery(&req, 42).is_err());
    }
}

//! Voice UDP packets: RTP header and IP address discovery.

use anyhow::{bail, Result};

/// RTP payload type Discord uses for Opus audio.
pub const OPUS_PAYLOAD_TYPE: u8 = 0x78;
/// Payload types of H.264 video and its retransmissions (Go Live streams,
/// received and sent): Discord's own table (101/102; 103/104 are H.265).
/// Viewers read our packets with that table, so a stream sent as 103 was
/// taken for H.265 and never shown.
pub const H264_PAYLOAD_TYPE: u8 = 101;
pub const H264_RTX_PAYLOAD_TYPE: u8 = 102;
/// Samples per packet: 20 ms at 48 kHz.
pub const SAMPLES_PER_FRAME: u32 = 960;
/// Opus silence frame: Discord wants a few when someone stops talking.
pub const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// Length of the IP discovery request and response.
pub const DISCOVERY_LEN: usize = 74;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    /// 12 byte header: version 2, no extensions, Opus payload.
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

/// RTP header extension of our video packets (RFC 8285, one-byte form):
/// the profile and length (1 word) stay in clear, the element is encrypted
/// with the payload. The element is WebRTC's playout delay (id 5) set to
/// zero: show the frames at once, as screen sharing wants.
pub const EXTENSION_HEADER: [u8; 4] = [0xBE, 0xDE, 0x00, 0x01];
pub const PLAYOUT_DELAY: [u8; 4] = [0x52, 0x00, 0x00, 0x00];

/// Header of a video packet we send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoHeader {
    pub payload_type: u8,
    /// Last packet of a frame.
    pub marker: bool,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl VideoHeader {
    /// The part in clear: 12 byte header with the extension bit, then the
    /// extension's profile and length.
    pub fn clear_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = 0x80 | 0x10;
        b[1] = (self.payload_type & 0x7F) | if self.marker { 0x80 } else { 0 };
        b[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        b[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        b[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        b[12..16].copy_from_slice(&EXTENSION_HEADER);
        b
    }
}

/// `true` if the packet looks like RTCP (types 200–206, version 2).
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 8 && packet[0] >> 6 == 2 && (200..=206).contains(&packet[1])
}

/// What a viewer asks of our video (from RTCP feedback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feedback {
    /// Picture loss (PLI) or full intra request (FIR): send a keyframe.
    Keyframe { media_ssrc: u32 },
    /// Generic NACK: these packets were lost.
    Nack { media_ssrc: u32, lost: Vec<u16> },
}

/// Most packets a single NACK can ask again (more is ignored).
const MAX_NACKED: usize = 256;

/// Reads the feedback in a (decrypted) compound RTCP packet. Anything else
/// (reports, unknown types, truncated parts) is skipped.
pub fn parse_feedback(packet: &[u8]) -> Vec<Feedback> {
    let mut out = Vec::new();
    let mut rest = packet;
    while rest.len() >= 8 && rest[0] >> 6 == 2 {
        let fmt = rest[0] & 0x1F;
        let kind = rest[1];
        let len = 4 * (u16::from_be_bytes([rest[2], rest[3]]) as usize + 1);
        let Some(part) = rest.get(..len) else { break };
        rest = &rest[len..];
        let ssrc_at = |i: usize| -> Option<u32> {
            part.get(i..i + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        match (kind, fmt) {
            // PLI: the media SSRC after the sender's.
            (206, 1) => {
                if let Some(media_ssrc) = ssrc_at(8) {
                    out.push(Feedback::Keyframe { media_ssrc });
                }
            }
            // FIR: one entry per media SSRC (SSRC, sequence, 3 reserved).
            (206, 4) => {
                for entry in part.get(12..).unwrap_or_default().chunks_exact(8) {
                    let media_ssrc = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
                    out.push(Feedback::Keyframe { media_ssrc });
                }
            }
            // Generic NACK: (lost packet, bitmask of the 16 after it) pairs.
            (205, 1) => {
                let Some(media_ssrc) = ssrc_at(8) else { continue };
                let mut lost = Vec::new();
                for entry in part.get(12..).unwrap_or_default().chunks_exact(4) {
                    let pid = u16::from_be_bytes([entry[0], entry[1]]);
                    let blp = u16::from_be_bytes([entry[2], entry[3]]);
                    lost.push(pid);
                    for bit in 0..16u16 {
                        if blp & (1 << bit) != 0 {
                            lost.push(pid.wrapping_add(bit + 1));
                        }
                    }
                    if lost.len() >= MAX_NACKED {
                        lost.truncate(MAX_NACKED);
                        break;
                    }
                }
                if !lost.is_empty() {
                    out.push(Feedback::Nack { media_ssrc, lost });
                }
            }
            _ => {}
        }
    }
    out
}

/// RTCP sender report (RFC 3550 §6.4.1), without reception blocks: lets the
/// viewers line up audio and video. Clear part and part to encrypt.
pub fn sender_report(
    ssrc: u32,
    ntp: u64,
    rtp_timestamp: u32,
    packets: u32,
    octets: u32,
) -> ([u8; 8], [u8; 20]) {
    let mut clear = [0u8; 8];
    clear[0] = 0x80;
    clear[1] = 200;
    clear[2..4].copy_from_slice(&6u16.to_be_bytes()); // 7 words, minus one
    clear[4..8].copy_from_slice(&ssrc.to_be_bytes());
    let mut body = [0u8; 20];
    body[0..8].copy_from_slice(&ntp.to_be_bytes());
    body[8..12].copy_from_slice(&rtp_timestamp.to_be_bytes());
    body[12..16].copy_from_slice(&packets.to_be_bytes());
    body[16..20].copy_from_slice(&octets.to_be_bytes());
    (clear, body)
}

/// Wall clock as an NTP timestamp (seconds since 1900, 32.32 fixed point).
pub fn ntp_now() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() + 2_208_988_800;
    let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs << 32) | frac
}

/// Header read from a received packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub header: RtpHeader,
    /// Clear bytes at the start of the packet (12 + CSRC + 4 if there is an extension).
    pub clear_len: usize,
    /// Bytes of extension data, encrypted at the start of the payload.
    pub extension_len: usize,
    pub payload_type: u8,
    /// Last packet of a video frame.
    pub marker: bool,
}

/// `true` if the packet is RTP audio (and not RTCP or something else).
pub fn is_audio(packet: &[u8]) -> bool {
    packet.len() >= 12 && packet[0] >> 6 == 2 && packet[1] & 0x7F == OPUS_PAYLOAD_TYPE
}

/// `true` if the packet is RTP media with a dynamic payload type (96–127):
/// RTCP (types 200–206 in the same byte) and anything else are not.
pub fn is_media(packet: &[u8]) -> bool {
    packet.len() >= 12 && packet[0] >> 6 == 2 && packet[1] & 0x7F >= 96
}

pub fn parse(packet: &[u8]) -> Result<Parsed> {
    if !is_media(packet) {
        bail!("not an RTP media packet");
    }
    let csrc = (packet[0] & 0x0F) as usize;
    let has_extension = packet[0] & 0x10 != 0;
    let mut clear_len = 12 + 4 * csrc;
    let mut extension_len = 0;
    if has_extension {
        if packet.len() < clear_len + 4 {
            bail!("truncated RTP extension");
        }
        let words = u16::from_be_bytes([packet[clear_len + 2], packet[clear_len + 3]]);
        extension_len = 4 * words as usize;
        clear_len += 4;
    }
    if packet.len() < clear_len {
        bail!("truncated RTP header");
    }
    Ok(Parsed {
        header: RtpHeader {
            sequence: u16::from_be_bytes([packet[2], packet[3]]),
            timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        },
        clear_len,
        extension_len,
        payload_type: packet[1] & 0x7F,
        marker: packet[1] & 0x80 != 0,
    })
}

/// RTCP generic NACK (RFC 4585 §6.2.1): asks `media_ssrc` again for the
/// packets `lost` (each entry: a sequence number and a bitmask of the 16
/// after it). Clear part (header and our SSRC) and part to encrypt.
pub fn nack(our_ssrc: u32, media_ssrc: u32, lost: &[u16]) -> ([u8; 8], Vec<u8>) {
    let mut entries: Vec<(u16, u16)> = Vec::new();
    for &seq in lost {
        match entries.last_mut() {
            Some((pid, blp)) if seq.wrapping_sub(*pid) >= 1 && seq.wrapping_sub(*pid) <= 16 => {
                *blp |= 1 << (seq.wrapping_sub(*pid) - 1);
            }
            _ => entries.push((seq, 0)),
        }
    }
    let mut clear = [0u8; 8];
    clear[0] = 0x81; // version 2, FMT 1 (generic NACK)
    clear[1] = 205; // transport feedback
    let words = 2 + entries.len() as u16; // after the first word
    clear[2..4].copy_from_slice(&words.to_be_bytes());
    clear[4..8].copy_from_slice(&our_ssrc.to_be_bytes());
    let mut body = media_ssrc.to_be_bytes().to_vec();
    for (pid, blp) in entries {
        body.extend_from_slice(&pid.to_be_bytes());
        body.extend_from_slice(&blp.to_be_bytes());
    }
    (clear, body)
}

/// Payload of a retransmission (RFC 4588): the original sequence number,
/// then the original payload. `None` if too short (padding).
pub fn rtx_original(payload: &[u8]) -> Option<(u16, &[u8])> {
    (payload.len() > 2).then(|| (u16::from_be_bytes([payload[0], payload[1]]), &payload[2..]))
}

/// RTCP Picture Loss Indication (RFC 4585): asks the sender of `media_ssrc`
/// for a keyframe. Returns the part in clear (header and our SSRC) and the
/// part to encrypt (the SSRC of the video), as the "rtpsize" modes want.
pub fn picture_loss(our_ssrc: u32, media_ssrc: u32) -> ([u8; 8], [u8; 4]) {
    let mut clear = [0u8; 8];
    clear[0] = 0x81; // version 2, FMT 1 (PLI)
    clear[1] = 206; // payload-specific feedback
    clear[2..4].copy_from_slice(&2u16.to_be_bytes()); // 3 words, minus one
    clear[4..8].copy_from_slice(&our_ssrc.to_be_bytes());
    (clear, media_ssrc.to_be_bytes())
}

/// IP discovery request: the server answers with the address and port it
/// sees our packets coming from (needed behind NAT).
pub fn discovery_request(ssrc: u32) -> [u8; DISCOVERY_LEN] {
    let mut b = [0u8; DISCOVERY_LEN];
    b[0..2].copy_from_slice(&1u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    b
}

/// IP discovery response (also used by the demo's fake server).
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

/// Reads the IP discovery response: (address, port).
pub fn parse_discovery(packet: &[u8], ssrc: u32) -> Result<(String, u16)> {
    if packet.len() < DISCOVERY_LEN || packet[0..2] != 2u16.to_be_bytes() {
        bail!("invalid IP discovery response");
    }
    if packet[4..8] != ssrc.to_be_bytes() {
        bail!("IP discovery response for another SSRC");
    }
    let raw = &packet[8..72];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let address = std::str::from_utf8(&raw[..end])?.to_string();
    if address.is_empty() {
        bail!("IP discovery without an address");
    }
    let port = u16::from_be_bytes([packet[72], packet[73]]);
    Ok((address, port))
}

/// Incoming discovery packet (recognised by the fake server).
pub fn is_discovery_request(packet: &[u8]) -> Option<u32> {
    (packet.len() == DISCOVERY_LEN && packet[0..2] == 1u16.to_be_bytes())
        .then(|| u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_header() {
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
        // RTCP (type 200) and short packets are not audio.
        assert!(!is_audio(&[0x80, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_media(&[0x80, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!p.marker);
        assert_eq!(p.payload_type, OPUS_PAYLOAD_TYPE);
        // Video: another payload type, with the marker of the last packet.
        let mut v = b;
        v[1] = 0x80 | H264_PAYLOAD_TYPE;
        let p = parse(&v).unwrap();
        assert!(p.marker && p.payload_type == H264_PAYLOAD_TYPE);
        assert!(!is_audio(&v));
        // NACK: 10 alone, 30 with 31 and 46 in its bitmask, 60 alone.
        let (clear, body) = nack(1, 2, &[10, 30, 31, 46, 60]);
        assert_eq!(clear, [0x81, 205, 0, 5, 0, 0, 0, 1]);
        assert_eq!(body, [0, 0, 0, 2, 0, 10, 0, 0, 0, 30, 0x80, 0x01, 0, 60, 0, 0]);
        assert_eq!(rtx_original(&[0, 7, 0xAA]), Some((7, &[0xAA][..])));
        assert!(rtx_original(&[0, 7]).is_none());
        let (clear, enc) = picture_loss(1, 2);
        assert_eq!(clear, [0x81, 206, 0, 2, 0, 0, 0, 1]);
        assert_eq!(enc, [0, 0, 0, 2]);
        assert!(parse(&b[..8]).is_err());
    }

    #[test]
    fn video_header_and_feedback() {
        let h = VideoHeader {
            payload_type: H264_PAYLOAD_TYPE,
            marker: true,
            sequence: 7,
            timestamp: 9000,
            ssrc: 5,
        };
        let b = h.clear_bytes();
        let p = parse(&b).unwrap();
        assert!(p.marker && p.payload_type == H264_PAYLOAD_TYPE);
        assert_eq!((p.clear_len, p.extension_len), (16, 4));
        assert_eq!(
            (p.header.sequence, p.header.timestamp, p.header.ssrc),
            (7, 9000, 5)
        );
        assert!(!is_rtcp(&b));

        // A PLI and a NACK as we send them, then a FIR, in one compound packet.
        let (c, e) = picture_loss(1, 42);
        let mut compound = [&c[..], &e[..]].concat();
        let (c, e) = nack(1, 42, &[10, 12, 26, 30]);
        compound.extend_from_slice(&c);
        compound.extend_from_slice(&e);
        compound.extend_from_slice(&[0x84, 206, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 43, 1, 0, 0, 0]);
        assert!(is_rtcp(&compound));
        assert_eq!(
            parse_feedback(&compound),
            vec![
                Feedback::Keyframe { media_ssrc: 42 },
                Feedback::Nack {
                    media_ssrc: 42,
                    lost: vec![10, 12, 26, 30]
                },
                Feedback::Keyframe { media_ssrc: 43 },
            ]
        );
        // Truncated or other packets: nothing, no panic.
        assert!(parse_feedback(&compound[..10]).len() <= 1);
        assert!(parse_feedback(&[0x80, 200, 0, 6, 0, 0, 0, 1]).is_empty());
        assert!(parse_feedback(&[0x81, 205, 0, 2, 0, 0, 0, 1, 0, 0]).is_empty());
        let (clear, body) = sender_report(5, 1 << 32, 90, 3, 400);
        assert_eq!(clear, [0x80, 200, 0, 6, 0, 0, 0, 5]);
        assert_eq!(&body[12..], &[0, 0, 0, 3, 0, 0, 1, 144]);
        assert!(ntp_now() >> 32 > 3_900_000_000);
    }

    #[test]
    fn header_with_csrc_and_extension() {
        let mut b = RtpHeader {
            sequence: 1,
            timestamp: 2,
            ssrc: 3,
        }
        .to_bytes()
        .to_vec();
        b[0] |= 0x10 | 0x01; // extension and one CSRC
        b.extend_from_slice(&[0, 0, 0, 7]); // CSRC
        b.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x02]); // 2 word extension
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
        assert_eq!(
            parse_discovery(&resp, 42).unwrap(),
            ("203.0.113.7".to_string(), 50004)
        );
        assert!(parse_discovery(&resp, 43).is_err());
        assert!(parse_discovery(&req, 42).is_err());
    }
}

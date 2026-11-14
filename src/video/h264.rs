//! H.264 RTP packetization and depacketization (RFC 6184, packetization
//! mode 1).
//!
//! Sending (our own stream): [`packetize`] splits an Annex B frame into NAL
//! units, each in one packet if it fits, otherwise in FU-A fragments.
//!
//! Receiving:
//! The packets of a frame share the RTP timestamp; the last one has the
//! marker bit, and the next frame starts at the following sequence number.
//! Packets may arrive out of order: they wait in a small buffer, ordered by
//! sequence number, until their frame is complete. A frame is given up as
//! lost only when two later frames are already complete (a missing packet
//! that never comes); the caller is told, so it can ask for a keyframe.
//!
//! Each NAL unit is written in Annex B form with a 4-byte start code, as the
//! sender's DAVE encryptor produced the frame. Empty packets (padding used to
//! probe the bandwidth) only keep the sequence going.

use std::collections::BTreeMap;

/// Largest frame accepted (a 4K keyframe is a few hundred KB).
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
/// Most packets waiting for their frame (several frames' worth).
const MAX_PENDING: usize = 2048;
/// Complete frames after a gap before the gap is declared lost: time for a
/// retransmission (asked with a NACK) to arrive, about 130 ms at 30 fps.
const LOSS_AFTER_FRAMES: usize = 4;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// NAL unit types (H.264 table 7-1) and RTP payload structures.
const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const STAP_A: u8 = 24;
const FU_A: u8 = 28;

struct Packet {
    timestamp: u32,
    marker: bool,
    payload: Vec<u8>,
}

/// What a packet produced.
#[derive(Default, Debug)]
pub struct Output {
    /// Complete frames, in order.
    pub frames: Vec<Vec<u8>>,
    /// At least one frame was lost (or invalid): a keyframe is needed for a
    /// clean picture again.
    pub lost: bool,
}

#[derive(Default)]
pub struct Depacketizer {
    pending: BTreeMap<u64, Packet>,
    /// Sequence number (extended to 64 bits) where the next frame starts.
    next_start: Option<u64>,
    /// Last extended sequence number seen, to extend the next one.
    last_ext: Option<u64>,
}

impl Depacketizer {
    /// Adds a packet; returns the frames it completed.
    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Output {
        let mut out = Output::default();
        let ext = self.extend(seq);
        match self.next_start {
            // Old or repeated packet.
            Some(start) if ext < start => return out,
            // Not in step yet: the frame after the first end of frame we see.
            None => {
                if marker {
                    self.next_start = Some(ext + 1);
                    self.pending.retain(|&s, _| s > ext);
                }
                return out;
            }
            _ => {}
        }
        if self.pending.len() >= MAX_PENDING {
            // Far behind: start again from the next frame.
            self.pending.clear();
            self.next_start = None;
            out.lost = true;
            return out;
        }
        self.pending.insert(
            ext,
            Packet {
                timestamp,
                marker,
                payload: payload.to_vec(),
            },
        );
        loop {
            if let Some(frame) = self.complete_frame(&mut out) {
                out.frames.push(frame);
                continue;
            }
            if !self.skip_lost() {
                break;
            }
            out.lost = true;
        }
        out
    }

    /// Sequence numbers missing between the next frame and the newest packet
    /// received (at most `limit`): the ones to ask again with a NACK.
    pub fn missing(&self, limit: usize) -> Vec<u16> {
        let (Some(start), Some((&newest, _))) = (self.next_start, self.pending.iter().next_back()) else {
            return Vec::new();
        };
        (start..newest)
            .filter(|s| !self.pending.contains_key(s))
            .take(limit)
            .map(|s| s as u16)
            .collect()
    }

    /// 16-bit sequence numbers → 64 bits, following the wrap-around.
    fn extend(&mut self, seq: u16) -> u64 {
        let ext = match self.last_ext {
            None => u64::from(seq) + (1 << 16),
            Some(last) => {
                let base = last & !0xFFFF;
                let candidates = [base.wrapping_sub(1 << 16), base, base + (1 << 16)];
                candidates
                    .into_iter()
                    .map(|b| b + u64::from(seq))
                    .min_by_key(|c| c.abs_diff(last))
                    .unwrap_or(last)
            }
        };
        if self.last_ext.is_none_or(|l| ext > l) {
            self.last_ext = Some(ext);
        }
        ext
    }

    /// The next frame, if all its packets are here.
    fn complete_frame(&mut self, out: &mut Output) -> Option<Vec<u8>> {
        let start = self.next_start?;
        let mut timestamp: Option<u32> = None;
        let mut end = start;
        loop {
            let p = self.pending.get(&end)?;
            // Padding does not count: it carries the previous frame's time.
            if !p.payload.is_empty() {
                match timestamp {
                    None => timestamp = Some(p.timestamp),
                    Some(t) if t != p.timestamp => {
                        // Another frame already: the end of this one was lost.
                        self.pending.retain(|&s, _| s >= end);
                        self.next_start = Some(end);
                        out.lost = true;
                        return self.complete_frame(out);
                    }
                    _ => {}
                }
            }
            if p.marker {
                break;
            }
            end += 1;
        }
        let packets: Vec<Packet> = (start..=end).filter_map(|s| self.pending.remove(&s)).collect();
        self.next_start = Some(end + 1);
        if packets.iter().all(|p| p.payload.is_empty()) {
            return self.complete_frame(out);
        }
        if let Some(frame) = assemble(packets.iter().map(|p| p.payload.as_slice())) {
            Some(frame)
        } else {
            // Complete but invalid: dropped, and the next one is tried.
            out.lost = true;
            self.complete_frame(out)
        }
    }

    /// A packet of the next frame never came: if enough later frames are
    /// complete, gives it up and moves to the first packet of a later frame
    /// (by timestamp). `true` if it did.
    fn skip_lost(&mut self) -> bool {
        let Some(start) = self.next_start else {
            return false;
        };
        let mut media = self.pending.range(start..).filter(|(_, p)| !p.payload.is_empty());
        let Some((_, first)) = media.next() else {
            return false;
        };
        let broken = first.timestamp;
        let later_ends = self
            .pending
            .range(start..)
            .filter(|(_, p)| p.marker && p.timestamp != broken)
            .count();
        if later_ends < LOSS_AFTER_FRAMES {
            return false;
        }
        let Some(next) = self
            .pending
            .range(start..)
            .find(|(_, p)| !p.payload.is_empty() && p.timestamp != broken)
            .map(|(&s, _)| s)
        else {
            return false;
        };
        self.pending.retain(|&s, _| s >= next);
        self.next_start = Some(next);
        true
    }
}

/// Puts the RTP payloads of one frame together; `None` if one is invalid.
fn assemble<'a>(payloads: impl Iterator<Item = &'a [u8]>) -> Option<Vec<u8>> {
    let mut frame = Vec::new();
    let mut in_fragment = false;
    for p in payloads {
        // Padding only: nothing to add.
        let Some(&first) = p.first() else { continue };
        match first & 0x1F {
            1..=23 => {
                if in_fragment {
                    return None;
                }
                write(&mut frame, &[&START_CODE, p])?;
            }
            STAP_A => {
                if in_fragment {
                    return None;
                }
                let mut rest = &p[1..];
                while !rest.is_empty() {
                    if rest.len() < 2 {
                        return None;
                    }
                    let size = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                    if size == 0 || rest.len() < 2 + size {
                        return None;
                    }
                    write(&mut frame, &[&START_CODE, &rest[2..2 + size]])?;
                    rest = &rest[2 + size..];
                }
            }
            FU_A => {
                if p.len() < 3 {
                    return None;
                }
                let header = p[1];
                let (start, end) = (header & 0x80 != 0, header & 0x40 != 0);
                let data = &p[2..];
                if start {
                    if in_fragment {
                        return None;
                    }
                    // The NAL header is rebuilt from the FU indicator and header.
                    let nal = (first & 0xE0) | (header & 0x1F);
                    write(&mut frame, &[&START_CODE, &[nal], data])?;
                } else {
                    if !in_fragment {
                        return None;
                    }
                    write(&mut frame, &[data])?;
                }
                in_fragment = !end;
            }
            // STAP-B, MTAP and FU-B are not used in packetization mode 1.
            _ => return None,
        }
    }
    (!in_fragment && !frame.is_empty()).then_some(frame)
}

fn write(frame: &mut Vec<u8>, parts: &[&[u8]]) -> Option<()> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    if frame.len() + len > MAX_FRAME {
        return None;
    }
    for p in parts {
        frame.extend_from_slice(p);
    }
    Some(())
}

/// Largest RTP payload we send: with the RTP header, the extension, the
/// transport tag and nonce a packet stays well below the usual 1500 byte MTU.
pub const MAX_PAYLOAD: usize = 1100;

/// NAL units of an Annex B frame (3 or 4-byte start codes), without the
/// start codes. Empty units are skipped.
pub fn split_nals(frame: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= frame.len() {
        if frame[i] == 0 && frame[i + 1] == 0 && frame[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &(_, begin)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(frame.len(), |&(code, _)| code);
        // A 4-byte start code: its leading zero belongs to it, not to this unit.
        if starts.get(k + 1).is_some() && end > begin && frame[end - 1] == 0 {
            end -= 1;
        }
        if end > begin {
            out.push(&frame[begin..end]);
        }
    }
    out
}

/// RTP payloads of one frame, in order: a NAL unit per packet when it fits in
/// `max` bytes, otherwise FU-A fragments. The last payload is the end of the
/// frame (the RTP marker goes on its packet).
pub fn packetize(frame: &[u8], max: usize) -> Vec<Vec<u8>> {
    let max = max.max(3);
    let mut out = Vec::new();
    for nal in split_nals(frame) {
        if nal.len() <= max {
            out.push(nal.to_vec());
            continue;
        }
        let indicator = (nal[0] & 0xE0) | FU_A;
        let kind = nal[0] & 0x1F;
        let chunks: Vec<&[u8]> = nal[1..].chunks(max - 2).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut header = kind;
            if i == 0 {
                header |= 0x80;
            }
            if i == chunks.len() - 1 {
                header |= 0x40;
            }
            let mut p = Vec::with_capacity(chunk.len() + 2);
            p.push(indicator);
            p.push(header);
            p.extend_from_slice(chunk);
            out.push(p);
        }
    }
    out
}

/// The frame starts a new picture sequence (IDR, or parameter sets): after a
/// loss, decoding can resume from it.
pub fn is_keyframe(frame: &[u8]) -> bool {
    nal_types(frame).any(|t| t == NAL_IDR || t == NAL_SPS)
}

/// Types of the NAL units of an Annex B frame.
fn nal_types(frame: &[u8]) -> impl Iterator<Item = u8> + '_ {
    frame
        .windows(4)
        .enumerate()
        .filter(|(_, w)| w[..3] == [0, 0, 1])
        .filter_map(|(i, _)| frame.get(i + 3).map(|b| b & 0x1F))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(kind: u8, len: usize) -> Vec<u8> {
        let mut v = vec![0x60 | kind];
        v.extend((0..len).map(|i| (i % 200) as u8 + 1));
        v
    }

    /// Packets of a frame: (payload, marker).
    fn fu_packets(n: &[u8], chunk: usize) -> Vec<(Vec<u8>, bool)> {
        let chunks: Vec<&[u8]> = n[1..].chunks(chunk).collect();
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut h = n[0] & 0x1F;
                if i == 0 {
                    h |= 0x80;
                }
                if i == chunks.len() - 1 {
                    h |= 0x40;
                }
                let mut p = vec![(n[0] & 0xE0) | FU_A, h];
                p.extend_from_slice(c);
                (p, i == chunks.len() - 1)
            })
            .collect()
    }

    /// A depacketizer already in step (it saw the end of a frame at `seq - 1`).
    fn synced(seq: u16) -> Depacketizer {
        let mut d = Depacketizer::default();
        d.push(seq.wrapping_sub(1), 0, true, &nal(1, 3));
        d
    }

    #[test]
    fn single_nal_and_stap_a() {
        let mut d = synced(1);
        let sps = nal(7, 10);
        let pps = nal(8, 4);
        let mut stap = vec![0x78];
        for n in [&sps, &pps] {
            stap.extend_from_slice(&(n.len() as u16).to_be_bytes());
            stap.extend_from_slice(n);
        }
        assert!(d.push(1, 90, false, &stap).frames.is_empty());
        let idr = nal(5, 20);
        let out = d.push(2, 90, true, &idr);
        let mut want = Vec::new();
        for n in [&sps, &pps, &idr] {
            want.extend_from_slice(&START_CODE);
            want.extend_from_slice(n);
        }
        assert_eq!(out.frames, vec![want.clone()]);
        assert!(!out.lost);
        assert!(is_keyframe(&want));
    }

    #[test]
    fn out_of_order_padding_and_wrap_around() {
        let idr = nal(5, 3000);
        let packets = fu_packets(&idr, 1000);
        // Starts just before the 16-bit wrap-around; padding in the middle;
        // the packets arrive shuffled.
        let mut d = synced(65534);
        let seqs = [65534u16, 65535, 0, 1];
        let items = [
            (seqs[0], packets[0].clone()),
            (seqs[1], (Vec::new(), false)), // padding
            (seqs[2], packets[1].clone()),
            (seqs[3], packets[2].clone()),
        ];
        let mut frames = Vec::new();
        for i in [2, 0, 3, 1] {
            let (s, (p, m)) = &items[i];
            let out = d.push(*s, 1000, *m, p);
            assert!(!out.lost);
            frames.extend(out.frames);
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(&frames[0][4..], &idr[..]);
    }

    #[test]
    fn a_lost_packet_is_given_up_after_some_frames() {
        let mut d = synced(10);
        let big = fu_packets(&nal(5, 2000), 1000);
        // Frame 1: packets 10, 11 (11 never arrives).
        assert!(d.push(10, 1, big[0].1, &big[0].0).frames.is_empty());
        // Later frames complete, but wait behind the gap...
        for (seq, ts) in [(12u16, 2u32), (13, 3), (14, 4)] {
            let out = d.push(seq, ts, true, &nal(1, 5));
            assert!(out.frames.is_empty() && !out.lost);
        }
        // ...until enough of them: the gap is given up, the loss reported.
        let out = d.push(15, 5, true, &nal(1, 6));
        assert!(out.lost);
        assert_eq!(out.frames.len(), 4);
        // Then everything flows again.
        let out = d.push(16, 6, true, &nal(1, 7));
        assert_eq!(out.frames.len(), 1);
        assert!(!out.lost && !is_keyframe(&out.frames[0]));
        // Old and repeated packets are ignored.
        assert!(d.push(12, 2, true, &nal(1, 5)).frames.is_empty());
    }

    #[test]
    fn missing_packets_to_ask_again() {
        let mut d = synced(100);
        let big = fu_packets(&nal(5, 4000), 1000);
        d.push(100, 1, false, &big[0].0);
        d.push(103, 1, true, &big[3].0);
        assert_eq!(d.missing(10), vec![101, 102]);
        assert_eq!(d.missing(1), vec![101]);
        // The retransmissions arrive: the frame is complete, nothing is missing.
        d.push(102, 1, false, &big[2].0);
        let out = d.push(101, 1, false, &big[1].0);
        assert_eq!(out.frames.len(), 1);
        assert!(d.missing(10).is_empty());
    }

    #[test]
    fn packetize_and_put_back_together() {
        // SPS, PPS, a big IDR (fragmented) and a small slice, with 4 and 3 byte
        // start codes.
        let (sps, pps, idr, slice) = (nal(7, 10), nal(8, 4), nal(5, 5000), nal(1, 30));
        let mut frame = Vec::new();
        for (n, code) in [
            (&sps, &START_CODE[..]),
            (&pps, &[0, 0, 1][..]),
            (&idr, &START_CODE[..]),
            (&slice, &START_CODE[..]),
        ] {
            frame.extend_from_slice(code);
            frame.extend_from_slice(n);
        }
        assert_eq!(split_nals(&frame), vec![&sps[..], &pps[..], &idr[..], &slice[..]]);
        let payloads = packetize(&frame, MAX_PAYLOAD);
        assert!(payloads.iter().all(|p| p.len() <= MAX_PAYLOAD));
        assert_eq!(payloads.len(), 3 + 5000usize.div_ceil(MAX_PAYLOAD - 2));
        let mut d = synced(10);
        let mut frames = Vec::new();
        let last = payloads.len() - 1;
        for (i, p) in payloads.iter().enumerate() {
            frames.extend(d.push(10 + i as u16, 900, i == last, p).frames);
        }
        // Back with 4-byte start codes everywhere.
        let expected: Vec<u8> = [&sps, &pps, &idr, &slice]
            .iter()
            .flat_map(|n| START_CODE.iter().chain(n.iter()).copied())
            .collect();
        assert_eq!(frames, vec![expected]);
        assert!(is_keyframe(&frames[0]));
        // Nothing to send without start codes.
        assert!(packetize(&[1, 2, 3], MAX_PAYLOAD).is_empty());
    }

    #[test]
    fn invalid_packets_never_panic() {
        let mut d = synced(1);
        for (i, p) in [
            &[][..],
            &[0x78, 0, 50, 1],
            &[0x7C],
            &[0x7C, 0x00],
            &[0x7D, 1, 2],
            &[0x79, 1],
        ]
        .into_iter()
        .enumerate()
        {
            let _ = d.push(1 + i as u16, i as u32, true, p);
        }
        // A fragment without its start: invalid frame, reported as lost.
        let out = d.push(7, 7, true, &[0x7C, 0x45, 1, 2, 3]);
        assert!(out.frames.is_empty() && out.lost);
        let big = nal(1, MAX_FRAME + 10);
        assert!(d.push(8, 8, true, &big).frames.is_empty());
        // Not in step yet: nothing until the end of a frame.
        let mut fresh = Depacketizer::default();
        assert!(fresh.push(5, 1, false, &nal(1, 3)).frames.is_empty());
        assert!(fresh.push(6, 1, true, &nal(1, 3)).frames.is_empty());
        assert_eq!(fresh.push(7, 2, true, &nal(1, 3)).frames.len(), 1);
    }
}

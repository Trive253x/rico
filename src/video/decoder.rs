//! H.264 decoding on its own thread, with OpenH264.
//!
//! This is the only file that uses OpenH264 (Cisco's C++ decoder, through
//! the `openh264` crate and FFI). Its input is video sent by other people, so:
//!
//! - it runs on a dedicated thread, fed through a small bounded queue: if it
//!   falls behind, frames are dropped (never queued without limit) and only
//!   a keyframe restarts the picture;
//! - a frame that does not decode (often a lost reference) does not stop the
//!   others: a keyframe is asked for (RTCP PLI, by the network side) and
//!   decoding goes on, so the picture comes back as soon as one arrives;
//! - a panic inside the decoding call is caught; the decoder is then thrown
//!   away and a new one waits for a keyframe;
//! - pictures larger than 4K are refused before any RGBA buffer is allocated;
//! - no frame, picture or key is ever logged.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use anyhow::Result;
use openh264::decoder::{DecodeOptions, DecodedYUV, Decoder, Flush};
use openh264::formats::YUVSource;
use tracing::{debug, warn};

use super::{Picture, MAX_HEIGHT, MAX_WIDTH};

/// Frames waiting to be decoded (about a tenth of a second at 30 fps).
const QUEUE: usize = 4;

/// Where decoded pictures go (called on the decoder thread).
pub type Sink = Box<dyn FnMut(Picture) + Send>;

/// Counters shared with the network side and the interface.
#[derive(Default)]
pub struct Shared {
    /// The decoder cannot go on without a keyframe (start, queue full,
    /// panic): other frames are skipped until one arrives.
    pub need_keyframe: AtomicBool,
    /// A keyframe would help (lost or broken frames): asked for, but the
    /// following frames are still decoded.
    pub want_keyframe: AtomicBool,
    pub decoded: AtomicU64,
    pub dropped: AtomicU64,
    /// Frames the decoder refused (part of `dropped`).
    pub errors: AtomicU64,
    pub keyframes: AtomicU64,
}

/// Handle to the decoder thread. Dropping it stops the thread.
pub struct VideoDecoder {
    tx: SyncSender<Vec<u8>>,
    pub shared: Arc<Shared>,
}

impl VideoDecoder {
    pub fn start(sink: Sink) -> Result<Self> {
        let (tx, rx) = sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            need_keyframe: AtomicBool::new(true),
            ..Default::default()
        });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || run(rx, sink, s))?;
        Ok(Self { tx, shared })
    }

    /// Queues an Annex B frame. While a keyframe is needed, other frames are
    /// skipped. `false` if it was dropped.
    pub fn push(&self, frame: Vec<u8>) -> bool {
        let keyframe = super::h264::is_keyframe(&frame);
        if self.shared.need_keyframe.load(Ordering::Relaxed) {
            if !keyframe {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            self.shared.need_keyframe.store(false, Ordering::Relaxed);
        }
        if keyframe {
            self.shared.want_keyframe.store(false, Ordering::Relaxed);
        }
        match self.tx.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                // Behind: what follows depends on the dropped frame.
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                self.shared.need_keyframe.store(true, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// A keyframe should be asked for (needed or just useful).
    pub fn needs_keyframe(&self) -> bool {
        self.shared.need_keyframe.load(Ordering::Relaxed) || self.shared.want_keyframe.load(Ordering::Relaxed)
    }

    /// Frames were lost before reaching the decoder.
    pub fn frames_lost(&self) {
        self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        self.shared.want_keyframe.store(true, Ordering::Relaxed);
    }
}

fn run(rx: Receiver<Vec<u8>>, mut sink: Sink, shared: Arc<Shared>) {
    let mut decoder: Option<Decoder> = None;
    while let Ok(frame) = rx.recv() {
        if decoder.is_none() {
            match Decoder::new() {
                Ok(d) => decoder = Some(d),
                Err(e) => {
                    warn!("video: decoder not available: {e}");
                    return;
                }
            }
        }
        let Some(d) = decoder.as_mut() else { return };
        if super::h264::is_keyframe(&frame) && shared.keyframes.fetch_add(1, Ordering::Relaxed) < 2 {
            debug!("video: keyframe ({:?})", nal_summary(&frame));
        }
        let result = catch_unwind(AssertUnwindSafe(|| decode(d, &frame)));
        match result {
            Ok(Ok(Some(picture))) => {
                if shared.decoded.load(Ordering::Relaxed) < 6 {
                    debug!("video: frame decoded ({:?})", nal_summary(&frame));
                }
                shared.decoded.fetch_add(1, Ordering::Relaxed);
                sink(picture);
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                if shared.errors.fetch_add(1, Ordering::Relaxed) < 6 {
                    debug!(
                        "video: frame not decoded: {e:#} (NAL types {:?})",
                        nal_summary(&frame)
                    );
                }
                shared.dropped.fetch_add(1, Ordering::Relaxed);
                shared.want_keyframe.store(true, Ordering::Relaxed);
            }
            Err(_) => {
                warn!("video: the decoder failed, starting a new one");
                decoder = None;
                shared.dropped.fetch_add(1, Ordering::Relaxed);
                shared.need_keyframe.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Types of the NAL units of a frame, for the log (no content): for slices
/// also the slice type (P, B, I), for parameter sets the profile.
fn nal_summary(frame: &[u8]) -> Vec<String> {
    let starts: Vec<usize> = (0..frame.len().saturating_sub(3))
        .filter(|&i| frame[i..i + 3] == [0, 0, 1])
        .map(|i| i + 3)
        .collect();
    starts
        .iter()
        .enumerate()
        .take(8)
        .map(|(n, &s)| {
            let end = starts.get(n + 1).map_or(frame.len(), |&e| e - 3);
            let nal = &frame[s..end.max(s)];
            let kind = nal.first().map_or(0, |b| b & 0x1F);
            match kind {
                1 | 5 => {
                    let slice = slice_type(&nal[1..]).map_or("?", |t| match t % 5 {
                        0 => "P",
                        1 => "B",
                        2 => "I",
                        _ => "SP/SI",
                    });
                    format!("{kind}:{slice}:{}", nal.len())
                }
                7 => format!("7:profile {}", nal.get(1).copied().unwrap_or(0)),
                _ => format!("{kind}:{}", nal.len()),
            }
        })
        .collect()
}

/// `slice_type` from a slice header (the second Exp-Golomb number).
fn slice_type(rbsp: &[u8]) -> Option<u32> {
    // The first bytes are enough; emulation prevention bytes (00 00 03) removed.
    let mut bytes = Vec::with_capacity(16);
    let mut zeros = 0;
    for &b in rbsp.iter().take(16) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        bytes.push(b);
    }
    let mut bit = 0usize;
    let mut read_ue = || -> Option<u32> {
        let get = |i: usize| bytes.get(i / 8).map(|b| (b >> (7 - i % 8)) & 1);
        let mut lead = 0;
        while get(bit)? == 0 {
            lead += 1;
            bit += 1;
            if lead > 31 {
                return None;
            }
        }
        bit += 1;
        let mut value = 0u32;
        for _ in 0..lead {
            value = (value << 1) | u32::from(get(bit)?);
            bit += 1;
        }
        Some((1 << lead) - 1 + value)
    };
    read_ue()?; // first_mb_in_slice
    read_ue()
}

fn decode(decoder: &mut Decoder, frame: &[u8]) -> Result<Option<Picture>> {
    // Never flush while a stream is running. In OpenH264 (up to 2.6, see
    // cisco/openh264 PR #4037) a flush in single-threaded decoding does not
    // give the flushed picture's buffer back to the pool: with High profile
    // streams (the decoder holds pictures back to reorder them) the pool is
    // empty after a few frames, and every frame fails until a keyframe. The
    // crate flushes by default whenever a frame gives no picture. Without it,
    // a held picture simply comes out with the next frame (one frame later).
    let options = DecodeOptions::new().flush_after_decode(Flush::NoFlush);
    match decoder.decode_with_options(frame, options)? {
        Some(yuv) => Ok(Some(to_picture(&yuv)?)),
        None => Ok(None),
    }
}

fn to_picture(yuv: &DecodedYUV<'_>) -> Result<Picture> {
    let (w, h) = yuv.dimensions();
    if w == 0 || h == 0 || w > MAX_WIDTH || h > MAX_HEIGHT {
        anyhow::bail!("picture size not supported ({w}×{h})");
    }
    let mut rgba = vec![0u8; w * h * 4];
    yuv.write_rgba8(&mut rgba);
    Ok(Picture {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A real H.264 stream made with OpenH264's encoder, packetized as
    /// Discord does (FU-A for big NAL units), put back together and decoded.
    #[test]
    fn encode_packetize_depacketize_decode() {
        use openh264::encoder::Encoder;
        use openh264::formats::{RgbSliceU8, YUVBuffer};

        let (w, h) = (64usize, 48usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 80, 160]).collect();
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (w, h)));
        let mut encoder = Encoder::new().unwrap();
        let bitstream = encoder.encode(&yuv).unwrap().to_vec();
        assert!(crate::video::h264::is_keyframe(&bitstream));

        // Annex B → RTP payloads (single NAL or FU-A) → depacketizer.
        let mut nals: Vec<&[u8]> = Vec::new();
        let mut i = 0;
        let starts: Vec<usize> = (0..bitstream.len().saturating_sub(3))
            .filter(|&k| bitstream[k..k + 3] == [0, 0, 1])
            .map(|k| k + 3)
            .collect();
        for (n, &s) in starts.iter().enumerate() {
            let end = starts.get(n + 1).map_or(bitstream.len(), |&e| {
                let mut e = e - 3;
                while e > s && bitstream[e - 1] == 0 {
                    e -= 1;
                }
                e
            });
            nals.push(&bitstream[s..end]);
            i += 1;
        }
        assert!(i >= 3, "SPS, PPS and a slice");
        let mut packets: Vec<Vec<u8>> = Vec::new();
        for nal in &nals {
            if nal.len() <= 200 {
                packets.push(nal.to_vec());
                continue;
            }
            let chunks: Vec<&[u8]> = nal[1..].chunks(200).collect();
            for (k, c) in chunks.iter().enumerate() {
                let mut fu = vec![(nal[0] & 0xE0) | 28, nal[0] & 0x1F];
                if k == 0 {
                    fu[1] |= 0x80;
                }
                if k == chunks.len() - 1 {
                    fu[1] |= 0x40;
                }
                fu.extend_from_slice(c);
                packets.push(fu);
            }
        }
        let mut d = crate::video::h264::Depacketizer::default();
        let last = packets.len() - 1;
        // In step after the end of a previous frame, as on a real stream.
        d.push(u16::MAX, 0, true, &[0x41, 1]);
        let mut frames = Vec::new();
        for (k, p) in packets.iter().enumerate() {
            frames.extend(d.push(k as u16, 3000, k == last, p).frames);
        }
        let frame = frames.pop().expect("frame put back together");

        let (tx, rx) = mpsc::channel();
        let decoder = VideoDecoder::start(Box::new(move |p| {
            let _ = tx.send(p);
        }))
        .unwrap();
        assert!(decoder.push(frame));
        let picture = rx.recv_timeout(Duration::from_secs(10)).expect("a picture");
        assert_eq!((picture.width, picture.height), (w as u32, h as u32));
        assert_eq!(picture.rgba.len(), w * h * 4);
        assert_eq!(decoder.shared.decoded.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn slice_types_for_the_log() {
        // first_mb_in_slice = 0 ("1"), slice_type = 0 ("1"): P.
        assert_eq!(slice_type(&[0b1100_0000]), Some(0));
        // first_mb 0, slice_type = 1 ("010"): B.
        assert_eq!(slice_type(&[0b1010_0000]), Some(1));
        // slice_type = 7 ("0001000"): I.
        assert_eq!(slice_type(&[0b1000_1000]), Some(7));
        assert_eq!(slice_type(&[]), None);
    }

    #[test]
    fn garbage_is_dropped_not_fatal() {
        let decoder = VideoDecoder::start(Box::new(|_| {})).unwrap();
        // Not a keyframe while one is needed: skipped before the decoder.
        assert!(!decoder.push(vec![0, 0, 0, 1, 0x41, 1, 2, 3]));
        // Looks like a keyframe but is garbage: the decoder survives it.
        assert!(decoder.push(vec![0, 0, 0, 1, 0x65, 0xFF, 0x00, 0x13, 0x37]));
        std::thread::sleep(Duration::from_millis(200));
        // A keyframe is asked for, but the following frames are not skipped.
        assert!(decoder.needs_keyframe());
        assert!(decoder.push(vec![0, 0, 0, 1, 0x41, 1, 2, 3]));
    }
}

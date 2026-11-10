//! H.264 encoding of our own Go Live stream, with OpenH264 on its own thread.
//!
//! The capture (screen or window, see [`crate::capture`]) puts its latest
//! picture, already scaled and converted to I420, in a one-place slot: if
//! the encoder is busy the older picture is replaced, never queued. The
//! encoder thread takes it at most `fps` times a second and puts the encoded
//! frames (Annex B, 4-byte start codes) in an [`Outbox`] read by the stream
//! connection, also bounded.
//!
//! - A keyframe is made at the start, when the connection asks for one
//!   (someone started watching or lost packets: RTCP PLI), and every
//!   [`KEYFRAME_EVERY`] anyway.
//! - When the screen does not change, Windows sends no pictures: the last
//!   one is encoded again once a second, so the stream stays alive and new
//!   viewers get a picture.
//! - A panic inside OpenH264 is caught: the encoder is thrown away and a new
//!   one starts with a keyframe.
//! - No picture or frame is ever logged.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, UsageType,
};
use openh264::formats::YUVSlices;
use openh264::OpenH264API;
use tracing::{debug, warn};

/// Encoded frames waiting for the connection (a few frames: if it falls
/// behind, the oldest are dropped and a keyframe is made).
const OUTBOX: usize = 6;
/// A keyframe at least this often, even if nobody asks.
pub const KEYFRAME_EVERY: Duration = Duration::from_secs(10);
/// The last picture is sent again after this long without a new one.
const REPEAT_AFTER: Duration = Duration::from_secs(1);
/// RTP clock of video (90 kHz).
pub const RTP_CLOCK: u64 = 90_000;

/// Size, frame rate and bit rate of what we send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
pub enum Quality {
    /// 1280×720 at 30 frames per second: what Discord allows without Nitro.
    #[default]
    Hd30,
    /// 1280×720 at 60 frames per second.
    Hd60,
    /// 1920×1080 at 30 frames per second.
    FullHd30,
    /// 854×480 at 30 frames per second, for slow connections.
    Sd30,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Hd30, Quality::Hd60, Quality::FullHd30, Quality::Sd30];

    /// Largest size sent (the picture is fitted inside, never enlarged).
    pub fn max_size(self) -> (u32, u32) {
        match self {
            Quality::Hd30 | Quality::Hd60 => (1280, 720),
            Quality::FullHd30 => (1920, 1080),
            Quality::Sd30 => (854, 480),
        }
    }

    pub fn fps(self) -> u32 {
        match self {
            Quality::Hd60 => 60,
            _ => 30,
        }
    }

    /// Target bit rate (bits per second).
    pub fn bitrate(self) -> u32 {
        match self {
            Quality::Hd30 => 2_500_000,
            Quality::Hd60 => 4_000_000,
            Quality::FullHd30 => 4_500_000,
            Quality::Sd30 => 1_200_000,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Hd30 => "720p 30 fps",
            Quality::Hd60 => "720p 60 fps",
            Quality::FullHd30 => "1080p 30 fps",
            Quality::Sd30 => "480p 30 fps",
        }
    }
}

/// A picture in I420 (Y plane, then U and V at half size), even sizes.
#[derive(Clone)]
pub struct Yuv {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Yuv {
    /// `None` if the sizes are odd, zero or do not match the data.
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> Option<Self> {
        let ok = width > 0
            && height > 0
            && width % 2 == 0
            && height % 2 == 0
            && width <= super::MAX_WIDTH
            && height <= super::MAX_HEIGHT
            && data.len() == width * height * 3 / 2;
        ok.then_some(Self { width, height, data })
    }

    fn planes(&self) -> (&[u8], &[u8], &[u8]) {
        let y = self.width * self.height;
        let c = y / 4;
        (
            &self.data[..y],
            &self.data[y..y + c],
            &self.data[y + c..y + 2 * c],
        )
    }
}

/// An encoded frame for the connection.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Annex B, with 4-byte start codes.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// RTP timestamp (90 kHz since the start).
    pub timestamp: u32,
}

/// Encoded frames, between the encoder thread and the connection. Shared
/// (`Arc`): a reconnection reads from the same outbox.
#[derive(Default)]
pub struct Outbox {
    frames: Mutex<VecDeque<EncodedFrame>>,
    /// Woken when a frame arrives.
    pub notify: tokio::sync::Notify,
    keyframe: AtomicBool,
    pub encoded: AtomicU64,
    pub dropped: AtomicU64,
}

impl Outbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn push(&self, frame: EncodedFrame) {
        let Ok(mut q) = self.frames.lock() else { return };
        if q.len() >= OUTBOX {
            // The connection is behind: what follows depends on dropped frames.
            let n = q.len();
            q.clear();
            self.dropped.fetch_add(n as u64, Ordering::Relaxed);
            self.keyframe.store(true, Ordering::Relaxed);
            if !frame.keyframe {
                return;
            }
        }
        q.push_back(frame);
        drop(q);
        self.encoded.fetch_add(1, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// Takes the frames ready to send.
    pub fn take(&self) -> Vec<EncodedFrame> {
        self.frames
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    /// Asks the encoder for a keyframe (a viewer joined or lost packets).
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    fn keyframe_requested(&self) -> bool {
        self.keyframe.swap(false, Ordering::Relaxed)
    }
}

/// The latest captured picture: one place, replaced if not yet taken.
#[derive(Default)]
struct Slot {
    picture: Mutex<Option<Yuv>>,
    wake: Condvar,
    stop: AtomicBool,
}

/// Where the capture puts its pictures. Cheap to clone.
#[derive(Clone)]
pub struct PictureInput(Arc<Slot>);

impl PictureInput {
    pub fn push(&self, picture: Yuv) {
        if let Ok(mut p) = self.0.picture.lock() {
            *p = Some(picture);
        }
        self.0.wake.notify_one();
    }
}

/// Handle to the encoder thread. Dropping it stops the thread.
pub struct VideoEncoder {
    slot: Arc<Slot>,
    pub outbox: Arc<Outbox>,
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        self.slot.stop.store(true, Ordering::Relaxed);
        self.slot.wake.notify_one();
    }
}

impl VideoEncoder {
    pub fn start(quality: Quality) -> Result<Self> {
        let slot = Arc::new(Slot::default());
        let outbox = Outbox::new();
        let (s, o) = (slot.clone(), outbox.clone());
        std::thread::Builder::new()
            .name("video-encode".into())
            .spawn(move || run(s, o, quality))?;
        Ok(Self { slot, outbox })
    }

    pub fn input(&self) -> PictureInput {
        PictureInput(self.slot.clone())
    }
}

fn config(quality: Quality) -> EncoderConfig {
    EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(quality.bitrate()))
        .max_frame_rate(FrameRate::from_hz(quality.fps() as f32))
        .profile(Profile::Baseline)
        .complexity(Complexity::Low)
        .skip_frames(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(
            quality.fps() * KEYFRAME_EVERY.as_secs() as u32,
        ))
}

fn new_encoder(quality: Quality) -> Option<Encoder> {
    match Encoder::with_api_config(OpenH264API::from_source(), config(quality)) {
        Ok(e) => Some(e),
        Err(e) => {
            warn!("video: encoder not available: {e}");
            None
        }
    }
}

/// Encodes one picture. `None` if OpenH264 failed or skipped the frame.
fn encode(encoder: &mut Encoder, picture: &Yuv, keyframe: bool) -> Option<(Vec<u8>, bool)> {
    let (y, u, v) = picture.planes();
    let source = YUVSlices::new(
        (y, u, v),
        (picture.width, picture.height),
        (picture.width, picture.width / 2, picture.width / 2),
    );
    if keyframe {
        encoder.force_intra_frame();
    }
    let stream = encoder.encode(&source).ok()?;
    let kind = stream.frame_type();
    if matches!(kind, FrameType::Skip | FrameType::Invalid) {
        return None;
    }
    let data = stream.to_vec();
    (!data.is_empty()).then_some((data, matches!(kind, FrameType::IDR | FrameType::I)))
}

fn run(slot: Arc<Slot>, outbox: Arc<Outbox>, quality: Quality) {
    let interval = Duration::from_secs(1) / quality.fps().max(1);
    let start = Instant::now();
    let mut encoder: Option<Encoder> = None;
    let mut last: Option<Yuv> = None;
    let mut last_sent = Instant::now();
    let mut last_keyframe: Option<Instant> = None;
    let mut next_at = Instant::now();
    loop {
        // Wait for a new picture, at most until the next repeat is due.
        let fresh = {
            let Ok(mut p) = slot.picture.lock() else { return };
            if p.is_none() && !slot.stop.load(Ordering::Relaxed) {
                let wait = REPEAT_AFTER
                    .saturating_sub(last_sent.elapsed())
                    .max(Duration::from_millis(5));
                p = match slot.wake.wait_timeout(p, wait) {
                    Ok((p, _)) => p,
                    Err(_) => return,
                };
            }
            p.take()
        };
        if slot.stop.load(Ordering::Relaxed) {
            debug!("video: encoder stopped");
            return;
        }
        let repeat = fresh.is_none() && last_sent.elapsed() >= REPEAT_AFTER;
        if let Some(p) = fresh {
            last = Some(p);
        } else if !repeat && !outbox.keyframe.load(Ordering::Relaxed) {
            continue;
        }
        let Some(picture) = last.as_ref() else { continue };
        // At most `fps` frames a second.
        let now = Instant::now();
        if now < next_at {
            std::thread::sleep(next_at - now);
        }
        next_at = Instant::now() + interval;
        if encoder.is_none() {
            encoder = new_encoder(quality);
            last_keyframe = None;
        }
        let Some(enc) = encoder.as_mut() else { return };
        let keyframe =
            outbox.keyframe_requested() || last_keyframe.is_none_or(|at| at.elapsed() >= KEYFRAME_EVERY);
        let result = catch_unwind(AssertUnwindSafe(|| encode(enc, picture, keyframe)));
        let (data, is_key) = match result {
            Ok(Some(out)) => out,
            Ok(None) => continue,
            Err(_) => {
                warn!("video: the encoder crashed, starting a new one");
                encoder = None;
                continue;
            }
        };
        if is_key {
            last_keyframe = Some(Instant::now());
        }
        last_sent = Instant::now();
        let timestamp = (start.elapsed().as_micros() as u64 * RTP_CLOCK / 1_000_000) as u32;
        outbox.push(EncodedFrame {
            data,
            keyframe: is_key,
            timestamp,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize, shift: usize) -> Yuv {
        let mut data = vec![128u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = ((x + y + shift) % 256) as u8;
            }
        }
        Yuv::new(w, h, data).unwrap()
    }

    #[test]
    fn yuv_sizes_are_checked() {
        assert!(Yuv::new(4, 4, vec![0; 24]).is_some());
        assert!(Yuv::new(3, 4, vec![0; 18]).is_none());
        assert!(Yuv::new(4, 4, vec![0; 23]).is_none());
        assert!(Yuv::new(0, 0, vec![]).is_none());
    }

    #[test]
    fn frames_are_encoded_and_decodable() {
        let enc = VideoEncoder::start(Quality::Sd30).unwrap();
        let input = enc.input();
        let mut frames = Vec::new();
        for i in 0..20 {
            input.push(gradient(320, 180, i * 4));
            std::thread::sleep(Duration::from_millis(40));
            frames.extend(enc.outbox.take());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while frames.len() < 5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            frames.extend(enc.outbox.take());
        }
        assert!(frames.len() >= 5, "{} frames", frames.len());
        assert!(frames[0].keyframe, "the first frame is a keyframe");
        assert!(frames[0].data.starts_with(&[0, 0, 0, 1]));
        assert!(frames.windows(2).all(|w| w[1].timestamp >= w[0].timestamp));
        // A keyframe on request.
        enc.outbox.request_keyframe();
        input.push(gradient(320, 180, 99));
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut keyed = false;
        while !keyed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            keyed = enc.outbox.take().iter().any(|f| f.keyframe);
        }
        assert!(keyed);
        // The frames decode with the decoder used for streams.
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut pictures = 0;
        for f in &frames {
            if let Ok(Some(_)) = decoder.decode(&f.data) {
                pictures += 1;
            }
        }
        assert!(pictures >= 3, "{pictures} pictures");
    }

    #[test]
    fn a_still_picture_is_sent_again() {
        let enc = VideoEncoder::start(Quality::Sd30).unwrap();
        enc.input().push(gradient(64, 64, 0));
        std::thread::sleep(Duration::from_millis(2300));
        let frames = enc.outbox.take();
        // The first one, then one repeat a second.
        assert!((2..=4).contains(&frames.len()), "{} frames", frames.len());
    }

    #[test]
    fn a_slow_connection_gets_a_keyframe_after_drops() {
        let outbox = Outbox::new();
        for i in 0..OUTBOX as u32 {
            outbox.push(EncodedFrame {
                data: vec![0, 0, 0, 1, 0x41],
                keyframe: i == 0,
                timestamp: i,
            });
        }
        // Full: a delta frame is dropped with the queue, a keyframe is asked.
        outbox.push(EncodedFrame {
            data: vec![0, 0, 0, 1, 0x41],
            keyframe: false,
            timestamp: 99,
        });
        assert!(outbox.take().is_empty());
        assert!(outbox.keyframe_requested());
        assert_eq!(outbox.dropped.load(Ordering::Relaxed), OUTBOX as u64);
    }
}

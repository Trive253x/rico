//! What we stream with Go Live: a screen or a window, and its sound.
//!
//! Nothing is captured until the user picks a source and starts the stream;
//! stopping the stream (or leaving the call, or closing ferrocord) drops
//! [`Running`], which stops the capture threads and releases the screen and
//! the sound.
//!
//! - Pictures (Windows: Windows Graphics Capture, through the
//!   `windows-capture` crate) are scaled and converted to I420
//!   ([`crate::video::convert`]) on the capture thread, then handed to the
//!   encoder ([`crate::video::encoder`]) through a one-place slot.
//! - Sound (Windows: WASAPI process loopback, through the `wasapi` crate)
//!   is the sound of the streamed window's program, or for a whole screen
//!   every program except ferrocord: the voices of the call never end up in
//!   the stream.
//! - The yellow border Windows draws around a captured item is turned off
//!   where Windows allows it (Windows 11); ferrocord shows "Live" in the
//!   call panel instead.
//! - The test picture ([`SourceKind::TestPattern`]) works everywhere and is
//!   what the demo offers first.
//!
//! Window titles stay in the interface: the log only gets counts.

#[cfg(windows)]
mod windows;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::audio::dsp::{Hum, SampleQueue};
use crate::video::convert::{self, Frame, PixelOrder};
use crate::video::decoder::Sink;
use crate::video::encoder::{Outbox, PictureInput, Quality, VideoEncoder};

/// Sound waiting for the stream connection (1 s of 48 kHz stereo).
pub const SOUND_QUEUE: usize = 2 * 48_000;
/// Width of the preview of what we send.
pub const PREVIEW_WIDTH: usize = 480;
/// How often the preview is refreshed.
const PREVIEW_EVERY: Duration = Duration::from_millis(250);
/// Most windows offered.
const MAX_WINDOWS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Screen,
    Window,
    /// A moving test picture with a soft tone (demo and tests).
    TestPattern,
}

/// Something that can be streamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The system's handle (monitor or window), as a number.
    pub id: u64,
    pub kind: SourceKind,
    pub name: String,
    /// The window's program (its sound is the stream's sound).
    pub process_id: Option<u32>,
    pub primary: bool,
}

impl Source {
    pub fn test_pattern() -> Self {
        Self {
            id: 0,
            kind: SourceKind::TestPattern,
            name: crate::i18n::t("Test picture", "Immagine di prova").into(),
            process_id: None,
            primary: false,
        }
    }
}

/// Screens, then windows (ferrocord's own excluded). Slow-ish (asks the
/// system for every window): call it off the interface thread.
pub fn sources() -> Vec<Source> {
    #[cfg(windows)]
    {
        let list = windows::sources(MAX_WINDOWS);
        tracing::debug!("capture: {} sources", list.len());
        list
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Screen capture works on this system.
pub fn supported() -> bool {
    cfg!(windows)
}

pub struct Options {
    pub quality: Quality,
    /// Stream the sound too.
    pub sound: bool,
    /// Where small copies of what we send go (the preview).
    pub preview: Option<Sink>,
}

/// A running capture. Dropping it stops everything.
pub struct Running {
    // Fields drop in order: the capture stops before the encoder.
    _video: Video,
    _sound: Option<Sound>,
    encoder: VideoEncoder,
    /// Sound for the stream connection (48 kHz interleaved stereo).
    pub sound: Option<SampleQueue>,
    /// The streamed window was closed (or the capture failed).
    pub ended: Arc<AtomicBool>,
    /// The sound could not be captured (the video goes on).
    pub sound_error: Option<String>,
}

impl Running {
    pub fn outbox(&self) -> Arc<Outbox> {
        self.encoder.outbox.clone()
    }

    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }
}

// The values are only kept to be dropped: that stops the capture.
#[allow(dead_code)]
enum Video {
    #[cfg(windows)]
    Windows(windows::VideoCapture),
    Pattern(Stopper),
}

#[allow(dead_code)]
enum Sound {
    #[cfg(windows)]
    Loopback(windows::Loopback),
    Pattern(Stopper),
}

/// Stops a helper thread when dropped.
struct Stopper(Arc<AtomicBool>);

impl Drop for Stopper {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Starts capturing `source`. The video must start; the sound may fail
/// (then `sound_error` says why and the stream goes on without it).
pub fn start(source: &Source, options: Options) -> Result<Running> {
    let encoder = VideoEncoder::start(options.quality)?;
    let ended = Arc::new(AtomicBool::new(false));
    let shaper = Shaper::new(
        encoder.input(),
        encoder.outbox.clone(),
        options.quality,
        options.preview,
    );
    let queue = SampleQueue::new(SOUND_QUEUE);
    let (video, sound, sound_error) = match source.kind {
        SourceKind::TestPattern => {
            let video = Video::Pattern(pattern_video(shaper, options.quality));
            let sound = options
                .sound
                .then(|| Sound::Pattern(pattern_sound(queue.clone())));
            (video, sound, None)
        }
        #[cfg(windows)]
        SourceKind::Screen | SourceKind::Window => {
            let video = Video::Windows(windows::VideoCapture::start(source, shaper, ended.clone())?);
            let (sound, error) = if options.sound {
                match windows::Loopback::start(source, queue.clone()) {
                    Ok(l) => (Some(Sound::Loopback(l)), None),
                    Err(e) => {
                        tracing::warn!("capture: sound not available: {e:#}");
                        (None, Some(format!("{e:#}")))
                    }
                }
            } else {
                (None, None)
            };
            (video, sound, error)
        }
        #[cfg(not(windows))]
        SourceKind::Screen | SourceKind::Window => {
            anyhow::bail!(crate::i18n::t(
                "Screen capture is only available on Windows for now.",
                "Per ora la cattura dello schermo è disponibile solo su Windows.",
            ))
        }
    };
    let has_sound = sound.is_some();
    Ok(Running {
        _video: video,
        _sound: sound,
        encoder,
        sound: has_sound.then_some(queue),
        ended,
        sound_error,
    })
}

/// Turns captured pictures into what the encoder and the preview want, at
/// most `fps` times a second (Windows may deliver more).
pub(crate) struct Shaper {
    input: PictureInput,
    /// For the timings in the log.
    outbox: Arc<Outbox>,
    quality: Quality,
    preview: Option<Sink>,
    last_preview: Option<Instant>,
    last_frame: Option<Instant>,
    min_interval: Duration,
}

impl Shaper {
    fn new(input: PictureInput, outbox: Arc<Outbox>, quality: Quality, preview: Option<Sink>) -> Self {
        Self {
            input,
            outbox,
            quality,
            preview,
            last_preview: None,
            last_frame: None,
            // A little below the frame interval, so timer jitter does not halve the rate.
            min_interval: Duration::from_secs(1) / quality.fps().max(1) * 9 / 10,
        }
    }

    /// Time spent copying a picture from the graphics card (for the log).
    pub(crate) fn add_copy_time(&self, spent: Duration) {
        self.outbox
            .copy_us
            .fetch_add(spent.as_micros() as u64, Ordering::Relaxed);
    }

    /// The minimum time between two pictures we keep.
    pub(crate) fn min_interval(&self) -> Duration {
        self.min_interval
    }

    /// `false` if a picture now would come too soon after the previous one:
    /// the caller can skip it before copying it from the graphics card.
    pub(crate) fn wants_frame(&self) -> bool {
        self.last_frame.is_none_or(|at| at.elapsed() >= self.min_interval)
    }

    /// A captured picture: scaled, converted and handed on (or skipped if
    /// it comes too soon after the previous one).
    pub(crate) fn push(&mut self, frame: &Frame<'_>) {
        if !self.wants_frame() {
            return;
        }
        self.last_frame = Some(Instant::now());
        let started = Instant::now();
        let size = convert::fit(frame.width, frame.height, self.quality.max_size());
        if let Some(yuv) = convert::to_i420(frame, size) {
            self.input.push(yuv);
        }
        self.outbox
            .convert_us
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        self.outbox.converted.fetch_add(1, Ordering::Relaxed);
        if let Some(sink) = self.preview.as_mut() {
            if self.last_preview.is_none_or(|at| at.elapsed() >= PREVIEW_EVERY) {
                self.last_preview = Some(Instant::now());
                if let Some(p) = convert::preview(frame, PREVIEW_WIDTH) {
                    sink(p);
                }
            }
        }
    }
}

/// The test picture: a bar sliding over a gradient, with a frame counter
/// drawn as blocks, so movement and lost frames are easy to see.
fn pattern_video(mut shaper: Shaper, quality: Quality) -> Stopper {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let _ = std::thread::Builder::new()
        .name("capture-pattern".into())
        .spawn(move || {
            let (w, h) = (1280usize, 720usize);
            let interval = Duration::from_secs(1) / quality.fps().max(1);
            let mut bgra = vec![0u8; w * h * 4];
            let mut n = 0u32;
            while !s.load(Ordering::Relaxed) {
                let started = Instant::now();
                draw_pattern(&mut bgra, w, h, n);
                shaper.push(&Frame {
                    data: &bgra,
                    width: w,
                    height: h,
                    pitch: w * 4,
                    order: PixelOrder::Bgra,
                });
                n = n.wrapping_add(1);
                std::thread::sleep(interval.saturating_sub(started.elapsed()));
            }
        });
    Stopper(stop)
}

fn draw_pattern(bgra: &mut [u8], w: usize, h: usize, n: u32) {
    let x0 = (n as usize * 12) % w;
    for (i, px) in bgra.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % w, i / w);
        let bar = x >= x0 && x < x0 + 80;
        // The counter: 16 blocks along the top, lit for the bits of `n`.
        let block = y < 40 && x < 16 * 40 && (n >> (x / 40)) & 1 == 1;
        let (r, g, b) = if block {
            (240, 240, 240)
        } else if bar {
            (222, 165, 96)
        } else {
            ((x * 120 / w) as u8 + 30, (y * 90 / h) as u8 + 40, 110)
        };
        px.copy_from_slice(&[b, g, r, 255]);
    }
}

/// A soft tone in the test stream's sound.
fn pattern_sound(queue: SampleQueue) -> Stopper {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let _ = std::thread::Builder::new()
        .name("capture-pattern-sound".into())
        .spawn(move || {
            let mut hum = Hum::new(330.0);
            let start = Instant::now();
            let mut sent = Duration::ZERO;
            while !s.load(Ordering::Relaxed) {
                // Keep pace with the clock, 20 ms at a time.
                while sent < start.elapsed() {
                    queue.push(&hum.next_frame());
                    sent += Duration::from_millis(20);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    Stopper(stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_pattern_streams_pictures_and_sound() {
        let pictures = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let p = pictures.clone();
        let running = start(
            &Source::test_pattern(),
            Options {
                quality: Quality::Sd30,
                sound: true,
                preview: Some(Box::new(move |pic| {
                    assert!(pic.width as usize <= PREVIEW_WIDTH);
                    p.fetch_add(1, Ordering::Relaxed);
                })),
            },
        )
        .unwrap();
        let outbox = running.outbox();
        // Read like the connection does, a little at a time (a debug build
        // with other tests running can be slow: up to 5 s).
        let sound = running.sound.clone().unwrap();
        let mut frames = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while (frames.len() < 5 || sound.len() < 2 * 960 * 10) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            frames.extend(outbox.take());
        }
        assert!(frames.len() >= 5, "{} frames", frames.len());
        assert!(frames[0].keyframe);
        assert!(sound.len() >= 2 * 960 * 10, "{} samples", sound.len());
        assert!(pictures.load(Ordering::Relaxed) >= 2);
        assert!(!running.has_ended());
        // Stopping really stops: no more frames after a moment.
        drop(running);
        std::thread::sleep(Duration::from_millis(200));
        let _ = outbox.take();
        std::thread::sleep(Duration::from_millis(300));
        assert!(outbox.take().is_empty());
    }

    #[test]
    fn without_sound_there_is_no_queue() {
        let running = start(
            &Source::test_pattern(),
            Options {
                quality: Quality::Sd30,
                sound: false,
                preview: None,
            },
        )
        .unwrap();
        assert!(running.sound.is_none() && running.sound_error.is_none());
    }
}

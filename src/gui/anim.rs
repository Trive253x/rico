//! Widget that shows an animated GIF (or WebP).
//!
//! Two costs are kept down here, without the rest of the GUI having to know:
//!
//! - **Memory.** An animation downloaded from the network keeps the original
//!   file (compressed, a few MB at most) and its first frame. All frames are
//!   decoded on a separate thread only when it appears on screen, and dropped
//!   again [`SLEEP_AFTER`] after it left the screen. A channel full of GIFs
//!   costs the files plus the frames of the few visible ones, instead of every
//!   frame of every GIF ever loaded.
//! - **CPU.** The widget does not rebuild the interface to animate: it asks
//!   iced to redraw at the time of the next frame, only while it is visible.
//!   Each redraw draws the whole window, so the times are aligned on a common
//!   [`FRAME_GRID`]: several visible GIFs share the same redraws and the
//!   window is redrawn at most ~30 times a second. Playback speed stays right
//!   (a frame is skipped when its time has passed).

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use iced::advanced::image as core_image;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{renderer, Clipboard, Shell, Widget};
use iced::widget::image::{self as img, FilterMethod};
use iced::{event, mouse, window, ContentFit, Element, Event, Length, Rectangle, Rotation, Size};

use crate::media::{self, RawFrame};

/// Frames dropped this long after the animation left the screen.
pub const SLEEP_AFTER: Duration = Duration::from_secs(15);
/// Redraw times are rounded up to a multiple of this (≈ 30 per second).
pub const FRAME_GRID: Duration = Duration::from_millis(33);
/// Animations decoded at the same time (each on its own thread).
const MAX_DECODING: usize = 2;
/// While the frames are being decoded the widget checks again this often.
const POLL: Duration = Duration::from_millis(50);

type Frames = Arc<Vec<(img::Handle, Duration)>>;

enum State {
    /// Only the first frame (the frames can be decoded from the source).
    Asleep,
    Decoding,
    Awake(Frames),
    /// Decoding failed: the first frame stays.
    Failed,
}

struct Inner {
    id: u64,
    first: img::Handle,
    /// Original file, to decode the frames again; `None` for animations made
    /// in memory (demo), which are always awake.
    source: Option<Arc<[u8]>>,
    /// Size the frames are scaled down to.
    max: (u32, u32),
    state: Mutex<State>,
    /// When it was last on screen (milliseconds from `epoch()`).
    last_seen: AtomicU64,
}

/// An animation ready to draw. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Animation(Arc<Inner>);

impl fmt::Debug for Animation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Animation({})", self.0.id)
    }
}

fn next_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn millis(at: Instant) -> u64 {
    at.saturating_duration_since(epoch()).as_millis() as u64
}

/// All animations, to put to sleep those no longer on screen.
fn registry() -> &'static Mutex<Vec<Weak<Inner>>> {
    static REGISTRY: OnceLock<Mutex<Vec<Weak<Inner>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn handles(raw: Vec<RawFrame>) -> Vec<(img::Handle, Duration)> {
    raw.into_iter()
        .map(|f| (img::Handle::from_rgba(f.width, f.height, f.rgba), f.delay))
        .collect()
}

impl Animation {
    /// From frames already decoded (demo): always awake. `None` without frames.
    pub fn new(raw: Vec<RawFrame>) -> Option<Self> {
        let frames = handles(raw);
        let first = frames.first()?.0.clone();
        Some(Self::register(Inner {
            id: next_id(),
            first,
            source: None,
            max: (0, 0),
            state: Mutex::new(State::Awake(Arc::new(frames))),
            last_seen: AtomicU64::new(0),
        }))
    }

    /// From a downloaded file: only the first frame is decoded now (off the
    /// interface thread: call it from a task). `None` if it is not animated.
    pub fn from_source(bytes: &[u8], max_w: u32, max_h: u32) -> anyhow::Result<Option<Self>> {
        let Some(first) = media::first_frame(bytes, max_w, max_h)? else {
            return Ok(None);
        };
        Ok(Some(Self::register(Inner {
            id: next_id(),
            first: img::Handle::from_rgba(first.width, first.height, first.rgba),
            source: Some(bytes.into()),
            max: (max_w, max_h),
            state: Mutex::new(State::Asleep),
            last_seen: AtomicU64::new(0),
        })))
    }

    fn register(inner: Inner) -> Self {
        let inner = Arc::new(inner);
        if inner.source.is_some() {
            if let Ok(mut all) = registry().lock() {
                all.push(Arc::downgrade(&inner));
            }
        }
        Self(inner)
    }

    /// Size of the original file (bytes), for the log.
    pub fn source_len(&self) -> usize {
        self.0.source.as_ref().map_or(0, |s| s.len())
    }

    /// Frames currently in memory (0 while asleep).
    pub fn frames_in_memory(&self) -> usize {
        match &*self.0.state.lock().unwrap_or_else(|e| e.into_inner()) {
            State::Awake(f) => f.len(),
            _ => 0,
        }
    }

    fn frames(&self) -> Option<Frames> {
        match &*self.0.state.lock().unwrap_or_else(|e| e.into_inner()) {
            State::Awake(f) => Some(f.clone()),
            _ => None,
        }
    }

    /// On screen now: it stays awake; if asleep, decoding starts (at most
    /// `MAX_DECODING` at a time). Returns `true` while frames are on the way.
    fn seen(&self, now: Instant) -> bool {
        self.0.last_seen.store(millis(now), Ordering::Relaxed);
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            State::Decoding => true,
            State::Asleep => {
                if start_decoding(&self.0) {
                    *state = State::Decoding;
                }
                true
            }
            State::Awake(_) | State::Failed => false,
        }
    }
}

static DECODING: AtomicUsize = AtomicUsize::new(0);

/// Decodes all the frames on a separate thread; `false` if too many are
/// already running (it will be tried again at the next check).
fn start_decoding(inner: &Arc<Inner>) -> bool {
    let Some(source) = inner.source.clone() else {
        return false;
    };
    if DECODING.fetch_add(1, Ordering::AcqRel) >= MAX_DECODING {
        DECODING.fetch_sub(1, Ordering::AcqRel);
        return false;
    }
    let weak = Arc::downgrade(inner);
    let (w, h) = inner.max;
    let spawned = std::thread::Builder::new()
        .name("gif-decode".into())
        .spawn(move || {
            let result = media::decode_animation(&source, w, h);
            DECODING.fetch_sub(1, Ordering::AcqRel);
            let Some(inner) = weak.upgrade() else { return };
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            *state = match result {
                Ok(Some(frames)) => {
                    let total: Duration = frames.iter().map(|f| f.delay).sum();
                    tracing::debug!(
                        "animation decoded: {} frames of {}×{}, {} KB, {} ms per frame",
                        frames.len(),
                        frames[0].width,
                        frames[0].height,
                        frames.iter().map(|f| f.rgba.len()).sum::<usize>() / 1024,
                        total.as_millis() / frames.len().max(1) as u128
                    );
                    State::Awake(Arc::new(handles(frames)))
                }
                Ok(None) => State::Failed,
                Err(e) => {
                    tracing::debug!("animation not decoded: {e:#}");
                    State::Failed
                }
            };
        });
    if spawned.is_err() {
        DECODING.fetch_sub(1, Ordering::AcqRel);
        return false;
    }
    true
}

/// Drops the frames of the animations not on screen for [`SLEEP_AFTER`].
/// Cheap and safe to call often: it runs at most once a second.
pub fn sweep() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = millis(Instant::now());
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 1000 && last != 0 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let dropped = put_to_sleep(now);
    if dropped > 0 {
        tracing::debug!("animations: {dropped} put to sleep (off screen)");
    }
}

/// Drops the frames of the animations not seen for [`SLEEP_AFTER`] at
/// `now` (milliseconds from `epoch()`); returns how many.
fn put_to_sleep(now: u64) -> usize {
    let Ok(mut all) = registry().lock() else { return 0 };
    let limit = SLEEP_AFTER.as_millis() as u64;
    let mut dropped = 0;
    all.retain(|weak| {
        let Some(inner) = weak.upgrade() else {
            return false;
        };
        if now.saturating_sub(inner.last_seen.load(Ordering::Relaxed)) > limit {
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*state, State::Awake(_)) {
                *state = State::Asleep;
                dropped += 1;
            }
        }
        true
    });
    dropped
}

/// Rounds a time up to the common grid, so that the animations on screen
/// share the same redraws.
fn on_grid(at: Instant) -> Instant {
    let since = at.saturating_duration_since(epoch()).as_nanos();
    let step = FRAME_GRID.as_nanos();
    let steps = since.div_ceil(step);
    epoch() + Duration::from_nanos((steps * step) as u64)
}

/// Shows an [`Animation`] fitted to the given size.
pub struct AnimatedImage<'a> {
    animation: &'a Animation,
    width: Length,
    height: Length,
}

pub fn animated(animation: &Animation) -> AnimatedImage<'_> {
    AnimatedImage {
        animation,
        width: Length::Shrink,
        height: Length::Shrink,
    }
}

impl AnimatedImage<'_> {
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

struct Playback {
    animation_id: u64,
    index: usize,
    /// When the current frame ends.
    next_at: Option<Instant>,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for AnimatedImage<'_>
where
    Renderer: core_image::Renderer<Handle = img::Handle>,
{
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Playback>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Playback {
            animation_id: self.animation.0.id,
            index: 0,
            next_at: None,
        })
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<Playback>();
        // The same place in the interface can receive another GIF.
        if state.animation_id != self.animation.0.id {
            *state = Playback {
                animation_id: self.animation.0.id,
                index: 0,
                next_at: None,
            };
        }
    }

    fn layout(&self, _tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        img::layout(
            renderer,
            limits,
            &self.animation.0.first,
            self.width,
            self.height,
            ContentFit::Contain,
            Rotation::default(),
        )
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return event::Status::Ignored;
        };
        sweep();
        // Off screen it does not animate (no useless redraws) and, after a
        // while, its frames are dropped.
        if !layout.bounds().intersects(viewport) {
            return event::Status::Ignored;
        }
        if self.animation.seen(now) {
            // Frames on the way: check again shortly.
            shell.request_redraw(window::RedrawRequest::At(now + POLL));
            return event::Status::Ignored;
        }
        let Some(frames) = self.animation.frames() else {
            return event::Status::Ignored;
        };
        let state = tree.state.downcast_mut::<Playback>();
        if state.index >= frames.len() {
            state.index = 0;
        }
        let mut next = *state.next_at.get_or_insert(now + frames[state.index].1);
        // Frames whose time has passed are skipped: the speed stays right
        // even if the redraws come later than asked (grid, busy window).
        let mut skipped = 0;
        while now >= next && skipped < frames.len() {
            state.index = (state.index + 1) % frames.len();
            next += frames[state.index].1;
            skipped += 1;
        }
        if now >= next {
            // Far behind (e.g. it was off screen): start again from here.
            next = now + frames[state.index].1;
        }
        state.next_at = Some(next);
        shell.request_redraw(window::RedrawRequest::At(on_grid(next)));
        event::Status::Ignored
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<Playback>();
        let frames = self.animation.frames();
        let handle = match &frames {
            Some(f) if !f.is_empty() => &f[state.index.min(f.len() - 1)].0,
            _ => &self.animation.0.first,
        };
        img::draw(
            renderer,
            layout,
            handle,
            ContentFit::Contain,
            FilterMethod::Linear,
            Rotation::default(),
            1.0,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<AnimatedImage<'a>> for Element<'a, Message, Theme, Renderer>
where
    Renderer: core_image::Renderer<Handle = img::Handle> + 'a,
{
    fn from(widget: AnimatedImage<'a>) -> Self {
        Element::new(widget)
    }
}

/// Fake GIF for the demo: a circle moving on a background that changes colour.
pub fn demo_animation(width: u32, height: u32) -> Option<Animation> {
    let frames = (0..12)
        .map(|i| {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            let cx = (width as f32) * (0.2 + 0.6 * (i as f32 / 11.0));
            let cy = height as f32 / 2.0;
            let r = height as f32 / 4.0;
            for y in 0..height {
                for x in 0..width {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    if dx * dx + dy * dy < r * r {
                        rgba.extend_from_slice(&[222, 165, 96, 255]);
                    } else {
                        rgba.extend_from_slice(&[(30 + i * 10) as u8, 60, 110, 255]);
                    }
                }
            }
            RawFrame {
                width,
                height,
                rgba,
                delay: Duration::from_millis(80),
            }
        })
        .collect();
    Animation::new(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(frames: u32) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = RgbaImage::from_pixel(8, 8, Rgba([(i * 40) as u8, 100, 200, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(50, 1)))
                    .unwrap();
            }
        }
        out
    }

    #[test]
    fn asleep_until_seen_then_awake_then_asleep_again() {
        let a = Animation::from_source(&gif(4), 100, 100).unwrap().unwrap();
        // Downloaded: only the first frame in memory.
        assert_eq!(a.frames_in_memory(), 0);
        assert!(a.source_len() > 0);
        // On screen: the frames are decoded on another thread.
        let now = Instant::now();
        let mut tries = 0;
        while a.seen(now) {
            std::thread::sleep(Duration::from_millis(10));
            tries += 1;
            assert!(tries < 500, "frames never arrived");
        }
        assert_eq!(a.frames_in_memory(), 4);
        // Seen recently: it stays awake.
        let seen = a.0.last_seen.load(Ordering::Relaxed);
        put_to_sleep(seen + 1000);
        assert_eq!(a.frames_in_memory(), 4);
        // Off screen for long enough: back to sleep, the first frame stays.
        put_to_sleep(seen + SLEEP_AFTER.as_millis() as u64 + 1);
        assert_eq!(a.frames_in_memory(), 0);
        // A still image is not an animation.
        assert!(Animation::from_source(&gif(1), 100, 100).unwrap().is_none());
    }

    #[test]
    fn redraws_on_a_common_grid() {
        let a = on_grid(Instant::now() + Duration::from_millis(5));
        let b = on_grid(Instant::now() + Duration::from_millis(7));
        let grid = FRAME_GRID.as_nanos();
        assert_eq!(a.duration_since(epoch()).as_nanos() % grid, 0);
        assert!(b >= a);
    }
}

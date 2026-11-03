//! Widget che mostra una GIF animata.
//!
//! I fotogrammi sono già decodificati (vedi `media::decode_animation`). Il
//! widget non ricostruisce l'interfaccia per animarsi: chiede a iced di
//! ridisegnare la finestra all'istante del fotogramma successivo, e lo fa solo
//! finché è visibile (fuori dallo schermo l'animazione si ferma e riparte
//! quando torna in vista).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::advanced::image as core_image;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{renderer, Clipboard, Shell, Widget};
use iced::widget::image::{self as img, FilterMethod};
use iced::{event, mouse, window, ContentFit, Element, Event, Length, Rectangle, Rotation, Size};

use crate::media::RawFrame;

/// Fotogrammi di una GIF pronti da disegnare. Economico da clonare.
#[derive(Clone)]
pub struct Animation {
    id: u64,
    frames: Arc<Vec<(img::Handle, Duration)>>,
}

impl fmt::Debug for Animation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Animation({} fotogrammi)", self.frames.len())
    }
}

impl Animation {
    /// `None` se non ci sono fotogrammi.
    pub fn new(raw: Vec<RawFrame>) -> Option<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let frames: Vec<_> = raw
            .into_iter()
            .map(|f| (img::Handle::from_rgba(f.width, f.height, f.rgba), f.delay))
            .collect();
        (!frames.is_empty()).then(|| Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            frames: Arc::new(frames),
        })
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }
}

/// Mostra un'[`Animation`] adattata alle dimensioni date.
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

struct State {
    animation_id: u64,
    index: usize,
    /// Quando passare al fotogramma successivo.
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
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            animation_id: self.animation.id,
            index: 0,
            next_at: None,
        })
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        // Lo stesso posto nell'interfaccia può ricevere un'altra GIF.
        if state.animation_id != self.animation.id {
            *state = State {
                animation_id: self.animation.id,
                index: 0,
                next_at: None,
            };
        }
    }

    fn layout(&self, _tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        img::layout(
            renderer,
            limits,
            &self.animation.frames[0].0,
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
        // Fuori dallo schermo non si anima (niente ridisegni inutili).
        if !layout.bounds().intersects(viewport) {
            return event::Status::Ignored;
        }
        let state = tree.state.downcast_mut::<State>();
        let frames = &self.animation.frames;
        let next = *state.next_at.get_or_insert(now + frames[state.index].1);
        if now >= next {
            state.index = (state.index + 1) % frames.len();
            state.next_at = Some(now + frames[state.index].1);
        }
        if let Some(at) = state.next_at {
            shell.request_redraw(window::RedrawRequest::At(at));
        }
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
        let state = tree.state.downcast_ref::<State>();
        let index = state.index.min(self.animation.frames.len() - 1);
        img::draw(
            renderer,
            layout,
            &self.animation.frames[index].0,
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

/// GIF finta per la demo: un cerchio che si sposta su uno sfondo che cambia colore.
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

//! Going to a message of the open channel (the one a reply answers): finds
//! where it is in the list of messages and gives the scroll offset that
//! shows it.
//!
//! iced has no "scroll to this widget": an operation walks the widget tree,
//! notes the bounds of the list and of the message's container, and works
//! out the offset. Only positions are read, nothing else of the interface.

use iced::advanced::widget::operation::{Operation, Outcome, Scrollable};
use iced::advanced::widget::{self, Id};
use iced::{Rectangle, Task, Vector};

/// The id of the container of a message in the list.
pub fn container_id(message_id: &str) -> Id {
    Id::from(format!("message-{message_id}"))
}

/// The offset of `list` (a scrollable anchored at the bottom) that shows
/// the message near its top; nothing if the message is not in the list.
pub fn offset_of(list: Id, message_id: &str) -> Task<f32> {
    widget::operate(Find {
        list,
        target: container_id(message_id),
        viewport: None,
        found: None,
    })
}

struct Find {
    list: Id,
    target: Id,
    /// Bounds of the list and of all its content.
    viewport: Option<(Rectangle, Rectangle)>,
    found: Option<Rectangle>,
}

impl Operation<f32> for Find {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<f32>)) {
        // Nothing more to look at once the message is found.
        if self.found.is_none() {
            operate(self);
        }
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        if self.found.is_none() && id == Some(&self.target) {
            self.found = Some(bounds);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        _translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.list) {
            self.viewport = Some((bounds, content_bounds));
        }
    }

    fn finish(&self) -> Outcome<f32> {
        match (self.viewport, self.found) {
            (Some((bounds, content)), Some(message)) => {
                Outcome::Some(bottom_offset(bounds, content, message))
            }
            _ => Outcome::None,
        }
    }
}

/// Offset from the bottom (the list is anchored there) that puts `message`
/// a little below the top of the list, within what can be scrolled.
fn bottom_offset(bounds: Rectangle, content: Rectangle, message: Rectangle) -> f32 {
    let scrollable = (content.height - bounds.height).max(0.0);
    let from_top = (message.y - content.y - bounds.height / 4.0).clamp(0.0, scrollable);
    scrollable - from_top
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(y: f32, height: f32) -> Rectangle {
        Rectangle {
            x: 0.0,
            y,
            width: 100.0,
            height,
        }
    }

    #[test]
    fn offsets_from_the_bottom() {
        // A 400 px list over 2000 px of messages, starting at y = 100.
        let (bounds, content) = (rect(100.0, 400.0), rect(100.0, 2000.0));
        // A message at the very top: the most that can be scrolled.
        assert_eq!(bottom_offset(bounds, content, rect(100.0, 40.0)), 1600.0);
        // In the middle: shown a quarter below the top of the list.
        assert_eq!(bottom_offset(bounds, content, rect(1100.0, 40.0)), 700.0);
        // At the bottom: no scroll.
        assert_eq!(bottom_offset(bounds, content, rect(2060.0, 40.0)), 0.0);
        // Content shorter than the list: nothing to scroll.
        assert_eq!(bottom_offset(bounds, rect(100.0, 300.0), rect(200.0, 40.0)), 0.0);
    }
}

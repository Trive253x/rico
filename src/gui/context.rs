//! The right-click menu of a message.
//!
//! iced has no context menu: [`RightClick`] wraps a message and, on a right
//! click over it, publishes where the cursor is *in the window*; the menu is
//! then drawn as a layer of the whole window at that point (moved inside the
//! window if it would go out of it).
//!
//! The message list scrolls, and inside a scrollable the cursor a widget sees
//! is moved by the scroll. The `translation` iced passes to `overlay()` (called
//! on every event before `update`) undoes exactly that, so it is kept and
//! added back. Only positions are read.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{overlay, renderer, Clipboard, Shell, Widget};
use iced::alignment::Vertical;
use iced::widget::{button, column, container, mouse_area, responsive, row, rule, text, Space};
use iced::{mouse, Element, Event, Length, Padding, Point, Rectangle, Size, Vector};

use super::style::{self, pal};
use super::Message;
use crate::i18n::t;

/// Wraps a message: a right click publishes `on_right_press(position)`.
pub struct RightClick<'a, M> {
    content: Element<'a, M>,
    on_right_press: Box<dyn Fn(Point) -> M + 'a>,
}

impl<'a, M> RightClick<'a, M> {
    pub fn new(content: impl Into<Element<'a, M>>, on_right_press: impl Fn(Point) -> M + 'a) -> Self {
        Self {
            content: content.into(),
            on_right_press: Box::new(on_right_press),
        }
    }
}

/// The translation of the last `overlay()` call: content to window.
#[derive(Default)]
struct State {
    translation: Vector,
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for RightClick<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        if shell.is_event_captured() {
            return;
        }
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            if let Some(p) = cursor.position_over(layout.bounds()) {
                let t = tree.state.downcast_ref::<State>().translation;
                shell.publish((self.on_right_press)(p + t));
                shell.capture_event();
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        tree.state.downcast_mut::<State>().translation = translation;
        let (first, _) = tree.children.split_at_mut(1);
        self.content
            .as_widget_mut()
            .overlay(&mut first[0], layout, renderer, viewport, translation)
    }
}

impl<'a, M: 'a> From<RightClick<'a, M>> for Element<'a, M> {
    fn from(r: RightClick<'a, M>) -> Self {
        Element::new(r)
    }
}

/// The open menu: which message, where, and what it allows.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub message_id: String,
    pub at: Point,
    pub copy_text: bool,
    pub react: bool,
    pub reply: bool,
    pub edit: bool,
    pub delete: bool,
}

/// What the menu can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CopyText,
    CopyLink,
    CopyId,
    Reply,
    React,
    Edit,
    Delete,
}

const WIDTH: f32 = 200.0;
const ITEM: f32 = 30.0;

impl Menu {
    fn items(&self) -> Vec<(Action, &'static str, bool)> {
        let mut v = Vec::new();
        if self.react {
            v.push((Action::React, t("Add reaction", "Aggiungi reazione"), false));
        }
        if self.reply {
            v.push((Action::Reply, t("Reply", "Rispondi"), false));
        }
        if self.edit {
            v.push((Action::Edit, t("Edit", "Modifica"), false));
        }
        if self.copy_text {
            v.push((Action::CopyText, t("Copy text", "Copia testo"), false));
        }
        v.push((
            Action::CopyLink,
            t("Copy message link", "Copia link del messaggio"),
            false,
        ));
        v.push((
            Action::CopyId,
            t("Copy message ID", "Copia ID del messaggio"),
            false,
        ));
        if self.delete {
            v.push((Action::Delete, t("Delete message", "Elimina messaggio"), true));
        }
        v
    }

    /// Top-left corner of the menu in a window of `size`: at the cursor,
    /// moved left or up when it would come out of the window.
    pub fn corner(&self, size: Size, rows: usize) -> Point {
        let height = rows as f32 * ITEM + 16.0;
        let x = if self.at.x + WIDTH > size.width {
            (self.at.x - WIDTH).max(0.0)
        } else {
            self.at.x
        };
        let y = if self.at.y + height > size.height {
            (size.height - height).max(0.0)
        } else {
            self.at.y
        };
        Point::new(x, y)
    }
}

/// The menu as a layer of the window; a click elsewhere closes it.
pub fn view(menu: &Menu) -> Element<'_, Message> {
    let menu = menu.clone();
    iced::widget::stack![
        mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(Message::CloseMessageMenu)
            .on_right_press(Message::CloseMessageMenu),
        responsive(move |size| {
            let items = menu.items();
            let rows = items.len() + usize::from(menu.delete);
            let corner = menu.corner(size, rows);
            let mut list = column![].spacing(1);
            let mut split = false;
            for (action, label, danger) in items {
                if danger && !split {
                    list = list.push(container(rule::horizontal(1)).padding([3, 4]));
                    split = true;
                }
                list = list.push(
                    button(
                        row![text(label)
                            .size(13)
                            .color(if danger { pal::red() } else { pal::text() })]
                        .align_y(Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([6, 10])
                    .style(style::list_item(false))
                    .on_press(Message::MessageAction(action)),
                );
            }
            let card = container(list).width(WIDTH).padding(4).style(style::card);
            container(iced::widget::opaque(card))
                .padding(Padding {
                    top: corner.y,
                    left: corner.x,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into()
        }),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(at: Point) -> Menu {
        Menu {
            message_id: "1".into(),
            at,
            copy_text: true,
            react: true,
            reply: true,
            edit: false,
            delete: true,
        }
    }

    #[test]
    fn the_menu_stays_in_the_window() {
        let size = Size::new(1000.0, 700.0);
        assert_eq!(
            menu(Point::new(100.0, 100.0)).corner(size, 6),
            Point::new(100.0, 100.0)
        );
        // Near the right edge: opens to the left of the cursor.
        assert_eq!(menu(Point::new(950.0, 100.0)).corner(size, 6).x, 750.0);
        // Near the bottom: moved up to fit.
        let c = menu(Point::new(100.0, 690.0)).corner(size, 6);
        assert!(c.y + 6.0 * ITEM + 16.0 <= 700.0);
    }

    #[test]
    fn only_what_is_allowed() {
        let m = menu(Point::ORIGIN);
        let actions: Vec<Action> = m.items().into_iter().map(|(a, _, _)| a).collect();
        assert!(!actions.contains(&Action::Edit));
        assert_eq!(actions.last(), Some(&Action::Delete));
        let m = Menu {
            copy_text: false,
            react: false,
            reply: false,
            delete: false,
            ..m
        };
        let actions: Vec<Action> = m.items().into_iter().map(|(a, _, _)| a).collect();
        assert_eq!(actions, vec![Action::CopyLink, Action::CopyId]);
    }
}

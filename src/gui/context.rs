//! Right-click menus: of a message, and of a person (author, member list,
//! voice channel).
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
}

/// Top-left corner of a menu opened at `at` in a window of `size`.
pub fn corner(at: Point, size: Size, rows: usize) -> Point {
    let height = rows as f32 * ITEM + 16.0;
    let x = if at.x + WIDTH > size.width {
        (at.x - WIDTH).max(0.0)
    } else {
        at.x
    };
    let y = if at.y + height > size.height {
        (size.height - height).max(0.0)
    } else {
        at.y
    };
    Point::new(x, y)
}

/// One line of a menu: a separator, or an action.
#[derive(Debug, Clone)]
enum Line {
    Split,
    Item {
        label: &'static str,
        danger: bool,
        on_press: Message,
    },
}

/// The message menu as a layer of the window; a click elsewhere closes it.
pub fn view(menu: &Menu) -> Element<'_, Message> {
    let mut lines = Vec::new();
    let mut split = false;
    for (action, label, danger) in menu.items() {
        if danger && !split {
            lines.push(Line::Split);
            split = true;
        }
        lines.push(Line::Item {
            label,
            danger,
            on_press: Message::MessageAction(action),
        });
    }
    layer(menu.at, lines, Message::CloseMessageMenu)
}

/// A menu at `at`, as a layer of the whole window.
fn layer<'a>(at: Point, lines: Vec<Line>, close: Message) -> Element<'a, Message> {
    iced::widget::stack![
        mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(close.clone())
            .on_right_press(close),
        responsive(move |size| {
            let corner = corner(at, size, lines.len());
            let mut list = column![].spacing(1);
            for line in &lines {
                list = match line {
                    Line::Split => list.push(container(rule::horizontal(1)).padding([3, 4])),
                    Line::Item {
                        label,
                        danger,
                        on_press,
                    } => list.push(
                        button(
                            row![text(*label)
                                .size(13)
                                .color(if *danger { pal::red() } else { pal::text() })]
                            .align_y(Vertical::Center),
                        )
                        .width(Length::Fill)
                        .padding([6, 10])
                        .style(style::list_item(false))
                        .on_press(on_press.clone()),
                    ),
                };
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

/// The open menu of a person: who, where they were clicked, and what is
/// allowed (decided when it opens, checked again when an action runs).
#[derive(Debug, Clone, PartialEq)]
pub struct UserMenu {
    pub user_id: String,
    /// The server it was opened in (`None` in direct messages).
    pub guild_id: Option<String>,
    pub at: Point,
    pub actions: Vec<UserAction>,
}

/// What the menu of a person can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    Profile,
    Mention,
    /// Opens the direct messages with them (made if there are none yet).
    Message,
    /// Mute or unmute them in calls, only for us.
    LocalMute(bool),
    /// Server mute (`true`) or unmute in voice.
    ServerMute(bool),
    ServerDeafen(bool),
    /// Out of the voice channel.
    Disconnect,
    /// Opens the user card on the timeout choices.
    Timeout,
    Kick,
    Ban,
    CopyName,
    CopyId,
}

impl UserAction {
    fn label(self) -> &'static str {
        match self {
            Self::Profile => t("Profile", "Profilo"),
            Self::Mention => t("Mention", "Menziona"),
            Self::Message => t("Message", "Messaggio"),
            Self::LocalMute(true) => t("Mute (only for you)", "Silenzia (solo per te)"),
            Self::LocalMute(false) => t("Unmute (only for you)", "Riattiva (solo per te)"),
            Self::ServerMute(true) => t("Server mute", "Silenzia nel server"),
            Self::ServerMute(false) => t("Server unmute", "Riattiva nel server"),
            Self::ServerDeafen(true) => t("Server deafen", "Disattiva l'audio nel server"),
            Self::ServerDeafen(false) => t("Server undeafen", "Riattiva l'audio nel server"),
            Self::Disconnect => t("Disconnect from voice", "Disconnetti dalla voce"),
            Self::Timeout => t("Timeout…", "Timeout…"),
            Self::Kick => t("Kick…", "Espelli…"),
            Self::Ban => t("Ban…", "Banna…"),
            Self::CopyName => t("Copy username", "Copia nome utente"),
            Self::CopyId => t("Copy user ID", "Copia ID utente"),
        }
    }

    /// Removes someone or takes something away from them on the server.
    fn danger(self) -> bool {
        matches!(
            self,
            Self::Disconnect
                | Self::Timeout
                | Self::Kick
                | Self::Ban
                | Self::ServerMute(true)
                | Self::ServerDeafen(true)
        )
    }

    /// The group it is shown in: the groups are split by a line.
    fn group(self) -> u8 {
        match self {
            Self::Profile | Self::Mention | Self::Message => 0,
            Self::LocalMute(_) => 1,
            Self::ServerMute(_) | Self::ServerDeafen(_) | Self::Disconnect => 2,
            Self::Timeout | Self::Kick | Self::Ban => 3,
            Self::CopyName | Self::CopyId => 4,
        }
    }
}

/// The menu of a person as a layer of the window.
pub fn user_view(menu: &UserMenu) -> Element<'_, Message> {
    let mut lines = Vec::new();
    let mut group = None;
    for &action in &menu.actions {
        if group.is_some_and(|g| g != action.group()) {
            lines.push(Line::Split);
        }
        group = Some(action.group());
        lines.push(Line::Item {
            label: action.label(),
            danger: action.danger(),
            on_press: Message::UserAction(action),
        });
    }
    layer(menu.at, lines, Message::CloseUserMenu)
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
            corner(Point::new(100.0, 100.0), size, 6),
            Point::new(100.0, 100.0)
        );
        // Near the right edge: opens to the left of the cursor.
        assert_eq!(corner(Point::new(950.0, 100.0), size, 6).x, 750.0);
        // Near the bottom: moved up to fit.
        let c = corner(Point::new(100.0, 690.0), size, 6);
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

//! Right-click menus: of a message, of a person (author, member list,
//! voice channel), and of a server icon.
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
use iced::widget::{
    button, column, container, mouse_area, responsive, row, rule, slider, space, text, Space,
};
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

/// One line of a menu: a separator, an action, or someone's volume.
#[derive(Debug, Clone)]
enum Line {
    Split,
    Item {
        label: &'static str,
        danger: bool,
        on_press: Box<Message>,
    },
    /// Their volume in calls, only for us: a slider (it takes two rows).
    Volume {
        user_id: String,
        volume: f32,
        muted: bool,
    },
}

impl Line {
    /// Rows it takes, to place the menu inside the window.
    fn rows(&self) -> usize {
        match self {
            Self::Volume { .. } => 2,
            _ => 1,
        }
    }
}

/// Someone's volume in calls, only for us: name of the setting, percentage
/// and a slider (0–200%, steps of 5%; saved when it is let go). Used by the
/// person's menu and by the people next to a stream.
pub fn volume_slider<'a>(user_id: &str, volume: f32, muted: bool, label: bool) -> Element<'a, Message> {
    let id = user_id.to_string();
    let percent = text(if muted {
        t("muted", "muto").to_string()
    } else {
        format!("{}%", (volume * 100.0).round() as u32)
    })
    .size(11)
    .color(if volume > 1.0 && !muted {
        pal::yellow()
    } else {
        pal::muted()
    });
    let level = slider(
        0.0..=crate::audio::MAX_GAIN * 100.0,
        volume * 100.0,
        move |percent| Message::UserVolume {
            user_id: id.clone(),
            percent,
        },
    )
    .step(5.0_f32)
    .on_release(Message::SaveSettings);
    if label {
        column![
            row![
                text(t("User volume", "Volume utente"))
                    .size(12)
                    .color(pal::muted()),
                space::horizontal(),
                percent,
            ]
            .align_y(Vertical::Center),
            level,
        ]
        .spacing(4)
        .into()
    } else {
        row![level, container(percent).width(36)]
            .spacing(6)
            .align_y(Vertical::Center)
            .into()
    }
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
            on_press: Box::new(Message::MessageAction(action)),
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
            let corner = corner(at, size, lines.iter().map(Line::rows).sum());
            let mut list = column![].spacing(1);
            for line in &lines {
                list = match line {
                    Line::Split => list.push(container(rule::horizontal(1)).padding([3, 4])),
                    Line::Volume {
                        user_id,
                        volume,
                        muted,
                    } => list.push(container(volume_slider(user_id, *volume, *muted, true)).padding([6, 10])),
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
                        .on_press((**on_press).clone()),
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
    /// Their volume can be set here (not for ourselves).
    pub volume: bool,
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
    /// The code that verifies their end-to-end encryption key in our call.
    Verify,
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
            Self::Verify => t(
                "Verify end-to-end encryption",
                "Verifica la crittografia end-to-end",
            ),
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
            Self::LocalMute(_) | Self::Verify => 1,
            Self::ServerMute(_) | Self::ServerDeafen(_) | Self::Disconnect => 2,
            Self::Timeout | Self::Kick | Self::Ban => 3,
            Self::CopyName | Self::CopyId => 4,
        }
    }
}

/// The open menu of a server icon.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerMenu {
    pub guild_id: String,
    pub name: String,
    pub at: Point,
    pub actions: Vec<ServerAction>,
    /// "Leave server" was chosen: the confirmation shows instead.
    pub confirm_leave: bool,
}

/// What the menu of a server can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAction {
    /// Everything in it read (here and on the other devices).
    MarkRead,
    /// Mute (`true`) or unmute the server for us: no notifications, no
    /// unread marks.
    Mute(bool),
    /// The folder editor.
    Folders,
    CopyId,
    /// Asks first.
    Leave,
}

impl ServerAction {
    fn label(self) -> &'static str {
        match self {
            Self::MarkRead => t("Mark as read", "Segna come già letto"),
            Self::Mute(true) => t("Mute server", "Silenzia server"),
            Self::Mute(false) => t("Unmute server", "Riattiva notifiche del server"),
            Self::Folders => t("Server folders…", "Cartelle dei server…"),
            Self::CopyId => t("Copy server ID", "Copia ID server"),
            Self::Leave => t("Leave server", "Abbandona il server"),
        }
    }

    fn group(self) -> u8 {
        match self {
            Self::MarkRead => 0,
            Self::Mute(_) => 1,
            Self::Folders => 2,
            Self::CopyId => 3,
            Self::Leave => 4,
        }
    }
}

/// The menu of a server as a layer of the window, or the confirmation to
/// leave it.
pub fn server_view(menu: &ServerMenu) -> Element<'_, Message> {
    if menu.confirm_leave {
        let question = tf!(
            "Leave {}? You can join again only with an invite.",
            "Abbandonare {}? Potrai rientrare solo con un invito.",
            menu.name
        );
        let body = column![
            text(question)
                .size(13)
                .shaping(iced::widget::text::Shaping::Advanced),
            row![
                Space::new().width(Length::Fill),
                button(text(t("Cancel", "Annulla")).size(13))
                    .padding([5, 12])
                    .style(style::ghost)
                    .on_press(Message::CloseServerMenu),
                button(text(t("Leave", "Abbandona")).size(13).font(style::SEMIBOLD))
                    .padding([5, 12])
                    .style(style::control(style::Control::Leave))
                    .on_press(Message::ConfirmLeaveServer),
            ]
            .spacing(8)
            .align_y(Vertical::Center),
        ]
        .spacing(12);
        let at = menu.at;
        return iced::widget::stack![
            mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseServerMenu)
                .on_right_press(Message::CloseServerMenu),
            container(iced::widget::opaque(
                container(body).width(POPUP_WIDTH).padding(12).style(style::card)
            ))
            .padding(Padding {
                top: at.y,
                left: at.x,
                right: 0.0,
                bottom: 0.0,
            }),
        ]
        .into();
    }
    let mut lines = Vec::new();
    let mut group = None;
    for &action in &menu.actions {
        if group.is_some_and(|g| g != action.group()) {
            lines.push(Line::Split);
        }
        group = Some(action.group());
        lines.push(Line::Item {
            label: action.label(),
            danger: action == ServerAction::Leave,
            on_press: Box::new(Message::ServerAction(action)),
        });
    }
    layer(menu.at, lines, Message::CloseServerMenu)
}

/// The small window opened from a person's menu for kick, ban and timeout:
/// the confirmation, or first the timeout lengths. At the menu's place.
#[derive(Debug, Clone, PartialEq)]
pub struct ModPopup {
    pub user_id: String,
    pub guild_id: String,
    /// Their name in that server, for the question.
    pub name: String,
    pub at: Point,
    pub step: ModStep,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModStep {
    /// "Kick / Ban / Time out …?" with Confirm and Cancel.
    Confirm(crate::discord::rest::ModAction),
    /// The timeout lengths; one of them leads to the confirmation.
    Timeout,
}

const POPUP_WIDTH: f32 = 290.0;

/// The popup as a layer of the window; a click elsewhere closes it.
pub fn mod_view(p: &ModPopup) -> Element<'_, Message> {
    let popup = p.clone();
    iced::widget::stack![
        mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(Message::CloseModPopup)
            .on_right_press(Message::CloseModPopup),
        responsive(move |size| {
            // About 150 px tall; kept inside the window like the menus.
            let at = popup.at;
            let x = if at.x + POPUP_WIDTH + 8.0 > size.width {
                (size.width - POPUP_WIDTH - 8.0).max(0.0)
            } else {
                at.x
            };
            let y = if at.y + 150.0 > size.height {
                (size.height - 150.0).max(0.0)
            } else {
                at.y
            };
            let card = container(popup_body(&popup))
                .width(POPUP_WIDTH)
                .padding(12)
                .style(style::card);
            container(iced::widget::opaque(card))
                .padding(Padding {
                    top: y,
                    left: x,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into()
        }),
    ]
    .into()
}

fn popup_body<'a>(p: &ModPopup) -> Element<'a, Message> {
    use crate::discord::rest::ModAction;
    let cancel = || {
        button(text(t("Cancel", "Annulla")).size(13))
            .padding([5, 12])
            .style(style::ghost)
            .on_press(Message::CloseModPopup)
    };
    match &p.step {
        ModStep::Confirm(action) => {
            let verb = match action {
                ModAction::Kick => t("Kick", "Espelli"),
                ModAction::Ban => t("Ban", "Banna"),
                _ => t("Time out", "Timeout"),
            };
            column![
                text(super::profile::confirm_question(action, &p.name))
                    .size(13)
                    .shaping(iced::widget::text::Shaping::Advanced),
                row![
                    Space::new().width(Length::Fill),
                    cancel(),
                    button(text(verb).size(13).font(style::SEMIBOLD))
                        .padding([5, 12])
                        .style(style::control(style::Control::Leave))
                        .on_press(Message::ConfirmModPopup),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            ]
            .spacing(12)
            .into()
        }
        ModStep::Timeout => {
            let mut choices = row![].spacing(6);
            for c in super::profile::TIMEOUTS {
                choices = choices.push(
                    button(text(c.to_string()).size(12))
                        .padding([4, 10])
                        .style(style::ghost)
                        .on_press(Message::PickModTimeout(c.0)),
                );
            }
            column![
                text(tf!("Time out {} for…", "Timeout di {} per…", p.name))
                    .size(13)
                    .shaping(iced::widget::text::Shaping::Advanced),
                choices.wrap().vertical_spacing(6),
                row![Space::new().width(Length::Fill), cancel()],
            ]
            .spacing(10)
            .into()
        }
    }
}

/// The menu of a person as a layer of the window: their volume first (as
/// Discord's), then the actions.
pub fn user_view<'a>(menu: &'a UserMenu, audio: &crate::audio::AudioSettings) -> Element<'a, Message> {
    let mut lines = Vec::new();
    if menu.volume {
        lines.push(Line::Volume {
            user_id: menu.user_id.clone(),
            volume: audio.user_volume(&menu.user_id),
            muted: audio.muted_users.contains(&menu.user_id),
        });
        lines.push(Line::Split);
    }
    let mut group = None;
    for &action in &menu.actions {
        if group.is_some_and(|g| g != action.group()) {
            lines.push(Line::Split);
        }
        group = Some(action.group());
        lines.push(Line::Item {
            label: action.label(),
            danger: action.danger(),
            on_press: Box::new(Message::UserAction(action)),
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

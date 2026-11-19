//! The Friends view, on the direct messages side: friends (online or all),
//! friend requests received and sent, blocked people, and the field to send
//! a request by username.
//!
//! The data is in the `Store` (`store/friends.rs`). Every action here is a
//! click of the user; the answer comes back as a message and, on Discord,
//! as a gateway event too. Nothing is sent in the demo.

use std::collections::{HashMap, HashSet};

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, row, scrollable, space, stack, text, text_input, Column, Space,
};
use iced::{Element, Length, Padding, Task};

use super::style::*;
use super::{avatar_with, hline, Gui, Message, Preview, AVATAR_PX};
use crate::discord::member_list::Status;
use crate::discord::models::{RelationKind, Relationship};
use crate::discord::rest;
use crate::i18n::t;
use crate::store::{FriendsTab, Store};

/// Most avatars downloaded for the list (the first ones: the visible ones).
pub const PICTURES: usize = 60;
const AVATAR: f32 = 32.0;

/// A change to a relationship, from a button of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Accepts a friend request.
    Accept(String),
    /// Removes a friend, declines or cancels a request, unblocks.
    Remove(String),
}

impl Op {
    fn user_id(&self) -> &str {
        match self {
            Self::Accept(id) | Self::Remove(id) => id,
        }
    }
}

#[derive(Debug, Clone)]
pub enum FriendsMsg {
    Tab(FriendsTab),
    Input(String),
    SendRequest,
    RequestSent(Result<String, String>),
    Do(Op),
    /// "Remove friend" asks first (`None` = cancel).
    AskRemove(Option<String>),
    Done {
        op: Op,
        result: Result<(), String>,
    },
}

#[derive(Debug, Default)]
pub struct FriendsUi {
    /// The view is shown in place of a conversation.
    pub open: bool,
    pub tab: FriendsTab,
    /// The username in the Add tab.
    pub input: String,
    /// A request from the Add tab is under way.
    pub sending: bool,
    /// The answer to the last request: (text, is an error).
    pub add_result: Option<(String, bool)>,
    /// People with an action under way: their buttons wait.
    pub busy: HashSet<String>,
    /// The friend whose removal waits for a confirmation.
    pub confirm_remove: Option<String>,
}

impl Gui {
    pub(super) fn friends_update(&mut self, msg: FriendsMsg) -> Task<Message> {
        match msg {
            FriendsMsg::Tab(tab) => {
                self.friends.tab = tab;
                self.friends.confirm_remove = None;
                if tab == FriendsTab::Add {
                    return iced::widget::operation::focus(input_id());
                }
                return self.request_small_images();
            }
            FriendsMsg::Input(s) => {
                // Usernames are at most 32 characters: no need for more.
                self.friends.input = s.chars().take(64).collect();
                self.friends.add_result = None;
            }
            FriendsMsg::SendRequest => return self.send_friend_request(),
            FriendsMsg::RequestSent(result) => {
                self.friends.sending = false;
                self.friends.add_result = Some(match result {
                    Ok(name) => {
                        self.friends.input.clear();
                        (
                            tf!(
                                "Friend request sent to {name}.",
                                "Richiesta di amicizia inviata a {name}."
                            ),
                            false,
                        )
                    }
                    Err(e) => (e, true),
                });
            }
            FriendsMsg::AskRemove(id) => self.friends.confirm_remove = id,
            FriendsMsg::Do(op) => return self.friend_op(op),
            FriendsMsg::Done { op, result } => {
                self.friends.busy.remove(op.user_id());
                match result {
                    Ok(()) => self.apply_friend_op(&op),
                    Err(e) => self.status = Some((e, true)),
                }
            }
        }
        Task::none()
    }

    fn send_friend_request(&mut self) -> Task<Message> {
        if self.friends.sending {
            return Task::none();
        }
        let name = match rest::friend_username(&self.friends.input) {
            Ok(n) => n,
            Err(e) => {
                self.friends.add_result = Some((e, true));
                return Task::none();
            }
        };
        if self.demo {
            self.friends.add_result = Some((
                t(
                    "Nothing is sent in the demo: no friend request.",
                    "Nella demo non si invia nulla: nessuna richiesta di amicizia.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        self.friends.sending = true;
        Task::perform(
            async move {
                client
                    .send_friend_request(&name)
                    .await
                    .map(|()| name)
                    .map_err(|e| rest::friend_error(&e))
            },
            |r| Message::Friends(FriendsMsg::RequestSent(r)),
        )
    }

    fn friend_op(&mut self, op: Op) -> Task<Message> {
        let id = op.user_id().to_string();
        self.friends.confirm_remove = None;
        // Only for people in the list, once at a time.
        if self.store.relation(&id).is_none() || self.friends.busy.contains(&id) {
            return Task::none();
        }
        if matches!(op, Op::Accept(_)) && self.store.relation(&id) != Some(RelationKind::Incoming) {
            return Task::none();
        }
        if self.demo {
            // Only the demo's data changes.
            self.apply_friend_op(&op);
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        self.friends.busy.insert(id);
        Task::perform(
            async move {
                let result = match &op {
                    Op::Accept(id) => client.accept_friend(id).await,
                    Op::Remove(id) => client.remove_relationship(id).await,
                };
                let result = result.map_err(|e| rest::friend_error(&e));
                (op, result)
            },
            |(op, result)| Message::Friends(FriendsMsg::Done { op, result }),
        )
    }

    /// What Discord will also say with a gateway event: shown at once.
    fn apply_friend_op(&mut self, op: &Op) {
        match op {
            Op::Accept(id) => self.store.upsert_relationship(Relationship {
                id: id.clone(),
                kind: 1,
                user: None,
                nickname: None,
            }),
            Op::Remove(id) => self.store.remove_relationship(id),
        }
    }
}

pub fn input_id() -> iced::widget::Id {
    iced::widget::Id::new("friend-request")
}

/// The IDs whose avatars the list shows first.
pub fn shown_people(ui: &FriendsUi, store: &Store) -> Vec<String> {
    store
        .friends_in(ui.tab)
        .into_iter()
        .take(PICTURES)
        .map(str::to_string)
        .collect()
}

fn msg(m: FriendsMsg) -> Message {
    Message::Friends(m)
}

/// The view, in place of the conversation.
pub fn view<'a>(
    ui: &'a FriendsUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let pending = store.incoming_requests();
    let adding = ui.tab == FriendsTab::Add;
    let tab_button = |tab: FriendsTab, label: &'static str| -> Element<'a, Message> {
        let mut content = row![text(label).size(13)].spacing(6).align_y(Vertical::Center);
        if tab == FriendsTab::Pending && pending > 0 {
            content = content.push(badge(pending));
        }
        button(content)
            .on_press(msg(FriendsMsg::Tab(tab)))
            .padding([4, 10])
            .style(style_tab(ui.tab == tab))
            .into()
    };
    let header = container(
        row![
            text(t("Friends", "Amici")).font(BOLD),
            container(Space::new().width(1).height(18)).style(fill(pal::border())),
            tab_button(FriendsTab::Online, t("Online", "In linea")),
            tab_button(FriendsTab::All, t("All", "Tutti")),
            tab_button(FriendsTab::Pending, t("Pending", "In attesa")),
            tab_button(FriendsTab::Blocked, t("Blocked", "Bloccati")),
            button(text(t("Add friend", "Aggiungi amico")).size(13).font(SEMIBOLD))
                .on_press(msg(FriendsMsg::Tab(FriendsTab::Add)))
                .padding([4, 10])
                .style(move |th, st| if adding { ghost(th, st) } else { primary(th, st) }),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding([0, 16])
    .height(48)
    .width(Length::Fill)
    .align_y(Vertical::Center);

    let body: Element<'a, Message> = if ui.tab == FriendsTab::Add {
        add_view(ui)
    } else {
        list_view(ui, store, images)
    };
    column![header, hline(pal::border()), body]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn style_tab(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    tab(active)
}

fn badge<'a>(n: usize) -> Element<'a, Message> {
    container(
        text(n.min(99).to_string())
            .size(11)
            .font(BOLD)
            .color(iced::Color::WHITE),
    )
    .padding([0, 5])
    .style(rounded(pal::red(), 8.0))
    .into()
}

fn add_view(ui: &FriendsUi) -> Element<'_, Message> {
    let can_send = !ui.sending && !ui.input.trim().is_empty();
    let field = text_input(
        t("Username, e.g. ada.lovelace", "Nome utente, es. ada.lovelace"),
        &ui.input,
    )
    .id(input_id())
    .on_input(|s| msg(FriendsMsg::Input(s)))
    .on_submit(msg(FriendsMsg::SendRequest))
    .padding(10)
    .style(input);
    let send = button(
        text(if ui.sending {
            t("Sending…", "Invio…")
        } else {
            t("Send friend request", "Invia richiesta di amicizia")
        })
        .size(13)
        .font(SEMIBOLD),
    )
    .on_press_maybe(can_send.then(|| msg(FriendsMsg::SendRequest)))
    .padding([8, 14])
    .style(primary);
    let mut col = column![
        text(t("Add a friend", "Aggiungi un amico")).font(BOLD),
        text(t(
            "Write their Discord username (not the display name). They will see your request.",
            "Scrivi il suo nome utente Discord (non il nome visualizzato). Vedrà la tua richiesta.",
        ))
        .size(13)
        .color(pal::muted()),
        row![field, send].spacing(8).align_y(Vertical::Center),
    ]
    .spacing(10)
    .max_width(640);
    if let Some((note, error)) = &ui.add_result {
        col = col.push(
            text(note.as_str())
                .size(13)
                .color(if *error { pal::red() } else { pal::green() })
                .shaping(Shaping::Advanced),
        );
    }
    container(col).padding(20).width(Length::Fill).into()
}

fn list_view<'a>(
    ui: &'a FriendsUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let people = store.friends_in(ui.tab);
    let title = match ui.tab {
        FriendsTab::Online => t("Online", "In linea"),
        FriendsTab::Pending => t("Pending", "In attesa"),
        FriendsTab::Blocked => t("Blocked", "Bloccati"),
        _ => t("All friends", "Tutti gli amici"),
    };
    if people.is_empty() {
        let empty = match ui.tab {
            FriendsTab::Online => t(
                "No friends online right now.",
                "Nessun amico in linea in questo momento.",
            ),
            FriendsTab::Pending => t("No friend requests.", "Nessuna richiesta di amicizia."),
            FriendsTab::Blocked => t("You have not blocked anyone.", "Non hai bloccato nessuno."),
            _ => t(
                "No friends yet: add one with \"Add friend\".",
                "Ancora nessun amico: aggiungine uno con \"Aggiungi amico\".",
            ),
        };
        return container(text(empty).color(pal::muted()))
            .padding(20)
            .width(Length::Fill)
            .into();
    }
    let mut list = Column::new().spacing(2).padding(Padding {
        top: 4.0,
        right: 12.0,
        bottom: 12.0,
        left: 12.0,
    });
    list = list.push(
        container(
            text(format!("{} — {}", title.to_uppercase(), people.len()))
                .size(11)
                .font(BOLD)
                .color(pal::muted()),
        )
        .padding([10, 8]),
    );
    for id in people {
        list = list.push(person_row(ui, store, images, id));
        list = list.push(container(Space::new().width(Length::Fill).height(1)).style(fill(pal::soft())));
    }
    scrollable(list)
        .direction(super::thin())
        .style(thin_scrollbar)
        .height(Length::Fill)
        .into()
}

fn status_colour(s: Status) -> iced::Color {
    match s {
        Status::Online => pal::green(),
        Status::Idle => pal::yellow(),
        Status::DoNotDisturb => pal::red(),
        Status::Offline => pal::muted(),
    }
}

fn status_label(s: Status) -> &'static str {
    match s {
        Status::Online => t("Online", "In linea"),
        Status::Idle => t("Idle", "Inattivo"),
        Status::DoNotDisturb => t("Do not disturb", "Non disturbare"),
        Status::Offline => t("Offline", "Non in linea"),
    }
}

fn person_row<'a>(
    ui: &'a FriendsUi,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    id: &'a str,
) -> Element<'a, Message> {
    let kind = store.relation(id);
    let name = store.friend_name(id);
    let username = store
        .users
        .get(id)
        .map(|u| u.username.clone())
        .unwrap_or_default();
    let image = store.avatar_url(None, id, AVATAR_PX).and_then(|u| images.get(&u));
    let presence = store.friend_presence(id);
    let face: Element<'a, Message> = if kind == Some(RelationKind::Friend) {
        let dot = 11.0;
        stack![
            avatar_with(&name, id, AVATAR, image),
            container(
                container(Space::new().width(dot).height(dot))
                    .style(rounded(status_colour(presence.status), dot / 2.0))
            )
            .padding(Padding {
                top: AVATAR - dot + 2.0,
                left: AVATAR - dot + 2.0,
                ..Padding::ZERO
            }),
        ]
        .into()
    } else {
        avatar_with(&name, id, AVATAR, image)
    };
    let detail = match kind {
        Some(RelationKind::Friend) => presence
            .activity
            .clone()
            .unwrap_or_else(|| status_label(presence.status).to_string()),
        Some(RelationKind::Incoming) => t("Incoming friend request", "Richiesta di amicizia ricevuta").into(),
        Some(RelationKind::Outgoing) => t("Outgoing friend request", "Richiesta di amicizia inviata").into(),
        Some(RelationKind::Blocked) => t("Blocked", "Bloccato").into(),
        None => String::new(),
    };
    let mut names = row![text(name.clone())
        .size(14)
        .font(SEMIBOLD)
        .shaping(Shaping::Advanced)]
    .spacing(6)
    .align_y(Vertical::Center);
    if !username.is_empty() {
        names = names.push(
            text(username)
                .size(12)
                .color(pal::muted())
                .shaping(Shaping::Advanced)
                .wrapping(text::Wrapping::None),
        );
    }
    let who = row![
        face,
        column![
            names,
            text(detail)
                .size(12)
                .color(pal::muted())
                .shaping(Shaping::Advanced)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2)
    ]
    .spacing(12)
    .align_y(Vertical::Center);
    let user = id.to_string();
    let open = super::context::RightClick::new(
        button(container(who).clip(true))
            .on_press(Message::OpenProfile {
                user_id: id.to_string(),
                guild_id: None,
            })
            .width(Length::Fill)
            .padding([6, 8])
            .style(list_item(false)),
        move |at| Message::OpenUserMenu {
            user_id: user.clone(),
            guild_id: None,
            at,
        },
    );

    let busy = ui.busy.contains(id);
    let act = |label: &'static str, op: FriendsMsg, strong: bool, danger: bool| -> Element<'a, Message> {
        let mut label = text(label).size(13);
        if danger {
            label = label.color(pal::red());
        }
        button(label)
            .on_press_maybe((!busy).then_some(Message::Friends(op)))
            .padding([5, 10])
            .style(move |th, st| if strong { primary(th, st) } else { ghost(th, st) })
            .into()
    };
    let id_s = id.to_string();
    let actions: Element<'a, Message> = if ui.confirm_remove.as_deref() == Some(id) {
        row![
            text(tf!(
                "Remove {name} from your friends?",
                "Rimuovere {name} dagli amici?"
            ))
            .size(13)
            .shaping(Shaping::Advanced),
            act(
                t("Remove", "Rimuovi"),
                FriendsMsg::Do(Op::Remove(id_s)),
                false,
                true
            ),
            act(t("Cancel", "Annulla"), FriendsMsg::AskRemove(None), false, false),
        ]
        .spacing(6)
        .align_y(Vertical::Center)
        .into()
    } else {
        match kind {
            Some(RelationKind::Friend) => row![
                button(text(t("Message", "Messaggio")).size(13))
                    .on_press(Message::MessageUser(id_s.clone()))
                    .padding([5, 10])
                    .style(ghost),
                act(
                    t("Remove", "Rimuovi"),
                    FriendsMsg::AskRemove(Some(id_s)),
                    false,
                    true
                ),
            ]
            .spacing(6)
            .into(),
            Some(RelationKind::Incoming) => row![
                act(
                    t("Accept", "Accetta"),
                    FriendsMsg::Do(Op::Accept(id_s.clone())),
                    true,
                    false
                ),
                act(
                    t("Decline", "Rifiuta"),
                    FriendsMsg::Do(Op::Remove(id_s)),
                    false,
                    true
                ),
            ]
            .spacing(6)
            .into(),
            Some(RelationKind::Outgoing) => act(
                t("Cancel request", "Annulla richiesta"),
                FriendsMsg::Do(Op::Remove(id_s)),
                false,
                true,
            ),
            Some(RelationKind::Blocked) => act(
                t("Unblock", "Sblocca"),
                FriendsMsg::Do(Op::Remove(id_s)),
                false,
                false,
            ),
            None => space().into(),
        }
    };
    row![open, actions].spacing(8).align_y(Vertical::Center).into()
}

/// The "Friends" entry at the top of the direct messages, with the number
/// of requests waiting.
pub fn entry<'a>(open: bool, pending: usize) -> Element<'a, Message> {
    let mut line = row![
        container(super::icons::icon(
            &super::icons::FRIENDS,
            18.0,
            if open { pal::text() } else { pal::muted() }
        ))
        .width(DM_ICON)
        .center_x(DM_ICON),
        text(t("Friends", "Amici"))
            .size(14)
            .font(if open { BOLD } else { SEMIBOLD }),
    ]
    .spacing(8)
    .align_y(Vertical::Center);
    if pending > 0 {
        line = line.push(space::horizontal()).push(badge(pending));
    }
    button(line)
        .on_press(Message::OpenFriends)
        .width(Length::Fill)
        .padding([6, 8])
        .style(list_item(open))
        .into()
}

const DM_ICON: f32 = 24.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_name_their_person() {
        assert_eq!(Op::Accept("1".into()).user_id(), "1");
        assert_eq!(Op::Remove("2".into()).user_id(), "2");
    }
}

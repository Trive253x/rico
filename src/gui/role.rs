//! Card of a role, opened by clicking a role mention (`@Moderators`): its
//! colour, the faces of its members, how many are online and how many Rico
//! has loaded, the online ones as chips, and on request the whole list. A
//! click on a person opens their profile.

use std::collections::HashMap;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, mouse_area, opaque, row, scrollable, space, text, Column, Space,
};
use iced::{Element, Length};

use super::style::*;
use super::{avatar_with, Message, Preview, AVATAR_PX};
use crate::discord::member_list::{Row, Status};
use crate::i18n::t;
use crate::store::Store;

/// Faces in the row at the top.
pub const FACES: usize = 6;
/// Online people shown as chips before "See all".
const CHIPS: usize = 12;
/// At most this many names in the whole list (the count says how many there are).
const MAX_SHOWN: usize = 200;
const WIDTH: f32 = 300.0;

/// The open role card. Members and their status are found once, when it opens.
#[derive(Debug, Clone)]
pub struct RoleView {
    pub guild_id: String,
    pub role_id: String,
    /// `(user ID, name)`, sorted by name.
    pub members: Vec<(String, String)>,
    /// The online ones (online, idle, do not disturb), in the same order.
    pub online: Vec<(String, String)>,
    /// The whole list is open ("See all").
    pub show_all: bool,
}

impl RoleView {
    pub fn new(store: &Store, guild_id: String, role_id: String) -> Self {
        let members = store.role_members(&guild_id, &role_id);
        // Status from the member list; who is not in it is not counted as online.
        let status: HashMap<&str, Status> = store
            .member_lists
            .get(&guild_id)
            .map(|l| {
                l.known()
                    .filter_map(|r| match r {
                        Row::Member { user_id, status, .. } => Some((user_id.as_str(), *status)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let online = members
            .iter()
            .filter(|(id, _)| status.get(id.as_str()).is_some_and(|s| *s != Status::Offline))
            .cloned()
            .collect();
        Self {
            guild_id,
            role_id,
            members,
            online,
            show_all: false,
        }
    }

    /// The people whose avatar the card shows (to download them).
    pub fn shown_people(&self) -> impl Iterator<Item = &str> {
        let all = if self.show_all { MAX_SHOWN } else { 0 };
        self.members
            .iter()
            .take(FACES.max(all))
            .chain(self.online.iter().take(CHIPS))
            .map(|(id, _)| id.as_str())
    }
}

pub fn view<'a>(
    v: &'a RoleView,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let role = store.role(&v.guild_id, &v.role_id);
    let name = match role {
        Some(r) if v.role_id == v.guild_id => r.name.trim_start_matches('@').to_string(),
        Some(r) => r.name.clone(),
        None => t("unknown-role", "ruolo-sconosciuto").to_string(),
    };
    let colour = role
        .map(|r| r.color)
        .filter(|c| *c != 0)
        .map_or_else(pal::muted, |c| {
            iced::Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
        });
    let image = |id: &str| {
        store
            .avatar_url(Some(&v.guild_id), id, AVATAR_PX)
            .and_then(|u| images.get(&u))
    };
    let open = |id: &str| Message::OpenProfile {
        user_id: id.to_string(),
        guild_id: Some(v.guild_id.clone()),
    };

    let header = row![
        container(Space::new().width(10).height(10)).style(rounded(colour, 5.0)),
        text(name)
            .size(16)
            .font(BOLD)
            .color(colour)
            .shaping(Shaping::Advanced),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let mut body = column![header].spacing(12);
    let count = v.members.len();

    if count == 0 {
        body = body.push(
            text(t(
                "No member with this role loaded yet.",
                "Ancora nessun membro con questo ruolo caricato.",
            ))
            .size(12)
            .color(pal::muted()),
        );
    } else {
        // Faces in a row, then how many more.
        let mut faces = row![].spacing(4).align_y(Vertical::Center);
        for (id, member) in v.members.iter().take(FACES) {
            faces = faces.push(
                button(avatar_with(member, id, 34.0, image(id)))
                    .padding(0)
                    .style(bare)
                    .on_press(open(id)),
            );
        }
        if count > FACES {
            faces = faces.push(text(format!("+{}", count - FACES)).size(12).color(pal::muted()));
        }
        body = body.push(faces);

        // Online now and loaded in all.
        let tile = |n: usize, label: &'a str, c: iced::Color| {
            container(
                column![
                    text(n.to_string()).size(18).font(BOLD).color(c),
                    text(label).size(11).color(pal::muted()),
                ]
                .spacing(1),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(rounded(pal::bg(), 8.0))
        };
        body = body.push(
            row![
                tile(v.online.len(), t("online now", "online ora"), pal::green()),
                tile(count, t("loaded so far", "caricati finora"), pal::text()),
            ]
            .spacing(8),
        );

        // The online ones as chips.
        if !v.online.is_empty() {
            let mut chips = row![].spacing(6);
            for (id, member) in v.online.iter().take(CHIPS) {
                chips = chips.push(
                    button(
                        row![
                            avatar_with(member, id, 20.0, image(id)),
                            text(member.clone()).size(12).shaping(Shaping::Advanced),
                        ]
                        .spacing(6)
                        .align_y(Vertical::Center),
                    )
                    .padding([3, 9])
                    .style(chip_button)
                    .on_press(open(id)),
                );
            }
            body = body.push(
                column![
                    text(t("ONLINE NOW", "ONLINE ADESSO"))
                        .size(11)
                        .font(BOLD)
                        .color(pal::muted()),
                    chips.wrap().vertical_spacing(6),
                ]
                .spacing(6),
            );
        }

        // The whole list, on request.
        if v.show_all {
            let mut list = Column::new().spacing(1);
            for (id, member) in v.members.iter().take(MAX_SHOWN) {
                list = list.push(
                    button(
                        row![
                            avatar_with(member, id, 22.0, image(id)),
                            text(member.clone()).size(13).shaping(Shaping::Advanced),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .on_press(open(id))
                    .width(Length::Fill)
                    .padding([4, 6])
                    .style(ghost),
                );
            }
            if count > MAX_SHOWN {
                list = list.push(
                    text(tf!("and {} more", "e altri {}", count - MAX_SHOWN))
                        .size(12)
                        .color(pal::muted()),
                );
            }
            body = body.push(scrollable(list).height(Length::Shrink));
        } else if count > FACES {
            body = body.push(
                button(
                    text(tf!("See all {count}", "Vedi tutti i {count}"))
                        .size(12)
                        .width(Length::Fill)
                        .align_x(Horizontal::Center),
                )
                .on_press(Message::RoleShowAll)
                .width(Length::Fill)
                .padding([7, 10])
                .style(ghost),
            );
        }
        // The member list is loaded a page at a time: who has not been seen
        // yet is not here, and the card says so.
        body = body.push(
            text(t(
                "Only the members Rico has loaded so far: there may be more.",
                "Solo i membri che Rico ha caricato finora: potrebbero essercene altri.",
            ))
            .size(11)
            .color(pal::muted()),
        );
    }

    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseRole)
        .padding([4, 10])
        .style(ghost);
    body = body.push(row![space::horizontal(), close]);

    let card = container(body.width(WIDTH))
        .padding([14, 16])
        .max_height(560.0)
        .style(outlined);

    // A click on the card must not close it; around it, it does.
    let card = mouse_area(card).on_press(Message::Noop);
    opaque(
        mouse_area(
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .style(backdrop),
        )
        .on_press(Message::CloseRole),
    )
}

/// A rounded chip for a person.
fn chip_button(theme: &iced::Theme, status: button::Status) -> button::Style {
    let mut s = ghost(theme, status);
    s.border = iced::border::rounded(999);
    if matches!(status, button::Status::Active) {
        s.background = Some(iced::Background::Color(pal::bg()));
    }
    s
}

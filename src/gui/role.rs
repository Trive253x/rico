//! Card of a role, opened by clicking a role mention (`@Moderators`): its
//! colour, and the members with it that Rico knows. A click on a name opens
//! that person's profile.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, mouse_area, opaque, row, scrollable, space, text, Column};
use iced::{Element, Length};

use super::style::*;
use super::Message;
use crate::i18n::t;
use crate::store::Store;

/// At most this many names in the card (the count says how many there are).
const MAX_SHOWN: usize = 200;
const WIDTH: f32 = 300.0;

/// The open role card. The members are found once, when it opens.
#[derive(Debug, Clone)]
pub struct RoleView {
    pub guild_id: String,
    pub role_id: String,
    /// `(user ID, name)`, sorted by name.
    pub members: Vec<(String, String)>,
}

impl RoleView {
    pub fn new(store: &Store, guild_id: String, role_id: String) -> Self {
        let members = store.role_members(&guild_id, &role_id);
        Self {
            guild_id,
            role_id,
            members,
        }
    }
}

pub fn view<'a>(v: &'a RoleView, store: &'a Store) -> Element<'a, Message> {
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

    let header = row![
        container(space().width(12).height(12)).style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(colour)),
            border: iced::border::rounded(6),
            ..Default::default()
        }),
        text(format!("@{name}"))
            .size(16)
            .font(BOLD)
            .color(colour)
            .shaping(Shaping::Advanced),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    // The member list of the server is loaded a page at a time: who has not
    // been seen yet is not here, and the card says so.
    let count = v.members.len();
    let note = if count == 0 {
        t(
            "No member with this role loaded yet.",
            "Ancora nessun membro con questo ruolo caricato.",
        )
        .to_string()
    } else if count == 1 {
        t(
            "1 member loaded so far (there may be more)",
            "1 membro caricato finora (potrebbero essercene altri)",
        )
        .to_string()
    } else {
        tf!(
            "{count} members loaded so far (there may be more)",
            "{count} membri caricati finora (potrebbero essercene altri)"
        )
    };

    let mut list = Column::new().spacing(1);
    for (user_id, member) in v.members.iter().take(MAX_SHOWN) {
        list = list.push(
            button(text(member.clone()).size(13).shaping(Shaping::Advanced))
                .on_press(Message::OpenProfile {
                    user_id: user_id.clone(),
                    guild_id: Some(v.guild_id.clone()),
                })
                .width(Length::Fill)
                .padding([4, 8])
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

    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseRole)
        .padding([4, 10])
        .style(ghost);

    let card = container(
        column![
            header,
            text(note).size(12).color(pal::muted()),
            scrollable(list).height(Length::Shrink),
            row![space::horizontal(), close],
        ]
        .spacing(10)
        .width(WIDTH),
    )
    .padding([14, 16])
    .max_height(460.0)
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

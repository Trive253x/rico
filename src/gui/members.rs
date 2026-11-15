//! The members of a server on the right of its channels, like Discord's
//! member list: groups (hoisted roles, online, offline) with their count,
//! each member with avatar, online status and name in their role's colour.
//! A click opens the user card (profile, volume, moderation).

use std::collections::HashMap;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, mouse_area, row, scrollable, stack, text, Space};
use iced::{Element, Length, Padding};

use super::style::*;
use super::{avatar_with, Message, Preview, AVATAR_PX};
use crate::discord::member_list::{self, MemberList, Row, Status};
use crate::i18n::t;
use crate::store::Store;

/// Width of the column.
pub const WIDTH: f32 = 240.0;
const AVATAR: f32 = 32.0;

fn status_colour(s: Status) -> iced::Color {
    match s {
        Status::Online => pal::green(),
        Status::Idle => pal::yellow(),
        Status::DoNotDisturb => pal::red(),
        Status::Offline => pal::muted(),
    }
}

fn colour(c: u32) -> iced::Color {
    iced::Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// The column for the server `guild_id`.
pub fn view<'a>(
    store: &'a Store,
    guild_id: &'a str,
    images: &'a HashMap<String, Preview>,
    loaded_pages: usize,
) -> Element<'a, Message> {
    let list = store.member_lists.get(guild_id);
    let mut rows = column![].spacing(1);
    let mut shown = 0usize;
    match list {
        None => {
            rows = rows.push(
                container(
                    text(t("Loading the members…", "Caricamento dei membri…"))
                        .size(12)
                        .color(pal::muted()),
                )
                .padding([8, 8]),
            );
        }
        Some(list) => {
            for row_data in list.known() {
                match row_data {
                    Row::Group { id, count } => {
                        rows = rows.push(
                            container(
                                text(format!(
                                    "{} — {count}",
                                    store.group_name(guild_id, id).to_uppercase()
                                ))
                                .size(11)
                                .font(BOLD)
                                .color(pal::muted())
                                .shaping(Shaping::Advanced),
                            )
                            .padding(Padding {
                                top: 14.0,
                                bottom: 4.0,
                                left: 8.0,
                                right: 8.0,
                            }),
                        );
                    }
                    Row::Member { user_id, status } => {
                        shown += 1;
                        rows = rows.push(member_row(store, guild_id, user_id, *status, images));
                    }
                    Row::Unknown => {}
                }
            }
            if more_to_load(list, loaded_pages) {
                rows = rows.push(
                    container(
                        button(text(t("Show more", "Mostra altri")).size(12))
                            .on_press(Message::MemberListMore)
                            .padding([4, 10])
                            .style(ghost),
                    )
                    .padding([8, 8]),
                );
            }
            if shown == 0 {
                rows = rows.push(
                    container(
                        text(t("No members to show", "Nessun membro da mostrare"))
                            .size(12)
                            .color(pal::muted()),
                    )
                    .padding([8, 8]),
                );
            }
        }
    }
    container(
        scrollable(rows.padding([6, 6]))
            .direction(super::thin())
            .height(Length::Fill),
    )
    .width(WIDTH)
    .height(Length::Fill)
    .style(fill(pal::panel()))
    .into()
}

/// Every row asked has arrived (so there may be more), and more pages can
/// be asked.
fn more_to_load(list: &MemberList, loaded_pages: usize) -> bool {
    loaded_pages < member_list::MAX_PAGES && list.rows.len() >= loaded_pages * member_list::PAGE
}

fn member_row<'a>(
    store: &'a Store,
    guild_id: &'a str,
    user_id: &'a str,
    status: Status,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let name = store.users.get(user_id).map_or_else(
        || t("user", "utente").into(),
        |u| store.name_in(Some(guild_id), u),
    );
    let image = store
        .avatar_url(Some(guild_id), user_id, AVATAR_PX)
        .and_then(|a| images.get(&a));
    let offline = status == Status::Offline;
    // The status dot on the bottom right of the avatar, as on Discord.
    let face = stack![
        avatar_with(&name, user_id, AVATAR, image),
        container(
            container(Space::new().width(10).height(10))
                .style(rounded(status_colour(status), 5.0))
                .padding(0)
        )
        .padding(Padding {
            top: AVATAR - 12.0,
            left: AVATAR - 12.0,
            ..Padding::ZERO
        }),
    ];
    let name_colour = store
        .role_colour(guild_id, user_id)
        .map_or_else(pal::text, colour);
    let label = text(name)
        .size(14)
        .color(if offline { pal::muted() } else { name_colour })
        .shaping(Shaping::Advanced)
        .wrapping(text::Wrapping::None);
    let open = Message::OpenProfile {
        user_id: user_id.to_string(),
        guild_id: Some(guild_id.to_string()),
    };
    let line = row![face, container(label).width(Length::Fill).clip(true)]
        .spacing(10)
        .align_y(Vertical::Center);
    mouse_area(
        button(line)
            .width(Length::Fill)
            .padding([4, 8])
            .style(list_item(false))
            .on_press(open.clone()),
    )
    .on_right_press(open)
    .into()
}

//! User profile card, opened by clicking an avatar or a name.
//!
//! Two views, as on Discord: the account profile (name, avatar, banner, bio,
//! pronouns) and, inside a server, the server profile (nickname, server
//! avatar and banner, server bio, roles, member since). The data comes from
//! `GET /users/{id}/profile`, asked only when the card is opened; images come
//! from Discord's CDN.

use std::collections::HashMap;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, opaque, row, scrollable, text, Space,
};
use iced::{Element, Length};

use super::style::*;
use super::{avatar_with, Message, Preview, SmallKind};
use crate::discord::models::{GuildMember, ProfileDetails, User, UserProfile};
use crate::i18n::t;
use crate::media::{self, Shape};
use crate::store::Store;

/// Card width and banner size (logical pixels); images are asked at twice the size.
const WIDTH: f32 = 380.0;
const BANNER_H: f32 = 130.0;
const AVATAR: f32 = 84.0;
const AVATAR_PX: u32 = 168;
const BANNER_PX: (u32, u32) = (760, 260);
/// Longest bio shown (Discord allows 190 characters, 400 with Nitro).
const MAX_BIO: usize = 1000;
/// Most roles shown.
const MAX_ROLES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Account,
    Server,
}

#[derive(Debug, Clone)]
pub enum Data {
    Loading,
    Ready(Box<UserProfile>),
    Failed(String),
}

pub struct ProfileView {
    pub user_id: String,
    /// Server the profile was opened from, if any.
    pub guild_id: Option<String>,
    pub tab: Tab,
    pub data: Data,
}

impl ProfileView {
    pub fn new(user_id: String, guild_id: Option<String>) -> Self {
        let tab = if guild_id.is_some() {
            Tab::Server
        } else {
            Tab::Account
        };
        Self {
            user_id,
            guild_id,
            tab,
            data: Data::Loading,
        }
    }

    fn profile(&self) -> Option<&UserProfile> {
        match &self.data {
            Data::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// Server part, when the server tab is shown.
    fn server(&self) -> Option<(&str, Option<&GuildMember>, Option<&ProfileDetails>)> {
        let guild = self.guild_id.as_deref().filter(|_| self.tab == Tab::Server)?;
        let p = self.profile();
        Some((
            guild,
            p.and_then(|p| p.guild_member.as_ref()),
            p.and_then(|p| p.guild_member_profile.as_ref()),
        ))
    }

    fn avatar_url(&self, store: &Store) -> Option<String> {
        let server = self.server().and_then(|(g, member, _)| {
            let hash = member.and_then(|m| m.avatar.as_deref());
            media::member_avatar_url(g, &self.user_id, hash, AVATAR_PX)
        });
        server.or_else(|| {
            let hash = self
                .profile()
                .map(|p| &p.user)
                .or_else(|| store.users.get(&self.user_id))
                .and_then(|u| u.avatar.as_deref());
            media::avatar_url(&self.user_id, hash, AVATAR_PX)
        })
    }

    fn banner_url(&self) -> Option<String> {
        let p = self.profile()?;
        let server = self.server().and_then(|(g, member, details)| {
            let hash = member
                .and_then(|m| m.banner.as_deref())
                .or_else(|| details.and_then(|d| d.banner.as_deref()));
            media::member_banner_url(g, &self.user_id, hash)
        });
        server.or_else(|| {
            let hash = p
                .user
                .banner
                .as_deref()
                .or_else(|| p.user_profile.as_ref().and_then(|d| d.banner.as_deref()));
            media::banner_url(&self.user_id, hash)
        })
    }

    /// Images the card shows, to download: avatar and banner.
    pub(super) fn images(&self, store: &Store) -> Vec<(String, SmallKind)> {
        let mut out = Vec::new();
        if let Some(url) = self.avatar_url(store) {
            out.push((url, SmallKind::Shaped(Shape::Circle, AVATAR_PX, AVATAR_PX)));
        }
        if let Some(url) = self.banner_url() {
            out.push((url, SmallKind::Shaped(Shape::Plain, BANNER_PX.0, BANNER_PX.1)));
        }
        out
    }
}

/// When the account was created, from the snowflake ID (milliseconds since 2015).
fn created_at(id: &str) -> Option<chrono::NaiveDate> {
    let ms = (id.parse::<u64>().ok()? >> 22) + 1_420_070_400_000;
    chrono::DateTime::from_timestamp_millis(ms as i64).map(|d| d.date_naive())
}

fn date_label(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let lang = crate::i18n::lang();
    format!("{} {} {}", d.day(), crate::i18n::month(lang, d.month()), d.year())
}

fn heading<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(11).font(BOLD).color(pal::muted()).into()
}

/// Bio text: limited, with line breaks kept.
fn bio_text(bio: &str) -> String {
    let mut s: String = bio.chars().take(MAX_BIO).collect();
    if bio.chars().count() > MAX_BIO {
        s.push('…');
    }
    s
}

fn rgb(color: u32) -> iced::Color {
    iced::Color::from_rgb8((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

/// The profile card over a dark background; a click outside closes it.
pub fn view<'a>(
    p: &'a ProfileView,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
) -> Element<'a, Message> {
    let profile = p.profile();
    let user: Option<&User> = profile.map(|x| &x.user).or_else(|| store.users.get(&p.user_id));
    let (guild, member, server_details) = match p.server() {
        Some((g, m, d)) => (Some(g), m.or_else(|| store.member(g, &p.user_id)), d),
        None => (None, None, None),
    };

    // Banner: the image, or the profile colour, or the user's colour.
    let accent = server_details
        .and_then(|d| d.accent_color)
        .or_else(|| {
            profile
                .and_then(|x| x.user_profile.as_ref())
                .and_then(|d| d.accent_color)
        })
        .or_else(|| user.and_then(|u| u.accent_color))
        .map(rgb)
        .unwrap_or_else(|| user_color(&p.user_id));
    let banner: Element<'a, Message> = match p.banner_url().and_then(|u| images.get(&u)) {
        Some(Preview::Ready(h)) => img::Image::new(h.clone())
            .width(WIDTH)
            .height(BANNER_H)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        _ => container(Space::new(WIDTH, BANNER_H)).style(fill(accent)).into(),
    };

    let account_name = user.map(User::display_name).unwrap_or("…").to_string();
    let name = match (guild, user) {
        (Some(g), Some(u)) => member
            .and_then(|m| m.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| store.name_in(Some(g), u)),
        _ => account_name.clone(),
    };
    let big_avatar = avatar_with(
        &name,
        &p.user_id,
        AVATAR,
        p.avatar_url(store).and_then(|u| images.get(&u)),
    );

    let mut names = column![text(name.clone()).size(20).font(BOLD).shaping(Shaping::Advanced)].spacing(2);
    if let Some(u) = user {
        names = names.push(
            text(format!("@{}", u.username))
                .size(13)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        );
    }
    let details = server_details
        .or_else(|| profile.and_then(|x| x.user_profile.as_ref()))
        .filter(|_| p.tab == Tab::Server)
        .or_else(|| profile.and_then(|x| x.user_profile.as_ref()));
    if let Some(pronouns) = details
        .and_then(|d| d.pronouns.as_deref())
        .filter(|s| !s.is_empty())
    {
        names = names.push(
            text(pronouns.to_string())
                .size(12)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        );
    }

    let mut body = column![row![big_avatar, names].spacing(14).align_y(Vertical::Center)].spacing(14);

    if p.guild_id.is_some() {
        let tab_button = |label: &'a str, which: Tab| {
            button(text(label).size(12))
                .on_press(Message::ProfileTab(which))
                .padding([4, 10])
                .style(tab(p.tab == which))
        };
        body = body.push(
            row![
                tab_button(t("Server profile", "Profilo del server"), Tab::Server),
                tab_button(t("Account", "Account"), Tab::Account),
            ]
            .spacing(6),
        );
    }

    match &p.data {
        Data::Loading => {
            body = body.push(text(t("Loading the profile…", "Caricamento del profilo…")).color(pal::muted()))
        }
        Data::Failed(e) => {
            body = body.push(
                text(tf!("Profile unavailable: {e}", "Profilo non disponibile: {e}"))
                    .size(12)
                    .color(pal::red()),
            )
        }
        Data::Ready(_) => {}
    }

    // About me: the server bio on the server tab, if there is one.
    let bio = server_details
        .filter(|_| p.tab == Tab::Server)
        .and_then(|d| d.bio.as_deref())
        .filter(|b| !b.trim().is_empty())
        .or_else(|| {
            profile
                .and_then(|x| x.user_profile.as_ref())
                .and_then(|d| d.bio.as_deref())
        })
        .filter(|b| !b.trim().is_empty());
    if let Some(bio) = bio {
        body = body.push(
            column![
                heading(t("ABOUT ME", "INFORMAZIONI")),
                text(bio_text(bio)).size(13).shaping(Shaping::Advanced)
            ]
            .spacing(4),
        );
    }

    if let (Some(g), Some(m)) = (guild, member) {
        if let Some(joined) = m
            .joined_at
            .as_deref()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        {
            body = body.push(
                column![
                    heading(t("MEMBER OF THE SERVER SINCE", "MEMBRO DEL SERVER DAL")),
                    text(date_label(joined.date_naive())).size(13)
                ]
                .spacing(4),
            );
        }
        let mut roles: Vec<_> = store
            .guild(g)
            .map(|guild| guild.roles.iter().filter(|r| m.roles.contains(&r.id)).collect())
            .unwrap_or_default();
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        if !roles.is_empty() {
            let chips = roles.into_iter().take(MAX_ROLES).map(|r| {
                let color = if r.color == 0 { pal::muted() } else { rgb(r.color) };
                container(
                    row![
                        super::dot(color, 8.0),
                        text(r.name.clone()).size(12).shaping(Shaping::Advanced)
                    ]
                    .spacing(5)
                    .align_y(Vertical::Center),
                )
                .padding([3, 8])
                .style(fill(pal::soft()))
                .into()
            });
            body = body.push(column![heading(t("ROLES", "RUOLI")), row(chips).spacing(6).wrap()].spacing(6));
        }
    } else if let Some(created) = created_at(&p.user_id) {
        body = body.push(
            column![
                heading(t("DISCORD MEMBER SINCE", "SU DISCORD DAL")),
                text(date_label(created)).size(13)
            ]
            .spacing(4),
        );
    }

    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseProfile)
        .padding([4, 10])
        .style(ghost);
    body = body.push(row![horizontal_space(), close]);

    let card = container(
        column![
            banner,
            container(scrollable(body.padding([0, 4])))
                .padding([14, 16])
                .max_height(440.0)
        ]
        .width(WIDTH),
    )
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
        .on_press(Message::CloseProfile),
    )
}

/// Profile made up from what the store knows, for the demo (no network).
pub fn demo_profile(store: &Store, user_id: &str, guild_id: Option<&str>) -> Option<UserProfile> {
    let user = store.users.get(user_id)?.clone();
    let guild_member = guild_id.map(|g| {
        let mut m = store.member(g, user_id).cloned().unwrap_or_default();
        m.joined_at
            .get_or_insert_with(|| "2024-03-14T18:00:00+00:00".to_string());
        if m.roles.is_empty() {
            if let Some(r) = store.guild(g).and_then(|g| g.roles.first()) {
                m.roles.push(r.id.clone());
            }
        }
        m
    });
    Some(UserProfile {
        user,
        user_profile: Some(ProfileDetails {
            bio: Some(
                t(
                    "Rust, home networks and too many browser tabs.\nThis is demo data.",
                    "Rust, reti di casa e troppe schede aperte.\nQuesti sono dati della demo.",
                )
                .to_string(),
            ),
            pronouns: None,
            accent_color: None,
            banner: None,
        }),
        guild_member,
        guild_member_profile: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_creation_date_from_the_id() {
        // The example snowflake of Discord's documentation.
        let d = created_at("175928847299117063").unwrap();
        assert_eq!(d.to_string(), "2016-04-30");
        assert!(created_at("not a number").is_none());
    }

    #[test]
    fn long_bios_are_cut() {
        let long = "a".repeat(MAX_BIO + 50);
        assert_eq!(bio_text(&long).chars().count(), MAX_BIO + 1);
        assert_eq!(bio_text("short"), "short");
    }
}

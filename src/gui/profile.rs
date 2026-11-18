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
    button, column, container, mouse_area, opaque, pick_list, row, scrollable, slider, space, text, Space,
};
use iced::{Element, Length};

use super::icons;
use super::style::*;
use super::voice::control_button;
use super::{avatar_with, Message, Preview, SmallKind};
use crate::audio::{AudioSettings, MAX_GAIN};
use crate::discord::models::{GuildMember, ProfileDetails, User, UserProfile};
use crate::discord::rest::ModAction;
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
    /// A moderation action waiting for "Confirm".
    pub pending: Option<ModAction>,
    /// A moderation action is on its way to Discord.
    pub busy: bool,
    /// How the last moderation action went (text for the card).
    pub outcome: Option<Result<String, String>>,
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
            pending: None,
            busy: false,
            outcome: None,
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

fn heading(label: &str) -> Element<'_, Message> {
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
/// What the card shows besides the profile: the volume chosen for this
/// person, and whether `@mention` can go in the message being written.
pub struct Extras<'a> {
    pub audio: &'a AudioSettings,
    pub can_mention: bool,
}

pub fn view<'a>(
    p: &'a ProfileView,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    extras: Extras<'a>,
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
        .map_or_else(|| user_color(&p.user_id), rgb);
    let banner: Element<'a, Message> = match p.banner_url().and_then(|u| images.get(&u)) {
        Some(Preview::Ready(h)) => img::Image::new(h.clone())
            .width(WIDTH)
            .height(BANNER_H)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        _ => container(Space::new().width(WIDTH).height(BANNER_H))
            .style(fill(accent))
            .into(),
    };

    let account_name = user.map_or("…", User::display_name).to_string();
    let name = match (guild, user) {
        (Some(g), Some(u)) => member
            .and_then(|m| m.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| store.name_in(Some(g), u)),
        _ => account_name,
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
            body = body.push(text(t("Loading the profile…", "Caricamento del profilo…")).color(pal::muted()));
        }
        Data::Failed(e) => {
            body = body.push(
                text(tf!("Profile unavailable: {e}", "Profilo non disponibile: {e}"))
                    .size(12)
                    .color(pal::red()),
            );
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

    if store.me_id() != Some(p.user_id.as_str()) {
        body = body.push(volume_section(p, &extras));
    }
    body = body.push(quick_actions(p, &extras));
    if let Some(g) = p.guild_id.as_deref() {
        if let Some(section) = moderation_section(p, store, g, &name) {
            body = body.push(section);
        }
    }

    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseProfile)
        .padding([4, 10])
        .style(ghost);
    body = body.push(row![space::horizontal(), close]);

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

/// This person's volume in calls, only for us: mute, slider, percentage.
/// It works whether or not they are in a call now (it is kept).
fn volume_section<'a>(p: &'a ProfileView, extras: &Extras<'a>) -> Element<'a, Message> {
    let id = p.user_id.clone();
    let muted = extras.audio.muted_users.contains(&p.user_id);
    let volume = extras.audio.user_volume(&p.user_id);
    let speaker = control_button(
        if muted {
            &icons::VOLUME_OFF
        } else if volume < 0.5 {
            &icons::VOLUME_LOW
        } else {
            &icons::VOLUME
        },
        if muted {
            super::style::Control::Off
        } else {
            super::style::Control::On
        },
        if muted {
            t(
                "Muted for you: click to hear them",
                "Silenziato per te: clic per sentirlo",
            )
        } else {
            t("Mute for you", "Silenzia per te")
        }
        .into(),
        Some(Message::ToggleUserMute(id.clone())),
        32.0,
    );
    let level = slider(0.0..=MAX_GAIN * 100.0, volume * 100.0, move |percent| {
        Message::UserVolume {
            user_id: id.clone(),
            percent,
        }
    })
    .step(5.0_f32)
    .on_release(Message::SaveSettings);
    let percent = text(if muted {
        t("muted", "muto").to_string()
    } else {
        format!("{}%", (volume * 100.0).round() as u32)
    })
    .size(12)
    .width(48)
    .color(if volume > 1.0 && !muted {
        pal::yellow()
    } else {
        pal::muted()
    });
    column![
        heading(t("VOLUME IN CALLS", "VOLUME NELLE CHIAMATE")),
        row![speaker, level, percent]
            .spacing(10)
            .align_y(Vertical::Center),
        text(t(
            "Only you hear the difference; it stays for the next calls.",
            "Solo tu senti la differenza; resta per le prossime chiamate.",
        ))
        .size(11)
        .color(pal::muted()),
    ]
    .spacing(6)
    .into()
}

/// Mention in the message being written, copy the ID.
fn quick_actions<'a>(p: &'a ProfileView, extras: &Extras<'a>) -> Element<'a, Message> {
    let small = |label: &'a str, msg: Option<Message>| {
        button(text(label).size(12))
            .on_press_maybe(msg)
            .padding([4, 10])
            .style(ghost)
    };
    row![
        small(
            t("Mention", "Menziona"),
            extras
                .can_mention
                .then(|| Message::MentionUser(p.user_id.clone()))
        ),
        small(
            t("Copy ID", "Copia ID"),
            Some(Message::CopyUserId(p.user_id.clone()))
        ),
    ]
    .spacing(6)
    .into()
}

/// A timeout length for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutChoice(pub u64);

pub const TIMEOUTS: [TimeoutChoice; 6] = [
    TimeoutChoice(60),
    TimeoutChoice(5 * 60),
    TimeoutChoice(10 * 60),
    TimeoutChoice(3600),
    TimeoutChoice(24 * 3600),
    TimeoutChoice(7 * 24 * 3600),
];

impl std::fmt::Display for TimeoutChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&duration_label(self.0))
    }
}

fn duration_label(secs: u64) -> String {
    match secs {
        s if s < 3600 => tf!("{} min", "{} min", s / 60),
        s if s < 24 * 3600 => tf!("{} h", "{} h", s / 3600),
        s if s < 2 * 24 * 3600 => t("1 day", "1 giorno").to_string(),
        s if s < 7 * 24 * 3600 => tf!("{} days", "{} giorni", s / (24 * 3600)),
        s if s < 2 * 7 * 24 * 3600 => t("1 week", "1 settimana").to_string(),
        s => tf!("{} weeks", "{} settimane", s / (7 * 24 * 3600)),
    }
}

/// A voice channel for the "Move to" pick list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelChoice(String, String);

impl std::fmt::Display for ChannelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "🔊 {}", self.1)
    }
}

/// The question asked before kicking, banning or timing someone out.
pub fn confirm_question(action: &ModAction, name: &str) -> String {
    match action {
        ModAction::Kick => tf!(
            "Kick {name} from the server? They can come back with an invite.",
            "Espellere {name} dal server? Potrà rientrare con un invito."
        ),
        ModAction::Ban => tf!(
            "Ban {name} from the server? They cannot come back until the ban is removed.",
            "Bannare {name} dal server? Non potrà rientrare finché il ban non viene tolto."
        ),
        ModAction::Timeout(Some(secs)) => tf!(
            "Time out {name} for {}? They will not be able to write, react or speak.",
            "Mettere {name} in timeout per {}? Non potrà scrivere, reagire né parlare.",
            duration_label(*secs)
        ),
        _ => String::new(),
    }
}

/// The words shown when an action went through.
pub fn done_label(action: &ModAction) -> &'static str {
    match action {
        ModAction::Mute(true) => t("Muted in the server.", "Silenziato nel server."),
        ModAction::Mute(false) => t("Unmuted in the server.", "Microfono riattivato nel server."),
        ModAction::Deafen(true) => t("Deafened in the server.", "Audio disattivato nel server."),
        ModAction::Deafen(false) => t("Undeafened in the server.", "Audio riattivato nel server."),
        ModAction::Move(_) => t("Moved.", "Spostato."),
        ModAction::Disconnect => t("Disconnected from voice.", "Scollegato dalla voce."),
        ModAction::Timeout(Some(_)) => t("Timed out.", "Messo in timeout."),
        ModAction::Timeout(None) => t("Timeout removed.", "Timeout rimosso."),
        ModAction::Kick => t("Kicked from the server.", "Espulso dal server."),
        ModAction::Ban => t("Banned from the server.", "Bannato dal server."),
    }
}

/// Server moderation, only with the permissions (and above them in the
/// roles for kick, ban and timeout). Removing someone or timing them out
/// asks for a confirmation in the card first.
fn moderation_section<'a>(
    p: &'a ProfileView,
    store: &'a Store,
    guild_id: &str,
    name: &str,
) -> Option<Element<'a, Message>> {
    let m = store.moderation(guild_id, &p.user_id);
    if !m.any() {
        return None;
    }
    let busy = p.busy || p.pending.is_some();
    let act = |a: ModAction| (!busy).then_some(Message::Moderate(a));
    let plain = |label: &'a str, msg: Option<Message>| {
        button(text(label).size(12))
            .on_press_maybe(msg)
            .padding([4, 10])
            .style(ghost)
    };
    let strong = |label: &'a str, msg: Option<Message>| {
        button(text(label).size(12).font(SEMIBOLD))
            .on_press_maybe(msg)
            .padding([4, 10])
            .style(control(Control::Leave))
    };
    let mut section = column![heading(t("MODERATION", "MODERAZIONE"))].spacing(8);

    if let Some(channel) = &m.in_voice {
        let flags = store.voice_status.get(&p.user_id).copied().unwrap_or_default();
        let mut voice_row = row![].spacing(6).align_y(Vertical::Center);
        if m.mute {
            voice_row = voice_row.push(plain(
                if flags.mute {
                    t("Unmute in the server", "Riattiva il microfono")
                } else {
                    t("Mute in the server", "Silenzia nel server")
                },
                act(ModAction::Mute(!flags.mute)),
            ));
        }
        if m.deafen {
            voice_row = voice_row.push(plain(
                if flags.deaf {
                    t("Undeafen", "Riattiva l'audio")
                } else {
                    t("Deafen in the server", "Disattiva l'audio")
                },
                act(ModAction::Deafen(!flags.deaf)),
            ));
        }
        if m.move_members {
            voice_row = voice_row.push(plain(t("Disconnect", "Scollega"), act(ModAction::Disconnect)));
        }
        section = section.push(
            column![
                text(tf!(
                    "In voice: 🔊 {}",
                    "In voce: 🔊 {}",
                    store.channel_names.get(channel).cloned().unwrap_or_default()
                ))
                .size(12)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
                voice_row.wrap(),
            ]
            .spacing(6),
        );
        if m.move_members {
            let targets: Vec<ChannelChoice> = store
                .move_targets(guild_id)
                .into_iter()
                .filter(|(id, _)| id != channel)
                .map(|(id, name)| ChannelChoice(id, name))
                .collect();
            if !targets.is_empty() {
                let pick = pick_list(targets, None::<ChannelChoice>, move |c: ChannelChoice| {
                    Message::Moderate(ModAction::Move(c.0))
                })
                .placeholder(t("Move to…", "Sposta in…"))
                .text_size(12)
                .padding([4, 10]);
                section = section.push(if busy {
                    Element::from(Space::new().width(0).height(0))
                } else {
                    pick.into()
                });
            }
        }
    }

    let mut member_row = row![].spacing(6).align_y(Vertical::Center);
    if m.timeout {
        let timed_out = store
            .member(guild_id, &p.user_id)
            .and_then(|mm| mm.communication_disabled_until.as_deref())
            .is_some_and(|until| crate::permissions::timed_out(Some(until)));
        if timed_out {
            member_row = member_row.push(plain(
                t("Remove timeout", "Togli il timeout"),
                act(ModAction::Timeout(None)),
            ));
        } else if !busy {
            member_row = member_row.push(
                pick_list(TIMEOUTS, None::<TimeoutChoice>, |c: TimeoutChoice| {
                    Message::Moderate(ModAction::Timeout(Some(c.0)))
                })
                .placeholder(t("Timeout…", "Timeout…"))
                .text_size(12)
                .padding([4, 10]),
            );
        }
    }
    if m.kick {
        member_row = member_row.push(strong(t("Kick", "Espelli"), act(ModAction::Kick)));
    }
    if m.ban {
        member_row = member_row.push(strong(t("Ban", "Banna"), act(ModAction::Ban)));
    }
    section = section.push(member_row.wrap());

    // The confirmation, in the card itself.
    if let Some(action) = &p.pending {
        let question = confirm_question(action, name);
        section = section.push(
            container(
                column![
                    text(question).size(12).shaping(Shaping::Advanced),
                    row![
                        strong(t("Confirm", "Conferma"), Some(Message::ConfirmModeration)),
                        plain(t("Cancel", "Annulla"), Some(Message::CancelModeration)),
                    ]
                    .spacing(6),
                ]
                .spacing(8),
            )
            .padding(10)
            .width(Length::Fill)
            .style(fill(pal::soft())),
        );
    }
    if p.busy {
        section = section.push(
            text(t("Sending to Discord…", "Invio a Discord…"))
                .size(12)
                .color(pal::muted()),
        );
    }
    match &p.outcome {
        Some(Ok(done)) => {
            section = section.push(
                text(done.clone())
                    .size(12)
                    .color(pal::green())
                    .shaping(Shaping::Advanced),
            );
        }
        Some(Err(e)) => {
            section = section.push(
                text(tf!("Not done: {e}", "Non riuscito: {e}"))
                    .size(12)
                    .color(pal::red()),
            );
        }
        None => {}
    }
    Some(section.into())
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

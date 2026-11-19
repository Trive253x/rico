//! The members of a server on the right of its channels, like Discord's
//! member list, in four styles chosen in the settings:
//!
//! - **classic**: groups (hoisted roles, online, offline) with their count,
//!   avatar with the online status and the name in the role's colour;
//! - **compact**: short rows, a search field and the counts;
//! - **rich**: bigger cards with what each person is doing (in voice,
//!   streaming, typing, custom status);
//! - **by activity**: who is in voice first, then who wrote here recently,
//!   then online, and offline folded.
//!
//! A click on a person opens the user card (profile, volume, moderation).
//! Everything shown comes from data Rico already has (member list, voice
//! states, typing, loaded messages): no request is made for it. The work
//! done here is a pass over at most the 500 rows of the list.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, row, scrollable, stack, text, text_input, Column, Space};
use iced::{Element, Length, Padding};

use super::style::*;
use super::{avatar_with, icons, stream, Message, Preview, AVATAR_PX};
use crate::discord::member_list::{self, MemberList, Row, Status};
use crate::i18n::t;
use crate::settings::MemberStyle;
use crate::store::Store;

/// Width of the column.
pub const WIDTH: f32 = 240.0;
const AVATAR: f32 = 32.0;
/// "Wrote here recently" in the by-activity style.
const RECENT: Duration = Duration::from_secs(15 * 60);
/// Most offline faces in the folded row.
const OFFLINE_FACES: usize = 5;

/// How the column is drawn, from the settings and the column's own state.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    pub style: MemberStyle,
    /// Text of the search field (compact style).
    pub filter: &'a str,
    /// The offline people are unfolded (by-activity style).
    pub offline_open: bool,
    /// The open channel, for who is typing or wrote there.
    pub channel_id: Option<&'a str>,
    /// The people we muted for ourselves (marked in voice).
    pub muted_for_us: &'a std::collections::BTreeSet<String>,
}

fn status_colour(s: Status) -> iced::Color {
    match s {
        Status::Online => pal::green(),
        Status::Idle => pal::yellow(),
        Status::DoNotDisturb => pal::red(),
        Status::Offline => pal::muted(),
    }
}

fn status_text(s: Status) -> Option<&'static str> {
    match s {
        Status::Idle => Some(t("idle", "inattivo")),
        Status::DoNotDisturb => Some(t("do not disturb", "non disturbare")),
        Status::Online | Status::Offline => None,
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
    look: Look<'a>,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = match store.member_lists.get(guild_id) {
        None => note_line(t("Loading the members…", "Caricamento dei membri…")),
        Some(list) => {
            let ctx = Ctx {
                store,
                guild_id,
                images,
                look,
            };
            let mut col = match look.style {
                MemberStyle::Classic => classic(&ctx, list, false),
                MemberStyle::Compact => compact(&ctx, list),
                MemberStyle::Rich => classic(&ctx, list, true),
                MemberStyle::Activity => by_activity(&ctx, list),
            };
            if more_to_load(list, loaded_pages) {
                col = col.push(
                    container(
                        button(text(t("Show more", "Mostra altri")).size(12))
                            .on_press(Message::MemberListMore)
                            .padding([4, 10])
                            .style(ghost),
                    )
                    .padding([8, 8]),
                );
            }
            col.into()
        }
    };
    container(
        scrollable(container(body).padding([6, 6]))
            .direction(super::thin())
            .height(Length::Fill),
    )
    .width(WIDTH)
    .height(Length::Fill)
    .style(fill(pal::panel()))
    .into()
}

struct Ctx<'a> {
    store: &'a Store,
    guild_id: &'a str,
    images: &'a HashMap<String, Preview>,
    look: Look<'a>,
}

impl<'a> Ctx<'a> {
    fn name(&self, user_id: &str) -> String {
        self.store.users.get(user_id).map_or_else(
            || t("user", "utente").into(),
            |u| self.store.name_in(Some(self.guild_id), u),
        )
    }

    fn name_colour(&self, user_id: &str, status: Status) -> iced::Color {
        if status == Status::Offline {
            return pal::muted();
        }
        self.store
            .role_colour(self.guild_id, user_id)
            .map_or_else(pal::text, colour)
    }

    fn image(&self, user_id: &str) -> Option<&'a Preview> {
        self.store
            .avatar_url(Some(self.guild_id), user_id, AVATAR_PX)
            .and_then(|a| self.images.get(&a))
    }

    /// The person's Go Live, if they are streaming in this server.
    fn stream(&self, user_id: &str) -> Option<stream::StreamRef> {
        let (guild, channel) = self.store.voice.get(user_id)?;
        (guild == self.guild_id && self.store.streaming.contains(user_id)).then(|| stream::StreamRef {
            guild_id: guild.clone(),
            channel_id: channel.clone(),
            user_id: user_id.to_string(),
        })
    }

    fn open(&self, user_id: &str) -> Message {
        Message::OpenProfile {
            user_id: user_id.to_string(),
            guild_id: Some(self.guild_id.to_string()),
        }
    }

    /// What the person is doing, for the rich style: in voice, typing here,
    /// custom status or game, otherwise idle / do not disturb.
    fn doing(&self, user_id: &str, status: Status, activity: Option<&String>) -> Option<String> {
        if let Some(channel) = self.store.voice_channel_name(self.guild_id, user_id) {
            let what = if self.store.streaming.contains(user_id) {
                t("Streaming in", "In streaming in")
            } else {
                t("In voice ·", "In voce ·")
            };
            return Some(format!("{what} {channel}"));
        }
        if let Some(c) = self.look.channel_id {
            if self.store.typing_ids(c).contains(&user_id) {
                return Some(t("Typing…", "Sta scrivendo…").into());
            }
        }
        if let Some(a) = activity {
            return Some(a.clone());
        }
        status_text(status).map(str::to_string)
    }
}

/// Classic and rich: the groups of the list as Discord sends them.
fn classic<'a>(ctx: &Ctx<'a>, list: &'a MemberList, rich: bool) -> Column<'a, Message> {
    let mut rows = column![].spacing(if rich { 4 } else { 1 });
    let mut shown = 0usize;
    for row_data in list.known() {
        match row_data {
            Row::Group { id, count } => {
                rows = rows.push(group_header(
                    format!(
                        "{} — {count}",
                        ctx.store.group_name(ctx.guild_id, id).to_uppercase()
                    ),
                    None,
                ));
            }
            Row::Member {
                user_id,
                status,
                activity,
            } => {
                shown += 1;
                rows = rows.push(if rich {
                    rich_row(ctx, user_id, *status, activity.as_ref())
                } else {
                    classic_row(ctx, user_id, *status)
                });
            }
            Row::Unknown => {}
        }
    }
    if shown == 0 {
        rows = rows.push(note_line(t("No members to show", "Nessun membro da mostrare")));
    }
    rows
}

fn group_header<'a>(label: String, dot: Option<iced::Color>) -> Element<'a, Message> {
    let title = text(label)
        .size(11)
        .font(BOLD)
        .color(pal::muted())
        .shaping(Shaping::Advanced);
    let line: Element<'a, Message> = match dot {
        Some(c) => row![
            container(Space::new().width(8).height(8)).style(rounded(c, 4.0)),
            title
        ]
        .spacing(6)
        .align_y(Vertical::Center)
        .into(),
        None => title.into(),
    };
    container(line)
        .padding(Padding {
            top: 14.0,
            bottom: 4.0,
            left: 8.0,
            right: 8.0,
        })
        .into()
}

fn note_line(s: &str) -> Element<'_, Message> {
    container(text(s).size(12).color(pal::muted()))
        .padding([8, 8])
        .into()
}

/// Avatar with the status dot on the bottom right, as on Discord.
fn face<'a>(ctx: &Ctx<'a>, user_id: &str, name: &str, size: f32, status: Status) -> Element<'a, Message> {
    let dot = (size * 0.32).round().max(7.0);
    stack![
        avatar_with(name, user_id, size, ctx.image(user_id)),
        container(
            container(Space::new().width(dot).height(dot)).style(rounded(status_colour(status), dot / 2.0))
        )
        .padding(Padding {
            top: size - dot + 2.0,
            left: size - dot + 2.0,
            ..Padding::ZERO
        }),
    ]
    .into()
}

fn clickable<'a>(
    ctx: &Ctx<'a>,
    user_id: &str,
    content: Element<'a, Message>,
    pad: [u16; 2],
) -> Element<'a, Message> {
    let open = ctx.open(user_id);
    let (user, guild) = (user_id.to_string(), ctx.guild_id.to_string());
    // A right click opens the person's menu.
    super::context::RightClick::new(
        button(content)
            .width(Length::Fill)
            .padding(pad)
            .style(list_item(false))
            .on_press(open),
        move |at| Message::OpenUserMenu {
            user_id: user.clone(),
            guild_id: Some(guild.clone()),
            at,
        },
    )
    .into()
}

fn name_text<'a>(name: String, size: u32, colour: iced::Color) -> Element<'a, Message> {
    container(
        text(name)
            .size(size)
            .color(colour)
            .shaping(Shaping::Advanced)
            .wrapping(text::Wrapping::None),
    )
    .width(Length::Fill)
    .clip(true)
    .into()
}

fn classic_row<'a>(ctx: &Ctx<'a>, user_id: &'a str, status: Status) -> Element<'a, Message> {
    let name = ctx.name(user_id);
    let line = row![
        face(ctx, user_id, &name, AVATAR, status),
        name_text(name, 14, ctx.name_colour(user_id, status)),
    ]
    .spacing(10)
    .align_y(Vertical::Center);
    clickable(ctx, user_id, line.into(), [4, 8])
}

fn rich_row<'a>(
    ctx: &Ctx<'a>,
    user_id: &'a str,
    status: Status,
    activity: Option<&String>,
) -> Element<'a, Message> {
    const SIZE: f32 = 36.0;
    let name = ctx.name(user_id);
    // A ring in the status colour around the avatar.
    let ring = container(avatar_with(&name, user_id, SIZE, ctx.image(user_id)))
        .padding(2)
        .style(move |_: &iced::Theme| container::Style {
            border: iced::border::rounded(SIZE / 2.0 + 3.0)
                .color(status_colour(status))
                .width(2),
            ..Default::default()
        });
    let mut top = row![name_text(name, 14, ctx.name_colour(user_id, status))]
        .spacing(6)
        .align_y(Vertical::Center);
    if let Some(s) = ctx.stream(user_id) {
        top = top.push(stream::live_badge(Message::OpenStreamPreview(s)));
    }
    let mut lines = column![top].spacing(2);
    if status != Status::Offline {
        if let Some(doing) = ctx.doing(user_id, status, activity) {
            lines = lines.push(
                text(doing)
                    .size(12)
                    .color(pal::muted())
                    .shaping(Shaping::Advanced)
                    .wrapping(text::Wrapping::None),
            );
        }
    }
    let line = row![ring, container(lines).width(Length::Fill).clip(true)]
        .spacing(10)
        .align_y(Vertical::Center);
    clickable(ctx, user_id, line.into(), [6, 8])
}

/// Compact: a search field and the counts, then short rows.
fn compact<'a>(ctx: &Ctx<'a>, list: &'a MemberList) -> Column<'a, Message> {
    let search = text_input(t("Search a member", "Cerca un membro"), ctx.look.filter)
        .on_input(Message::MemberFilter)
        .size(13)
        .padding([5, 8])
        .style(input);
    let counts = text(format!("{} · {} online", list.member_count, list.online_count))
        .size(11)
        .color(pal::muted());
    let mut rows =
        column![container(row![search, counts].spacing(8).align_y(Vertical::Center)).padding([4, 4]),]
            .spacing(1);
    let filter = ctx.look.filter.trim().to_lowercase();
    let mut shown = 0usize;
    for row_data in list.known() {
        match row_data {
            // While searching, only the people found, without groups.
            Row::Group { id, count } if filter.is_empty() => {
                let dot = match id.as_str() {
                    "online" => Some(pal::green()),
                    "offline" => Some(pal::muted()),
                    _ => ctx
                        .store
                        .role(ctx.guild_id, id)
                        .map(|r| r.color)
                        .filter(|c| *c != 0)
                        .map(colour),
                };
                rows = rows.push(group_header(
                    format!(
                        "{} · {count}",
                        ctx.store.group_name(ctx.guild_id, id).to_uppercase()
                    ),
                    dot,
                ));
            }
            Row::Member { user_id, status, .. } => {
                let name = ctx.name(user_id);
                if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                    continue;
                }
                shown += 1;
                rows = rows.push(compact_row(ctx, user_id, name, *status));
            }
            _ => {}
        }
    }
    if shown == 0 {
        rows = rows.push(note_line(if filter.is_empty() {
            t("No members to show", "Nessun membro da mostrare")
        } else {
            t(
                "Nobody found among the loaded members",
                "Nessuno trovato tra i membri caricati",
            )
        }));
    }
    rows
}

fn compact_row<'a>(ctx: &Ctx<'a>, user_id: &'a str, name: String, status: Status) -> Element<'a, Message> {
    let colour = ctx.name_colour(user_id, status);
    let mut line = row![
        face(ctx, user_id, &name, 22.0, status),
        name_text(name, 13, colour)
    ]
    .spacing(8)
    .align_y(Vertical::Center);
    if let Some(s) = ctx.stream(user_id) {
        line = line.push(stream::live_badge(Message::OpenStreamPreview(s)));
    } else if ctx.store.voice_channel_name(ctx.guild_id, user_id).is_some() {
        line = line.push(text(t("in voice", "in voce")).size(11).color(pal::muted()));
    } else if let Some(s) = status_text(status) {
        line = line.push(text(s).size(11).color(pal::muted()));
    }
    clickable(ctx, user_id, line.into(), [3, 6])
}

/// By activity: voice channels, who is active here, online, offline.
fn by_activity<'a>(ctx: &Ctx<'a>, list: &'a MemberList) -> Column<'a, Message> {
    let store = ctx.store;
    let mut col = column![].spacing(8);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut sections = 0usize;

    // Who is in a voice channel of this server, by channel.
    let mut channels: Vec<(&str, Vec<&str>)> = Vec::new();
    for (user, (guild, channel)) in &store.voice {
        if guild != ctx.guild_id {
            continue;
        }
        match channels.iter_mut().find(|(c, _)| c == channel) {
            Some((_, users)) => users.push(user.as_str()),
            None => channels.push((channel.as_str(), vec![user.as_str()])),
        }
    }
    channels.sort_by_key(|(c, _)| store.channel_names.get(*c).map(|n| n.to_lowercase()));
    for (channel, mut users) in channels {
        users.sort_by_cached_key(|u| ctx.name(u).to_lowercase());
        let title = format!(
            "{} {}",
            t("In voice ·", "In voce ·"),
            store.channel_names.get(channel).map_or("…", String::as_str)
        );
        let mut card = column![row![
            icons::icon(&icons::HEADPHONES, 14.0, pal::green()),
            text(title).size(12).font(BOLD).shaping(Shaping::Advanced),
        ]
        .spacing(6)
        .align_y(Vertical::Center)]
        .spacing(2);
        for user in users {
            seen.insert(user);
            let name = ctx.name(user);
            let mut line = row![
                avatar_with(&name, user, 24.0, ctx.image(user)),
                name_text(name, 13, ctx.name_colour(user, Status::Online)),
            ]
            .spacing(8)
            .align_y(Vertical::Center);
            if let Some(s) = ctx.stream(user) {
                line = line.push(stream::live_badge(Message::OpenStreamPreview(s)));
            }
            let flags = store.voice_status.get(user).copied().unwrap_or_default();
            if let Some(m) = super::voice::marks(flags, ctx.look.muted_for_us.contains(user), 13.0) {
                line = line.push(m);
            }
            card = card.push(clickable(ctx, user, line.into(), [3, 4]));
        }
        sections += 1;
        col = col.push(
            container(card)
                .padding([8, 8])
                .width(Length::Fill)
                .style(rounded(pal::bg(), 8.0)),
        );
    }

    // Status of everyone in the list, to place the others.
    let mut status: HashMap<&str, Status> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for r in list.known() {
        if let Row::Member {
            user_id, status: s, ..
        } = r
        {
            if status.insert(user_id.as_str(), *s).is_none() {
                order.push(user_id.as_str());
            }
        }
    }

    // Who is typing here, then who wrote here recently.
    if let Some(channel) = ctx.look.channel_id {
        let typing = store.typing_ids(channel);
        let mut active: Vec<(&str, bool)> = typing.iter().map(|u| (*u, true)).collect();
        for u in store.recent_authors(channel, RECENT) {
            if !active.iter().any(|(a, _)| *a == u) {
                active.push((u, false));
            }
        }
        active.retain(|(u, _)| !seen.contains(u) && store.member(ctx.guild_id, u).is_some());
        if !active.is_empty() {
            let mut section = column![group_header(t("ACTIVE HERE", "ATTIVI QUI").into(), None)].spacing(1);
            for (user, is_typing) in active {
                seen.insert(user);
                let s = status.get(user).copied().unwrap_or(Status::Online);
                let name = ctx.name(user);
                let what = if is_typing {
                    t("Typing…", "Sta scrivendo…")
                } else {
                    t("Wrote recently", "Ha scritto da poco")
                };
                let line = row![
                    face(ctx, user, &name, AVATAR, s),
                    column![
                        name_text(name, 14, ctx.name_colour(user, s)),
                        text(what).size(12).color(pal::muted()),
                    ]
                    .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Vertical::Center);
                section = section.push(clickable(ctx, user, line.into(), [4, 8]));
            }
            sections += 1;
            col = col.push(section);
        }
    }

    // Online, then offline folded.
    let online: Vec<&str> = order
        .iter()
        .copied()
        .filter(|u| !seen.contains(u) && status.get(u) != Some(&Status::Offline))
        .collect();
    let offline: Vec<&str> = order
        .iter()
        .copied()
        .filter(|u| !seen.contains(u) && status.get(u) == Some(&Status::Offline))
        .collect();
    if !online.is_empty() {
        let mut section = column![group_header(format!("ONLINE — {}", online.len()), None)].spacing(1);
        for user in online {
            section = section.push(classic_row(ctx, user, status[user]));
        }
        sections += 1;
        col = col.push(section);
    }
    if !offline.is_empty() {
        let mut faces = row![].spacing(2);
        for user in offline.iter().take(OFFLINE_FACES) {
            let name = ctx.name(user);
            faces = faces.push(avatar_with(&name, user, 22.0, ctx.image(user)));
        }
        let arrow = if ctx.look.offline_open { "▴" } else { "▾" };
        // The label first and on one line; the faces take what is left
        // (cut if the column is narrow), so the count never wraps.
        let fold = button(
            row![
                text(format!("Offline — {}", offline.len()))
                    .size(12)
                    .color(pal::muted())
                    .wrapping(iced::widget::text::Wrapping::None),
                container(faces).width(Length::Fill).clip(true),
                text(arrow).size(12).color(pal::muted()),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        )
        .width(Length::Fill)
        .padding([8, 8])
        .style(list_item(false))
        .on_press(Message::ToggleOfflineMembers);
        let mut section = column![fold].spacing(1);
        if ctx.look.offline_open {
            for user in offline {
                section = section.push(classic_row(ctx, user, Status::Offline));
            }
        }
        sections += 1;
        col = col.push(section);
    }
    if sections == 0 {
        col = col.push(note_line(t("No members to show", "Nessun membro da mostrare")));
    }
    col
}

/// Every row asked has arrived (so there may be more), and more pages can
/// be asked.
fn more_to_load(list: &MemberList, loaded_pages: usize) -> bool {
    loaded_pages < member_list::MAX_PAGES && list.rows.len() >= loaded_pages * member_list::PAGE
}

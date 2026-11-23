//! The main window's frame: top bar, server column, channel panel, member column and status bar.

use super::*;
use iced::widget::column;

impl Gui {
    pub(super) fn view_main(&self) -> Element<'_, Message> {
        column![self.top_bar(), hline(pal::border())]
            .push(self.update.banner())
            .push(self.main_body())
            .into()
    }

    pub(super) fn main_body(&self) -> Element<'_, Message> {
        column![
            row![
                self.server_rail(),
                vline(pal::soft()),
                self.channel_panel(),
                vline(pal::border()),
                self.chat(),
            ]
            .push(self.member_column())
            .height(Length::Fill),
            hline(pal::border()),
            self.status_bar(),
        ]
        .into()
    }

    /// The member list on the right, in a server channel when it is shown.
    pub(super) fn member_column(&self) -> Option<Element<'_, Message>> {
        if !self.settings.member_list {
            return None;
        }
        let channel = self.store.current.as_ref()?;
        let guild = self.store.channel_guild.get(channel)?;
        let pages = self.member_list_sub.as_ref().map_or(1, |(_, _, p)| *p);
        Some(
            row![
                vline(pal::border()),
                members::view(
                    &self.store,
                    guild,
                    &self.small_images,
                    pages,
                    members::Look {
                        style: self.settings.member_style,
                        filter: &self.member_filter,
                        offline_open: self.offline_open,
                        channel_id: self.store.current.as_deref(),
                        muted_for_us: &self.settings.audio.muted_users,
                    },
                )
            ]
            .into(),
        )
    }

    pub(super) fn top_bar(&self) -> Element<'_, Message> {
        let mut crumbs = row![text("Rico").font(BOLD)]
            .spacing(10)
            .align_y(Vertical::Center);
        if self.friends.open && self.store.current.is_none() {
            crumbs = crumbs
                .push(
                    text(t("Direct messages", "Messaggi diretti"))
                        .size(13)
                        .color(pal::muted()),
                )
                .push(text("/").size(13).color(pal::muted()))
                .push(text(t("Friends", "Amici")).size(13));
        }
        if let Some(c) = &self.store.current {
            let place = match self.store.guild_of(c) {
                Some(g) => g.name().to_string(),
                None => t("Direct messages", "Messaggi diretti").to_string(),
            };
            crumbs = crumbs
                .push(
                    text(place)
                        .size(13)
                        .color(pal::muted())
                        .shaping(Shaping::Advanced),
                )
                .push(text("/").size(13).color(pal::muted()))
                .push(
                    text(self.store.channel_label(c))
                        .size(13)
                        .shaping(Shaping::Advanced),
                );
        }
        let me = self.store.me.as_ref();
        let mut right = row![].spacing(10).align_y(Vertical::Center);
        if let Some(u) = me {
            let image = self
                .store
                .avatar_url(None, &u.id, AVATAR_PX)
                .and_then(|a| self.small_images.get(&a));
            right = right.push(avatar_button(
                u.display_name(),
                &u.id,
                22.0,
                image,
                Message::OpenProfile {
                    user_id: u.id.clone(),
                    guild_id: None,
                },
            ));
        }
        right = right.push(
            tooltip(
                button(text(i18n::lang().code().to_ascii_uppercase()).size(12))
                    .on_press(Message::ToggleLanguage)
                    .padding([3, 8])
                    .style(ghost),
                container(text(t("Language: English", "Lingua: italiano")).size(12))
                    .padding(6)
                    .style(style::tooltip),
                tooltip::Position::Bottom,
            )
            .gap(4),
        );
        if !self.demo {
            right = right.push(
                button(text(t("Sign out", "Esci")).size(12))
                    .on_press(Message::Logout)
                    .padding([3, 10])
                    .style(ghost),
            );
        }
        container(row![crumbs, space::horizontal(), right].align_y(Vertical::Center))
            .padding([0, 12])
            .height(36)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(pal::title()))
            .into()
    }

    pub(super) fn server_rail(&self) -> Element<'_, Message> {
        let mut col = Column::new()
            .spacing(10)
            .padding(Padding {
                top: 12.0,
                right: 0.0,
                bottom: 12.0,
                left: 0.0,
            })
            .align_x(Horizontal::Center)
            .width(Length::Fill);
        col = col.push(rail_button(
            "DM".to_string(),
            t("Direct messages", "Messaggi diretti").to_string(),
            self.guild.is_none(),
            self.store.dms_have_unread(),
            self.store.dm_mentions(),
            None,
            Message::SelectGuild(None),
        ));
        col = col.push(container(Space::new().width(24).height(1)).style(fill(pal::border())));
        let guild_button = |i: usize| -> Element<'_, Message> {
            let g = &self.store.guilds[i];
            let guild_id = g.id.clone();
            context::RightClick::new(
                rail_button(
                    initials(g.name()),
                    g.name().to_string(),
                    self.guild == Some(i),
                    self.store.guild_has_unread(&g.id),
                    self.store.guild_mentions(&g.id),
                    media::guild_icon_url(&g.id, g.icon(), ICON_PX).and_then(|u| self.small_images.get(&u)),
                    Message::SelectGuild(Some(i)),
                ),
                move |at| Message::OpenServerMenu {
                    guild_id: guild_id.clone(),
                    at,
                },
            )
            .into()
        };
        for entry in self.store.rail() {
            match entry {
                crate::store::RailEntry::Guild(i) => col = col.push(guild_button(*i)),
                crate::store::RailEntry::Folder { id, .. } => {
                    col = col.push(folders::rail_folder(
                        &self.store,
                        &self.small_images,
                        entry,
                        self.open_folders.contains(id),
                        self.guild,
                        guild_button,
                    ));
                }
            }
        }
        if !self.store.guilds.is_empty() {
            col = col.push(voice::control_button(
                &icons::FOLDER,
                Control::On,
                t(
                    "Server folders (also right click on a folder)",
                    "Cartelle dei server (anche clic destro su una cartella)",
                )
                .into(),
                Some(Message::OpenFolderEditor),
                32.0,
            ));
        }
        container(scrollable(col).direction(thin()).style(thin_scrollbar))
            .width(64)
            .height(Length::Fill)
            .style(fill(pal::panel()))
            .into()
    }

    pub(super) fn channel_panel(&self) -> Element<'_, Message> {
        let title = match self.guild.and_then(|i| self.store.guilds.get(i)) {
            Some(g) => g.name().to_string(),
            None => t("Direct messages", "Messaggi diretti").to_string(),
        };
        let header = container(text(title).font(BOLD).shaping(Shaping::Advanced))
            .padding([0, 12])
            .height(40)
            .width(Length::Fill)
            .align_y(Vertical::Center);

        let mut list = Column::new().spacing(1).padding([8, 6]);
        if self.guild.is_none() {
            list = list
                .push(friends::entry(
                    self.friends.open && self.store.current.is_none(),
                    self.store.incoming_requests(),
                ))
                .push(container(Space::new().width(Length::Fill).height(1)).style(fill(pal::soft())));
        }
        let rows = self.store.channel_rows(self.guild);
        if rows.is_empty() {
            let msg = if self.conn == Conn::Online || self.demo {
                t("No channels", "Nessun canale")
            } else {
                t("Loading…", "Caricamento…")
            };
            list = list.push(container(text(msg).size(13).color(pal::muted())).padding(8));
        }
        for r in rows {
            match r {
                ChannelRow::Category(name) => {
                    list = list.push(
                        container(
                            text(name.to_uppercase())
                                .size(11)
                                .font(BOLD)
                                .color(pal::muted())
                                .shaping(Shaping::Advanced),
                        )
                        .padding(Padding {
                            top: 12.0,
                            right: 4.0,
                            bottom: 4.0,
                            left: 6.0,
                        }),
                    );
                }
                ChannelRow::Channel {
                    id,
                    name,
                    kind: k,
                    selectable,
                } => {
                    let selected = self.store.current.as_deref() == Some(id.as_str());
                    let unread = self.store.unread.contains(&id);
                    let mut label = text(name.clone()).size(13).shaping(Shaping::Advanced);
                    if unread {
                        label = label.font(BOLD);
                    } else if !selected {
                        label = label.color(pal::muted());
                    }
                    // Private conversations: the other person's avatar (or the
                    // group's picture), small, instead of the "@".
                    let dm = self
                        .guild
                        .is_none()
                        .then(|| self.store.dms.iter().find(|c| c.id == id))
                        .flatten();
                    let is_voice = k == kind::GUILD_VOICE || k == kind::STAGE;
                    // A voice channel we can see but not join: a padlock, as on Discord.
                    let locked = is_voice && !self.store.can_connect(&id);
                    let icon: Element<'_, Message> = match dm {
                        Some(c) => {
                            let (colour_id, url) = self.store.dm_picture(c, AVATAR_PX);
                            let image = url.and_then(|u| self.small_images.get(&u));
                            avatar_with(&name, &colour_id, DM_AVATAR, image)
                        }
                        None => text(if locked {
                            "🔒"
                        } else {
                            crate::text::channel_symbol(k)
                        })
                        .size(13)
                        .color(pal::muted())
                        .width(16)
                        .shaping(Shaping::Advanced)
                        .into(),
                    };
                    let mut line = row![icon, label]
                        .spacing(if dm.is_some() { 8 } else { 4 })
                        .align_y(Vertical::Center);
                    let mentions = self.store.mentions(&id);
                    if mentions > 0 {
                        line = line.push(space::horizontal()).push(mention_badge(mentions));
                    } else if unread {
                        line = line.push(space::horizontal()).push(dot(pal::accent(), 6.0));
                    }
                    let in_voice = self.voice.as_ref().is_some_and(|v| v.channel_id == id);
                    let press = if selectable {
                        Some(Message::OpenChannel(id.clone()))
                    } else if is_voice && !locked {
                        Some(Message::JoinVoice(id.clone()))
                    } else {
                        None
                    };
                    let item = button(line)
                        .width(Length::Fill)
                        .padding([4, 8])
                        .style(list_item(selected || in_voice))
                        .on_press_maybe(press);
                    list = list.push(item);
                    // Who is in the voice channel, with who is speaking in green.
                    if is_voice {
                        for (user_id, name) in self.store.voice_members(&id) {
                            let speaking = self
                                .voice
                                .as_ref()
                                .is_some_and(|v| v.channel_id == id && v.speaking.contains(&user_id));
                            let mut label = text(name.clone()).size(12).shaping(Shaping::Advanced);
                            label = if speaking {
                                label.color(pal::green()).font(SEMIBOLD)
                            } else {
                                label.color(pal::muted())
                            };
                            let guild = self.store.channel_guild.get(&id).cloned();
                            let image = self
                                .store
                                .avatar_url(guild.as_deref(), &user_id, AVATAR_PX)
                                .and_then(|a| self.small_images.get(&a));
                            let mut member = row![avatar_with(&name, &user_id, 18.0, image), label]
                                .spacing(6)
                                .align_y(Vertical::Center);
                            if speaking {
                                member = member.push(dot(pal::green(), 6.0));
                            }
                            let flags = self.store.voice_status.get(&user_id).copied().unwrap_or_default();
                            let local = self.settings.audio.muted_users.contains(&user_id);
                            if let Some(m) = voice::marks(flags, local, 13.0) {
                                member = member.push(space::horizontal()).push(m);
                            }
                            // Streaming (Go Live): the LIVE badge opens the preview.
                            let live = guild
                                .as_ref()
                                .filter(|_| self.store.streaming.contains(&user_id))
                                .map(|g| stream::StreamRef {
                                    guild_id: g.clone(),
                                    channel_id: id.clone(),
                                    user_id: user_id.clone(),
                                });
                            if let Some(s) = &live {
                                if flags.marks().is_empty() && !local {
                                    member = member.push(space::horizontal());
                                }
                                member =
                                    member.push(stream::live_badge(Message::OpenStreamPreview(s.clone())));
                            }
                            let (menu_user, menu_guild) = (user_id.clone(), guild.clone());
                            let profile = Message::OpenProfile {
                                user_id: user_id.clone(),
                                guild_id: guild,
                            };
                            // Highlighted on hover like the channels. A click opens
                            // the profile; on someone streaming it watches them, the
                            // mouse over them shows the small preview (like Discord)
                            // and the right click opens the person's menu.
                            let row_button = button(member)
                                .width(Length::Fill)
                                .padding([2, 8])
                                .style(list_item(false));
                            let entry: Element<'_, Message> = match live {
                                Some(s) => {
                                    let preview = media::stream_key(&s.guild_id, &s.channel_id, &s.user_id)
                                        .and_then(|k| self.hover_previews.get(&k));
                                    let in_channel = self.voice.as_ref().is_some_and(|v| v.channel_id == id);
                                    let card = stream::hover_card(
                                        &self.store,
                                        &s,
                                        preview,
                                        &self.small_images,
                                        in_channel,
                                    );
                                    iced::widget::tooltip(
                                        mouse_area(row_button.on_press(Message::WatchStreamOf(s.clone())))
                                            .on_enter(Message::HoverStream(s)),
                                        card,
                                        iced::widget::tooltip::Position::Right,
                                    )
                                    .gap(8)
                                    .into()
                                }
                                None => row_button.on_press(profile).into(),
                            };
                            let entry = context::RightClick::new(entry, move |at| Message::OpenUserMenu {
                                user_id: menu_user.clone(),
                                guild_id: menu_guild.clone(),
                                at,
                            });
                            list = list.push(container(entry).padding(Padding {
                                left: 22.0,
                                ..Padding::ZERO
                            }));
                        }
                    }
                }
            }
        }

        // Our status: the dot on the avatar, and under the name the custom
        // status (or the status) once connected.
        let my_status = self.store.my_status.as_ref();
        let my_presence = my_status
            .map(crate::discord::folders::StatusSettings::presence)
            .unwrap_or_default();
        let my_line: String = match &self.conn {
            Conn::Online => my_status
                .and_then(|s| s.custom.as_ref())
                .filter(|c| c.active(now_ms()))
                .map_or_else(
                    || my_presence.label().to_string(),
                    |c| {
                        let text: String = c.text.chars().take(28).collect();
                        if c.text.chars().count() > 28 {
                            format!("{text}…")
                        } else {
                            text
                        }
                    },
                ),
            Conn::Connecting => t("connecting…", "connessione…").into(),
            Conn::Error(_) => t("disconnected", "disconnesso").into(),
        };
        let user_panel: Element<'_, Message> = match &self.store.me {
            Some(me) => container(
                row![
                    button(
                        row![
                            stack![
                                avatar_with(
                                    me.display_name(),
                                    &me.id,
                                    28.0,
                                    self.store
                                        .avatar_url(None, &me.id, AVATAR_PX)
                                        .and_then(|a| self.small_images.get(&a)),
                                ),
                                container(
                                    container(presence::dot(my_presence, 9.0))
                                        .padding(2)
                                        .style(rounded(pal::panel(), 7.0)),
                                )
                                .width(32)
                                .height(32)
                                .align_x(Horizontal::Right)
                                .align_y(Vertical::Bottom),
                            ],
                            column![
                                text(me.display_name().to_string())
                                    .size(13)
                                    .font(SEMIBOLD)
                                    .shaping(Shaping::Advanced),
                                text(my_line)
                                    .size(11)
                                    .color(pal::muted())
                                    .shaping(Shaping::Advanced)
                                    .wrapping(iced::widget::text::Wrapping::None),
                            ]
                            .spacing(1),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([4, 6])
                    .style(list_item(self.status_menu.is_some()))
                    .on_press(Message::OpenStatusMenu),
                    voice::control_button(
                        &icons::GEAR,
                        Control::On,
                        t("Settings (Ctrl + ,)", "Impostazioni (Ctrl + ,)").into(),
                        Some(Message::OpenSettings),
                        30.0,
                    ),
                ]
                .spacing(4)
                .align_y(Vertical::Center),
            )
            .padding([0, 6])
            .height(52)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .into(),
            None => Space::new().width(Length::Fill).height(52).into(),
        };

        let voice_panel: Element<'_, Message> = match &self.voice {
            Some(v) => {
                let name = self
                    .store
                    .channel_names
                    .get(&v.channel_id)
                    .cloned()
                    .unwrap_or_else(|| t("voice channel", "canale vocale").into());
                let can_speak = self.store.can(&v.channel_id, permissions::SPEAK);
                let can_stream = self.store.can(&v.channel_id, permissions::STREAM);
                column![
                    voice::panel(v, name, can_speak, can_stream, self.golive.as_ref()),
                    hline(pal::soft())
                ]
                .into()
            }
            None => Space::new().width(0).height(0).into(),
        };

        container(column![
            header,
            hline(pal::soft()),
            scrollable(list)
                .direction(thin())
                .style(thin_scrollbar)
                .height(Length::Fill),
            hline(pal::soft()),
            voice_panel,
            user_panel,
        ])
        .width(240)
        .height(Length::Fill)
        .style(fill(pal::panel()))
        .into()
    }

    pub(super) fn status_bar(&self) -> Element<'_, Message> {
        let (color, label) = match &self.conn {
            _ if self.demo => (style::pal::yellow(), "Demo".to_string()),
            Conn::Online => (pal::green(), t("Connected", "Connesso").to_string()),
            Conn::Connecting => (pal::yellow(), t("Connecting…", "Connessione…").to_string()),
            Conn::Error(e) => (
                pal::red(),
                tf!("Disconnected: {e} — retrying…", "Disconnesso: {e} — riprovo…"),
            ),
        };
        let mut left = row![dot(color, 8.0), text(label).size(12).color(pal::muted())]
            .spacing(6)
            .align_y(Vertical::Center);
        if let Some((msg, is_error)) = &self.status {
            left = left.push(text("·").size(12).color(pal::muted())).push(
                text(msg.clone())
                    .size(12)
                    .color(if *is_error { pal::red() } else { pal::accent() }),
            );
        }
        container(left)
            .padding([0, 10])
            .height(24)
            .width(Length::Fill)
            .align_y(Vertical::Center)
            .style(fill(pal::panel()))
            .into()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rail_button(
    label: String,
    name: String,
    selected: bool,
    unread: bool,
    mentions: u32,
    image: Option<&Preview>,
    on_press: Message,
) -> Element<'_, Message> {
    // Small bar on the left: tall if selected, a dot if there are unread messages.
    let marker_height = if selected {
        22.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    let marker = container(Space::new().width(3).height(marker_height)).style(rounded(pal::text(), 2.0));
    // The server icon, already cut to a rounded square; until it arrives (or
    // if the server has none) the initials.
    let icon = match image {
        Some(Preview::Ready(handle)) => button(img::Image::new(handle.clone()).width(40).height(40))
            .width(40)
            .height(40)
            .padding(0)
            .style(style::bare),
        _ => button(
            container(text(label).size(13).font(SEMIBOLD).shaping(Shaping::Advanced))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .width(40)
        .height(40)
        .padding(0)
        .style(server(selected)),
    }
    .on_press(on_press);
    let with_tip = tooltip(
        with_mentions(icon, mentions),
        text(name).size(12).shaping(Shaping::Advanced),
        tooltip::Position::Right,
    )
    .padding(6)
    .style(style::tooltip);
    row![marker, Space::new().width(7), with_tip, Space::new().width(10)]
        .align_y(Vertical::Center)
        .into()
}

/// The red number of mentions waiting ("99+" above 99).
pub(super) fn mention_badge<'a>(n: u32) -> Element<'a, Message> {
    let label = if n > 99 { "99+".to_string() } else { n.to_string() };
    container(text(label).size(11).font(BOLD).color(iced::Color::WHITE))
        .padding([0, 5])
        .style(rounded(pal::red(), 8.0))
        .into()
}

/// An icon of the server column with the mentions waiting at its
/// bottom-right corner, as on Discord.
pub(super) fn with_mentions<'a>(
    icon: impl Into<Element<'a, Message>>,
    mentions: u32,
) -> Element<'a, Message> {
    if mentions == 0 {
        return icon.into();
    }
    iced::widget::stack![
        container(icon).padding(Padding {
            top: 0.0,
            right: 4.0,
            bottom: 2.0,
            left: 0.0
        }),
        container(mention_badge(mentions))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom),
    ]
    .into()
}

pub(super) fn vline<'a>(color: iced::Color) -> Element<'a, Message> {
    container(Space::new().width(1).height(Length::Fill))
        .style(fill(color))
        .into()
}

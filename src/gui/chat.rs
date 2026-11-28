//! The open conversation: the message list, each message's body (text, emojis, mentions, previews, replies) and the composer area.

use super::*;
use iced::widget::column;
use std::collections::HashSet;

impl Gui {
    pub(super) fn chat(&self) -> Element<'_, Message> {
        if self.friends.open && self.guild.is_none() && self.store.current.is_none() {
            return friends::view(&self.friends, &self.store, &self.small_images);
        }
        let Some(channel_id) = self.store.current.clone() else {
            return container(
                column![
                    text(t("Welcome to Rico", "Benvenuto in Rico"))
                        .size(18)
                        .font(BOLD),
                    text(t(
                        "Pick a server on the left, then a channel.",
                        "Scegli un server a sinistra, poi un canale."
                    ))
                    .color(pal::muted()),
                ]
                .spacing(8)
                .align_x(Horizontal::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into();
        };

        let topic = match self.store.guild_of(&channel_id) {
            Some(g) => g.name().to_string(),
            None => t("Direct messages", "Messaggi diretti").to_string(),
        };
        let mut header_row = row![
            text(self.store.channel_label(&channel_id))
                .font(BOLD)
                .shaping(Shaping::Advanced),
            text(topic)
                .size(13)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        ]
        .spacing(12)
        .align_y(Vertical::Center);
        // In a server: the button that shows or hides its members.
        if self.store.channel_guild.contains_key(&channel_id) {
            let shown = self.settings.member_list;
            header_row = header_row.push(space::horizontal()).push(voice::control_button(
                &icons::PEOPLE,
                style::Control::On,
                if shown {
                    t("Hide the member list", "Nascondi l'elenco dei membri")
                } else {
                    t("Show the member list", "Mostra l'elenco dei membri")
                }
                .into(),
                Some(Message::ToggleMemberList),
                30.0,
            ));
        }
        let header = container(header_row)
            .padding([0, 16])
            .height(44)
            .width(Length::Fill)
            .align_y(Vertical::Center);

        let body: Element<'_, Message> = match self.store.messages.get(&channel_id) {
            Some(list) if !list.is_empty() => scrollable(self.message_list(list))
                .id(messages_id())
                // `direction` replaces the whole scrollbar, anchoring included:
                // it must be set before `anchor_bottom`.
                .direction(thin())
                .anchor_bottom()
                .on_scroll(Message::Scrolled)
                .style(thin_scrollbar)
                .height(Length::Fill)
                .into(),
            _ => {
                let msg = if self.store.loading.contains(&channel_id) {
                    t("Loading messages…", "Caricamento dei messaggi…")
                } else if !self.store.can(&channel_id, permissions::READ_MESSAGE_HISTORY) {
                    t(
                        "You do not have permission to read the history of this channel: new messages will appear here.",
                        "Non hai il permesso di leggere la cronologia di questo canale: qui compariranno i nuovi messaggi.",
                    )
                } else if !self.store.can_send(&channel_id) {
                    t("No messages.", "Nessun messaggio.")
                } else {
                    t(
                        "No messages: write the first one!",
                        "Nessun messaggio: scrivi il primo!",
                    )
                };
                container(text(msg).color(pal::muted()))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center)
                    .into()
            }
        };

        let placeholder = tf!(
            "Message {}",
            "Scrivi in {}",
            self.store.channel_label(&channel_id)
        );
        // Without the permission to send, a field that cannot be typed in
        // (as on Discord) and no GIF, emoji or send buttons.
        let can_send = self.store.can_send(&channel_id);
        let mut bars = Column::new().spacing(4);
        if can_send {
            if self.editing.is_some() {
                bars = bars.push(compose::edit_bar());
            } else if let Some(r) = &self.replying {
                bars = bars.push(compose::reply_bar(r));
            }
            if !self.pending_files.is_empty() {
                bars = bars.push(compose::files_row(&self.pending_files));
            }
        }
        let ready =
            !self.composer.trim().is_empty() || (!self.pending_files.is_empty() && self.editing.is_none());
        let send_label = if self.editing.is_some() {
            t("Save", "Salva")
        } else {
            t("Send", "Invia")
        };
        let can_attach = self.editing.is_none() && self.store.can_attach(&channel_id);
        let composer: Element<'_, Message> = if can_send {
            row![
                tooltip(
                    button(
                        container(icons::icon(&icons::ATTACH, 22.0, pal::muted()))
                            .center_x(26)
                            .center_y(24),
                    )
                    .on_press_maybe((can_attach && !self.picking_files).then_some(Message::PickFiles))
                    .padding([8, 8])
                    .style(style::tab(false)),
                    text(t(
                        "Attach files (or drop them here)",
                        "Allega file (o trascinali qui)"
                    ))
                    .size(12),
                    tooltip::Position::Top,
                )
                .style(style::tooltip),
                text_input(&placeholder, &self.composer)
                    .id(composer_id())
                    .on_input(Message::ComposerChanged)
                    .on_submit(Message::Send)
                    .padding(11)
                    .style(input),
                self.picker_button(&icons::GIF, picker::Tab::Gif, "GIF"),
                self.picker_button(&icons::SMILE, picker::Tab::Emoji, "Emoji"),
                button(text(send_label).font(SEMIBOLD))
                    .on_press_maybe(ready.then_some(Message::Send))
                    .padding([10, 16])
                    .style(primary),
            ]
            .spacing(8)
            .align_y(Vertical::Center)
            .into()
        } else {
            text_input(no_send_permission(), "")
                .id(composer_id())
                .padding(11)
                .style(input)
                .into()
        };

        let typing = self.store.typing_in(&channel_id);
        let typing_line = match typing.as_slice() {
            [] => String::new(),
            [one] => tf!("{one} is typing…", "{one} sta scrivendo…"),
            [a, b] => tf!("{a} and {b} are typing…", "{a} e {b} stanno scrivendo…"),
            _ => t("Several people are typing…", "Più persone stanno scrivendo…").to_string(),
        };

        column![
            header,
            hline(pal::soft()),
            body,
            container(column![bars, composer].spacing(4)).padding(Padding {
                top: 8.0,
                right: 16.0,
                bottom: 2.0,
                left: 16.0,
            }),
            container(
                text(typing_line)
                    .size(12)
                    .color(pal::muted())
                    .shaping(Shaping::Advanced)
            )
            .padding([0, 20])
            .height(22),
        ]
        .width(Length::Fill)
        .into()
    }

    pub(super) fn message_list<'a>(&'a self, list: &'a [DMessage]) -> Element<'a, Message> {
        let ctx = self.store.text_ctx(self.store.current.as_deref());
        let mut col = Column::new().padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 8.0,
            left: 0.0,
        });
        let guild = self
            .store
            .current
            .as_deref()
            .and_then(|c| self.store.channel_guild.get(c))
            .map(String::as_str);
        let mut prev_author: Option<&str> = None;
        let mut prev_time: Option<DateTime<Local>> = None;
        let mut prev_day: Option<NaiveDate> = None;
        let channel_id = self.store.current.as_deref().unwrap_or_default();
        // Clicking an existing reaction only needs to see the history.
        let toggle_reactions = self.store.can(
            channel_id,
            permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY,
        );

        for m in list {
            let ts = crate::text::parse_ts(&m.timestamp);
            let day = ts.map(|t| t.date_naive());
            if let Some(d) = day.filter(|d| Some(*d) != prev_day) {
                col = col.push(day_separator(d));
                prev_author = None;
            }
            let grouped = prev_author == Some(m.author.id.as_str())
                && m.referenced_message.is_none()
                && matches!((prev_time, ts), (Some(a), Some(b)) if (b - a).num_seconds() < GROUP_GAP_SECS);

            let mentions_me = ctx.mentions_me(m);
            let content = message_body(
                m,
                &ctx,
                &self.settings,
                &self.previews,
                &self.small_images,
                &self.revealed,
                toggle_reactions,
            );
            let time = ts
                .map(|t| t.format(self.settings.time_format()).to_string())
                .unwrap_or_default();

            let inner: Element<'a, Message> = if self.settings.compact {
                // One line: time, name, text; no avatar.
                let name = self.store.name_in(guild, &m.author);
                let open_profile = Message::OpenProfile {
                    user_id: m.author.id.clone(),
                    guild_id: guild.map(str::to_string),
                };
                let line = row![
                    text(time).size(11).color(pal::muted()).width(COMPACT_TIME),
                    person_menu(
                        button(
                            text(name)
                                .font(SEMIBOLD)
                                .shaping(Shaping::Advanced)
                                .color(user_color(&m.author.id))
                        )
                        .padding([0.0, NAME_PAD])
                        .style(name_link)
                        .on_press(open_profile),
                        &m.author.id,
                        guild,
                    ),
                    content,
                ]
                .spacing(6)
                .align_y(Vertical::Top);
                match &m.referenced_message {
                    Some(r) => column![
                        row![Space::new().width(COMPACT_TIME + 6.0), reply_preview(r, &ctx)],
                        line
                    ]
                    .spacing(2)
                    .into(),
                    None => line.into(),
                }
            } else if grouped {
                // Same left edge as the text of a message with its author.
                row![Space::new().width(32.0 + 4.0 + AUTHOR_GAP + NAME_PAD), content].into()
            } else {
                let name = self.store.name_in(guild, &m.author);
                let open_profile = Message::OpenProfile {
                    user_id: m.author.id.clone(),
                    guild_id: guild.map(str::to_string),
                };
                // The name is a button highlighted on hover (like the channels),
                // with NAME_PAD on the sides: the rest of the column is moved by
                // the same amount so the text stays aligned with the name.
                let indent = |e: Element<'a, Message>| {
                    container(e).padding(Padding {
                        left: NAME_PAD,
                        ..Padding::ZERO
                    })
                };
                let mut right = Column::new().spacing(2);
                if let Some(r) = &m.referenced_message {
                    right = right.push(indent(reply_preview(r, &ctx)));
                }
                right = right
                    .push(
                        row![
                            person_menu(
                                button(
                                    text(name.clone())
                                        .font(SEMIBOLD)
                                        .shaping(Shaping::Advanced)
                                        .color(user_color(&m.author.id))
                                )
                                .padding([1.0, NAME_PAD])
                                .style(name_link)
                                .on_press(open_profile.clone()),
                                &m.author.id,
                                guild,
                            ),
                            text(time).size(11).color(pal::muted()),
                        ]
                        .spacing(8)
                        .align_y(Vertical::Center),
                    )
                    .push(indent(content));
                let image = self
                    .store
                    .avatar_url(guild, &m.author.id, AVATAR_PX)
                    .and_then(|a| self.small_images.get(&a));
                let picture = person_menu(
                    avatar_button(&name, &m.author.id, 32.0, image, open_profile),
                    &m.author.id,
                    guild,
                );
                row![picture, right].spacing(AUTHOR_GAP).into()
            };

            let hovered = self.hovered.as_deref() == Some(m.id.as_str());
            let background = if self.highlight.as_deref() == Some(m.id.as_str()) {
                Some(pal::selected())
            } else if mentions_me {
                Some(pal::mention_bg())
            } else if hovered {
                Some(pal::panel())
            } else {
                None
            };
            let top = if self.settings.compact {
                2.0
            } else if grouped {
                1.0
            } else {
                8.0
            };
            let item = container(inner)
                .id(jump::container_id(&m.id))
                .width(Length::Fill)
                .padding(Padding {
                    top,
                    right: 12.0,
                    bottom: 2.0,
                    left: 16.0,
                })
                .style(move |_| container::Style {
                    background: background.map(iced::Background::Color),
                    ..Default::default()
                });
            // The actions only over the message under the mouse (or the one
            // waiting for the delete confirmation): computed for one message.
            let confirming = self.confirm_delete.as_deref() == Some(m.id.as_str());
            let actions = (hovered || confirming)
                .then(|| {
                    let allowed = compose::Allowed {
                        react: self.store.can(
                            channel_id,
                            permissions::VIEW_CHANNEL
                                | permissions::READ_MESSAGE_HISTORY
                                | permissions::ADD_REACTIONS,
                        ),
                        reply: self.store.can_reply(channel_id),
                        edit: self.store.can_edit(channel_id, &m.id),
                        delete: self.store.can_delete(channel_id, &m.id),
                    };
                    compose::actions(&m.id, allowed, confirming)
                })
                .flatten();
            // Always a stack, so the structure does not change on hover.
            let overlay: Element<'a, Message> = match actions {
                Some(a) => container(a)
                    .width(Length::Fill)
                    .align_x(Horizontal::Right)
                    .padding(Padding {
                        top: 0.0,
                        right: 16.0,
                        bottom: 0.0,
                        left: 0.0,
                    })
                    .into(),
                None => Space::new().width(0).height(0).into(),
            };
            let id = m.id.clone();
            col = col.push(context::RightClick::new(
                mouse_area(stack![item, overlay])
                    .on_enter(Message::HoverStart(m.id.clone()))
                    .on_exit(Message::HoverEnd(m.id.clone())),
                move |at| Message::OpenMessageMenu(id.clone(), at),
            ));

            prev_author = Some(m.author.id.as_str());
            prev_time = ts;
            prev_day = day;
        }
        col.into()
    }

    /// GIF and emoji buttons: line icons like the rest of the app.
    pub(super) fn picker_button<'a>(
        &self,
        handle: &std::sync::LazyLock<iced::widget::svg::Handle>,
        tab: picker::Tab,
        tip: &'a str,
    ) -> Element<'a, Message> {
        let active = self.picker.as_ref().is_some_and(|p| p.tab == tab);
        // On the accent background of an open picker, the dark of the window.
        let color = if active { pal::bg() } else { pal::muted() };
        tooltip(
            button(
                container(icons::icon(handle, 22.0, color))
                    .center_x(26)
                    .center_y(24),
            )
            .on_press(Message::TogglePicker(tab))
            .padding([8, 8])
            .style(style::tab(active)),
            text(tip).size(12),
            tooltip::Position::Top,
        )
        .style(style::tooltip)
        .into()
    }
}

/// A person's name or avatar: a right click opens their menu.
pub(super) fn person_menu<'a>(
    content: impl Into<Element<'a, Message>>,
    user_id: &str,
    guild_id: Option<&str>,
) -> Element<'a, Message> {
    let (user_id, guild_id) = (user_id.to_string(), guild_id.map(str::to_string));
    context::RightClick::new(content, move |at| Message::OpenUserMenu {
        user_id: user_id.clone(),
        guild_id: guild_id.clone(),
        at,
    })
    .into()
}

pub(super) fn no_send_permission() -> &'static str {
    t(
        "You do not have permission to send messages in this channel.",
        "Non hai il permesso di inviare messaggi in questo canale.",
    )
}

pub(super) fn day_separator<'a>(day: NaiveDate) -> Element<'a, Message> {
    let label = crate::text::day_label(day);
    container(
        row![
            hline(pal::soft()),
            text(label).size(12).color(pal::muted()),
            hline(pal::soft())
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding(Padding {
        top: 10.0,
        right: 4.0,
        bottom: 2.0,
        left: 16.0,
    })
    .into()
}

pub(super) fn reply_preview<'a>(
    r: &crate::discord::models::MessagePreview,
    ctx: &Ctx,
) -> Element<'a, Message> {
    let who = r
        .author
        .as_ref()
        .map_or("?", crate::discord::models::User::display_name)
        .to_string();
    let full = crate::text::render_content(&r.content, ctx).replace('\n', " ");
    let mut content: String = full.chars().take(90).collect();
    if content.len() < full.len() {
        content.push('…');
    }
    let line = row![
        text("↳").size(12).color(pal::muted()).shaping(Shaping::Advanced),
        text(format!("{who}: {content}"))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
    ]
    .spacing(6);
    // A click goes to the message (when Discord gave its id).
    let jump = (!r.id.is_empty()).then(|| Message::JumpToMessage(r.id.clone()));
    button(line)
        .on_press_maybe(jump)
        .padding([0, 4])
        .style(name_link)
        .into()
}

pub(super) fn message_body<'a>(
    m: &DMessage,
    ctx: &Ctx,
    prefs: &Settings,
    previews: &'a HashMap<String, Preview>,
    small_images: &'a HashMap<String, Preview>,
    revealed: &HashSet<String>,
    toggle_reactions: bool,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(6);
    let text_spoilers = Spoilers::new(&m.id, "t", revealed);
    // Without images, embeds are not shown (their link stays in the text).
    let embeds: Vec<media::EmbedPreview> = if prefs.show_images {
        m.embeds.iter().filter_map(media::embed_preview).collect()
    } else {
        Vec::new()
    };
    // As on Discord: if the message is only a GIF link, only the GIF is shown.
    let only_link = embeds
        .iter()
        .any(|e| e.link.as_deref() == Some(m.content.trim()) && !m.content.trim().is_empty());

    if !m.content.is_empty() && !only_link {
        let segs = segments(&m.content, ctx);
        col = col.push(rich_segments(
            segs,
            m.edited_timestamp.is_some(),
            small_images,
            ctx.guild_id,
            &text_spoilers,
            14.0,
        ));
    } else if m.attachments.is_empty() && m.embeds.is_empty() {
        let note = if m.kind == 0 {
            t("(empty message)", "(messaggio vuoto)")
        } else {
            t("(system message)", "(messaggio di sistema)")
        };
        col = col.push(text(note).size(13).color(pal::muted()));
    }

    for a in &m.attachments {
        let spoiler = Spoilers::new(&m.id, &format!("a{}", a.id), revealed);
        if a.is_image() && prefs.show_images {
            if a.is_spoiler() && spoiler.hidden(0) {
                let (w, h) = media::display_size(a.width.unwrap_or(1), a.height.unwrap_or(1));
                col = col.push(spoiler_box(w, h, spoiler.key(0)));
                continue;
            }
            let state = preview_key(a).and_then(|k| previews.get(&k));
            if !matches!(state, Some(Preview::Failed)) {
                col = col.push(image_preview(a, state));
                continue;
            }
        }
        let name: Element<'a, Message> = if a.is_spoiler() && spoiler.hidden(0) {
            button(text(t("Spoiler: click to show", "Spoiler: clic per mostrare")).size(13))
                .padding(0)
                .style(name_link)
                .on_press(Message::RevealSpoiler(spoiler.key(0)))
                .into()
        } else {
            text(a.filename.clone())
                .size(13)
                .color(pal::accent())
                .shaping(Shaping::Advanced)
                .into()
        };
        col = col.push(
            container(
                row![
                    text(t("Attachment", "Allegato")).size(11).color(pal::muted()),
                    name,
                    text(human_size(a.size)).size(11).color(pal::muted()),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            )
            .padding([8, 12])
            .style(outlined),
        );
    }

    // A link written in a spoiler: its preview stays hidden with it.
    let link_hidden = |link: Option<&str>| {
        link.and_then(|l| crate::text::link_spoiler(&m.content, l))
            .filter(|n| text_spoilers.hidden(*n))
    };
    for e in &embeds {
        let (w, h) = media::display_size(e.width, e.height);
        if let Some(n) = link_hidden(e.link.as_deref()) {
            col = col.push(spoiler_box(w, h, text_spoilers.key(n)));
            continue;
        }
        let state = embed_source(e, previews).and_then(|k| previews.get(k));
        let caption: Element<'a, Message> = match &e.provider {
            Some(name) => text(name.clone())
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced)
                .into(),
            None => Space::new().width(0).height(0).into(),
        };
        // Opened and saved as what is shown (a GIF's clip stays animated).
        let open = embed_source(e, previews)
            .map(|u| Message::OpenViewer(Box::new(media::link_image(u, e.width, e.height))));
        col = col.push(column![media_box(state, w, h, open), caption].spacing(4));
    }

    // Pages' previews and bots' cards (images allowed or not: their text
    // is text; their pictures only with images on).
    for (i, card) in m.embeds.iter().filter_map(media::embed_card).enumerate() {
        if let Some(n) = link_hidden(card.link.as_deref()) {
            col = col.push(spoiler_box(
                media::PREVIEW_MAX_W as f32,
                80.0,
                text_spoilers.key(n),
            ));
            continue;
        }
        let only_pictures = card.title.is_none()
            && card.description.is_none()
            && card.fields.is_empty()
            && card.author.is_none()
            && card.link.is_none();
        if only_pictures && !prefs.show_images {
            continue;
        }
        let spoilers = Spoilers::new(&m.id, &format!("e{i}"), revealed);
        col = col.push(card_view(
            card,
            ctx,
            previews,
            small_images,
            &spoilers,
            prefs.show_images,
        ));
    }

    if !m.reactions.is_empty() && prefs.show_reactions {
        let chips = m.reactions.iter().fold(Row::new().spacing(6), |r, re| {
            let label: Element<'a, Message> = match (&re.emoji.id, &re.emoji.name) {
                (Some(id), name) => row![
                    custom_emoji(
                        small_images,
                        id,
                        name.as_deref().unwrap_or("emoji"),
                        re.emoji.animated,
                        16.0
                    ),
                    text(re.count.to_string()).size(12),
                ]
                .spacing(5)
                .align_y(Vertical::Center)
                .into(),
                _ => text(format!("{} {}", re.emoji.label(), re.count))
                    .size(12)
                    .shaping(Shaping::Advanced)
                    .into(),
            };
            let toggle = toggle_reactions.then(|| Message::React {
                message_id: m.id.clone(),
                emoji: re.emoji.clone(),
                add: !re.me,
            });
            r.push(
                button(label)
                    .on_press_maybe(toggle)
                    .padding([2, 10])
                    .style(reaction(re.me)),
            )
        });
        col = col.push(chips);
    }
    col.into()
}

/// What a click on a mention opens: the person's profile (with the server
/// part in a server), or the role card (roles only exist in servers).
pub(super) fn mention_link(target: Option<&Target>, guild_id: Option<&str>) -> Option<Message> {
    match target? {
        Target::User(id) => Some(Message::OpenProfile {
            user_id: id.clone(),
            guild_id: guild_id.map(str::to_string),
        }),
        Target::Role(id) => Some(Message::OpenRole {
            guild_id: guild_id?.to_string(),
            role_id: id.clone(),
        }),
    }
}

/// Style of a piece of message text; mentions can be clicked.
pub(super) fn styled_span(
    t: String,
    kind: SegmentKind,
    link: Option<Message>,
) -> text::Span<'static, Message, iced::Font> {
    let s = styled_text(t, kind);
    match link {
        Some(m) => s.link(m),
        None => s,
    }
}

pub(super) fn styled_text(t: String, kind: SegmentKind) -> text::Span<'static, Message, iced::Font> {
    match kind {
        SegmentKind::Plain => span(t),
        SegmentKind::Mention(mine) => span(t)
            .font(SEMIBOLD)
            .color(if mine { pal::yellow() } else { pal::accent() })
            .background(pal::chip()),
        SegmentKind::Channel => span(t).color(pal::accent()).background(pal::chip()),
        SegmentKind::Emoji => span(t).color(pal::yellow()),
        SegmentKind::Time => span(t).color(pal::muted()),
    }
}

/// The spoilers of a message's text, of one attachment or of one card:
/// which ones were revealed. Keys are `message:scope:n`.
pub(super) struct Spoilers<'r> {
    prefix: String,
    revealed: &'r HashSet<String>,
}

impl<'r> Spoilers<'r> {
    pub(super) fn new(message_id: &str, scope: &str, revealed: &'r HashSet<String>) -> Self {
        Self {
            prefix: format!("{message_id}:{scope}"),
            revealed,
        }
    }

    pub(super) fn key(&self, n: usize) -> String {
        format!("{}:{n}", self.prefix)
    }

    pub(super) fn hidden(&self, n: usize) -> bool {
        !self.revealed.contains(&self.key(n))
    }
}

/// One piece of text: a mention can be clicked, a hidden spoiler is a
/// block of colour that shows it when clicked.
fn segment_span(
    text: String,
    seg: &Segment,
    guild_id: Option<&str>,
    spoilers: &Spoilers,
) -> text::Span<'static, Message, iced::Font> {
    match seg.spoiler {
        Some(n) if spoilers.hidden(n) => span(text)
            .color(pal::muted())
            .background(pal::muted())
            .link(Message::RevealSpoiler(spoilers.key(n))),
        Some(_) if seg.kind == SegmentKind::Plain => span(text).background(pal::chip()),
        _ => styled_span(text, seg.kind, mention_link(seg.target.as_ref(), guild_id)),
    }
}

/// Text with mentions, spoilers and server emojis. If it is made only of
/// emojis they are shown large, as on Discord; otherwise they sit in the
/// text, which wraps word by word.
pub(super) fn rich_segments<'a>(
    segs: Vec<Segment>,
    edited: bool,
    images: &'a HashMap<String, Preview>,
    guild_id: Option<&str>,
    spoilers: &Spoilers,
    size: f32,
) -> Element<'a, Message> {
    let edited_note = || span(t(" (edited)", " (modificato)")).size(11).color(pal::muted());
    // An emoji in a hidden spoiler is hidden as text.
    let shown_emoji = |s: &Segment| s.emoji.is_some() && !s.spoiler.is_some_and(|n| spoilers.hidden(n));
    if !segs.iter().any(&shown_emoji) {
        let mut spans: Vec<_> = segs
            .iter()
            .map(|seg| segment_span(seg.text.clone(), seg, guild_id, spoilers))
            .collect();
        if edited {
            spans.push(edited_note());
        }
        return rich_text(spans).size(size).on_link_click(|m| m).into();
    }
    let count = segs.iter().filter(|s| shown_emoji(s)).count();
    let only_emojis = segs.iter().all(|s| shown_emoji(s) || s.text.trim().is_empty());
    if only_emojis && count <= 27 {
        let mut r = Row::new().spacing(4).align_y(Vertical::Center);
        for e in segs.iter().filter_map(|s| s.emoji.as_ref()) {
            r = r.push(custom_emoji(images, &e.id, &e.name, e.animated, 44.0));
        }
        if edited {
            r = r.push(rich_text(vec![edited_note()]));
        }
        return r.wrap().into();
    }

    let new_line = || Row::new().align_y(Vertical::Center);
    let mut lines = Column::new().spacing(2);
    let mut line = new_line();
    for seg in &segs {
        if let Some(e) = seg.emoji.as_ref().filter(|_| shown_emoji(seg)) {
            line = line.push(custom_emoji(images, &e.id, &e.name, e.animated, 22.0));
            continue;
        }
        let mut parts = seg.text.split('\n').peekable();
        while let Some(part) = parts.next() {
            for piece in part.split_inclusive(' ') {
                let word = piece.trim_end_matches(' ');
                if !word.is_empty() {
                    line = line.push(
                        rich_text(vec![segment_span(word.to_string(), seg, guild_id, spoilers)])
                            .size(size)
                            .on_link_click(|m| m),
                    );
                }
                for _ in 0..piece.len() - word.len() {
                    line = line.push(Space::new().width(WORD_SPACE));
                }
            }
            if parts.peek().is_some() {
                lines = lines.push(std::mem::replace(&mut line, new_line()).wrap());
            }
        }
    }
    if edited {
        line = line.push(rich_text(vec![edited_note()]));
    }
    lines.push(line.wrap()).into()
}

/// A hidden picture or preview, its size kept: a click shows it.
fn spoiler_box<'a>(w: f32, h: f32, key: String) -> Element<'a, Message> {
    let label = text(t("SPOILER", "SPOILER"))
        .size(13)
        .font(BOLD)
        .color(pal::text());
    mouse_area(
        container(label)
            .width(w)
            .height(h)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(pal::soft())),
                border: iced::Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    )
    .on_press(Message::RevealSpoiler(key))
    .interaction(mouse::Interaction::Pointer)
    .into()
}

/// Largest side of a card's small picture (on the right).
pub(super) const CARD_THUMB: f32 = 80.0;
/// Width of a card, as Discord's.
const CARD_WIDTH: f32 = 432.0;

/// The size a card's small picture is shown at.
pub(super) fn thumb_size(w: u32, h: u32) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (CARD_THUMB / w).min(CARD_THUMB / h).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// A page's preview or a bot's card: a coloured bar on the left, then
/// site, author, title, text, fields, picture and footer. Texts go through
/// the same mentions and spoilers as messages (bots' markdown is shown as
/// it is written). Links are not opened: the page's address is shown.
pub(super) fn card_view<'a>(
    card: media::EmbedCard,
    ctx: &Ctx,
    previews: &'a HashMap<String, Preview>,
    small_images: &'a HashMap<String, Preview>,
    spoilers: &Spoilers,
    show_images: bool,
) -> Element<'a, Message> {
    let mut body = Column::new().spacing(6);
    let bare = card.provider.is_none()
        && card.author.is_none()
        && card.title.is_none()
        && card.description.is_none()
        && card.fields.is_empty();
    if let Some(p) = card.provider {
        body = body.push(text(p).size(11).color(pal::muted()).shaping(Shaping::Advanced));
    }
    if let Some(a) = card.author {
        body = body.push(text(a).size(13).font(SEMIBOLD).shaping(Shaping::Advanced));
    }
    if let Some(title) = card.title {
        let colour = if card.link.is_some() {
            pal::accent()
        } else {
            pal::text()
        };
        body = body.push(
            text(title)
                .size(14)
                .font(BOLD)
                .color(colour)
                .shaping(Shaping::Advanced),
        );
    }
    // Spoilers are numbered across the card's texts.
    let mut first = 0;
    let mut texts = |s: &str, size: f32| -> Element<'a, Message> {
        let mut segs = segments(s, ctx);
        for seg in &mut segs {
            seg.spoiler = seg.spoiler.map(|n| n + first);
        }
        first += crate::text::spoiler_parts(s)
            .iter()
            .filter(|(_, n)| n.is_some())
            .count();
        rich_segments(segs, false, small_images, ctx.guild_id, spoilers, size)
    };
    if let Some(d) = card.description {
        body = body.push(texts(&d, 13.0));
    }
    // Inline fields three to a row, as Discord; the others on their own.
    let mut row_of = Row::new().spacing(12);
    let mut in_row = 0;
    for (name, value, inline) in card.fields {
        let field = column![
            text(name).size(12).font(BOLD).shaping(Shaping::Advanced),
            texts(&value, 13.0)
        ]
        .spacing(2)
        .width(if inline {
            Length::FillPortion(1)
        } else {
            Length::Fill
        });
        if (!inline || in_row == 3) && in_row > 0 {
            body = body.push(std::mem::replace(&mut row_of, Row::new().spacing(12)));
            in_row = 0;
        }
        if inline {
            row_of = row_of.push(field);
            in_row += 1;
        } else {
            body = body.push(field);
        }
    }
    if in_row > 0 {
        body = body.push(row_of);
    }
    if show_images {
        if let Some(i) = &card.image {
            let (w, h) = media::display_size(i.width, i.height);
            let open = Message::OpenViewer(Box::new(media::link_image(&i.url, i.width, i.height)));
            body = body.push(media_box(previews.get(&i.url), w, h, Some(open)));
        }
    }
    let mut footer: Vec<String> = card.footer.into_iter().collect();
    if let Some(when) = card.timestamp.as_deref().and_then(crate::text::parse_ts) {
        footer.push(when.format(t("%Y-%m-%d %H:%M", "%d/%m/%Y %H:%M")).to_string());
    }
    if !footer.is_empty() {
        body = body.push(
            text(footer.join(" • "))
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced),
        );
    }
    if let Some(link) = card.link.filter(|_| bare) {
        body = body.push(text(link).size(12).color(pal::muted()));
    }
    let mut content = Row::new().spacing(12).push(body.width(Length::Fill));
    if show_images {
        if let Some(th) = &card.thumbnail {
            let (w, h) = thumb_size(th.width, th.height);
            let open = Message::OpenViewer(Box::new(media::link_image(&th.url, th.width, th.height)));
            content = content.push(media_box(previews.get(&th.url), w, h, Some(open)));
        }
    }
    let bar = card.color.map_or_else(pal::border, |c| {
        iced::Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
    });
    // The bar is the outer background showing on the left (a filling
    // child of a shrinking row would get no height).
    let inner = container(content)
        .padding([10, 12])
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(pal::panel())),
            border: iced::Border {
                radius: iced::border::Radius::default().right(6.0),
                ..Default::default()
            },
            ..Default::default()
        });
    container(inner)
        .padding(iced::Padding {
            left: 4.0,
            ..iced::Padding::ZERO
        })
        .max_width(CARD_WIDTH)
        .clip(true)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(bar)),
            border: iced::Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Image of a server emoji; until it arrives it takes up its space, if it
/// cannot be downloaded the name stays.
pub(super) fn custom_emoji<'a>(
    images: &'a HashMap<String, Preview>,
    id: &str,
    name: &str,
    is_animated: bool,
    size: f32,
) -> Element<'a, Message> {
    match images.get(&media::emoji_url(id, is_animated)) {
        Some(Preview::Ready(handle)) => img::Image::new(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Preview::Animated(a)) => animated(a).width(size).height(size).into(),
        Some(Preview::Failed) => text(format!(":{name}:"))
            .size(14)
            .color(pal::yellow())
            .shaping(Shaping::Advanced)
            .into(),
        _ => Space::new().width(size).height(size).into(),
    }
}

/// Image box: animated, static or loading, always the same size (so the
/// chat does not jump when the image arrives).
pub(super) fn media_box<'a>(
    state: Option<&'a Preview>,
    w: f32,
    h: f32,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let picture: Element<'a, Message> = match state {
        Some(Preview::Ready(handle)) => img::Image::new(handle.clone())
            .width(w)
            .height(h)
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Preview::Animated(animation)) => animated(animation).width(w).height(h).into(),
        _ => {
            return container(
                text(t("Loading preview…", "Caricamento anteprima…"))
                    .size(12)
                    .color(pal::muted()),
            )
            .width(w)
            .height(h)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(outlined)
            .into()
        }
    };
    match on_press {
        Some(message) => mouse_area(picture)
            .on_press(message)
            .interaction(mouse::Interaction::Pointer)
            .into(),
        None => picture,
    }
}

/// Preview of an attached image, with the file name and size below.
pub(super) fn image_preview<'a>(
    a: &crate::discord::models::Attachment,
    state: Option<&'a Preview>,
) -> Element<'a, Message> {
    let (w, h) = media::display_size(a.width.unwrap_or(1), a.height.unwrap_or(1));
    let picture = media_box(state, w, h, Some(Message::OpenViewer(Box::new(a.clone()))));
    let caption = row![
        text(a.filename.clone())
            .size(11)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
        text(human_size(a.size)).size(11).color(pal::muted()),
    ]
    .spacing(8);
    column![picture, caption].spacing(4).into()
}

/// Key of the preview in the cache: the address to download.
pub(super) fn preview_key(a: &crate::discord::models::Attachment) -> Option<String> {
    if a.url == crate::demo::DEMO_IMAGE_URL {
        return Some(a.url.clone());
    }
    media::preview_url(a)
}

/// First usable address for a link preview: the animated GIF, or the static
/// image if the GIF could not be downloaded.
pub(super) fn embed_source<'p>(
    e: &'p media::EmbedPreview,
    previews: &HashMap<String, Preview>,
) -> Option<&'p str> {
    e.sources
        .iter()
        .map(String::as_str)
        .find(|url| !matches!(previews.get(*url), Some(Preview::Failed)))
}

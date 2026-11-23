//! The open conversation: the message list, each message's body (text, emojis, mentions, previews, replies) and the composer area.

use super::*;
use iced::widget::column;

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
    toggle_reactions: bool,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(6);
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
        let edited = m.edited_timestamp.is_some();
        if segs.iter().any(|s| s.emoji.is_some()) {
            col = col.push(content_with_emojis(segs, edited, small_images, ctx.guild_id));
        } else {
            let mut spans: Vec<_> = segs
                .into_iter()
                .map(|seg| {
                    let link = mention_link(seg.target.as_ref(), ctx.guild_id);
                    styled_span(seg.text, seg.kind, link)
                })
                .collect();
            if edited {
                spans.push(span(t(" (edited)", " (modificato)")).size(11).color(pal::muted()));
            }
            col = col.push(rich_text(spans).size(14).on_link_click(|m| m));
        }
    } else if m.attachments.is_empty() && embeds.is_empty() {
        let note = if m.kind == 0 {
            t("(empty message)", "(messaggio vuoto)")
        } else {
            t("(system message)", "(messaggio di sistema)")
        };
        col = col.push(text(note).size(13).color(pal::muted()));
    }

    for a in &m.attachments {
        if a.is_image() && prefs.show_images {
            let state = preview_key(a).and_then(|k| previews.get(&k));
            if !matches!(state, Some(Preview::Failed)) {
                col = col.push(image_preview(a, state));
                continue;
            }
        }
        col = col.push(
            container(
                row![
                    text(t("Attachment", "Allegato")).size(11).color(pal::muted()),
                    text(a.filename.clone())
                        .size(13)
                        .color(pal::accent())
                        .shaping(Shaping::Advanced),
                    text(human_size(a.size)).size(11).color(pal::muted()),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            )
            .padding([8, 12])
            .style(outlined),
        );
    }

    for e in &embeds {
        let state = embed_source(e, previews).and_then(|k| previews.get(k));
        let (w, h) = media::display_size(e.width, e.height);
        let caption: Element<'a, Message> = match &e.provider {
            Some(name) => text(name.clone())
                .size(11)
                .color(pal::muted())
                .shaping(Shaping::Advanced)
                .into(),
            None => Space::new().width(0).height(0).into(),
        };
        col = col.push(column![media_box(state, w, h, None), caption].spacing(4));
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

/// Text with server emojis. If the message is made only of emojis they are
/// shown large, as on Discord; otherwise they sit in the text, which wraps
/// word by word.
pub(super) fn content_with_emojis<'a>(
    segs: Vec<Segment>,
    edited: bool,
    images: &'a HashMap<String, Preview>,
    guild_id: Option<&str>,
) -> Element<'a, Message> {
    let edited_note = || text(t(" (edited)", " (modificato)")).size(11).color(pal::muted());
    let count = segs.iter().filter(|s| s.emoji.is_some()).count();
    let only_emojis = segs.iter().all(|s| s.emoji.is_some() || s.text.trim().is_empty());
    if only_emojis && count <= 27 {
        let mut r = Row::new().spacing(4).align_y(Vertical::Center);
        for e in segs.iter().filter_map(|s| s.emoji.as_ref()) {
            r = r.push(custom_emoji(images, &e.id, &e.name, e.animated, 44.0));
        }
        if edited {
            r = r.push(edited_note());
        }
        return r.wrap().into();
    }

    let new_line = || Row::new().align_y(Vertical::Center);
    let mut lines = Column::new().spacing(2);
    let mut line = new_line();
    for seg in segs {
        if let Some(e) = &seg.emoji {
            line = line.push(custom_emoji(images, &e.id, &e.name, e.animated, 22.0));
            continue;
        }
        let mut parts = seg.text.split('\n').peekable();
        while let Some(part) = parts.next() {
            for piece in part.split_inclusive(' ') {
                let word = piece.trim_end_matches(' ');
                if !word.is_empty() {
                    let link = mention_link(seg.target.as_ref(), guild_id);
                    line = line.push(
                        rich_text(vec![styled_span(word.to_string(), seg.kind, link)])
                            .size(14)
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
        line = line.push(edited_note());
    }
    lines.push(line.wrap()).into()
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

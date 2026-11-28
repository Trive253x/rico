//! Images: previews of attachments and links, avatars, icons and emojis, the GIF and emoji picker, the full-screen viewer.

use super::*;
use iced::widget::column;

impl Gui {
    /// Starts downloading the previews still missing among the loaded
    /// messages of the open channel, newest first (at most `MAX_DOWNLOADS` at
    /// a time; the others start as the running ones finish). Includes the
    /// older messages loaded by scrolling up.
    pub(super) fn request_previews(&mut self) -> Task<Message> {
        // (address, size to decode animated GIFs at). The picker's GIFs come
        // first, if it is open: they are the ones being looked at.
        // Only with their previews turned on: they come from Klipy.
        let mut wanted: Vec<(String, (u32, u32))> = self
            .picker
            .as_ref()
            .filter(|_| self.settings.gif_picker_previews)
            .map(|p| {
                p.shown_gifs()
                    .iter()
                    .map(|g| {
                        let (w, h) = picker::gif_size(g);
                        (g.src.clone(), ((w * self.scale) as u32, (h * self.scale) as u32))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let channel = self
            .store
            .current
            .as_ref()
            .and_then(|c| self.store.messages.get(c));
        // Images in the chat only if the settings want them.
        let channel = channel.filter(|_| self.settings.show_images);
        wanted.extend(match channel {
            Some(list) => list
                .iter()
                .rev()
                .flat_map(|m| {
                    // Hidden spoilers are downloaded only once shown.
                    let text_spoilers = chat::Spoilers::new(&m.id, "t", &self.revealed);
                    let link_shown = |link: Option<&str>| {
                        link.and_then(|l| crate::text::link_spoiler(&m.content, l))
                            .is_none_or(|n| !text_spoilers.hidden(n))
                    };
                    let attachments = m
                        .attachments
                        .iter()
                        .filter(|a| a.is_image())
                        .filter(|a| {
                            !a.is_spoiler()
                                || !chat::Spoilers::new(&m.id, &format!("a{}", a.id), &self.revealed)
                                    .hidden(0)
                        })
                        .filter_map(|a| {
                            let size = decode_size(a.width.unwrap_or(1), a.height.unwrap_or(1), self.scale);
                            preview_key(a).map(|k| (k, size))
                        });
                    let embeds = m
                        .embeds
                        .iter()
                        .filter_map(media::embed_preview)
                        .filter(|e| link_shown(e.link.as_deref()))
                        .filter_map(|e| {
                            let size = decode_size(e.width, e.height, self.scale);
                            embed_source(&e, &self.previews).map(|k| (k.to_string(), size))
                        });
                    let cards = m
                        .embeds
                        .iter()
                        .filter_map(media::embed_card)
                        .filter(|c| link_shown(c.link.as_deref()))
                        .flat_map(|c| {
                            let thumb = c.thumbnail.map(|i| {
                                let (w, h) = chat::thumb_size(i.width, i.height);
                                let size = (
                                    ((w * self.scale).ceil() as u32).min(i.width),
                                    ((h * self.scale).ceil() as u32).min(i.height),
                                );
                                (i.url, size)
                            });
                            let image = c.image.map(|i| {
                                let size = decode_size(i.width, i.height, self.scale);
                                (i.url, size)
                            });
                            thumb.into_iter().chain(image)
                        });
                    attachments.chain(embeds).chain(cards).collect::<Vec<_>>()
                })
                .take(MAX_IMAGES_PER_CHANNEL)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        });
        let mut tasks = Vec::new();
        for (url, (w, h)) in wanted {
            if self.previews.contains_key(&url) {
                continue;
            }
            if url == crate::demo::DEMO_IMAGE_URL {
                let (w, h) = (960, 600);
                let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
                self.remember_preview(url, Preview::Ready(handle));
                continue;
            }
            if url == crate::demo::DEMO_GIF_URL {
                let state = match anim::demo_animation(320, 200) {
                    Some(a) => Preview::Animated(a),
                    None => Preview::Failed,
                };
                self.remember_preview(url, state);
                continue;
            }
            if self.downloads >= MAX_DOWNLOADS {
                break;
            }
            self.downloads += 1;
            self.remember_preview(url.clone(), Preview::Loading);
            let key = url.clone();
            tasks.push(Task::perform(
                async move { load_preview(url, w, h).await.map_err(|e| format!("{e:#}")) },
                move |result| Message::PreviewLoaded { url: key, result },
            ));
        }
        if !tasks.is_empty() {
            tracing::debug!("previews: {} started, {} running", tasks.len(), self.downloads);
        }
        tasks.push(self.request_small_images());
        Task::batch(tasks)
    }

    /// Small images that should be on screen, most important first.
    pub(super) fn wanted_small_images(&self) -> Vec<(String, SmallKind)> {
        let mut wanted: Vec<(String, SmallKind)> = Vec::new();
        let as_avatar = |url: String| (url, SmallKind::Shaped(media::Shape::Circle, AVATAR_PX, AVATAR_PX));
        // Server icons: always visible in the bar on the left.
        for g in &self.store.guilds {
            if let Some(url) = media::guild_icon_url(&g.id, g.icon(), ICON_PX) {
                wanted.push((url, SmallKind::Shaped(media::Shape::Rounded, ICON_PX, ICON_PX)));
            }
        }
        // The open stream preview, and the small ones shown on hover.
        let (pw, ph) = stream::PREVIEW_PX;
        if let Some(url) = self.stream_view.as_ref().and_then(|v| v.image_url()) {
            wanted.push((url.to_string(), SmallKind::Shaped(media::Shape::Plain, pw, ph)));
        }
        for p in self.hover_previews.values() {
            if let Some(url) = p.image_url() {
                wanted.push((url.to_string(), SmallKind::Shaped(media::Shape::Plain, pw, ph)));
            }
        }
        // The faces in the open role card.
        if let Some(v) = &self.role_view {
            for id in v.shown_people() {
                if let Some(url) = self.store.avatar_url(Some(&v.guild_id), id, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
        }
        // The open profile.
        if let Some(p) = &self.profile {
            for (url, kind) in p.images(&self.store) {
                wanted.push((url, kind));
            }
        }
        if let Some(me) = self.store.me_id() {
            if let Some(url) = self.store.avatar_url(None, me, AVATAR_PX) {
                wanted.push(as_avatar(url));
            }
        }
        // The people of the Friends view.
        if self.friends.open && self.guild.is_none() {
            for id in friends::shown_people(&self.friends, &self.store) {
                if let Some(url) = self.store.avatar_url(None, &id, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
        }
        // Pictures of the private conversations, when their list is shown
        // (the most recent first).
        if self.guild.is_none() {
            for c in self.store.dms.iter().take(MAX_DM_PICTURES) {
                if let (_, Some(url)) = self.store.dm_picture(c, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
        }
        // The people next to the stream being watched.
        if let Some(w) = &self.watch {
            for (user, _) in self.store.voice_members(&w.stream.channel_id) {
                if let Some(url) = self.store.avatar_url(Some(&w.stream.guild_id), &user, AVATAR_PX) {
                    wanted.push(as_avatar(url));
                }
            }
        }
        let channel = self.store.current.clone();
        let guild = channel
            .as_ref()
            .and_then(|c| self.store.channel_guild.get(c))
            .cloned();
        // Who is in the voice channels of the open server.
        if let Some(g) = &guild {
            for (user, (vg, _)) in &self.store.voice {
                if vg == g {
                    if let Some(url) = self.store.avatar_url(Some(g), user, AVATAR_PX) {
                        wanted.push(as_avatar(url));
                    }
                }
            }
        }
        // The member list on the right (the first ones: the visible ones).
        if let Some(list) = guild
            .as_ref()
            .filter(|_| self.settings.member_list)
            .and_then(|g| self.store.member_lists.get(g))
        {
            for row in list.known().take(MEMBER_LIST_PICTURES) {
                if let crate::discord::member_list::Row::Member { user_id, .. } = row {
                    if let Some(url) = self.store.avatar_url(Some(&list.guild_id), user_id, AVATAR_PX) {
                        wanted.push(as_avatar(url));
                    }
                }
            }
        }
        let Some(channel_id) = channel else { return wanted };
        // Authors of the latest messages.
        if let Some(list) = self.store.messages.get(&channel_id) {
            let mut seen = std::collections::HashSet::new();
            for m in list.iter().rev().take(MAX_IMAGES_PER_CHANNEL) {
                if seen.insert(m.author.id.as_str()) {
                    if let Some(url) = self.store.avatar_url(guild.as_deref(), &m.author.id, AVATAR_PX) {
                        wanted.push(as_avatar(url));
                    }
                }
            }
        }
        // Server emojis: those of the picker first, if it is open, then
        // those in the channel's messages and reactions.
        let mut emojis = 0;
        if self.picker.as_ref().is_some_and(|p| p.tab == picker::Tab::Emoji) {
            for e in self.store.usable_emojis(&channel_id) {
                wanted.push((
                    media::emoji_url(&e.id, e.animated),
                    SmallKind::Emoji { animated: e.animated },
                ));
            }
        }
        if let Some(list) = self.store.messages.get(&channel_id) {
            for m in list.iter().rev() {
                for e in crate::text::custom_emojis(&m.content) {
                    wanted.push((
                        media::emoji_url(&e.id, e.animated),
                        SmallKind::Emoji { animated: e.animated },
                    ));
                    emojis += 1;
                }
                for r in &m.reactions {
                    if let Some(id) = &r.emoji.id {
                        let animated = r.emoji.animated;
                        wanted.push((media::emoji_url(id, animated), SmallKind::Emoji { animated }));
                        emojis += 1;
                    }
                }
                if emojis >= MAX_EMOJIS_PER_CHANNEL {
                    break;
                }
            }
        }
        wanted
    }

    /// Downloads the small images still missing (server icons, avatars, the
    /// open profile, server emojis), a few at a time.
    pub(super) fn request_small_images(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for (url, kind) in self.wanted_small_images() {
            if self.small_images.contains_key(&url) {
                continue;
            }
            if self.demo {
                let image = match kind {
                    SmallKind::Emoji { animated } => demo_emoji(&url, animated),
                    SmallKind::Shaped(shape, w, h) => demo_shaped(w, h, shape),
                };
                self.remember_small_image(url, image);
                continue;
            }
            if self.small_downloads >= MAX_SMALL_DOWNLOADS {
                break;
            }
            self.small_downloads += 1;
            self.remember_small_image(url.clone(), Preview::Loading);
            let key = url.clone();
            tasks.push(Task::perform(
                async move {
                    match kind {
                        SmallKind::Emoji { .. } => load_preview(url, 48, 48).await,
                        SmallKind::Shaped(shape, w, h) => load_shaped(url, w, h, shape).await,
                    }
                    .map_err(|e| format!("{e:#}"))
                },
                move |result| Message::SmallImageLoaded { url: key, result },
            ));
        }
        Task::batch(tasks)
    }

    pub(super) fn remember_small_image(&mut self, url: String, state: Preview) {
        self.small_images.insert(url.clone(), state);
        self.small_order.push_back(url);
        let mut checked = 0;
        while self.small_images.len() > MAX_SMALL_IMAGES && checked < self.small_order.len() {
            let Some(old) = self.small_order.pop_front() else {
                break;
            };
            if matches!(self.small_images.get(&old), Some(Preview::Loading)) {
                self.small_order.push_back(old);
                checked += 1;
            } else {
                self.small_images.remove(&old);
            }
        }
    }

    pub(super) fn toggle_picker(&mut self, tab: picker::Tab) -> Task<Message> {
        // The composer's buttons write in the text field.
        self.reacting = None;
        match self.picker.as_mut() {
            Some(p) if p.tab == tab => {
                self.close_picker();
                return iced::widget::operation::focus(composer_id());
            }
            Some(p) => p.tab = tab,
            None => {
                let has_server = self
                    .store
                    .current
                    .as_ref()
                    .is_some_and(|c| !self.store.usable_emojis(c).is_empty());
                self.picker = Some(Picker::new(tab, !self.recent_emojis.is_empty(), has_server));
            }
        }
        Task::batch([
            iced::widget::operation::focus(picker::search_id()),
            self.load_gifs(),
            self.request_small_images(),
        ])
    }

    /// The first time the GIF tab opens, it loads the trending ones.
    pub(super) fn load_gifs(&mut self) -> Task<Message> {
        match self.picker.as_mut() {
            Some(p) if p.tab == picker::Tab::Gif && matches!(p.gifs, Gifs::NotLoaded) => {
                p.gifs = Gifs::Loading;
                let seq = p.seq;
                self.search_gifs(seq)
            }
            _ => Task::none(),
        }
    }

    pub(super) fn search_gifs(&mut self, seq: u64) -> Task<Message> {
        let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
            return Task::none();
        };
        p.searching = true;
        if self.demo {
            let result = Ok(crate::demo::gif_results(&p.gif_query));
            return self.update(Message::GifResults { seq, result });
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let query = p.gif_query.clone();
        Task::perform(
            async move { client.gifs(&query).await.map_err(|e| format!("{e:#}")) },
            move |result| Message::GifResults { seq, result },
        )
    }

    pub(super) fn close_picker(&mut self) {
        self.reacting = None;
        if let Some(p) = self.picker.take() {
            self.forget_gif_previews(p.gifs);
        }
    }

    /// The previews of the picker's GIFs are no longer needed: freed right away
    /// (they are already decoded animations and take a lot of memory).
    pub(super) fn forget_gif_previews(&mut self, gifs: Gifs) {
        let Gifs::Ready(list) = gifs else {
            return;
        };
        for g in list {
            if g.src != crate::demo::DEMO_GIF_URL {
                self.previews.remove(&g.src);
            }
        }
        let previews = &self.previews;
        self.preview_order.retain(|url| previews.contains_key(url));
    }

    pub(super) fn open_viewer(&mut self, attachment: Attachment) -> Task<Message> {
        let cached = preview_key(&attachment).and_then(|k| self.previews.get(&k));
        // An animation (a GIF from a link) is shown as it is, already here.
        if let Some(Preview::Animated(animation)) = cached {
            self.viewer = Some(ImageViewer {
                image: Preview::Animated(animation.clone()),
                attachment,
                url: None,
                preview: None,
                save: SaveState::Idle,
            });
            return Task::none();
        }
        let preview = match cached {
            Some(Preview::Ready(h)) => Some(h.clone()),
            _ => None,
        };
        if attachment.url == crate::demo::DEMO_IMAGE_URL {
            let (w, h) = (1920, 1200);
            let handle = img::Handle::from_rgba(w, h, media::demo_pixels(w, h));
            self.viewer = Some(ImageViewer {
                attachment,
                url: None,
                image: Preview::Ready(handle),
                preview,
                save: SaveState::Idle,
            });
            return Task::none();
        }
        let url = media::viewer_url(&attachment);
        self.viewer = Some(ImageViewer {
            attachment,
            url: url.clone(),
            image: if url.is_some() {
                Preview::Loading
            } else {
                Preview::Failed
            },
            preview,
            save: SaveState::Idle,
        });
        let Some(url) = url else {
            return Task::none();
        };
        let key = url.clone();
        Task::perform(
            async move {
                media::fetch(&url, media::VIEWER_MAX_BYTES)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::ViewerLoaded { url: key, result },
        )
    }

    pub(super) fn save_attachment(&mut self) -> Task<Message> {
        let demo = self.demo;
        let Some(v) = self.viewer.as_mut() else {
            return Task::none();
        };
        if matches!(v.save, SaveState::Saving) {
            return Task::none();
        }
        if demo || v.attachment.url == crate::demo::DEMO_IMAGE_URL {
            self.status = Some((
                t(
                    "Nothing is downloaded in the demo.",
                    "Nella demo non si scarica nulla.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        // The original is on the CDN; the media proxy is the fallback.
        let url = if v.attachment.url.is_empty() {
            v.attachment.proxy_url.clone()
        } else {
            v.attachment.url.clone()
        };
        let filename = v.attachment.filename.clone();
        v.save = SaveState::Saving;
        Task::perform(
            async move {
                media::save_to(&url, &media::download_dir(), &filename)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Saved,
        )
    }

    pub(super) fn view_viewer<'a>(v: &'a ImageViewer) -> Element<'a, Message> {
        let a = &v.attachment;
        let mut info = row![
            text(a.filename.clone()).font(SEMIBOLD).shaping(Shaping::Advanced),
            text(human_size(a.size)).size(12).color(pal::muted()),
        ]
        .spacing(10)
        .align_y(Vertical::Center);
        if let (Some(w), Some(h)) = (a.width, a.height) {
            info = info.push(text(format!("{w}×{h}")).size(12).color(pal::muted()));
        }

        let save: Element<'a, Message> = match &v.save {
            SaveState::Idle | SaveState::Failed(_) => button(text(t("Download", "Scarica")).font(SEMIBOLD))
                .on_press(Message::SaveAttachment)
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saving => button(text(t("Saving…", "Salvataggio…")))
                .padding([6, 14])
                .style(primary)
                .into(),
            SaveState::Saved(path) => button(text(t("Show in folder", "Mostra nella cartella")))
                .on_press(Message::ShowInFolder(path.clone()))
                .padding([6, 14])
                .style(ghost)
                .into(),
        };
        let close = button(text(t("Close (Esc)", "Chiudi (Esc)")))
            .on_press(Message::CloseViewer)
            .padding([6, 14])
            .style(ghost);
        let top = container(
            row![info, space::horizontal(), save, close]
                .spacing(10)
                .align_y(Vertical::Center),
        )
        .padding([10, 16])
        .width(Length::Fill)
        .style(fill(pal::panel()));

        let mut footer = None;
        let body: Element<'a, Message> = match (&v.image, &v.preview) {
            (Preview::Ready(handle), _) => img::Viewer::new(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .min_scale(0.5)
                .max_scale(8.0)
                .into(),
            (Preview::Loading, Some(small)) => {
                footer = Some(t(
                    "Loading the large version…",
                    "Caricamento della versione grande…",
                ));
                img::Image::new(small.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Contain)
                    .into()
            }
            (Preview::Loading, None) => text(t("Loading…", "Caricamento…")).color(pal::muted()).into(),
            (Preview::Animated(animation), _) => animated(animation)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            (Preview::Failed, _) => text(t(
                "Image unavailable: you can still download it.",
                "Immagine non disponibile: puoi comunque scaricarla.",
            ))
            .color(pal::muted())
            .into(),
        };
        if let SaveState::Failed(e) = &v.save {
            footer = Some(e.as_str());
        }
        // Click on the image: it must not close the view.
        let picture = mouse_area(
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .on_press(Message::Noop);
        let mut panel = column![top, picture].height(Length::Fill);
        let hint = footer.unwrap_or_else(|| {
            t(
                "Mouse wheel to zoom, drag to move",
                "Rotellina per lo zoom, trascina per spostare",
            )
        });
        panel = panel.push(
            container(text(hint).size(12).color(pal::muted()).shaping(Shaping::Advanced))
                .padding([6, 16])
                .width(Length::Fill)
                .style(fill(pal::panel())),
        );

        // The dark background around closes the view; `opaque` blocks clicks
        // towards the chat below.
        opaque(
            mouse_area(
                container(container(panel).style(style::outlined))
                    .padding(32)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(style::backdrop),
            )
            .on_press(Message::CloseViewer),
        )
    }

    pub(super) fn remember_preview(&mut self, url: String, state: Preview) {
        self.previews.insert(url.clone(), state);
        self.preview_order.push_back(url);
        // Drop the oldest that are done; those downloading stay.
        let mut checked = 0;
        while self.previews.len() > MAX_PREVIEWS && checked < self.preview_order.len() {
            let Some(old) = self.preview_order.pop_front() else {
                break;
            };
            if matches!(self.previews.get(&old), Some(Preview::Loading)) {
                self.preview_order.push_back(old);
                checked += 1;
            } else {
                self.previews.remove(&old);
            }
        }
    }
}

impl Gui {
    /// The images messages of `Gui::handle`.
    pub(super) fn handle_images(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PreviewLoaded { url, result } => {
                self.downloads = self.downloads.saturating_sub(1);
                let state = match result {
                    Ok(Loaded::Image(bytes)) => {
                        tracing::debug!("preview downloaded ({} KB)", bytes.len() / 1024);
                        Preview::Ready(img::Handle::from_bytes(bytes))
                    }
                    Ok(Loaded::Animated(animation)) => {
                        tracing::debug!("animated GIF downloaded ({} KB)", animation.source_len() / 1024);
                        Preview::Animated(animation)
                    }
                    Ok(Loaded::Pixels { width, height, rgba }) => {
                        Preview::Ready(img::Handle::from_rgba(width, height, rgba))
                    }
                    Err(e) => {
                        tracing::debug!("preview not downloaded: {e}");
                        Preview::Failed
                    }
                };
                // It may have been dropped meanwhile (logout, full cache).
                if let Some(slot) = self.previews.get_mut(&url) {
                    *slot = state;
                }
                return self.request_previews();
            }
            Message::OpenViewer(attachment) => return self.open_viewer(*attachment),
            Message::ViewerLoaded { url, result } => {
                if let Some(v) = self
                    .viewer
                    .as_mut()
                    .filter(|v| v.url.as_deref() == Some(url.as_str()))
                {
                    v.image = match result {
                        Ok(bytes) => {
                            tracing::debug!("full image downloaded ({} KB)", bytes.len() / 1024);
                            Preview::Ready(img::Handle::from_bytes(bytes))
                        }
                        Err(e) => {
                            tracing::debug!("full image not downloaded: {e}");
                            Preview::Failed
                        }
                    };
                }
            }
            Message::CloseViewer => self.viewer = None,
            Message::SaveAttachment => return self.save_attachment(),
            Message::Saved(result) => {
                let state = match result {
                    Ok(path) => {
                        tracing::info!("attachment saved in {}", path.display());
                        self.status = Some((tf!("Saved in {}", "Salvato in {}", path.display()), false));
                        SaveState::Saved(path)
                    }
                    Err(e) => {
                        self.status =
                            Some((tf!("Saving failed: {e}", "Salvataggio non riuscito: {e}"), true));
                        SaveState::Failed(e)
                    }
                };
                if let Some(v) = self.viewer.as_mut() {
                    v.save = state;
                }
            }
            Message::ShowInFolder(path) => show_in_folder(&path),
            Message::TogglePicker(tab) => return self.toggle_picker(tab),
            Message::PickerTab(tab) => {
                if let Some(p) = self.picker.as_mut() {
                    p.tab = tab;
                }
                // GIFs are sent, never used as reactions.
                if tab != picker::Tab::Emoji {
                    self.reacting = None;
                }
                return Task::batch([
                    iced::widget::operation::focus(picker::search_id()),
                    self.load_gifs(),
                    self.request_small_images(),
                ]);
            }
            Message::ClosePicker => {
                self.close_picker();
                return iced::widget::operation::focus(composer_id());
            }
            Message::EmojiQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.emoji_query = q;
                }
            }
            Message::EmojiSection(section) => {
                if let Some(p) = self.picker.as_mut() {
                    p.section = section;
                    p.emoji_query.clear();
                }
            }
            Message::PickServerEmoji(name) => {
                if let Some(message_id) = self.reacting.take() {
                    let emoji = self.store.current.as_deref().and_then(|c| {
                        self.store
                            .usable_emojis(c)
                            .into_iter()
                            .find(|e| e.name == name)
                            .map(|e| Emoji {
                                id: Some(e.id.clone()),
                                name: Some(e.name.clone()),
                                animated: e.animated,
                            })
                    });
                    self.close_picker();
                    return match emoji {
                        Some(emoji) => self.react(message_id, emoji, true),
                        None => Task::none(),
                    };
                }
                let _ = write!(self.composer, ":{name}:");
                self.close_picker();
                return Task::batch([
                    iced::widget::operation::focus(composer_id()),
                    iced::widget::operation::move_cursor_to_end(composer_id()),
                ]);
            }
            Message::SmallImageLoaded { url, result } => {
                self.small_downloads = self.small_downloads.saturating_sub(1);
                let state = match result {
                    Ok(Loaded::Image(bytes)) => Preview::Ready(img::Handle::from_bytes(bytes)),
                    Ok(Loaded::Animated(a)) => Preview::Animated(a),
                    Ok(Loaded::Pixels { width, height, rgba }) => {
                        Preview::Ready(img::Handle::from_rgba(width, height, rgba))
                    }
                    Err(e) => {
                        tracing::debug!("small image not downloaded: {e}");
                        Preview::Failed
                    }
                };
                if let Some(slot) = self.small_images.get_mut(&url) {
                    *slot = state;
                }
                return self.request_small_images();
            }
            Message::PickEmoji(emoji) => {
                self.recent_emojis.retain(|e| *e != emoji);
                self.recent_emojis.insert(0, emoji);
                self.recent_emojis.truncate(picker::MAX_RECENT);
                if let Some(message_id) = self.reacting.take() {
                    self.close_picker();
                    let emoji = Emoji {
                        id: None,
                        name: Some(emoji.to_string()),
                        animated: false,
                    };
                    return self.react(message_id, emoji, true);
                }
                self.composer.push_str(emoji);
                self.close_picker();
                return Task::batch([
                    iced::widget::operation::focus(composer_id()),
                    iced::widget::operation::move_cursor_to_end(composer_id()),
                ]);
            }
            Message::GifQuery(q) => {
                if let Some(p) = self.picker.as_mut() {
                    p.gif_query = q;
                    p.seq += 1;
                    let seq = p.seq;
                    // Search only when typing pauses for a moment
                    // (the timer is created inside the task, i.e. in iced's runtime).
                    let pause = async { tokio::time::sleep(Duration::from_millis(350)).await };
                    return Task::perform(pause, move |()| Message::GifSearch(seq));
                }
            }
            Message::GifSearch(seq) => return self.search_gifs(seq),
            Message::GifResults { seq, result } => {
                let Some(p) = self.picker.as_mut().filter(|p| p.seq == seq) else {
                    return Task::none();
                };
                p.searching = false;
                let old = std::mem::replace(
                    &mut p.gifs,
                    match result {
                        Ok(list) => Gifs::Ready(list),
                        Err(e) => Gifs::Failed(e),
                    },
                );
                tracing::debug!("GIF: {} results", p.shown_gifs().len());
                self.forget_gif_previews(old);
                return self.request_previews();
            }
            Message::PickGif(url) => {
                self.close_picker();
                return Task::batch([self.send_text(url), iced::widget::operation::focus(composer_id())]);
            }
            _ => {}
        }
        Task::none()
    }
}

/// Fake emoji for the demo: a coloured circle (or the demo GIF if animated).
pub(super) fn demo_emoji(url: &str, is_animated: bool) -> Preview {
    if is_animated {
        return anim::demo_animation(48, 48).map_or(Preview::Failed, Preview::Animated);
    }
    let seed = url
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let colors = [[222, 165, 96], [116, 173, 232], [161, 193, 129], [197, 159, 224]];
    let [r, g, b] = colors[seed as usize % colors.len()];
    let mut px = Vec::with_capacity(48 * 48 * 4);
    for y in 0..48i32 {
        for x in 0..48i32 {
            let (dx, dy) = (x - 24, y - 24);
            let inside = dx * dx + dy * dy < 21 * 21;
            px.extend_from_slice(&[r, g, b, if inside { 255 } else { 0 }]);
        }
    }
    Preview::Ready(img::Handle::from_rgba(48, 48, px))
}

/// Fake avatar or icon for the demo: the demo picture cut to shape.
pub(super) fn demo_shaped(w: u32, h: u32, shape: media::Shape) -> Preview {
    let mut px = media::demo_pixels(w, h);
    media::cut_shape(&mut px, w, h, shape);
    Preview::Ready(img::Handle::from_rgba(w, h, px))
}

/// Size at which to decode a GIF: that of the preview in screen pixels
/// (`scale` = the screen's scale factor), without exceeding the original.
/// More would only cost memory: every frame is kept decoded while visible.
pub(super) fn decode_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let (w, h) = media::display_size(width, height);
    (
        ((w * scale).ceil() as u32).min(width.max(1)),
        ((h * scale).ceil() as u32).min(height.max(1)),
    )
}

/// Opens the folder of the downloaded file in the system file manager.
pub(super) fn show_in_folder(path: &std::path::Path) {
    use std::process::Command;
    // No shell: the path is passed as an argument.
    let result = if cfg!(windows) {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else {
        Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open the folder: {e}");
    }
}

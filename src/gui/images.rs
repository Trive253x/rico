//! Images: previews of attachments and links, avatars, icons and emojis, the GIF and emoji picker, the full-screen viewer.

use super::{
    anim, animated, button, composer_id, container, decode_size, demo_emoji, demo_shaped, embed_source, fill,
    friends, ghost, human_size, img, load_preview, load_shaped, media, mouse_area, opaque, pal, picker,
    preview_key, primary, row, space, stream, style, t, text, Attachment, ContentFit, Element, Gifs, Gui,
    Horizontal, ImageViewer, Length, Message, Picker, Preview, SaveState, Shaping, SmallKind, Task, Vertical,
    AVATAR_PX, ICON_PX, MAX_DM_PICTURES, MAX_DOWNLOADS, MAX_EMOJIS_PER_CHANNEL, MAX_IMAGES_PER_CHANNEL,
    MAX_PREVIEWS, MAX_SMALL_DOWNLOADS, MAX_SMALL_IMAGES, MEMBER_LIST_PICTURES, SEMIBOLD,
};
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
                    let attachments = m.attachments.iter().filter(|a| a.is_image()).filter_map(|a| {
                        let size = decode_size(a.width.unwrap_or(1), a.height.unwrap_or(1), self.scale);
                        preview_key(a).map(|k| (k, size))
                    });
                    let embeds = m.embeds.iter().filter_map(media::embed_preview).filter_map(|e| {
                        let size = decode_size(e.width, e.height, self.scale);
                        embed_source(&e, &self.previews).map(|k| (k.to_string(), size))
                    });
                    attachments.chain(embeds).collect::<Vec<_>>()
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
        let preview = preview_key(&attachment).and_then(|k| match self.previews.get(&k) {
            Some(Preview::Ready(h)) => Some(h.clone()),
            _ => None,
        });
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

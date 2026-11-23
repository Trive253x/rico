//! Messages of the open channel: history, sending, replies, edits, deletions, reactions, files, read marks, going to a message.

use super::{
    compose, composer_id, context, jump, media, messages_id, no_send_permission, notify, permissions, rest,
    scrollable, t, Attachment, Emoji, Gui, Jump, Message, MessageDelete, MessagePreview, PathBuf, Screen,
    Task, HIGHLIGHT, JUMP_PAGES,
};

impl Gui {
    /// Tells Discord the channel is read up to its newest message, as the
    /// official client does when a channel is looked at (so the other
    /// devices agree). Never twice for the same message, and at most once
    /// every few seconds per channel: newer messages wait a moment.
    pub(super) fn ack(&mut self, channel_id: String) -> Task<Message> {
        const SPACING: std::time::Duration = std::time::Duration::from_secs(3);
        if self.demo {
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let Some(latest) = self.store.latest_message(&channel_id) else {
            return Task::none();
        };
        if !media::valid_id(&channel_id) || !media::valid_id(&latest) {
            return Task::none();
        }
        let now = std::time::Instant::now();
        if let Some((sent, when)) = self.acked.get(&channel_id) {
            if *sent == latest {
                return Task::none();
            }
            if now.duration_since(*when) < SPACING {
                if !self.ack_due.insert(channel_id.clone()) {
                    return Task::none();
                }
                let wait = SPACING.saturating_sub(now.duration_since(*when));
                return Task::perform(tokio::time::sleep(wait), move |()| Message::AckDue(channel_id));
            }
        }
        if self.acked.len() >= 2000 && !self.acked.contains_key(&channel_id) {
            self.acked.clear();
        }
        self.acked.insert(channel_id.clone(), (latest.clone(), now));
        Task::perform(
            async move {
                if let Err(e) = client.ack(&channel_id, &latest).await {
                    tracing::debug!("read position not sent: {e:#}");
                }
            },
            |()| Message::Noop,
        )
    }

    /// A Windows notification for a new message, if the account's settings
    /// want one, we are not looking at its channel, our status is not "Do not
    /// disturb", and not too many came just now.
    pub(super) fn maybe_notify(&mut self, m: &crate::discord::models::Message) {
        const BURST: usize = 5;
        const WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
        if !self.settings.notifications || self.demo || self.screen != Screen::Main {
            return;
        }
        let looking = self.focused && self.store.current.as_deref() == Some(m.channel_id.as_str());
        let dnd = self
            .store
            .my_status
            .as_ref()
            .is_some_and(|s| s.presence() == crate::discord::folders::Presence::DoNotDisturb);
        if looking || dnd || !self.store.should_notify(m) {
            return;
        }
        let now = std::time::Instant::now();
        while self
            .toast_times
            .front()
            .is_some_and(|t| now.duration_since(*t) > WINDOW)
        {
            self.toast_times.pop_front();
        }
        if self.toast_times.len() >= BURST {
            return;
        }
        self.toast_times.push_back(now);
        let guild = self.store.channel_guild.get(&m.channel_id).cloned();
        let author = self.store.name_in(guild.as_deref(), &m.author);
        let title = match self.store.guild_of(&m.channel_id) {
            Some(g) => format!(
                "{author} (#{}, {})",
                self.store.channel_label(&m.channel_id).trim_start_matches('#'),
                g.name()
            ),
            None => author,
        };
        let body = if !self.settings.notification_text {
            t("New message", "Nuovo messaggio").to_string()
        } else if m.content.trim().is_empty() {
            if m.attachments.is_empty() {
                t("Sent a message", "Ha inviato un messaggio").to_string()
            } else {
                t("Sent a file", "Ha inviato un file").to_string()
            }
        } else {
            let ctx = self.store.text_ctx(Some(&m.channel_id));
            notify::short(&crate::text::render_content(&m.content, &ctx))
        };
        notify::show(notify::Toast {
            channel_id: m.channel_id.clone(),
            title,
            body,
        });
    }

    /// Goes to a message of the open channel: at once if it is loaded,
    /// otherwise after loading older pages (at most [`JUMP_PAGES`]).
    pub(super) fn jump_to(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !crate::media::valid_id(&message_id) {
            return Task::none();
        }
        let loaded = self
            .store
            .messages
            .get(&channel_id)
            .is_some_and(|list| list.iter().any(|m| m.id == message_id));
        if loaded {
            self.jump = None;
            return self.show_message(message_id);
        }
        self.jump = Some(Jump {
            channel_id: channel_id.clone(),
            message_id,
            pages_left: JUMP_PAGES,
        });
        self.continue_jump(&channel_id)
    }

    /// After a page of history: the message looked for is there, or one
    /// more page is loaded, or the search ends.
    pub(super) fn continue_jump(&mut self, channel_id: &str) -> Task<Message> {
        let Some(jump) = self.jump.as_mut().filter(|j| j.channel_id == channel_id) else {
            return Task::none();
        };
        let found = self
            .store
            .messages
            .get(channel_id)
            .is_some_and(|list| list.iter().any(|m| m.id == jump.message_id));
        if found {
            let id = jump.message_id.clone();
            self.jump = None;
            return self.show_message(id);
        }
        // The demo has no older pages to load.
        let before = self.store.older_cursor(channel_id).filter(|_| !self.demo);
        match before {
            Some(before) if jump.pages_left > 0 => {
                jump.pages_left -= 1;
                self.fetch_history(channel_id.to_string(), Some(before))
            }
            _ => {
                self.jump = None;
                self.status = Some((
                    t(
                        "The original message is too far back or no longer exists.",
                        "Il messaggio originale è troppo indietro o non esiste più.",
                    )
                    .to_string(),
                    true,
                ));
                Task::none()
            }
        }
    }

    /// Scrolls to a loaded message and highlights it for a moment.
    pub(super) fn show_message(&mut self, message_id: String) -> Task<Message> {
        self.highlight = Some(message_id.clone());
        let id = message_id.clone();
        Task::batch([
            jump::offset_of(messages_id(), &message_id).map(Message::JumpOffset),
            Task::perform(tokio::time::sleep(HIGHLIGHT), move |()| Message::EndHighlight(id)),
        ])
    }

    pub(super) fn fetch_history(&mut self, channel_id: String, before: Option<String>) -> Task<Message> {
        if self.demo {
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        if !self.store.begin_loading(&channel_id) {
            return Task::none();
        }
        let older = before.is_some();
        let id = channel_id.clone();
        Task::perform(
            async move {
                client
                    .messages(&channel_id, before.as_deref())
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| Message::History {
                channel_id: id.clone(),
                older,
                result,
            },
        )
    }

    pub(super) fn send(&mut self) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if self.editing.is_some() {
            return self.save_edit();
        }
        if !self.store.can_send(&channel_id) {
            return Task::none();
        }
        let content = self.composer.trim().to_string();
        if content.is_empty() && self.pending_files.is_empty() {
            return Task::none();
        }
        if self.reading_files > 0 {
            self.status = Some((
                t(
                    "Wait: the files are still being read.",
                    "Attendi: i file sono ancora in lettura.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        // `:name:` of the channel server's emojis (that server only).
        let content = self.store.prepare_outgoing(&channel_id, &content);
        if content.chars().count() > rest::MAX_MESSAGE_CHARS {
            self.status = Some((
                t(
                    "The message is longer than 2000 characters.",
                    "Il messaggio supera i 2000 caratteri.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        let files = std::mem::take(&mut self.pending_files)
            .into_iter()
            .map(|f| rest::Upload {
                filename: f.name,
                bytes: f.bytes,
            })
            .collect();
        self.composer.clear();
        self.send_out(rest::Outgoing {
            content,
            files,
            ..Default::default()
        })
    }

    /// Sends a text to the open channel (also the link of a GIF chosen in the picker).
    pub(super) fn send_text(&mut self, content: String) -> Task<Message> {
        self.send_out(rest::Outgoing {
            content,
            ..Default::default()
        })
    }

    /// Sends a message to the open channel, as a reply if one was chosen.
    /// The permissions are checked here, whatever button started it.
    pub(super) fn send_out(&mut self, mut out: rest::Outgoing) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        // Checked again here: the GIF picker sends through this too.
        if !self.store.can_send(&channel_id) {
            self.status = Some((no_send_permission().into(), true));
            return Task::none();
        }
        if !out.files.is_empty() && !self.store.can_attach(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to send files in this channel.",
                    "Non hai il permesso di inviare file in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if let Some(r) = self.replying.take() {
            if r.channel_id == channel_id && self.store.can_reply(&channel_id) {
                out.reply_to = Some(r.message_id);
                out.ping_reply = r.ping;
            }
        }
        self.status = None;
        let bottom =
            iced::widget::operation::snap_to(messages_id(), scrollable::RelativeOffset { x: 0.0, y: 0.0 });

        if self.demo {
            let mut msg = crate::demo::local_message(
                &channel_id,
                self.store.me.clone(),
                &out.content,
                self.store.messages.get(&channel_id),
            );
            // In the demo the GIFs chosen in the picker become an embed right away.
            if let Some(embed) = crate::demo::gif_embed(&out.content) {
                msg.embeds = vec![embed];
            }
            if let Some(reply) = out.reply_to.as_deref() {
                msg.kind = 19;
                msg.referenced_message = self.store.find_message(&channel_id, reply).map(|r| {
                    Box::new(MessagePreview {
                        id: r.id.clone(),
                        author: Some(r.author.clone()),
                        content: r.content.clone(),
                    })
                });
            }
            // Nothing is uploaded: the files are listed with their names.
            msg.attachments = out
                .files
                .iter()
                .map(|f| Attachment {
                    filename: f.filename.clone(),
                    size: f.bytes.len() as u64,
                    ..Default::default()
                })
                .collect();
            self.store.insert_message(msg);
            return Task::batch([bottom, self.request_previews()]);
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let out = Box::new(out);
        let sent = Task::perform(
            async move {
                let result = client
                    .send_message(&channel_id, &out)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                (out, result)
            },
            |(out, result)| Message::Sent { out, result },
        );
        Task::batch([sent, bottom])
    }

    pub(super) fn start_reply(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_reply(&channel_id) {
            return Task::none();
        }
        let guild = self.store.channel_guild.get(&channel_id).cloned();
        let Some(m) = self.store.find_message(&channel_id, &message_id) else {
            return Task::none();
        };
        let author = self.store.name_in(guild.as_deref(), &m.author);
        // Like Discord: no notification when replying to ourselves.
        let ping = self.store.me_id() != Some(m.author.id.as_str());
        if self.editing.is_some() {
            self.cancel_edit();
        }
        self.confirm_delete = None;
        self.replying = Some(compose::Reply {
            channel_id,
            message_id,
            author,
            ping,
        });
        iced::widget::operation::focus(composer_id())
    }

    pub(super) fn start_edit(&mut self, message_id: String) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_edit(&channel_id, &message_id) {
            return Task::none();
        }
        let Some(content) = self
            .store
            .find_message(&channel_id, &message_id)
            .map(|m| m.content.clone())
        else {
            return Task::none();
        };
        let draft = match self.editing.take() {
            Some(e) => e.draft,
            None => std::mem::take(&mut self.composer),
        };
        self.replying = None;
        self.confirm_delete = None;
        self.composer = content;
        self.editing = Some(compose::Editing {
            channel_id,
            message_id,
            draft,
        });
        Task::batch([
            iced::widget::operation::focus(composer_id()),
            iced::widget::operation::move_cursor_to_end(composer_id()),
        ])
    }

    /// Leaves the edit: the text that was being written comes back.
    pub(super) fn cancel_edit(&mut self) {
        if let Some(e) = self.editing.take() {
            self.composer = e.draft;
        }
    }

    pub(super) fn save_edit(&mut self) -> Task<Message> {
        let Some(e) = self.editing.clone() else {
            return Task::none();
        };
        if self.store.current.as_deref() != Some(e.channel_id.as_str())
            || !self.store.can_edit(&e.channel_id, &e.message_id)
        {
            self.cancel_edit();
            return Task::none();
        }
        let content = self.composer.trim().to_string();
        if content.is_empty() {
            // As on Discord, an empty edit is a delete: it asks first.
            self.cancel_edit();
            self.confirm_delete = Some(e.message_id);
            return Task::none();
        }
        let content = self.store.prepare_outgoing(&e.channel_id, &content);
        if content.chars().count() > rest::MAX_MESSAGE_CHARS {
            self.status = Some((
                t(
                    "The message is longer than 2000 characters.",
                    "Il messaggio supera i 2000 caratteri.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        self.cancel_edit();
        let unchanged = self
            .store
            .find_message(&e.channel_id, &e.message_id)
            .is_some_and(|m| m.content == content);
        if unchanged {
            return Task::none();
        }
        if self.demo {
            if let Some(mut m) = self.store.find_message(&e.channel_id, &e.message_id).cloned() {
                m.content = content;
                m.edited_timestamp = Some(chrono::Local::now().to_rfc3339());
                self.store.replace_message(m);
            }
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                client
                    .edit_message(&e.channel_id, &e.message_id, &content)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Edited,
        )
    }

    pub(super) fn delete_message(&mut self) -> Task<Message> {
        let Some(message_id) = self.confirm_delete.take() else {
            return Task::none();
        };
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if !self.store.can_delete(&channel_id, &message_id) {
            return Task::none();
        }
        if self.demo {
            self.forget_message(&message_id);
            self.store.delete_message(&MessageDelete {
                id: message_id,
                channel_id,
            });
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = client
                    .delete_message(&channel_id, &message_id)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (channel_id, message_id, result)
            },
            |(channel_id, message_id, result)| Message::Deleted {
                channel_id,
                message_id,
                result,
            },
        )
    }

    /// A message is gone: the reply, the edit or the confirmation about it too.
    pub(super) fn forget_message(&mut self, message_id: &str) {
        if self.replying.as_ref().is_some_and(|r| r.message_id == message_id) {
            self.replying = None;
        }
        if self.editing.as_ref().is_some_and(|e| e.message_id == message_id) {
            self.cancel_edit();
        }
        if self.confirm_delete.as_deref() == Some(message_id) {
            self.confirm_delete = None;
        }
        if self.reacting.as_deref() == Some(message_id) {
            self.close_picker();
        }
    }

    pub(super) fn react(&mut self, message_id: String, emoji: Emoji, add: bool) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        let Some(m) = self.store.find_message(&channel_id, &message_id) else {
            return Task::none();
        };
        // Already in the state asked (a double click, or an event that
        // arrived meanwhile): nothing to do.
        let mine = m.reactions.iter().any(|r| r.me && r.emoji.same(&emoji));
        if mine == add {
            return Task::none();
        }
        if add && !self.store.can_react(&channel_id, &message_id, &emoji) {
            self.status = Some((
                t(
                    "You do not have permission to add reactions here.",
                    "Non hai il permesso di aggiungere reazioni qui.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if self.demo {
            self.store.local_reaction(&channel_id, &message_id, &emoji, add);
            return self.request_small_images();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        // The count changes when the gateway event arrives (for everyone the
        // same way), not before: no reaction counted twice.
        Task::perform(
            async move {
                client
                    .react(&channel_id, &message_id, &emoji, add)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::Reacted,
        )
    }

    /// The right-click menu of a message of the open channel: only what
    /// the permissions allow (checked again when an action runs).
    pub(super) fn open_message_menu(&mut self, message_id: String, at: iced::Point) {
        let Some(channel_id) = self.store.current.clone() else {
            return;
        };
        let Some(m) = self.store.find_message(&channel_id, &message_id) else {
            return;
        };
        let copy_text = !m.content.trim().is_empty();
        self.message_menu = Some(context::Menu {
            at,
            copy_text,
            react: self.store.can(
                &channel_id,
                permissions::VIEW_CHANNEL | permissions::READ_MESSAGE_HISTORY | permissions::ADD_REACTIONS,
            ),
            reply: self.store.can_reply(&channel_id),
            edit: self.store.can_edit(&channel_id, &message_id),
            delete: self.store.can_delete(&channel_id, &message_id),
            message_id,
        });
    }

    pub(super) fn message_action(&mut self, action: context::Action) -> Task<Message> {
        let Some(menu) = self.message_menu.take() else {
            return Task::none();
        };
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        let id = menu.message_id;
        match action {
            context::Action::CopyText => self
                .store
                .find_message(&channel_id, &id)
                .map_or_else(Task::none, |m| iced::clipboard::write(m.content.clone())),
            context::Action::CopyLink => {
                let guild = self
                    .store
                    .channel_guild
                    .get(&channel_id)
                    .cloned()
                    .unwrap_or_else(|| "@me".to_string());
                iced::clipboard::write(format!("https://discord.com/channels/{guild}/{channel_id}/{id}"))
            }
            context::Action::CopyId => iced::clipboard::write(id),
            context::Action::Reply => self.update(Message::StartReply(id)),
            context::Action::React => self.update(Message::OpenReactionPicker(id)),
            context::Action::Edit => self.update(Message::StartEdit(id)),
            context::Action::Delete => self.update(Message::AskDelete(id)),
        }
    }

    /// The attach button: the system's file dialog, off the interface thread.
    /// The chosen files go through the same checks as dropped ones.
    pub(super) fn pick_files(&mut self) -> Task<Message> {
        if self.picking_files || self.screen != Screen::Main || self.editing.is_some() {
            return Task::none();
        }
        self.picking_files = true;
        // Starts from the Documents folder (otherwise Windows may open the
        // program's working folder); then it remembers the last one.
        let start =
            directories::UserDirs::new().and_then(|d| d.document_dir().map(std::path::Path::to_path_buf));
        Task::perform(
            async move {
                let mut dialog = rfd::AsyncFileDialog::new().set_title(t("Attach files", "Allega file"));
                if let Some(dir) = start {
                    dialog = dialog.set_directory(dir);
                }
                dialog
                    .pick_files()
                    .await
                    .map(|files| {
                        files
                            .into_iter()
                            .take(rest::MAX_UPLOAD_FILES)
                            .map(|f| f.path().to_path_buf())
                            .collect()
                    })
                    .unwrap_or_default()
            },
            Message::FilesPicked,
        )
    }

    pub(super) fn file_dropped(&mut self, path: PathBuf) -> Task<Message> {
        let Some(channel_id) = self.store.current.clone() else {
            return Task::none();
        };
        if self.screen != Screen::Main || self.editing.is_some() {
            return Task::none();
        }
        if !self.store.can_attach(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to send files in this channel.",
                    "Non hai il permesso di inviare file in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        if self.pending_files.len() + self.reading_files >= rest::MAX_UPLOAD_FILES {
            self.status = Some((
                t(
                    "At most 10 files per message.",
                    "Al massimo 10 file per messaggio.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        let room = match compose::room_for_one_more(&self.pending_files) {
            Ok(room) => room,
            Err(e) => {
                self.status = Some((e, true));
                return Task::none();
            }
        };
        self.reading_files += 1;
        Task::perform(compose::read_file(path, room), Message::FileRead)
    }
}

//! People and servers: profiles, the right-click menus, moderation, direct messages, our status.

use super::{context, media, now_ms, profile, t, voice, window, Gui, Message, ModAction, Screen, Task};

impl Gui {
    pub(super) fn open_profile(&mut self, user_id: String, guild_id: Option<String>) -> Task<Message> {
        let mut view = profile::ProfileView::new(user_id.clone(), guild_id.clone());
        let key = (user_id.clone(), guild_id.clone());
        if let Some(p) = self.profile_cache.get(&key) {
            view.data = profile::Data::Ready(p.clone());
            self.profile = Some(view);
            return self.request_small_images();
        }
        if self.demo {
            view.data = match profile::demo_profile(&self.store, &user_id, guild_id.as_deref()) {
                Some(p) => profile::Data::Ready(Box::new(p)),
                None => profile::Data::Failed(t("unknown user", "utente sconosciuto").into()),
            };
            self.profile = Some(view);
            return self.request_small_images();
        }
        self.profile = Some(view);
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        let images = self.request_small_images();
        let fetch = Task::perform(
            async move {
                let result = rest
                    .profile(&user_id, guild_id.as_deref())
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                (user_id, guild_id, result)
            },
            |(user_id, guild_id, result)| Message::ProfileLoaded {
                user_id,
                guild_id,
                result,
            },
        );
        Task::batch([images, fetch])
    }

    /// Sends a moderation action, after checking again the permissions and
    /// the role hierarchy. The answer arrives as `Moderated`.
    pub(super) fn send_moderation(
        &mut self,
        guild_id: String,
        user_id: String,
        action: ModAction,
    ) -> Task<Message> {
        let allowed = self.store.moderation(&guild_id, &user_id);
        let ok = match &action {
            ModAction::Kick => allowed.kick,
            ModAction::Ban => allowed.ban,
            ModAction::Timeout(_) => allowed.timeout,
            ModAction::Mute(_) => allowed.mute,
            ModAction::Deafen(_) => allowed.deafen,
            ModAction::Move(_) | ModAction::Disconnect => allowed.move_members,
        };
        if !ok {
            return Task::none();
        }
        if self.demo {
            // Nothing is sent in the demo: the effect is shown locally.
            return Task::done(Message::Moderated {
                user_id,
                guild_id,
                action,
                result: Ok(()),
            });
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = rest
                    .moderate(&guild_id, &user_id, &action)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (user_id, guild_id, action, result)
            },
            |(user_id, guild_id, action, result)| Message::Moderated {
                user_id,
                guild_id,
                action,
                result,
            },
        )
    }

    pub(super) fn moderate(&mut self, action: ModAction, confirmed: bool) -> Task<Message> {
        let Some(p) = self.profile.as_mut() else {
            return Task::none();
        };
        let Some(guild_id) = p.guild_id.clone() else {
            return Task::none();
        };
        let user_id = p.user_id.clone();
        if p.busy {
            return Task::none();
        }
        // Only what the permissions and the role hierarchy allow.
        let allowed = self.store.moderation(&guild_id, &user_id);
        let ok = match &action {
            ModAction::Kick => allowed.kick,
            ModAction::Ban => allowed.ban,
            ModAction::Timeout(_) => allowed.timeout,
            ModAction::Mute(_) => allowed.mute,
            ModAction::Deafen(_) => allowed.deafen,
            ModAction::Move(_) | ModAction::Disconnect => allowed.move_members,
        };
        if !ok {
            return Task::none();
        }
        let needs_confirmation = matches!(
            action,
            ModAction::Kick | ModAction::Ban | ModAction::Timeout(Some(_))
        );
        if needs_confirmation && !confirmed {
            p.pending = Some(action);
            p.outcome = None;
            return Task::none();
        }
        p.pending = None;
        p.outcome = None;
        if self.demo {
            // Nothing is sent in the demo: the effect is shown locally.
            return Task::done(Message::Moderated {
                user_id,
                guild_id,
                action,
                result: Ok(()),
            });
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        p.busy = true;
        Task::perform(
            async move {
                let result = rest
                    .moderate(&guild_id, &user_id, &action)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (user_id, guild_id, action, result)
            },
            |(user_id, guild_id, action, result)| Message::Moderated {
                user_id,
                guild_id,
                action,
                result,
            },
        )
    }

    pub(super) fn open_server_menu(&mut self, guild_id: String, at: iced::Point) {
        use context::ServerAction as A;
        let Some(g) = self.store.guild(&guild_id) else {
            return;
        };
        let mut actions = Vec::new();
        if !self.store.guild_unread(&guild_id).is_empty() {
            actions.push(A::MarkRead);
        }
        actions.push(A::Mute(!self.store.guild_muted(&guild_id)));
        actions.push(A::Folders);
        actions.push(A::CopyId);
        // The owner cannot leave (Discord refuses: the server must be
        // handed over or deleted first).
        if g.owner_id() != self.store.me_id() {
            actions.push(A::Leave);
        }
        self.server_menu = Some(context::ServerMenu {
            name: g.name().to_string(),
            guild_id,
            at,
            actions,
            confirm_leave: false,
        });
    }

    pub(super) fn server_action(&mut self, action: context::ServerAction) -> Task<Message> {
        use context::ServerAction as A;
        let Some(menu) = self.server_menu.as_mut() else {
            return Task::none();
        };
        if !menu.actions.contains(&action) {
            return Task::none();
        }
        if action == A::Leave {
            menu.confirm_leave = true;
            return Task::none();
        }
        let Some(menu) = self.server_menu.take() else {
            return Task::none();
        };
        let guild_id = menu.guild_id;
        match action {
            A::MarkRead => {
                let channels = self.store.guild_unread(&guild_id);
                for (c, m) in &channels {
                    self.store.mark_read(c, Some(m));
                }
                let Some(client) = self.rest.clone().filter(|_| !self.demo) else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        for chunk in channels.chunks(crate::discord::rest::MAX_BULK_ACKS) {
                            if let Err(e) = client.ack_bulk(chunk).await {
                                tracing::debug!("mark as read not sent: {e:#}");
                            }
                        }
                    },
                    |()| Message::Noop,
                )
            }
            A::Mute(muted) => {
                if self.demo {
                    self.store
                        .set_notify_settings(crate::discord::models::GuildNotifySettings {
                            guild_id: Some(guild_id),
                            muted,
                            mute_config: None,
                            message_notifications: 3,
                            suppress_everyone: false,
                            suppress_roles: false,
                            channel_overrides: Vec::new(),
                        });
                    return Task::none();
                }
                let Some(client) = self.rest.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        client
                            .set_guild_muted(&guild_id, muted)
                            .await
                            .map(Box::new)
                            .map_err(|e| format!("{e:#}"))
                    },
                    Message::GuildMuted,
                )
            }
            A::Folders => self.update(Message::OpenFolderEditor),
            A::CopyId => {
                self.status = Some((t("Server ID copied.", "ID del server copiato.").into(), false));
                iced::clipboard::write(guild_id)
            }
            A::Leave => Task::none(),
        }
    }

    /// "Leave" in the confirmation: Discord removes the server, then the
    /// gateway's `GUILD_DELETE` takes it out of the column.
    pub(super) fn leave_server(&mut self) -> Task<Message> {
        let Some(menu) = self.server_menu.take().filter(|m| m.confirm_leave) else {
            return Task::none();
        };
        if self.demo {
            self.status = Some((
                t(
                    "Nothing is sent in the demo: you are still in the server.",
                    "Nella demo non si invia nulla: sei ancora nel server.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        let guild_id = menu.guild_id;
        Task::perform(
            async move { client.leave_guild(&guild_id).await.map_err(|e| format!("{e:#}")) },
            Message::LeftGuild,
        )
    }

    /// A click on a notification: the window to the front, on that channel.
    pub(super) fn open_from_notification(&mut self, channel_id: String) -> Task<Message> {
        if !media::valid_id(&channel_id) || self.screen != Screen::Main {
            return Task::none();
        }
        let guild = match self.store.channel_guild.get(&channel_id) {
            Some(g) => match self.store.guilds.iter().position(|x| &x.id == g) {
                Some(i) => Some(i),
                None => return Task::none(),
            },
            None if self.store.is_dm(&channel_id) => None,
            None => return Task::none(),
        };
        if !self.store.can(&channel_id, crate::permissions::VIEW_CHANNEL) {
            return Task::none();
        }
        self.guild = guild;
        let open = self.open_channel(channel_id);
        Task::batch([window::latest().and_then(window::gain_focus), open])
    }

    /// The right-click menu of a person: only what is possible and allowed
    /// (checked again when an action runs).
    pub(super) fn open_user_menu(&mut self, user_id: String, guild_id: Option<String>, at: iced::Point) {
        use context::UserAction as A;
        if !media::valid_id(&user_id) {
            return;
        }
        // Only a server we are in.
        let guild_id = guild_id.filter(|g| self.store.guild(g).is_some());
        let me = self.store.me_id() == Some(user_id.as_str());
        let mut actions = vec![A::Profile];
        if self
            .store
            .current
            .as_deref()
            .is_some_and(|c| self.store.can_send(c))
        {
            actions.push(A::Mention);
        }
        if !me {
            actions.push(A::Message);
            let muted = self.settings.audio.muted_users.contains(&user_id);
            actions.push(A::LocalMute(!muted));
            // In our end-to-end encrypted call: their verification code.
            if let Some(v) = self.voice.as_ref().filter(|v| {
                v.is_live() && matches!(v.encryption, crate::discord::voice::Encryption::Active { .. })
            }) {
                if self.store.voice.get(&user_id) == Some(&(v.guild_id.clone(), v.channel_id.clone())) {
                    actions.push(A::Verify);
                }
            }
        }
        if let Some(g) = guild_id.as_deref() {
            let allowed = self.store.moderation(g, &user_id);
            let flags = self.store.voice_status.get(&user_id);
            if allowed.in_voice.is_some() {
                if allowed.mute {
                    actions.push(A::ServerMute(!flags.is_some_and(|f| f.mute)));
                }
                if allowed.deafen {
                    actions.push(A::ServerDeafen(!flags.is_some_and(|f| f.deaf)));
                }
                if allowed.move_members {
                    actions.push(A::Disconnect);
                }
            }
            if allowed.timeout {
                actions.push(A::Timeout);
            }
            if allowed.kick {
                actions.push(A::Kick);
            }
            if allowed.ban {
                actions.push(A::Ban);
            }
        }
        if self.store.users.contains_key(&user_id) {
            actions.push(A::CopyName);
        }
        actions.push(A::CopyId);
        self.user_menu = Some(context::UserMenu {
            user_id,
            guild_id,
            at,
            actions,
        });
    }

    /// Opens the direct messages with someone (Discord makes them if there
    /// are none yet), on a click of the user.
    pub(super) fn message_user(&mut self, user_id: String) -> Task<Message> {
        if let Some(dm) = self.store.dm_with(&user_id).map(str::to_string) {
            let task = self.update(Message::SelectGuild(None));
            return Task::batch([task, self.update(Message::OpenChannel(dm))]);
        }
        if self.demo {
            self.status = Some((
                t(
                    "Nothing is sent in the demo: no new conversation.",
                    "Nella demo non si invia nulla: nessuna nuova conversazione.",
                )
                .into(),
                false,
            ));
            return Task::none();
        }
        let Some(client) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                client
                    .open_dm(&user_id)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"))
            },
            Message::DmOpened,
        )
    }

    pub(super) fn user_action(&mut self, action: context::UserAction) -> Task<Message> {
        use context::UserAction as A;
        let Some(menu) = self.user_menu.take() else {
            return Task::none();
        };
        if !menu.actions.contains(&action) {
            return Task::none();
        }
        let (user_id, guild_id) = (menu.user_id.clone(), menu.guild_id.clone());
        // Moderation from the menu: mute, deafen and disconnect at once;
        // kick, ban and timeout in a small window at the menu's place.
        let popup = |gui: &mut Self, step: context::ModStep| {
            if let Some(g) = guild_id.clone() {
                let name = gui
                    .store
                    .users
                    .get(&user_id)
                    .map_or_else(|| user_id.clone(), |u| gui.store.name_in(Some(&g), u));
                gui.mod_popup = Some(context::ModPopup {
                    user_id: user_id.clone(),
                    guild_id: g,
                    name,
                    at: menu.at,
                    step,
                });
            }
            Task::none()
        };
        let direct = |gui: &mut Self, action: ModAction| match guild_id.clone() {
            Some(g) => gui.send_moderation(g, user_id.clone(), action),
            None => Task::none(),
        };
        match action {
            A::Profile => self.open_profile(user_id, guild_id),
            A::Mention => {
                if self
                    .store
                    .current
                    .as_deref()
                    .is_some_and(|c| self.store.can_send(c))
                {
                    self.update(Message::MentionUser(user_id))
                } else {
                    Task::none()
                }
            }
            A::Message => self.message_user(user_id),
            A::LocalMute(_) => self.update(Message::ToggleUserMute(user_id)),
            A::Verify => {
                let name = self
                    .store
                    .users
                    .get(&user_id)
                    .map_or_else(|| user_id.clone(), |u| self.store.name_in(guild_id.as_deref(), u));
                let handle = self.voice.as_ref().and_then(|v| v.handle.as_ref());
                self.verification = Some(voice::Verification {
                    user_id: user_id.clone(),
                    name,
                    code: handle.is_none().then_some(None),
                });
                if let Some(h) = handle {
                    h.verification_code(user_id);
                }
                Task::none()
            }
            A::ServerMute(on) => direct(self, ModAction::Mute(on)),
            A::ServerDeafen(on) => direct(self, ModAction::Deafen(on)),
            A::Disconnect => direct(self, ModAction::Disconnect),
            A::Timeout => popup(self, context::ModStep::Timeout),
            A::Kick => popup(self, context::ModStep::Confirm(ModAction::Kick)),
            A::Ban => popup(self, context::ModStep::Confirm(ModAction::Ban)),
            A::CopyName => match self.store.users.get(&user_id) {
                Some(u) => {
                    self.status = Some((t("Username copied.", "Nome utente copiato.").into(), false));
                    iced::clipboard::write(u.username.clone())
                }
                None => Task::none(),
            },
            A::CopyId => self.update(Message::CopyUserId(user_id)),
        }
    }

    /// Saves our status in the account settings (field 11 only) and shows it
    /// at once; in the demo only here.
    pub(super) fn save_status(
        &mut self,
        new: crate::discord::folders::StatusSettings,
        close: bool,
    ) -> Task<Message> {
        if self.status_menu.as_ref().is_some_and(|m| m.saving) {
            return Task::none();
        }
        if self.demo {
            self.store.my_status = Some(new);
            if close {
                self.status_menu = None;
            }
            return Task::none();
        }
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        if let Some(m) = self.status_menu.as_mut() {
            m.saving = true;
            m.error = None;
        }
        let body = crate::discord::folders::encode_status_update(&new);
        // Shown right away; if Discord refuses, the settings are read again.
        self.store.my_status = Some(new);
        self.send_presence();
        Task::perform(
            async move {
                rest.update_settings_proto(&body, None)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            Message::StatusSaved,
        )
    }

    /// Sends our status on the gateway (op 3) if it changed since the last
    /// time: after connecting, after a change here or on another device.
    pub(super) fn send_presence(&mut self) {
        let Some(s) = self.store.my_status.as_ref() else {
            return;
        };
        let Some(link) = self.gateway_link.as_ref() else {
            return;
        };
        let custom = s
            .custom
            .as_ref()
            .filter(|c| c.active(now_ms()))
            .map(|c| (c.text.clone(), c.emoji_name.clone(), c.emoji_id));
        let presence = (s.presence().code(), custom);
        if self.sent_presence.as_ref() == Some(&presence) {
            return;
        }
        link.presence(presence.0, presence.1.clone());
        tracing::info!("status sent: {}", presence.0);
        self.sent_presence = Some(presence);
    }
}

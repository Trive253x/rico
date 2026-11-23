//! Voice calls, watching streams and our own Go Live: joining, leaving and the events of their connections.

use super::{
    demo_shaped, golive, media, permissions, settings, stream, t, voice, voice_net, watch, window,
    window_mode, Gui, Message, Task, VoiceEvent, VoiceState,
};

impl Gui {
    /// A key in the window: the new push-to-talk key while it is being
    /// chosen, otherwise push-to-talk itself (when the system-wide key is
    /// not registered: that one already reports it).
    pub(super) fn window_key(
        &mut self,
        code: &str,
        modifiers: crate::audio::hotkey::Modifiers,
        down: bool,
    ) -> Task<Message> {
        use crate::audio::hotkey::Hotkey;
        if let Some(p) = self.settings_panel.as_mut().filter(|p| p.choosing_key) {
            if !down {
                return Task::none();
            }
            if code == "Escape" {
                p.choosing_key = false;
                return Task::none();
            }
            // A modifier alone, or a key that cannot be used: keep waiting.
            let Some(key) = Hotkey::from_press(code, modifiers) else {
                return Task::none();
            };
            p.choosing_key = false;
            return self.update(Message::Setting(settings::Change::PushToTalkKey(Some(
                key.to_string(),
            ))));
        }
        let Some(key) = self.push_to_talk_key() else {
            return Task::none();
        };
        if self.ptt.key().is_some() || code != key.code {
            return Task::none();
        }
        let held = down && modifiers == key.modifiers;
        if held != self.ptt_window_held {
            self.ptt_window_held = held;
            if let Some(h) = self.voice.as_ref().and_then(|v| v.handle.as_ref()) {
                h.push_to_talk(held);
            }
        }
        Task::none()
    }

    /// Push-to-talk is in use: in a call, turned on, with a key.
    pub(super) fn push_to_talk_key(&self) -> Option<crate::audio::hotkey::Hotkey> {
        if !self.settings.audio.push_to_talk || !self.voice.as_ref().is_some_and(voice::VoiceUi::is_live) {
            return None;
        }
        self.settings
            .audio
            .push_to_talk_key
            .as_deref()
            .and_then(crate::audio::hotkey::Hotkey::parse)
    }

    /// The system-wide push-to-talk key registered exactly while it is in
    /// use (Windows keeps that key for Rico while it is registered).
    pub(super) fn sync_push_to_talk(&mut self) {
        let key = self.push_to_talk_key();
        if key.as_ref().map(ToString::to_string).as_deref() == self.ptt.key() {
            return;
        }
        if key.is_none() {
            self.ptt_window_held = false;
        }
        if let Err(e) = self.ptt.set(key.as_ref()) {
            self.status = Some((e, true));
        }
    }

    pub(super) fn join_voice(&mut self, channel_id: String) {
        if !self.store.can_connect(&channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to join this voice channel.",
                    "Non hai il permesso di entrare in questo canale vocale.",
                )
                .into(),
                true,
            ));
            return;
        }
        if self
            .voice
            .as_ref()
            .is_some_and(|v| v.channel_id == channel_id && v.is_live())
        {
            return;
        }
        let Some(guild_id) = self.store.channel_guild.get(&channel_id).cloned() else {
            return;
        };
        // Our stream belongs to the channel we leave.
        self.stop_golive(None);
        if self.demo || self.gateway_link.is_none() {
            self.leave_voice();
        } else {
            // Switching channel: the new voice state moves us. Sending "leave"
            // first would make its late update close the new connection.
            self.stop_voice_connection();
        }
        self.voice_attempt += 1;
        if !self.demo {
            let Some(link) = &self.gateway_link else {
                self.status = Some((
                    t(
                        "Not connected to Discord: try again shortly.",
                        "Non connesso a Discord: riprova tra poco.",
                    )
                    .into(),
                    true,
                ));
                return;
            };
            link.voice_state(Some(guild_id.clone()), Some(channel_id.clone()));
        }
        tracing::info!("voice: joining a voice channel");
        let mut v = voice::VoiceUi::new(guild_id, channel_id, self.demo, self.voice_attempt);
        // Already in that channel for Discord: our state will not be sent again.
        let me = self.store.me_id().map(str::to_string);
        let already_there = me.and_then(|me| self.store.voice.get(&me).cloned())
            == Some((v.guild_id.clone(), v.channel_id.clone()));
        if !self.demo && already_there {
            v.session_id.clone_from(&self.voice_session);
            v.confirmed = v.session_id.is_some();
        }
        self.voice = Some(v);
    }

    /// Opens a profile card; the data is downloaded only now (or reused
    /// from earlier in this session).
    /// Asks Discord where the preview of the open stream card is.
    /// The small preview of a stream under the mouse: asked if never asked
    /// or older than a minute.
    pub(super) fn hover_stream(&mut self, s: stream::StreamRef) -> Task<Message> {
        let Some(key) = self.store.stream_key(&s.user_id) else {
            return Task::none();
        };
        if media::stream_key(&s.guild_id, &s.channel_id, &s.user_id).as_ref() != Some(&key)
            || !stream::HoverPreview::stale(self.hover_previews.get(&key))
        {
            return Task::none();
        }
        // Only streams still on, and a bounded number of them.
        let store = &self.store;
        self.hover_previews.retain(|k, _| {
            k.rsplit(':')
                .next()
                .and_then(|user| store.stream_key(user))
                .as_ref()
                == Some(k)
        });
        if self.hover_previews.len() >= stream::MAX_HOVER_PREVIEWS {
            return Task::none();
        }
        let mut preview = stream::HoverPreview {
            state: stream::PreviewState::Loading,
            asked: std::time::Instant::now(),
        };
        if self.demo {
            let url = format!("demo://stream/{}", s.user_id);
            let (w, h) = stream::PREVIEW_PX;
            self.remember_small_image(url.clone(), demo_shaped(w, h, media::Shape::Plain));
            preview.state = stream::PreviewState::Ready(url);
            self.hover_previews.insert(key, preview);
            return Task::none();
        }
        self.hover_previews.insert(key, preview);
        self.stream_preview_task(s)
    }

    pub(super) fn fetch_stream_preview(&mut self) -> Task<Message> {
        let Some(v) = &self.stream_view else {
            return Task::none();
        };
        let s = v.stream.clone();
        if self.demo {
            // The demo preview is drawn locally.
            let url = format!("demo://stream/{}", s.user_id);
            if let Some(v) = &mut self.stream_view {
                v.state = stream::PreviewState::Ready(url.clone());
            }
            let (w, h) = stream::PREVIEW_PX;
            self.remember_small_image(url, demo_shaped(w, h, media::Shape::Plain));
            return Task::none();
        }
        self.stream_preview_task(s)
    }

    /// Asks Discord for the preview of a stream; the answer is
    /// [`Message::StreamPreviewLoaded`].
    pub(super) fn stream_preview_task(&self, s: stream::StreamRef) -> Task<Message> {
        let Some(rest) = self.rest.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = rest
                    .stream_preview(&s.guild_id, &s.channel_id, &s.user_id)
                    .await
                    .map_err(|e| format!("{e:#}"));
                (s, result)
            },
            |(stream, result)| Message::StreamPreviewLoaded { stream, result },
        )
    }

    /// Closes our voice connection without telling Discord we left.
    pub(super) fn stop_voice_connection(&mut self) {
        if let Some(handle) = self.voice.take().and_then(|v| v.handle) {
            handle.leave();
        }
    }

    /// "Watch" in the stream card. Discord only lets you watch from inside
    /// the voice channel: we join it first if needed (microphone off).
    /// Full screen on or off, for the stream window.
    pub(super) fn set_fullscreen(&mut self, on: bool) -> Task<Message> {
        if self.fullscreen == on {
            return Task::none();
        }
        self.fullscreen = on;
        if let Some(w) = &mut self.watch {
            w.controls_shown = false;
        }
        window_mode(if on {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        })
    }

    pub(super) fn watch_stream(&mut self, s: stream::StreamRef) -> Task<Message> {
        if !self.store.can_connect(&s.channel_id) {
            self.status = Some((
                t(
                    "You do not have permission to join this voice channel.",
                    "Non hai il permesso di entrare in questo canale vocale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        let Some(key) = self.store.stream_key(&s.user_id) else {
            return Task::none();
        };
        self.stop_watching();
        // Into the channel first: switching channel stops any other stream.
        if self.voice.as_ref().is_none_or(|v| v.channel_id != s.channel_id) {
            self.join_voice(s.channel_id.clone());
        }
        let mut w = watch::WatchUi::new(s, key, self.demo, self.stream_volume);
        if self.demo {
            // No Discord in the demo: a moving test picture.
            w.status = watch::Status::Watching;
            watch::demo_feed(&w.frames);
        }
        self.watch = Some(w);
        self.ask_to_watch();
        // The avatars of the people next to the stream.
        self.request_previews()
    }

    /// Op 20, as soon as we are connected to the stream's voice channel.
    pub(super) fn ask_to_watch(&mut self) {
        let Some(w) = self.watch.as_mut().filter(|w| !w.asked && !w.demo) else {
            return;
        };
        let connected = self.voice.as_ref().is_some_and(|v| {
            v.channel_id == w.stream.channel_id && matches!(v.status, voice::Status::Connected(_))
        });
        let Some(link) = self.gateway_link.as_ref().filter(|_| connected) else {
            return;
        };
        link.watch_stream(w.key.clone());
        w.asked = true;
        w.status = watch::Status::Asking;
    }

    /// Stops watching (op 19) and closes the stream connection; we stay in
    /// the voice channel.
    pub(super) fn stop_watching(&mut self) {
        let Some(w) = self.watch.take() else { return };
        if let Some(h) = &w.handle {
            h.leave();
        }
        if let (true, Some(link)) = (w.asked, &self.gateway_link) {
            link.stop_stream(w.key);
        }
    }

    pub(super) fn on_watch_voice(&mut self, ev: VoiceEvent) {
        let Some(w) = self.watch.as_mut() else { return };
        match ev {
            VoiceEvent::Handle(h) => {
                if self.voice.as_ref().is_some_and(|v| v.deafened) {
                    h.set_deafened(true);
                }
                w.handle = Some(h);
                w.apply_volume();
            }
            VoiceEvent::Connected { .. } => w.status = watch::Status::Watching,
            VoiceEvent::Reconnecting(_) => w.status = watch::Status::Connecting,
            VoiceEvent::Stats(s) => w.stats = s,
            VoiceEvent::Closed(None) => {
                w.status = watch::Status::Ended(
                    t(
                        "The stream connection closed.",
                        "La connessione alla diretta si è chiusa.",
                    )
                    .into(),
                );
            }
            VoiceEvent::Closed(Some(reason)) => w.status = watch::Status::Ended(reason),
            VoiceEvent::AudioError(e) => self.status = Some((e, true)),
            _ => {}
        }
    }

    /// "Stream" in the call panel: the window to choose what, with the
    /// list made off the interface thread.
    pub(super) fn open_golive(&mut self) -> Task<Message> {
        let Some(v) = self
            .voice
            .as_ref()
            .filter(|v| matches!(v.status, voice::Status::Connected(_)))
        else {
            return Task::none();
        };
        if !self.store.can(&v.channel_id, permissions::STREAM) {
            self.status = Some((
                t(
                    "You do not have permission to stream in this channel.",
                    "Non hai il permesso di trasmettere in questo canale.",
                )
                .into(),
                true,
            ));
            return Task::none();
        }
        self.golive_picker = Some(golive::Picker::default());
        let demo = self.demo;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || golive::sources(demo))
                    .await
                    .unwrap_or_default()
            },
            Message::GoLiveSources,
        )
    }

    /// "Go Live" in the picker: the capture starts, then the stream.
    pub(super) fn start_golive(&mut self) {
        let Some(p) = self.golive_picker.as_mut() else {
            return;
        };
        let Some(source) = p.source().cloned() else { return };
        let Some(v) = self
            .voice
            .as_ref()
            .filter(|v| matches!(v.status, voice::Status::Connected(_)))
        else {
            p.error = Some(t("Join a voice channel first.", "Prima entra in un canale vocale.").into());
            return;
        };
        let (guild_id, channel_id) = (v.guild_id.clone(), v.channel_id.clone());
        let Some(me) = self.store.me_id().map(str::to_string) else {
            return;
        };
        let Some(key) = media::stream_key(&guild_id, &channel_id, &me) else {
            return;
        };
        if !self.demo && self.gateway_link.is_none() {
            p.error = Some(
                t(
                    "Not connected to Discord: try again shortly.",
                    "Non connesso a Discord: riprova tra poco.",
                )
                .into(),
            );
            return;
        }
        let preview = std::sync::Arc::new(watch::FrameSlot::default());
        let quality = self.settings.golive_quality;
        let options = crate::capture::Options {
            quality,
            sound: self.settings.golive_sound,
            preview: Some(preview.sink()),
        };
        let running = match crate::capture::start(&source, options) {
            Ok(r) => r,
            Err(e) => {
                p.error = Some(tf!("Could not start: {e:#}", "Impossibile avviare: {e:#}"));
                return;
            }
        };
        self.golive_picker = None;
        self.golive_attempt += 1;
        let sound_error = running.sound_error.clone();
        self.golive = Some(golive::GoLiveUi {
            guild_id: guild_id.clone(),
            channel_id: channel_id.clone(),
            key,
            status: if self.demo {
                golive::Status::Connecting
            } else {
                golive::Status::Starting
            },
            source_name: source.name.clone(),
            quality,
            rtc_server_id: None,
            server: None,
            attempt: self.golive_attempt,
            handle: None,
            stats: voice_net::VoiceStats::default(),
            encryption: voice_net::Encryption::Pending,
            preview,
            demo: self.demo,
            sound_error,
            running,
        });
        if self.demo {
            // Our LIVE badge next to us in the channel, as Discord would show it.
            self.store.streaming.insert(me);
        } else if let Some(link) = &self.gateway_link {
            link.start_stream(guild_id, channel_id);
        }
        tracing::info!("golive: started ({:?}, {})", source.kind, quality.label());
    }

    /// Ends our stream (button, leaving the call, Discord, the window
    /// closed): op 19, connection closed, capture stopped. `reason` is shown.
    pub(super) fn stop_golive(&mut self, reason: Option<String>) {
        let Some(g) = self.golive.take() else { return };
        if let Some(h) = &g.handle {
            h.leave();
        }
        if g.demo {
            if let Some(me) = self.store.me_id().map(str::to_string) {
                self.store.streaming.remove(&me);
            }
        } else if let Some(link) = &self.gateway_link {
            link.stop_stream(g.key.clone());
        }
        tracing::info!("golive: stopped after {} frames", g.stats.video_sent);
        // Dropping `g` stops the capture and releases the screen and the sound.
        drop(g);
        if let Some(r) = reason {
            self.status = Some((r, true));
        }
    }

    pub(super) fn on_golive_voice(&mut self, ev: VoiceEvent) {
        let Some(g) = self.golive.as_mut() else { return };
        match ev {
            VoiceEvent::Handle(h) => {
                // Our stream's connection plays nothing.
                h.set_deafened(true);
                g.handle = Some(h);
            }
            VoiceEvent::Connected { .. } => g.status = golive::Status::Connected,
            VoiceEvent::Reconnecting(_) => g.status = golive::Status::Connecting,
            VoiceEvent::Encryption(e) => g.encryption = e,
            VoiceEvent::Stats(s) => {
                g.stats = s;
                if g.running.has_ended() {
                    self.stop_golive(Some(
                        t(
                            "The streamed window was closed: stream ended.",
                            "La finestra trasmessa è stata chiusa: diretta finita.",
                        )
                        .into(),
                    ));
                }
            }
            VoiceEvent::Closed(reason) => {
                let reason = reason.map(|r| tf!("Stream ended: {r}", "Diretta finita: {r}"));
                self.stop_golive(
                    reason.or_else(|| Some(t("Your stream has ended.", "La tua diretta è finita.").into())),
                );
            }
            VoiceEvent::AudioError(e) => self.status = Some((e, true)),
            _ => {}
        }
    }

    pub(super) fn leave_voice(&mut self) {
        self.stop_golive(None);
        self.stop_watching();
        let Some(v) = self.voice.take() else { return };
        if let Some(handle) = &v.handle {
            handle.leave();
        }
        if v.demo {
            if let Some(me) = self.store.me_id().map(str::to_string) {
                self.store.voice.remove(&me);
            }
        } else if let Some(link) = &self.gateway_link {
            link.voice_state(Some(v.guild_id), None);
        }
    }

    /// Our voice state carries the `session_id` for the voice server; if
    /// Discord removes us from the channel, it closes here too.
    pub(super) fn on_voice_state(&mut self, state: VoiceState) {
        self.store.apply_voice_state(&state, None);
        if self.store.me_id() != Some(state.user_id.as_str()) {
            return;
        }
        self.voice_session = state.channel_id.is_some().then(|| state.session_id.clone());
        let Some(v) = self.voice.as_mut().filter(|v| !v.demo) else {
            return;
        };
        let moved = state.channel_id.as_ref().is_some_and(|c| *c != v.channel_id)
            || (state.channel_id.is_none() && v.confirmed);
        match state.channel_id {
            Some(channel) => {
                if channel != v.channel_id {
                    // Moved by someone else: start again in the new channel.
                    v.channel_id = channel;
                    v.server = None;
                    self.voice_attempt += 1;
                    v.attempt = self.voice_attempt;
                }
                v.session_id = Some(state.session_id);
                v.confirmed = true;
            }
            // A "left" that arrives before Discord confirmed this join belongs
            // to the previous channel: ignored.
            None if v.confirmed => self.voice = None,
            None => {}
        }
        if moved {
            self.stop_golive(Some(
                t(
                    "You left the voice channel: stream ended.",
                    "Sei uscito dal canale vocale: diretta finita.",
                )
                .into(),
            ));
        }
    }

    pub(super) fn on_voice(&mut self, ev: VoiceEvent) {
        let me = self.store.me_id().map(str::to_string);
        let Some(v) = self.voice.as_mut() else { return };
        match ev {
            VoiceEvent::VerificationCode { user_id, code } => {
                if let Some(card) = self.verification.as_mut().filter(|c| c.user_id == user_id) {
                    card.code = Some(code);
                }
            }
            VoiceEvent::Handle(h) => v.handle = Some(h),
            VoiceEvent::Connecting => {
                if v.status != voice::Status::Joining {
                    v.status = voice::Status::Connecting;
                }
            }
            VoiceEvent::Connected { mode } => {
                v.status = voice::Status::Connected(mode);
                if v.demo {
                    if let Some(me) = me {
                        self.store
                            .voice
                            .insert(me, (v.guild_id.clone(), v.channel_id.clone()));
                    }
                }
                // A stream was waiting for us to be in its channel.
                self.ask_to_watch();
            }
            VoiceEvent::Speaking { user_id, speaking } => {
                if speaking {
                    v.speaking.insert(user_id);
                } else {
                    v.speaking.remove(&user_id);
                }
            }
            VoiceEvent::Stats(stats) => v.stats = stats,
            VoiceEvent::Encryption(e) => v.encryption = e,
            VoiceEvent::Microphone(on) => {
                v.mic_on = on;
                if on {
                    v.deafened = false;
                }
            }
            VoiceEvent::AudioError(e) => v.audio_error = Some(e),
            VoiceEvent::Reconnecting(_) => v.status = voice::Status::Reconnecting,
            VoiceEvent::Closed(None) => self.voice = None,
            VoiceEvent::Closed(Some(reason)) => {
                v.status = voice::Status::Failed(reason);
                v.speaking.clear();
                v.handle = None;
                v.mic_on = false;
                // On Discord we would still be in the channel: leave.
                if v.demo {
                    if let Some(me) = me {
                        self.store.voice.remove(&me);
                    }
                } else if let Some(link) = &self.gateway_link {
                    link.voice_state(Some(v.guild_id.clone()), None);
                }
            }
        }
    }

    /// The audio settings to the voice connections (call and stream).
    pub(super) fn push_audio(&self) {
        for h in [
            self.voice.as_ref().and_then(|v| v.handle.as_ref()),
            self.watch.as_ref().and_then(|w| w.handle.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            h.set_audio(self.settings.audio.clone());
        }
    }
}

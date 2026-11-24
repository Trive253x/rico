//! What the GUI listens to: the gateway, the QR login, voice, streams and Go Live (real or demo), timers and the window.

use super::*;

impl Gui {
    pub(super) fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            window::open_events().map(Message::WindowSeen),
            window::resize_events().map(|(id, _)| Message::WindowSeen(id)),
        ];
        if !self.demo {
            match &self.token {
                Some(token) => subs.push(keyed(("gateway", self.session), gateway_stream(token.clone()))),
                None if self.screen == Screen::Login => {
                    subs.push(keyed(("qr", self.login.attempt), qr_stream()));
                }
                None => {}
            }
        }
        // Stream window: Esc leaves full screen or goes to the mini player,
        // F11 full screen, the arrows change the volume. Not with the mini
        // player: the keys are for the rest of Rico then. Not while a menu or
        // a small window is open over it either: Esc closes only that.
        if self.watch_keys() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(named),
                    ..
                }) => match named {
                    keyboard::key::Named::Escape => Some(Message::WatchEscape),
                    keyboard::key::Named::F11 => Some(Message::ToggleFullscreen),
                    keyboard::key::Named::ArrowUp => Some(Message::StreamVolumeStep(1)),
                    keyboard::key::Named::ArrowDown => Some(Message::StreamVolumeStep(-1)),
                    _ => None,
                },
                _ => None,
            }));
        }
        // The stream's controls go away when the mouse stays still: a timer
        // only while they are shown.
        if self.watch.as_ref().is_some_and(watch::WatchUi::needs_idle_timer) {
            subs.push(iced::time::every(std::time::Duration::from_millis(500)).map(|_| Message::WatchIdle));
        }
        if self.folder_editor.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseFolderEditor),
                _ => None,
            }));
        }
        if self.golive_picker.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseGoLive),
                _ => None,
            }));
        }
        // Our stream's connection: to the local test server in the demo.
        if let Some(g) = &self.golive {
            if g.demo {
                let me = self.store.me_id().unwrap_or("0").to_string();
                subs.push(keyed(
                    ("golive-demo", g.attempt),
                    golive_demo_stream(g.guild_id.clone(), me, g.feed()),
                ));
            } else if let Some(server) = g.server_info(
                self.store.me_id().unwrap_or_default(),
                self.voice_session.as_deref(),
            ) {
                subs.push(keyed(
                    ("golive", g.attempt, server.endpoint.clone()),
                    golive_stream(server, g.feed(), self.settings.audio.clone()),
                ));
            }
        }
        // Ctrl+, opens the settings; Esc closes them (and the menus).
        if self.message_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseMessageMenu),
                _ => None,
            }));
        } else if self.mod_popup.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseModPopup),
                _ => None,
            }));
        } else if self.server_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseServerMenu),
                _ => None,
            }));
        } else if self.verification.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseVerification),
                _ => None,
            }));
        } else if self.user_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseUserMenu),
                _ => None,
            }));
        } else if self.status_menu.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseStatusMenu),
                _ => None,
            }));
        } else if self.settings_panel.as_ref().is_some_and(|p| !p.choosing_key) {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseSettings),
                _ => None,
            }));
        } else if self.screen == Screen::Main {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Character(c),
                    modifiers,
                    ..
                }) if c.as_str() == "," && modifiers.command() => Some(Message::OpenSettings),
                _ => None,
            }));
        }
        // Keys for push-to-talk: being chosen in the settings, or held in the
        // window when the system-wide key could not be registered.
        let choosing_key = self.settings_panel.as_ref().is_some_and(|p| p.choosing_key);
        if choosing_key || (self.push_to_talk_key().is_some() && self.ptt.key().is_none()) {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    physical_key,
                    modifiers,
                    repeat: false,
                    ..
                }) => Some(Message::WindowKey {
                    code: ptt::code_of(physical_key)?,
                    modifiers: ptt::modifiers_of(modifiers),
                    down: true,
                }),
                iced::Event::Keyboard(keyboard::Event::KeyReleased {
                    physical_key,
                    modifiers,
                    ..
                }) => Some(Message::WindowKey {
                    code: ptt::code_of(physical_key)?,
                    modifiers: ptt::modifiers_of(modifiers),
                    down: false,
                }),
                _ => None,
            }));
        }
        if self.ptt.key().is_some() {
            subs.push(Subscription::run(ptt::events));
        }
        // The focus of the window: for notifications and read positions, and
        // for "like the system", whose theme is looked at again when the
        // window gets the focus (it is changed from outside the app), never
        // on a timer.
        subs.push(event::listen_with(|event, _status, _window| match event {
            iced::Event::Window(window::Event::Focused) => Some(Message::Focus(true)),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::Focus(false)),
            _ => None,
        }));
        if self.settings.notifications && !self.demo && self.screen == Screen::Main {
            subs.push(Subscription::run(notify::events));
        }
        // The stream connection, once Discord said where.
        if let Some(w) = &self.watch {
            let session = self.voice.as_ref().and_then(|v| v.session_id.as_deref());
            if let Some(server) = w.server_info(self.store.me_id().unwrap_or_default(), session) {
                subs.push(keyed(
                    ("watch", w.attempt, server.endpoint.clone()),
                    watch_stream(server, w.frames.sink(), self.settings.audio.clone()),
                ));
            }
        }
        // Esc closes the stream preview.
        if self.stream_view.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseStreamPreview),
                _ => None,
            }));
        }
        if self.role_view.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseRole),
                _ => None,
            }));
        }
        if self.profile.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseProfile),
                _ => None,
            }));
        }
        // Esc closes the full-screen view (even if the text field has focus).
        if self.viewer.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseViewer),
                _ => None,
            }));
        }
        // Voice connection: in the demo towards the local fake server.
        if let Some(v) = self.voice.as_ref().filter(|v| v.is_live()) {
            if v.demo {
                let others: Vec<String> = self
                    .store
                    .voice_members(&v.channel_id)
                    .into_iter()
                    .map(|(id, _)| id)
                    .filter(|id| Some(id.as_str()) != self.store.me_id())
                    .collect();
                let me = self.store.me_id().unwrap_or("0").to_string();
                subs.push(keyed(
                    ("voice-demo", v.attempt),
                    voice_demo_stream(
                        v.guild_id.clone(),
                        v.channel_id.clone(),
                        me,
                        others,
                        self.settings.audio.clone(),
                    ),
                ));
            } else if let Some(server) = v.server_info(self.store.me_id().unwrap_or_default()) {
                subs.push(keyed(
                    ("voice", v.attempt, server.endpoint.clone()),
                    voice_stream(server, self.settings.audio.clone()),
                ));
            }
        }
        // Esc cancels a delete, an edit or a reply (when nothing is open above
        // the chat).
        let composing = self.confirm_delete.is_some() || self.editing.is_some() || self.replying.is_some();
        if composing
            && self.picker.is_none()
            && self.viewer.is_none()
            && self.profile.is_none()
            && self.watch.is_none()
            && self.settings_panel.is_none()
            && self.stream_view.is_none()
            && self.folder_editor.is_none()
        {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ComposerEscape),
                _ => None,
            }));
        }
        // Files dropped on the window go with the next message (only after
        // the user presses Send).
        if self.screen == Screen::Main && self.store.current.is_some() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
                _ => None,
            }));
        }
        // Esc also closes the emoji and GIF picker.
        if self.picker.is_some() && self.viewer.is_none() {
            subs.push(event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ClosePicker),
                _ => None,
            }));
        }
        // Only needed to make "is typing…" disappear when it expires.
        if self.store.has_typing() {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        // Then one update check a day (a timer that fires once a day).
        if self.updates_on() {
            subs.push(iced::time::every(crate::update::CHECK_EVERY).map(|_| Message::UpdateCheck));
        }
        Subscription::batch(subs)
    }
}

/// A subscription known by `id`: kept running while `id` stays the same,
/// restarted when it changes (iced 0.14 only offers this for data that can
/// be hashed, and the connections carry keys and buffers).
pub(super) fn keyed<I>(id: I, stream: impl Stream<Item = Message> + Send + 'static) -> Subscription<Message>
where
    I: std::hash::Hash + 'static,
{
    struct Keyed<I, S> {
        id: I,
        stream: S,
    }
    impl<I, S> subscription::Recipe for Keyed<I, S>
    where
        I: std::hash::Hash + 'static,
        S: Stream<Item = Message> + Send + 'static,
    {
        type Output = Message;

        fn hash(&self, state: &mut subscription::Hasher) {
            use std::hash::Hash;
            std::any::TypeId::of::<I>().hash(state);
            self.id.hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: subscription::EventStream,
        ) -> futures_util::stream::BoxStream<'static, Message> {
            Box::pin(self.stream)
        }
    }
    subscription::from_recipe(Keyed { id, stream })
}

/// Gateway as a stream of messages: it stops by itself when iced drops the subscription.
pub(super) fn gateway_stream(token: String) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<GatewayEvent>();
        let (link, commands) = gateway::commands();
        let _ = output.send(Message::GatewayLink(link)).await;
        let worker = gateway::run(token, tx, commands);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Gateway(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

pub(super) fn voice_stream(
    server: voice_net::VoiceServer,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let worker = voice_net::run(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            tx,
        );
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Voice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Connection to a Go Live stream: audio and video; pictures go to `sink`.
pub(super) fn watch_stream(
    server: voice_net::VoiceServer,
    sink: crate::video::decoder::Sink,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    // The sink is used once: the subscription with the same ID is not restarted.
    let sink = std::sync::Mutex::new(Some(sink));
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let sink = sink.lock().ok().and_then(|mut s| s.take());
        let worker = voice_net::run_with_video(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            sink,
            tx,
        );
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::WatchVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Our stream's connection to Discord.
pub(super) fn golive_stream(
    server: voice_net::VoiceServer,
    feed: voice_net::GoLiveFeed,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        // No devices: this connection plays nothing and has no microphone.
        let options = voice_net::VoiceOptions {
            devices: false,
            audio,
            golive: Some(feed),
        };
        let worker = voice_net::run(server, options, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::GoLiveVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

/// Demo: our stream goes to a local fake voice server, where a fake viewer
/// end-to-end decrypts it (and asks for a keyframe, as viewers do).
pub(super) fn golive_demo_stream(
    guild_id: String,
    me: String,
    feed: voice_net::GoLiveFeed,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let config = voice_net::mock::MockConfig {
            users: vec!["901".into(), "902".into()],
            ask_keyframe_after: Some(60),
            ..Default::default()
        };
        let mock = match voice_net::mock::start(config).await {
            Ok(m) => m,
            Err(e) => {
                let _ = output
                    .send(Message::GoLiveVoice(VoiceEvent::Closed(Some(format!(
                        "demo voice server not started: {e:#}"
                    )))))
                    .await;
                return;
            }
        };
        let server = voice_net::VoiceServer {
            endpoint: mock.endpoint.clone(),
            token: "demo".into(),
            guild_id,
            channel_id: voice_net::mock::CHANNEL_ID.to_string(),
            session_id: "demo".into(),
            user_id: me,
            stream: Some((voice_net::mock::CHANNEL_ID + 1).to_string()),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        let options = voice_net::VoiceOptions {
            devices: false,
            audio: AudioSettings::default(),
            golive: Some(feed),
        };
        let worker = voice_net::run(server, options, tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::GoLiveVoice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
        drop(mock);
    })
}

/// Demo: local fake voice server, with the others in the channel talking in
/// turn. The client is the same one used with Discord.
pub(super) fn voice_demo_stream(
    guild_id: String,
    channel_id: String,
    me: String,
    others: Vec<String>,
    audio: AudioSettings,
) -> impl Stream<Item = Message> {
    iced::stream::channel(64, move |mut output: Output| async move {
        let config = voice_net::mock::MockConfig {
            users: others,
            channel_id: channel_id.parse().unwrap_or(voice_net::mock::CHANNEL_ID),
            ..Default::default()
        };
        let mock = match voice_net::mock::start(config).await {
            Ok(m) => m,
            Err(e) => {
                let _ = output
                    .send(Message::Voice(VoiceEvent::Closed(Some(format!(
                        "demo voice server not started: {e:#}"
                    )))))
                    .await;
                return;
            }
        };
        let server = voice_net::VoiceServer {
            endpoint: mock.endpoint.clone(),
            token: "demo".into(),
            guild_id,
            channel_id,
            session_id: "demo".into(),
            user_id: me,
            stream: None,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VoiceEvent>();
        // Speakers play the synthetic voices; the microphone only opens on request.
        let worker = voice_net::run(
            server,
            voice_net::VoiceOptions {
                devices: true,
                audio,
                golive: None,
            },
            tx,
        );
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Voice(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
        drop(mock);
    })
}

pub(super) fn qr_stream() -> impl Stream<Item = Message> {
    iced::stream::channel(16, |mut output: Output| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AuthEvent>();
        let worker = remote_auth::run(tx);
        let forward = async move {
            while let Some(ev) = rx.recv().await {
                if output.send(Message::Auth(ev)).await.is_err() {
                    break;
                }
            }
        };
        tokio::join!(worker, forward);
    })
}

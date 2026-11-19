//! The settings window: everything Rico itself can change, grouped like
//! Discord's (account, appearance, accessibility, voice, text and images,
//! language, shortcuts, about). Settings that live on the Discord account
//! (status, privacy, DMs) are not here yet: nothing here is sent to Discord.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, opaque, pick_list, row, rule, scrollable, slider, space, text, toggler,
    Column, Space,
};
use iced::{Element, Length, Padding};

use super::style::*;
use super::Message;
use crate::audio::{AudioSettings, MAX_GAIN};
use crate::i18n::{self, t};
use crate::settings::{MemberStyle, Settings, ThemeChoice, MAX_ZOOM, MIN_ZOOM};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Account,
    Appearance,
    Accessibility,
    Voice,
    TextImages,
    Language,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Self; 8] = [
        Self::Account,
        Self::Appearance,
        Self::Accessibility,
        Self::Voice,
        Self::TextImages,
        Self::Language,
        Self::Shortcuts,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Account => t("My account", "Il mio account"),
            Self::Appearance => t("Appearance", "Aspetto"),
            Self::Accessibility => t("Accessibility", "Accessibilità"),
            Self::Voice => t("Voice & video", "Voce e video"),
            Self::TextImages => t("Text & images", "Testo e immagini"),
            Self::Language => t("Language", "Lingua"),
            Self::Shortcuts => t("Keyboard shortcuts", "Scorciatoie da tastiera"),
            Self::About => t("About Rico", "Informazioni su Rico"),
        }
    }
}

/// A change made in the window.
#[derive(Debug, Clone)]
pub enum Change {
    Theme(ThemeChoice),
    MemberStyle(MemberStyle),
    CheckUpdates(bool),
    UpdateCandidates(bool),
    Zoom(f32),
    Compact(bool),
    Clock24h(bool),
    Autoplay(bool),
    ShowImages(bool),
    ShowReactions(bool),
    GifPickerPreviews(bool),
    InputDevice(Device),
    OutputDevice(Device),
    InputVolume(f32),
    OutputVolume(f32),
    AutoSensitivity(bool),
    Sensitivity(f32),
    NoiseSuppression(bool),
    EchoCancellation(bool),
    AutoGain(bool),
    PushToTalk(bool),
    /// The push-to-talk key (saved form), or none.
    PushToTalkKey(Option<String>),
    GoLiveQuality(crate::video::encoder::Quality),
    GoLiveSound(bool),
}

impl Change {
    /// A slider being dragged: saved when it is released, not at every step.
    pub fn is_drag(&self) -> bool {
        matches!(
            self,
            Self::Zoom(_) | Self::InputVolume(_) | Self::OutputVolume(_) | Self::Sensitivity(_)
        )
    }

    /// Touches the audio (the voice connections must hear of it).
    pub fn is_audio(&self) -> bool {
        matches!(
            self,
            Self::InputDevice(_)
                | Self::OutputDevice(_)
                | Self::InputVolume(_)
                | Self::OutputVolume(_)
                | Self::AutoSensitivity(_)
                | Self::Sensitivity(_)
                | Self::NoiseSuppression(_)
                | Self::EchoCancellation(_)
                | Self::AutoGain(_)
                | Self::PushToTalk(_)
                | Self::PushToTalkKey(_)
        )
    }
}

/// Applies a change to the settings.
pub fn apply(s: &mut Settings, change: Change) {
    match change {
        Change::Theme(theme) => s.theme = theme,
        Change::MemberStyle(style) => s.member_style = style,
        Change::CheckUpdates(on) => s.check_updates = on,
        Change::UpdateCandidates(on) => s.update_candidates = on,
        Change::Zoom(zoom) => s.zoom = zoom,
        Change::Compact(on) => s.compact = on,
        Change::Clock24h(on) => s.clock_24h = on,
        Change::Autoplay(on) => s.autoplay = on,
        Change::ShowImages(on) => s.show_images = on,
        Change::ShowReactions(on) => s.show_reactions = on,
        Change::GifPickerPreviews(on) => s.gif_picker_previews = on,
        Change::InputDevice(d) => s.audio.input_device = d.0,
        Change::OutputDevice(d) => s.audio.output_device = d.0,
        Change::InputVolume(v) => s.audio.input_volume = v / 100.0,
        Change::OutputVolume(v) => s.audio.output_volume = v / 100.0,
        Change::AutoSensitivity(on) => s.audio.auto_sensitivity = on,
        Change::Sensitivity(db) => s.audio.sensitivity_db = db,
        Change::NoiseSuppression(on) => s.audio.noise_suppression = on,
        Change::EchoCancellation(on) => s.audio.echo_cancellation = on,
        Change::AutoGain(on) => s.audio.auto_gain = on,
        Change::PushToTalk(on) => s.audio.push_to_talk = on,
        Change::PushToTalkKey(key) => {
            if key.is_none() {
                s.audio.push_to_talk = false;
            }
            s.audio.push_to_talk_key = key;
        }
        Change::GoLiveQuality(q) => s.golive_quality = q,
        Change::GoLiveSound(on) => s.golive_sound = on,
    }
    *s = s.clone().sanitized();
}

/// An audio device in a list: `None` is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device(pub Option<String>);

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(name) => f.write_str(name),
            None => f.write_str(t("System default", "Predefinito di sistema")),
        }
    }
}

/// State of the open window.
pub struct Panel {
    pub section: Section,
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
    /// Waiting for the key to use for push-to-talk.
    pub choosing_key: bool,
}

impl Panel {
    /// Opens on "Appearance". The audio devices arrive later from
    /// [`list_devices`]: listing them can take a while.
    pub fn new() -> Self {
        Self {
            section: Section::Appearance,
            inputs: vec![Device(None)],
            outputs: vec![Device(None)],
            choosing_key: false,
        }
    }

    pub fn set_devices(&mut self, inputs: Vec<String>, outputs: Vec<String>) {
        let list = |names: Vec<String>| {
            std::iter::once(Device(None))
                .chain(names.into_iter().map(|n| Device(Some(n))))
                .collect()
        };
        self.inputs = list(inputs);
        self.outputs = list(outputs);
    }
}

/// Lists microphones and speakers off the interface thread (the system can
/// take tens of milliseconds); the answer is [`Message::AudioDevices`].
pub fn list_devices() -> iced::Task<Message> {
    iced::Task::perform(
        async {
            tokio::task::spawn_blocking(|| {
                (
                    crate::audio::device::device_names(true),
                    crate::audio::device::device_names(false),
                )
            })
            .await
            .unwrap_or_default()
        },
        |(inputs, outputs)| Message::AudioDevices { inputs, outputs },
    )
}

/// The whole window, over the interface.
pub fn view<'a>(
    p: &'a Panel,
    s: &'a Settings,
    store: &'a Store,
    system_dark: bool,
    update_status: String,
) -> Element<'a, Message> {
    let mut nav = Column::new().spacing(2).width(220);
    nav = nav.push(
        text(t("SETTINGS", "IMPOSTAZIONI"))
            .size(11)
            .font(BOLD)
            .color(pal::muted()),
    );
    nav = nav.push(Space::new().height(6));
    for section in Section::ALL {
        nav = nav.push(
            button(text(section.label()).size(13))
                .width(Length::Fill)
                .padding([6, 10])
                .style(list_item(section == p.section))
                .on_press(Message::SettingsSection(section)),
        );
    }

    let body: Element<'a, Message> = match p.section {
        Section::Account => account(store),
        Section::Appearance => appearance(s, system_dark),
        Section::Accessibility => accessibility(s),
        Section::Voice => voice(p, &s.audio),
        Section::TextImages => text_images(s),
        Section::Language => language(),
        Section::Shortcuts => shortcuts(),
        Section::About => about(s, update_status),
    };

    let close = button(text(t("Close (Esc)", "Chiudi (Esc)")).size(12))
        .on_press(Message::CloseSettings)
        .padding([4, 10])
        .style(ghost);
    let content = column![
        row![
            text(p.section.label()).size(20).font(BOLD),
            space::horizontal(),
            close
        ]
        .align_y(Vertical::Center),
        scrollable(container(body).padding(Padding {
            right: 14.0,
            ..Padding::ZERO
        }))
        .height(Length::Fill)
        .style(thin_scrollbar),
    ]
    .spacing(16)
    .max_width(720);

    opaque(
        container(
            row![
                container(nav)
                    .padding(20)
                    .height(Length::Fill)
                    .style(fill(pal::panel())),
                container(content)
                    .padding(Padding {
                        top: 20.0,
                        right: 24.0,
                        bottom: 20.0,
                        left: 32.0,
                    })
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(fill(pal::bg())),
    )
}

fn heading<'a>(label: &str) -> Element<'a, Message> {
    text(label.to_string())
        .size(11)
        .font(BOLD)
        .color(pal::muted())
        .into()
}

fn note<'a>(label: &str) -> Element<'a, Message> {
    text(label.to_string())
        .size(12)
        .color(pal::muted())
        .shaping(Shaping::Advanced)
        .into()
}

/// A labelled switch with an explanation under it.
fn switch<'a>(label: &str, help: &str, on: bool, change: fn(bool) -> Change) -> Element<'a, Message> {
    column![
        row![
            text(label.to_string()).size(14).shaping(Shaping::Advanced),
            space::horizontal(),
            toggler(on)
                .on_toggle(move |v| Message::Setting(change(v)))
                .size(20),
        ]
        .align_y(Vertical::Center),
        note(help),
    ]
    .spacing(4)
    .into()
}

/// A slider in percent with its value, saved when released.
fn percent_slider<'a>(
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    change: fn(f32) -> Change,
) -> Element<'a, Message> {
    let shown = (value * 100.0).round();
    row![
        slider(range, shown, move |v| Message::Setting(change(v)))
            .step(5.0_f32)
            .on_release(Message::SaveSettings)
            .width(Length::Fill),
        text(format!("{shown:.0}%")).size(13).width(52),
    ]
    .spacing(12)
    .align_y(Vertical::Center)
    .into()
}

fn section(items: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    Column::with_children(items).spacing(14).into()
}

fn account(store: &Store) -> Element<'_, Message> {
    let mut items = vec![];
    if let Some(me) = &store.me {
        let username = me.username.clone();
        items.push(
            container(
                column![
                    text(me.display_name().to_string())
                        .size(18)
                        .font(BOLD)
                        .shaping(Shaping::Advanced),
                    text(username)
                        .size(13)
                        .color(pal::muted())
                        .shaping(Shaping::Advanced),
                ]
                .spacing(4),
            )
            .padding(16)
            .width(Length::Fill)
            .style(rounded(pal::panel(), 8.0))
            .into(),
        );
    }
    items.push(note(t(
        "Status, privacy and direct message settings are saved on your Discord account: they will \
         come in a later version. Email, password and two-factor authentication are changed only \
         on Discord's website or app.",
        "Stato, privacy e impostazioni dei messaggi diretti sono salvati sul tuo account Discord: \
         arriveranno in una prossima versione. Email, password e autenticazione a due fattori si \
         cambiano solo dal sito o dall'app di Discord.",
    )));
    items.push(
        button(text(t("Sign out", "Esci dall'account")).size(13))
            .on_press(Message::Logout)
            .padding([6, 14])
            .style(ghost)
            .into(),
    );
    section(items)
}

fn appearance(s: &Settings, system_dark: bool) -> Element<'_, Message> {
    // Themes: a small preview of each, the chosen one outlined.
    let mut grid = Column::new().spacing(10);
    for chunk in ThemeChoice::ALL.chunks(4) {
        let mut r = row![].spacing(10);
        for &choice in chunk {
            r = r.push(theme_card(choice, s.theme == choice, system_dark));
        }
        grid = grid.push(r);
    }
    let zoom = row![
        slider(
            MIN_ZOOM * 100.0..=MAX_ZOOM * 100.0,
            (s.zoom * 100.0).round(),
            |v| { Message::Setting(Change::Zoom(v / 100.0)) }
        )
        .step(5.0_f32)
        .on_release(Message::SaveSettings)
        .width(Length::Fill),
        text(format!("{:.0}%", s.zoom * 100.0)).size(13).width(52),
        button(text(t("Reset", "Ripristina")).size(12))
            .on_press(Message::Setting(Change::Zoom(1.0)))
            .padding([3, 10])
            .style(ghost),
    ]
    .spacing(12)
    .align_y(Vertical::Center);
    section(vec![
        heading(t("THEME", "TEMA")),
        grid.into(),
        note(t(
            "\"Like the system\" follows Windows: light or dark, also when it changes.",
            "\"Come il sistema\" segue Windows: chiaro o scuro, anche quando cambia.",
        )),
        rule::horizontal(1).into(),
        heading(t("ZOOM", "ZOOM")),
        zoom.into(),
        note(t(
            "Size of the whole interface, on top of the display scaling of Windows.",
            "Dimensione di tutta l'interfaccia, oltre al ridimensionamento dello schermo di Windows.",
        )),
        rule::horizontal(1).into(),
        heading(t("MEMBER LIST", "ELENCO MEMBRI")),
        member_styles(s.member_style),
        note(s.member_style.description()),
        rule::horizontal(1).into(),
        heading(t("MESSAGES", "MESSAGGI")),
        switch(
            t("Compact mode", "Modalità compatta"),
            t(
                "One line per message, without avatars: more messages on screen.",
                "Una riga per messaggio, senza avatar: più messaggi sullo schermo.",
            ),
            s.compact,
            Change::Compact,
        ),
        switch(
            t("24-hour clock", "Orologio a 24 ore"),
            t(
                "Times as 14:05 instead of 2:05 PM.",
                "Orari come 14:05 invece di 2:05 PM.",
            ),
            s.clock_24h,
            Change::Clock24h,
        ),
    ])
}

/// The four ways of showing the member list, the chosen one highlighted.
fn member_styles(current: MemberStyle) -> Element<'static, Message> {
    let mut r = row![].spacing(8);
    for style in MemberStyle::ALL {
        r = r.push(
            button(text(style.label()).size(13))
                .on_press(Message::Setting(Change::MemberStyle(style)))
                .padding([6, 12])
                .style(tab(style == current)),
        );
    }
    r.wrap().into()
}

fn theme_card(choice: ThemeChoice, selected: bool, system_dark: bool) -> Element<'static, Message> {
    let c = colors_of(choice, system_dark);
    let line =
        |w: f32, color: iced::Color| container(Space::new().width(w).height(6.0)).style(rounded(color, 3.0));
    let preview = container(
        row![
            container(Space::new().width(18.0).height(50.0)).style(rounded(c.panel, 4.0)),
            column![line(60.0, c.accent), line(80.0, c.muted), line(48.0, c.text)].spacing(7),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding(8)
    .width(Length::Fill)
    .style(rounded(c.bg, 6.0));
    let label = text(choice.label()).size(12).shaping(Shaping::Advanced);
    button(column![preview, label].spacing(6).align_x(Horizontal::Center))
        .width(150)
        .padding(6)
        .style(theme_choice(selected))
        .on_press(Message::Setting(Change::Theme(choice)))
        .into()
}

fn accessibility(s: &Settings) -> Element<'_, Message> {
    section(vec![switch(
        t("Play GIFs and animated emojis", "Riproduci GIF ed emoji animate"),
        t(
            "When off they stay still and play only while the mouse is over them.",
            "Se disattivato restano ferme e si animano solo passandoci sopra col mouse.",
        ),
        s.autoplay,
        Change::Autoplay,
    )])
}

fn voice<'a>(p: &'a Panel, a: &'a AudioSettings) -> Element<'a, Message> {
    let mut items = vec![];
    if !cfg!(windows) {
        items.push(note(t(
            "Microphone and speakers work only on Windows for now.",
            "Microfono e altoparlanti per ora funzionano solo su Windows.",
        )));
    }
    let input = Device(a.input_device.clone());
    let output = Device(a.output_device.clone());
    items.push(
        row![
            column![
                heading(t("INPUT DEVICE", "DISPOSITIVO DI INPUT")),
                pick_list(p.inputs.as_slice(), Some(input), |d| Message::Setting(
                    Change::InputDevice(d)
                ))
                .width(Length::Fill)
                .text_size(13),
            ]
            .spacing(6)
            .width(Length::FillPortion(1)),
            column![
                heading(t("OUTPUT DEVICE", "DISPOSITIVO DI OUTPUT")),
                pick_list(p.outputs.as_slice(), Some(output), |d| Message::Setting(
                    Change::OutputDevice(d)
                ))
                .width(Length::Fill)
                .text_size(13),
            ]
            .spacing(6)
            .width(Length::FillPortion(1)),
        ]
        .spacing(16)
        .into(),
    );
    items.push(
        button(text(t("Look for devices again", "Cerca di nuovo i dispositivi")).size(12))
            .on_press(Message::RefreshAudioDevices)
            .padding([3, 10])
            .style(ghost)
            .into(),
    );
    items.push(
        row![
            column![
                heading(t("INPUT VOLUME", "VOLUME DI INPUT")),
                percent_slider(a.input_volume, 0.0..=MAX_GAIN * 100.0, Change::InputVolume),
            ]
            .spacing(6)
            .width(Length::FillPortion(1)),
            column![
                heading(t("OUTPUT VOLUME", "VOLUME DI OUTPUT")),
                percent_slider(a.output_volume, 0.0..=MAX_GAIN * 100.0, Change::OutputVolume),
            ]
            .spacing(6)
            .width(Length::FillPortion(1)),
        ]
        .spacing(16)
        .into(),
    );
    items.push(rule::horizontal(1).into());
    items.push(heading(t("INPUT SENSITIVITY", "SENSIBILITÀ DI INPUT")));
    items.push(switch(
        t("Decide automatically", "Determina automaticamente"),
        t(
            "The microphone sends only when you speak louder than the background noise.",
            "Il microfono trasmette solo quando parli più forte del rumore di fondo.",
        ),
        a.auto_sensitivity,
        Change::AutoSensitivity,
    ));
    let threshold = slider(-80.0..=0.0, a.sensitivity_db, |v| {
        Message::Setting(Change::Sensitivity(v))
    })
    .step(1.0_f32)
    .on_release(Message::SaveSettings)
    .width(Length::Fill);
    let mut sensitivity = row![].spacing(12).align_y(Vertical::Center);
    if a.auto_sensitivity {
        sensitivity = sensitivity.push(note(t(
            "Turn off \"Decide automatically\" to choose the level.",
            "Disattiva \"Determina automaticamente\" per scegliere il livello.",
        )));
    } else {
        sensitivity = sensitivity
            .push(threshold)
            .push(text(format!("{:.0} dB", a.sensitivity_db)).size(13).width(60));
    }
    items.push(sensitivity.into());
    items.push(rule::horizontal(1).into());
    items.push(heading(t("INPUT MODE", "MODALITÀ DI INPUT")));
    let key = a
        .push_to_talk_key
        .as_deref()
        .and_then(crate::audio::hotkey::Hotkey::parse);
    let ptt_switch = toggler(a.push_to_talk).size(20);
    let ptt_switch = if key.is_some() {
        ptt_switch.on_toggle(|v| Message::Setting(Change::PushToTalk(v)))
    } else {
        ptt_switch
    };
    items.push(
        column![
            row![
                text(t("Push to talk", "Push-to-talk")).size(14),
                space::horizontal(),
                ptt_switch,
            ]
            .align_y(Vertical::Center),
            note(t(
                "The microphone sends only while you hold the key, also when Rico is in the background. Off: it sends when it hears your voice.",
                "Il microfono trasmette solo mentre tieni premuto il tasto, anche con Rico in secondo piano. Spento: trasmette quando sente la tua voce.",
            )),
        ]
        .spacing(4)
        .into(),
    );
    let mut key_row = row![text(t("Key", "Tasto")).size(13).width(60)]
        .spacing(10)
        .align_y(Vertical::Center);
    if p.choosing_key {
        key_row = key_row
            .push(
                container(
                    text(t(
                        "Press the key… (Esc to cancel)",
                        "Premi il tasto… (Esc per annullare)",
                    ))
                    .size(13),
                )
                .padding([4, 10])
                .style(outlined),
            )
            .push(
                button(text(t("Cancel", "Annulla")).size(12))
                    .on_press(Message::ChoosePushToTalkKey(false))
                    .padding([3, 10])
                    .style(ghost),
            );
    } else {
        match &key {
            Some(k) => {
                key_row = key_row
                    .push(
                        container(text(k.label()).size(13).font(SEMIBOLD))
                            .padding([4, 10])
                            .style(outlined),
                    )
                    .push(
                        button(text(t("Change", "Cambia")).size(12))
                            .on_press(Message::ChoosePushToTalkKey(true))
                            .padding([3, 10])
                            .style(ghost),
                    )
                    .push(
                        button(text(t("Remove", "Rimuovi")).size(12))
                            .on_press(Message::Setting(Change::PushToTalkKey(None)))
                            .padding([3, 10])
                            .style(ghost),
                    );
            }
            None => {
                key_row = key_row.push(
                    button(text(t("Choose a key", "Scegli un tasto")).size(12))
                        .on_press(Message::ChoosePushToTalkKey(true))
                        .padding([3, 10])
                        .style(primary),
                );
            }
        }
    }
    items.push(key_row.into());
    if key.is_some() {
        items.push(note(t(
            "While you are in a call with push-to-talk on, Windows gives this key to Rico only: pick one you do not type with (F13–F24, a mouse side button mapped to a key, Numpad keys, `).",
            "Durante una chiamata con il push-to-talk acceso, Windows dà questo tasto solo a Rico: scegline uno che non usi per scrivere (F13–F24, un tasto laterale del mouse mappato su un tasto, il tastierino, `).",
        )));
    }
    items.push(rule::horizontal(1).into());
    items.push(heading(t("VOICE PROCESSING", "ELABORAZIONE DELLA VOCE")));
    items.push(switch(
        t("Noise suppression", "Soppressione del rumore"),
        t(
            "Turns down steady background noise (fans, keys, traffic) before your voice is sent.",
            "Abbassa il rumore di fondo costante (ventole, tasti, traffico) prima di inviare la tua voce.",
        ),
        a.noise_suppression,
        Change::NoiseSuppression,
    ));
    items.push(switch(
        t("Echo cancellation", "Cancellazione dell'eco"),
        t(
            "Removes from your microphone what your speakers play, so others do not hear themselves when you have no headphones.",
            "Toglie dal microfono quello che suonano gli altoparlanti, così gli altri non si sentono quando non usi le cuffie.",
        ),
        a.echo_cancellation,
        Change::EchoCancellation,
    ));
    items.push(switch(
        t("Automatic gain control", "Controllo automatico del guadagno"),
        t(
            "Evens out your microphone level when you speak softly or loudly.",
            "Uniforma il livello del microfono quando parli piano o forte.",
        ),
        a.auto_gain,
        Change::AutoGain,
    ));
    items.push(note(t(
        "Changes apply at once, also during a call. The microphone still opens only with its button.",
        "Le modifiche valgono subito, anche durante una chiamata. Il microfono si apre sempre solo col suo pulsante.",
    )));
    section(items)
}

fn text_images(s: &Settings) -> Element<'_, Message> {
    section(vec![
        switch(
            t("Show images and GIFs", "Mostra immagini e GIF"),
            t(
                "Attachments, links and GIFs are downloaded and shown in the chat. When off only \
                 their name is shown and nothing is downloaded.",
                "Allegati, link e GIF vengono scaricati e mostrati nella chat. Se disattivato si vede \
                 solo il nome e non si scarica niente.",
            ),
            s.show_images,
            Change::ShowImages,
        ),
        switch(
            t("Show reactions", "Mostra le reazioni"),
            t(
                "The emoji reactions under the messages.",
                "Le reazioni con emoji sotto i messaggi.",
            ),
            s.show_reactions,
            Change::ShowReactions,
        ),
        switch(
            t("Previews in the GIF picker", "Anteprime nel selettore GIF"),
            t(
                "The GIF picker's images come from Klipy (Discord's GIF provider), which then sees \
                 your IP address — not your account or your messages. When off the picker shows only \
                 the titles; sending GIFs and the GIFs in the chat work the same.",
                "Le immagini del selettore GIF arrivano da Klipy (il fornitore delle GIF di Discord), \
                 che vede così il tuo indirizzo IP — non il tuo account né i tuoi messaggi. Se \
                 disattivato il selettore mostra solo i titoli; l'invio delle GIF e le GIF nella chat \
                 funzionano lo stesso.",
            ),
            s.gif_picker_previews,
            Change::GifPickerPreviews,
        ),
    ])
}

fn language<'a>() -> Element<'a, Message> {
    let current = i18n::lang();
    let choice = |lang: i18n::Lang, label: &'static str| {
        button(text(label).size(14))
            .width(Length::Fill)
            .padding([8, 12])
            .style(list_item(current == lang))
            .on_press_maybe((current != lang).then_some(Message::ToggleLanguage))
    };
    section(vec![
        choice(i18n::Lang::En, "English").into(),
        choice(i18n::Lang::It, "Italiano").into(),
        note(t(
            "Also with the EN / IT button in the top bar.",
            "Anche con il pulsante EN / IT nella barra in alto.",
        )),
    ])
}

fn shortcuts<'a>() -> Element<'a, Message> {
    let rows: [(&str, &str); 7] = [
        ("Enter", t("Send the message", "Invia il messaggio")),
        (
            "Esc",
            t(
                "Close a window, a card or the picker",
                "Chiudi una finestra, una scheda o il selettore",
            ),
        ),
        ("Esc", t("Stop watching a stream", "Esci dalla diretta")),
        (
            "↑ ↓",
            t(
                "Stream volume, while watching",
                "Volume della diretta, mentre la guardi",
            ),
        ),
        ("Ctrl + ,", t("Open the settings", "Apri le impostazioni")),
        ("Tab", t("Next field", "Campo successivo")),
        (
            ":name:",
            t("Server emoji in a message", "Emoji del server in un messaggio"),
        ),
    ];
    let mut col = Column::new().spacing(8);
    for (keys, what) in rows {
        col = col.push(
            row![
                container(text(keys).size(12).font(SEMIBOLD))
                    .padding([3, 8])
                    .style(rounded(pal::panel(), 4.0)),
                text(what).size(13).shaping(Shaping::Advanced),
            ]
            .spacing(14)
            .align_y(Vertical::Center),
        );
    }
    col.into()
}

fn about<'a>(s: &Settings, update_status: String) -> Element<'a, Message> {
    let path = |p: Option<std::path::PathBuf>| {
        p.map_or_else(
            || t("not available", "non disponibile").into(),
            |p| p.display().to_string(),
        )
    };
    section(vec![
        text(format!("Rico {}", env!("CARGO_PKG_VERSION")))
            .size(16)
            .font(BOLD)
            .into(),
        note(t(
            "An alternative Discord client written in Rust.",
            "Un client Discord alternativo scritto in Rust.",
        )),
        heading(t("UPDATES", "AGGIORNAMENTI")),
        switch(
            t(
                "Check for updates on GitHub",
                "Controlla gli aggiornamenti su GitHub",
            ),
            t(
                "Once a day Rico asks GitHub for the newest release (GitHub sees your IP address) and \
                 installs only updates signed by Rico's author, never an older version. Nothing \
                 is installed until you press Restart now.",
                "Una volta al giorno Rico chiede a GitHub l'ultima versione (GitHub vede il tuo \
                 indirizzo IP) e installa solo aggiornamenti firmati dall'autore di Rico, mai una \
                 versione più vecchia. Niente viene installato finché non premi Riavvia ora.",
            ),
            s.check_updates,
            Change::CheckUpdates,
        ),
        switch(
            t("Also candidates", "Anche le candidate"),
            t(
                "Test versions published before a release, signed the same way. They can have bugs.",
                "Versioni di prova pubblicate prima di una release, firmate allo stesso modo. Possono avere difetti.",
            ),
            s.update_candidates,
            Change::UpdateCandidates,
        ),
        note(&update_status),
        heading(t("FOLDERS", "CARTELLE")),
        note(&tf!(
            "Settings: {}",
            "Impostazioni: {}",
            path(crate::config::settings_dir())
        )),
        note(&tf!("Logs: {}", "Log: {}", path(crate::config::log_dir()))),
        heading(t("COMPONENTS", "COMPONENTI")),
        note(t(
            "Voice: Opus (libopus) and DAVE end-to-end encryption. Streams: OpenH264 by Cisco, \
             built from source (Cisco's H.264 patent licence does not cover it).",
            "Voce: Opus (libopus) e crittografia end-to-end DAVE. Dirette: OpenH264 di Cisco, \
             compilato dai sorgenti (la licenza brevetti H.264 di Cisco non lo copre).",
        )),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_applied_and_kept_in_range() {
        let mut s = Settings::default();
        apply(&mut s, Change::Theme(ThemeChoice::Purple));
        apply(&mut s, Change::Zoom(3.0));
        apply(&mut s, Change::OutputVolume(150.0));
        apply(&mut s, Change::InputDevice(Device(Some("USB mic".into()))));
        apply(&mut s, Change::Autoplay(false));
        assert_eq!(s.theme, ThemeChoice::Purple);
        assert_eq!(s.zoom, MAX_ZOOM);
        assert_eq!(s.audio.output_volume, 1.5);
        assert_eq!(s.audio.input_device.as_deref(), Some("USB mic"));
        assert!(!s.autoplay);
        assert!(Change::Zoom(1.0).is_drag() && !Change::Compact(true).is_drag());
        assert!(Change::Sensitivity(-30.0).is_audio() && !Change::Theme(ThemeChoice::Red).is_audio());
    }
}

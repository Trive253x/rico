//! Preferences: the settings window, server folders, theme, language and the downloadable voice models.

use super::*;

impl Gui {
    /// The preferences messages of `Gui::handle`.
    pub(super) fn handle_preferences(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::GetDpdfNet(noise) => return self.get_dpdfnet(noise),
            Message::DpdfNetReady(noise, result) => match result {
                Ok(()) => {
                    self.dpdfnet_download = settings::ModelDownload::Idle;
                    return self.update(Message::Setting(settings::Change::NoiseSuppression(noise)));
                }
                Err(e) => {
                    tracing::warn!("noise suppression: DPDFNet model not available: {e}");
                    self.dpdfnet_download = settings::ModelDownload::Failed(e);
                }
            },
            Message::SettingsLoaded(result) => {
                match result.map(|b64| crate::discord::folders::decode_settings(&b64)) {
                    Ok(Ok(snap)) => {
                        self.store.apply_settings(snap);
                        self.send_presence();
                    }
                    Ok(Err(e)) => tracing::warn!("account settings unreadable: {e:#}"),
                    Err(e) => tracing::warn!("account settings unavailable: {e}"),
                }
            }
            Message::ToggleFolder(id) => {
                if !self.open_folders.remove(&id) {
                    self.open_folders.insert(id);
                }
            }
            Message::OpenFolderEditor => {
                if self.screen == Screen::Main && self.folder_editor.is_none() {
                    self.picker = None;
                    self.folder_editor = Some(folders::Editor::new(&self.store));
                }
            }
            Message::CloseFolderEditor => {
                if !self.folder_editor.as_ref().is_some_and(|e| e.saving) {
                    self.folder_editor = None;
                }
            }
            Message::FolderEdit(edit) => {
                if let Some(e) = self.folder_editor.as_mut().filter(|e| !e.saving) {
                    e.apply(edit);
                }
            }
            Message::SaveFolders => return self.save_folders(),
            Message::FoldersSaved(result) => {
                let Some(e) = self.folder_editor.as_mut() else {
                    return Task::none();
                };
                e.saving = false;
                match result {
                    Ok((b64, out_of_date)) => {
                        if let Ok(snap) = crate::discord::folders::decode_settings(&b64) {
                            self.store.apply_settings(snap);
                        }
                        if out_of_date {
                            // Changed meanwhile elsewhere: start again from Discord's.
                            let mut fresh = folders::Editor::new(&self.store);
                            fresh.error = Some(
                                t(
                                    "The folders were changed meanwhile on another device: here they are \
                                     as they are now, make your changes again.",
                                    "Le cartelle sono state cambiate nel frattempo da un altro dispositivo: \
                                     eccole come sono ora, rifai le modifiche.",
                                )
                                .into(),
                            );
                            self.folder_editor = Some(fresh);
                        } else {
                            self.folder_editor = None;
                        }
                    }
                    Err(err) => e.error = Some(tf!("Not saved: {err}", "Non salvate: {err}")),
                }
            }
            Message::OpenSettings => {
                if self.screen == Screen::Main && self.settings_panel.is_none() {
                    self.picker = None;
                    self.settings_panel = Some(settings::Panel::new());
                    return settings::list_devices();
                }
            }
            Message::CloseSettings => self.settings_panel = None,
            Message::ChoosePushToTalkKey(on) => {
                if let Some(p) = &mut self.settings_panel {
                    p.choosing_key = on;
                }
            }
            Message::OpenGifSettings => {
                let task = self.update(Message::OpenSettings);
                if let Some(p) = &mut self.settings_panel {
                    p.section = settings::Section::TextImages;
                }
                return task;
            }
            Message::SettingsSection(section) => {
                if let Some(p) = &mut self.settings_panel {
                    p.section = section;
                }
            }
            Message::Setting(change) => {
                let drag = change.is_drag();
                let audio = change.is_audio();
                let images_before = (self.settings.show_images, self.settings.gif_picker_previews);
                let theme_before = self.settings.theme;
                settings::apply(&mut self.settings, change);
                self.apply_look();
                let mut tasks = Vec::new();
                if self.settings.theme == ThemeChoice::System && theme_before != ThemeChoice::System {
                    tasks.push(check_system_theme());
                }
                if audio {
                    self.push_audio();
                }
                if !drag {
                    tasks.push(self.save_settings());
                }
                if (self.settings.show_images, self.settings.gif_picker_previews) != images_before {
                    tasks.push(self.request_previews());
                }
                return Task::batch(tasks);
            }
            Message::SaveSettings => return self.save_settings(),
            Message::SaveFailed(Some(e)) => self.status = Some((e, true)),
            Message::RefreshAudioDevices => {
                if self.settings_panel.is_some() {
                    return settings::list_devices();
                }
            }
            Message::AudioDevices { inputs, outputs } => {
                if let Some(p) = &mut self.settings_panel {
                    p.set_devices(inputs, outputs);
                }
            }
            Message::SystemTheme(dark) => {
                if dark != self.system_dark {
                    self.system_dark = dark;
                    self.apply_look();
                }
            }
            Message::ToggleLanguage => {
                let next = match i18n::lang() {
                    i18n::Lang::En => i18n::Lang::It,
                    i18n::Lang::It => i18n::Lang::En,
                };
                i18n::set_lang(next);
                // The demo never writes settings (it also runs in the tests).
                if !self.demo {
                    return write_in_background(config::language_writer(next), |e| {
                        tf!("Language not saved: {e}", "Lingua non salvata: {e}")
                    });
                }
            }
            _ => {}
        }
        Task::none()
    }
}

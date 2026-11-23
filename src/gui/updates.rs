//! In-app updates in the window: the state, the bar at the top and the line
//! in Settings › About. The checks and the rules are in `crate::update`.

use std::path::PathBuf;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, container, row, space, text};
use iced::{Element, Length};

use super::style::*;
use super::write_in_background;
use super::Message;
use crate::i18n::t;
use crate::update::Verified;
use iced::Task;

/// Where the updater is.
#[derive(Debug, Clone, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// Downloaded and checked: waiting for "Restart now".
    Ready {
        verified: Box<Verified>,
        path: PathBuf,
    },
    /// The installer is being started; Rico closes.
    Installing,
}

/// What the last check said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LastCheck {
    #[default]
    Never,
    UpToDate,
    CannotCheck,
}

#[derive(Debug, Default)]
pub struct UpdateUi {
    pub phase: Phase,
    pub last: LastCheck,
    /// The signed manifest says this version has a known security flaw.
    pub insecure: bool,
    /// Started with `--updated`: the version just installed.
    pub updated_to: Option<String>,
    /// The bar was closed with ×.
    pub dismissed: bool,
    /// The manifest being downloaded (kept until the download is ready).
    pub pending: Option<Box<Verified>>,
}

impl UpdateUi {
    /// The line in Settings › About.
    pub fn status_line(&self, enabled: bool) -> String {
        if !crate::update::configured() {
            return t(
                "In-app updates are not set up in this build yet.",
                "Gli aggiornamenti dall'app non sono ancora attivi in questa versione.",
            )
            .into();
        }
        if !enabled {
            return t("Not checked (turned off).", "Non controllati (disattivati).").into();
        }
        match (&self.phase, &self.last) {
            (Phase::Checking, _) => t("Checking…", "Controllo…").into(),
            (Phase::Downloading { version, .. }, _) => {
                tf!("Downloading Rico {version}…", "Scarico Rico {version}…")
            }
            (Phase::Ready { verified, .. }, _) => tf!(
                "Rico {} is ready: restart to update.",
                "Rico {} è pronto: riavvia per aggiornare.",
                verified.manifest.version
            ),
            (Phase::Installing, _) => t("Installing…", "Installazione…").into(),
            (_, LastCheck::UpToDate) => t("Up to date.", "Aggiornato.").into(),
            (_, LastCheck::CannotCheck) => t(
                "Cannot check for updates.",
                "Impossibile controllare gli aggiornamenti.",
            )
            .into(),
            (_, LastCheck::Never) => t("Not checked yet.", "Non ancora controllati.").into(),
        }
    }

    /// The bar at the top of the window, when there is something to say.
    pub fn banner(&self) -> Option<Element<'_, Message>> {
        if self.insecure && !matches!(self.phase, Phase::Installing) {
            let ready = matches!(self.phase, Phase::Ready { .. });
            let mut r = row![text(t(
                "This version of Rico has a known security flaw: update it.",
                "Questa versione di Rico ha una falla di sicurezza nota: aggiornala.",
            ))
            .size(13)
            .color(pal::red())]
            .spacing(10)
            .align_y(Vertical::Center);
            r = r.push(space::horizontal());
            if ready {
                r = r.push(restart_button());
            }
            return Some(bar(r.into(), pal::red()));
        }
        if self.dismissed {
            return None;
        }
        let content: Element<'_, Message> = match &self.phase {
            Phase::Downloading { version, done, total } => {
                let pct = if *total > 0 { done * 100 / total } else { 0 };
                row![
                    text(tf!(
                        "Downloading Rico {version}… {pct}%",
                        "Scarico Rico {version}… {pct}%"
                    ))
                    .size(13),
                    space::horizontal(),
                    close_button(),
                ]
                .align_y(Vertical::Center)
                .into()
            }
            Phase::Ready { verified, .. } => row![
                text(tf!(
                    "Rico {} is ready.",
                    "Rico {} è pronto.",
                    verified.manifest.version
                ))
                .size(13)
                .shaping(Shaping::Advanced),
                space::horizontal(),
                notes_button(),
                restart_button(),
                close_button(),
            ]
            .spacing(8)
            .align_y(Vertical::Center)
            .into(),
            Phase::Installing => text(t(
                "Starting the update: Rico closes and opens again by itself.",
                "Avvio dell'aggiornamento: Rico si chiude e si riapre da solo.",
            ))
            .size(13)
            .into(),
            _ => {
                let v = self.updated_to.as_ref()?;
                row![
                    text(tf!("Rico updated to {v}.", "Rico aggiornato alla {v}.")).size(13),
                    space::horizontal(),
                    notes_button(),
                    close_button(),
                ]
                .spacing(8)
                .align_y(Vertical::Center)
                .into()
            }
        };
        Some(bar(content, pal::accent()))
    }
}

fn restart_button<'a>() -> Element<'a, Message> {
    button(text(t("Restart now", "Riavvia ora")).size(13).font(SEMIBOLD))
        .on_press(Message::RestartToUpdate)
        .padding([4, 12])
        .style(primary)
        .into()
}

fn notes_button<'a>() -> Element<'a, Message> {
    button(text(t("What's new", "Novità")).size(13))
        .on_press(Message::OpenUpdateNotes)
        .padding([4, 10])
        .style(ghost)
        .into()
}

/// The release notes to open: the ready update's, or the installed
/// version's after an update. Only pages of Rico's releases on GitHub.
pub fn notes_url(ui: &UpdateUi) -> Option<String> {
    let url = match &ui.phase {
        Phase::Ready { verified, .. } => verified.notes_url()?.to_string(),
        _ => format!(
            "https://github.com/Trive253x/rico/releases/tag/v{}",
            ui.updated_to.as_ref()?
        ),
    };
    safe_url(&url).then_some(url)
}

/// A plain address of Rico's releases: nothing a program could read as an
/// option or a command.
fn safe_url(url: &str) -> bool {
    url.starts_with("https://github.com/Trive253x/rico/releases/")
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'-' | b'_'))
}

fn close_button<'a>() -> Element<'a, Message> {
    button(text("×").size(14))
        .on_press(Message::DismissUpdate)
        .padding([0, 8])
        .style(ghost)
        .into()
}

fn bar(content: Element<'_, Message>, colour: iced::Color) -> Element<'_, Message> {
    let edge = container(space().width(4).height(Length::Fill)).style(fill(colour));
    container(row![edge, container(content).padding([6, 12]).width(Length::Fill)].height(36))
        .width(Length::Fill)
        .style(fill(pal::panel()))
        .into()
}

impl super::Gui {
    /// A moderation action from the user card: the ones that remove
    /// someone or time them out wait for a confirmation first.
    /// Updates are checked: chosen in the settings, set up in this build
    /// (the owner's keys, Windows), never in the demo.
    pub(super) fn updates_on(&self) -> bool {
        self.settings.check_updates && crate::update::configured() && !self.demo
    }

    pub(super) fn check_for_update(&mut self) -> Task<Message> {
        if !self.updates_on() || !matches!(self.update.phase, Phase::Idle) {
            return Task::none();
        }
        self.update.phase = Phase::Checking;
        let candidates = self.settings.update_candidates;
        Task::perform(
            async move {
                let v = crate::update::fetch_best(candidates)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                let state = tokio::task::spawn_blocking(crate::update::load_state)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok((Box::new(v), state))
            },
            Message::UpdateChecked,
        )
    }

    pub(super) fn update_checked(
        &mut self,
        result: Result<(Box<crate::update::Verified>, crate::update::State), String>,
    ) -> Task<Message> {
        use crate::update::{self as up, Verdict};
        self.update.phase = Phase::Idle;
        let (v, mut state) = match result {
            Ok(x) => x,
            Err(e) => {
                tracing::info!("update: check failed: {e}");
                self.update.last = LastCheck::CannotCheck;
                return Task::none();
            }
        };
        let current = up::current_version();
        let now = chrono::Utc::now().timestamp();
        match up::evaluate(&v, &current, &state, now, self.settings.update_candidates) {
            Ok(verdict) => {
                state.remember(&v);
                let save = write_in_background(
                    move || up::save_state(&state),
                    |e| {
                        tf!(
                            "Update state not saved: {e}",
                            "Stato degli aggiornamenti non salvato: {e}"
                        )
                    },
                );
                self.update.insecure = up::is_insecure(&v, &current);
                match verdict {
                    Verdict::UpToDate => {
                        self.update.last = LastCheck::UpToDate;
                        save
                    }
                    Verdict::Newer => {
                        tracing::info!("update: {} available", v.manifest.version);
                        Task::batch([save, self.download_update(v)])
                    }
                }
            }
            Err(r) => {
                tracing::warn!("update: manifest refused: {r}");
                self.update.last = LastCheck::CannotCheck;
                Task::none()
            }
        }
    }

    /// The installer, in the background, with its progress.
    pub(super) fn download_update(&mut self, v: Box<crate::update::Verified>) -> Task<Message> {
        let Some(dir) = crate::update::download_dir() else {
            self.update.last = LastCheck::CannotCheck;
            return Task::none();
        };
        self.update.phase = Phase::Downloading {
            version: v.manifest.version.clone(),
            done: 0,
            total: v.manifest.size,
        };
        self.update.dismissed = false;
        self.update.pending = Some(v.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stream =
            futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|p| (p, rx)) });
        Task::batch([
            Task::perform(
                crate::update::download(*v, dir, move |p| {
                    let _ = tx.send(p);
                }),
                |()| Message::Noop,
            ),
            Task::run(stream, Message::UpdateProgress),
        ])
    }

    pub(super) fn update_progress(&mut self, p: crate::update::Progress) {
        use crate::update::Progress;
        match p {
            Progress::Bytes { done, total } => {
                if let Phase::Downloading {
                    done: d, total: t, ..
                } = &mut self.update.phase
                {
                    *d = done;
                    *t = total;
                }
            }
            Progress::Ready(path) => {
                if let Some(verified) = self.update.pending.take() {
                    self.update.phase = Phase::Ready { verified, path };
                }
            }
            Progress::Failed(e) => {
                tracing::warn!("update: download failed: {e}");
                self.update.pending = None;
                self.update.phase = Phase::Idle;
                self.update.last = LastCheck::CannotCheck;
            }
        }
    }

    /// "Restart now": calls and streams are left (microphone and screen
    /// capture released), the installer is checked again and started off the
    /// interface thread, then Rico closes.
    pub(super) fn restart_to_update(&mut self) -> Task<Message> {
        let Phase::Ready { verified, path } = std::mem::take(&mut self.update.phase) else {
            return Task::none();
        };
        self.update.phase = Phase::Installing;
        self.leave_voice();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || crate::update::start_installer(&path, &verified))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("{e:#}"))
            },
            Message::InstallerStarted,
        )
    }
}

impl super::Gui {
    /// The updates messages of `Gui::handle`.
    pub(super) fn handle_updates(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::UpdateCheck => return self.check_for_update(),
            Message::UpdateChecked(result) => return self.update_checked(result),
            Message::UpdateProgress(p) => self.update_progress(p),
            Message::RestartToUpdate => return self.restart_to_update(),
            Message::InstallerStarted(result) => match result {
                Ok(()) => return iced::exit(),
                Err(e) => {
                    tracing::warn!("update: installer not started: {e}");
                    self.update.phase = Phase::Idle;
                    self.update.last = LastCheck::CannotCheck;
                    self.status = Some((
                        tf!(
                            "The update could not start: {e}",
                            "L'aggiornamento non è partito: {e}"
                        ),
                        true,
                    ));
                }
            },
            Message::DismissUpdate => self.update.dismissed = true,
            Message::OpenUpdateNotes => {
                if let Some(url) = notes_url(&self.update) {
                    open_in_browser(&url);
                }
            }
            _ => {}
        }
        Task::none()
    }
}

/// Opens a checked web address (Rico's release notes) in the default
/// browser. No shell: the address is one argument.
pub(super) fn open_in_browser(url: &str) {
    use std::process::Command;
    let result = if cfg!(windows) {
        Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open the browser: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rico_release_pages_open() {
        assert!(safe_url("https://github.com/Trive253x/rico/releases/tag/v0.18.0"));
        assert!(safe_url(
            "https://github.com/Trive253x/rico/releases/tag/v0.18.0-rc.1"
        ));
        for bad in [
            "https://github.com/someone/else/releases/tag/v1",
            "https://github.com/Trive253x/rico/releases/tag/v1 --help",
            "https://github.com/Trive253x/rico/releases/tag/v1\"&calc",
            "http://github.com/Trive253x/rico/releases/tag/v1",
            "file:///C:/Windows",
        ] {
            assert!(!safe_url(bad), "{bad}");
        }
        let ui = UpdateUi {
            updated_to: Some("0.18.0".into()),
            ..Default::default()
        };
        assert_eq!(
            notes_url(&ui).as_deref(),
            Some("https://github.com/Trive253x/rico/releases/tag/v0.18.0")
        );
        assert_eq!(notes_url(&UpdateUi::default()), None);
    }
}

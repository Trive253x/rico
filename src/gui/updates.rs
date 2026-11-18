//! In-app updates in the window: the state, the bar at the top and the line
//! in Settings › About. The checks and the rules are in `crate::update`.

use std::path::PathBuf;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, container, row, space, text};
use iced::{Element, Length};

use super::style::*;
use super::Message;
use crate::i18n::t;
use crate::update::Verified;

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
                    close_button(),
                ]
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

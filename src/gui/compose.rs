//! Writing messages beyond plain text: replying, editing and deleting our
//! messages, files to upload, and the actions shown over the message under
//! the mouse.
//!
//! Only state and widgets: what is allowed is decided by the `Store`
//! (`can_reply`, `can_edit`, `can_delete`, `can_attach`, `can_react`) and
//! checked again in `Gui::update` before every request.

use std::path::PathBuf;
use std::sync::LazyLock;

use iced::alignment::Vertical;
use iced::widget::text::Shaping;
use iced::widget::{button, container, row, space, svg, text, tooltip, Row};
use iced::{Element, Length};
use tokio::io::AsyncReadExt;

use super::icons;
use super::style::{self, pal, BOLD, SEMIBOLD};
use super::Message;
use crate::discord::rest::{self, MAX_UPLOAD_BYTES, MAX_UPLOAD_FILES};
use crate::i18n::t;

/// The message being replied to.
#[derive(Debug, Clone)]
pub struct Reply {
    pub channel_id: String,
    pub message_id: String,
    /// Name of its author, as shown in the bar above the text field.
    pub author: String,
    /// The author gets a notification (`@` on, as in Discord's client).
    pub ping: bool,
}

/// One of our messages being edited in the text field.
#[derive(Debug, Clone)]
pub struct Editing {
    pub channel_id: String,
    pub message_id: String,
    /// What was in the text field before: it comes back after the edit.
    pub draft: String,
}

/// A file dropped in the window, read and waiting to be sent.
#[derive(Clone)]
pub struct PendingFile {
    /// Name only, never the folders it came from.
    pub name: String,
    pub bytes: bytes::Bytes,
}

impl std::fmt::Debug for PendingFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PendingFile({:?}, {} bytes)", self.name, self.bytes.len())
    }
}

/// Bytes of the files already waiting.
pub fn pending_bytes(files: &[PendingFile]) -> u64 {
    files.iter().map(|f| f.bytes.len() as u64).sum()
}

/// Reads a dropped file, off the interface thread. Only regular files, and
/// at most `room` bytes (what is left of the message's limit): bigger files
/// are refused before reading them, and a file that grows while being read
/// is refused too.
pub async fn read_file(path: PathBuf, room: u64) -> Result<PendingFile, String> {
    let name = path
        .file_name()
        .map_or_else(|| "file".to_string(), |n| rest::upload_name(&n.to_string_lossy()));
    let too_big = || {
        tf!(
            "{name} is too big: at most 10 MB per message.",
            "{name} è troppo grande: al massimo 10 MB per messaggio."
        )
    };
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| read_error(&name, &e))?;
    if !meta.is_file() {
        return Err(tf!(
            "{name} is not a file: only files can be sent.",
            "{name} non è un file: si possono inviare solo file."
        ));
    }
    if meta.len() > room {
        return Err(too_big());
    }
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| read_error(&name, &e))?;
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take(room + 1)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| read_error(&name, &e))?;
    if buf.len() as u64 > room {
        return Err(too_big());
    }
    if buf.is_empty() {
        return Err(tf!("{name} is empty.", "{name} è vuoto."));
    }
    Ok(PendingFile {
        name,
        bytes: buf.into(),
    })
}

fn read_error(name: &str, e: &std::io::Error) -> String {
    tf!("{name} could not be read: {e}", "Impossibile leggere {name}: {e}")
}

/// Bytes left for one more file, or why no file can be added.
pub fn room_for_one_more(files: &[PendingFile]) -> Result<u64, String> {
    if files.len() >= MAX_UPLOAD_FILES {
        return Err(t(
            "At most 10 files per message.",
            "Al massimo 10 file per messaggio.",
        )
        .into());
    }
    let room = MAX_UPLOAD_BYTES.saturating_sub(pending_bytes(files));
    if room == 0 {
        return Err(t("The files are bigger than 10 MB.", "I file superano i 10 MB.").into());
    }
    Ok(room)
}

/// Bar above the text field: "Replying to …", with the `@` switch.
pub fn reply_bar<'a>(r: &Reply) -> Element<'a, Message> {
    let ping = button(text(if r.ping { "@ ON" } else { "@ OFF" }).size(11).font(BOLD))
        .on_press(Message::TogglePingReply)
        .padding([2, 8])
        .style(style::tab(r.ping));
    let ping = tooltip(
        ping,
        text(t(
            "Notify the author of the message",
            "Avvisa l'autore del messaggio",
        ))
        .size(12),
        tooltip::Position::Top,
    )
    .style(style::tooltip);
    bar(
        row![
            text(t("Replying to", "Rispondi a")).size(12).color(pal::muted()),
            text(r.author.clone())
                .size(12)
                .font(SEMIBOLD)
                .shaping(Shaping::Advanced),
        ]
        .spacing(5)
        .into(),
        Some(ping.into()),
        Message::CancelReply,
    )
}

/// Bar above the text field while one of our messages is edited.
pub fn edit_bar<'a>() -> Element<'a, Message> {
    bar(
        text(t(
            "Editing the message — Enter to save, Esc to cancel",
            "Modifica del messaggio — Invio per salvare, Esc per annullare",
        ))
        .size(12)
        .color(pal::muted())
        .into(),
        None,
        Message::CancelEdit,
    )
}

/// The files waiting to be sent, each with a button to remove it.
pub fn files_row<'a>(files: &[PendingFile]) -> Element<'a, Message> {
    let chips = files.iter().enumerate().fold(Row::new().spacing(6), |r, (i, f)| {
        r.push(
            container(
                row![
                    text(f.name.clone()).size(12).shaping(Shaping::Advanced),
                    text(super::human_size(f.bytes.len() as u64))
                        .size(11)
                        .color(pal::muted()),
                    button(text("×").size(13))
                        .on_press(Message::RemoveFile(i))
                        .padding([0, 6])
                        .style(style::ghost),
                ]
                .spacing(6)
                .align_y(Vertical::Center),
            )
            .padding([2, 8])
            .style(style::outlined),
        )
    });
    bar(chips.wrap().into(), None, Message::ClearFiles)
}

fn bar<'a>(
    content: Element<'a, Message>,
    extra: Option<Element<'a, Message>>,
    close: Message,
) -> Element<'a, Message> {
    let mut r = row![content, space::horizontal()]
        .spacing(8)
        .align_y(Vertical::Center);
    if let Some(e) = extra {
        r = r.push(e);
    }
    r = r.push(
        button(text("×").size(14))
            .on_press(close)
            .padding([0, 8])
            .style(style::ghost),
    );
    container(r)
        .padding([4, 10])
        .width(Length::Fill)
        .style(style::fill(pal::panel()))
        .into()
}

/// What can be done on one message (already checked in the `Store`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Allowed {
    pub react: bool,
    pub reply: bool,
    pub edit: bool,
    pub delete: bool,
}

/// Small bar over the message under the mouse: react, reply, edit, delete,
/// as icons with their name in a tooltip. While a delete is waiting for
/// confirmation, only "Delete" and "Cancel".
pub fn actions<'a>(message_id: &str, allowed: Allowed, confirming: bool) -> Option<Element<'a, Message>> {
    let id = message_id.to_string();
    let r: Row<'a, Message> = if confirming {
        row![
            text(t("Delete this message?", "Eliminare questo messaggio?"))
                .size(12)
                .color(pal::text()),
            action(t("Delete", "Elimina"), Message::ConfirmDelete, true),
            action(t("Cancel", "Annulla"), Message::CancelDelete, false),
        ]
    } else {
        if !(allowed.react || allowed.reply || allowed.edit || allowed.delete) {
            return None;
        }
        let mut r = Row::new();
        if allowed.react {
            r = r.push(icon_action(
                &icons::REACT,
                t("Add a reaction", "Aggiungi una reazione"),
                Message::OpenReactionPicker(id.clone()),
                false,
            ));
        }
        if allowed.reply {
            r = r.push(icon_action(
                &icons::REPLY,
                t("Reply", "Rispondi"),
                Message::StartReply(id.clone()),
                false,
            ));
        }
        if allowed.edit {
            r = r.push(icon_action(
                &icons::EDIT,
                t("Edit", "Modifica"),
                Message::StartEdit(id.clone()),
                false,
            ));
        }
        if allowed.delete {
            r = r.push(icon_action(
                &icons::TRASH,
                t("Delete", "Elimina"),
                Message::AskDelete(id),
                true,
            ));
        }
        r
    };
    // A plain background without a border, so the icons stay readable over
    // the text of the message.
    Some(
        container(r.spacing(2).align_y(Vertical::Center))
            .padding(2)
            .style(style::rounded(pal::panel(), 6.0))
            .into(),
    )
}

/// One icon of the bar, with its name in a tooltip.
fn icon_action<'a>(
    handle: &LazyLock<svg::Handle>,
    label: &'a str,
    msg: Message,
    danger: bool,
) -> Element<'a, Message> {
    let color = if danger { pal::red() } else { pal::muted() };
    let b = button(icons::icon(handle, 16.0, color))
        .on_press(msg)
        .padding(5)
        .style(style::icon_button);
    tooltip(b, text(label).size(12), tooltip::Position::Top)
        .style(style::tooltip)
        .into()
}

fn action(label: &str, msg: Message, danger: bool) -> Element<'_, Message> {
    let label = text(label).size(12);
    let label = if danger { label.color(pal::red()) } else { label };
    button(label)
        .on_press(msg)
        .padding([3, 8])
        .style(style::icon_button)
        .into()
}

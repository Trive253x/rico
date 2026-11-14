//! Server folders: how they look in the server column, and the window that
//! edits them. The folders are the account's (Discord keeps them in the
//! settings): changes are made on a copy and sent to Discord only with
//! "Save".

use std::collections::{HashMap, HashSet};

use iced::alignment::{Horizontal, Vertical};
use iced::widget::image as img;
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, opaque, pick_list, row, scrollable, text,
    text_input, tooltip, Column, Space,
};
use iced::{Color, Element, Length};

use super::icons::{self, icon};
use super::style::*;
use super::{Message, Preview};
use crate::discord::folders::GuildFolders;
use crate::i18n::t;
use crate::media;
use crate::store::{RailEntry, Store};
use crate::text::initials;

/// Colours offered for a folder (0xRRGGBB).
pub const COLORS: [u64; 10] = [
    0x5865f2, 0x3498db, 0x1abc9c, 0x2ecc71, 0xf1c40f, 0xe67e22, 0xe74c3c, 0xe91e63, 0x9b59b6, 0x95a5a6,
];

use super::ICON_PX;

/// Size of a server icon in the column.
const ICON: f32 = 40.0;

pub fn rgb(color: Option<u64>) -> Color {
    let c = color.unwrap_or(0x5865f2);
    Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Where a server can be moved to, in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    NoFolder,
    Folder(i64, String),
    NewFolder,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFolder => f.write_str(t("No folder", "Nessuna cartella")),
            Self::Folder(_, name) => f.write_str(name),
            Self::NewFolder => f.write_str(t("New folder…", "Nuova cartella…")),
        }
    }
}

/// A change in the editor.
#[derive(Debug, Clone)]
pub enum Edit {
    Rename(i64, String),
    Color(i64, u64),
    Ungroup(i64),
    /// Entry (folder or single server) up or down.
    MoveEntry(usize, i32),
    /// Server up or down inside its folder.
    MoveInFolder(String, i32),
    Move(String, Target),
}

/// The editor: a copy of the folders being changed.
pub struct Editor {
    pub draft: GuildFolders,
    /// The folders when the editor opened (to know if anything changed).
    pub original: GuildFolders,
    pub saving: bool,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(store: &Store) -> Self {
        let mut folders = store.folders().cloned().unwrap_or_default();
        folders.normalize(&store.guild_ids());
        Self {
            draft: folders.clone(),
            original: folders,
            saving: false,
            error: None,
        }
    }

    pub fn changed(&self) -> bool {
        self.draft != self.original
    }

    pub fn apply(&mut self, edit: Edit) {
        let d = &mut self.draft;
        match edit {
            Edit::Rename(id, name) => d.rename(id, &name),
            Edit::Color(id, color) => d.set_color(id, Some(color)),
            Edit::Ungroup(id) => d.ungroup(id),
            Edit::MoveEntry(i, by) => d.move_entry(i, by),
            Edit::MoveInFolder(g, by) => d.move_in_folder(&g, by),
            Edit::Move(g, Target::NoFolder) => d.remove_from_folder(&g),
            Edit::Move(g, Target::Folder(id, _)) => d.move_to_folder(&g, id),
            Edit::Move(g, Target::NewFolder) => {
                let n = d.folders.iter().filter(|f| f.is_folder()).count() + 1;
                let name = format!("{} {n}", t("Folder", "Cartella"));
                let color = COLORS[n % COLORS.len()];
                d.new_folder(&g, Some(name), Some(color));
            }
        }
        self.error = None;
    }
}

fn folder_name(name: Option<&str>) -> String {
    match name {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => t("Folder", "Cartella").to_string(),
    }
}

fn guild_icon<'a>(
    store: &Store,
    images: &'a HashMap<String, Preview>,
    index: usize,
    size: f32,
) -> Element<'a, Message> {
    let Some(g) = store.guilds.get(index) else {
        return Space::new(size, size).into();
    };
    let image = media::guild_icon_url(&g.id, g.icon(), ICON_PX).and_then(|u| images.get(&u));
    match image {
        Some(Preview::Ready(h)) => img::Image::new(h.clone()).width(size).height(size).into(),
        _ => container(
            text(initials(g.name()))
                .size((size * 0.36).max(8.0))
                .font(SEMIBOLD)
                .shaping(Shaping::Advanced),
        )
        .width(size)
        .height(size)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style(rounded(pal::title(), size * 0.3))
        .into(),
    }
}

/// A folder in the server column. Closed: its servers' icons in a small
/// grid on the folder's colour; open: the folder icon, then its servers on a
/// band of the same colour. A right click opens the editor.
#[allow(clippy::too_many_arguments)]
pub fn rail_folder<'a>(
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    entry: &RailEntry,
    open: bool,
    selected: Option<usize>,
    guild_button: impl Fn(usize) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let RailEntry::Folder {
        id,
        name,
        color,
        guilds,
    } = entry
    else {
        return Space::new(0, 0).into();
    };
    let tint = rgb(*color);
    let has_selected = selected.is_some_and(|s| guilds.contains(&s));
    let unread = guilds
        .iter()
        .filter_map(|i| store.guilds.get(*i))
        .any(|g| store.guild_has_unread(&g.id));
    let face: Element<'a, Message> = if open {
        container(icon(&icons::FOLDER, 22.0, tint))
            .width(ICON)
            .height(ICON)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    } else {
        let mini = |k: usize| -> Element<'a, Message> {
            match guilds.get(k) {
                Some(&i) => guild_icon(store, images, i, 15.0),
                None => Space::new(15, 15).into(),
            }
        };
        container(
            column![
                row![mini(0), mini(1)].spacing(3),
                row![mini(2), mini(3)].spacing(3)
            ]
            .spacing(3),
        )
        .width(ICON)
        .height(ICON)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .into()
    };
    let head = button(face)
        .width(ICON)
        .height(ICON)
        .padding(0)
        .style(folder_face(tint, open))
        .on_press(Message::ToggleFolder(*id));
    let head = tooltip(
        mouse_area(head).on_right_press(Message::OpenFolderEditor),
        text(folder_name(name.as_deref()))
            .size(12)
            .shaping(Shaping::Advanced),
        tooltip::Position::Right,
    )
    .padding(6)
    .style(super::style::tooltip);
    // Same marker as the servers: a dot for unread, tall when one of its
    // servers is open and the folder is closed.
    let marker_height = if has_selected && !open {
        22.0
    } else if unread && !open {
        8.0
    } else {
        0.0
    };
    let marker = container(Space::new(3, marker_height)).style(rounded(pal::text(), 2.0));
    let head_row = row![marker, Space::with_width(7), head, Space::with_width(10)].align_y(Vertical::Center);
    if !open {
        return head_row.into();
    }
    let mut inner = Column::new().spacing(8).align_x(Horizontal::Center);
    inner = inner.push(head_row);
    for &i in guilds {
        inner = inner.push(guild_button(i));
    }
    container(inner)
        .padding([4, 0])
        .style(rounded(Color { a: 0.18, ..tint }, 12.0))
        .into()
}

/// The editor window.
pub fn view<'a>(
    e: &'a Editor,
    store: &'a Store,
    images: &'a HashMap<String, Preview>,
    demo: bool,
) -> Element<'a, Message> {
    let index: HashMap<&str, usize> = store
        .guilds
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();
    let targets: Vec<Target> = std::iter::once(Target::NoFolder)
        .chain(
            e.draft
                .folders
                .iter()
                .filter_map(|f| f.id.map(|id| Target::Folder(id, folder_name(f.name.as_deref())))),
        )
        .chain(std::iter::once(Target::NewFolder))
        .collect();

    let arrows = |up: Option<Message>, down: Option<Message>| {
        row![
            button(text("↑").size(12))
                .padding([2, 8])
                .style(ghost)
                .on_press_maybe(up),
            button(text("↓").size(12))
                .padding([2, 8])
                .style(ghost)
                .on_press_maybe(down),
        ]
        .spacing(4)
    };
    let guild_row = |gid: &'a str,
                     folder: Option<i64>,
                     up: Option<Message>,
                     down: Option<Message>|
     -> Element<'a, Message> {
        let Some(&i) = index.get(gid) else {
            return Space::new(0, 0).into();
        };
        let name = store.guilds[i].name().to_string();
        let current = match folder {
            Some(id) => targets
                .iter()
                .find(|t| matches!(t, Target::Folder(f, _) if *f == id))
                .cloned(),
            None => Some(Target::NoFolder),
        };
        let gid_owned = gid.to_string();
        row![
            guild_icon(store, images, i, 28.0),
            text(name).size(13).shaping(Shaping::Advanced).width(Length::Fill),
            arrows(up, down),
            pick_list(targets.clone(), current, move |t| {
                Message::FolderEdit(Edit::Move(gid_owned.clone(), t))
            })
            .text_size(12)
            .width(170),
        ]
        .spacing(10)
        .align_y(Vertical::Center)
        .into()
    };

    let n = e.draft.folders.len();
    let mut list = Column::new().spacing(8);
    let mut seen = HashSet::new();
    for (k, f) in e.draft.folders.iter().enumerate() {
        let up = (k > 0).then_some(Message::FolderEdit(Edit::MoveEntry(k, -1)));
        let down = (k + 1 < n).then_some(Message::FolderEdit(Edit::MoveEntry(k, 1)));
        match f.id {
            None => {
                for gid in &f.guild_ids {
                    if index.contains_key(gid.as_str()) && seen.insert(gid.clone()) {
                        list = list.push(
                            container(guild_row(gid, None, up.clone(), down.clone()))
                                .padding([6, 10])
                                .style(rounded(pal::panel(), 8.0)),
                        );
                    }
                }
            }
            Some(id) => {
                let tint = rgb(f.color);
                let name_value = f.name.clone().unwrap_or_default();
                let mut swatches = row![].spacing(4);
                for c in COLORS {
                    let chosen = f.color == Some(c);
                    swatches = swatches.push(
                        button(Space::new(16, 16))
                            .padding(0)
                            .style(swatch(rgb(Some(c)), chosen))
                            .on_press(Message::FolderEdit(Edit::Color(id, c))),
                    );
                }
                let header = row![
                    icon(&icons::FOLDER, 22.0, tint),
                    text_input(t("Folder name", "Nome della cartella"), &name_value)
                        .on_input(move |s| Message::FolderEdit(Edit::Rename(id, s)))
                        .size(13)
                        .padding([4, 8])
                        .style(input)
                        .width(Length::Fill),
                    arrows(up, down),
                    button(text(t("Ungroup", "Sciogli")).size(12))
                        .padding([3, 10])
                        .style(ghost)
                        .on_press(Message::FolderEdit(Edit::Ungroup(id))),
                ]
                .spacing(10)
                .align_y(Vertical::Center);
                let mut inside = Column::new().spacing(6);
                let m = f.guild_ids.len();
                for (j, gid) in f.guild_ids.iter().enumerate() {
                    if !index.contains_key(gid.as_str()) || !seen.insert(gid.clone()) {
                        continue;
                    }
                    let up = (j > 0).then(|| Message::FolderEdit(Edit::MoveInFolder(gid.clone(), -1)));
                    let down = (j + 1 < m).then(|| Message::FolderEdit(Edit::MoveInFolder(gid.clone(), 1)));
                    inside = inside.push(guild_row(gid, Some(id), up, down));
                }
                list = list.push(
                    container(
                        column![
                            header,
                            swatches,
                            container(inside).padding(iced::Padding {
                                left: 28.0,
                                ..iced::Padding::ZERO
                            })
                        ]
                        .spacing(8),
                    )
                    .padding(10)
                    .style(folder_box(tint)),
                );
            }
        }
    }

    let status: Element<'a, Message> = match (&e.error, e.saving) {
        (Some(err), _) => text(err.clone())
            .size(12)
            .color(pal::red())
            .shaping(Shaping::Advanced)
            .into(),
        (None, true) => text(t("Saving on Discord…", "Salvataggio su Discord…"))
            .size(12)
            .color(pal::muted())
            .into(),
        (None, false) if demo => text(t(
            "Demo: changes stay here, nothing is sent.",
            "Demo: le modifiche restano qui, non si invia niente.",
        ))
        .size(12)
        .color(pal::muted())
        .into(),
        (None, false) => text(t(
            "The folders are saved on your Discord account: they change on every device.",
            "Le cartelle sono salvate sul tuo account Discord: cambiano su tutti i dispositivi.",
        ))
        .size(12)
        .color(pal::muted())
        .into(),
    };
    let save = button(text(t("Save", "Salva")).size(13).font(SEMIBOLD))
        .padding([6, 16])
        .style(primary)
        .on_press_maybe((e.changed() && !e.saving).then_some(Message::SaveFolders));
    let cancel = button(text(t("Cancel (Esc)", "Annulla (Esc)")).size(13))
        .padding([6, 14])
        .style(ghost)
        .on_press(Message::CloseFolderEditor);

    let card = container(
        column![
            text(t("Server folders", "Cartelle dei server"))
                .size(18)
                .font(BOLD),
            text(t(
                "Choose a folder for each server with the menu on the right, or \"New folder…\". \
                 A folder left empty disappears.",
                "Scegli la cartella di ogni server con il menu a destra, oppure \"Nuova cartella…\". \
                 Una cartella che resta vuota sparisce.",
            ))
            .size(12)
            .color(pal::muted())
            .shaping(Shaping::Advanced),
            scrollable(container(list).padding(iced::Padding {
                right: 12.0,
                ..iced::Padding::ZERO
            }))
            .height(Length::Fill)
            .style(thin_scrollbar),
            row![status, horizontal_space(), cancel, save]
                .spacing(8)
                .align_y(Vertical::Center),
        ]
        .spacing(12),
    )
    .padding(20)
    .width(640)
    .height(Length::FillPortion(9))
    .style(outlined);
    let card = mouse_area(card).on_press(Message::Noop);
    opaque(
        mouse_area(
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(30)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .style(backdrop),
        )
        .on_press(Message::CloseFolderEditor),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::folders::MAX_NAME;

    #[test]
    fn editor_moves_and_new_folders() {
        let store = Store::demo();
        let mut e = Editor::new(&store);
        assert!(!e.changed());
        // Rust Community into a new folder, then back out.
        e.apply(Edit::Move("100".into(), Target::NewFolder));
        assert!(e.changed());
        let new = e.draft.find("100").map(|(i, _)| e.draft.folders[i].id).unwrap();
        assert!(new.is_some());
        e.apply(Edit::Move("100".into(), Target::NoFolder));
        assert!(
            e.draft.folders.iter().filter(|f| f.is_folder()).count() == 1,
            "the empty folder is gone"
        );
        e.apply(Edit::Rename(
            e.draft.folders.iter().find_map(|f| f.id).unwrap(),
            "x".repeat(200),
        ));
        assert!(e
            .draft
            .folders
            .iter()
            .filter_map(|f| f.name.as_ref())
            .all(|n| n.chars().count() <= MAX_NAME));
    }
}

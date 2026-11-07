//! Emoji and GIF picker, opened by the buttons next to the message field.
//!
//! The emojis are the Unicode ones (data from the `emojis` crate) plus those
//! of the open channel's server (they can only be used there); GIFs come from
//! Discord's search (`/gifs/search`, provided by Klipy today) and are sent as
//! links, exactly as the official client does.

use std::collections::HashMap;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, row, scrollable, text, text_input, Column, Row,
};
use iced::{mouse, Element, Length};

use super::style::*;
use super::{custom_emoji, media_box, Message, Preview};
use crate::discord::models::{GifResult, GuildEmoji};
use crate::i18n::t;
use crate::text::initials;

pub const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 450.0;
const PADDING: f32 = 12.0;
const EMOJI_COLUMNS: usize = 9;
const EMOJI_CELL: f32 = 42.0;
/// Most emojis shown for a search.
const MAX_FOUND: usize = 135;
pub const MAX_RECENT: usize = 18;
const GIF_GAP: f32 = 8.0;
/// The two columns leave room on the right for the scroll bar.
const GIF_COLUMN: f32 = (WIDTH - 2.0 * PADDING - GIF_GAP - 14.0) / 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Gif,
    Emoji,
}

/// Emoji group shown when nothing is searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    /// Emojis of the open channel's server.
    Server,
    Group(emojis::Group),
}

/// Emojis of the open channel's server, to show in the picker.
pub struct ServerEmojis<'a> {
    pub name: &'a str,
    pub emojis: Vec<&'a GuildEmoji>,
    /// Images already downloaded, by address.
    pub images: &'a HashMap<String, Preview>,
}

/// Groups in Discord's order, with the bar icon and the title (en, it).
const GROUPS: [(emojis::Group, &str, &str, &str); 9] = [
    (
        emojis::Group::SmileysAndEmotion,
        "😀",
        "Smileys & emotion",
        "Faccine ed emozioni",
    ),
    (emojis::Group::PeopleAndBody, "👋", "People", "Persone"),
    (
        emojis::Group::AnimalsAndNature,
        "🐶",
        "Animals & nature",
        "Animali e natura",
    ),
    (
        emojis::Group::FoodAndDrink,
        "🍕",
        "Food & drink",
        "Cibo e bevande",
    ),
    (
        emojis::Group::TravelAndPlaces,
        "🚗",
        "Travel & places",
        "Viaggi e luoghi",
    ),
    (emojis::Group::Activities, "⚽", "Activities", "Attività"),
    (emojis::Group::Objects, "💡", "Objects", "Oggetti"),
    (emojis::Group::Symbols, "❤️", "Symbols", "Simboli"),
    (emojis::Group::Flags, "🏁", "Flags", "Bandiere"),
];

#[derive(Debug, Clone)]
pub enum Gifs {
    /// GIF tab never opened: nothing has been requested yet.
    NotLoaded,
    Loading,
    Ready(Vec<GifResult>),
    Failed(String),
}

pub struct Picker {
    pub tab: Tab,
    pub emoji_query: String,
    pub section: Section,
    pub gif_query: String,
    pub gifs: Gifs,
    /// Number of the latest search: answers to earlier ones are dropped.
    pub seq: u64,
    pub searching: bool,
}

impl Picker {
    pub fn new(tab: Tab, has_recent: bool, has_server: bool) -> Self {
        Self {
            tab,
            emoji_query: String::new(),
            section: if has_recent {
                Section::Recent
            } else if has_server {
                Section::Server
            } else {
                Section::Group(GROUPS[0].0)
            },
            gif_query: String::new(),
            gifs: Gifs::NotLoaded,
            seq: 0,
            searching: false,
        }
    }

    /// GIFs shown now (to download their previews).
    pub fn shown_gifs(&self) -> &[GifResult] {
        match (&self.gifs, self.tab) {
            (Gifs::Ready(list), Tab::Gif) => list,
            _ => &[],
        }
    }
}

pub fn search_id() -> text_input::Id {
    text_input::Id::new("picker-search")
}

/// Emojis the Windows 10/11 fonts can draw (the newest ones would show up
/// as little boxes).
fn supported(e: &emojis::Emoji) -> bool {
    e.unicode_version() <= emojis::UnicodeVersion::new(14, 0)
}

/// Emojis to show: search results (English name or shortcode), otherwise
/// those of the chosen section.
pub fn emoji_list(query: &str, section: Section, recent: &[&'static str]) -> Vec<&'static str> {
    let query = query.trim().to_lowercase().replace(':', "");
    if !query.is_empty() {
        return emojis::iter()
            .filter(|e| supported(e))
            .filter(|e| matches_query(e, &query))
            .take(MAX_FOUND)
            .map(|e| e.as_str())
            .collect();
    }
    match section {
        Section::Recent => recent.to_vec(),
        Section::Server => Vec::new(),
        Section::Group(group) => emojis::iter()
            .filter(|e| e.group() == group && supported(e))
            .map(|e| e.as_str())
            .collect(),
    }
}

/// Server emojis to show: those whose name contains the search, otherwise
/// all of them if their section is open.
pub fn server_list<'a>(query: &str, section: Section, emojis: &[&'a GuildEmoji]) -> Vec<&'a GuildEmoji> {
    let query = query.trim().to_lowercase().replace(':', "");
    if !query.is_empty() {
        return emojis
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&query))
            .copied()
            .collect();
    }
    if section == Section::Server {
        emojis.to_vec()
    } else {
        Vec::new()
    }
}

/// A word of the name or shortcode starts with the search ("cat" finds 🐱
/// but not "multiplication"); with several words all must be there.
fn matches_query(e: &emojis::Emoji, query: &str) -> bool {
    query.split_whitespace().all(|q| {
        e.name().split([' ', '-', ':']).any(|w| w.starts_with(q))
            || e.shortcodes().any(|s| s.split('_').any(|w| w.starts_with(q)))
    })
}

/// Size of a GIF in the two-column grid.
pub fn gif_size(g: &GifResult) -> (f32, f32) {
    let ratio = if g.width > 0 && g.height > 0 {
        g.height as f32 / g.width as f32
    } else {
        1.0
    };
    (GIF_COLUMN, (GIF_COLUMN * ratio).clamp(60.0, 320.0))
}

pub fn view<'a>(
    p: &'a Picker,
    previews: &'a HashMap<String, Preview>,
    recent: &'a [&'static str],
    server: Option<ServerEmojis<'a>>,
) -> Element<'a, Message> {
    let tab = |label: &'static str, which: Tab| {
        button(text(label).size(13).font(SEMIBOLD))
            .on_press(Message::PickerTab(which))
            .padding([5, 12])
            .style(super::style::tab(p.tab == which))
    };
    let header = row![
        tab("GIF", Tab::Gif),
        tab("Emoji", Tab::Emoji),
        horizontal_space(),
        button(text("Esc").size(12).color(MUTED))
            .on_press(Message::ClosePicker)
            .padding([4, 8])
            .style(ghost),
    ]
    .spacing(6)
    .align_y(Vertical::Center);

    let body = match p.tab {
        Tab::Emoji => view_emoji(p, recent, server),
        Tab::Gif => view_gif(p, previews),
    };
    container(column![header, body].spacing(10))
        .padding(PADDING)
        .width(WIDTH)
        .height(HEIGHT)
        .style(card)
        .into()
}

fn view_emoji<'a>(
    p: &'a Picker,
    recent: &'a [&'static str],
    server: Option<ServerEmojis<'a>>,
) -> Element<'a, Message> {
    let search = text_input(
        t("Search emojis (English names)", "Cerca emoji (nome in inglese)"),
        &p.emoji_query,
    )
    .id(search_id())
    .on_input(Message::EmojiQuery)
    .padding(8)
    .style(input);

    let section_button = |icon: String, size: u16, section: Section| {
        let active = p.emoji_query.trim().is_empty() && p.section == section;
        // Bold only for the server initials: with emojis it would pick a
        // black and white font.
        let font = if section == Section::Server {
            BOLD
        } else {
            iced::Font::DEFAULT
        };
        button(
            text(icon)
                .size(size)
                .font(font)
                .shaping(Shaping::Advanced)
                .align_x(Horizontal::Center),
        )
        .on_press(Message::EmojiSection(section))
        .width(34)
        .padding([4, 0])
        .style(super::style::tab(active))
    };
    let mut sections = Row::new().spacing(2);
    if !recent.is_empty() {
        sections = sections.push(section_button("🕘".into(), 17, Section::Recent));
    }
    if let Some(server) = server.as_ref().filter(|s| !s.emojis.is_empty()) {
        // Server initials, as in the server bar.
        sections = sections.push(section_button(initials(server.name), 12, Section::Server));
    }
    for (group, icon, _, _) in GROUPS {
        sections = sections.push(section_button(icon.into(), 17, Section::Group(group)));
    }

    let title = if !p.emoji_query.trim().is_empty() {
        t("Results", "Risultati").to_string()
    } else {
        match p.section {
            Section::Recent => t("Recently used", "Usate di recente").to_string(),
            Section::Server => match &server {
                Some(s) => tf!("{} emojis", "Emoji di {}", s.name),
                None => String::new(),
            },
            Section::Group(g) => GROUPS
                .iter()
                .find(|(group, _, _, _)| *group == g)
                .map(|&(_, _, en, it)| t(en, it).to_string())
                .unwrap_or_default(),
        }
    };

    // Server emojis first, then the Unicode ones.
    let mut cells: Vec<Element<'a, Message>> = Vec::new();
    if let Some(server) = &server {
        for e in server_list(&p.emoji_query, p.section, &server.emojis) {
            let picture = custom_emoji(server.images, &e.id, &e.name, e.animated, 30.0);
            cells.push(
                button(container(picture).center_x(Length::Fill).center_y(Length::Fill))
                    .on_press(Message::PickServerEmoji(e.name.clone()))
                    .width(EMOJI_CELL)
                    .height(EMOJI_CELL)
                    .padding(2)
                    .style(list_item(false))
                    .into(),
            );
        }
    }
    for e in emoji_list(&p.emoji_query, p.section, recent) {
        cells.push(
            button(
                text(e)
                    .size(24)
                    .shaping(Shaping::Advanced)
                    .width(Length::Fill)
                    .align_x(Horizontal::Center),
            )
            .on_press(Message::PickEmoji(e))
            .width(EMOJI_CELL)
            .height(EMOJI_CELL)
            .padding(2)
            .style(list_item(false))
            .into(),
        );
    }
    let grid: Element<'a, Message> = if cells.is_empty() {
        container(text(t("No emojis found", "Nessuna emoji trovata")).color(MUTED))
            .width(Length::Fill)
            .padding(20)
            .align_x(Horizontal::Center)
            .into()
    } else {
        let mut rows = Column::new().spacing(2);
        let mut cells = cells.into_iter().peekable();
        while cells.peek().is_some() {
            rows = rows.push(Row::with_children(cells.by_ref().take(EMOJI_COLUMNS)).spacing(2));
        }
        rows.into()
    };

    column![
        search,
        sections,
        text(title).size(12).font(SEMIBOLD).color(MUTED),
        scrollable(grid)
            .direction(super::thin())
            .style(thin_scrollbar)
            .height(Length::Fill),
    ]
    .spacing(8)
    .into()
}

fn view_gif<'a>(p: &'a Picker, previews: &'a HashMap<String, Preview>) -> Element<'a, Message> {
    let search = text_input(t("Search GIFs", "Cerca GIF"), &p.gif_query)
        .id(search_id())
        .on_input(Message::GifQuery)
        .padding(8)
        .style(input);

    let query = p.gif_query.trim();
    let mut title = if query.is_empty() {
        t("Trending GIFs", "GIF di tendenza").to_string()
    } else {
        tf!("Results for “{query}”", "Risultati per «{query}»")
    };
    if p.searching {
        title.push_str(t(" · searching…", " · ricerca…"));
    }

    let note = |s: String| -> Element<'a, Message> {
        container(text(s).color(MUTED).shaping(Shaping::Advanced))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    };
    let body = match &p.gifs {
        Gifs::NotLoaded | Gifs::Loading => note(t("Loading GIFs…", "Caricamento delle GIF…").into()),
        Gifs::Failed(e) => note(tf!("GIFs unavailable: {e}", "GIF non disponibili: {e}")),
        Gifs::Ready(list) if list.is_empty() => note(t("No GIFs found", "Nessuna GIF trovata").into()),
        Gifs::Ready(list) => {
            // Two columns filled as on Discord: each GIF goes into the shorter one.
            let mut cols: [Vec<Element<'a, Message>>; 2] = [Vec::new(), Vec::new()];
            let mut heights = [0.0f32; 2];
            for g in list {
                let (w, h) = gif_size(g);
                let i = usize::from(heights[1] < heights[0]);
                heights[i] += h + GIF_GAP;
                let item = mouse_area(media_box(previews.get(&g.src), w, h, None))
                    .on_press(Message::PickGif(g.url.clone()))
                    .interaction(mouse::Interaction::Pointer);
                cols[i].push(item.into());
            }
            let [left, right] = cols.map(|c| Column::with_children(c).spacing(GIF_GAP));
            scrollable(row![left, right].spacing(GIF_GAP))
                .direction(super::thin())
                .style(thin_scrollbar)
                .height(Length::Fill)
                .into()
        }
    };

    column![
        search,
        text(title)
            .size(12)
            .font(SEMIBOLD)
            .color(MUTED)
            .shaping(Shaping::Advanced),
        body,
        text(t(
            "Click a GIF to send it · GIFs provided by Klipy",
            "Clic su una GIF per inviarla · GIF fornite da Klipy",
        ))
        .size(11)
        .color(MUTED)
        .shaping(Shaping::Advanced),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_search() {
        let found = emoji_list("cat", Section::Recent, &[]);
        assert!(found.contains(&"🐱"), "{found:?}");
        assert!(found.len() <= MAX_FOUND);
        // Also with Discord-style shortcodes.
        assert!(emoji_list(":joy:", Section::Recent, &[]).contains(&"😂"));
        assert!(emoji_list("no-emoji-has-this-name", Section::Recent, &[]).is_empty());
        assert!(!emoji_list("cat", Section::Recent, &[]).contains(&"✖️"));
        assert!(emoji_list("cat face", Section::Recent, &[]).contains(&"🐱"));
    }

    #[test]
    fn emoji_sections() {
        let smileys = emoji_list("", Section::Group(emojis::Group::SmileysAndEmotion), &[]);
        assert!(smileys.contains(&"😀"));
        assert!(!smileys.contains(&"🐱"));
        assert_eq!(emoji_list("", Section::Recent, &["🦀"]), vec!["🦀"]);
    }

    #[test]
    fn server_emojis() {
        let e = |name: &str| GuildEmoji {
            id: "1".into(),
            name: name.into(),
            animated: false,
            available: true,
            roles: vec![],
        };
        let (a, b) = (e("ferris"), e("cargo"));
        let all = [&a, &b];
        assert_eq!(server_list("", Section::Server, &all).len(), 2);
        assert!(server_list("", Section::Recent, &all).is_empty());
        let found = server_list(":FER", Section::Recent, &all);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "ferris");
    }

    #[test]
    fn gif_size_in_the_grid() {
        let g = GifResult {
            url: "u".into(),
            src: "s".into(),
            title: String::new(),
            width: 220,
            height: 110,
        };
        let (w, h) = gif_size(&g);
        assert_eq!(w, GIF_COLUMN);
        assert!((h - GIF_COLUMN / 2.0).abs() < 0.5);
    }
}

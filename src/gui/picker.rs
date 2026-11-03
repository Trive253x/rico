//! Selettore di emoji e GIF, aperto dai pulsanti accanto al campo di scrittura.
//!
//! Le emoji sono quelle Unicode (dati del crate `emojis`); le GIF arrivano
//! dalla ricerca di Discord (`/gifs/search`, oggi fornita da Klipy) e si
//! inviano come link, esattamente come fa il client ufficiale.

use std::collections::HashMap;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Shaping;
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, row, scrollable, text, text_input, Column, Row,
};
use iced::{mouse, Element, Length};

use super::style::*;
use super::{media_box, Message, Preview};
use crate::discord::models::GifResult;

pub const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 450.0;
const PADDING: f32 = 12.0;
const EMOJI_COLUMNS: usize = 9;
const EMOJI_CELL: f32 = 42.0;
/// Emoji mostrate al massimo per una ricerca.
const MAX_FOUND: usize = 135;
pub const MAX_RECENT: usize = 18;
const GIF_GAP: f32 = 8.0;
const GIF_COLUMN: f32 = (WIDTH - 2.0 * PADDING - GIF_GAP - 6.0) / 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Gif,
    Emoji,
}

/// Gruppo di emoji mostrato quando non si cerca nulla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    Group(emojis::Group),
}

/// Gruppi nell'ordine di Discord, con l'icona della barra e il titolo.
const GROUPS: [(emojis::Group, &str, &str); 9] = [
    (emojis::Group::SmileysAndEmotion, "😀", "Faccine ed emozioni"),
    (emojis::Group::PeopleAndBody, "👋", "Persone"),
    (emojis::Group::AnimalsAndNature, "🐶", "Animali e natura"),
    (emojis::Group::FoodAndDrink, "🍕", "Cibo e bevande"),
    (emojis::Group::TravelAndPlaces, "🚗", "Viaggi e luoghi"),
    (emojis::Group::Activities, "⚽", "Attività"),
    (emojis::Group::Objects, "💡", "Oggetti"),
    (emojis::Group::Symbols, "❤️", "Simboli"),
    (emojis::Group::Flags, "🏁", "Bandiere"),
];

#[derive(Debug, Clone)]
pub enum Gifs {
    /// Scheda GIF mai aperta: non si è ancora chiesto nulla.
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
    /// Numero dell'ultima ricerca: le risposte di quelle precedenti si scartano.
    pub seq: u64,
    pub searching: bool,
}

impl Picker {
    pub fn new(tab: Tab, has_recent: bool) -> Self {
        Self {
            tab,
            emoji_query: String::new(),
            section: if has_recent {
                Section::Recent
            } else {
                Section::Group(GROUPS[0].0)
            },
            gif_query: String::new(),
            gifs: Gifs::NotLoaded,
            seq: 0,
            searching: false,
        }
    }

    /// GIF mostrate ora (per scaricarne le anteprime).
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

/// Emoji che i font di Windows 10/11 sanno disegnare (le più nuove
/// comparirebbero come quadratini).
fn supported(e: &emojis::Emoji) -> bool {
    e.unicode_version() <= emojis::UnicodeVersion::new(14, 0)
}

/// Emoji da mostrare: risultati della ricerca (nome inglese o shortcode),
/// altrimenti quelle della sezione scelta.
pub fn emoji_list(query: &str, section: Section, recent: &[&'static str]) -> Vec<&'static str> {
    let query = query.trim().to_lowercase().replace(':', "");
    if !query.is_empty() {
        return emojis::iter()
            .filter(|e| supported(e))
            .filter(|e| e.name().contains(&query) || e.shortcodes().any(|s| s.contains(&query)))
            .take(MAX_FOUND)
            .map(|e| e.as_str())
            .collect();
    }
    match section {
        Section::Recent => recent.to_vec(),
        Section::Group(group) => emojis::iter()
            .filter(|e| e.group() == group && supported(e))
            .map(|e| e.as_str())
            .collect(),
    }
}

/// Misura di una GIF nella griglia a due colonne.
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
        Tab::Emoji => view_emoji(p, recent),
        Tab::Gif => view_gif(p, previews),
    };
    container(column![header, body].spacing(10))
        .padding(PADDING)
        .width(WIDTH)
        .height(HEIGHT)
        .style(card)
        .into()
}

fn view_emoji<'a>(p: &'a Picker, recent: &'a [&'static str]) -> Element<'a, Message> {
    let search = text_input("Cerca emoji (nome in inglese)", &p.emoji_query)
        .id(search_id())
        .on_input(Message::EmojiQuery)
        .padding(8)
        .style(input);

    let section_button = |icon: &'static str, section: Section| {
        let active = p.emoji_query.trim().is_empty() && p.section == section;
        button(
            text(icon)
                .size(17)
                .shaping(Shaping::Advanced)
                .align_x(Horizontal::Center),
        )
        .on_press(Message::EmojiSection(section))
        .width(38)
        .padding([4, 0])
        .style(super::style::tab(active))
    };
    let mut sections = Row::new().spacing(2);
    if !recent.is_empty() {
        sections = sections.push(section_button("🕘", Section::Recent));
    }
    for (group, icon, _) in GROUPS {
        sections = sections.push(section_button(icon, Section::Group(group)));
    }

    let title = if !p.emoji_query.trim().is_empty() {
        "Risultati".to_string()
    } else {
        match p.section {
            Section::Recent => "Usate di recente".to_string(),
            Section::Group(g) => GROUPS
                .iter()
                .find(|(group, _, _)| *group == g)
                .map(|(_, _, name)| name.to_string())
                .unwrap_or_default(),
        }
    };

    let list = emoji_list(&p.emoji_query, p.section, recent);
    let grid: Element<'a, Message> = if list.is_empty() {
        container(text("Nessuna emoji trovata").color(MUTED))
            .width(Length::Fill)
            .padding(20)
            .align_x(Horizontal::Center)
            .into()
    } else {
        let rows = list.chunks(EMOJI_COLUMNS).map(|chunk| {
            chunk
                .iter()
                .fold(Row::new().spacing(2), |r, e| {
                    r.push(
                        button(
                            text(*e)
                                .size(24)
                                .shaping(Shaping::Advanced)
                                .width(Length::Fill)
                                .align_x(Horizontal::Center),
                        )
                        .on_press(Message::PickEmoji(e))
                        .width(EMOJI_CELL)
                        .height(EMOJI_CELL)
                        .padding(2)
                        .style(list_item(false)),
                    )
                })
                .into()
        });
        Column::with_children(rows).spacing(2).into()
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
    let search = text_input("Cerca GIF", &p.gif_query)
        .id(search_id())
        .on_input(Message::GifQuery)
        .padding(8)
        .style(input);

    let query = p.gif_query.trim();
    let mut title = if query.is_empty() {
        "GIF di tendenza".to_string()
    } else {
        format!("Risultati per «{query}»")
    };
    if p.searching {
        title.push_str(" · ricerca…");
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
        Gifs::NotLoaded | Gifs::Loading => note("Caricamento delle GIF…".into()),
        Gifs::Failed(e) => note(format!("GIF non disponibili: {e}")),
        Gifs::Ready(list) if list.is_empty() => note("Nessuna GIF trovata".into()),
        Gifs::Ready(list) => {
            // Due colonne riempite come su Discord: ogni GIF va in quella più corta.
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
        text("Clic su una GIF per inviarla · GIF fornite da Klipy")
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
    fn ricerca_delle_emoji() {
        let found = emoji_list("cat", Section::Recent, &[]);
        assert!(found.contains(&"🐱"), "{found:?}");
        assert!(found.len() <= MAX_FOUND);
        // Anche con lo shortcode alla Discord.
        assert!(emoji_list(":joy:", Section::Recent, &[]).contains(&"😂"));
        assert!(emoji_list("nessuna-emoji-si-chiama-cosi", Section::Recent, &[]).is_empty());
    }

    #[test]
    fn sezioni_delle_emoji() {
        let smileys = emoji_list("", Section::Group(emojis::Group::SmileysAndEmotion), &[]);
        assert!(smileys.contains(&"😀"));
        assert!(!smileys.contains(&"🐱"));
        assert_eq!(emoji_list("", Section::Recent, &["🦀"]), vec!["🦀"]);
    }

    #[test]
    fn misura_delle_gif() {
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

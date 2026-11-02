//! Colori e stili della GUI: tema "One Dark", lo stesso del mock gpui.

use iced::theme::Palette;
use iced::widget::{button, container, scrollable, text_input};
use iced::{font, Background, Border, Color, Font, Shadow, Theme};

const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

pub const BG: Color = rgb(0x282c33);
pub const PANEL: Color = rgb(0x2f343e);
pub const TITLE: Color = rgb(0x3b414d);
pub const HOVER: Color = rgb(0x363c46);
pub const SELECTED: Color = rgb(0x454a56);
pub const BORDER: Color = rgb(0x464b57);
pub const SOFT: Color = rgb(0x363c46);
pub const TEXT: Color = rgb(0xdce0e5);
pub const MUTED: Color = rgb(0xa9afbc);
pub const ACCENT: Color = rgb(0x74ade8);
pub const GREEN: Color = rgb(0xa1c181);
pub const YELLOW: Color = rgb(0xdec184);
pub const RED: Color = rgb(0xd07277);
pub const CHIP: Color = rgb(0x34404f);
pub const MENTION_BG: Color = rgb(0x3d3a2f);
const PURPLE: Color = rgb(0xc59fe0);
const CYAN: Color = rgb(0x6fc3c9);
const ORANGE: Color = rgb(0xe0a46f);

pub const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..Font::DEFAULT
};
pub const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..Font::DEFAULT
};

pub fn theme() -> Theme {
    Theme::custom(
        "One Dark".to_string(),
        Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: GREEN,
            danger: RED,
        },
    )
}

/// Colore stabile per un utente (avatar e nome).
pub fn user_color(id: &str) -> Color {
    const COLORS: [Color; 6] = [YELLOW, GREEN, ACCENT, PURPLE, CYAN, ORANGE];
    let h = id
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[h as usize % COLORS.len()]
}

fn border(color: Color, width: f32, radius: f32) -> Border {
    Border {
        color,
        width,
        radius: radius.into(),
    }
}

pub fn fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

pub fn rounded(background: Color, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(background)),
        border: border(Color::TRANSPARENT, 0.0, radius),
        ..Default::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: border(BORDER, 1.0, 12.0),
        ..Default::default()
    }
}

pub fn outlined(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: border(BORDER, 1.0, 6.0),
        ..Default::default()
    }
}

/// Sfondo scuro semitrasparente dietro la vista a schermo intero.
pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.82, ..Color::BLACK })),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(TITLE)),
        border: border(BORDER, 1.0, 6.0),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

/// Riga di una lista (canali): trasparente, evidenziata al passaggio o se selezionata.
pub fn list_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => Some(SELECTED),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(HOVER),
            _ => None,
        };
        button::Style {
            background: background.map(Background::Color),
            text_color: TEXT,
            border: border(Color::TRANSPARENT, 0.0, 4.0),
            shadow: Shadow::default(),
        }
    }
}

/// Icona di un server nella barra a sinistra.
pub fn server(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color) = if selected {
            (ACCENT, BG)
        } else if hovered {
            (SELECTED, TEXT)
        } else {
            (TITLE, TEXT)
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            // Quadrato arrotondato se selezionato, cerchio altrimenti.
            border: border(
                Color::TRANSPARENT,
                0.0,
                if selected || hovered { 10.0 } else { 18.0 },
            ),
            shadow: Shadow::default(),
        }
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Color { a: 0.85, ..ACCENT },
        button::Status::Disabled => SELECTED,
        button::Status::Active => ACCENT,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: BG,
        border: border(Color::TRANSPARENT, 0.0, 6.0),
        shadow: Shadow::default(),
    }
}

pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(SELECTED)),
        _ => None,
    };
    button::Style {
        background,
        text_color: TEXT,
        border: border(BORDER, 1.0, 6.0),
        shadow: Shadow::default(),
    }
}

pub fn tab(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color) = match (active, hovered) {
            (true, _) => (Some(ACCENT), BG),
            (false, true) => (Some(HOVER), TEXT),
            (false, false) => (None, MUTED),
        };
        button::Style {
            background: background.map(Background::Color),
            text_color,
            border: border(Color::TRANSPARENT, 0.0, 6.0),
            shadow: Shadow::default(),
        }
    }
}

/// Reazione sotto un messaggio; bordo colorato se l'hai messa tu.
pub fn reaction(mine: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, _| button::Style {
        background: Some(Background::Color(if mine { CHIP } else { PANEL })),
        text_color: TEXT,
        border: border(if mine { ACCENT } else { BORDER }, 1.0, 12.0),
        shadow: Shadow::default(),
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused);
    text_input::Style {
        background: Background::Color(PANEL),
        border: border(if focused { ACCENT } else { BORDER }, 1.0, 8.0),
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.35, ..ACCENT },
    }
}

pub fn thin_scrollbar(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let active = !matches!(status, scrollable::Status::Active);
    let rail = || scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: if active { MUTED } else { BORDER },
            border: border(Color::TRANSPARENT, 0.0, 3.0),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail(),
        horizontal_rail: rail(),
        gap: None,
    }
}

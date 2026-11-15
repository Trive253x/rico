//! GUI colours and styles: the "One Dark" theme, the same as the gpui mock-up.

use iced::theme::Palette;
use iced::widget::{button, container, scrollable, text_input};
use std::sync::atomic::{AtomicUsize, Ordering};

use iced::{font, Background, Border, Color, Font, Shadow, Theme};

pub use crate::settings::ThemeChoice;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

/// Colours of a theme.
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub bg: Color,
    pub panel: Color,
    pub title: Color,
    pub hover: Color,
    pub selected: Color,
    pub border: Color,
    pub soft: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub chip: Color,
    pub mention_bg: Color,
    pub purple: Color,
    pub cyan: Color,
    pub orange: Color,
}

/// The original dark theme ("One Dark", like the gpui mock-up).
const DARK: Colors = Colors {
    bg: rgb(0x282c33),
    panel: rgb(0x2f343e),
    title: rgb(0x3b414d),
    hover: rgb(0x363c46),
    selected: rgb(0x454a56),
    border: rgb(0x464b57),
    soft: rgb(0x363c46),
    text: rgb(0xdce0e5),
    muted: rgb(0xa9afbc),
    accent: rgb(0x74ade8),
    green: rgb(0xa1c181),
    yellow: rgb(0xdec184),
    red: rgb(0xd07277),
    chip: rgb(0x34404f),
    mention_bg: rgb(0x3d3a2f),
    purple: rgb(0xc59fe0),
    cyan: rgb(0x6fc3c9),
    orange: rgb(0xe0a46f),
};

const LIGHT: Colors = Colors {
    bg: rgb(0xfafafa),
    panel: rgb(0xf0f0f1),
    title: rgb(0xe4e5e8),
    hover: rgb(0xe8e9ec),
    selected: rgb(0xdadce1),
    border: rgb(0xcbcdd3),
    soft: rgb(0xe8e9ec),
    text: rgb(0x2b2d33),
    muted: rgb(0x656a75),
    accent: rgb(0x3a76d0),
    green: rgb(0x3d8a3a),
    yellow: rgb(0x93660a),
    red: rgb(0xc4434b),
    chip: rgb(0xdde6f3),
    mention_bg: rgb(0xfbf0d2),
    purple: rgb(0x8a4fc4),
    cyan: rgb(0x1d8790),
    orange: rgb(0xb35d1a),
};

const AMOLED: Colors = Colors {
    bg: rgb(0x000000),
    panel: rgb(0x0b0c0e),
    title: rgb(0x16181c),
    hover: rgb(0x15171b),
    selected: rgb(0x24272d),
    border: rgb(0x2b2f36),
    soft: rgb(0x15171b),
    text: rgb(0xe6e8eb),
    muted: rgb(0x9aa0ab),
    chip: rgb(0x17202b),
    mention_bg: rgb(0x2a2616),
    ..DARK
};

const PURPLE: Colors = Colors {
    bg: rgb(0x241d33),
    panel: rgb(0x2c2340),
    title: rgb(0x3a2e54),
    hover: rgb(0x342a4b),
    selected: rgb(0x46385f),
    border: rgb(0x4c3d68),
    soft: rgb(0x342a4b),
    text: rgb(0xebe4f6),
    muted: rgb(0xb5a8cd),
    accent: rgb(0xb48cf0),
    yellow: rgb(0xe3c47f),
    red: rgb(0xe07a8a),
    chip: rgb(0x3a2d55),
    mention_bg: rgb(0x433a2c),
    purple: rgb(0xd5aeff),
    ..DARK
};

/// Light brown: a warm light theme.
const LATTE: Colors = Colors {
    bg: rgb(0xf5ede3),
    panel: rgb(0xebe0d2),
    title: rgb(0xdfd0bd),
    hover: rgb(0xe5d8c6),
    selected: rgb(0xd8c5ac),
    border: rgb(0xc9b497),
    soft: rgb(0xe5d8c6),
    text: rgb(0x3b2e22),
    muted: rgb(0x75614f),
    accent: rgb(0x9b5c33),
    green: rgb(0x55783a),
    yellow: rgb(0x91620f),
    red: rgb(0xb0443d),
    chip: rgb(0xe3d3bd),
    mention_bg: rgb(0xf2e0b3),
    purple: rgb(0x85559a),
    cyan: rgb(0x367f7a),
    orange: rgb(0xad5f25),
};

const BLUE: Colors = Colors {
    bg: rgb(0x1b2433),
    panel: rgb(0x202b3d),
    title: rgb(0x2a3850),
    hover: rgb(0x263247),
    selected: rgb(0x33425e),
    border: rgb(0x384863),
    soft: rgb(0x263247),
    text: rgb(0xe1e8f2),
    muted: rgb(0xa5b2c7),
    accent: rgb(0x6fb0ff),
    green: rgb(0x9cc98a),
    red: rgb(0xe07b80),
    chip: rgb(0x23344f),
    mention_bg: rgb(0x3a3a2c),
    cyan: rgb(0x6fcfd8),
    ..DARK
};

const RED: Colors = Colors {
    bg: rgb(0x2e1f22),
    panel: rgb(0x37252a),
    title: rgb(0x472f35),
    hover: rgb(0x402a30),
    selected: rgb(0x553840),
    border: rgb(0x5b3d45),
    soft: rgb(0x402a30),
    text: rgb(0xf3e5e7),
    muted: rgb(0xc6aaaf),
    accent: rgb(0xe8956f),
    green: rgb(0xa7c48a),
    yellow: rgb(0xe6c27d),
    red: rgb(0xe2646d),
    chip: rgb(0x45292f),
    mention_bg: rgb(0x4a3a26),
    purple: rgb(0xd3a2e0),
    cyan: rgb(0x7cc4c4),
    orange: rgb(0xe8a46f),
};

const PALETTES: [Colors; 7] = [DARK, LIGHT, AMOLED, PURPLE, LATTE, BLUE, RED];

/// Index in [`PALETTES`] of the theme in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Switches theme; `system_dark` decides [`ThemeChoice::System`].
pub fn apply(choice: ThemeChoice, system_dark: bool) {
    let index = match choice {
        ThemeChoice::Dark => 0,
        ThemeChoice::Light => 1,
        ThemeChoice::System if system_dark => 0,
        ThemeChoice::System => 1,
        ThemeChoice::Amoled => 2,
        ThemeChoice::Purple => 3,
        ThemeChoice::Latte => 4,
        ThemeChoice::Blue => 5,
        ThemeChoice::Red => 6,
    };
    CURRENT.store(index, Ordering::Relaxed);
}

/// Colours of a theme, for the previews in the settings.
pub fn colors_of(choice: ThemeChoice, system_dark: bool) -> Colors {
    let before = CURRENT.load(Ordering::Relaxed);
    apply(choice, system_dark);
    let colors = current();
    CURRENT.store(before, Ordering::Relaxed);
    colors
}

pub fn current() -> Colors {
    PALETTES[CURRENT.load(Ordering::Relaxed).min(PALETTES.len() - 1)]
}

/// The colours of the theme in use.
pub mod pal {
    use iced::Color;

    macro_rules! colour {
        ($($name:ident),*) => {
            $(
                pub fn $name() -> Color {
                    super::current().$name
                }
            )*
        };
    }

    colour!(
        bg, panel, title, hover, selected, border, soft, text, muted, accent, green, yellow, red, chip,
        mention_bg, purple, cyan, orange
    );
}

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
        "ferrocord".to_string(),
        Palette {
            background: pal::bg(),
            text: pal::text(),
            primary: pal::accent(),
            success: pal::green(),
            danger: pal::red(),
        },
    )
}

/// Stable colour for a user (avatar and name).
pub fn user_color(id: &str) -> Color {
    let colors = [
        pal::yellow(),
        pal::green(),
        pal::accent(),
        pal::purple(),
        pal::cyan(),
        pal::orange(),
    ];
    let h = id
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
    colors[h as usize % colors.len()]
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
        background: Some(Background::Color(pal::panel())),
        border: border(pal::border(), 1.0, 12.0),
        ..Default::default()
    }
}

pub fn outlined(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal::panel())),
        border: border(pal::border(), 1.0, 6.0),
        ..Default::default()
    }
}

/// Dark translucent background behind the full-screen view.
pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color {
            a: 0.82,
            ..Color::BLACK
        })),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(pal::title())),
        border: border(pal::border(), 1.0, 6.0),
        text_color: Some(pal::text()),
        ..Default::default()
    }
}

/// List row (channels): transparent, highlighted on hover or when selected.
pub fn list_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => Some(pal::selected()),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(pal::hover()),
            _ => None,
        };
        button::Style {
            background: background.map(Background::Color),
            text_color: pal::text(),
            border: border(Color::TRANSPARENT, 0.0, 4.0),
            shadow: Shadow::default(),
        }
    }
}

/// Face of a server folder in the column, on the folder's colour.
pub fn folder_face(tint: Color, open: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let alpha = match (open, hovered) {
            (true, false) => 0.0,
            (true, true) => 0.15,
            (false, false) => 0.28,
            (false, true) => 0.4,
        };
        button::Style {
            background: Some(Background::Color(Color { a: alpha, ..tint })),
            text_color: pal::text(),
            border: border(Color::TRANSPARENT, 0.0, 12.0),
            shadow: Shadow::default(),
        }
    }
}

/// A colour to choose (folder colours); the chosen one is outlined.
pub fn swatch(color: Color, chosen: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(color)),
            text_color: pal::text(),
            border: border(
                if chosen || hovered {
                    pal::text()
                } else {
                    Color::TRANSPARENT
                },
                2.0,
                4.0,
            ),
            shadow: Shadow::default(),
        }
    }
}

/// A folder in the folder editor: a band of its colour on the left.
pub fn folder_box(tint: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(pal::panel())),
        border: border(Color { a: 0.6, ..tint }, 1.0, 8.0),
        ..Default::default()
    }
}

/// A theme in the settings: the chosen one has an accent outline.
pub fn theme_choice(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if hovered {
                pal::hover()
            } else {
                pal::panel()
            })),
            text_color: pal::text(),
            border: border(
                if selected {
                    pal::accent()
                } else {
                    Color::TRANSPARENT
                },
                2.0,
                8.0,
            ),
            shadow: Shadow::default(),
        }
    }
}

/// Author's name in a message: highlighted on hover, a step brighter than
/// the list rows because the message under the mouse is highlighted too.
pub fn name_link(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(pal::selected())),
        _ => None,
    };
    button::Style {
        background,
        text_color: pal::text(),
        border: border(Color::TRANSPARENT, 0.0, 4.0),
        shadow: Shadow::default(),
    }
}

/// Around an avatar that opens the profile: a light ring on hover (the
/// button has 2 px of padding, the ring is the background showing around the
/// round picture).
pub fn avatar_ring(size: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: hovered.then_some(Background::Color(pal::muted())),
            text_color: pal::text(),
            border: border(Color::TRANSPARENT, 0.0, size / 2.0 + 2.0),
            shadow: Shadow::default(),
        }
    }
}

/// Server icon in the left bar.
pub fn server(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color) = if selected {
            (pal::accent(), pal::bg())
        } else if hovered {
            (pal::selected(), pal::text())
        } else {
            (pal::title(), pal::text())
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            // Rounded square when selected, circle otherwise.
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
        button::Status::Hovered | button::Status::Pressed => Color {
            a: 0.85,
            ..pal::accent()
        },
        button::Status::Disabled => pal::selected(),
        button::Status::Active => pal::accent(),
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: pal::bg(),
        border: border(Color::TRANSPARENT, 0.0, 6.0),
        shadow: Shadow::default(),
    }
}

pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(pal::selected())),
        _ => None,
    };
    button::Style {
        background,
        text_color: pal::text(),
        border: border(pal::border(), 1.0, 6.0),
        shadow: Shadow::default(),
    }
}

/// Icon button with no border: a soft background only under the mouse.
pub fn icon_button(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(pal::selected())),
        _ => None,
    };
    button::Style {
        background,
        text_color: pal::text(),
        border: border(Color::TRANSPARENT, 0.0, 6.0),
        shadow: Shadow::default(),
    }
}

/// Button around an image (server icons): no background or border at all.
pub fn bare(_: &Theme, _: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: pal::text(),
        border: border(Color::TRANSPARENT, 0.0, 0.0),
        shadow: Shadow::default(),
    }
}

/// How a round control button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// On, as usual (microphone open, sound on).
    On,
    /// Turned off by the user (muted): red tint, so it is seen at a glance.
    Off,
    /// Leaves something (hang up, stop watching): solid red.
    Leave,
}

/// Round icon button of the voice and stream controls.
pub fn control(kind: Control) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = match (kind, hovered) {
            (_, _) if status == button::Status::Disabled => pal::soft(),
            (Control::On, false) => pal::soft(),
            (Control::On, true) => pal::selected(),
            (Control::Off, false) => Color {
                a: 0.22,
                ..pal::red()
            },
            (Control::Off, true) => Color {
                a: 0.34,
                ..pal::red()
            },
            (Control::Leave, false) => pal::red(),
            (Control::Leave, true) => Color {
                a: 0.85,
                ..pal::red()
            },
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: if kind == Control::Leave {
                pal::bg()
            } else {
                pal::text()
            },
            border: border(Color::TRANSPARENT, 0.0, 999.0),
            shadow: Shadow::default(),
        }
    }
}

/// Icon colour for a [`control`] button.
pub fn control_icon(kind: Control, enabled: bool) -> Color {
    match kind {
        _ if !enabled => pal::muted(),
        Control::On => pal::text(),
        Control::Off => pal::red(),
        Control::Leave => pal::bg(),
    }
}

pub fn tab(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color) = match (active, hovered) {
            (true, _) => (Some(pal::accent()), pal::bg()),
            (false, true) => (Some(pal::hover()), pal::text()),
            (false, false) => (None, pal::muted()),
        };
        button::Style {
            background: background.map(Background::Color),
            text_color,
            border: border(Color::TRANSPARENT, 0.0, 6.0),
            shadow: Shadow::default(),
        }
    }
}

/// Reaction under a message; coloured border if you added it.
pub fn reaction(mine: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, _| button::Style {
        background: Some(Background::Color(if mine { pal::chip() } else { pal::panel() })),
        text_color: pal::text(),
        border: border(if mine { pal::accent() } else { pal::border() }, 1.0, 12.0),
        shadow: Shadow::default(),
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused);
    text_input::Style {
        background: Background::Color(pal::panel()),
        border: border(if focused { pal::accent() } else { pal::border() }, 1.0, 8.0),
        icon: pal::muted(),
        placeholder: pal::muted(),
        value: pal::text(),
        selection: Color {
            a: 0.35,
            ..pal::accent()
        },
    }
}

pub fn thin_scrollbar(_: &Theme, status: scrollable::Status) -> scrollable::Style {
    let active = !matches!(status, scrollable::Status::Active);
    let rail = || scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: if active { pal::muted() } else { pal::border() },
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

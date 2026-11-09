//! Small line icons for the voice and stream controls, drawn for ferrocord
//! (24×24, white strokes: the colour is set when drawing).

use std::sync::LazyLock;

use iced::widget::svg::{self, Svg};
use iced::{Color, Length};

macro_rules! icon {
    ($name:ident, $body:literal) => {
        pub static $name: LazyLock<svg::Handle> = LazyLock::new(|| {
            svg::Handle::from_memory(
                concat!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"##,
                    $body,
                    "</svg>"
                )
                .as_bytes(),
            )
        });
    };
}

icon!(
    MIC,
    r##"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0"/><path d="M12 17.5V21M8.5 21h7"/>"##
);
icon!(
    MIC_OFF,
    r##"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0"/><path d="M12 17.5V21M8.5 21h7"/><path d="M4 4l16 16" stroke-width="2.4"/>"##
);
icon!(
    HEADPHONES,
    r##"<path d="M4 16v-4a8 8 0 0 1 16 0v4"/><rect x="3" y="14" width="5" height="7" rx="2"/><rect x="16" y="14" width="5" height="7" rx="2"/>"##
);
icon!(
    HEADPHONES_OFF,
    r##"<path d="M4 16v-4a8 8 0 0 1 16 0v4"/><rect x="3" y="14" width="5" height="7" rx="2"/><rect x="16" y="14" width="5" height="7" rx="2"/><path d="M4 4l16 16" stroke-width="2.4"/>"##
);
icon!(
    VOLUME,
    r##"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="#fff"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6"/><path d="M18 6.5a8 8 0 0 1 0 11"/>"##
);
icon!(
    VOLUME_LOW,
    r##"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="#fff"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6"/>"##
);
icon!(
    VOLUME_OFF,
    r##"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="#fff"/><path d="M16 9.5l5 5M21 9.5l-5 5"/>"##
);
icon!(
    HANG_UP,
    r##"<path d="M3 14.5c4.8-4.6 13.2-4.6 18 0l-2.2 2.6-3.8-1.2v-2.4a9.5 9.5 0 0 0-6 0v2.4L5.2 17.1z" fill="#fff" stroke-width="1.5"/>"##
);
icon!(
    EXIT,
    r##"<path d="M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/><path d="M15 8l4 4-4 4M19 12H9"/>"##
);

/// An icon of `size` pixels in `color`.
pub fn icon<'a>(handle: &LazyLock<svg::Handle>, size: f32, color: Color) -> Svg<'a> {
    Svg::new((**handle).clone())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_, _| svg::Style { color: Some(color) })
}

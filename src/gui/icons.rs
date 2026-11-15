//! Small line icons for the voice, stream and message controls, drawn for Rico
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
    FOLDER,
    r##"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="#fff" stroke-width="1.5"/>"##
);
icon!(
    GEAR,
    r##"<circle cx="12" cy="12" r="3"/><path d="M12 2.5v2.6M12 18.9v2.6M2.5 12h2.6M18.9 12h2.6M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8"/><circle cx="12" cy="12" r="6.5"/>"##
);
icon!(
    EXIT,
    r##"<path d="M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/><path d="M15 8l4 4-4 4M19 12H9"/>"##
);
icon!(
    FULLSCREEN,
    r##"<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>"##
);
icon!(
    FULLSCREEN_EXIT,
    r##"<path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/>"##
);
icon!(
    PEOPLE,
    r##"<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M16 4.6a3.5 3.5 0 0 1 0 6.8M18 14.2a6.5 6.5 0 0 1 3.5 5.8"/>"##
);

icon!(
    SCREEN,
    r##"<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/><path d="M12 7.5v5M9.8 9.7 12 7.5l2.2 2.2"/>"##
);

icon!(
    REACT,
    r##"<path d="M20.5 12a8.5 8.5 0 1 1-6-8.1"/><path d="M8.5 14.5a4.2 4.2 0 0 0 7 0"/><path d="M9 9.5h.01M15 9.5h.01" stroke-width="2.6"/><path d="M19 2.5v5M16.5 5h5"/>"##
);
icon!(
    SMILE,
    r##"<circle cx="12" cy="12" r="9"/><path d="M8.5 14a4.2 4.2 0 0 0 7 0"/><path d="M9 9.5h.01M15 9.5h.01" stroke-width="2.6"/>"##
);
icon!(
    GIF,
    r##"<rect x="1.5" y="4.5" width="21" height="15" rx="3.5" stroke-width="1.5"/><path d="M9.2 8.5H6.7a2.5 2.5 0 0 0-2.5 2.5v2a2.5 2.5 0 0 0 2.5 2.5h.5a2 2 0 0 0 2-2V12H7.2M12.5 8.5v7M16 15.5v-7h4M16 12h3.2"/>"##
);
icon!(
    REPLY,
    r##"<path d="M9.5 5 4 10.5 9.5 16"/><path d="M4 10.5h9.5a6.5 6.5 0 0 1 6.5 6.5v2"/>"##
);
icon!(
    EDIT,
    r##"<path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4z"/><path d="M14.5 5.5l3 3"/>"##
);
icon!(
    TRASH,
    r##"<path d="M4 6.5h16M9.5 6.5V4.5h5v2"/><path d="M6 6.5l1 13a1.5 1.5 0 0 0 1.5 1.4h7a1.5 1.5 0 0 0 1.5-1.4l1-13"/><path d="M10 11v6M14 11v6"/>"##
);

/// An icon of `size` pixels in `color`.
pub fn icon<'a>(handle: &LazyLock<svg::Handle>, size: f32, color: Color) -> Svg<'a> {
    Svg::new((**handle).clone())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_, _| svg::Style { color: Some(color) })
}

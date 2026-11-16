//! Our status (online, idle, do not disturb, invisible) and custom status:
//! the menu that opens from our name at the bottom left.
//!
//! Both are saved in the account settings (field 11 of the settings
//! protobuf, see `discord::folders`), so every device shows them, and sent to
//! the gateway (op 3) so that this session shows them at once.

use std::fmt;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, column, container, mouse_area, opaque, pick_list, row, rule, space, text, text_input, Space,
};
use iced::{Color, Element, Length, Padding};

use super::style::{self, ghost, list_item, pal, primary, SEMIBOLD};
use super::Message;
use crate::discord::folders::{CustomStatus, Presence, StatusSettings, MAX_CUSTOM_STATUS};
use crate::i18n::t;

/// When the custom status goes away by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClearAfter {
    #[default]
    Never,
    HalfHour,
    Hour,
    FourHours,
    /// At midnight (local time).
    Today,
}

impl ClearAfter {
    pub const ALL: [Self; 5] = [
        Self::Never,
        Self::HalfHour,
        Self::Hour,
        Self::FourHours,
        Self::Today,
    ];

    /// The end, in Unix ms (0 = never), counting from `now`.
    pub fn expires_at_ms(self, now: chrono::DateTime<chrono::Local>) -> u64 {
        let minutes = match self {
            Self::Never => return 0,
            Self::HalfHour => 30,
            Self::Hour => 60,
            Self::FourHours => 240,
            Self::Today => {
                let midnight = (now.date_naive() + chrono::Days::new(1))
                    .and_hms_opt(0, 0, 0)
                    .and_then(|m| m.and_local_timezone(chrono::Local).earliest());
                return midnight.map_or(0, |m| u64::try_from(m.timestamp_millis()).unwrap_or(0));
            }
        };
        let end = now + chrono::Duration::minutes(minutes);
        u64::try_from(end.timestamp_millis()).unwrap_or(0)
    }
}

impl fmt::Display for ClearAfter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Never => t("Don't clear", "Non cancellare"),
            Self::HalfHour => t("30 minutes", "30 minuti"),
            Self::Hour => t("1 hour", "1 ora"),
            Self::FourHours => t("4 hours", "4 ore"),
            Self::Today => t("Today", "Oggi"),
        })
    }
}

/// The open menu.
#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub text: String,
    pub clear_after: ClearAfter,
    pub saving: bool,
    pub error: Option<String>,
}

impl Menu {
    /// Starts from the custom status shown now.
    pub fn new(current: Option<&StatusSettings>, now_ms: u64) -> Self {
        let text = current
            .and_then(|s| s.custom.as_ref())
            .filter(|c| c.active(now_ms))
            .map(|c| c.text.clone())
            .unwrap_or_default();
        Self {
            text,
            ..Default::default()
        }
    }

    pub fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Text(s) => self.text = s.chars().take(MAX_CUSTOM_STATUS).collect(),
            Edit::ClearAfter(c) => self.clear_after = c,
        }
    }

    /// The custom status to save (`None` if the text is empty).
    pub fn custom(&self, now: chrono::DateTime<chrono::Local>) -> Option<CustomStatus> {
        let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
        let c = CustomStatus::new(&self.text, self.clear_after.expires_at_ms(now), now_ms);
        (!c.is_empty()).then_some(c)
    }
}

#[derive(Debug, Clone)]
pub enum Edit {
    Text(String),
    ClearAfter(ClearAfter),
}

/// The colour of the dot of a status.
pub fn colour(p: Presence) -> Color {
    match p {
        Presence::Online => pal::green(),
        Presence::Idle => pal::yellow(),
        Presence::DoNotDisturb => pal::red(),
        Presence::Invisible => pal::muted(),
    }
}

pub fn dot<'a>(p: Presence, size: f32) -> Element<'a, Message> {
    container(Space::new().width(size).height(size))
        .style(style::rounded(colour(p), size / 2.0))
        .into()
}

/// The menu, above our name at the bottom left; a click outside closes it.
pub fn view<'a>(menu: &'a Menu, current: Presence, has_custom: bool, me_id: &str) -> Element<'a, Message> {
    let mut statuses = column![].spacing(2);
    for p in Presence::ALL {
        let mut line = row![dot(p, 10.0), text(p.label()).size(13).width(Length::Fill)]
            .spacing(10)
            .align_y(Vertical::Center);
        if p == current {
            line = line.push(text("✓").size(13).color(pal::accent()));
        }
        let mut entry = column![line].spacing(1);
        if p == Presence::Invisible {
            entry = entry.push(
                container(
                    text(t(
                        "Others see you offline; you can still use everything.",
                        "Gli altri ti vedono offline; puoi comunque usare tutto.",
                    ))
                    .size(11)
                    .color(pal::muted()),
                )
                .padding(Padding {
                    left: 20.0,
                    ..Padding::ZERO
                }),
            );
        }
        statuses = statuses.push(
            button(entry)
                .width(Length::Fill)
                .padding([6, 8])
                .style(list_item(p == current))
                .on_press_maybe((!menu.saving).then_some(Message::SetPresence(p))),
        );
    }

    let input = text_input(t("What are you up to?", "Cosa stai facendo?"), &menu.text)
        .on_input(|s| Message::StatusEdit(Edit::Text(s)))
        .on_submit(Message::SaveCustomStatus)
        .padding(8)
        .size(13)
        .style(style::input);
    let clear_after = row![
        text(t("Clear after", "Cancella dopo"))
            .size(12)
            .color(pal::muted()),
        space::horizontal(),
        pick_list(&ClearAfter::ALL[..], Some(menu.clear_after), |c| {
            Message::StatusEdit(Edit::ClearAfter(c))
        })
        .text_size(12)
        .padding([4, 8]),
    ]
    .align_y(Vertical::Center);
    let save = button(text(t("Save", "Salva")).size(12).font(SEMIBOLD))
        .padding([5, 12])
        .style(primary)
        .on_press_maybe((!menu.saving).then_some(Message::SaveCustomStatus));
    let mut actions = row![space::horizontal()].spacing(6);
    if has_custom {
        actions = actions.push(
            button(text(t("Remove", "Rimuovi")).size(12))
                .padding([5, 12])
                .style(ghost)
                .on_press_maybe((!menu.saving).then_some(Message::ClearCustomStatus)),
        );
    }
    actions = actions.push(save);

    let mut card = column![
        text(t("STATUS", "STATO"))
            .size(11)
            .font(SEMIBOLD)
            .color(pal::muted()),
        statuses,
        rule::horizontal(1),
        text(t("CUSTOM STATUS", "STATO PERSONALIZZATO"))
            .size(11)
            .font(SEMIBOLD)
            .color(pal::muted()),
        input,
        clear_after,
        actions,
    ]
    .spacing(8);
    if let Some(e) = &menu.error {
        card = card.push(text(e.clone()).size(12).color(pal::red()));
    }
    card = card.push(rule::horizontal(1));
    card = card.push(
        button(text(t("View profile", "Mostra profilo")).size(12))
            .width(Length::Fill)
            .padding([5, 8])
            .style(ghost)
            .on_press(Message::OpenProfile {
                user_id: me_id.to_string(),
                guild_id: None,
            }),
    );

    let panel = container(opaque(container(card).width(290).padding(12).style(style::card)))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Left)
        .align_y(Vertical::Bottom)
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 60.0,
            left: 72.0,
        });
    iced::widget::stack![
        mouse_area(Space::new().width(Length::Fill).height(Length::Fill)).on_press(Message::CloseStatusMenu),
        panel
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::folders::clean_status_text;
    use chrono::TimeZone;

    #[test]
    fn clear_after_times() {
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 16, 22, 30, 0).unwrap();
        let ms = |d: chrono::DateTime<chrono::Local>| d.timestamp_millis() as u64;
        assert_eq!(ClearAfter::Never.expires_at_ms(now), 0);
        assert_eq!(ClearAfter::Hour.expires_at_ms(now), ms(now) + 3_600_000);
        let midnight = chrono::Local.with_ymd_and_hms(2026, 11, 17, 0, 0, 0).unwrap();
        assert_eq!(ClearAfter::Today.expires_at_ms(now), ms(midnight));
    }

    #[test]
    fn the_menu_builds_the_custom_status() {
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 16, 22, 30, 0).unwrap();
        let mut m = Menu::default();
        m.apply(Edit::Text("  At work\n".into()));
        m.apply(Edit::ClearAfter(ClearAfter::HalfHour));
        let c = m.custom(now).unwrap();
        assert_eq!(c.text, "At work");
        assert!(c.active(now.timestamp_millis() as u64));
        m.apply(Edit::Text("   ".into()));
        assert!(m.custom(now).is_none());
        m.apply(Edit::Text("x".repeat(500)));
        assert_eq!(m.text.chars().count(), MAX_CUSTOM_STATUS);
        assert_eq!(clean_status_text(&m.text).chars().count(), MAX_CUSTOM_STATUS);
    }
}

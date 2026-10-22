//! A choice put to the player as a list — what of a game to download, which of its builds to play,
//! where downloads go. The whole window, like the save conflict's chooser: one question at a time,
//! answered with the keys that walk every other list.

use iced::widget::{self as widget, button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Element, Length};

use crate::app::Message;
use crate::theme;

/// A row's height, fixed so the list scrolls the cursor into view by arithmetic.
pub const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 8.0;

pub fn list_id() -> widget::Id {
    widget::Id::new("picker-list")
}

/// Where the list scrolls to keep row `index` in view, a couple of rows from the top.
pub fn scroll_offset(index: usize) -> f32 {
    (index.saturating_sub(2) as f32 * (ROW_H + ROW_GAP)).max(0.0)
}

/// Scroll the list to keep row `index` in view — see [`scroll_offset`]. A picker just opened asks
/// for its cursor's row, since the list keeps whatever offset the last picker left it at.
pub fn scroll_to<T>(index: usize) -> iced::Task<T> {
    iced::widget::operation::scroll_to(list_id(), scrollable::AbsoluteOffset { x: 0.0, y: scroll_offset(index) })
}

/// One answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// The line under it: which file, which drive.
    pub detail: String,
    /// Its figure, at the right: a size, free space.
    pub aside: String,
    /// Marked beside its label — "on this device", "in use".
    pub mark: Option<String>,
}

impl Row {
    /// An answer with nothing at its right and no mark.
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { label: label.into(), detail: detail.into(), aside: String::new(), mark: None }
    }
}

/// The question, its answers, and the one the cursor is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub title: String,
    pub note: String,
    pub rows: Vec<Row>,
    pub cursor: usize,
}

impl Picker {
    /// Move the cursor, clamped to the ends. Answers whether it moved.
    pub fn step(&mut self, delta: isize) -> bool {
        let before = self.cursor;
        self.cursor = self.cursor.saturating_add_signed(delta).min(self.rows.len().saturating_sub(1));
        before != self.cursor
    }
}

pub fn view<'a>(picker: &Picker) -> Element<'a, Message> {
    let mut list = column![].spacing(ROW_GAP);
    for (i, answer) in picker.rows.iter().enumerate() {
        let lit = i == picker.cursor;
        let mut heading = row![text(answer.label.clone())
            .size(theme::text_size::TITLE_MEDIUM)
            .font(if lit { theme::MEDIUM } else { theme::REGULAR })
            .color(theme::TEXT_PRIMARY)]
        .spacing(10)
        .align_y(Alignment::Center);
        if let Some(mark) = &answer.mark {
            heading = heading.push(text(mark.to_uppercase()).size(10.0).font(theme::BOLD).color(theme::ACCENT));
        }
        let body = row![
            column![
                heading,
                text(answer.detail.clone()).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY),
            ]
            .spacing(3)
            .width(Length::Fill),
            text(answer.aside.clone()).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY),
        ]
        .spacing(16)
        .align_y(Alignment::Center);
        list = list.push(
            button(container(body).height(Length::Fill).center_y(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fixed(ROW_H))
                .padding([0, 18])
                .on_press(Message::Picked(i))
                .style(crate::ui::style::row(lit, 12.0)),
        );
    }

    let card = column![
        text(picker.title.clone()).size(theme::text_size::HEADLINE_SMALL).font(theme::MEDIUM).color(theme::TEXT_PRIMARY),
        text(picker.note.clone()).size(theme::text_size::BODY_MEDIUM).color(theme::TEXT_SECONDARY),
        Space::new().height(Length::Fixed(4.0)),
        scrollable(list).id(list_id()).height(Length::Fill),
        text("Up / Down to choose  ·  Enter or A picks it  ·  Esc or B cancels")
            .size(theme::text_size::LABEL_MEDIUM)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(12)
    .max_width(760);
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([40, 28])
        .center_x(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(theme::BACKGROUND)), ..Default::default() })
        .into()
}

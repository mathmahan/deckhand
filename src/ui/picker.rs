//! A choice put to the player as a list — what of a game to download, which of its builds to play,
//! where downloads go. The whole window, like the save conflict's chooser: one question at a time,
//! answered with the keys that walk every other list.
//!
//! Drawn as a page is, on Home's ground: the question where a page has its heading, and the
//! answers as glass rows, the one under the cursor lifted on the ink ring. [`sheet`] and [`heading`]
//! draw the text entry and the save conflict the same way.

use iced::widget::{self as widget, button, column, container, row, scrollable, stack, text, Space};
use iced::{Alignment, Element, Length, Padding};

use crate::app::Message;
use crate::theme::{self, alpha};

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
        let mut heading = row![text(answer.label.clone()).size(17.0).font(theme::SEMIBOLD).color(theme::INK)].spacing(10).align_y(Alignment::Center);
        if let Some(mark) = &answer.mark {
            heading = heading.push(text(mark.to_uppercase()).size(11.0).font(theme::BOLD).color(theme::ACCENT_400));
        }
        let body = row![
            column![heading, text(answer.detail.clone()).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY)].spacing(3).width(Length::Fill),
            text(answer.aside.clone()).size(theme::text_size::LABEL_LARGE).color(alpha(theme::INK, 0.76)),
        ]
        .spacing(16)
        .align_y(Alignment::Center);
        list = list.push(
            button(container(body).height(Length::Fill).center_y(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fixed(ROW_H))
                .padding([0, 20])
                .on_press(Message::Picked(i))
                .style(crate::ui::style::glass_row(lit, 16.0)),
        );
    }

    let count = match picker.rows.len() {
        1 => "1 OPTION".to_string(),
        n => format!("{n} OPTIONS"),
    };
    let page = column![
        heading("CHOOSE", Some(count), picker.title.clone(), picker.note.clone()),
        Space::new().height(Length::Fixed(22.0)),
        // Room at the bottom of the list, so the lit row's shadow is not cut off by the scroll.
        scrollable(list.push(Space::new().height(Length::Fixed(16.0))))
            .id(list_id())
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
            .height(Length::Fill),
    ];
    sheet(page.into(), vec![("A", "Choose".to_string()), ("B", "Cancel".to_string())])
}

/// What takes the whole window for a moment — a choice, a field to type in, a save conflict: Home's
/// ground with the accent wash, `page` from the top at the page's margin, and what the pad's buttons
/// do at the foot. Drawn over the page, which it hides.
pub fn sheet<'a>(page: Element<'a, Message>, hints: Vec<(&'a str, String)>) -> Element<'a, Message> {
    let margin = crate::ui::tab_bar::MARGIN;
    stack![
        crate::ui::home::plain_stage(),
        column![
            container(page).padding(Padding { top: 40.0, right: margin, bottom: 0.0, left: margin }).width(Length::Fill).height(Length::Fill),
            crate::ui::tab_bar::hint_line(hints),
        ],
    ]
    .into()
}

/// A sheet's heading, as a page's: the tag and a word beside it, the question, and what it is about.
pub fn heading<'a>(tag: &str, eyebrow: Option<String>, title: String, note: String) -> Element<'a, Message> {
    let mut top = row![crate::ui::home::tag(tag.to_string(), true)].spacing(12).align_y(Alignment::Center);
    if let Some(eyebrow) = eyebrow {
        top = top.push(text(eyebrow).size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82)));
    }
    let mut heading = column![top, text(title).size(40.0).font(theme::DISPLAY).color(theme::INK)].spacing(12).max_width(1000);
    if !note.is_empty() {
        heading = heading.push(text(note).size(17.0).color(alpha(theme::INK, 0.86)));
    }
    heading.into()
}

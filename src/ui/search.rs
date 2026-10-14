//! The Search page — a port of `browse/ui/SearchView.kt`: an on-screen keyboard over results from
//! every system.
//!
//! Results are a list, never a grid: they come from systems whose box art has different shapes, and
//! a grid of mixed ratios has no rhythm to scan. Each row says which system it is from and whether
//! the game is on this device, the two things a cross-system list loses that a grid would show.

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Element, Length, Padding};

use crate::app::Message;
use crate::downloads::{Downloads, SyncState};
use crate::library::{Game, Library};
use crate::search::{BACKSPACE, ROWS, SearchFocus};
use crate::theme;
use crate::ui::home::truncate;

/// A result row's height and the gap under it — fixed, so the cursor's scroll offset is computed.
const ROW_H: f32 = 44.0;
const ROW_GAP: f32 = 4.0;
const KEY_W: f32 = 44.0;
const KEY_H: f32 = 40.0;
const KEY_GAP: f32 = 8.0;

pub fn results_id() -> widget::Id {
    widget::Id::new("search-results")
}

/// Where the results list scrolls for the cursor on `index`: that row a quarter of the list down.
pub fn scroll_offset(index: usize, list_h: f32) -> f32 {
    (index as f32 * (ROW_H + ROW_GAP) - list_h / 4.0).max(0.0)
}

pub fn search_view<'a>(
    focus: &SearchFocus,
    results: &[&'a Game],
    library: &'a Library,
    downloads: &Downloads,
) -> Element<'a, Message> {
    let heading = row![
        text("Search").size(theme::text_size::HEADLINE_SMALL).font(theme::MEDIUM).color(theme::TEXT_PRIMARY),
        text(format!("{}▌", focus.query)).size(theme::text_size::BODY_LARGE).color(theme::ACCENT),
    ]
    .spacing(24)
    .align_y(Alignment::Center);

    let hint = text("Type, or walk the keyboard  ·  Down into the results  ·  Enter opens a game  ·  Esc goes back")
        .size(theme::text_size::LABEL_MEDIUM)
        .color(theme::TEXT_MUTED);

    let cursor = focus.result(results.len());
    let mut list = column![].spacing(ROW_GAP);
    for (i, game) in results.iter().enumerate() {
        list = list.push(result_row(i, game, library, downloads, focus.in_results && i == cursor));
    }
    let results: Element<Message> = if results.is_empty() {
        let message = if focus.query.trim().is_empty() { "Every system, by title — ff7 and sotc work too." } else { "No matches." };
        text(message).size(theme::text_size::BODY_LARGE).color(theme::TEXT_SECONDARY).into()
    } else {
        // Room under the last row, so it scrolls up to the same anchor as the rest.
        scrollable(column![list, Space::new().height(Length::Fixed(160.0))])
            .id(results_id())
            .on_scroll(|_| Message::Scrolled)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    column![heading, hint, keyboard(focus), results]
        .spacing(16)
        .padding(Padding { top: 20.0, right: 28.0, bottom: 0.0, left: 28.0 })
        .into()
}

/// The keyboard, one rectangle: every row as wide as the widest, its keys stretched to fill.
fn keyboard<'a>(focus: &SearchFocus) -> Element<'a, Message> {
    let widest = ROWS.iter().map(|r| r.chars().count()).max().unwrap_or(1) as f32;
    let width = widest * KEY_W + (widest - 1.0) * KEY_GAP;
    let mut rows = column![].spacing(KEY_GAP);
    for (r, keys) in ROWS.iter().enumerate() {
        let mut line = row![].spacing(KEY_GAP).width(Length::Fixed(width));
        for (c, key) in keys.chars().enumerate() {
            let lit = !focus.in_results && focus.key_row == r && focus.key_col == c;
            let label = match key {
                ' ' => "␣".to_string(),
                BACKSPACE => "⌫".to_string(),
                k => k.to_string(),
            };
            line = line.push(
                button(container(text(label).size(theme::text_size::BODY_LARGE)).center_x(Length::Fill).center_y(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fixed(KEY_H))
                    .padding(0)
                    .on_press(Message::SearchKey(key))
                    .style(crate::ui::style::key(lit, false)),
            );
        }
        rows = rows.push(line);
    }
    rows.into()
}

fn result_row<'a>(index: usize, game: &'a Game, library: &'a Library, downloads: &Downloads, lit: bool) -> Element<'a, Message> {
    let badge = match downloads.state_of(game.builds()) {
        SyncState::Installed => "ON DEVICE".to_string(),
        SyncState::UpdateAvailable => "UPDATE".to_string(),
        SyncState::Downloading(p) | SyncState::Unpacking(p) => format!("{}%", (p * 100.0) as i32),
        SyncState::Failed(_) => "FAILED".to_string(),
        SyncState::RemoteOnly => String::new(),
    };
    let line = row![
        text(truncate(&game.title, 60))
            .size(theme::text_size::BODY_LARGE)
            .font(if lit { theme::MEDIUM } else { theme::REGULAR })
            .color(if lit { theme::TEXT_PRIMARY } else { theme::TEXT_BODY })
            .wrapping(Wrapping::None)
            .width(Length::Fill),
        text(badge).size(theme::text_size::LABEL_MEDIUM).font(theme::BOLD).color(theme::ACCENT),
        text(library.system_name(&game.system_id))
            .size(theme::text_size::LABEL_LARGE)
            .color(theme::TEXT_SECONDARY)
            .wrapping(Wrapping::None),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    button(container(line).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 14])
        .on_press(Message::SearchResultClicked(index))
        .style(crate::ui::style::row(lit, 10.0))
        .into()
}

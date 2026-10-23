//! The Search page — a port of `browse/ui/SearchView.kt`: an on-screen keyboard beside results from
//! every system.
//!
//! Results are a list, never a grid: they come from systems whose box art has different shapes, and
//! a grid of mixed ratios has no rhythm to scan. Each row says which system it is from and whether
//! the game is on this device, the two things a cross-system list loses that a grid would show.
//!
//! Drawn as Home and Settings are, on Home's ground: the query over the keyboard at the left, as
//! glass, the key under the cursor filled with the ink; the results at the right as glass rows with
//! their covers, the one under the cursor lifted on the ink ring.

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::covers::Covers;
use crate::downloads::{Downloads, SyncState};
use crate::library::{Game, Library};
use crate::search::{BACKSPACE, ROWS, SearchFocus};
use crate::theme::{self, alpha, glass};
use crate::ui::home::truncate;

/// A result row's height and the gap under it — fixed, so the cursor's scroll offset is computed.
const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 8.0;
const KEY_W: f32 = 44.0;
const KEY_H: f32 = 44.0;
const KEY_GAP: f32 = 8.0;
/// The cover at a result's left.
const COVER: (f32, f32) = (34.0, 48.0);

pub fn results_id() -> widget::Id {
    widget::Id::new("search-results")
}

/// Where the results list scrolls for the cursor on `index`: that row a quarter of the list down.
pub fn scroll_offset(index: usize, list_h: f32) -> f32 {
    (index as f32 * (ROW_H + ROW_GAP) - list_h / 4.0).max(0.0)
}

pub fn search_view<'a>(focus: &SearchFocus, results: &[&'a Game], library: &'a Library, downloads: &Downloads, covers: &Covers) -> Element<'a, Message> {
    let margin = crate::ui::tab_bar::MARGIN;
    let widest = ROWS.iter().map(|r| r.chars().count()).max().unwrap_or(1) as f32;
    let keyboard_w = widest * KEY_W + (widest - 1.0) * KEY_GAP;

    let left = column![
        row![
            crate::ui::home::tag("SEARCH".into(), true),
            text("EVERY SYSTEM, BY TITLE").size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        query(focus),
        keyboard(focus),
    ]
    .spacing(18)
    .width(Length::Fixed(keyboard_w));

    let right: Element<Message> = if results.is_empty() {
        let (headline, under) = if focus.query.trim().is_empty() {
            ("Find any game".to_string(), "Every system, by title — ff7 and sotc work too.".to_string())
        } else {
            ("No matches".to_string(), format!("Nothing in the library goes by “{}”.", focus.query.trim()))
        };
        column![text(headline).size(32.0).font(theme::DISPLAY).color(theme::INK), text(under).size(16.0).color(alpha(theme::INK, 0.82))]
            .spacing(8)
            .padding(Padding { top: 44.0, ..Padding::ZERO })
            .into()
    } else {
        let cursor = focus.result(results.len());
        let mut list = column![].spacing(ROW_GAP);
        for (i, game) in results.iter().enumerate() {
            list = list.push(result_row(i, game, library, downloads, covers, focus.in_results && i == cursor));
        }
        let count = if results.len() == 1 { "1 RESULT".to_string() } else { format!("{} RESULTS", results.len()) };
        column![
            container(text(count).size(12.0).font(theme::BOLD).color(alpha(theme::INK, 0.6))).height(Length::Fixed(26.0)).align_y(iced::alignment::Vertical::Center),
            // Room under the last row, so it scrolls up to the same anchor as the rest.
            scrollable(column![list, Space::new().height(Length::Fixed(160.0))].padding(Padding { right: 14.0, ..Padding::ZERO }))
                .id(results_id())
                .on_scroll(|_| Message::Scrolled)
                .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .spacing(10)
        .into()
    };

    let hints = if focus.in_results {
        vec![("A", "Open the game".to_string()), ("B", "Back".to_string())]
    } else {
        vec![("A", "Type the key".to_string()), ("X", "Erase".to_string()), ("B", "Back".to_string())]
    };
    column![
        row![left, container(right).width(Length::Fill).height(Length::Fill)]
            .spacing(32)
            .padding(Padding { top: 18.0, right: margin, bottom: 0.0, left: margin })
            .height(Length::Fill),
        crate::ui::tab_bar::hint_line(hints),
    ]
    .into()
}

/// What is typed, in glass with the accent edge of the field being typed into.
fn query<'a>(focus: &SearchFocus) -> Element<'a, Message> {
    let words: Element<Message> = if focus.query.is_empty() {
        row![
            text("▌").size(26.0).color(theme::ACCENT_400),
            text("Type a title").size(22.0).color(alpha(theme::INK, 0.4)),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    } else {
        text(format!("{}▌", focus.query)).size(26.0).font(theme::DISPLAY).color(theme::INK).wrapping(Wrapping::None).into()
    };
    container(words)
        .width(Length::Fill)
        .height(Length::Fixed(60.0))
        .padding([0, 18])
        .align_y(iced::alignment::Vertical::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { color: theme::ACCENT_400, width: 2.0, radius: 14.0.into() },
            ..Default::default()
        })
        .into()
}

/// The keyboard, one rectangle: every row as wide as the widest, its keys stretched to fill.
fn keyboard<'a>(focus: &SearchFocus) -> Element<'a, Message> {
    let mut rows = column![].spacing(KEY_GAP);
    for (r, keys) in ROWS.iter().enumerate() {
        let mut line = row![].spacing(KEY_GAP).width(Length::Fill);
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

/// A result: its cover, its name and system, and what this device has of it.
fn result_row<'a>(index: usize, game: &'a Game, library: &'a Library, downloads: &Downloads, covers: &Covers, lit: bool) -> Element<'a, Message> {
    let (w, h) = COVER;
    let cover: Element<Message> = match game.cover_url.as_deref().and_then(|url| covers.handle(url)) {
        Some(handle) => image(handle.clone()).content_fit(ContentFit::Cover).width(Length::Fixed(w)).height(Length::Fixed(h)).border_radius(5.0).into(),
        None => container(Space::new()).width(Length::Fixed(w)).height(Length::Fixed(h)).style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { radius: 5.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into(),
    };
    let mark = match downloads.state_of(game.builds()) {
        SyncState::Installed => Some(("ON THIS DEVICE".to_string(), theme::ACCENT_400)),
        SyncState::UpdateAvailable => Some(("UPDATE".to_string(), theme::WARNING)),
        SyncState::Downloading(p) | SyncState::Unpacking(p) => Some((format!("{}%", (p * 100.0) as i32), theme::ACCENT_400)),
        SyncState::Failed(_) => Some(("FAILED".to_string(), theme::ERROR)),
        SyncState::RemoteOnly => None,
    };
    let mut line = row![
        cover,
        column![
            text(truncate(&game.title, 56)).size(16.0).font(theme::SEMIBOLD).color(theme::INK).wrapping(Wrapping::None),
            text(library.system_name(&game.system_id)).size(13.0).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None),
        ]
        .spacing(3)
        .width(Length::Fill),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    if let Some((words, tone)) = mark {
        line = line.push(
            container(text(words).size(11.0).font(theme::BOLD).color(tone)).height(Length::Fixed(24.0)).padding([0, 10]).center_y(Length::Fixed(24.0)).style(move |_| {
                container::Style {
                    background: Some(Background::Color(alpha(tone, 0.12))),
                    border: Border { color: alpha(tone, 0.3), width: 1.0, radius: 12.0.into() },
                    ..Default::default()
                }
            }),
        );
    }

    button(container(line).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 14])
        .on_press(Message::SearchResultClicked(index))
        .style(crate::ui::style::glass_row(lit, 14.0))
        .into()
}

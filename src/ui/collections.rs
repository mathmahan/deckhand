//! The Collections page — a port of the Kotlin app's `CollectionsPageView`: every collection RomM has
//! for this library, by kind, as a card grid. Picking one opens it as a Browse grid spanning every
//! system it touches.
//!
//! The kinds are chips across the top — favourites, the player's own, smart ones, then what RomM
//! generates from its metadata: genres, franchises, series, companies, game modes. Each card is a
//! collection, shown with its best-rated game's cover and how many of its games this library has —
//! except under favourites, where each card is a favourite game and opens its page.

use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::collections::{CollectionsFocus, Kind, Zone};
use crate::covers::Covers;
use crate::focus::CARD_COLS;
use crate::theme;
use crate::ui::home::truncate;

/// A card's height, and the gap between them. Uniform: these are chrome, not box art.
const CARD_H: f32 = 214.0;
const CARD_GAP: f32 = 14.0;
/// How tall a card's cover is drawn — and so the size its art is fetched at.
pub const COVER_H: f32 = 120.0;

pub fn page_id() -> widget::Id {
    widget::Id::new("collections-page")
}

/// How far down the grid the row holding card `index` starts, for scrolling the cursor into view.
pub fn row_offset(index: usize) -> f32 {
    (index / CARD_COLS) as f32 * (CARD_H + CARD_GAP)
}

/// The row of cards a scroll offset puts at the top.
pub fn row_at(offset: f32) -> usize {
    (offset / (CARD_H + CARD_GAP)).floor().max(0.0) as usize
}

/// The cards worth having covers for at a scroll offset, in a page `height` tall: the rows on
/// screen, one above and two below. The height is the window's, which over-states the grid's — the
/// safe direction, a spare row rather than a band of empty cards.
pub fn cards_in_view(offset: f32, height: f32) -> std::ops::Range<usize> {
    let first = row_at(offset).saturating_sub(1);
    let rows = (height / (CARD_H + CARD_GAP)).ceil() as usize + 3;
    first * CARD_COLS..(first + rows) * CARD_COLS
}

/// One card: a collection, or under favourites a game.
pub struct Card<'a> {
    pub name: &'a str,
    /// Under the name: how many games a collection has here, or a game's system.
    pub note: String,
    pub cover_url: Option<&'a str>,
    /// What pressing it does.
    pub open: Message,
}

/// A collection card's note.
pub fn games(count: usize) -> String {
    if count == 1 { "1 game".to_string() } else { format!("{count} games") }
}

pub struct CollectionsContext<'a> {
    pub kinds: Vec<Kind>,
    /// The current kind's cards.
    pub cards: Vec<Card<'a>>,
    pub focus: &'a CollectionsFocus,
    pub covers: &'a Covers,
    /// Said under the heading when RomM's smart and generated collections are still on their way,
    /// or could not be read.
    pub note: Option<String>,
}

pub fn collections_view<'a>(ctx: &CollectionsContext<'a>) -> Element<'a, Message> {
    let mut heading = column![text("Collections").size(theme::text_size::HEADLINE_SMALL).font(theme::MEDIUM).color(theme::TEXT_PRIMARY)];
    if let Some(note) = &ctx.note {
        heading = heading.push(Space::new().height(Length::Fixed(4.0)));
        heading = heading.push(text(note.clone()).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_MUTED));
    }

    if ctx.kinds.is_empty() {
        let empty = text(
            "No collections yet — favourite a game and it is listed here. RomM builds genres, franchises and more \
             from its metadata once a scan has matched your games, and your own appear here as soon as they hold \
             a game in this library.",
        )
        .size(theme::text_size::BODY_LARGE)
        .color(theme::TEXT_SECONDARY);
        return column![heading, empty].spacing(16).padding(Padding { top: 20.0, right: 28.0, bottom: 0.0, left: 28.0 }).into();
    }

    let mut chips = row![].spacing(10).align_y(Alignment::Center);
    for (i, kind) in ctx.kinds.iter().enumerate() {
        let active = i == ctx.focus.kind;
        let lit = active && ctx.focus.zone == Zone::Kinds;
        chips = chips.push(
            button(text(kind.label()).size(theme::text_size::LABEL_LARGE).font(if active { theme::SEMIBOLD } else { theme::MEDIUM }))
                .padding([6, 14])
                .on_press(Message::CollectionKindPicked(i))
                .style(crate::ui::style::chip(active, lit)),
        );
    }

    // A plain wrapping grid of fixed-portion cards, padded on a short last row so its cards keep a
    // full row's width rather than stretching across the page.
    let cursor = (ctx.focus.zone == Zone::Cards).then(|| ctx.focus.cards.index());
    let mut grid = column![].spacing(CARD_GAP);
    for (row_index, chunk) in ctx.cards.chunks(CARD_COLS).enumerate() {
        let mut cards = row![].spacing(CARD_GAP);
        for (col, item) in chunk.iter().enumerate() {
            let i = row_index * CARD_COLS + col;
            cards = cards.push(card(item, cursor == Some(i), ctx.covers));
        }
        for _ in chunk.len()..CARD_COLS {
            cards = cards.push(Space::new().width(Length::FillPortion(1)));
        }
        grid = grid.push(cards);
    }

    let page = column![grid, Space::new().height(Length::Fixed(24.0))].padding([0, 28]);
    column![
        column![heading, chips].spacing(14).padding(Padding { top: 20.0, right: 28.0, bottom: 16.0, left: 28.0 }),
        scrollable(page)
            .id(page_id())
            .on_scroll(|viewport| Message::CollectionsScrolled(viewport.absolute_offset().y))
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .into()
}

/// One card: its cover, its name, and its note.
fn card<'a>(item: &Card, focused: bool, covers: &Covers) -> Element<'a, Message> {
    let cover: Element<Message> = match item.cover_url.and_then(|u| covers.handle(u)) {
        Some(handle) => image(handle.clone()).content_fit(ContentFit::Contain).height(Length::Fixed(COVER_H)).into(),
        None => Space::new().height(Length::Fixed(COVER_H)).into(),
    };
    let body = column![
        container(cover).width(Length::Fill).height(Length::Fixed(COVER_H)).center_x(Length::Fill),
        Space::new().height(Length::Fixed(10.0)),
        text(truncate(item.name, 34))
            .size(theme::text_size::BODY_MEDIUM)
            .font(theme::MEDIUM)
            .color(if focused { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY })
            .align_x(Alignment::Center),
        Space::new().height(Length::Fixed(4.0)),
        text(item.note.clone())
            .size(theme::text_size::LABEL_LARGE)
            .color(if focused { theme::ACCENT } else { theme::TEXT_MUTED }),
    ]
    .align_x(Alignment::Center);

    button(container(body).width(Length::Fill).center_y(Length::Fixed(CARD_H)).padding(12))
        .width(Length::FillPortion(1))
        .padding(0)
        .on_press(item.open.clone())
        .style(crate::ui::style::row(focused, 12.0))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_asked_for_around_what_is_on_screen() {
        assert_eq!(cards_in_view(0.0, 800.0), 0..(4 + 3) * CARD_COLS, "the top: nothing above to keep");
        let deep = row_offset(50 * CARD_COLS);
        let window = cards_in_view(deep, 800.0);
        assert_eq!(window.start, 49 * CARD_COLS, "a row above the first on screen");
        assert!(window.contains(&(53 * CARD_COLS)) && !window.contains(&(60 * CARD_COLS)));
    }
}

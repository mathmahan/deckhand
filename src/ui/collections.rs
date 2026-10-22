//! The Collections page — a port of the Kotlin app's `CollectionsPageView`: every collection RomM has
//! for this library, by kind, as a card grid. Picking one opens it as a Browse grid spanning every
//! system it touches.
//!
//! Drawn as Home and the Library are. The kinds sit on a glass strip under the header, walked with the
//! triggers as the Library's systems are — favourites, the player's own, smart ones, then what RomM
//! generates from its metadata: genres, franchises, series, companies, game modes. Under them, the
//! kind's heading where Home has its spotlight, then its cards on glass. Each card is a collection,
//! shown with its best-rated game's cover and how many of its games this library has — except under
//! favourites, where each card is a favourite game and opens its page.

use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::collections::{CollectionsFocus, Kind, Zone};
use crate::covers::Covers;
use crate::focus::CARD_COLS;
use crate::theme::{self, alpha};
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
    let margin = crate::ui::tab_bar::MARGIN;
    let Some(kind) = ctx.kinds.get(ctx.focus.kind).copied() else {
        let empty = text(
            "No collections yet — favourite a game and it is listed here. RomM builds genres, franchises and more \
             from its metadata once a scan has matched your games, and your own appear here as soon as they hold \
             a game in this library.",
        )
        .size(17.0)
        .color(alpha(theme::INK, 0.76));
        return column![heading_of("Collections".into(), String::new(), ctx.note.clone()), empty]
            .spacing(20)
            .padding(Padding { top: 30.0, right: margin, bottom: 0.0, left: margin })
            .into();
    };

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
    // Room either side of the cards for the cursor's ring and its shadow, inside the page's margin.
    let page = column![grid, Space::new().height(Length::Fixed(24.0))].padding([0.0, margin]);

    let count = match kind {
        Kind::Favourites => games(ctx.cards.len()),
        _ if ctx.cards.len() == 1 => "1 collection".to_string(),
        _ => format!("{} collections", ctx.cards.len()),
    };
    let title = format!("{}{}", kind.label()[..1].to_uppercase(), &kind.label()[1..]);
    let heading = heading_of(title, format!("{}  ·  {}", count, description(kind)), ctx.note.clone());

    column![
        container(kinds(ctx)).padding(Padding { top: 8.0, right: margin, bottom: 0.0, left: margin }),
        container(heading).padding(Padding { top: 22.0, right: margin, bottom: 20.0, left: margin }),
        scrollable(page)
            .id(page_id())
            .on_scroll(|viewport| Message::CollectionsScrolled(viewport.absolute_offset().y))
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
            .width(Length::Fill)
            .height(Length::Fill),
        crate::ui::tab_bar::hint_line(vec![("A", if ctx.focus.zone == Zone::Kinds { "Choose" } else { "Open" }.to_string())]),
    ]
    .into()
}

/// The kinds, on the glass the Library's filters stand in, with the triggers that walk them. The one
/// shown is filled with the ink; with the cursor up on the strip, it is ringed as well.
fn kinds<'a>(ctx: &CollectionsContext<'a>) -> Element<'a, Message> {
    use crate::ui::browse::{chip, key, strip};
    let lit = ctx.focus.zone == Zone::Kinds;
    let mut pills = row![container(key("L2")).padding([0, 4])].spacing(4).align_y(Alignment::Center);
    for (i, kind) in ctx.kinds.iter().enumerate() {
        let shown = i == ctx.focus.kind;
        pills = pills.push(chip(kind.label().to_string(), shown, lit && shown, Message::CollectionKindPicked(i)));
    }
    strip(pills.push(container(key("R2")).padding([0, 4])))
}

/// The page's heading, where Home has its spotlight: its tag, the kind's name, how many it holds and
/// what they are, and — while RomM's are still on their way — a note saying so.
fn heading_of<'a>(title: String, about: String, note: Option<String>) -> Element<'a, Message> {
    let mut heading = column![
        crate::ui::home::tag("COLLECTIONS".into(), true),
        Space::new().height(Length::Fixed(12.0)),
        text(title).size(40.0).font(theme::DISPLAY).color(theme::INK),
    ];
    if !about.is_empty() {
        heading = heading.push(Space::new().height(Length::Fixed(8.0))).push(text(about).size(16.0).color(alpha(theme::INK, 0.82)));
    }
    if let Some(note) = note {
        heading = heading.push(Space::new().height(Length::Fixed(6.0))).push(text(note).size(13.0).color(theme::TEXT_SECONDARY));
    }
    heading.into()
}

/// What a kind's collections are, in a line under its name.
fn description(kind: Kind) -> &'static str {
    match kind {
        Kind::Favourites => "the games you marked, here or in RomM",
        Kind::Yours => "the ones you made in RomM",
        Kind::Smart => "filled by the rules you gave them in RomM",
        Kind::Genre => "as RomM's metadata files your games",
        Kind::Franchise => "the franchises your games belong to",
        Kind::Series => "series of games, as IGDB groups them",
        Kind::Company => "who made and published your games",
        Kind::Mode => "single player, co-op, split screen and the rest",
    }
}

/// One card, on glass: its cover on the page's ground, its name, and its note. The cursor's is
/// lifted on a ring of the brightest ink, as a Settings row is.
fn card<'a>(item: &Card, focused: bool, covers: &Covers) -> Element<'a, Message> {
    let cover: Element<Message> = match item.cover_url.and_then(|u| covers.handle(u)) {
        Some(handle) => image(handle.clone()).content_fit(ContentFit::Contain).height(Length::Fixed(COVER_H)).into(),
        None => Space::new().height(Length::Fixed(COVER_H)).into(),
    };
    let body = column![
        container(cover).width(Length::Fill).height(Length::Fixed(COVER_H)).center_x(Length::Fill),
        Space::new().height(Length::Fixed(12.0)),
        text(truncate(item.name, 34)).size(15.0).font(theme::SEMIBOLD).color(theme::INK).align_x(Alignment::Center),
        Space::new().height(Length::Fixed(4.0)),
        text(item.note.clone()).size(13.0).color(if focused { theme::ACCENT_400 } else { alpha(theme::INK, 0.6) }),
    ]
    .align_x(Alignment::Center);
    button(container(body).width(Length::Fill).center_y(Length::Fixed(CARD_H)).padding(12))
        .width(Length::FillPortion(1))
        .padding(0)
        .on_press(item.open.clone())
        .style(crate::ui::style::glass_row(focused, 18.0))
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

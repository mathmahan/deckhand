//! The Browse page — one system's library as a cover grid, drawn in Home's look.
//!
//! The system list down the left edge on a glass panel, and beside it, top to bottom: the game under
//! the cursor in a spotlight as on Home's, over its screenshot; the filter and sort pills; and the
//! grid, with the A-Z rail pinned to its right edge. The list and the rail are part of the width
//! budget rather than overlays, because the grid geometry in [`crate::grid`] is only meaningful if
//! the width it divides up is the width tiles actually get.

use iced::widget::text::LineHeight;
use iced::widget::{self as widget, button, column, container, image, mouse_area, row, scrollable, text, Space};
use iced::{alignment, Alignment, Background, Border, Color, ContentFit, Element, Length, Padding, Shadow, Vector};

use iced_fonts::lucide;

use crate::app::Message;
use crate::covers::Covers;
use crate::focus::{BrowseFocus, BrowseZone};
use crate::grid::{self, GridSpec};
use crate::collections::Scope;
use crate::library::Game;
use crate::theme::{self, alpha, glass};
use crate::ui::home;
use crate::view_model::{Filter, Sort, SystemView};

/// How far an unfocused tile is dimmed.
///
/// Lighter than Home's shelf dim: a grid is a wall of covers you are scanning, so dropping the
/// unfocused ones as far as a shelf does makes the whole page read as switched off. It only has to
/// be enough that the focused tile is obviously the bright one.
const UNFOCUSED_DIM: f32 = 0.72;

/// How far a cover's shape may stray from its tile's and still be cropped to fill it: 6%, a crop of
/// a few pixels, whose spill on a renderer that does not clip stays inside the gap between tiles.
const STRAY_TOLERANCE: f32 = 0.06;

/// Whether art of shape `art_ratio` is drawn whole in a box of `box_ratio` rather than cropped to
/// fill it — see [`STRAY_TOLERANCE`].
pub fn is_stray(art_ratio: f32, box_ratio: f32) -> bool {
    box_ratio > 0.0 && (art_ratio / box_ratio - 1.0).abs() > STRAY_TOLERANCE
}

pub fn grid_id() -> widget::Id {
    widget::Id::new("browse-grid")
}

pub fn system_list_id() -> widget::Id {
    widget::Id::new("browse-systems")
}

/// A row of the system list, and the gap under it. Fixed, so where a row sits in the list is
/// computed rather than measured — see [`system_row_span`].
const SYSTEM_ROW_H: f32 = 38.0;
const SYSTEM_ROW_GAP: f32 = 2.0;

/// Distance between the tops of two list rows.
pub const SYSTEM_ROW_PITCH: f32 = SYSTEM_ROW_H + SYSTEM_ROW_GAP;

/// The space above the list's first row, under its heading.
const LIST_TOP: f32 = 2.0;

/// The list's heading, above the rows and outside their scroll.
const LIST_HEADING_H: f32 = 44.0;

/// The panel the list stands on: under the header, and short of the window's bottom edge.
const PANEL_TOP: f32 = 12.0;
const PANEL_BOTTOM: f32 = 24.0;

/// How wide a system's name may run: the panel, less its scrollbar strip (8), the list's padding
/// (18), a row's own padding (24), the platform icon and the gaps either side of the name (40), and
/// the game count (24).
const SYSTEM_NAME_W: f32 = grid::SYSTEM_LIST_W - 114.0;

/// The platform icon's box in the system list: as tall as the text beside it reads well against,
/// and as wide as the widest icons RomM ships (a console drawn side on).
pub const LOGO_H: f32 = 22.0;
const LOGO_W: f32 = 24.0;

/// The spotlight over the grid, from under the header to the chips. Everything in it has a fixed
/// height, so walking the grid changes what it says without moving the grid under it.
pub const HERO_H: f32 = 196.0;

/// The page's height, under the header, below which the spotlight is left out: in a short window
/// the grid keeps the room.
pub const HERO_BELOW: f32 = 560.0;

/// The spotlight's one-line name.
pub const HERO_TITLE_SIZE: f32 = 40.0;
pub const HERO_TITLE_LINE: f32 = 44.0;

/// The filter and sort pills' row, with the room above and below it.
pub const CHIPS_H: f32 = 72.0;

/// How wide the spotlight's name may run. Short of Home's, since the text starts right of the
/// system list and so further over the picture.
pub const HERO_TITLE_W: f32 = 520.0;

/// How the page darkens left to right behind the spotlight's text — see [`home::SHADE`]. Further
/// across than Home's, since the text starts right of the system list.
pub const SHADE: [(f32, f32); 4] = [(0.0, 1.0), (0.42, 0.94), (0.62, 0.6), (0.86, 0.0)];

/// Where system `index`'s row sits in the list, top and bottom, for scrolling it into view.
pub fn system_row_span(index: usize) -> (f32, f32) {
    let top = LIST_TOP + index as f32 * SYSTEM_ROW_PITCH;
    (top, top + SYSTEM_ROW_H)
}

/// How far down the grid row `row` starts. Every row is one pitch, whatever it holds.
pub fn row_top(spec: &GridSpec, row: usize) -> f32 {
    row as f32 * spec.pitch
}

/// Where the grid starts, from the top of the window: under the header, the spotlight and the
/// chips. What the picture behind the spotlight fades out before.
pub fn grid_top() -> f32 {
    crate::ui::tab_bar::TAB_BAR_HEIGHT + HERO_H + CHIPS_H
}

/// One entry of the list down Browse's left edge: a system, or — on a collection's grid — another
/// collection of the same kind, so the list steps between collections the way it does systems.
pub struct Place {
    pub scope: Scope,
    pub name: String,
    pub count: usize,
    /// RomM's icon for a platform, by URL; None for a collection.
    pub logo: Option<String>,
}

/// The game under the grid's cursor, as the spotlight shows it.
pub struct Hero<'a> {
    pub game: &'a Game,
    /// Its name, cut to the spotlight's one line — measured by the app, which keeps the answer.
    pub title: String,
    /// How many of its achievements this user has earned, when RomM said.
    pub earned: Option<usize>,
    /// Its system, named where the grid holds more than one: a collection's.
    pub system: Option<&'a str>,
    /// For dates: a game played this year says "1 Oct", one played before says the year too.
    pub this_year: i64,
}

pub struct BrowseContext<'a> {
    /// The list's entries, and which of them the grid is showing.
    pub places: Vec<Place>,
    /// What the list holds, over it: the systems, or a collection's kind.
    pub heading: String,
    pub scope: &'a Scope,
    /// Owned rather than borrowed: the view is built per frame from the library, and the elements
    /// below borrow the `&'a Game`s *inside* it. Holding it by reference would tie the returned
    /// element to a local that dies at the end of the call.
    pub view: SystemView<'a>,
    /// The spotlight, unless the window is too short for it — see [`HERO_BELOW`] — or the grid is
    /// empty.
    pub hero: Option<Hero<'a>>,
    pub focus: &'a BrowseFocus,
    pub covers: &'a Covers,
    /// What this device has of each game, for the tiles' corner badges — see
    /// [`crate::downloads`].
    pub downloads: &'a crate::downloads::Downloads,
    pub spec: GridSpec,
    /// Which tiles share a row, and how wide each is — see [`crate::grid::Packing`].
    pub packing: crate::grid::Packing,
    /// The rows worth building — see [`crate::grid::rows_in_view`]. Everything outside it is
    /// replaced by a spacer of exactly the height those rows would have taken, so the grid's total
    /// height, its scroll geometry and [`row_top`] are all unchanged by the window moving.
    pub rows: std::ops::Range<usize>,
    pub filter: Filter,
    pub sort: Sort,
    /// Chip counts, so each chip states how much it would leave you with before you press it.
    pub counts: ChipCounts,
    /// The focus cross-fade, and the instant to read it at — see [`crate::animation`].
    pub fade: &'a crate::animation::FocusFade,
    pub now: std::time::Instant,
    /// The card under the mouse pointer, if any.
    pub hovered: Option<crate::animation::Cursor>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChipCounts {
    pub all: usize,
    pub installed: usize,
    pub unplayed: usize,
    pub favourites: usize,
}

impl ChipCounts {
    fn of(&self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.all,
            Filter::Installed => self.installed,
            Filter::Unplayed => self.unplayed,
            Filter::Favourites => self.favourites,
        }
    }
}

pub fn browse_view<'a>(ctx: BrowseContext<'a>) -> Element<'a, Message> {
    let grid_focused = ctx.focus.zone() == BrowseZone::Grid;

    let empty = ctx.view.is_empty();
    let systems = system_list(&ctx);
    let hero = ctx.hero.as_ref().map(|hero| spotlight(hero, ctx.downloads));
    let chips = chips_row(&ctx);
    let rail = letter_rail(&ctx);
    // Consumes the context, so the tiles can hold the `&'a Game`s out of its view.
    let grid = grid_column(ctx, grid_focused);

    // The rail is pointless beside an empty grid — there is nothing to jump to.
    let body = if empty { row![grid] } else { row![grid, rail] };
    let page = match hero {
        Some(hero) => column![hero, chips, body],
        None => column![chips, body],
    };
    let page = page.width(Length::Fill).height(Length::Fill);
    row![systems, page]
        .spacing(grid::LIST_GAP)
        .padding(Padding { left: grid::LIST_LEFT, right: grid::RAIL_RIGHT, ..Padding::ZERO })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The system list: every system down the left edge, each with how many games it holds, on a
/// panel of the glass the header's pills are cut from.
///
/// A column rather than a row of pills across the top, because it has to hold anything from a
/// handful of systems to the 200-odd RomM supports: a column shows three times as many at once,
/// scrolls the way a pad and a mouse wheel both already move, and never cuts a name off at the
/// window's edge.
fn system_list<'a>(ctx: &BrowseContext<'a>) -> Element<'a, Message> {
    let cursor = (ctx.focus.zone() == BrowseZone::SystemList).then_some(ctx.focus.system);
    let mut list = column![].spacing(SYSTEM_ROW_GAP);
    for (i, place) in ctx.places.iter().enumerate() {
        let logo = place.logo.as_deref().and_then(|url| ctx.covers.handle(url)).cloned();
        list = list.push(system_row(place, logo, place.scope == *ctx.scope, cursor == Some(i)));
    }

    // The triggers step through the list from anywhere on the page: said beside its name.
    let heading = row![
        text(ctx.heading.clone()).size(12.0).font(theme::BOLD).color(theme::TEXT_SECONDARY).width(Length::Fill),
        key("L2"),
        key("R2"),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .height(Length::Fixed(LIST_HEADING_H))
    .padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 22.0 });

    let rows = scrollable(list.padding(Padding { top: LIST_TOP, right: 6.0, bottom: 14.0, left: 12.0 }))
        .id(system_list_id())
        .on_scroll(|_| Message::Scrolled)
        // Given a strip of its own beside the rows, rather than drawn over their right edge
        // where the game counts are.
        .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).spacing(4)))
        .height(Length::Fill);

    let panel = container(column![heading, rows]).width(Length::Fill).height(Length::Fill).style(|_| container::Style {
        background: Some(Background::Color(glass(0.05))),
        border: Border { color: glass(0.10), width: 1.0, radius: 20.0.into() },
        ..Default::default()
    });
    container(panel)
        .width(Length::Fixed(grid::SYSTEM_LIST_W))
        .height(Length::Fill)
        .padding(Padding { top: PANEL_TOP, bottom: PANEL_BOTTOM, ..Padding::ZERO })
        .into()
}

/// A pad button with a name longer than a letter — a trigger — in a rounded box, set beside what it
/// does.
fn key<'a>(name: &'a str) -> Element<'a, Message> {
    container(text(name).size(10.0).font(theme::BOLD).color(alpha(theme::INK, 0.7)))
        .padding([2.0, 5.0])
        .style(|_| container::Style {
            border: Border { color: alpha(theme::INK, 0.55), width: 1.5, radius: 5.0.into() },
            ..Default::default()
        })
        .into()
}

/// One place in the list. `current` is the one open in the grid; `under_cursor` is where the list's
/// own cursor is while you walk it, which is a different place until you press Right or Enter.
///
/// The open one is filled with the brightest ink, as the header's current destination is; the
/// cursor is a ring of its own, so both show at once while you walk the list.
fn system_row<'a>(place: &Place, logo: Option<iced::widget::image::Handle>, current: bool, under_cursor: bool) -> Element<'a, Message> {
    let (key, open) = match &place.scope {
        Scope::System(id) => (id.as_str(), Message::OpenSystem(id.clone())),
        Scope::Collection(key) => (key.as_str(), Message::OpenCollection(key.clone())),
    };
    let hue = theme::system_hue(key);
    // RomM's own icon for the platform; the system's hue dot until it has arrived, for a platform
    // RomM has none for, and for a collection.
    let mark: Element<Message> = match logo {
        Some(handle) => image(handle).content_fit(ContentFit::Contain).width(Length::Fixed(LOGO_W)).height(Length::Fixed(LOGO_H)).into(),
        None => container(
            container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0))).style(move |_| container::Style {
                background: Some(Background::Color(hue)),
                border: Border { radius: 4.0.into(), ..Default::default() },
                ..Default::default()
            }),
        )
        .width(Length::Fixed(LOGO_W))
        .center_x(Length::Fixed(LOGO_W))
        .into(),
    };
    let name_chars = home::chars_across(SYSTEM_NAME_W, theme::text_size::LABEL_LARGE);

    button(
        row![
            mark,
            text(home::truncate(&place.name, name_chars))
                .size(theme::text_size::LABEL_LARGE)
                .font(if current { theme::SEMIBOLD } else { theme::MEDIUM })
                .wrapping(iced::widget::text::Wrapping::None)
                .width(Length::Fill),
            text(place.count.to_string()).size(13.0).color(if current { theme::NEUTRAL_700 } else { theme::NEUTRAL_500 }),
        ]
        .spacing(8)
        .height(Length::Fill)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(SYSTEM_ROW_H))
    .padding([0, 12])
    .on_press(open)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let fill = if current {
            Some(theme::INK)
        } else if under_cursor {
            Some(glass(0.08))
        } else if hovered {
            Some(glass(0.06))
        } else {
            None
        };
        button::Style {
            background: fill.map(Background::Color),
            text_color: if current {
                theme::GROUND
            } else if under_cursor {
                theme::INK
            } else {
                alpha(theme::INK, 0.82)
            },
            border: Border {
                // A ring of ink would vanish into the open row's fill; there the accent rings it.
                color: match (under_cursor, current) {
                    (true, true) => theme::ACCENT_400,
                    (true, false) => theme::INK,
                    (false, _) => Color::TRANSPARENT,
                },
                width: if under_cursor { 2.0 } else { 0.0 },
                radius: 12.0.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    })
    .into()
}

// ---------------------------------------------------------------------------------------------
// The spotlight
// ---------------------------------------------------------------------------------------------

/// The game under the grid's cursor, as Home's spotlight shows its own: where it is and when it was
/// played, its name, what it is, the way to its page, and what is worth knowing before pressing.
/// Over its screenshot, which the page under it draws — see [`home::stage`].
///
/// One line of name and no description: the grid needs the room more than the spotlight does.
fn spotlight<'a>(hero: &Hero<'a>, downloads: &crate::downloads::Downloads) -> Element<'a, Message> {
    let game = hero.game;
    let state = downloads.state_of(game.builds());
    let tag = if state.is_here() {
        home::tag("ON THIS DEVICE".into(), true)
    } else if state.is_busy() {
        home::tag("DOWNLOADING".into(), true)
    } else {
        home::tag("ON ROMM".into(), false)
    };
    let info = column![
        home::eyebrow_of(tag, hero.system, game, hero.this_year),
        Space::new().height(Length::Fixed(12.0)),
        text(hero.title.clone())
            .size(HERO_TITLE_SIZE)
            .font(theme::DISPLAY)
            .line_height(LineHeight::Absolute(HERO_TITLE_LINE.into()))
            .wrapping(iced::widget::text::Wrapping::None)
            .color(theme::INK)
            .height(Length::Fixed(HERO_TITLE_LINE)),
        Space::new().height(Length::Fixed(8.0)),
        home::meta_line(game),
        Space::new().height(Length::Fixed(16.0)),
        row![home::view_button(game), home::facts(game, &state, hero.earned)].spacing(24).align_y(Alignment::Center),
    ];
    container(info)
        .width(Length::Fill)
        .height(Length::Fixed(HERO_H))
        .padding([0.0, grid::GRID_PADDING])
        .align_y(alignment::Vertical::Bottom)
        .into()
}

// ---------------------------------------------------------------------------------------------
// The chips
// ---------------------------------------------------------------------------------------------

/// The filter pills, then the sort pills, each a glass strip as the header's destinations are. One
/// row, because they answer one question together: which games, in what order.
fn chips_row<'a>(ctx: &BrowseContext<'a>) -> Element<'a, Message> {
    let zone_lit = ctx.focus.zone() == BrowseZone::Chips;
    let cursor = ctx.focus.chip;

    let mut filters = row![].spacing(4).align_y(Alignment::Center);
    for (i, filter) in Filter::ALL.iter().enumerate() {
        // "all" carries no count: it is the absence of a filter, and "all 374" beside the 374 the
        // system list shows for this system is the same number twice.
        let label = match filter {
            Filter::All => filter.label().to_string(),
            other => format!("{} {}", other.label(), ctx.counts.of(*other)),
        };
        filters = filters.push(chip(label, ctx.filter == *filter, zone_lit && cursor == i, Message::FilterSelected(*filter)));
    }

    let mut sorts = row![container(lucide::arrow_up_down().size(14.0).color(theme::TEXT_SECONDARY)).padding([0.0, 6.0])]
        .spacing(4)
        .align_y(Alignment::Center);
    for (i, sort) in Sort::ALL.iter().enumerate() {
        sorts = sorts.push(chip(
            sort.label().to_string(),
            ctx.sort == *sort,
            zone_lit && cursor == Filter::ALL.len() + i,
            Message::SortSelected(*sort),
        ));
    }

    row![strip(filters), Space::new().width(Length::Fill), strip(sorts)]
        .align_y(Alignment::Center)
        .height(Length::Fixed(CHIPS_H))
        // Lined up with the grid's tiles, both ends: the rail stands to the right of them.
        .padding(Padding { top: 4.0, right: grid::GRID_PADDING + grid::ALPHA_RAIL_W, bottom: 0.0, left: grid::GRID_PADDING })
        .into()
}

/// A row of pills on the glass the header's pill is cut from.
fn strip<'a>(pills: iced::widget::Row<'a, Message>) -> Element<'a, Message> {
    container(pills)
        .padding(4)
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.05))),
            border: Border { color: glass(0.10), width: 1.0, radius: 999.0.into() },
            ..Default::default()
        })
        .into()
}

/// One pill: the chosen one filled with the brightest ink and its words gone dark, as the header's
/// current destination is. Under the pad's cursor a ring of the ink — or, on the ink's own fill,
/// of the accent.
fn chip<'a>(label: String, selected: bool, lit: bool, message: Message) -> Element<'a, Message> {
    button(container(text(label).size(14.0).font(if selected { theme::SEMIBOLD } else { theme::MEDIUM })).center_y(Length::Fill))
        .height(Length::Fixed(32.0))
        .padding([0, 14])
        .on_press(message)
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let fill = match (selected, lit || hovered) {
                (true, _) => Some(theme::INK),
                (false, true) => Some(glass(0.12)),
                (false, false) => None,
            };
            let ring = match (lit, selected) {
                (true, true) => theme::ACCENT_400,
                (true, false) => theme::INK,
                (false, _) => Color::TRANSPARENT,
            };
            button::Style {
                background: fill.map(Background::Color),
                text_color: if selected { theme::GROUND } else { alpha(theme::INK, 0.82) },
                border: Border { color: ring, width: if lit { 2.0 } else { 0.0 }, radius: 999.0.into() },
                shadow: Shadow::default(),
                snap: true,
            }
        })
        .into()
}

// ---------------------------------------------------------------------------------------------
// The grid
// ---------------------------------------------------------------------------------------------

fn grid_column<'a>(ctx: BrowseContext<'a>, grid_focused: bool) -> Element<'a, Message> {
    let (spec, focus_index, filter) = (ctx.spec, ctx.focus.index(), ctx.filter);
    let packing = ctx.packing;
    let rows_wanted = ctx.rows.clone();
    let (covers, downloads) = (ctx.covers, ctx.downloads);
    let (fade, now, hovered) = (ctx.fade, ctx.now, ctx.hovered);
    let empty = ctx.view.is_empty();
    let games = ctx.view.games;

    if empty {
        return container(
            text(match filter {
                Filter::Installed => "Nothing from this system is on this device yet.",
                Filter::Favourites => "No favourites in this system.",
                Filter::Unplayed => "Every game here has been played.",
                Filter::All => "This system has no games.",
            })
            .size(theme::text_size::BODY_LARGE)
            .color(theme::TEXT_SECONDARY),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();
    }

    let row_count = packing.rows().len();
    let rows = rows_wanted.start.min(row_count)..rows_wanted.end.min(row_count);

    // A row is pinned to exactly the height the pitch says it is, rather than left to settle on
    // whatever its tallest child measures. Two things depend on that being true rather than nearly
    // true: the spacers below stand in for rows and have to be the same height as the rows they
    // replace, and `row_top` scrolls by multiples of the pitch. Left natural, a row came out
    // ~1.2 units shorter than the pitch — invisible over one row, a row and a half of drift by the
    // bottom of a long grid.
    let row_h = spec.tile_h + grid::LABEL_H;
    let (above, below) = spacers(&spec, row_count, &rows);
    let mut grid = column![].spacing(grid::GAP);

    if above > 0.0 {
        grid = grid.push(Space::new().height(Length::Fixed(above)));
    }

    // Each row as the packing laid it out: every tile one height, each its own width, left-aligned
    // with a ragged right edge rather than covers cropped or stretched to fill a column.
    for row in &packing.rows()[rows] {
        let mut tiles = row![].spacing(grid::GAP).align_y(Alignment::Start).height(Length::Fixed(row_h));
        for i in row.clone() {
            let focused = grid_focused && focus_index == i;
            let key = crate::animation::Cursor::BrowseTile(i);
            let lit = fade.amount(key, focused, now);
            let size = (packing.width(i), spec.tile_h);
            tiles = tiles.push(tile(games[i], size, lit, key, hovered == Some(key), covers, downloads));
        }
        grid = grid.push(tiles);
    }

    if below > 0.0 {
        grid = grid.push(Space::new().height(Length::Fixed(below)));
    }

    scrollable(column![grid, Space::new().height(Length::Fixed(24.0))].padding([0, 16]))
        .id(grid_id())
        .on_scroll(|viewport| Message::GridScrolled(viewport.absolute_offset().y))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The height a grid of `row_count` rows lays out to: a pitch each, less the trailing gap.
///
/// Only the tests need to state this — the grid never builds all its rows — but it is the number
/// [`spacers`] exists to preserve, so it is written down next to it rather than inside them.
#[cfg(test)]
fn grid_height(spec: &GridSpec, row_count: usize) -> f32 {
    if row_count == 0 { 0.0 } else { row_count as f32 * spec.pitch - grid::GAP }
}

/// The two spacers that stand in for the rows outside the window: what goes above the first row
/// built, and what goes below the last.
///
/// Each is one gap short of the rows it replaces, because the grid column's own `spacing` supplies
/// the gap between a spacer and the row beside it. Getting this wrong would not misplace a tile
/// within its row — it would change the grid's *total* height, and every scroll offset in the page,
/// `row_top` included, is measured against that.
fn spacers(spec: &GridSpec, row_count: usize, rows: &std::ops::Range<usize>) -> (f32, f32) {
    let above = if rows.start > 0 { rows.start as f32 * spec.pitch - grid::GAP } else { 0.0 };
    let below = if rows.end < row_count {
        (row_count - rows.end) as f32 * spec.pitch - grid::GAP
    } else {
        0.0
    };
    (above.max(0.0), below.max(0.0))
}

/// One grid tile. `lit` is how focused it is: 1 under the cursor, 0 away from it, and part way
/// through a hand-off in between. `hovered` is the mouse pointer resting on it — see
/// [`crate::ui::home::hover_look`] for how the two are kept apart.
fn tile<'a>(
    game: &'a crate::library::Game,
    (w, h): (f32, f32),
    lit: f32,
    key: crate::animation::Cursor,
    hovered: bool,
    covers: &Covers,
    downloads: &crate::downloads::Downloads,
) -> Element<'a, Message> {
    use crate::animation::{lerp, mix};

    let (bright, rest) = home::hover_look(lit, hovered);

    // A tile is its game's *system's* cover shape — the packing gives a PS1 game a wider tile than a
    // PS2 one beside it — so a cover nearly always fills it, cropped by a hair at most. A scan of
    // another shape than its system's is drawn whole instead, centred on its own colour: cropping
    // it would rely on the renderer clipping the image to its tile, and iced 0.14's tiny-skia does
    // not, so the overflow painted across the neighbouring tiles.
    let meta = game.cover_url.as_deref().and_then(|u| covers.meta(u));
    let stray = meta.is_some_and(|m| is_stray(m.ratio, w / h.max(1.0)));
    let backdrop = match meta {
        Some(m) if stray => mix(theme::SURFACE_VARIANT, m.dominant, 0.35),
        _ => theme::SURFACE_VARIANT,
    };

    let art: Element<Message> = match game.cover_url.as_deref().and_then(|u| covers.handle(u)) {
        Some(handle) => image(handle.clone())
            .content_fit(if stray { ContentFit::Contain } else { ContentFit::Cover })
            .width(Length::Fixed(w - 2.0 * home::RING))
            .height(Length::Fixed(h - 2.0 * home::RING))
            .opacity(lerp(UNFOCUSED_DIM, 1.0, bright))
            // Inside its ring, as on Home's shelves — see `home::ring_of`.
            .border_radius(8.0 - home::RING)
            .into(),
        None => container(
            text(crate::ui::home::truncate(&game.title, 48))
                .size(13.0)
                .font(theme::EXTRABOLD)
                .color(theme::TEXT_BODY),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(10)
        .align_y(Alignment::End)
        .into(),
    };

    // Ringed in the brightest ink under the cursor, as Home's card is, and stood off the page by a
    // shadow that comes up with it.
    let ring = mix(rest, theme::INK, lit);
    let cover = container(home::ring_of(home::badged(art, &downloads.state_of(game.builds())), backdrop, ring))
        .width(Length::Fixed(w))
        .height(Length::Fixed(h))
        .clip(true)
        .style(move |_| container::Style {
            shadow: Shadow {
                color: alpha(Color::BLACK, 0.55 * lit),
                offset: Vector::new(0.0, lerp(4.0, 14.0, lit)),
                blur_radius: lerp(8.0, 32.0, lit),
            },
            border: Border { radius: 8.0.into(), ..Default::default() },
            ..Default::default()
        });

    let card = button(
        column![
            cover,
            Space::new().height(Length::Fixed(6.0)),
            text(home::truncate(&game.title, 22))
                .size(theme::text_size::BODY_MEDIUM)
                // Weight cannot be half a step, so it flips at the half-way point rather than
                // interpolating. The colour underneath it is what carries the fade.
                .font(if lit > 0.5 { theme::SEMIBOLD } else { theme::MEDIUM })
                .color(mix(theme::TEXT_BODY, theme::INK, bright)),
        ]
        .width(Length::Fixed(w)),
    )
    .padding(0)
    .on_press(Message::OpenDetail(game.rom_id))
    .style(crate::ui::style::bare);

    mouse_area(card).on_enter(Message::Hovered(key)).on_exit(Message::Unhovered(key)).into()
}

/// The A-Z rail: one entry per letter actually present in the current view.
///
/// Built from what is on screen rather than from the alphabet, so a letter is never a jump to a
/// game a filter has removed — and it disappears entirely under a non-alphabetical sort, where a
/// letter would land somewhere arbitrary.
fn letter_rail<'a>(ctx: &BrowseContext<'a>) -> Element<'a, Message> {
    let lit_zone = ctx.focus.zone() == BrowseZone::Rail;
    let mut rail = column![].align_x(Alignment::Center).spacing(1);

    for (i, (letter, target)) in ctx.view.letters.iter().enumerate() {
        let lit = lit_zone && ctx.focus.letter == i;
        rail = rail.push(
            button(container(text(letter.to_string()).size(11.0).font(theme::BOLD)).center_x(Length::Fill))
                .width(Length::Fixed(20.0))
                .padding([1, 0])
                .on_press(Message::JumpToLetter(*target))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    let fill = if lit {
                        Some(theme::INK)
                    } else if hovered {
                        Some(glass(0.12))
                    } else {
                        None
                    };
                    button::Style {
                        background: fill.map(Background::Color),
                        text_color: if lit {
                            theme::GROUND
                        } else if hovered {
                            theme::INK
                        } else {
                            alpha(theme::INK, 0.62)
                        },
                        border: Border { radius: 5.0.into(), ..Default::default() },
                        shadow: Shadow::default(),
                        snap: true,
                    }
                }),
        );
    }

    // On a strip of glass as tall as its letters, rather than the whole height beside the grid.
    let strip = container(rail).width(Length::Fixed(26.0)).padding([6, 0]).style(|_| container::Style {
        background: Some(Background::Color(glass(0.05))),
        border: Border { color: glass(0.10), width: 1.0, radius: 13.0.into() },
        ..Default::default()
    });
    container(scrollable(strip).on_scroll(|_| Message::Scrolled).height(Length::Fill))
        .width(Length::Fixed(grid::ALPHA_RAIL_W))
        .height(Length::Fill)
        .padding([0, 4])
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cover_of_another_systems_shape_is_drawn_whole_and_a_near_one_fills_its_tile() {
        assert!(is_stray(1.16, 0.67), "a PS1 jewel case among PSP boxes");
        assert!(is_stray(0.67, 1.37), "a tall box among wide SNES ones");
        assert!(!is_stray(0.70, 0.71), "a scan a hair off its system's shape is still cropped");
        assert!(!is_stray(1.0, 0.0), "no box, no judgement");
    }

    fn spec() -> GridSpec {
        crate::grid::grid_for(1.0, crate::grid::usable_width_for(1280.0), 1.0, 0)
    }

    /// What the column actually lays out to, given the spacers and the rows between them: each
    /// built row is a pitch less its gap, and `spacing` puts a gap between every pair of children.
    fn laid_out_height(spec: &GridSpec, row_count: usize, rows: std::ops::Range<usize>) -> f32 {
        let (above, below) = spacers(spec, row_count, &rows);
        let mut children: Vec<f32> = Vec::new();
        if above > 0.0 {
            children.push(above);
        }
        children.extend(std::iter::repeat_n(spec.tile_h + grid::LABEL_H, rows.len()));
        if below > 0.0 {
            children.push(below);
        }
        let gaps = grid::GAP * children.len().saturating_sub(1) as f32;
        children.iter().sum::<f32>() + gaps
    }

    #[test]
    fn a_windowed_grid_is_exactly_as_tall_as_the_whole_one() {
        // The property the whole thing rests on: which rows are built must not change the height of
        // the grid, or scrolling would fight the window — every offset the page scrolls to, and
        // every offset it reads back, is measured against that height.
        let spec = spec();
        let row_count = 200;
        let whole = grid_height(&spec, row_count);
        for rows in [0..9, 7..19, 50..60, 191..200, 0..200] {
            let windowed = laid_out_height(&spec, row_count, rows.clone());
            // A tenth of a unit, not zero: both sides are sums of f32s past 50 000, where the
            // representable step is already ~0.004. The layout accumulates the same way, and the
            // difference is four orders of magnitude below one pixel.
            assert!(
                (windowed - whole).abs() < 0.1,
                "window {rows:?} laid out {windowed}, whole grid is {whole}"
            );
        }
    }

    #[test]
    fn a_window_holding_no_rows_still_reserves_the_whole_grid() {
        // Reachable when a scroll lands past the rows a filter left behind, for the frame before
        // the cursor is re-seated. The grid has to keep its height through it rather than collapsing
        // and throwing the scroll position away.
        let spec = spec();
        for empty in [0..0, 200..200, 60..60] {
            let height = laid_out_height(&spec, 200, empty.clone());
            assert!(
                (height - grid_height(&spec, 200)).abs() < 0.1,
                "empty window {empty:?} collapsed the grid to {height}"
            );
        }
    }

    #[test]
    fn a_rows_top_is_where_row_top_says_it_is() {
        // The pinned row height is what makes this true: left to measure itself a row came out
        // ~1.2 units under the pitch, so `row_top` scrolled a little further than the row it
        // named — a row and a half of drift by the bottom of a thousand-game system.
        let spec = spec();
        let row_h = spec.tile_h + grid::LABEL_H;
        for row in [0usize, 1, 17, 199] {
            let top = row as f32 * (row_h + grid::GAP);
            assert!((top - row_top(&spec, row)).abs() < 0.01);
        }
    }
}

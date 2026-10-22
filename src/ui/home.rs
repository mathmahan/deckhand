//! Home: a spotlight over shelves of covers.
//!
//! The spotlight is whichever game the cursor is on: its logo, what it is, the start of what it is
//! about and what can be done with it, set over that game's own screenshot — faded into the page at
//! its edges, and blurred into a light that colours all the rest of it. Under it, the shelves: the
//! one with the cursor at the top of what is left of the screen, the next peeking up from the
//! bottom edge to say there is more, and the whole column sliding a shelf at a time as the cursor
//! moves between them.
//!
//! Nothing here leans on an effect a renderer may not have. The blur is a screenshot shrunk to a
//! thumbnail and stretched back over the window by the GPU; the fades are baked into the picture's
//! own alpha; the scrims are gradients; the lift of the card under the cursor is padding and its
//! glow a box shadow. So the page draws the same on the software renderer as on the GPU.

use std::sync::OnceLock;
use std::time::Instant;

use iced::widget::text::LineHeight;
use iced::widget::{
    self as widget, button, column, container, image, keyed_column, mouse_area, row, scrollable, stack, text, Space,
};
use iced::{
    alignment, Alignment, Background, Border, Color, ContentFit, Element, Font, Gradient, Length, Padding, Radians,
    Shadow, Vector,
};

use iced_fonts::lucide;

use crate::animation::{lerp, mix, Backdrop, Cursor, FocusFade, Glide};
use crate::app::Message;
use crate::covers::{self, Covers, Kind};
use crate::downloads::{human_bytes, Downloads, SyncState};
use crate::focus::HomeFocus;
use crate::library::{Game, Library, Shelf};
use crate::theme::{self, alpha};
use crate::ui::tab_bar::{glyph, MARGIN};

/// How tall a cover is on every shelf. One height for every shelf, so every shelf is one height —
/// which is what lets the column slide a whole shelf at a time.
pub const COVER_H: f32 = 144.0;

/// How far the card under the spotlight rises out of its shelf.
const LIFT: f32 = 8.0;

/// Gap between cards on a shelf.
const CARD_GAP: f32 = 20.0;

/// The spotlight's band, from under the header to the shelves. Its text sits on the bottom of it,
/// over the picture, and everything from the meta line down has a fixed height — so walking a shelf
/// changes what the band says without moving its button; only the tag over a shorter name moves.
pub const SPOTLIGHT_H: f32 = 400.0;

/// The spotlight's band in a window too short for all of it: the description and the facts go,
/// so the shelf with the cursor stays on screen. See [`HomeContext::compact`].
const COMPACT_H: f32 = 290.0;

/// The body height below which Home goes [`COMPACT_H`]: the full band, the gap and the shelf with
/// the cursor, with a little of the next one under it.
pub const COMPACT_BELOW: f32 = SPOTLIGHT_H + ROWS_GAP + ROW_TOP + COVER_H + ROW_BOTTOM + 40.0;

/// The spotlight's text column.
pub const SPOTLIGHT_W: f32 = 560.0;

/// The logo's box: as tall as a two-line title, and capped in width so a very wide wordmark does not
/// run under the picture.
const LOGO_H: f32 = 100.0;
const LOGO_W: f32 = 380.0;

/// The description's two lines.
pub const SUMMARY_SIZE: f32 = 15.0;
pub const SUMMARY_LINE: f32 = 22.0;

/// Between the spotlight and the window the shelves show through.
const ROWS_GAP: f32 = 20.0;

/// A shelf's heading, and the gap under it.
const HEADING_H: f32 = 28.0;
const HEADING_GAP: f32 = 2.0;

/// The room a shelf's row keeps above its covers and below them, for the lifted card and for the
/// shadows. The row is a scrollable, and a scrollable clips: without the room a shadow would stop
/// at a hard edge.
const ROW_TOP: f32 = 20.0;
const ROW_BOTTOM: f32 = 24.0;

/// Between one shelf's row and the next shelf's heading.
const SHELF_GAP: f32 = 16.0;

/// One shelf, heading to heading. Every shelf is this tall, so [`Glide`] can count in shelves.
pub const SHELF_PITCH: f32 = HEADING_H + HEADING_GAP + ROW_TOP + COVER_H + ROW_BOTTOM + SHELF_GAP;

/// The spotlight's picture: a box at the window's top right, under the header.
pub const PICTURE: (f32, f32) = (840.0, 560.0);

/// The cover card drawn in the picture's place for a game RomM has no screenshot of.
pub const CARD_H: f32 = 320.0;
const CARD_MAX_W: f32 = 440.0;

/// How far the cards the spotlight is not on are dimmed. Load-bearing: on a shelf of covers a
/// brightness step reads as "this one" from across a room, where a ring alone does not.
const UNFOCUSED_DIM: f32 = 0.6;

/// How much of the shelves' right edge fades out — a row that runs off the screen says so.
const EDGE_FADE: f32 = 140.0;

/// How much of the next card shows past the one the cursor is on before a shelf scrolls — so the
/// cursor walks across the cards already on screen, and the row only moves once it reaches the
/// edge, with a card's worth still showing that there is more.
pub const SHELF_PEEK: f32 = 72.0;

pub struct HomeContext<'a> {
    pub library: &'a Library,
    pub shelves: &'a [Shelf],
    pub focus: &'a HomeFocus,
    pub covers: &'a Covers,
    /// What this device has of each game. Live state rather than a property of the library
    /// snapshot, which is why it is threaded in here instead of sitting on [`Game`] — a download
    /// finishing has to change the spotlight's facts and a card's badge without rebuilding the
    /// library.
    pub downloads: &'a Downloads,
    /// The connected server, for resolving host-relative RomM asset paths.
    pub host: &'a str,
    /// The focus cross-fade, and the instant to read it — and the other two animations — at.
    pub fade: &'a FocusFade,
    pub glide: &'a Glide,
    pub backdrop: &'a Backdrop,
    /// Captured once per frame in `App::view` rather than per card, so every card on the page
    /// agrees about how far through a fade it is.
    pub now: Instant,
    /// The card under the mouse pointer, if any.
    pub hovered: Option<Cursor>,
    /// How many of the spotlit game's achievements this user has earned, when RomM said.
    pub earned: Option<usize>,
    /// The spotlit game's name and description, cut to the lines the spotlight gives them — see
    /// [`clamp_lines`]. Measured by the app, which keeps the answer, rather than on every frame.
    pub title: String,
    pub summary: String,
    /// For dates: a game played this year says "1 Oct", one played before says the year too.
    pub this_year: i64,
    /// The window is too short for the whole spotlight and a shelf under it — a small desktop
    /// window — so the spotlight drops its description and facts. See [`COMPACT_BELOW`].
    pub compact: bool,
}

impl<'a> HomeContext<'a> {
    /// Resolve a host-relative RomM asset path (`/assets/romm/resources/…`) against the server.
    fn asset_url(&self, path: &str) -> String {
        asset_url(self.host, path)
    }

    /// The spotlight band's height.
    fn spotlight_h(&self) -> f32 {
        if self.compact { COMPACT_H } else { SPOTLIGHT_H }
    }

    /// Where the window the shelves show through starts, from the top of the window.
    fn rows_top(&self) -> f32 {
        crate::ui::tab_bar::TAB_BAR_HEIGHT + self.spotlight_h() + ROWS_GAP
    }

    /// What the page under Home draws from.
    fn stage(&self) -> Stage<'a> {
        Stage {
            library: self.library,
            covers: self.covers,
            backdrop: self.backdrop,
            now: self.now,
            band: self.spotlight_h(),
            rows_top: self.rows_top(),
            shade: &SHADE,
            cover_cards: true,
        }
    }
}

/// How Home's page darkens left to right behind the spotlight's text: where across the window, and
/// how much of the ground is laid over the picture there.
pub const SHADE: [(f32, f32); 4] = [(0.0, 1.0), (0.28, 0.94), (0.5, 0.6), (0.74, 0.0)];

/// What the page under a spotlight draws from — Home's, and the Library's over its grid.
pub struct Stage<'a> {
    pub library: &'a Library,
    pub covers: &'a Covers,
    pub backdrop: &'a Backdrop,
    pub now: Instant,
    /// The spotlight's band, from under the header: a cover card stands in it, centred.
    pub band: f32,
    /// Where what lies under the spotlight starts, from the top of the window: the picture fades out
    /// before it.
    pub rows_top: f32,
    /// How the page darkens left to right behind the spotlight's text — see [`SHADE`].
    pub shade: &'a [(f32, f32)],
    /// A game with no screenshot has its cover stood in the band as its picture. Not on a page that
    /// shows the cover already.
    pub cover_cards: bool,
}

impl Stage<'_> {
    /// The spotlight's picture's box: as tall as it can be and still fade out before the rows under
    /// it, at the box's own shape.
    fn picture_size(&self) -> (f32, f32) {
        let height = (self.rows_top + 70.0).min(PICTURE.1);
        (height * PICTURE.0 / PICTURE.1, height)
    }
}

/// Resolve a host-relative RomM asset path against `host`. A path that is already fully-qualified
/// is passed through, so a RomM that sends absolute URLs still works.
pub fn asset_url(host: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}{}", host.trim_end_matches('/'), path)
    }
}

/// What the spotlight shows of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture<'a> {
    /// A screenshot, filling the picture's box and lighting the page.
    Shot(&'a str),
    /// The cover, as a card, for a game RomM has no screenshot of. It still lights the page.
    Cover(&'a str),
}

impl<'a> Picture<'a> {
    pub fn url(self) -> &'a str {
        match self {
            Picture::Shot(url) | Picture::Cover(url) => url,
        }
    }
}

/// What the spotlight shows of `game`: its first screenshot, else its cover, else nothing.
pub fn picture_of(game: &Game) -> Option<Picture<'_>> {
    game.screenshots
        .first()
        .map(|url| Picture::Shot(url))
        .or_else(|| game.cover_url.as_deref().map(Picture::Cover))
}

/// The id of one shelf's row, so the card under the cursor can be scrolled into view.
pub fn shelf_id(shelf: &Shelf) -> widget::Id {
    // `Id::new` is const and takes a `&'static str`; a per-shelf id is built at runtime.
    widget::Id::from(format!("shelf-{}", shelf.id))
}

/// Where the card at `index` lies across a shelf, in the row's own scroll space: the margin, then
/// every card and gap before it.
pub fn card_span(shelf: &Shelf, index: usize, covers: &Covers, library: &Library) -> (f32, f32) {
    let left = MARGIN + card_offset(shelf, index, covers, library);
    let width = shelf.games.get(index).map_or(0.0, |g| card_width(g, COVER_H, covers, library));
    (left, left + width)
}

/// How far along a shelf the card at `index` starts, in the row's own scroll space.
pub fn card_offset(shelf: &Shelf, index: usize, covers: &Covers, library: &Library) -> f32 {
    shelf.games.iter().take(index).map(|g| card_width(g, COVER_H, covers, library) + CARD_GAP).sum()
}

/// A card's width: the shared height times the art's own ratio.
pub fn card_width(game: &Game, cover_h: f32, covers: &Covers, library: &Library) -> f32 {
    let ratio = game
        .cover_url
        .as_deref()
        .and_then(|url| covers.meta(url))
        .map(|m| m.ratio)
        .unwrap_or_else(|| library.cover_ratio(&game.system_id));
    cover_h * ratio
}

/// The title's size where a game has no logo: large for a short name, smaller for a long one, so
/// most names fit the logo's box in a line or two.
pub fn title_size(title: &str) -> f32 {
    match title.chars().count() {
        0..=18 => 48.0,
        19..=30 => 40.0,
        _ => 34.0,
    }
}

/// The page under everything else on Home — the header included, which floats on it.
pub fn page_backdrop<'a>(ctx: &HomeContext<'a>) -> Element<'a, Message> {
    stage(&ctx.stage())
}

/// The page under a spotlight: the spotlit game's light over the whole window, its picture at the
/// top right, and the scrims that keep the text over them readable.
///
/// Two games while the spotlight hands over from one to the next, each at its own strength — see
/// [`Backdrop`].
pub fn stage<'a>(ctx: &Stage<'a>) -> Element<'a, Message> {
    let mut light = stack![].width(Length::Fill).height(Length::Fill);
    let mut pictures = stack![].width(Length::Fill).height(Length::Fill);
    let mut cards = stack![].width(Length::Fill).height(Length::Fill);

    for (rom_id, amount) in ctx.backdrop.layers(ctx.now) {
        let Some(picture) = ctx.library.game(rom_id).and_then(picture_of) else { continue };
        if amount <= 0.0 {
            continue;
        }
        // The light: a 64x40 thumbnail stretched over the window, which the GPU's own filtering
        // turns into a soft wash of the picture's colours.
        if let Some(handle) = ctx.covers.handle(&covers::key(picture.url(), Kind::Ambient)) {
            light = light.push(
                image(handle.clone())
                    .content_fit(ContentFit::Fill)
                    .filter_method(image::FilterMethod::Linear)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .opacity(amount),
            );
        }
        match picture {
            Picture::Shot(url) => {
                let key = covers::key(url, Kind::Spotlight);
                // Prepared at the pixels it is drawn at — see `covers::Kind::Spotlight` — so it is
                // drawn one to one, and a console's pixel art stays square.
                if let Some(handle) = ctx.covers.handle(&key) {
                    let (width, height) = ctx.picture_size();
                    pictures = pictures.push(
                        container(
                            image(handle.clone())
                                .content_fit(ContentFit::Fill)
                                .width(Length::Fixed(width))
                                .height(Length::Fixed(height))
                                .opacity(amount),
                        )
                        .align_right(Length::Fill)
                        .height(Length::Fill),
                    );
                }
            }
            Picture::Cover(_) if !ctx.cover_cards => {}
            Picture::Cover(url) => {
                let key = covers::key(url, Kind::Feature);
                if let (Some(handle), Some(meta)) = (ctx.covers.handle(&key), ctx.covers.meta(&key)) {
                    cards = cards.push(cover_card(handle.clone(), meta.ratio, amount, ctx.band));
                }
            }
        }
    }

    stack![
        ground(),
        light,
        pictures,
        scrims(ctx.rows_top, ctx.shade),
        // Over the scrims rather than under them: the card is the picture, and the left scrim would
        // dim its left half into the page.
        cards,
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Home's ground and nothing on it: the page under a spotlight with no game in it.
pub fn ground<'a>() -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(theme::GROUND)), ..Default::default() })
        .into()
}

/// Under a page with no game to show, and its header: Home's ground, and a wash of the accent at the
/// top right where Home has its picture. Settings' and the Collections page's.
pub fn plain_stage<'a>() -> Element<'a, Message> {
    let (w, h) = WASH_BOX;
    stack![
        ground(),
        container(image(accent_wash().clone()).content_fit(ContentFit::Fill).width(Length::Fixed(w)).height(Length::Fixed(h)))
            .width(Length::Fill)
            .align_right(Length::Fill),
    ]
    .into()
}

/// The wash's box, at the window's top right.
const WASH_BOX: (f32, f32) = (740.0, 400.0);

/// The accent fading out from a little in from the window's top right corner: an ellipse's worth,
/// of which the box shows the part inside the window. Drawn once, small — it has no edge to keep
/// sharp — and stretched to the box.
fn accent_wash() -> &'static image::Handle {
    static WASH: OnceLock<image::Handle> = OnceLock::new();
    WASH.get_or_init(|| {
        const SCALE: f32 = 5.0;
        let (w, h) = ((WASH_BOX.0 / SCALE) as u32, (WASH_BOX.1 / SCALE) as u32);
        // In the box's own units: the middle, and how far the light reaches across and down.
        let (cx, cy, rx, ry) = (450.0, 90.0, 450.0, 310.0);
        let [r, g, b, _] = theme::ACCENT_600.into_rgba8();
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let dx = ((x as f32 + 0.5) * SCALE - cx) / rx;
                let dy = ((y as f32 + 0.5) * SCALE - cy) / ry;
                let reach = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                rgba.extend([r, g, b, (0.30 * reach * 255.0).round() as u8]);
            }
        }
        image::Handle::from_rgba(w, h, rgba)
    })
}

/// The cover of a game with no screenshot, where its picture would be: a card at the right of the
/// spotlight, with a shadow to stand it off the light behind it.
fn cover_card<'a>(handle: image::Handle, ratio: f32, amount: f32, band: f32) -> Element<'a, Message> {
    let width = (CARD_H.min(band - 40.0) * ratio).min(CARD_MAX_W);
    let height = width / ratio.max(0.01);
    let card = container(
        image(handle)
            .content_fit(ContentFit::Fill)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .border_radius(14.0)
            .opacity(amount),
    )
    .style(move |_| container::Style {
        shadow: Shadow { color: alpha(Color::BLACK, 0.6 * amount), offset: Vector::new(0.0, 40.0), blur_radius: 90.0 },
        border: Border { radius: 14.0.into(), ..Default::default() },
        ..Default::default()
    });
    // Centred in the band beside the text: the band runs from under the header to the shelves.
    container(card)
        .padding(Padding { top: crate::ui::tab_bar::TAB_BAR_HEIGHT + (band - height) / 2.0, right: 132.0, bottom: 0.0, left: 0.0 })
        .align_right(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The three washes of the page's own colour that keep text readable over the picture: down the left
/// where the spotlight's text is, as `shade` says, across the top under the header, and up from the
/// bottom under the shelves — rising from a little above `rows_top`, where the shelves' window
/// starts.
fn scrims<'a>(rows_top: f32, shade: &[(f32, f32)]) -> Element<'a, Message> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let left = wash(FRAC_PI_2, shade);
    let top = container(wash(PI, &[(0.0, 0.82), (1.0, 0.0)]).height(Length::Fixed(160.0))).height(Length::Fill);
    let bottom = column![Space::new().height(Length::Fixed(rows_top - 112.0)), wash(0.0, &[(0.0, 0.94), (0.64, 0.94), (1.0, 0.0)])]
        .height(Length::Fill);
    stack![left, top, bottom].width(Length::Fill).height(Length::Fill).into()
}

/// A full-width gradient of the page's ground, at `angle` (iced's: π/2 runs left to right, π top to
/// bottom, 0 bottom to top), through `stops` of offset and opacity.
fn wash<'a>(angle: f32, stops: &[(f32, f32)]) -> container::Container<'a, Message> {
    let linear = stops
        .iter()
        .fold(iced::gradient::Linear::new(Radians(angle)), |linear, (offset, a)| linear.add_stop(*offset, alpha(theme::GROUND, *a)));
    container(Space::new()).width(Length::Fill).height(Length::Fill).style(move |_| container::Style {
        background: Some(Background::Gradient(Gradient::Linear(linear))),
        ..Default::default()
    })
}

/// Home itself: the spotlight, and under it the shelves.
pub fn home_view<'a>(ctx: &HomeContext<'a>) -> Element<'a, Message> {
    let (Some(game), Some(shelf)) = (ctx.focus.spotlit(ctx.shelves), ctx.shelves.get(ctx.focus.shelf_pos())) else {
        return container(
            text("Your library is empty — add some games in RomM and refresh.")
                .size(theme::text_size::BODY_LARGE)
                .color(theme::TEXT_SECONDARY),
        )
        .center(Length::Fill)
        .into();
    };

    column![spotlight(ctx, shelf, game), Space::new().height(Length::Fixed(ROWS_GAP)), shelves_window(ctx)]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------------------------
// The spotlight
// ---------------------------------------------------------------------------------------------

fn spotlight<'a>(ctx: &HomeContext<'a>, shelf: &'a Shelf, game: &'a Game) -> Element<'a, Message> {
    let state = ctx.downloads.state_of(game.builds());
    let mut info = column![
        eyebrow(ctx, shelf, game),
        Space::new().height(Length::Fixed(18.0)),
        title_art(ctx, game),
        Space::new().height(Length::Fixed(20.0)),
        meta_line(game),
    ]
    .width(Length::Fixed(SPOTLIGHT_W));
    if !ctx.compact {
        info = info.push(Space::new().height(Length::Fixed(10.0))).push(
            container(
                text(ctx.summary.clone())
                    .size(SUMMARY_SIZE)
                    .line_height(LineHeight::Absolute(SUMMARY_LINE.into()))
                    .color(alpha(theme::INK, 0.76)),
            )
            .width(Length::Fixed(SPOTLIGHT_W - 40.0))
            .height(Length::Fixed(SUMMARY_LINE * 2.0))
            .clip(true),
        );
    }
    info = info.push(Space::new().height(Length::Fixed(24.0))).push(view_button(game));
    if !ctx.compact {
        info = info.push(Space::new().height(Length::Fixed(18.0))).push(facts(game, &state, ctx.earned));
    }

    container(info)
        .width(Length::Fill)
        .height(Length::Fixed(ctx.spotlight_h()))
        .padding([0.0, MARGIN])
        .align_y(alignment::Vertical::Bottom)
        .into()
}

/// The shelf the game is on, as a filled tag, then its system and when it was last played — or,
/// for one never played, when it arrived.
fn eyebrow<'a>(ctx: &HomeContext<'a>, shelf: &'a Shelf, game: &'a Game) -> Element<'a, Message> {
    eyebrow_of(tag(shelf.title.to_uppercase(), true), Some(ctx.library.system_name(&game.system_id)), game, ctx.this_year)
}

/// A tag, then `system` — when there is one to name — and when the game was last played, or for one
/// never played, when it arrived.
pub fn eyebrow_of<'a>(tag: Element<'a, Message>, system: Option<&str>, game: &Game, this_year: i64) -> Element<'a, Message> {
    let when = match (game.last_played.as_deref(), game.created_at.as_deref()) {
        (Some(played), _) => crate::library::short_date(played, this_year).map(|d| format!("Played {d}")),
        (None, Some(added)) => crate::library::short_date(added, this_year).map(|d| format!("Added {d}")),
        (None, None) => None,
    };
    let line = [system.map(str::to_string), when].into_iter().flatten().collect::<Vec<_>>().join("  ·  ").to_uppercase();
    row![tag, text(line).size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82))]
        .spacing(12)
        .height(Length::Fixed(26.0))
        .align_y(Alignment::Center)
        .into()
}

/// The spotlight's tag: filled with the accent, or glass where it says something quieter.
pub fn tag<'a>(words: String, filled: bool) -> Element<'a, Message> {
    container(text(words).size(12.0).font(theme::BOLD).color(if filled { Color::WHITE } else { alpha(theme::INK, 0.86) }))
        .height(Length::Fixed(26.0))
        .padding([0.0, 10.0])
        .align_y(alignment::Vertical::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(if filled { theme::ACCENT_600 } else { theme::glass(0.08) })),
            border: Border { color: if filled { Color::TRANSPARENT } else { theme::glass(0.16) }, width: 1.0, radius: 13.0.into() },
            ..Default::default()
        })
        .into()
}

/// RomM's transparent logo when the game has one, the name set large when it does not — never
/// taller than a two-line name.
fn title_art<'a>(ctx: &HomeContext<'a>, game: &'a Game) -> Element<'a, Message> {
    let logo = game.logo_path.as_deref().map(|path| ctx.asset_url(path)).and_then(|url| {
        let handle = ctx.covers.handle(&url)?.clone();
        Some((handle, ctx.covers.meta(&url)?.ratio))
    });
    let art: Element<'a, Message> = match logo {
        // Sized from the logo's own shape rather than fitted into the box: `Contain` would centre a
        // narrow logo in it, and the logo belongs at the left edge with the text under it.
        Some((handle, ratio)) => {
            let width = (LOGO_H * ratio).min(LOGO_W);
            image(handle)
                .content_fit(ContentFit::Fill)
                .width(Length::Fixed(width))
                .height(Length::Fixed(width / ratio.max(0.01)))
                .into()
        }
        None => {
            let size = title_size(&game.title);
            text(ctx.title.clone())
                .size(size)
                .font(theme::DISPLAY)
                .line_height(LineHeight::Relative(1.05))
                .color(theme::INK)
                .width(Length::Fixed(SPOTLIGHT_W))
                .into()
        }
    };
    // As tall as what is in it, so the tag above sits on it: everything under it is a fixed height
    // and the band sits on its bottom edge, so a short name moves only the tag, never the button.
    container(art).width(Length::Fill).max_height(LOGO_H).into()
}

/// Year, genres, players and rating, in one line that never wraps.
pub fn meta_line<'a>(game: &'a Game) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = Vec::new();
    let ink = alpha(theme::INK, 0.88);
    let mut push = |part: Element<'a, Message>| {
        if !parts.is_empty() {
            parts.push(text("·").size(15.0).color(alpha(theme::INK, 0.4)).into());
        }
        parts.push(part);
    };
    if let Some(year) = game.year {
        push(text(year.to_string()).size(15.0).color(ink).into());
    }
    let genres = game.genres.iter().take(2).map(|g| short_genre(g)).collect::<Vec<_>>().join(" · ");
    if !genres.is_empty() {
        push(text(genres).size(15.0).color(ink).into());
    }
    if let Some(players) = game.players.as_deref().map(players_label) {
        push(text(players).size(15.0).color(ink).into());
    }
    if let Some(rating) = game.rating {
        push(
            row![text("★").size(15.0).color(theme::STAR), text(format!("{rating:.1}")).size(15.0).color(ink)]
                .spacing(5)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    iced::widget::Row::with_children(parts)
        .spacing(10)
        .height(Length::Fixed(22.0))
        .align_y(Alignment::Center)
        .into()
}

/// The spotlight's one button: the game's own page, where it is played or fetched, its saves and
/// states are, and it is a favourite. A on its card presses it, whichever shelf the card is on.
pub fn view_button<'a>(game: &'a Game) -> Element<'a, Message> {
    let label = row![glyph("A", true), text("View").size(16.0).font(theme::BOLD).color(Color::WHITE)]
        .spacing(10)
        .align_y(Alignment::Center);
    button(container(label).height(Length::Fill).align_y(Alignment::Center))
        .height(Length::Fixed(52.0))
        .padding([0.0, 22.0])
        .on_press(Message::OpenDetail(game.rom_id))
        .style(crate::ui::style::spotlight(true, false))
        .into()
}

/// What is worth knowing before pressing: how big it is and whether it is here, how long it takes,
/// and how far into its achievements you are — `earned` of them, when RomM said. Each only when RomM
/// says.
pub fn facts<'a>(game: &'a Game, state: &SyncState, earned: Option<usize>) -> Element<'a, Message> {
    let mut items = row![].spacing(22).height(Length::Fixed(20.0)).align_y(Alignment::Center);
    let fact = |icon: iced::widget::Text<'a>, words: String| -> Element<'a, Message> {
        let ink = alpha(theme::INK, 0.72);
        row![icon.size(15.0).color(ink), text(words).size(14.0).color(ink)].spacing(7).align_y(Alignment::Center).into()
    };
    if let Some(bytes) = game.size_bytes.filter(|b| *b > 0) {
        let size = human_bytes(bytes as u64);
        items = items.push(fact(
            lucide::hard_drive(),
            if state.is_here() { format!("{size} on this device") } else { format!("{size} to download") },
        ));
    }
    if game.beat_secs.is_some() {
        items = items.push(fact(lucide::clock_four(), format!("Main story {}", game.beat_label())));
    }
    if game.achievements > 0 {
        let total = game.achievements;
        items = items.push(fact(
            lucide::trophy(),
            match earned {
                Some(earned) => format!("{earned} of {total} achievements"),
                None if total == 1 => "1 achievement".into(),
                None => format!("{total} achievements"),
            },
        ));
    }
    items.into()
}

/// IGDB's genre names, cut to what fits a meta line.
pub fn short_genre(genre: &str) -> &str {
    match genre {
        "Role-playing (RPG)" => "RPG",
        "Hack and slash/Beat 'em up" => "Beat ’em up",
        "Card & Board Game" => "Board game",
        "Turn-based strategy (TBS)" | "Real Time Strategy (RTS)" => "Strategy",
        "Point-and-click" => "Point & click",
        "Quiz/Trivia" => "Quiz",
        "Simulator" => "Simulation",
        other => other,
    }
}

/// RomM's player count — "1", "1-2" — as words.
pub fn players_label(players: &str) -> String {
    let players = players.trim();
    if players == "1" {
        "1 player".into()
    } else {
        format!("{} players", players.replace('-', "–"))
    }
}

// ---------------------------------------------------------------------------------------------
// The shelves
// ---------------------------------------------------------------------------------------------

/// The shelves, seen through the window under the spotlight — see [`crate::ui::slide`].
///
/// Only the shelf the cursor is on and the ones below it are built: a shelf above the window is not
/// there at all. The column is shown from one heading down — the shelf with the cursor is named by
/// the spotlight's tag, so its own heading sits just out of sight — and slides by [`Glide`].
///
/// Keyed by shelf, so a shelf's row keeps its scroll position as it moves up the column: a plain
/// column matches rows by place, and the row now first would take over the scroll of the one that
/// was.
fn shelves_window<'a>(ctx: &HomeContext<'a>) -> Element<'a, Message> {
    let (first, into) = ctx.glide.window(ctx.now);
    let shelves = ctx
        .shelves
        .iter()
        .enumerate()
        .skip(first)
        .fold(keyed_column([]).width(Length::Fill), |column, (index, shelf)| column.push(shelf_key(shelf), shelf_band(ctx, shelf, index)));
    let window = crate::ui::slide::slide(shelves, HEADING_H + HEADING_GAP + into * SHELF_PITCH);

    let edge = container(
        container(Space::new()).width(Length::Fixed(EDGE_FADE)).height(Length::Fill).style(|_| container::Style {
            background: Some(Background::Gradient(Gradient::Linear(
                iced::gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
                    .add_stop(0.0, Color::TRANSPARENT)
                    .add_stop(1.0, theme::GROUND),
            ))),
            ..Default::default()
        }),
    )
    .align_right(Length::Fill)
    .height(Length::Fill);

    // The wheel walks the shelves as Up and Down do. A shelf's own row scrolls only sideways, so a
    // turn of the wheel over it is not taken by the row and arrives here.
    mouse_area(stack![window, edge].width(Length::Fill).height(Length::Fill))
        .on_scroll(Message::HomeWheel)
        .into()
}

/// A key for a shelf in the keyed column: its id, hashed — the column wants a `Copy` key.
fn shelf_key(shelf: &Shelf) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    shelf.id.hash(&mut hasher);
    hasher.finish()
}

/// One shelf: its heading and its row of covers.
fn shelf_band<'a>(ctx: &HomeContext<'a>, shelf: &'a Shelf, shelf_index: usize) -> Element<'a, Message> {
    let heading = row![
        text(shelf.title.as_str()).size(20.0).font(theme::DISPLAY).color(theme::INK),
        text(shelf.note.as_str()).size(14.0).color(alpha(theme::INK, 0.68)),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .height(Length::Fixed(HEADING_H))
    .padding([0.0, MARGIN]);

    let cards = shelf
        .games
        .iter()
        .enumerate()
        .fold(row![].spacing(CARD_GAP).align_y(Alignment::End), |cards, (index, game)| cards.push(card(ctx, shelf_index, index, game)));

    let strip = scrollable(container(cards).padding(Padding { top: ROW_TOP - LIFT, right: MARGIN, bottom: ROW_BOTTOM, left: MARGIN }))
        .id(shelf_id(shelf))
        .on_scroll(|_| Message::Scrolled)
        .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::hidden()))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_TOP + COVER_H + ROW_BOTTOM));

    column![heading, Space::new().height(Length::Fixed(HEADING_GAP)), strip, Space::new().height(Length::Fixed(SHELF_GAP))].into()
}

/// One card. Its light, its lift and its ring follow the cursor, which the spotlight is on: that is
/// how the eye finds where the cursor went.
fn card<'a>(ctx: &HomeContext<'a>, shelf_index: usize, index: usize, game: &'a Game) -> Element<'a, Message> {
    let key = Cursor::HomeCard(shelf_index, index);
    let spotlit = ctx.focus.is_focused(ctx.shelves, shelf_index, index);
    // How lit this card is *right now*: 1 in the spotlight, 0 well away from it, and somewhere
    // between while the spotlight is handing over to or from it.
    let lit = ctx.fade.amount(key, spotlit, ctx.now);
    let hovered = ctx.hovered == Some(key);
    let (bright, rest) = hover_look(lit, hovered);
    let width = card_width(game, COVER_H, ctx.covers, ctx.library);

    let art: Element<Message> = match game.cover_url.as_deref().and_then(|u| ctx.covers.handle(u)) {
        Some(handle) => image(handle.clone())
            .content_fit(ContentFit::Cover)
            .width(Length::Fixed(width - 2.0 * RING))
            .height(Length::Fixed(COVER_H - 2.0 * RING))
            .opacity(lerp(UNFOCUSED_DIM, 1.0, bright))
            .border_radius(8.0 - RING)
            .into(),
        None => placeholder(game, ctx.library.system_name(&game.system_id), 14.0, bright),
    };
    let ring = if spotlit { mix(rest, theme::INK, lit) } else { rest };
    let cover = container(ring_of(badged(art, &ctx.downloads.state_of(game.builds())), theme::SURFACE_VARIANT, ring))
        .width(Length::Fixed(width))
        .height(Length::Fixed(COVER_H))
        .style(move |_| container::Style {
            shadow: Shadow {
                color: alpha(Color::BLACK, lerp(0.4, 0.55, lit)),
                offset: Vector::new(0.0, lerp(8.0, 16.0, lit)),
                blur_radius: lerp(20.0, 36.0, lit),
            },
            border: Border { radius: 8.0.into(), ..Default::default() },
            ..Default::default()
        });

    // The lift is padding moved from above the card to below it, inside a box that stays put: the
    // card rises out of the row without the row, or any card after it, moving at all.
    let lifted = container(cover)
        .padding(Padding { top: LIFT * (1.0 - lit), bottom: LIFT * lit, left: 0.0, right: 0.0 })
        .height(Length::Fixed(COVER_H + LIFT));

    let card = button(lifted).padding(0).on_press(Message::HomeCardClicked(shelf_index, index)).style(crate::ui::style::bare);
    mouse_area(card).on_enter(Message::Hovered(key)).on_exit(Message::Unhovered(key)).into()
}

// ---------------------------------------------------------------------------------------------
// Measuring text
// ---------------------------------------------------------------------------------------------

/// `text` cut to what fits in `lines` lines `width` wide, with an ellipsis where it was cut.
///
/// Measured rather than counted. iced has no line clamp, and a cut by character count is either
/// cautious enough to leave half a line empty or bold enough to spill onto one more. This lays the
/// text out exactly as the view will — the same face, size, line height and wrapping, through iced's
/// own paragraph — and finds the longest cut at a word boundary that still fits.
pub fn clamp_lines(text: &str, width: f32, lines: usize, size: f32, line_height: f32, font: Font) -> String {
    use iced::advanced::text::Paragraph as _;
    let fits = |candidate: &str| {
        let paragraph = iced::advanced::graphics::text::Paragraph::with_text(iced::advanced::Text {
            content: candidate,
            bounds: iced::Size::new(width, f32::INFINITY),
            size: size.into(),
            line_height: LineHeight::Absolute(line_height.into()),
            font,
            align_x: iced::advanced::text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Auto,
            wrapping: iced::advanced::text::Wrapping::Word,
        });
        // Wide as well as tall: a word longer than the line is not broken, it runs past the edge.
        let bounds = paragraph.min_bounds();
        bounds.height <= line_height * lines as f32 + 0.5 && bounds.width <= width + 0.5
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if fits(&text) {
        return text;
    }
    let cut = |end: usize| format!("{}…", text[..end].trim_end_matches([',', ';', ':', '.', ' ']));
    let cuts: Vec<usize> = text.match_indices(' ').map(|(at, _)| at).collect();
    // The longest prefix that fits, by bisection over the word boundaries: fitting only ever
    // stops as the cut grows, so the boundary between fits and does not is a single place.
    let (mut fit, mut overflow) = (0, cuts.len());
    if cuts.is_empty() || !fits(&cut(cuts[0])) {
        // Not even a first word fits: cut by characters instead, as few as it takes.
        let mut end = text.len();
        while end > 0 && !fits(&cut(end)) {
            end = text[..end].char_indices().next_back().map_or(0, |(at, _)| at);
        }
        return cut(end);
    }
    while overflow - fit > 1 {
        let middle = (fit + overflow) / 2;
        if fits(&cut(cuts[middle])) { fit = middle } else { overflow = middle }
    }
    cut(cuts[fit])
}

// ---------------------------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------------------------

/// Lay a download badge over a cover's bottom-left corner, or hand the art back untouched for a
/// game that is only on the server.
///
/// Shared by the shelf cards and the Browse grid, both of which draw the badge the same way and
/// into a clipping container, so it is cropped with the art rather than growing the card.
pub fn badged<'a>(art: Element<'a, Message>, state: &SyncState) -> Element<'a, Message> {
    let Some(badge) = crate::ui::downloads::badge(state) else { return art };
    stack![
        art,
        container(badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(6),
    ]
    .into()
}

/// How wide a cover's ring is, and so how far its art sits inside the card.
pub const RING: f32 = 3.0;

/// A cover in its ring: the art inset by [`RING`], and the ring drawn *over* it at the card's edge
/// in `ring` — the brightest ink under the cursor, the page's own colour away from it.
///
/// The art is inset, and every ring is opaque, because a renderer may not round or crop an image:
/// tiny-skia draws one square-cornered whatever its container or its own `border_radius` say. Drawn
/// to the card's edge, its corners poked out past a rounded ring. Inset inside an opaque ring of 8
/// px corners they fall under the ring's stroke, so every renderer draws the same rounded card;
/// away from the cursor the ring is the page's own colour, and only the rounding shows.
///
/// The ring is also why the focused cover is not zoomed, as the Compose original's is. A zoom
/// drawn inside the card — `Image::scale` — is cropped to the card by wgpu but not by tiny-skia,
/// which painted the art past the ring on every side; a zoom of the whole card would shift every
/// card after it. The ring and the brightness say which card it is without either.
///
/// `ground` fills the inside behind the art, for art that does not cover it. Nothing is drawn
/// outside the ring, so no other edge shows beside it.
pub fn ring_of<'a>(art: Element<'a, Message>, ground: Color, ring: Color) -> Element<'a, Message> {
    let inside = container(art).width(Length::Fill).height(Length::Fill).style(move |_| container::Style {
        background: Some(Background::Color(ground)),
        border: Border { radius: (8.0 - RING).into(), ..Default::default() },
        ..Default::default()
    });
    let ring = container(Space::new()).width(Length::Fill).height(Length::Fill).style(move |_| container::Style {
        border: Border { color: ring, width: RING, radius: 8.0.into() },
        ..Default::default()
    });
    stack![container(inside).padding(RING), ring].into()
}

/// How a game card reads under the mouse pointer, as against under the cursor.
///
/// The pointer lifts a card out of the dim; only the cursor adds its ring. The two must not look the
/// same: the pointer can rest on one card while the cursor sits on another, and Confirm presses the
/// cursor's. Each also sets the cover's ring — the muted accent for the pointer — see [`ring_of`].
///
/// Returns how bright the card is — what drives the art's opacity and the title's colour — and
/// the ring colour it rests at before the cursor's is mixed in.
pub fn hover_look(lit: f32, hovered: bool) -> (f32, Color) {
    // Opaque, both: the ring hides the art's corners — see [`ring_of`]. At rest it is the ground
    // the cards sit on, which is what a ring at rest has to vanish into.
    if hovered {
        (1.0, theme::FOCUS_RING)
    } else {
        (lit, theme::GROUND)
    }
}

/// A game with no art: its system tagged small in the top corner and its title set large across
/// the bottom, so an art-less card is still identifiable at shelf size instead of a blank box.
///
/// The ground is the card's own surface, not the system's hue. The hue is a *marking* colour, and
/// filling a whole card with it turns a shelf of unscraped games into a row of coloured blocks that
/// out-shouts every real cover beside them.
fn placeholder<'a>(game: &'a Game, system_label: &'a str, title_size: f32, lit: f32) -> Element<'a, Message> {
    // An art-less card carries its whole identity in text, so it dims through its colours rather
    // than through an image opacity — the same brightness step, applied where the ink is.
    let title = mix(theme::TEXT_BODY, theme::TEXT_PRIMARY, lit);
    container(
        column![
            system_tag(system_label),
            Space::new().height(Length::Fill),
            text(truncate(&game.title, 60)).size(title_size).font(theme::EXTRABOLD).color(title),
        ]
        .padding(10),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(mix(theme::SURFACE_VARIANT, theme::SURFACE, lit))),
        ..Default::default()
    })
    .into()
}

/// The small system chip an art-less card carries in its top corner.
fn system_tag<'a>(system_label: &'a str) -> Element<'a, Message> {
    container(text(system_label.to_uppercase()).size(9.0).font(theme::BOLD).color(alpha(theme::NEUTRAL_100, 0.8)))
        .padding([3, 6])
        .style(|_| container::Style {
            background: Some(Background::Color(alpha(Color::BLACK, 0.32))),
            border: Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// Roughly how many characters of `size`-pt text fit across `width`.
///
/// An estimate rather than a measurement, for the many short labels where one is good enough — see
/// [`clamp_lines`] for where it is not. 0.55 em per character is a shade wide for DM Sans, which is
/// the safe direction to be wrong in: a title that stops one character early reads as considered,
/// where one that wraps costs a line and gets clipped.
pub fn chars_across(width: f32, size: f32) -> usize {
    ((width / (size * 0.55)).floor() as usize).max(6)
}

/// [`truncate`] for a string already owned — handed back as it is when it fits.
pub fn truncated(s: String, max: usize) -> String {
    if s.chars().count() <= max { s } else { truncate(&s, max) }
}

/// Clip to `max` characters on a word boundary where there is one nearby.
///
/// Cutting on a word boundary matters most for long text, where a mid-word cut reads as a rendering
/// bug rather than as a deliberate summary.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max).collect();
    let cut = clipped.rfind(' ').filter(|i| *i > max * 2 / 3).unwrap_or(clipped.len());
    format!("{}…", clipped[..cut].trim_end_matches([' ', ',', '.', ';']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_prefers_a_word_boundary_but_still_cuts_a_long_word() {
        assert_eq!(truncate("short", 20), "short");
        // The limit falls mid-word, and there is a space late enough to back up to.
        assert_eq!(truncate("one two three four five", 16), "one two three…");
        // No boundary late enough to be worth using: cut at the limit rather than throwing away
        // most of the text to reach an early space.
        assert_eq!(truncate("one twentycharacterword", 20), "one twentycharacterw…");
        assert_eq!(truncate("aaaaaaaaaaaaaaaaaaaa", 10), "aaaaaaaaaa…");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        // A multi-byte title must not be sliced mid-character.
        assert_eq!(truncate("ポケットモンスター", 4), "ポケット…");
    }

    fn shelf(id: &str, n: usize) -> Shelf {
        Shelf {
            id: id.into(),
            title: id.into(),
            note: String::new(),
            games: (0..n)
                .map(|i| Game {
                    rom_id: i as i64,
                    title: format!("Game {i}"),
                    sort_title: format!("game {i}"),
                    system_id: "snes".into(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn library() -> Library {
        Library::new(vec![crate::library::System { id: "snes".into(), name: "SNES".into(), cover_ratio: 1.37 }], vec![])
    }

    #[test]
    fn card_offsets_accumulate_each_cards_own_width() {
        let shelf = shelf("a", 4);
        let (covers, library) = (Covers::default(), library());

        // No art loaded, so every card falls back to the system's 1.37 box.
        let step = COVER_H * 1.37 + CARD_GAP;
        assert_eq!(card_offset(&shelf, 0, &covers, &library), 0.0, "the first card is the origin");
        assert!((card_offset(&shelf, 1, &covers, &library) - step).abs() < 0.01);
        assert!((card_offset(&shelf, 3, &covers, &library) - step * 3.0).abs() < 0.01);
        assert_eq!(card_span(&shelf, 0, &covers, &library).0, MARGIN, "the row starts at the page's margin");
    }

    #[test]
    fn walking_a_shelf_moves_the_cursor_across_the_screen_before_the_row() {
        let shelf = shelf("a", 14);
        let (covers, library) = (Covers::default(), library());
        let viewport = 1280.0;
        let mut offset = 0.0;
        let mut moved = Vec::new();
        for index in 0..14 {
            let (left, right) = card_span(&shelf, index, &covers, &library);
            let to = crate::ui::reveal::offset_to_reveal(left, right, offset, viewport, SHELF_PEEK);
            moved.push(to.is_some());
            offset = to.unwrap_or(offset);
        }
        let first = moved.iter().position(|m| *m).expect("a long shelf scrolls in the end");
        assert!(first >= 3, "the cursor takes a few steps before the row moves, got {moved:?}");
        assert!(moved[first..].iter().all(|m| *m), "then the row follows each step");
    }

    #[test]
    fn every_shelf_is_one_height_so_the_column_can_slide_a_shelf_at_a_time() {
        assert_eq!(SHELF_PITCH, HEADING_H + HEADING_GAP + ROW_TOP + COVER_H + ROW_BOTTOM + SHELF_GAP);
        // The spotlight, its gap and the first shelf's cards fit an 800-tall panel under the header,
        // with the next shelf's heading and a slice of its covers still showing beneath.
        let below_header = 800.0 - crate::ui::tab_bar::TAB_BAR_HEIGHT;
        let first_row_bottom = SPOTLIGHT_H + ROWS_GAP + ROW_TOP + COVER_H + ROW_BOTTOM;
        let next_covers_top = first_row_bottom + SHELF_GAP + HEADING_H + HEADING_GAP + ROW_TOP;
        assert!(next_covers_top + 40.0 < below_header, "the next shelf peeks in: {next_covers_top} of {below_header}");
    }

    #[test]
    fn genres_and_players_read_as_words() {
        assert_eq!(short_genre("Role-playing (RPG)"), "RPG");
        assert_eq!(short_genre("Racing"), "Racing");
        assert_eq!(players_label("1"), "1 player");
        assert_eq!(players_label("1-2"), "1–2 players");
    }

    #[test]
    fn the_spotlight_shows_a_screenshot_before_a_cover() {
        let mut game = Game { cover_url: Some("c".into()), ..Default::default() };
        assert_eq!(picture_of(&game), Some(Picture::Cover("c")));
        game.screenshots = vec!["s".into(), "t".into()];
        assert_eq!(picture_of(&game), Some(Picture::Shot("s")));
        assert_eq!(picture_of(&Game::default()), None);
    }

    #[test]
    fn a_long_title_gets_a_smaller_size() {
        assert!(title_size("Shadowrun") > title_size("Pokémon Yellow Special Pikachu Edition"));
    }

    #[test]
    fn clamped_text_fits_its_lines_and_says_where_it_was_cut() {
        let long = "word ".repeat(200);
        let clamped = clamp_lines(&long, 300.0, 2, 15.0, 22.0, theme::REGULAR);
        assert!(clamped.ends_with('…'), "cut, and says so: {clamped}");
        assert!(clamped.len() < long.len());
        let short = "A short description.";
        assert_eq!(clamp_lines(short, 300.0, 2, 15.0, 22.0, theme::REGULAR), short, "what fits is left alone");
        let unbroken = "x".repeat(400);
        assert!(clamp_lines(&unbroken, 100.0, 1, 15.0, 22.0, theme::REGULAR).ends_with('…'), "even with no space to cut at");
    }
}

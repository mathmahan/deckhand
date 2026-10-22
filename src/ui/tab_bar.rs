//! The header: the wordmark, the places you browse in one glass pill, and what is going on — the
//! downloads, the saves — at the right as icons, beside Settings as a gear. Icons, so that nothing
//! going on can crowd the gear off a 1280 px panel; the words are in their tooltips.
//!
//! The Kotlin app replaced a left rail with a bar at the top for a reason worth keeping: the rail
//! cost 200 px of a 1280 px panel permanently to show six of twenty-nine systems. Navigation at
//! the top hands that width back to the covers.
//!
//! The bar has no ground of its own. On Home it floats over the spotlight's picture, which runs to
//! the top of the screen, and everywhere else over the page's own colour — so it reads as part of
//! the page rather than as a strip across the top of it. The pills carry their own glass instead.

use iced::widget::{button, column, container, row, text, tooltip, Space, Text};
use iced::{Alignment, Background, Border, Color, Element, Length, Shadow, Theme};

use iced_fonts::lucide;

use crate::app::Message;
use crate::theme::{self, alpha, glass};

/// The destinations the bar can rest on, in the order it walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home,
    Browse,
    Collections,
    Search,
    Downloads,
    Settings,
}

impl Tab {
    /// Every destination, in the bar's order — what the shoulder buttons and the pad's Left/Right on
    /// the bar walk through.
    pub const ALL: [Tab; 6] = [Tab::Home, Tab::Browse, Tab::Collections, Tab::Search, Tab::Downloads, Tab::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            // Browse is the whole collection, a system or a collection at a time: a library.
            Tab::Browse => "Library",
            Tab::Collections => "Collections",
            Tab::Search => "Search",
            Tab::Downloads => "Downloads",
            Tab::Settings => "Settings",
        }
    }

    /// The tab's Lucide glyph.
    ///
    /// Returned as the widget rather than as a character so the font and the shaping mode come
    /// with it — an icon font set with the wrong shaping renders as a box, and getting that right
    /// once here beats repeating it at the call site.
    fn icon<'a>(self) -> Text<'a> {
        match self {
            Tab::Home => lucide::house(),
            Tab::Browse => lucide::library(),
            Tab::Collections => lucide::library_big(),
            Tab::Search => lucide::search(),
            Tab::Downloads => lucide::download(),
            Tab::Settings => lucide::settings(),
        }
    }
}

/// Height of the bar, and the content offset every destination sits below.
pub const TAB_BAR_HEIGHT: f32 = 72.0;

/// The bar's side margin — and Home's and the Library's, whose spotlight, shelves and system list
/// line up under the wordmark. Kept narrow: on the Deck's panel every unit of it is one the covers
/// do not get.
pub const MARGIN: f32 = 32.0;

/// How the header's saves icon reads — see [`Status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saves {
    /// Nothing has been synced yet this session, so there is nothing to say.
    Quiet,
    Syncing,
    InSync,
    /// The last run went, but some saves did not.
    Attention,
    /// The last run could not start.
    Failed,
}

/// What is going on, for the icons at the bar's right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    /// Downloads running or waiting, and how far they have got between them — None with none.
    pub downloads: Option<(usize, f32)>,
    pub saves: Saves,
}

/// The bar, with `current` filled in — and ringed as well when the pad's cursor is on the bar
/// (`focused`), which is how a pad reaches every destination.
pub fn tab_bar<'a>(current: Tab, focused: bool, status: Status) -> Element<'a, Message> {
    let places = [Tab::Home, Tab::Browse, Tab::Collections, Tab::Search]
        .into_iter()
        .fold(row![].spacing(4).align_y(Alignment::Center), |pill, tab| pill.push(place(tab, current, focused)));

    let pill = container(places).padding(5).style(glass_pill);

    let right = row![downloads_icon(current, focused, status.downloads), saves_icon(status.saves), gear(current, focused)]
        .spacing(10)
        .align_y(Alignment::Center);

    // The pixel wordmark, three pixels a cell: about the height the name had as text.
    let (letters, cells_w, cells_h) = theme::wordmark();
    let wordmark = row![
        iced::widget::image(theme::logo()).width(Length::Fixed(30.0)).height(Length::Fixed(30.0)),
        iced::widget::image(letters)
            .width(Length::Fixed(cells_w as f32 * 3.0))
            .height(Length::Fixed(cells_h as f32 * 3.0))
            .filter_method(iced::widget::image::FilterMethod::Nearest),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    row![
        wordmark,
        pill,
        Space::new().width(Length::Fill),
        right,
    ]
    .spacing(32)
    .align_y(Alignment::Center)
    .padding([0.0, MARGIN])
    .height(Length::Fixed(TAB_BAR_HEIGHT))
    .width(Length::Fill)
    .into()
}

/// One place in the pill. The one you are in is filled with the brightest ink and named with its
/// icon as well as its word; the rest are words alone, so the pill reads as one strip with a
/// marker in it rather than four buttons.
///
/// Search is the exception: a magnifier rather than a word, and beside it the pad button that opens
/// it from anywhere — the one destination with a shortcut of its own.
fn place<'a>(tab: Tab, current: Tab, focused: bool) -> Element<'a, Message> {
    let selected = tab == current;
    let content: Element<'a, Message> = match (tab, selected) {
        (Tab::Search, false) => row![tab.icon().size(17.0), glyph("Y", false)].spacing(8).align_y(Alignment::Center).into(),
        (_, true) => row![tab.icon().size(16.0), text(tab.label()).size(15.0).font(theme::SEMIBOLD)]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        (_, false) => text(tab.label()).size(15.0).font(theme::MEDIUM).into(),
    };
    let padding = if tab == Tab::Search && !selected { [0.0, 12.0] } else { [0.0, 18.0] };
    button(container(content).height(Length::Fill).align_y(Alignment::Center))
        .height(Length::Fixed(36.0))
        .padding(padding)
        .on_press(Message::TabSelected(tab))
        .style(pill_style(selected, selected && focused, false))
        .into()
}

/// The size of each icon at the bar's right, the gear's included.
const ICON: f32 = 44.0;

/// Downloads, as an icon that is also the way to the Downloads page. While downloads are coming it
/// is lit, with how far they have got between them under the glyph; the tooltip says how many.
fn downloads_icon<'a>(current: Tab, focused: bool, downloads: Option<(usize, f32)>) -> Element<'a, Message> {
    let selected = current == Tab::Downloads;
    let arrow = Tab::Downloads.icon().size(18.0);
    let (content, words): (Element<'a, Message>, String) = match downloads {
        Some((count, progress)) => {
            let arrow = if selected { arrow } else { arrow.color(theme::ACCENT_400) };
            (column![arrow, meter(progress, selected)].spacing(4).align_x(Alignment::Center).into(), format!("{count} downloading"))
        }
        None => (arrow.into(), Tab::Downloads.label().into()),
    };
    let icon = button(container(content).center(Length::Fill))
        .width(Length::Fixed(ICON))
        .height(Length::Fixed(ICON))
        .padding(0)
        .on_press(Message::TabSelected(Tab::Downloads))
        .style(pill_style(selected, selected && focused, true));
    tip(icon, words)
}

/// A download's progress, small enough to sit under the downloads glyph.
fn meter<'a>(progress: f32, selected: bool) -> Element<'a, Message> {
    const W: f32 = 22.0;
    let done = container(Space::new())
        .width(Length::Fixed(W * progress.clamp(0.0, 1.0)))
        .height(Length::Fixed(3.0))
        .style(move |_| rounded(if selected { theme::ACCENT_600 } else { theme::ACCENT_400 }, 2.0));
    container(done)
        .width(Length::Fixed(W))
        .height(Length::Fixed(3.0))
        .style(move |_| rounded(if selected { alpha(theme::GROUND, 0.18) } else { glass(0.16) }, 2.0))
        .into()
}

/// Where the saves stand, as an icon that is always there: a dim cloud before anything has synced,
/// then its colour and shape say how the last sync went, and the tooltip says it in words.
/// Information, not a destination: there is nothing to press, and the pad's walk along the bar
/// passes it by.
fn saves_icon<'a>(saves: Saves) -> Element<'a, Message> {
    let (sign, color, words) = match saves {
        Saves::Quiet => (lucide::cloud(), alpha(theme::INK, 0.5), "No saves synced yet"),
        Saves::Syncing => (lucide::refresh_cw(), theme::ACCENT_400, "Syncing saves"),
        Saves::InSync => (lucide::cloud(), theme::IN_SYNC, "Saves in sync"),
        Saves::Attention => (lucide::cloud_alert(), theme::WARNING, "Saves need a look"),
        Saves::Failed => (lucide::cloud_off(), theme::ERROR, "Save sync failed"),
    };
    let icon = container(sign.size(18.0).color(color)).center(Length::Fixed(ICON)).style(glass_pill);
    tip(icon, words.into())
}

/// `content` with `words` under it while the pointer rests on it.
fn tip<'a>(content: impl Into<Element<'a, Message>>, words: String) -> Element<'a, Message> {
    let words = container(text(words).size(13.0).color(theme::INK)).padding([6.0, 10.0]).style(|_| container::Style {
        background: Some(Background::Color(theme::GROUND)),
        border: Border { color: glass(0.16), width: 1.0, radius: 8.0.into() },
        ..Default::default()
    });
    tooltip(content, words, tooltip::Position::Bottom).gap(6).into()
}

/// Settings, as a gear: a utility rather than a place to browse.
fn gear<'a>(current: Tab, focused: bool) -> Element<'a, Message> {
    let selected = current == Tab::Settings;
    button(container(Tab::Settings.icon().size(18.0)).center(Length::Fill))
        .width(Length::Fixed(ICON))
        .height(Length::Fixed(ICON))
        .padding(0)
        .on_press(Message::TabSelected(Tab::Settings))
        .style(pill_style(selected, selected && focused, true))
        .into()
}

/// A pad button's letter in a ring — the button that does a thing, set beside the thing. `solid`
/// for the one on a filled button, where a ring alone would vanish into the fill.
pub fn glyph<'a>(letter: &'a str, solid: bool) -> Element<'a, Message> {
    let size = if solid { 24.0 } else { 20.0 };
    container(text(letter).size(if solid { 12.0 } else { 11.0 }).font(theme::BOLD).color(if solid { theme::ACCENT_800 } else { theme::INK }))
        .center(Length::Fixed(size))
        .style(move |_| container::Style {
            background: solid.then_some(Background::Color(Color::WHITE)),
            border: Border { color: if solid { Color::TRANSPARENT } else { alpha(theme::INK, 0.7) }, width: if solid { 0.0 } else { 1.5 }, radius: (size / 2.0).into() },
            ..Default::default()
        })
        .into()
}

/// The quiet line at a page's foot saying what the pad's buttons do there: each button's glyph and
/// its words, at the right, lined up under the header's icons.
pub fn hint_line<'a>(hints: Vec<(&'a str, String)>) -> Element<'a, Message> {
    let mut line = row![Space::new().width(Length::Fill)].spacing(22).align_y(Alignment::Center);
    for (button, words) in hints {
        line = line.push(row![glyph(button, false), text(words).size(13.0).color(theme::TEXT_SECONDARY)].spacing(8).align_y(Alignment::Center));
    }
    container(line).padding([0.0, MARGIN]).center_y(Length::Fixed(44.0)).into()
}

/// The glass the bar's pill and icons stand in: faint, with a faint edge.
fn glass_pill(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(glass(0.07))),
        border: Border { color: glass(0.12), width: 1.0, radius: 999.0.into() },
        ..Default::default()
    }
}

/// A plain fill of `color` with its corners rounded by `radius`: a meter's track and its bar.
fn rounded(color: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        border: Border { radius: radius.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A pill of the bar. Selected, it is filled with the brightest ink and its words go dark; under the
/// pad's cursor it is ringed in the accent too, so "the bar has the cursor" reads from a couch.
/// `chip` gives it the glass the stand-alone icons sit in; the pill's own places are bare inside it.
fn pill_style(selected: bool, lit: bool, chip: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let fill = match (selected, hovered, chip) {
            (true, _, _) => Some(theme::INK),
            (false, true, _) => Some(glass(0.12)),
            (false, false, true) => Some(glass(0.07)),
            (false, false, false) => None,
        };
        let edge = match (lit, chip && !selected) {
            (true, _) => Border { color: theme::ACCENT_400, width: 2.0, radius: 999.0.into() },
            (false, true) => Border { color: glass(0.12), width: 1.0, radius: 999.0.into() },
            (false, false) => Border { radius: 999.0.into(), ..Default::default() },
        };
        button::Style {
            background: fill.map(Background::Color),
            text_color: if selected { theme::GROUND } else { alpha(theme::INK, 0.82) },
            border: edge,
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

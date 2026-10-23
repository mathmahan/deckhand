//! The Saves page — a port of the Kotlin app's `SavesView`: one game's save states and in-game saves
//! as RomM holds them, reached from the game's detail page.
//!
//! Two tabs, because a state and a battery save are different things and were never meant to be
//! walked as one list: a state is a snapshot an emulator took and usually carries a picture of the
//! moment; a battery save is the game's own file. On the left, the tab's records, newest first; on the
//! right, the one under the cursor — its screenshot where one exists, and what RomM records about it.
//! Nothing here is invented: no playtime, no in-game location, since RomM's records carry neither.
//!
//! Each row says whether this device holds that version ("on this device") and, for a RetroArch
//! state, which of RetroArch's slots it is here — the one to load it from. One in no slot can be put
//! into one.
//!
//! Drawn on the game page's backdrop, the game's picture and light behind it: the tabs on the
//! Library's glass strip, walked by the triggers as well as Left and Right, the records as glass
//! rows lifted on the ink ring under the cursor, and the one under it on a glass card.

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::covers::Covers;
use crate::downloads::human_bytes;
use crate::game_saves::{GameSaves, SavesFocus, Tab};
use crate::library::Game;
use crate::romm::models::SaveAsset;
use crate::theme::{self, alpha, glass};
use crate::ui::home::truncate;

/// A row's height and the gap under it — fixed, so the cursor's scroll offset is computed.
const ROW_H: f32 = 72.0;
const ROW_GAP: f32 = 8.0;
const THUMB_W: f32 = 78.0;
const THUMB_H: f32 = 52.0;
/// How tall the preview's screenshot is drawn — and so the size the art is fetched at.
pub const PREVIEW_H: f32 = 270.0;
const LIST_W: f32 = 480.0;

pub fn list_id() -> widget::Id {
    widget::Id::new("saves-list")
}

/// Where the list scrolls for the cursor on `row`: that row a quarter of the list down.
pub fn scroll_offset(row: usize, list_h: f32) -> f32 {
    (row as f32 * (ROW_H + ROW_GAP) - list_h / 4.0).max(0.0)
}

/// A record's screenshot as a URL the art cache knows, when it has one.
pub fn screenshot_url(record: &SaveAsset, host: &str) -> Option<String> {
    let path = record.screenshot.as_ref()?.download_path.as_deref().filter(|p| !p.trim().is_empty())?;
    Some(if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}/{}", host.trim_end_matches('/'), path.trim_start_matches('/'))
    })
}

pub struct SavesContext<'a> {
    pub game: &'a Game,
    /// None while RomM is being asked; the error when it could not be.
    pub saves: Option<&'a Result<std::sync::Arc<GameSaves>, String>>,
    pub focus: &'a SavesFocus,
    pub covers: &'a Covers,
    pub host: &'a str,
}

pub fn saves_view<'a>(ctx: &SavesContext<'a>) -> Element<'a, Message> {
    let margin = crate::ui::tab_bar::MARGIN;
    let loaded = ctx.saves.and_then(|r| r.as_ref().ok());
    let count = |tab: Tab| loaded.map(|s| s.list(tab).len());

    let back = button(text(format!("←  {}", truncate(&ctx.game.title, 60))).size(theme::text_size::BODY_MEDIUM).color(alpha(theme::INK, 0.72)))
        .padding([4, 2])
        .on_press(Message::Back)
        .style(crate::ui::style::link);
    let top = column![
        back,
        Space::new().height(Length::Fixed(10.0)),
        row![crate::ui::home::tag("SAVES".into(), true), text("AS ROMM HOLDS THEM").size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82))]
            .spacing(12)
            .align_y(Alignment::Center),
        Space::new().height(Length::Fixed(10.0)),
        text("Saves & states").size(40.0).font(theme::DISPLAY).color(theme::INK),
        Space::new().height(Length::Fixed(18.0)),
        tabs(ctx.focus.tab, count(Tab::States), count(Tab::Saves)),
    ]
    .padding(Padding { top: 0.0, right: margin, bottom: 18.0, left: margin });

    let body: Element<Message> = match ctx.saves {
        None => note("Asking RomM…"),
        Some(Err(why)) => note(&format!("Couldn't read this game's saves from RomM: {why}")),
        Some(Ok(saves)) => {
            let list = saves.list(ctx.focus.tab);
            if list.is_empty() {
                note(match ctx.focus.tab {
                    Tab::States => "No save states on RomM for this game yet.",
                    Tab::Saves => "No in-game saves on RomM for this game yet.",
                })
            } else {
                let cursor = ctx.focus.row(list.len());
                let mut rows = column![].spacing(ROW_GAP);
                for (i, record) in list.iter().enumerate() {
                    rows = rows.push(list_row(i, record, saves, ctx, i == cursor));
                }
                row![
                    scrollable(column![rows, Space::new().height(Length::Fixed(160.0))].padding(Padding { right: 14.0, ..Padding::ZERO }))
                        .id(list_id())
                        .on_scroll(|_| Message::Scrolled)
                        .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
                        .width(Length::Fixed(LIST_W))
                        .height(Length::Fill),
                    preview(&list[cursor], saves, ctx),
                ]
                .spacing(24)
                .into()
            }
        }
    };

    let press = match ctx.focus.tab {
        Tab::States => "Put into a slot",
        Tab::Saves => "Restore",
    };
    let mut hints = Vec::new();
    if loaded.is_some_and(|s| !s.list(ctx.focus.tab).is_empty()) {
        hints.push(("A", press.to_string()));
    }
    hints.push(("B", "Back".to_string()));
    column![top, container(body).padding([0.0, margin]).height(Length::Fill), crate::ui::tab_bar::hint_line(hints)].into()
}

fn note<'a>(message: &str) -> Element<'a, Message> {
    container(text(message.to_string()).size(17.0).color(alpha(theme::INK, 0.82)))
        .padding(Padding { top: 12.0, ..Padding::ZERO })
        .into()
}

/// The two tabs, on the glass strip the Library's filters stand in, with the triggers that walk
/// them — as Left and Right do from the list.
fn tabs<'a>(shown: Tab, states: Option<usize>, saves: Option<usize>) -> Element<'a, Message> {
    use crate::ui::browse::{chip, key, strip};
    let label = |words: &str, count: Option<usize>| match count {
        Some(n) => format!("{words}  {n}"),
        None => words.to_string(),
    };
    let pills = row![
        container(key("L2")).padding([0, 4]),
        chip(label("Save states", states), shown == Tab::States, false, Message::SavesTab(Tab::States)),
        chip(label("In-game saves", saves), shown == Tab::Saves, false, Message::SavesTab(Tab::Saves)),
        container(key("R2")).padding([0, 4]),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    strip(pills)
}

/// What a record is called on the page: a RetroArch state by the slot it is here, anything else by
/// its own name with RomM's history stamp taken off.
fn title(record: &SaveAsset, saves: &GameSaves) -> String {
    match saves.slots.get(&record.id) {
        Some(slot) => slot.label(),
        None => crate::sync::canonical(&record.file_name),
    }
}

/// When RomM last saw it change, in UTC, to the minute.
fn when(record: &SaveAsset) -> String {
    record
        .updated_at
        .as_deref()
        .and_then(crate::sync::parse_timestamp)
        .map(|t| {
            let stamp = crate::sync::format_timestamp(t);
            format!("{} {}", &stamp[..10], &stamp[11..16])
        })
        .unwrap_or_else(|| "—".to_string())
}

/// A record as a glass row, lifted on the ink ring under the cursor: a state's picture small, its
/// name and what made it, and when — with whether this device holds that version.
fn list_row<'a>(index: usize, record: &SaveAsset, saves: &GameSaves, ctx: &SavesContext<'a>, lit: bool) -> Element<'a, Message> {
    let here = saves.on_device.contains(&record.id);
    let detail = [
        record.emulator.clone().filter(|e| !e.is_empty()),
        record.file_size_bytes.map(|b| human_bytes(b.max(0) as u64)),
        saves.other_core.contains(&record.id).then(|| "won't load here".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("  ·  ");

    let mut line = row![].spacing(14).align_y(Alignment::Center);
    // States usually carry a picture; a row shows it small, the preview large.
    if ctx.focus.tab == Tab::States {
        line = line.push(thumbnail(screenshot_url(record, ctx.host).and_then(|u| ctx.covers.handle(&u).cloned())));
    }
    line = line.push(
        column![
            text(truncate(&title(record, saves), 36)).size(16.0).font(theme::SEMIBOLD).color(theme::INK).wrapping(Wrapping::None),
            text(detail).size(13.0).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None),
        ]
        .spacing(3)
        .width(Length::Fill),
    );
    let mut right = column![text(when(record)).size(13.0).color(alpha(theme::INK, 0.8)).wrapping(Wrapping::None)].spacing(6).align_x(Alignment::End);
    if here {
        right = right.push(text("ON THIS DEVICE").size(10.0).font(theme::BOLD).color(theme::ACCENT_400));
    }
    line = line.push(right);

    // Centred in the row's height, which a thumbnail would otherwise be the only thing to set.
    button(container(line).height(Length::Fill).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 14])
        .on_press(Message::SavesRowClicked(index))
        .style(crate::ui::style::glass_row(lit, 14.0))
        .into()
}

fn thumbnail<'a>(handle: Option<image::Handle>) -> Element<'a, Message> {
    let inner: Element<Message> = match handle {
        // Whole, not cropped: screenshots come in every shape a console drew (8:7, 3:2, 4:3), and
        // a crop leans on the renderer clipping the image, which tiny-skia does not.
        Some(handle) => image(handle).content_fit(ContentFit::Contain).width(Length::Fixed(THUMB_W)).height(Length::Fixed(THUMB_H)).into(),
        None => Space::new().width(Length::Fixed(THUMB_W)).height(Length::Fixed(THUMB_H)).into(),
    };
    container(inner)
        .width(Length::Fixed(THUMB_W))
        .height(Length::Fixed(THUMB_H))
        .style(|_| container::Style {
            background: Some(Background::Color(alpha(theme::GROUND, 0.6))),
            border: Border { color: glass(0.10), width: 1.0, radius: 6.0.into() },
            ..Default::default()
        })
        .clip(true)
        .into()
}

/// The record under the cursor, on a card of smoked glass — it stands on the game's picture, which
/// would show through clear glass and fight its own — with its picture, and what RomM records about
/// it.
fn preview<'a>(record: &SaveAsset, saves: &GameSaves, ctx: &SavesContext<'a>) -> Element<'a, Message> {
    let mut panel = column![].spacing(18).width(Length::Fill);

    // A state keeps the same picture box whether or not it has a picture, so walking the list does
    // not make the facts jump; an in-game save carries none, and gets none unless RomM has one.
    let shot = screenshot_url(record, ctx.host).and_then(|u| ctx.covers.handle(&u).cloned());
    if ctx.focus.tab == Tab::States || shot.is_some() {
        let inner: Element<Message> = match shot {
            Some(handle) => image(handle).content_fit(ContentFit::Contain).height(Length::Fixed(PREVIEW_H)).into(),
            None => text(if record.screenshot.is_some() { "Loading the screenshot…" } else { "No screenshot was saved with this state." })
                .size(theme::text_size::BODY_MEDIUM)
                .color(theme::TEXT_SECONDARY)
                .into(),
        };
        panel = panel.push(
            container(inner)
                .width(Length::Fill)
                .height(Length::Fixed(PREVIEW_H))
                .center_x(Length::Fill)
                .center_y(Length::Fixed(PREVIEW_H))
                .style(|_| container::Style {
                    background: Some(Background::Color(alpha(theme::GROUND, 0.6))),
                    border: Border { radius: 12.0.into(), ..Default::default() },
                    ..Default::default()
                })
                .clip(true),
        );
    }

    // The facts as a strip, the way the detail page states its own — the file name on a line of its
    // own, since it is the one long enough to need it.
    let here = saves.on_device.contains(&record.id);
    let mut cells: Vec<(&str, String)> = vec![
        ("SAVED", format!("{} UTC", when(record))),
        ("EMULATOR", record.emulator.clone().filter(|e| !e.is_empty()).unwrap_or_else(|| "not recorded".into())),
        ("SIZE", record.file_size_bytes.map(|b| human_bytes(b.max(0) as u64)).unwrap_or_else(|| "—".into())),
    ];
    if let Some(slot) = saves.slots.get(&record.id) {
        cells.push(("RETROARCH SLOT", slot.number()));
    }
    // Another core's state is never in a slot here: say why it has none.
    if saves.other_core.contains(&record.id) {
        let core = saves.core.clone().unwrap_or_else(|| "this device's core".into());
        cells.push(("LOADS HERE", format!("no — {core} runs it here")));
    }
    if let Some(slot) = record.slot.as_deref().filter(|s| !s.is_empty() && ctx.focus.tab == Tab::Saves) {
        cells.push(("SLOT", slot.to_string()));
    }
    cells.push(("THIS DEVICE", if here { "has this version".into() } else { "doesn't have it".into() }));
    if let Some(others) = saves.elsewhere.get(&record.id) {
        cells.push(("ALSO ON", others.join(", ")));
    }

    let fact = |label: &'static str, value: String| {
        column![text(label).size(11.0).font(theme::BOLD).color(alpha(theme::INK, 0.55)), text(value).size(16.0).color(theme::INK)].spacing(4)
    };
    let mut strip = row![].spacing(32);
    for (label, value) in cells {
        strip = strip.push(fact(label, value));
    }
    // Wrapped, since how many facts there are depends on the record and the page on the scale.
    panel = panel.push(fact("FILE", record.file_name.clone())).push(strip.wrap().vertical_spacing(14.0));

    let card = container(panel).padding(18).width(Length::Fill).style(|_| container::Style {
        background: Some(Background::Color(alpha(theme::GROUND, 0.82))),
        border: Border { color: glass(0.10), width: 1.0, radius: 18.0.into() },
        ..Default::default()
    });
    // A scroll as insurance only: at the default scale it all fits, at the largest it may not.
    scrollable(column![card, Space::new().height(Length::Fixed(16.0))]).on_scroll(|_| Message::Scrolled).width(Length::Fill).height(Length::Fill).into()
}

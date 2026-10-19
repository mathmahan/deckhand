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

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::covers::Covers;
use crate::downloads::human_bytes;
use crate::game_saves::{GameSaves, SavesFocus, Tab};
use crate::library::Game;
use crate::romm::models::SaveAsset;
use crate::theme;
use crate::ui::home::truncate;

/// A row's height and the gap under it — fixed, so the cursor's scroll offset is computed.
const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 6.0;
const THUMB_W: f32 = 72.0;
const THUMB_H: f32 = 48.0;
/// How tall the preview's screenshot is drawn — and so the size the art is fetched at.
pub const PREVIEW_H: f32 = 270.0;
const LIST_W: f32 = 460.0;

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
    let loaded = ctx.saves.and_then(|r| r.as_ref().ok());
    let count = |tab: Tab| loaded.map(|s| s.list(tab).len());

    let back = button(text(format!("←  {}", truncate(&ctx.game.title, 60))).size(theme::text_size::BODY_MEDIUM))
        .padding([4, 2])
        .on_press(Message::Back)
        .style(crate::ui::style::link);
    let heading = column![
        back,
        text("Saves & states").size(theme::text_size::HEADLINE_SMALL).font(theme::MEDIUM).color(theme::TEXT_PRIMARY),
    ]
    .spacing(6);
    let tabs = row![
        tab_button(Tab::States, "Save states", count(Tab::States), ctx.focus.tab == Tab::States),
        tab_button(Tab::Saves, "In-game saves", count(Tab::Saves), ctx.focus.tab == Tab::Saves),
    ]
    .spacing(28);

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
                    scrollable(column![rows, Space::new().height(Length::Fixed(160.0))])
                        .id(list_id())
                        .on_scroll(|_| Message::Scrolled)
                        .width(Length::Fixed(LIST_W))
                        .height(Length::Fill),
                    preview(&list[cursor], saves, ctx),
                ]
                .spacing(28)
                .into()
            }
        }
    };

    let hint = text(match ctx.focus.tab {
        Tab::States => "Up / Down to look through  ·  Enter or A puts a state into a slot  ·  Left / Right for the other tab  ·  Esc goes back",
        Tab::Saves => "Up / Down to look through  ·  Enter or A restores an in-game save  ·  Left / Right for the other tab  ·  Esc goes back",
    })
        .size(theme::text_size::LABEL_MEDIUM)
        .color(theme::TEXT_MUTED);

    column![heading, tabs, hint, body]
        .spacing(14)
        .padding(Padding { top: 16.0, right: 28.0, bottom: 0.0, left: 28.0 })
        .into()
}

fn note<'a>(message: &str) -> Element<'a, Message> {
    container(text(message.to_string()).size(theme::text_size::BODY_LARGE).color(theme::TEXT_SECONDARY))
        .padding(Padding { top: 24.0, ..Padding::ZERO })
        .into()
}

/// A tab: its label, how many it holds, and an underline when it is the one shown.
fn tab_button<'a>(tab: Tab, label: &str, count: Option<usize>, active: bool) -> Element<'a, Message> {
    let label = match count {
        Some(n) => format!("{label}  {n}"),
        None => label.to_string(),
    };
    let underline = container(Space::new().height(Length::Fixed(3.0)).width(Length::Fixed(if active { 112.0 } else { 0.0 })))
        .style(|_| container::Style { background: Some(Background::Color(theme::ACCENT)), border: Border { radius: 2.0.into(), ..Default::default() }, ..Default::default() });
    button(
        column![
            text(label)
                .size(theme::text_size::BODY_LARGE)
                .font(if active { theme::BOLD } else { theme::REGULAR })
                .color(if active { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY }),
            underline,
        ]
        .spacing(6),
    )
    .padding([4, 0])
    .on_press(Message::SavesTab(tab))
    .style(crate::ui::style::bare)
    .into()
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
            text(truncate(&title(record, saves), 40))
                .size(theme::text_size::BODY_LARGE)
                .font(if lit { theme::MEDIUM } else { theme::REGULAR })
                .color(if lit { theme::TEXT_PRIMARY } else { theme::TEXT_BODY })
                .wrapping(Wrapping::None),
            text(detail).size(theme::text_size::LABEL_MEDIUM).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None),
        ]
        .spacing(2)
        .width(Length::Fill),
    );
    line = line.push(
        column![
            text(when(record)).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None),
            text(if here { "ON THIS DEVICE" } else { "" }).size(10.0).font(theme::BOLD).color(theme::ACCENT),
        ]
        .spacing(4)
        .align_x(Alignment::End),
    );

    // Centred in the row's height, which a thumbnail would otherwise be the only thing to set.
    button(container(line).height(Length::Fill).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 12])
        .on_press(Message::SavesRowClicked(index))
        .style(crate::ui::style::row(lit, 12.0))
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
            background: Some(Background::Color(theme::SURFACE_VARIANT)),
            border: Border { color: theme::BORDER, width: 1.0, radius: 6.0.into() },
            ..Default::default()
        })
        .clip(true)
        .into()
}

/// The record under the cursor: its picture, and what RomM records about it.
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
                .color(theme::TEXT_MUTED)
                .into(),
        };
        panel = panel.push(
            container(inner)
                .width(Length::Fill)
                .height(Length::Fixed(PREVIEW_H))
                .center_x(Length::Fill)
                .center_y(Length::Fixed(PREVIEW_H))
                .style(|_| container::Style {
                    background: Some(Background::Color(theme::SURFACE_VARIANT)),
                    border: Border { color: theme::BORDER, width: 1.0, radius: 10.0.into() },
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
        column![
            text(label).size(10.0).font(theme::BOLD).color(theme::TEXT_MUTED),
            text(value).size(theme::text_size::BODY_MEDIUM).color(theme::TEXT_PRIMARY),
        ]
        .spacing(3)
    };
    let mut strip = row![].spacing(32);
    for (label, value) in cells {
        strip = strip.push(fact(label, value));
    }
    // Wrapped, since how many facts there are depends on the record and the page on the scale.
    panel = panel.push(fact("FILE", record.file_name.clone())).push(strip.wrap().vertical_spacing(14.0));

    // A scroll as insurance only: at the default scale it all fits, at the largest it may not.
    scrollable(panel).on_scroll(|_| Message::Scrolled).width(Length::Fill).height(Length::Fill).into()
}

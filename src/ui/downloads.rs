//! The Downloads page — a port of `browse/ui/DownloadsView.kt`: everything this device has pulled
//! down, and everything still coming.
//!
//! It exists because "what is on my Deck?" had no answer anywhere else. The per-system grids carry
//! a badge per tile but show one system at a time, so auditing a library meant walking all of them;
//! the Storage section in Settings gives a total but never says what makes it up. This is the one
//! page where the whole on-disk footprint is visible and removable in one pass — which is also what
//! makes it the natural home for the in-flight list, since a stalled or failed download is exactly
//! the thing you come looking for when the space does not add up.
//!
//! The two sections are **one** list to navigate, with a header drawn before the first row of each,
//! because they read as one page and that is what lets plain Up/Down be the whole of its
//! navigation.

use iced::border;
use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Element, Length, Padding, Shadow};

use crate::app::Message;
use crate::downloads::{human_bytes, Downloads, SyncState};
use crate::focus::DownloadsFocus;
use crate::library::Library;
use crate::theme;
use crate::ui::home::truncate;

/// A row's height, and the gap under it. Uniform — including the band a progress bar or a failure
/// reason is drawn in — so the cursor's scroll offset is computed rather than measured, the same
/// way the Settings page's cards work.
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 4.0;

/// A section header's height, including the space above it.
const HEADER_H: f32 = 44.0;

pub fn downloads_id() -> widget::Id {
    widget::Id::new("downloads-page")
}

/// One line of the page.
///
/// `bytes` is None while a download is in flight: the final size is not known until it lands, and
/// showing the bytes so far reads as the game being that small.
pub struct Entry<'a> {
    pub rom_id: i64,
    /// The game, with which build or disc of it when that is not all of it.
    pub title: String,
    pub system: &'a str,
    pub state: SyncState,
    pub bytes: Option<u64>,
    /// What else is worth knowing about a game that is here: pinned, an update waiting.
    pub note: Option<String>,
}

impl Entry<'_> {
    /// Whether this row sits in the lower section — on disk rather than on its way.
    fn on_device(&self) -> bool {
        self.state.is_here()
    }
}

/// A queue item as a row names it.
fn named(item: &crate::downloads::QueueItem) -> String {
    match (&item.build, &item.label) {
        (Some(build), Some(part)) => format!("{} — {build}, {part}", item.title),
        (Some(build), None) => format!("{} — {build}", item.title),
        (None, Some(part)) => format!("{} — {part}", item.title),
        (None, None) => item.title.clone(),
    }
}

/// The page's rows: what is coming, then what is here.
///
/// In-flight first because it is the volatile half, and the half you opened this page to check.
/// Within it, running and queued in the order asked for, then whatever failed. What is on disk
/// follows, largest first — the order that answers "which one do I remove".
///
/// The in-flight rows are built from the queue rather than from the library on purpose: a queue
/// resumed from the last run can hold a game whose system has not been loaded yet, and it still has
/// to be sayable. The title travels with the queue item for exactly that reason.
pub fn entries<'a>(library: &'a Library, downloads: &'a Downloads) -> Vec<Entry<'a>> {
    let mut rows = Vec::new();
    for (item, progress) in downloads.in_progress() {
        let state = match downloads.state(item.rom_id) {
            busy @ SyncState::Unpacking(_) => busy,
            _ => SyncState::Downloading(progress),
        };
        rows.push(Entry {
            rom_id: item.rom_id,
            title: named(item),
            system: library.system_name(&item.system_id),
            state,
            bytes: None,
            note: None,
        });
    }
    for (item, why) in downloads.failures() {
        rows.push(Entry {
            rom_id: item.rom_id,
            title: named(item),
            system: library.system_name(&item.system_id),
            state: SyncState::Failed(why.to_string()),
            bytes: None,
            note: None,
        });
    }
    for installed in downloads.installed_by_size() {
        let state = downloads.state(installed.rom_id);
        let note = [
            installed.pinned.then_some("pinned — kept when room is made"),
            (state == SyncState::UpdateAvailable).then_some("update waiting on RomM"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ");
        rows.push(Entry {
            rom_id: installed.rom_id,
            title: installed.name(),
            system: library.system_name(&installed.system_id),
            state: if state.is_here() { state } else { SyncState::Installed },
            bytes: Some(installed.bytes),
            note: (!note.is_empty()).then_some(note),
        });
    }
    rows
}

/// The corner badge a cover carries for what this device has of it, or None for a game that is
/// only on the server — which is most of them, and wants no mark at all.
///
/// Words rather than a glyph: a coloured dot in the corner of a cover is unambiguous only once you
/// have been told what it means, and the one place this is read from is across a room. The pill is
/// drawn over the art's bottom-left corner and clipped with it, so a narrow tile loses the end of
/// the word rather than pushing the layout about.
pub fn badge<'a>(state: &SyncState) -> Option<Element<'a, Message>> {
    let (label, fill, ink) = match state {
        SyncState::RemoteOnly => return None,
        SyncState::Downloading(progress) => {
            (format!("{}%", (progress * 100.0) as i32), theme::ACCENT, iced::Color::WHITE)
        }
        SyncState::Unpacking(progress) => (format!("UNPACKING {}%", (progress * 100.0) as i32), theme::ACCENT, iced::Color::WHITE),
        SyncState::Installed => ("ON DEVICE".to_string(), theme::ACCENT, iced::Color::WHITE),
        SyncState::UpdateAvailable => ("UPDATE".to_string(), theme::WARNING, iced::Color::BLACK),
        SyncState::Failed(_) => ("FAILED".to_string(), theme::ERROR, iced::Color::WHITE),
    };
    Some(
        container(
            text(label).size(9.0).font(theme::BOLD).color(ink).wrapping(Wrapping::None),
        )
        .padding([3, 7])
        .style(move |_| container::Style {
            background: Some(Background::Color(theme::alpha(fill, 0.92))),
            border: Border { radius: 5.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into(),
    )
}

/// How far down the page row `index` starts: every header above it, and every row before it.
///
/// There is always one header — whichever section the page opens with — and a second only where the
/// list actually crosses from one section to the other, which is the first on-disk row when
/// something is coming above it.
fn row_top(rows: &[Entry<'_>], index: usize) -> f32 {
    let split = rows.iter().position(Entry::on_device).filter(|at| *at > 0);
    let headers = 1 + usize::from(split.is_some_and(|at| at <= index));
    headers as f32 * HEADER_H + index as f32 * (ROW_H + ROW_GAP)
}

/// Where the list scrolls to for the cursor on row `index`: the row a quarter of the window down,
/// which keeps the rows after it in view as well as the ones before. The scrollable clamps the far
/// end.
pub fn scroll_offset(rows: &[Entry<'_>], index: usize, window_h: f32) -> f32 {
    (row_top(rows, index) - window_h / 4.0).max(0.0)
}

pub fn downloads_view<'a>(
    rows: Vec<Entry<'a>>,
    focus: &DownloadsFocus,
    installed_bytes: u64,
    roms_dir: &std::path::Path,
) -> Element<'a, Message> {
    let on_device = rows.iter().filter(|e| e.on_device()).count();
    let failed = rows.iter().filter(|e| matches!(e.state, SyncState::Failed(_))).count();
    let downloading = rows.len() - on_device - failed;
    let summary = [
        Some(format!("{on_device} on this device")),
        Some(format!("{} used", human_bytes(installed_bytes))),
        (downloading > 0).then(|| format!("{downloading} downloading")),
        (failed > 0).then(|| format!("{failed} failed")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("  ·  ");

    let heading = column![
        text("Downloads")
            .size(theme::text_size::HEADLINE_SMALL)
            .font(theme::MEDIUM)
            .color(theme::TEXT_PRIMARY),
        Space::new().height(Length::Fixed(4.0)),
        text(summary).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_MUTED),
        Space::new().height(Length::Fixed(2.0)),
        // The folder is named here and not only in Settings: this is the page you are on when you
        // want to know where the bytes went, and a path is the answer to that.
        text(truncate(&roms_dir.display().to_string(), 80))
            .size(theme::text_size::LABEL_MEDIUM)
            .color(theme::TEXT_SECONDARY)
            .wrapping(Wrapping::None),
    ]
    .padding(Padding { top: 20.0, right: 28.0, bottom: 10.0, left: 28.0 });

    if rows.is_empty() {
        return column![
            heading,
            container(
                text("Nothing downloaded yet — press Download on a game to fetch it.")
                    .size(theme::text_size::BODY_LARGE)
                    .color(theme::TEXT_SECONDARY),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        ]
        .into();
    }

    let cursor = focus.index(rows.len());
    let mut list = column![];
    for (i, entry) in rows.iter().enumerate() {
        if i == 0 {
            list = list.push(section_header(if entry.on_device() {
                "On this device"
            } else {
                "Downloading"
            }));
        } else if entry.on_device() && !rows[i - 1].on_device() {
            list = list.push(section_header("On this device"));
        }
        list = list.push(line(i, entry, i == cursor));
        list = list.push(Space::new().height(Length::Fixed(ROW_GAP)));
    }
    // Room under the last row, so it can scroll up to the same anchor as the rest instead of
    // stopping pinned to the bottom edge.
    list = list.push(Space::new().height(Length::Fixed(160.0)));

    column![
        heading,
        prompts(rows.get(cursor)),
        scrollable(list.padding([0, 28]))
            .id(downloads_id())
            .on_scroll(|_| Message::Scrolled)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .into()
}

/// What the two keys do on the row under the cursor.
///
/// Spelled out on the page rather than left to a tooltip, because Delete is the only key in this
/// app that removes anything — a key you have to discover is not one to put a deletion behind.
fn prompts<'a>(focused: Option<&Entry<'_>>) -> Element<'a, Message> {
    let hint = match focused.map(|e| &e.state) {
        Some(SyncState::Installed | SyncState::UpdateAvailable) => "Enter opens the game  ·  Del removes it from this device",
        Some(SyncState::Failed(_)) => "Enter tries again  ·  Del dismisses",
        _ => "Enter opens the game  ·  Del cancels",
    };
    container(text(hint).size(theme::text_size::LABEL_MEDIUM).color(theme::TEXT_MUTED))
        .padding(Padding { right: 28.0, bottom: 8.0, left: 28.0, ..Padding::ZERO })
        .into()
}

fn section_header<'a>(title: &'a str) -> Element<'a, Message> {
    container(
        text(title.to_uppercase())
            .size(theme::text_size::LABEL_MEDIUM)
            .font(theme::BOLD)
            .color(theme::TEXT_MUTED),
    )
    .height(Length::Fixed(HEADER_H))
    .align_y(iced::alignment::Vertical::Bottom)
    .padding(Padding { bottom: 8.0, ..Padding::ZERO })
    .into()
}

/// One row: the game, its system, what it is doing, and the one thing pressing it does.
fn line<'a>(index: usize, entry: &Entry<'a>, lit: bool) -> Element<'a, Message> {
    let downloading = match &entry.state {
        SyncState::Downloading(progress) | SyncState::Unpacking(progress) => Some(*progress),
        _ => None,
    };
    let failed = matches!(entry.state, SyncState::Failed(_));

    // The glowing left edge, as on the Settings cards — drawn clear on every other row rather than
    // left out, so the labels do not shift sideways as the cursor passes.
    let edge = container(Space::new().width(Length::Fixed(4.0)).height(Length::Fixed(28.0))).style(
        move |_| container::Style {
            background: lit.then_some(Background::Color(theme::ACCENT)),
            border: Border { radius: border::right(4.0), ..Default::default() },
            ..Default::default()
        },
    );

    let figure = match (&entry.state, entry.bytes) {
        (SyncState::Downloading(progress), _) => format!("{}%", (progress * 100.0) as i32),
        (SyncState::Unpacking(progress), _) => format!("unpack {}%", (progress * 100.0) as i32),
        (SyncState::Failed(_), _) => "failed".to_string(),
        (_, Some(bytes)) => human_bytes(bytes),
        _ => "—".to_string(),
    };

    let top = row![
        edge,
        text(truncate(&entry.title, 52))
            .size(theme::text_size::BODY_LARGE)
            .font(if lit { theme::MEDIUM } else { theme::REGULAR })
            .color(if lit { theme::TEXT_PRIMARY } else { theme::TEXT_BODY })
            .wrapping(Wrapping::None)
            .width(Length::Fill),
        text(entry.system)
            .size(theme::text_size::LABEL_LARGE)
            .color(theme::TEXT_SECONDARY)
            .wrapping(Wrapping::None),
        container(
            text(figure)
                .size(theme::text_size::LABEL_LARGE)
                .font(theme::MEDIUM)
                .color(if failed { theme::ERROR } else { theme::TEXT_SECONDARY })
                .wrapping(Wrapping::None),
        )
        .width(Length::Fixed(64.0))
        .align_right(Length::Fixed(64.0)),
        // A chip rather than a bare label: the mouse needs its own way to remove a game, since
        // Delete is a key the pointer does not have.
        remove_chip(entry.rom_id, &entry.state, lit),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .height(Length::Fixed(28.0));

    // The lower band of the row: the bar for something in flight, the reason for something that
    // stopped, and nothing at all for a game that is simply here. Fixed height either way, so
    // `row_top` stays exact.
    let under: Element<Message> = match (&entry.state, downloading) {
        // The bar carries the same job it does on a tile: a percentage that only moves every
        // 250 ms reads as static without something moving beside it.
        (_, Some(progress)) => progress_bar(progress),
        (SyncState::Failed(why), _) => text(truncate(why, 90))
            .size(theme::text_size::LABEL_MEDIUM)
            .color(theme::ERROR)
            .wrapping(Wrapping::None)
            .into(),
        (_, None) if entry.note.is_some() => text(truncate(entry.note.as_deref().unwrap_or_default(), 90))
            .size(theme::text_size::LABEL_MEDIUM)
            .color(if entry.state == SyncState::UpdateAvailable { theme::WARNING } else { theme::TEXT_MUTED })
            .wrapping(Wrapping::None)
            .into(),
        _ => Space::new().into(),
    };
    let has_under = downloading.is_some() || failed || entry.note.is_some();

    // Centred in the row as one block: the title alone for a game that is simply here, the title
    // over its bar or its note otherwise — that band starting where the title does.
    let mut body = column![top].padding(Padding { right: 14.0, ..Padding::ZERO });
    if has_under {
        body = body
            .push(Space::new().height(Length::Fixed(4.0)))
            .push(container(under).padding(Padding { left: 16.0, ..Padding::ZERO }));
    }
    button(container(body).height(Length::Fill).center_y(Length::Fill))
    .width(Length::Fill)
    .height(Length::Fixed(ROW_H))
    .padding(0)
    .on_press(Message::DownloadsClicked(index))
    .style(crate::ui::style::row(lit, 12.0))
    .into()
}

fn progress_bar<'a>(progress: f32) -> Element<'a, Message> {
    let filled = progress.clamp(0.0, 1.0);
    container(
        row![
            container(Space::new().height(Length::Fixed(4.0)))
                .width(Length::FillPortion((filled * 1000.0) as u16 + 1))
                .style(|_| container::Style {
                    background: Some(Background::Color(theme::ACCENT)),
                    border: Border { radius: 2.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            Space::new().width(Length::FillPortion(((1.0 - filled) * 1000.0) as u16 + 1)),
        ]
        .height(Length::Fixed(4.0)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(4.0))
    .style(|_| container::Style {
        background: Some(Background::Color(theme::alpha(theme::NEUTRAL_100, 0.12))),
        border: Border { radius: 2.0.into(), ..Default::default() },
        ..Default::default()
    })
    .into()
}

/// The chip naming what removing this row does — and doing it, for the pointer.
fn remove_chip<'a>(rom_id: i64, state: &SyncState, lit: bool) -> Element<'a, Message> {
    let label = match state {
        SyncState::Installed | SyncState::UpdateAvailable => "Remove",
        SyncState::Failed(_) => "Dismiss",
        _ => "Cancel",
    };
    button(
        text(label)
            .size(theme::text_size::LABEL_LARGE)
            .font(theme::MEDIUM)
            .wrapping(Wrapping::None),
    )
    .padding([5, 12])
    .on_press(Message::RemoveDownload(rom_id))
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: hovered.then_some(Background::Color(theme::alpha(theme::ERROR, 0.18))),
            text_color: if lit || hovered { theme::ERROR } else { theme::TEXT_SECONDARY },
            border: Border {
                color: if lit { theme::alpha(theme::ERROR, 0.6) } else { theme::BORDER },
                width: 1.0,
                radius: 9.0.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::{Installed, QueueItem};
    use crate::library::{Game, System};

    fn library() -> Library {
        Library::new(
            vec![
                System { id: "snes".into(), name: "Super Nintendo".into(), cover_ratio: 1.37 },
                System { id: "psx".into(), name: "PlayStation".into(), cover_ratio: 1.16 },
            ],
            vec![Game { rom_id: 1, system_id: "snes".into(), ..Default::default() }],
        )
    }

    /// Detached: a page test has no business reading or writing the real records.
    fn downloads() -> Downloads {
        Downloads::detached(std::env::temp_dir(), "https://romm.local")
    }

    #[test]
    fn what_is_coming_leads_and_what_is_on_disk_follows_largest_first() {
        let library = library();
        let mut downloads = downloads();
        downloads.enqueue(QueueItem { host: String::new(), rom_id: 1, system_id: "snes".into(), title: "Chrono".into(), ..Default::default() });
        downloads.start_next();
        downloads.progress(1, 0.25);
        downloads.enqueue(QueueItem { host: String::new(), rom_id: 2, system_id: "psx".into(), title: "Driver".into(), ..Default::default() });
        downloads.start_next();
        downloads.finish(2, Err("the download stopped early".into()));
        for (id, bytes) in [(3, 900u64), (4, 9000u64)] {
            downloads.finish(
                id,
                Ok(Installed {
                    host: String::new(),
                    rom_id: id,
                    title: format!("On disk {id}"),
                    system_id: "snes".into(),
                    platform_id: None,
                    dir: std::env::temp_dir(),
                    files: Vec::new(),
                    bytes,
                    ..Default::default()
                }),
            );
        }

        let rows = entries(&library, &downloads);
        assert_eq!(
            rows.iter().map(|e| e.rom_id).collect::<Vec<_>>(),
            vec![1, 2, 4, 3],
            "running, then failed, then on disk biggest first"
        );
        assert_eq!(rows[0].state, SyncState::Downloading(0.25));
        assert_eq!(rows[0].system, "Super Nintendo", "resolved through the library");
        assert_eq!(rows[1].system, "PlayStation");
        assert!(rows[0].bytes.is_none(), "a part-downloaded size reads as the game being that small");
        assert_eq!(rows[2].bytes, Some(9000));
    }

    #[test]
    fn a_queued_game_the_library_has_never_heard_of_is_still_named() {
        // What a queue resumed from the last run looks like before the library has loaded.
        let mut downloads = downloads();
        downloads.enqueue(QueueItem {
            host: String::new(),
            rom_id: 99,
            system_id: "dreamcast".into(),
            title: "Shenmue".into(),
            ..Default::default()
        });
        let empty = Library::default();
        let rows = entries(&empty, &downloads);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Shenmue");
        assert_eq!(rows[0].system, "dreamcast", "the slug, since there is no system to name it");
    }

    #[test]
    fn every_row_starts_below_the_header_of_the_section_it_is_in() {
        let library = library();
        let mut downloads = downloads();
        downloads.enqueue(QueueItem { host: String::new(), rom_id: 1, system_id: "snes".into(), title: "Coming".into(), ..Default::default() });
        downloads.finish(
            2,
            Ok(Installed {
                host: String::new(),
                rom_id: 2,
                title: "Here".into(),
                system_id: "snes".into(),
                platform_id: None,
                dir: std::env::temp_dir(),
                files: Vec::new(),
                bytes: 1,
                ..Default::default()
            }),
        );
        let rows = entries(&library, &downloads);

        assert_eq!(row_top(&rows, 0), HEADER_H, "under the Downloading header");
        assert_eq!(
            row_top(&rows, 1),
            2.0 * HEADER_H + ROW_H + ROW_GAP,
            "and the second section's header is between them"
        );
        assert_eq!(scroll_offset(&rows, 0, 800.0), 0.0, "the first row never scrolls the page");
    }
}

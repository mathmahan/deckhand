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
//!
//! Drawn as Home and Settings are, on Home's ground: how much is here where Home has its spotlight,
//! and each game as a glass row with its cover, the one under the cursor lifted on the ink ring.

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, ContentFit, Element, Length, Padding, Shadow};

use crate::app::Message;
use crate::covers::Covers;
use crate::downloads::{human_bytes, Downloads, SyncState};
use crate::focus::DownloadsFocus;
use crate::library::Library;
use crate::theme::{self, alpha, glass};
use crate::ui::home::truncate;

/// A row's height, and the gap under it. Uniform — including the band a progress bar or a failure
/// reason is drawn in — so the cursor's scroll offset is computed rather than measured, the same
/// way the Settings page's cards work.
const ROW_H: f32 = 76.0;
const ROW_GAP: f32 = 8.0;

/// The cover at a row's left.
const COVER: (f32, f32) = (40.0, 56.0);

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
    /// Its cover, when the library has the game.
    pub cover: Option<&'a str>,
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
    let cover = |rom_id: i64| library.game(rom_id).or_else(|| library.game_of_build(rom_id)).and_then(|g| g.cover_url.as_deref());
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
            cover: cover(item.rom_id),
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
            cover: cover(item.rom_id),
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
            cover: cover(installed.rom_id),
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
    covers: &Covers,
) -> Element<'a, Message> {
    let margin = crate::ui::tab_bar::MARGIN;
    let top = container(heading(&rows, installed_bytes, roms_dir)).padding(Padding { top: 18.0, right: margin, bottom: 18.0, left: margin });

    if rows.is_empty() {
        return column![top, Space::new().height(Length::Fill)].into();
    }

    let cursor = focus.index(rows.len());
    let mut list = column![];
    for (i, entry) in rows.iter().enumerate() {
        if i == 0 {
            list = list.push(section_header(if entry.on_device() { "On this device" } else { "Downloading" }));
        } else if entry.on_device() && !rows[i - 1].on_device() {
            list = list.push(section_header("On this device"));
        }
        list = list.push(line(i, entry, i == cursor, covers));
        list = list.push(Space::new().height(Length::Fixed(ROW_GAP)));
    }
    // Room under the last row, so it can scroll up to the same anchor as the rest instead of
    // stopping pinned to the bottom edge.
    list = list.push(Space::new().height(Length::Fixed(160.0)));

    column![
        top,
        scrollable(list.padding([0.0, margin]))
            .id(downloads_id())
            .on_scroll(|_| Message::Scrolled)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
            .width(Length::Fill)
            .height(Length::Fill),
        hints(rows.get(cursor)),
    ]
    .into()
}

/// The page's heading, where Home has its spotlight: what is on its way or here, where it went —
/// the folder is named here and not only in Settings, since this is the page you are on when you
/// want to know where the bytes went — and how much room it all takes, set large at the right.
fn heading<'a>(rows: &[Entry<'_>], installed_bytes: u64, roms_dir: &std::path::Path) -> Element<'a, Message> {
    let on_device = rows.iter().filter(|e| e.on_device()).count();
    let failed = rows.iter().filter(|e| matches!(e.state, SyncState::Failed(_))).count();
    let downloading = rows.len() - on_device - failed;
    let games = |n: usize| if n == 1 { "1 game".to_string() } else { format!("{n} games") };
    let eyebrow = [
        (downloading > 0).then(|| format!("{downloading} DOWNLOADING")),
        (failed > 0).then(|| format!("{failed} FAILED")),
        Some(format!("{on_device} ON THIS DEVICE")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("  ·  ");
    let (headline, under) = match (downloading, on_device) {
        (0, 0) if failed == 0 => ("Nothing downloaded yet".to_string(), "Press Download on a game to fetch it.".to_string()),
        (0, 0) => (format!("{} didn't download", games(failed)), "Try again, or dismiss it.".to_string()),
        (0, n) => (format!("{} on this device", games(n)), "Largest first: the ones to remove when room is short.".to_string()),
        (n, _) => (format!("Downloading {}", games(n)), "Open a game while it comes down, or cancel it here.".to_string()),
    };
    let left = column![
        row![crate::ui::home::tag("DOWNLOADS".into(), true), text(eyebrow).size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82))]
            .spacing(12)
            .align_y(Alignment::Center),
        text(headline).size(40.0).font(theme::DISPLAY).color(theme::INK),
        text(under).size(16.0).color(alpha(theme::INK, 0.82)),
        text(truncate(&roms_dir.display().to_string(), 80)).size(13.0).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None),
    ]
    .spacing(10)
    .width(Length::Fill);
    let used = column![
        text(human_bytes(installed_bytes)).size(40.0).font(theme::DISPLAY).color(theme::INK),
        text("used on this device").size(13.0).color(theme::TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::End);
    row![left, used].spacing(40).align_y(Alignment::End).into()
}

/// What A and Delete do on the row under the cursor.
///
/// Delete spelled out at the foot rather than left to be found, because it is the only key in this
/// app that removes anything — a key you have to discover is not one to put a deletion behind. The
/// pad has none: the game's own page removes it.
fn hints<'a>(focused: Option<&Entry<'_>>) -> Element<'a, Message> {
    let (press, delete) = match focused.map(|e| &e.state) {
        Some(SyncState::Installed | SyncState::UpdateAvailable) => ("Open the game", "Remove"),
        Some(SyncState::Failed(_)) => ("Try again", "Dismiss"),
        _ => ("Open the game", "Cancel"),
    };
    crate::ui::tab_bar::hint_line(vec![("A", press.to_string()), ("Del", delete.to_string())])
}

fn section_header<'a>(title: &'a str) -> Element<'a, Message> {
    container(text(title.to_uppercase()).size(12.0).font(theme::BOLD).color(alpha(theme::INK, 0.6)))
        .height(Length::Fixed(HEADER_H))
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(Padding { bottom: 10.0, ..Padding::ZERO })
        .into()
}

/// One row: the game's cover, its name, its system and what else is worth knowing — or the bar of
/// something in flight, the reason for something that stopped — its size or how far it has got,
/// and the one thing removing it does. A glass row, lifted on the ink ring under the cursor.
fn line<'a>(index: usize, entry: &Entry<'a>, lit: bool, covers: &Covers) -> Element<'a, Message> {
    let downloading = match &entry.state {
        SyncState::Downloading(progress) | SyncState::Unpacking(progress) => Some(*progress),
        _ => None,
    };
    let failed = matches!(entry.state, SyncState::Failed(_));

    let (w, h) = COVER;
    let cover: Element<Message> = match entry.cover.and_then(|url| covers.handle(url)) {
        Some(handle) => image(handle.clone()).content_fit(ContentFit::Cover).width(Length::Fixed(w)).height(Length::Fixed(h)).border_radius(6.0).into(),
        None => container(Space::new()).width(Length::Fixed(w)).height(Length::Fixed(h)).style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into(),
    };

    let figure = match (&entry.state, entry.bytes) {
        (SyncState::Downloading(progress), _) => format!("{}%", (progress * 100.0) as i32),
        (SyncState::Unpacking(progress), _) => format!("unpack {}%", (progress * 100.0) as i32),
        (SyncState::Failed(_), _) => "failed".to_string(),
        (_, Some(bytes)) => human_bytes(bytes),
        _ => "—".to_string(),
    };

    // Under the name: the bar for something in flight — a percentage that only moves every 250 ms
    // reads as static without something moving beside it — the reason for something that stopped,
    // and otherwise the system and the notes.
    let under: Element<Message> = match (&entry.state, downloading) {
        (_, Some(progress)) => row![text(entry.system).size(13.0).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None), progress_bar(progress)]
            .spacing(14)
            .align_y(Alignment::Center)
            .into(),
        (SyncState::Failed(why), _) => text(truncate(why, 90)).size(13.0).color(theme::ERROR).wrapping(Wrapping::None).into(),
        _ => {
            let mut words = row![text(entry.system).size(13.0).color(theme::TEXT_SECONDARY).wrapping(Wrapping::None)].spacing(8);
            if let Some(note) = &entry.note {
                let tone = if entry.state == SyncState::UpdateAvailable { theme::WARNING } else { theme::TEXT_SECONDARY };
                words = words.push(text("·").size(13.0).color(theme::TEXT_SECONDARY)).push(text(truncate(note, 80)).size(13.0).color(tone).wrapping(Wrapping::None));
            }
            words.into()
        }
    };

    let body = row![
        cover,
        column![
            text(truncate(&entry.title, 60)).size(17.0).font(theme::SEMIBOLD).color(theme::INK).wrapping(Wrapping::None),
            under,
        ]
        .spacing(5)
        .width(Length::Fill),
        container(text(figure).size(15.0).font(theme::SEMIBOLD).color(if failed { theme::ERROR } else { alpha(theme::INK, 0.86) }).wrapping(Wrapping::None))
            .width(Length::Fixed(90.0))
            .align_right(Length::Fixed(90.0)),
        // A chip rather than a bare label: the mouse needs its own way to remove a game, since
        // Delete is a key the pointer does not have.
        remove_chip(entry.rom_id, &entry.state, lit),
    ]
    .spacing(16)
    .align_y(Alignment::Center);

    let row = button(container(body).height(Length::Fill).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 16])
        .on_press(Message::DownloadsClicked(index))
        .style(crate::ui::style::glass_row(lit, 16.0));
    row.into()
}

fn progress_bar<'a>(progress: f32) -> Element<'a, Message> {
    let filled = progress.clamp(0.0, 1.0);
    container(
        row![
            container(Space::new().height(Length::Fixed(6.0)))
                .width(Length::FillPortion((filled * 1000.0) as u16 + 1))
                .style(|_| container::Style {
                    background: Some(Background::Color(theme::ACCENT_400)),
                    border: Border { radius: 3.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            Space::new().width(Length::FillPortion(((1.0 - filled) * 1000.0) as u16 + 1)),
        ]
        .height(Length::Fixed(6.0)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(6.0))
    .style(|_| container::Style {
        background: Some(Background::Color(glass(0.08))),
        border: Border { radius: 3.0.into(), ..Default::default() },
        ..Default::default()
    })
    .into()
}

/// The pill naming what removing this row does — and doing it, for the pointer. Glass, and filled
/// with the warning red under the cursor, as a Settings row's chip is for what takes something away.
fn remove_chip<'a>(rom_id: i64, state: &SyncState, lit: bool) -> Element<'a, Message> {
    let label = match state {
        SyncState::Installed | SyncState::UpdateAvailable => "Remove",
        SyncState::Failed(_) => "Dismiss",
        _ => "Cancel",
    };
    button(container(text(label).size(13.0).font(theme::SEMIBOLD).wrapping(Wrapping::None)).center_y(Length::Fill))
        .height(Length::Fixed(30.0))
        .padding([0, 14])
        .on_press(if matches!(state, SyncState::Failed(_)) { Message::DismissDownload(rom_id) } else { Message::RemoveDownload(rom_id) })
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let (fill, ink) = match (lit, hovered) {
                (true, _) => (theme::ERROR, Color::WHITE),
                (false, true) => (alpha(theme::ERROR, 0.18), theme::ERROR),
                (false, false) => (glass(0.08), theme::INK),
            };
            button::Style {
                background: Some(Background::Color(fill)),
                text_color: ink,
                border: Border { color: if lit { Color::TRANSPARENT } else { glass(0.14) }, width: 1.0, radius: 15.0.into() },
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

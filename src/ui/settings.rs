//! The Settings page — a port of `browse/ui/SettingsView.kt`, cut to what this build has settings
//! *for*.
//!
//! The Kotlin page has eight sections and six are here. Its RetroDECK, Emulators and ES-DE sections
//! configure the hand-off to RetroDECK, which this build makes without settings of its own: it
//! follows the emulator choices made in ES-DE. What is left to choose — RetroDECK, or a command of
//! the player's own — is the one row of *Playing*; see [`crate::launch`]. Saves is here because
//! battery-save sync needs no hand-off: it reads RetroArch's own config to find them — see
//! [`crate::sync`].
//!
//! Everything in `settings.json` is set here. Nothing on the page asks for typing it cannot take
//! from the pad: a path is walked to, folder by folder, and the two things that are text — a token,
//! a command line — open a field over an on-screen keyboard.
//!
//! Drawn as Home and the Library are: the sections on a glass strip under the header, walked with the
//! triggers as the Library's systems are, and one section at a time below it — its heading where
//! Home has its spotlight, then its rows. On Saves the sync is a card beside the heading rather than
//! a row. Every change is written straight through; there is no apply step to forget.

use std::path::PathBuf;
use std::time::Duration;

use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};
use iced_fonts::lucide;

use crate::app::Message;
use crate::config::{Connection, Settings};
use crate::downloads::human_bytes;
use crate::theme::{self, alpha, glass};
use crate::ui::home::truncated;

/// A row's height, and the gap under it.
const ROW_H: f32 = 60.0;
const ROW_GAP: f32 = 8.0;

/// The label's column: wide enough for the longest, so every value starts at one line down the page.
const LABEL_W: f32 = 260.0;

/// The longest value a row shows before it is cut. A row is one line tall, and an error message or
/// a path that wrapped would spill out of it.
const VALUE_CHARS: usize = 72;

/// The section's heading, beside the Saves section's sync card.
const HEADING_W: f32 = 560.0;
const SYNC_CARD_W: f32 = 420.0;

pub fn settings_id() -> widget::Id {
    widget::Id::new("settings-page")
}

/// The row under the cursor, as the part of the page scrolled to it.
pub fn cursor_id() -> widget::Id {
    widget::Id::new("settings-cursor")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Server,
    Playing,
    Saves,
    Display,
    Storage,
    About,
}

impl Section {
    pub const ALL: [Section; 6] =
        [Section::Server, Section::Playing, Section::Saves, Section::Display, Section::Storage, Section::About];

    fn title(self) -> &'static str {
        match self {
            Section::Server => "RomM server",
            Section::Playing => "Playing",
            Section::Saves => "Saves",
            Section::Display => "Display",
            Section::Storage => "Storage",
            Section::About => "About",
        }
    }

    /// What the section is for, under its name.
    fn description(self) -> &'static str {
        match self {
            Section::Server => "The RomM server your library lives on, and this device's place on it.",
            Section::Playing => "Which emulator starts a game, and how it picks up where you left off.",
            Section::Saves => "Battery saves and states are kept in sync with RomM, so a game picks up where it was left on any device.",
            Section::Display => "How large the page is drawn, and how many covers a row holds.",
            Section::Storage => "Where downloads go, how much room they may take, and what is kept on this device.",
            Section::About => "This build of Deckhand, its updates, and where its settings are kept.",
        }
    }

    fn position(self) -> usize {
        Section::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Its rows, in page order.
    pub fn rows(self) -> impl Iterator<Item = (usize, Row)> {
        Row::ALL.into_iter().enumerate().filter(move |(_, row)| row.section() == self)
    }
}

/// One row of the page, in page order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    ServerAddress,
    Connection,
    Certificate,
    Device,
    SignOut,
    Launcher,
    SystemEmulators,
    Resume,
    SteamLibrary,
    RetroArchConfig,
    SaveFolder,
    WatchSaves,
    EsDeHooks,
    StatePictures,
    UiScale,
    GridDensity,
    RomsFolder,
    Budget,
    Downloaded,
    ImageCache,
    LibraryCache,
    Version,
    Updates,
    ConfigFile,
}

impl Row {
    pub const ALL: [Row; 24] = [
        Row::ServerAddress,
        Row::Connection,
        Row::Certificate,
        Row::Device,
        Row::SignOut,
        Row::Launcher,
        Row::SystemEmulators,
        Row::Resume,
        Row::SteamLibrary,
        Row::RetroArchConfig,
        Row::SaveFolder,
        Row::WatchSaves,
        Row::EsDeHooks,
        Row::StatePictures,
        Row::UiScale,
        Row::GridDensity,
        Row::RomsFolder,
        Row::Budget,
        Row::Downloaded,
        Row::ImageCache,
        Row::LibraryCache,
        Row::Version,
        Row::Updates,
        Row::ConfigFile,
    ];

    fn label(self) -> &'static str {
        match self {
            Row::ServerAddress => "Server address",
            Row::Connection => "Connection",
            Row::Certificate => "Server certificate",
            Row::Device => "This device",
            Row::SignOut => "Sign out",
            Row::Launcher => "Default emulator",
            Row::SystemEmulators => "Emulator for each system",
            Row::SteamLibrary => "In Steam",
            Row::Resume => "Resume from the newest state",
            Row::RetroArchConfig => "RetroArch config",
            Row::SaveFolder => "RetroArch saves",
            Row::WatchSaves => "Sync as saves change",
            Row::EsDeHooks => "ES-DE launches",
            Row::StatePictures => "State pictures",
            Row::UiScale => "UI scale",
            Row::GridDensity => "Grid density",
            Row::RomsFolder => "Downloads go to",
            Row::Budget => "Space for games",
            Row::Downloaded => "Games on this device",
            Row::ImageCache => "Image cache",
            Row::LibraryCache => "Library cache",
            Row::Version => "Version",
            Row::Updates => "Updates",
            Row::ConfigFile => "Config file",
        }
    }

    pub fn section(self) -> Section {
        match self {
            Row::ServerAddress | Row::Connection | Row::Certificate | Row::Device | Row::SignOut => Section::Server,
            Row::Launcher | Row::SystemEmulators | Row::Resume | Row::SteamLibrary => Section::Playing,
            Row::RetroArchConfig | Row::SaveFolder | Row::WatchSaves | Row::EsDeHooks | Row::StatePictures => {
                Section::Saves
            }
            Row::UiScale | Row::GridDensity => Section::Display,
            Row::RomsFolder | Row::Budget | Row::Downloaded | Row::ImageCache | Row::LibraryCache => {
                Section::Storage
            }
            Row::Version | Row::Updates | Row::ConfigFile => Section::About,
        }
    }

    /// A stepped row's options, and which of them `settings` holds. None for every other row.
    ///
    /// The nearest step rather than an exact match: the file is hand-editable, and a value between
    /// two steps should light the one it is closest to rather than none of them.
    pub fn stepped(self, settings: &Settings) -> Option<(&'static [&'static str], usize)> {
        fn nearest(distances: impl Iterator<Item = f32>) -> usize {
            distances.enumerate().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(i, _)| i)
        }
        match self {
            Row::UiScale => Some((
                &theme::ui_scale::LABELS,
                nearest(theme::ui_scale::STEPS.iter().map(|s| (s - settings.ui_scale).abs())),
            )),
            Row::GridDensity => Some((
                &crate::grid::density::LABELS,
                nearest(
                    crate::grid::density::STEPS
                        .iter()
                        .map(|d| (d - settings.grid_density).abs() as f32),
                ),
            )),
            Row::Resume => Some((&["Off", "On"], usize::from(settings.resume_state))),
            Row::WatchSaves => Some((&["Off", "On"], usize::from(settings.watch_saves))),
            Row::Budget => {
                let set = settings.storage_budget_gb.unwrap_or(0) as f32;
                Some((
                    &crate::downloads::storage::BUDGET_LABELS,
                    nearest(crate::downloads::storage::BUDGET_STEPS.iter().map(|g| (*g as f32 - set).abs())),
                ))
            }
            _ => None,
        }
    }
}

/// The page's cursor: a row, which says the section shown as well.
#[derive(Debug, Default)]
pub struct SettingsFocus {
    index: usize,
}

impl SettingsFocus {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn row(&self) -> Row {
        Row::ALL[self.index]
    }

    pub fn section(&self) -> Section {
        self.row().section()
    }

    /// Put the cursor on a row — where a click landed.
    pub fn focus(&mut self, index: usize) {
        self.index = index.min(Row::ALL.len() - 1);
    }

    /// Up/Down, inside the section shown. Its ends clamp: the next section is the triggers' to
    /// show, and Up off its first row is Up into the bar.
    pub fn step(&mut self, delta: isize) -> bool {
        let to = self.index.saturating_add_signed(delta).min(Row::ALL.len() - 1);
        if to == self.index || Row::ALL[to].section() != self.section() {
            return false;
        }
        self.index = to;
        true
    }

    /// Show `section`, the cursor on its first row — where a click on the strip lands.
    pub fn show(&mut self, section: Section) {
        self.index = section.rows().next().map_or(0, |(index, _)| index);
    }

    /// The cursor's row is the first of its section, with the section's heading above it.
    pub fn at_top(&self) -> bool {
        self.section().rows().next().map(|(index, _)| index) == Some(self.index)
    }

    /// To the first row of the previous or next section — the keys that step systems everywhere
    /// else, `[`/`]` and the pad's L2/R2. Past either end it stays put, rather
    /// than "next" sending the cursor back up to the top of the section it is already in.
    pub fn jump_section(&mut self, delta: isize) -> bool {
        let here = self.row().section().position();
        let target = here.saturating_add_signed(delta).min(Section::ALL.len() - 1);
        if target == here {
            return false;
        }
        self.index = Row::ALL
            .iter()
            .position(|r| r.section() == Section::ALL[target])
            .unwrap_or(0);
        true
    }
}

/// What the last connection test found.
#[derive(Debug, Clone, Default)]
pub enum Check {
    #[default]
    Untested,
    Testing,
    /// Reached, and the token reads — carrying the account it reads as.
    Reachable(String),
    Failed(String),
}

/// What the last save sync did.
#[derive(Debug, Clone, Default)]
pub enum SaveSync {
    #[default]
    Idle,
    Running,
    Done(Box<crate::sync::Report>),
    /// The run could not start — no RetroArch, or RomM's save list out of reach.
    Failed(String),
}

/// Whether this device takes installs from RomM's web app — see `app::installs`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Installs {
    /// Not asked yet, or the last ask did not reach RomM.
    #[default]
    Unknown,
    /// RomM handed over what was waiting, and offers this device under *Install on device*.
    Taking,
    /// RomM tied no device to this token: it was paired by code, and only a QR pairing is.
    NotTied,
    /// This RomM has none to give: older than 5.4, or installs turned off on it.
    Unavailable,
}

impl Installs {
    /// Worth asking RomM (again).
    pub fn taking(self) -> bool {
        matches!(self, Installs::Unknown | Installs::Taking)
    }
}

pub struct SettingsContext<'a> {
    pub focus: &'a SettingsFocus,
    pub settings: &'a Settings,
    pub check: &'a Check,
    /// Where RetroArch keeps saves, or why that is not known. None while it is being looked for.
    pub save_folder: Option<&'a Result<PathBuf, String>>,
    /// The RetroArch config save sync reads, None inside when there is none on this device. None
    /// while it is being looked for.
    pub retroarch_found: Option<&'a Option<PathBuf>>,
    pub save_sync: &'a SaveSync,
    /// How the sync before a running one ended — what the sync card shows while one runs.
    pub sync_before: &'a SaveSync,
    /// How long ago the last sync finished, if one has this session.
    pub synced_ago: Option<Duration>,
    /// Where Deckhand's own update stands — see [`crate::release`].
    pub release: &'a crate::release::Status,
    /// Whether this runs as an AppImage, the one build that installs an update itself.
    pub appimage: bool,
    /// Whether RetroDECK is installed.
    pub retrodeck: bool,
    /// RetroDECK's install or first-run setup while one runs, as its row says it, and whether the
    /// row stops it.
    pub retrodeck_setup: Option<(String, bool)>,
    /// How many of the downloaded games are in Steam's library, of how many — None without Steam.
    pub steam: Option<(usize, usize)>,
    /// Where ES-DE's hooks stand. None while it is being read.
    pub hooks: Option<&'a crate::hooks::Status>,
    /// Whether RetroArch keeps a picture with each save state. None while it is being read.
    pub state_pictures: Option<&'a Result<crate::sync::retroarch::StatePictures, String>>,
    /// The art caches' size on disk. None while it is being measured.
    pub image_cache: Option<u64>,
    /// What this device has downloaded — see [`crate::downloads`].
    pub downloads: &'a crate::downloads::Downloads,
    pub games: usize,
    pub systems: usize,
    /// The systems with an emulator of their own, by name — see *Emulator for each system*.
    pub system_emulators: Vec<String>,
    pub installs: Installs,
}

pub fn settings_view<'a>(ctx: &SettingsContext<'_>) -> Element<'a, Message> {
    let section = ctx.focus.section();
    let margin = crate::ui::tab_bar::MARGIN;

    let mut top = row![heading(section)].align_y(Alignment::Start);
    if section == Section::Saves {
        top = top.push(Space::new().width(Length::Fill)).push(sync_card(ctx));
    }
    let mut rows = column![].spacing(ROW_GAP);
    for (index, setting) in section.rows() {
        rows = rows.push(setting_row(index, setting, ctx));
    }
    // Spaced so the tallest section, Saves with its card, fits a 1280x800 window at the Normal
    // scale. Larger scales scroll, the cursor's row kept in view.
    let body = column![top, Space::new().height(Length::Fixed(24.0)), rows, Space::new().height(Length::Fixed(16.0))]
        .padding(Padding { top: 22.0, right: margin, bottom: 0.0, left: margin });

    column![
        container(sections(section)).padding(Padding { top: 8.0, right: margin, bottom: 0.0, left: margin }),
        scrollable(body)
            .id(settings_id())
            .on_scroll(|_| Message::Scrolled)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
            .width(Length::Fill)
            .height(Length::Fill),
        hints(ctx),
    ]
    .into()
}

/// The sections, on the glass the Library's filters stand in, with the triggers that walk them.
fn sections<'a>(current: Section) -> Element<'a, Message> {
    use crate::ui::browse::{chip, key, strip};
    let mut pills = row![container(key("L2")).padding([0, 4])].spacing(4).align_y(Alignment::Center);
    for section in Section::ALL {
        pills = pills.push(chip(section.title().to_string(), section == current, false, Message::SettingsSection(section)));
    }
    strip(pills.push(container(key("R2")).padding([0, 4])))
}

/// The section's heading, where Home has its spotlight: where it is among the sections, its name,
/// and what it is for.
fn heading<'a>(section: Section) -> Element<'a, Message> {
    column![
        row![
            crate::ui::home::tag("SETTINGS".into(), true),
            text(format!("{} OF {}", section.position() + 1, Section::ALL.len()))
                .size(13.0)
                .font(theme::SEMIBOLD)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        text(section.title()).size(theme::text_size::DISPLAY_MEDIUM).font(theme::DISPLAY).color(theme::INK),
        text(section.description()).size(17.0).color(alpha(theme::INK, 0.86)),
    ]
    .spacing(12)
    .width(Length::Fixed(HEADING_W))
    .into()
}

/// The last sync, as a card beside the Saves heading: how it went and when, what it moved, and the
/// button that runs it again — which X presses from anywhere in the section.
///
/// A sync running changes the icon alone: the card goes on saying how the one before it went until
/// this one has something new to say, rather than emptying for the seconds it takes.
fn sync_card<'a>(ctx: &SettingsContext<'_>) -> Element<'a, Message> {
    let when = |prefix: &str| ctx.synced_ago.map(|ago| format!("{prefix} {}", ago_words(ago))).unwrap_or_default();
    let running = matches!(ctx.save_sync, SaveSync::Running);
    let shown = if running { ctx.sync_before } else { ctx.save_sync };
    let (icon, tone, headline, under) = match shown {
        SaveSync::Idle | SaveSync::Running => (
            lucide::cloud(),
            theme::TEXT_SECONDARY,
            "Not synced yet".to_string(),
            "Sync now sends this device's saves up and brings RomM's down".to_string(),
        ),
        SaveSync::Done(report) if !report.failed.is_empty() => {
            let n = report.failed.len();
            (lucide::cloud_alert(), theme::WARNING, format!("{n} save{} didn't sync", if n == 1 { "" } else { "s" }), when("Last tried"))
        }
        SaveSync::Done(_) => (lucide::cloud(), theme::IN_SYNC, "Everything is in sync".to_string(), when("Last synced")),
        // Why it could not start — no RomM to sync with, say — is the one thing to say about it.
        SaveSync::Failed(why) => (lucide::cloud_off(), theme::WARNING, "The sync didn't run".to_string(), truncated(why.clone(), 60)),
    };
    let (icon, tone) = if running { (lucide::refresh_cw(), theme::ACCENT_400) } else { (icon, tone) };
    let badge = container(icon.size(17.0).color(tone)).center(Length::Fixed(34.0)).style(move |_| container::Style {
        background: Some(Background::Color(alpha(tone, 0.16))),
        border: Border { radius: 17.0.into(), ..Default::default() },
        ..Default::default()
    });
    let mut top = column![text(headline).size(18.0).font(theme::BOLD).color(theme::INK)].spacing(2);
    if !under.is_empty() {
        top = top.push(text(under).size(13.0).color(theme::TEXT_SECONDARY));
    }
    let mut card = column![row![badge, top].spacing(12).align_y(Alignment::Center)].spacing(14);

    // What the run moved, as counts. What each save went through is more than a line here can say.
    if let SaveSync::Done(report) = shown {
        let last = match report.failed.len() {
            0 => (report.in_sync.to_string(), "already in sync", theme::IN_SYNC),
            n => (n.to_string(), "failed", theme::WARNING),
        };
        let counts = [(report.pushed.len().to_string(), "went up", theme::INK), (report.pulled.len().to_string(), "came down", theme::INK), last];
        let mut boxes = row![].spacing(8);
        for (value, label, ink) in counts {
            boxes = boxes.push(
                container(column![text(value).size(22.0).font(theme::DISPLAY).color(ink), text(label).size(12.0).color(theme::TEXT_SECONDARY)].spacing(2))
                    .width(Length::Fill)
                    .padding([10, 12])
                    .style(|_| container::Style {
                        background: Some(Background::Color(glass(0.05))),
                        border: Border { radius: 12.0.into(), ..Default::default() },
                        ..Default::default()
                    }),
            );
        }
        card = card.push(boxes);
    }

    // Pressable while one runs as well: a second press is let go — see `App::sync_saves`.
    let press = button(
        row![crate::ui::tab_bar::glyph("X", true), text("Sync now").size(15.0).font(theme::BOLD)]
            .spacing(9)
            .align_y(Alignment::Center),
    )
    .padding(Padding { top: 8.0, right: 18.0, bottom: 8.0, left: 10.0 })
    .style(|_, status| button::Style {
        background: Some(Background::Color(if matches!(status, button::Status::Hovered) { theme::ACCENT } else { theme::ACCENT_600 })),
        text_color: Color::WHITE,
        border: Border { radius: 12.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        snap: true,
    })
    .on_press(Message::SyncNow);
    card = card.push(press);

    container(card)
        .width(Length::Fixed(SYNC_CARD_W))
        .padding([18, 22])
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.07))),
            border: Border { color: glass(0.12), width: 1.0, radius: 20.0.into() },
            ..Default::default()
        })
        .into()
}

/// How long ago something was, the way the sync card says it.
fn ago_words(ago: Duration) -> String {
    match ago.as_secs() {
        0..60 => "just now".to_string(),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s @ 3600..86400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// One row: the label, the value beside it, and at the right what pressing it does — a chip, or the
/// options of a stepped row. The cursor's is lifted on a ring of the brightest ink.
fn setting_row<'a>(index: usize, setting: Row, ctx: &SettingsContext<'_>) -> Element<'a, Message> {
    let lit = index == ctx.focus.index();
    let parts = parts(setting, ctx);
    let mut line = row![
        text(setting.label())
            .size(16.0)
            .font(theme::SEMIBOLD)
            .color(theme::INK)
            .wrapping(Wrapping::None)
            .width(Length::Fixed(LABEL_W)),
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .height(Length::Fill);
    // The value takes the room between; without one, the room still goes between, so whatever is
    // at the right stays there.
    if !parts.iter().any(|part| matches!(part, Part::Words(..))) {
        line = line.push(Space::new().width(Length::Fill));
    }
    for part in parts {
        line = line.push(match part {
            Part::Words(words, tone) => value_text(words, tone),
            Part::Dot(ok) => status_dot(ok),
            Part::Act(label, danger) => action_chip(label, lit, danger),
            Part::Steps(options, selected) => segmented(setting, options, selected),
        });
    }

    let row = button(line)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .padding([0, 20])
        .on_press(Message::SettingsClicked(index))
        .style(crate::ui::style::glass_row(lit, 16.0));
    let row = container(row);
    if lit { row.id(cursor_id()).into() } else { row.into() }
}

/// What A and X do where the cursor is, said quietly at the foot of the page.
fn hints<'a>(ctx: &SettingsContext<'_>) -> Element<'a, Message> {
    let press = parts(ctx.focus.row(), ctx).into_iter().find_map(|part| match part {
        Part::Act(label, _) => Some(label),
        Part::Steps(..) => Some("Change"),
        _ => None,
    });
    let mut hints: Vec<(&str, String)> = press.map(|label| ("A", label.to_string())).into_iter().collect();
    if ctx.focus.section() == Section::Saves {
        hints.push(("X", "Sync now".to_string()));
    }
    crate::ui::tab_bar::hint_line(hints)
}

/// A piece of a row, right of its label.
enum Part {
    /// What the row says: its value, or where it stands.
    Words(String, Tone),
    /// Fine, wrong, or not known yet.
    Dot(Option<bool>),
    /// What pressing the row does, and whether it takes something away.
    Act(&'static str, bool),
    /// A stepped row's options, and which is set.
    Steps(&'static [&'static str], usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone {
    Plain,
    Accent,
    Warn,
}

/// What the RetroArch config row says: the one chosen, or what finding one turned up — said the way
/// the picker says it, since the picker looks too.
fn retroarch_note(chosen: Option<&PathBuf>, found: Option<&Option<PathBuf>>) -> (String, Tone) {
    match (chosen, found) {
        (Some(chosen), _) => (chosen.display().to_string(), Tone::Plain),
        (None, None) => ("looking…".to_string(), Tone::Plain),
        (None, Some(None)) => ("none found on this device".to_string(), Tone::Warn),
        (None, Some(Some(_))) => ("found automatically".to_string(), Tone::Plain),
    }
}

/// The right side of a row, by row.
fn parts(setting: Row, ctx: &SettingsContext<'_>) -> Vec<Part> {
    let connection = ctx.settings.connection.as_ref();
    match setting {
        // Read-only: pairing is what sets the address, together with a token that works for it.
        Row::ServerAddress => vec![Part::Words(
            connection.map_or_else(|| "not paired".to_string(), |c| c.host.clone()),
            Tone::Accent,
        )],
        Row::Connection => {
            let (ok, note, tone) = match ctx.check {
                Check::Untested => (None, "not tested".to_string(), Tone::Plain),
                Check::Testing => (None, "testing…".to_string(), Tone::Plain),
                Check::Reachable(user) => {
                    (Some(true), format!("reachable · signed in as {user}"), Tone::Plain)
                }
                Check::Failed(error) => (Some(false), error.clone(), Tone::Warn),
            };
            vec![Part::Dot(ok), Part::Words(note, tone), Part::Act("Test", false)]
        }
        // Reachability and encryption are different facts, so they get different rows: a working
        // plaintext server is reachable, and the thing worth saying is where its token travels.
        Row::Certificate => {
            let (note, tone) = match connection {
                None => ("—", Tone::Plain),
                Some(c) if c.host.starts_with("http://") => {
                    ("none · the token travels in the clear", Tone::Warn)
                }
                Some(c) if c.insecure => ("self-signed · accepted at pairing", Tone::Plain),
                Some(_) => ("verified", Tone::Plain),
            };
            vec![Part::Words(note.to_string(), tone)]
        }
        // Named so it can be found in RomM's token list: signing out here does not revoke it, and
        // revoking is done there.
        Row::Device => {
            let note = match connection {
                Some(Connection { token_name: Some(name), token_id: Some(id), .. }) => {
                    format!("{name} · token {id}")
                }
                Some(Connection { token_name: Some(name), .. }) => name.clone(),
                Some(_) => "paired before its token was recorded".to_string(),
                None => "not paired".to_string(),
            };
            let installs = match ctx.installs {
                Installs::Taking => " · takes installs from RomM",
                Installs::NotTied => " · pair by QR to take installs from RomM",
                Installs::Unknown | Installs::Unavailable => "",
            };
            vec![Part::Words(format!("{note}{installs}"), Tone::Plain)]
        }
        // Local: the token stays valid in RomM, and revoking it is done there.
        Row::SignOut => vec![
            Part::Words("downloaded games and saves stay on this device".to_string(), Tone::Plain),
            Part::Act("Sign out", true),
        ],
        Row::Launcher => match ctx.settings.launch_command.as_deref().filter(|c| !c.is_empty()) {
            Some(words) => vec![Part::Words(crate::launch::join_command(words), Tone::Plain), Part::Act("Change", false)],
            None if let Some((words, stops)) = &ctx.retrodeck_setup => {
                let mut row = vec![Part::Words(words.clone(), Tone::Accent)];
                if *stops {
                    row.push(Part::Act("Stop", true));
                }
                row
            }
            None if ctx.retrodeck => vec![Part::Words("RetroDECK".to_string(), Tone::Plain), Part::Act("Change", false)],
            None => vec![Part::Words("RetroDECK · not installed".to_string(), Tone::Warn), Part::Act("Change", false)],
        },
        Row::SystemEmulators => {
            let note = match ctx.system_emulators.as_slice() {
                [] => "none set, so every system uses the default".to_string(),
                [one] => format!("set for {one}"),
                [first @ .., last] if first.len() < 3 => format!("set for {} and {last}", first.join(", ")),
                many => format!("set for {} systems", many.len()),
            };
            vec![Part::Words(note, Tone::Plain), Part::Act("Change", false)]
        }
        Row::SteamLibrary => match ctx.steam {
            None => vec![Part::Words("Steam not found on this device".to_string(), Tone::Plain)],
            Some((_, 0)) => vec![Part::Words("nothing downloaded yet".to_string(), Tone::Plain)],
            Some((added, all)) if added >= all => vec![
                Part::Words(format!("all {all} downloaded games"), Tone::Plain),
                Part::Act("Refresh", false),
            ],
            Some((added, all)) => vec![
                Part::Words(format!("{added} of {all} downloaded games"), Tone::Plain),
                Part::Act("Add all", false),
            ],
        },
        Row::RetroArchConfig => {
            let (note, tone) = retroarch_note(ctx.settings.retroarch_config.as_ref(), ctx.retroarch_found);
            vec![Part::Words(note, tone), Part::Act("Change", false)]
        }
        Row::SaveFolder => vec![match ctx.save_folder {
            None => Part::Words("looking…".to_string(), Tone::Plain),
            Some(Ok(folder)) => Part::Words(folder.display().to_string(), Tone::Plain),
            Some(Err(why)) => Part::Words(why.clone(), Tone::Warn),
        }],
        Row::EsDeHooks => match ctx.hooks {
            None => vec![Part::Words("looking…".to_string(), Tone::Plain)],
            Some(status) => {
                let (ok, note) = status.describe();
                let chip = if ok { "Remove" } else if status.installed { "Reinstall" } else { "Install" };
                let mut row = vec![Part::Dot(Some(ok)), Part::Words(note, if ok { Tone::Plain } else { Tone::Warn })];
                if status.retrodeck {
                    row.push(Part::Act(chip, false));
                }
                row
            }
        },
        // RetroArch's own setting, which save sync sends the pictures of: on, off, or on but turned off
        // again by an override, which is named rather than edited.
        Row::StatePictures => match ctx.state_pictures {
            None => vec![Part::Words("looking…".to_string(), Tone::Plain)],
            Some(Err(_)) => vec![Part::Words("no RetroArch config found".to_string(), Tone::Plain)],
            Some(Ok(pictures)) if !pictures.on => vec![
                Part::Dot(Some(false)),
                Part::Words("off · states reach RomM without a picture".to_string(), Tone::Warn),
                Part::Act("Turn on", false),
            ],
            Some(Ok(pictures)) if !pictures.off_in.is_empty() => {
                let count = pictures.off_in.len();
                let note = format!("on · off again in {count} override{}", if count == 1 { "" } else { "s" });
                vec![Part::Dot(Some(false)), Part::Words(note, Tone::Warn), Part::Act("Show", false)]
            }
            Some(Ok(_)) => vec![Part::Dot(Some(true)), Part::Words("on · each state keeps a picture".to_string(), Tone::Plain)],
        },
        // What the row decides, beside its options.
        Row::UiScale | Row::GridDensity | Row::Budget | Row::Resume | Row::WatchSaves => {
            let what = match setting {
                Row::UiScale => "How large everything is drawn",
                Row::GridDensity => "How many covers a row of the Library holds",
                Row::Budget => "The longest unplayed games make room when a download would pass it",
                Row::Resume => "A game starts from its newest save state",
                _ => "A save goes up a few seconds after the game writes it",
            };
            let mut parts = vec![Part::Words(what.to_string(), Tone::Plain)];
            parts.extend(setting.stepped(ctx.settings).map(|(options, selected)| Part::Steps(options, selected)));
            parts
        }
        // Picked from a list of the drives here rather than typed: a path is not something a D-pad
        // can type.
        Row::RomsFolder => {
            let dir = ctx.settings.roms_dir();
            let free = crate::downloads::storage::available(&dir)
                .map(|f| format!(" · {} free", human_bytes(f)))
                .unwrap_or_default();
            vec![Part::Words(format!("{}{free}", dir.display()), Tone::Plain), Part::Act("Change", false)]
        }
        Row::Downloaded => {
            let count = ctx.downloads.installed_count();
            let budget = crate::downloads::storage::budget_bytes(ctx.settings.storage_budget_gb)
                .map(|b| format!(" of {}", human_bytes(b)))
                .unwrap_or_default();
            let note = if count == 0 {
                "nothing downloaded yet".to_string()
            } else {
                format!(
                    "{count} {} · {}{budget}",
                    if count == 1 { "game" } else { "games" },
                    human_bytes(ctx.downloads.total_bytes())
                )
            };
            // The removing is done on the Downloads page, where each game's own size is beside it —
            // a total is the wrong place to decide which one goes.
            vec![Part::Words(note, Tone::Plain), Part::Act("Manage", false)]
        }
        Row::ImageCache => {
            let note = match ctx.image_cache {
                None => "measuring…".to_string(),
                Some(0) => "empty".to_string(),
                Some(bytes) => format!("{} on disk", human_bytes(bytes)),
            };
            vec![Part::Words(note, Tone::Plain), Part::Act("Clear", true)]
        }
        Row::LibraryCache => vec![
            Part::Words(format!("{} games · {} systems", ctx.games, ctx.systems), Tone::Plain),
            Part::Act("Refresh", false),
        ],
        Row::Version => vec![Part::Words(env!("CARGO_PKG_VERSION").to_string(), Tone::Plain)],
        Row::Updates => {
            use crate::release::Status;
            match ctx.release {
                Status::Unasked => vec![Part::Words("not asked yet".to_string(), Tone::Plain), Part::Act("Check", false)],
                Status::Checking => vec![Part::Words("asking…".to_string(), Tone::Plain)],
                Status::Current => vec![
                    Part::Dot(Some(true)),
                    Part::Words("this is the newest".to_string(), Tone::Plain),
                    Part::Act("Check", false),
                ],
                Status::Out(release) => {
                    let about = release.headline.as_ref().map(|h| format!(" · {h}")).unwrap_or_default();
                    let mut row = vec![Part::Words(format!("{} is out{about}", release.version), Tone::Accent)];
                    // Anything but an AppImage is updated by hand; pressing the row says where from.
                    if ctx.appimage && release.appimage.is_some() {
                        row.push(Part::Act("Install", false));
                    }
                    row
                }
                Status::Installing(release, fraction) => vec![Part::Words(
                    format!("downloading {} · {}%", release.version, (fraction * 100.0).round() as u32),
                    Tone::Plain,
                )],
                Status::Installed(release) => vec![
                    Part::Dot(Some(true)),
                    Part::Words(format!("{} is in place · restart to use it", release.version), Tone::Plain),
                    Part::Act("Restart", false),
                ],
                Status::Failed(why, release) => vec![
                    Part::Dot(Some(false)),
                    Part::Words(why.clone(), Tone::Warn),
                    Part::Act(if release.is_some() { "Retry" } else { "Check" }, false),
                ],
            }
        }
        Row::ConfigFile => vec![Part::Words(Settings::file().display().to_string(), Tone::Plain)],
    }
}

fn value_text<'a>(value: String, tone: Tone) -> Element<'a, Message> {
    text(truncated(value, VALUE_CHARS))
        .size(theme::text_size::LABEL_LARGE)
        .color(match tone {
            Tone::Plain => theme::TEXT_SECONDARY,
            Tone::Accent => theme::ACCENT_400,
            Tone::Warn => theme::WARNING,
        })
        .wrapping(Wrapping::None)
        .width(Length::Fill)
        .into()
}

/// Fine, wrong, or not known yet.
fn status_dot<'a>(ok: Option<bool>) -> Element<'a, Message> {
    let color = match ok {
        Some(true) => theme::IN_SYNC,
        Some(false) => theme::WARNING,
        None => alpha(theme::INK, 0.25),
    };
    container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// The pill naming what pressing the row does — filled with the ink under the cursor, as the
/// button A presses. Drawn rather than a button of its own: the whole row is the button, so the
/// pointer and the pad do the same thing wherever they land on it.
fn action_chip<'a>(label: &'a str, lit: bool, danger: bool) -> Element<'a, Message> {
    let (fill, ink) = match (lit, danger) {
        (false, _) => (glass(0.08), theme::INK),
        (true, false) => (theme::INK, theme::GROUND),
        (true, true) => (theme::ERROR, Color::WHITE),
    };
    container(text(label).size(13.0).font(theme::SEMIBOLD).color(ink))
        .height(Length::Fixed(30.0))
        .padding([0, 14])
        .center_y(Length::Fixed(30.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(fill)),
            border: Border { color: if lit { Color::TRANSPARENT } else { glass(0.14) }, width: 1.0, radius: 15.0.into() },
            ..Default::default()
        })
        .into()
}

/// A stepped row's options, on the glass strip the Library's filters stand in, the one set filled
/// with the ink. Each is a button of its own, so a click picks it directly; the pad walks them with
/// Left/Right, and Confirm on the row steps on to the next.
fn segmented<'a>(setting: Row, options: &'static [&'static str], selected: usize) -> Element<'a, Message> {
    let mut pills = row![].spacing(3).align_y(Alignment::Center);
    for (i, option) in options.iter().enumerate() {
        let on = i == selected;
        pills = pills.push(
            button(container(text(*option).size(13.0).font(if on { theme::SEMIBOLD } else { theme::MEDIUM })).center_y(Length::Fill))
                .height(Length::Fixed(28.0))
                .padding([0, 16])
                .on_press(Message::SettingsPick(setting, i))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: match (on, hovered) {
                            (true, _) => Some(Background::Color(theme::INK)),
                            (false, true) => Some(Background::Color(glass(0.12))),
                            (false, false) => None,
                        },
                        text_color: if on { theme::GROUND } else { alpha(theme::INK, 0.82) },
                        border: Border { radius: 999.0.into(), ..Default::default() },
                        shadow: Shadow::default(),
                        snap: true,
                    }
                }),
        );
    }
    container(pills)
        .padding(3)
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { color: glass(0.12), width: 1.0, radius: 999.0.into() },
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_clamps_at_both_ends_of_its_section() {
        let mut focus = SettingsFocus::default();
        assert!(!focus.step(-1), "nothing above the first row");
        for _ in 0..Row::ALL.len() {
            focus.step(1);
        }
        assert_eq!(focus.row(), Row::SignOut, "the last of the server's rows: the next section is the triggers'");
        assert!(!focus.step(1));
        focus.show(Section::About);
        assert!(focus.at_top());
        for _ in 0..Row::ALL.len() {
            focus.step(1);
        }
        assert_eq!(focus.row(), *Row::ALL.last().unwrap());
        assert!(!focus.at_top());
    }

    #[test]
    fn a_section_jump_lands_on_the_first_row_of_the_next_section() {
        let mut focus = SettingsFocus::default();
        focus.step(2); // inside Server
        assert!(focus.jump_section(1));
        assert_eq!(focus.row(), Row::Launcher);
        assert!(focus.jump_section(-1));
        assert_eq!(focus.row(), Row::ServerAddress);
        // Already in the first section: back has nowhere to go and does not move.
        focus.step(1);
        assert!(!focus.jump_section(-1));
        assert_eq!(focus.row(), Row::Connection);
    }

    #[test]
    fn a_section_jump_past_the_last_section_stays_put() {
        let mut focus = SettingsFocus::default();
        focus.focus(Row::ALL.len() - 1);
        assert!(!focus.jump_section(1));
        assert_eq!(focus.row(), Row::ConfigFile, "not sent back up to the top of About");
    }

    #[test]
    fn a_sync_says_how_long_ago_in_the_largest_whole_unit() {
        assert_eq!(ago_words(Duration::from_secs(12)), "just now");
        assert_eq!(ago_words(Duration::from_secs(150)), "2 min ago");
        assert_eq!(ago_words(Duration::from_secs(7300)), "2 h ago");
        assert_eq!(ago_words(Duration::from_secs(3 * 86400 + 5)), "3 days ago");
    }

    #[test]
    fn a_value_between_steps_lights_the_nearest_one() {
        let settings = Settings { ui_scale: 1.2, grid_density: 7, ..Default::default() };
        assert_eq!(Row::UiScale.stepped(&settings).map(|(_, i)| i), Some(1), "1.2 is nearest Large");
        assert_eq!(Row::GridDensity.stepped(&settings).map(|(_, i)| i), Some(3), "past the end is Most");
        assert!(Row::Version.stepped(&settings).is_none());
    }

    #[test]
    fn the_retroarch_row_says_found_automatically_only_once_one_has_been_found() {
        let cfg = PathBuf::from("/home/deck/.var/app/net.retrodeck.retrodeck/config/retroarch/retroarch.cfg");
        assert_eq!(retroarch_note(None, None), ("looking…".to_string(), Tone::Plain));
        assert_eq!(retroarch_note(None, Some(&None)), ("none found on this device".to_string(), Tone::Warn));
        assert_eq!(retroarch_note(None, Some(&Some(cfg.clone()))), ("found automatically".to_string(), Tone::Plain));
        // One chosen is named, whatever finding one would turn up.
        let chosen = PathBuf::from("/opt/retroarch.cfg");
        assert_eq!(retroarch_note(Some(&chosen), Some(&Some(cfg))), ("/opt/retroarch.cfg".to_string(), Tone::Plain));
    }
}

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
//! The shape is the original's: one flat list of card rows under section headers, a breadcrumb of
//! pips naming the section the cursor is in, and every change written straight through — there is
//! no apply step to forget.

use std::path::PathBuf;

use iced::alignment::Vertical;
use iced::border;
use iced::widget::text::Wrapping;
use iced::widget::{self as widget, button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};

use crate::app::Message;
use crate::config::{Connection, Settings};
use crate::downloads::human_bytes;
use crate::theme;
use crate::ui::home::truncated;

/// A card's height, and the gap under it. Fixed, like every other scrolled page here, so the
/// cursor's scroll offset is computed rather than measured.
const CARD_H: f32 = 58.0;
const CARD_GAP: f32 = 8.0;

/// A section header: its title and one-line description, and the space above them.
const HEADER_H: f32 = 72.0;

/// The longest value a card shows before it is cut. A card is one line tall, and an error message
/// or a path that wrapped would spill out of it.
const VALUE_CHARS: usize = 72;

pub fn settings_id() -> widget::Id {
    widget::Id::new("settings-page")
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

    fn description(self) -> &'static str {
        match self {
            Section::Server => "Where your library lives.",
            Section::Playing => "Which emulator starts your games.",
            Section::Saves => "Battery saves kept in step with RomM.",
            Section::Display => "How the library looks on this screen.",
            Section::Storage => "What's kept on this device.",
            Section::About => "Version, updates and paths.",
        }
    }

    fn position(self) -> usize {
        Section::ALL.iter().position(|s| *s == self).unwrap_or(0)
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
    SyncSaves,
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
    pub const ALL: [Row; 25] = [
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
        Row::SyncSaves,
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
            Row::SyncSaves => "Sync",
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
            Row::RetroArchConfig | Row::SaveFolder | Row::SyncSaves | Row::WatchSaves | Row::EsDeHooks | Row::StatePictures => {
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

/// The page's cursor. One flat list, so one index.
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

    /// Put the cursor on a row — where a click landed.
    pub fn focus(&mut self, index: usize) {
        self.index = index.min(Row::ALL.len() - 1);
    }

    /// Up/Down. The ends clamp: there is nothing above the first row or below the last to hand to.
    pub fn step(&mut self, delta: isize) -> bool {
        let before = self.index;
        self.index = self.index.saturating_add_signed(delta).min(Row::ALL.len() - 1);
        self.index != before
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

/// How far down the list row `index`'s card starts: every section header up to and including its
/// own, and every card before it.
fn row_top(index: usize) -> f32 {
    let headers = Row::ALL[index].section().position() + 1;
    headers as f32 * HEADER_H + index as f32 * (CARD_H + CARD_GAP)
}

/// Where the list scrolls to for the cursor on row `index`: the card a quarter of the window down.
///
/// A quarter of the *window* is about a third of the list's own viewport once the tab bar, the
/// header strip and the title are taken off it — the original's anchor, which keeps the rows after
/// the cursor in view as well as the ones before it. The scrollable clamps the far end.
pub fn scroll_offset(index: usize, window_h: f32) -> f32 {
    (row_top(index) - window_h / 4.0).max(0.0)
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
    let title = column![
        text("Settings")
            .size(theme::text_size::HEADLINE_SMALL)
            .font(theme::MEDIUM)
            .color(theme::TEXT_PRIMARY),
        pips(ctx.focus.row().section()),
    ]
    .spacing(10)
    .padding(Padding { top: 20.0, right: 28.0, bottom: 4.0, left: 28.0 });

    let mut list = column![];
    for (i, setting) in Row::ALL.into_iter().enumerate() {
        if i == 0 || Row::ALL[i - 1].section() != setting.section() {
            list = list.push(section_header(setting.section()));
        }
        list = list.push(card(i, setting, ctx));
        list = list.push(Space::new().height(Length::Fixed(CARD_GAP)));
    }
    // Room under the last card, so it can scroll up to the same anchor as the rest instead of
    // stopping pinned to the bottom edge.
    list = list.push(Space::new().height(Length::Fixed(160.0)));

    column![
        title,
        scrollable(list.padding([0, 28]))
            .id(settings_id())
            .on_scroll(|_| Message::Scrolled)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .into()
}

/// The breadcrumb under the title: one pip per section, the cursor's filled and named.
fn pips<'a>(current: Section) -> Element<'a, Message> {
    let mut bar = row![].align_y(Alignment::Center);
    for (i, section) in Section::ALL.into_iter().enumerate() {
        if i > 0 {
            bar = bar.push(
                container(Space::new().width(Length::Fixed(34.0)).height(Length::Fixed(2.0))).style(
                    |_| container::Style {
                        background: Some(Background::Color(theme::alpha(theme::NEUTRAL_100, 0.10))),
                        ..Default::default()
                    },
                ),
            );
        }
        let on = section == current;
        let size = if on { 12.0 } else { 9.0 };
        bar = bar.push(
            container(Space::new().width(Length::Fixed(size)).height(Length::Fixed(size))).style(
                move |_| container::Style {
                    background: on.then_some(Background::Color(theme::ACCENT)),
                    border: Border {
                        color: if on { theme::ACCENT } else { theme::alpha(theme::NEUTRAL_100, 0.28) },
                        width: 2.0,
                        radius: (size / 2.0).into(),
                    },
                    ..Default::default()
                },
            ),
        );
        if on {
            bar = bar.push(
                text(section.title())
                    .size(theme::text_size::LABEL_LARGE)
                    .font(theme::SEMIBOLD)
                    .color(theme::ACCENT)
                    .wrapping(Wrapping::None),
            );
        }
    }
    bar.spacing(8).into()
}

fn section_header<'a>(section: Section) -> Element<'a, Message> {
    container(
        column![
            text(section.title())
                .size(theme::text_size::TITLE_MEDIUM)
                .font(theme::SEMIBOLD)
                .color(theme::TEXT_PRIMARY),
            text(section.description())
                .size(theme::text_size::LABEL_LARGE)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(2),
    )
    .height(Length::Fixed(HEADER_H))
    .align_y(Vertical::Bottom)
    .padding(Padding { bottom: 10.0, ..Padding::ZERO })
    .into()
}

/// One card: the label on the left, the row's value and its control on the right.
fn card<'a>(index: usize, setting: Row, ctx: &SettingsContext<'_>) -> Element<'a, Message> {
    let lit = index == ctx.focus.index();

    // The glowing left edge — the original's unambiguous "the cursor is here". Drawn clear on every
    // other card rather than left out, so labels do not shift sideways as the cursor passes.
    let edge = container(Space::new().width(Length::Fixed(4.0)).height(Length::Fixed(30.0))).style(
        move |_| container::Style {
            background: lit.then_some(Background::Color(theme::ACCENT)),
            border: Border { radius: border::right(4.0), ..Default::default() },
            ..Default::default()
        },
    );

    let mut line = row![
        edge,
        text(setting.label())
            .size(theme::text_size::BODY_LARGE)
            .color(if lit { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY })
            .wrapping(Wrapping::None)
            .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .height(Length::Fixed(CARD_H))
    .padding(Padding { right: 18.0, ..Padding::ZERO });
    for part in value(setting, ctx, lit) {
        line = line.push(part);
    }

    button(line)
        .width(Length::Fill)
        .padding(0)
        .on_press(Message::SettingsClicked(index))
        .style(crate::ui::style::row(lit, 14.0))
        .into()
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

/// The right side of a card, by row.
fn value<'a>(setting: Row, ctx: &SettingsContext<'_>, lit: bool) -> Vec<Element<'a, Message>> {
    let connection = ctx.settings.connection.as_ref();
    match setting {
        // Read-only: pairing is what sets the address, together with a token that works for it.
        Row::ServerAddress => vec![value_text(
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
            vec![status_dot(ok), value_text(note, tone), action_chip("Test", lit, false)]
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
            vec![value_text(note.to_string(), tone)]
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
            vec![value_text(format!("{note}{installs}"), Tone::Plain)]
        }
        // Local: the token stays valid in RomM, and revoking it is done there.
        Row::SignOut => vec![
            value_text("downloaded games and saves stay on this device".to_string(), Tone::Plain),
            action_chip("Sign out", lit, true),
        ],
        Row::Launcher => match ctx.settings.launch_command.as_deref().filter(|c| !c.is_empty()) {
            Some(words) => vec![value_text(crate::launch::join_command(words), Tone::Plain), action_chip("Change", lit, false)],
            None if let Some((words, stops)) = &ctx.retrodeck_setup => {
                let mut row = vec![value_text(words.clone(), Tone::Accent)];
                if *stops {
                    row.push(action_chip("Stop", lit, true));
                }
                row
            }
            None if ctx.retrodeck => vec![value_text("RetroDECK".to_string(), Tone::Plain), action_chip("Change", lit, false)],
            None => vec![value_text("RetroDECK · not installed".to_string(), Tone::Warn), action_chip("Change", lit, false)],
        },
        Row::SystemEmulators => {
            let note = match ctx.system_emulators.as_slice() {
                [] => "none set, so every system uses the default".to_string(),
                [one] => format!("set for {one}"),
                [first @ .., last] if first.len() < 3 => format!("set for {} and {last}", first.join(", ")),
                many => format!("set for {} systems", many.len()),
            };
            vec![value_text(note, Tone::Plain), action_chip("Change", lit, false)]
        }
        Row::SteamLibrary => match ctx.steam {
            None => vec![value_text("Steam not found on this device".to_string(), Tone::Plain)],
            Some((_, 0)) => vec![value_text("nothing downloaded yet".to_string(), Tone::Plain)],
            Some((added, all)) if added >= all => vec![
                value_text(format!("all {all} downloaded games"), Tone::Plain),
                action_chip("Refresh", lit, false),
            ],
            Some((added, all)) => vec![
                value_text(format!("{added} of {all} downloaded games"), Tone::Plain),
                action_chip("Add all", lit, false),
            ],
        },
        Row::RetroArchConfig => {
            let (note, tone) = retroarch_note(ctx.settings.retroarch_config.as_ref(), ctx.retroarch_found);
            vec![value_text(note, tone), action_chip("Change", lit, false)]
        }
        Row::SaveFolder => vec![match ctx.save_folder {
            None => value_text("looking…".to_string(), Tone::Plain),
            Some(Ok(folder)) => value_text(folder.display().to_string(), Tone::Plain),
            Some(Err(why)) => value_text(why.clone(), Tone::Warn),
        }],
        Row::SyncSaves => {
            let (ok, note, tone) = match ctx.save_sync {
                SaveSync::Idle => (None, "not synced yet".to_string(), Tone::Plain),
                SaveSync::Running => (None, "syncing…".to_string(), Tone::Plain),
                // A failure leads, with its reason, since the counts alone do not say which save.
                // Then a save set aside, which names where its copy went: the one line that tells
                // anyone the backups folder is there, said when there is something in it to want.
                SaveSync::Done(report) => {
                    let said = report.conflicts.first().or(report.removed.first()).or(report.notes.first());
                    match (report.failed.first(), said) {
                        (Some(first), _) => (Some(false), format!("{} · {first}", report.summary()), Tone::Warn),
                        // Something said about a run that moved nothing is the reason it moved nothing.
                        (None, Some(note)) => (Some(true), format!("{} · {note}", report.summary()), Tone::Plain),
                        (None, None) => (Some(true), report.summary(), Tone::Plain),
                    }
                }
                SaveSync::Failed(why) => (Some(false), why.clone(), Tone::Warn),
            };
            vec![status_dot(ok), value_text(note, tone), action_chip("Sync now", lit, false)]
        }
        Row::EsDeHooks => match ctx.hooks {
            None => vec![value_text("looking…".to_string(), Tone::Plain)],
            Some(status) => {
                let (ok, note) = status.describe();
                let chip = if ok { "Remove" } else if status.installed { "Reinstall" } else { "Install" };
                let mut row = vec![status_dot(Some(ok)), value_text(note, if ok { Tone::Plain } else { Tone::Warn })];
                if status.retrodeck {
                    row.push(action_chip(chip, lit, false));
                }
                row
            }
        },
        // RetroArch's own setting, which save sync sends the pictures of: on, off, or on but turned off
        // again by an override, which is named rather than edited.
        Row::StatePictures => match ctx.state_pictures {
            None => vec![value_text("looking…".to_string(), Tone::Plain)],
            Some(Err(_)) => vec![value_text("no RetroArch config found".to_string(), Tone::Plain)],
            Some(Ok(pictures)) if !pictures.on => vec![
                status_dot(Some(false)),
                value_text("off · states reach RomM without a picture".to_string(), Tone::Warn),
                action_chip("Turn on", lit, false),
            ],
            Some(Ok(pictures)) if !pictures.off_in.is_empty() => {
                let count = pictures.off_in.len();
                let note = format!("on · off again in {count} override{}", if count == 1 { "" } else { "s" });
                vec![status_dot(Some(false)), value_text(note, Tone::Warn), action_chip("Show", lit, false)]
            }
            Some(Ok(_)) => vec![status_dot(Some(true)), value_text("on · each state keeps a picture".to_string(), Tone::Plain)],
        },
        Row::UiScale | Row::GridDensity | Row::Budget | Row::Resume | Row::WatchSaves => setting
            .stepped(ctx.settings)
            .map(|(options, selected)| segmented(setting, options, selected, lit))
            .into_iter()
            .collect(),
        // Picked from a list of the drives here rather than typed: a path is not something a D-pad
        // can type.
        Row::RomsFolder => {
            let dir = ctx.settings.roms_dir();
            let free = crate::downloads::storage::available(&dir)
                .map(|f| format!(" · {} free", human_bytes(f)))
                .unwrap_or_default();
            vec![value_text(format!("{}{free}", dir.display()), Tone::Plain), action_chip("Change", lit, false)]
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
            vec![value_text(note, Tone::Plain), action_chip("Manage", lit, false)]
        }
        Row::ImageCache => {
            let note = match ctx.image_cache {
                None => "measuring…".to_string(),
                Some(0) => "empty".to_string(),
                Some(bytes) => format!("{} on disk", human_bytes(bytes)),
            };
            vec![value_text(note, Tone::Plain), action_chip("Clear", lit, true)]
        }
        Row::LibraryCache => vec![
            value_text(format!("{} games · {} systems", ctx.games, ctx.systems), Tone::Plain),
            action_chip("Refresh", lit, false),
        ],
        Row::Version => vec![value_text(env!("CARGO_PKG_VERSION").to_string(), Tone::Plain)],
        Row::Updates => {
            use crate::release::Status;
            match ctx.release {
                Status::Unasked => vec![value_text("not asked yet".to_string(), Tone::Plain), action_chip("Check", lit, false)],
                Status::Checking => vec![value_text("asking…".to_string(), Tone::Plain)],
                Status::Current => vec![
                    status_dot(Some(true)),
                    value_text("this is the newest".to_string(), Tone::Plain),
                    action_chip("Check", lit, false),
                ],
                Status::Out(release) => {
                    let about = release.headline.as_ref().map(|h| format!(" · {h}")).unwrap_or_default();
                    let mut row = vec![value_text(format!("{} is out{about}", release.version), Tone::Accent)];
                    // Anything but an AppImage is updated by hand; pressing the row says where from.
                    if ctx.appimage && release.appimage.is_some() {
                        row.push(action_chip("Install", lit, false));
                    }
                    row
                }
                Status::Installing(release, fraction) => vec![value_text(
                    format!("downloading {} · {}%", release.version, (fraction * 100.0).round() as u32),
                    Tone::Plain,
                )],
                Status::Installed(release) => vec![
                    status_dot(Some(true)),
                    value_text(format!("{} is in place · restart to use it", release.version), Tone::Plain),
                    action_chip("Restart", lit, false),
                ],
                Status::Failed(why, release) => vec![
                    status_dot(Some(false)),
                    value_text(why.clone(), Tone::Warn),
                    action_chip(if release.is_some() { "Retry" } else { "Check" }, lit, false),
                ],
            }
        }
        Row::ConfigFile => vec![value_text(Settings::file().display().to_string(), Tone::Plain)],
    }
}

fn value_text<'a>(value: String, tone: Tone) -> Element<'a, Message> {
    text(truncated(value, VALUE_CHARS))
        .size(theme::text_size::BODY_MEDIUM)
        .color(match tone {
            Tone::Plain => theme::TEXT_SECONDARY,
            Tone::Accent => theme::ACCENT,
            Tone::Warn => theme::ERROR,
        })
        .wrapping(Wrapping::None)
        .into()
}

/// Reachable, failed, or not known yet.
fn status_dot<'a>(ok: Option<bool>) -> Element<'a, Message> {
    let color = match ok {
        Some(true) => theme::ACCENT,
        Some(false) => theme::ERROR,
        None => theme::alpha(theme::NEUTRAL_100, 0.25),
    };
    container(Space::new().width(Length::Fixed(9.0)).height(Length::Fixed(9.0)))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border { radius: 4.5.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// The chip naming what pressing the row does. Drawn rather than a button of its own: the whole
/// card is the button, so the pointer and the pad do the same thing wherever they land on it.
fn action_chip<'a>(label: &'a str, lit: bool, danger: bool) -> Element<'a, Message> {
    let ink = match (lit, danger) {
        (false, _) => theme::TEXT_SECONDARY,
        (true, true) => theme::ERROR,
        (true, false) => theme::ACCENT,
    };
    container(
        text(label)
            .size(theme::text_size::LABEL_LARGE)
            .font(theme::MEDIUM)
            .color(ink),
    )
    .padding([6, 14])
    .style(move |_| container::Style {
        border: Border {
            color: if lit { theme::FOCUS_RING } else { theme::BORDER },
            width: 1.0,
            radius: 9.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// The original's segmented control. Each option is a button of its own, so a click picks it
/// directly; the pad walks it with Left/Right, and Confirm on the card cycles it.
fn segmented<'a>(
    setting: Row,
    options: &'static [&'static str],
    selected: usize,
    lit: bool,
) -> Element<'a, Message> {
    let mut bar = row![].spacing(2);
    for (i, option) in options.iter().enumerate() {
        let on = i == selected;
        bar = bar.push(
            button(
                text(*option)
                    .size(theme::text_size::LABEL_LARGE)
                    .font(if on { theme::SEMIBOLD } else { theme::REGULAR }),
            )
            .padding([5, 12])
            .on_press(Message::SettingsPick(setting, i))
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                let (fill, ink) = match (on, lit) {
                    (true, true) => (Some(theme::ACCENT), Color::WHITE),
                    (true, false) => (Some(theme::NEUTRAL_800), theme::TEXT_PRIMARY),
                    (false, _) if hovered => (Some(theme::ACCENT_800), theme::TEXT_PRIMARY),
                    (false, _) => (None, theme::TEXT_SECONDARY),
                };
                button::Style {
                    background: fill.map(Background::Color),
                    text_color: ink,
                    border: Border { radius: 8.0.into(), ..Default::default() },
                    shadow: Shadow::default(),
                    snap: true,
                }
            }),
        );
    }
    container(bar)
        .padding(3)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::alpha(Color::BLACK, 0.25))),
            border: Border { radius: 10.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_clamps_at_both_ends_of_the_list() {
        let mut focus = SettingsFocus::default();
        assert!(!focus.step(-1), "nothing above the first row");
        for _ in 0..Row::ALL.len() * 2 {
            focus.step(1);
        }
        assert_eq!(focus.row(), *Row::ALL.last().unwrap());
        assert!(!focus.step(1), "nothing below the last row");
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
    fn every_row_starts_below_its_own_section_header() {
        // Headers and cards are fixed height, so each row's first card sits exactly one header
        // below where the previous section's last card ended.
        for (i, setting) in Row::ALL.iter().enumerate().skip(1) {
            let step = row_top(i) - row_top(i - 1);
            let expected = if Row::ALL[i - 1].section() != setting.section() {
                CARD_H + CARD_GAP + HEADER_H
            } else {
                CARD_H + CARD_GAP
            };
            assert_eq!(step, expected, "row {i} ({setting:?})");
        }
        assert_eq!(scroll_offset(0, 800.0), 0.0, "the first row never scrolls the page");
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

//! The save sync page: what the last sync of every save did, game by game, and what a running one is
//! on — opened from the header's cloud and the sync card on Settings.
//!
//! Drawn as Home and Settings are: the run where Home has its spotlight — how it went, when, and
//! the button that runs it again — then what it did on the Library's glass strip as filters, walked
//! with the triggers, then a card per game with a line for each of its saves and states. Saves
//! already in sync are counted rather than listed, unless that filter is the one shown.

use std::time::Duration;

use iced::widget::{self as widget, button, column, container, image, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, ContentFit, Element, Length, Padding, Shadow};
use iced_fonts::lucide;

use crate::app::Message;
use crate::covers::Covers;
use crate::library::{Game, Library};
use crate::sync::{Event, Happened, Report};
use crate::theme::{self, alpha, glass};
use crate::ui::settings::SaveSync;

/// The cover beside a game's name on its card.
const COVER: (f32, f32) = (44.0, 60.0);

/// The game's column on its card, beside its saves.
const GAME_W: f32 = 300.0;

pub fn page_id() -> widget::Id {
    widget::Id::new("save-sync-page")
}

/// The card under the cursor, as the part of the page scrolled to it.
pub fn cursor_id() -> widget::Id {
    widget::Id::new("save-sync-cursor")
}

/// What the list shows: everything the run did, or one kind of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Everything but the saves already in sync, which are only counted.
    #[default]
    All,
    Up,
    Down,
    Settled,
    SetAside,
    LeftAlone,
    Failed,
    InSync,
}

impl Filter {
    pub const ALL: [Filter; 8] =
        [Filter::All, Filter::Up, Filter::Down, Filter::Settled, Filter::SetAside, Filter::LeftAlone, Filter::Failed, Filter::InSync];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Up => "Went up",
            Filter::Down => "Came down",
            Filter::Settled => "Settled",
            Filter::SetAside => "Set aside",
            Filter::LeftAlone => "Left alone",
            Filter::Failed => "Failed",
            Filter::InSync => "In sync",
        }
    }

    /// Whether the list under this filter has `happened` in it.
    fn takes(self, happened: &Happened) -> bool {
        match (self, happened) {
            (Filter::All, Happened::InSync) => false,
            (Filter::All, _) => true,
            (Filter::Up, Happened::Up)
            | (Filter::Down, Happened::Down)
            | (Filter::Settled, Happened::Settled { .. })
            | (Filter::SetAside, Happened::SetAside(_))
            | (Filter::LeftAlone, Happened::LeftAlone(_))
            | (Filter::Failed, Happened::Failed(_))
            | (Filter::InSync, Happened::InSync) => true,
            _ => false,
        }
    }

    /// How many of `events` it takes — for All, every one, the saves in sync too.
    fn count(self, events: &[Event]) -> usize {
        if self == Filter::All {
            return events.len();
        }
        events.iter().filter(|e| self.takes(&e.happened)).count()
    }

    /// The filters worth offering for `events`: All, and each that takes any.
    pub fn shown(events: &[Event]) -> Vec<Filter> {
        Self::ALL.into_iter().filter(|f| *f == Filter::All || f.count(events) > 0).collect()
    }
}

/// The page's cursor: the filter shown, and which game's card it is on.
#[derive(Debug, Default)]
pub struct SyncFocus {
    pub filter: Filter,
    pub card: usize,
}

/// One game's card: the game — None for saves no game in the library is named for — and what
/// happened to each of its saves the filter takes.
pub struct Group<'a> {
    pub game: Option<&'a Game>,
    pub events: Vec<&'a Event>,
}

/// `events` a card per game, as `filter` takes them: games with a save that failed first, then the
/// rest in the order the run came to them.
pub fn groups<'a>(events: &'a [Event], filter: Filter, library: &'a Library) -> Vec<Group<'a>> {
    let mut groups: Vec<(Option<i64>, Group<'a>)> = Vec::new();
    for event in events.iter().filter(|e| filter.takes(&e.happened)) {
        let game = event.rom_id.and_then(|id| library.game(id).or_else(|| library.game_of_build(id)));
        let key = game.map(|g| g.rom_id);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.events.push(event),
            None => groups.push((key, Group { game, events: vec![event] })),
        }
    }
    let failed = |g: &Group| g.events.iter().any(|e| matches!(e.happened, Happened::Failed(_)));
    let mut groups: Vec<Group<'a>> = groups.into_iter().map(|(_, g)| g).collect();
    // Stable: within each half the run's own order holds.
    groups.sort_by_key(|g| !failed(g));
    groups
}

pub struct SaveSyncContext<'a> {
    pub focus: &'a SyncFocus,
    pub library: &'a Library,
    pub covers: &'a Covers,
    pub save_sync: &'a SaveSync,
    /// How the sync before a running one ended: what the list shows while one runs.
    pub sync_before: &'a SaveSync,
    /// How long ago the last sync finished, if one has this session.
    pub synced_ago: Option<Duration>,
    /// Where a running sync is — see [`crate::sync::progress`].
    pub progress: Option<crate::sync::progress::Progress>,
    pub watch_saves: bool,
    /// Where back goes, by name.
    pub back_to: &'static str,
}

/// The report the page lists: the last one, and while a sync runs the one before it.
fn shown_report<'a>(ctx: &SaveSyncContext<'a>) -> Option<&'a Report> {
    let shown = if matches!(ctx.save_sync, SaveSync::Running) { ctx.sync_before } else { ctx.save_sync };
    match shown {
        SaveSync::Done(report) => Some(report),
        _ => None,
    }
}

pub fn save_sync_view<'a>(ctx: &SaveSyncContext<'a>) -> Element<'a, Message> {
    let margin = crate::ui::tab_bar::MARGIN;
    let report = shown_report(ctx);
    let events: &[Event] = report.map_or(&[], |r| r.events.as_slice());

    let mut body = column![back_link(ctx.back_to), Space::new().height(Length::Fixed(14.0)), heading(ctx, report)];
    if !events.is_empty() {
        body = body.push(Space::new().height(Length::Fixed(22.0))).push(filters(ctx.focus.filter, events));
    }
    let top = body.padding(Padding { top: 0.0, right: margin, bottom: 18.0, left: margin });

    let groups = groups(events, ctx.focus.filter, ctx.library);
    let cursor = ctx.focus.card.min(groups.len().saturating_sub(1));
    let mut list = column![].spacing(10);
    if matches!(ctx.save_sync, SaveSync::Running) && report.is_some() {
        list = list.push(text("From the last sync, while this one runs").size(13.0).color(theme::TEXT_SECONDARY));
    }
    for (i, group) in groups.iter().enumerate() {
        list = list.push(card(group, i == cursor, ctx.covers, ctx.library));
    }
    if let Some(report) = report {
        // Counted, not listed: a sweep finds most saves where it left them.
        let in_sync = Filter::InSync.count(events);
        if ctx.focus.filter == Filter::All && in_sync > 0 {
            let words = if in_sync == 1 { "And 1 save already in sync".to_string() } else { format!("And {in_sync} saves already in sync") };
            list = list.push(text(words).size(14.0).color(theme::TEXT_SECONDARY));
        }
        let about = about_run(report);
        if !about.is_empty() {
            let mut lines = column![text("About this run").size(16.0).font(theme::DISPLAY).color(theme::INK)].spacing(6);
            for line in about {
                lines = lines.push(text(line).size(14.0).color(alpha(theme::INK, 0.76)));
            }
            list = list.push(Space::new().height(Length::Fixed(10.0))).push(lines);
        }
    }

    let mut hints: Vec<(&str, String)> = Vec::new();
    if groups.get(cursor).and_then(|g| g.game).is_some() {
        hints.push(("A", "Open the game".into()));
    }
    hints.extend([("X", "Sync now".to_string()), ("B", "Back".to_string())]);

    column![
        top,
        scrollable(list.push(Space::new().height(Length::Fixed(16.0))).padding([0.0, margin]))
            .id(page_id())
            .on_scroll(|_| Message::Scrolled)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4).margin(4)))
            .width(Length::Fill)
            .height(Length::Fill),
        crate::ui::tab_bar::hint_line(hints),
    ]
    .into()
}

fn back_link<'a>(to: &'static str) -> Element<'a, Message> {
    button(text(format!("←  {to}")).size(theme::text_size::BODY_MEDIUM).color(alpha(theme::INK, 0.72)))
        .padding([4, 2])
        .on_press(Message::Back)
        .style(crate::ui::style::link)
        .into()
}

/// The run, where Home has its spotlight: when, how it went, a line on what it did — or, while one
/// runs, how far it has got — and the button that runs it again.
fn heading<'a>(ctx: &SaveSyncContext<'a>, report: Option<&Report>) -> Element<'a, Message> {
    let ago = ctx.synced_ago.map(crate::ui::settings::ago_words);
    let (eyebrow, icon, tone, headline, under) = match ctx.save_sync {
        SaveSync::Running => {
            let under = match ctx.progress {
                Some(p) if p.of > 0 => {
                    let on = p.rom_id.and_then(|id| ctx.library.game(id).or_else(|| ctx.library.game_of_build(id)));
                    match on {
                        Some(game) => format!("{} of {} saves so far · now on {}", p.done, p.of, game.title),
                        None => format!("{} of {} saves so far", p.done, p.of),
                    }
                }
                _ => "Finding the saves on this device and asking RomM about them…".to_string(),
            };
            ("RUNNING NOW".to_string(), lucide::refresh_cw(), theme::ACCENT_400, "Syncing your saves".to_string(), under)
        }
        SaveSync::Idle => (
            "NOT RUN YET".to_string(),
            lucide::cloud(),
            theme::TEXT_SECONDARY,
            "Not synced yet".to_string(),
            "Sync now sends this device's saves up and brings RomM's down.".to_string(),
        ),
        SaveSync::Failed(why) => (last_run(ago), lucide::cloud_off(), theme::WARNING, "The sync didn't run".to_string(), sentence(why)),
        SaveSync::Done(_) => {
            let events: &[Event] = report.map_or(&[], |r| r.events.as_slice());
            let failed = Filter::Failed.count(events);
            let games = groups(events, Filter::All, ctx.library).len();
            let looked = events.len();
            let acted = Filter::All.count(events) - Filter::InSync.count(events);
            let under = match (looked, acted) {
                (0, _) => "No saves were found to sync on this device.".to_string(),
                _ if failed > 0 => format!("{} looked at. The ones that didn't sync are first, with why.", saves(looked)),
                (_, 0) => format!("{} looked at; every one was already in sync.", saves(looked)),
                _ => format!("{} looked at. {} had something to do, across {}.", saves(looked), acted, plural(games, "game")),
            };
            match failed {
                0 => (last_run(ago), lucide::cloud(), theme::IN_SYNC, "Everything is in sync".to_string(), under),
                n => (last_run(ago), lucide::cloud_alert(), theme::WARNING, format!("{} didn't sync", plural(n, "save")), under),
            }
        }
    };
    let badge = container(icon.size(22.0).color(tone)).center(Length::Fixed(44.0)).style(move |_| container::Style {
        background: Some(Background::Color(alpha(tone, 0.16))),
        border: Border { radius: 22.0.into(), ..Default::default() },
        ..Default::default()
    });
    let mut left = column![
        row![
            crate::ui::home::tag("SAVE SYNC".into(), true),
            text(eyebrow).size(13.0).font(theme::SEMIBOLD).color(alpha(theme::INK, 0.82)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        row![badge, text(headline).size(40.0).font(theme::DISPLAY).color(theme::INK)].spacing(16).align_y(Alignment::Center),
        text(under).size(16.0).color(alpha(theme::INK, 0.82)),
    ]
    .spacing(10)
    .width(Length::Fill);
    if let (SaveSync::Running, Some(p)) = (ctx.save_sync, ctx.progress)
        && p.of > 0
    {
        left = left.push(progress_bar(p.done as f32 / p.of as f32));
    }

    let press = button(
        row![crate::ui::tab_bar::glyph("X", true), text("Sync now").size(15.0).font(theme::BOLD)].spacing(9).align_y(Alignment::Center),
    )
    .padding(Padding { top: 10.0, right: 18.0, bottom: 10.0, left: 10.0 })
    .on_press(Message::SyncNow)
    .style(|_, status| button::Style {
        background: Some(Background::Color(if matches!(status, button::Status::Hovered) { theme::ACCENT } else { theme::ACCENT_600 })),
        text_color: Color::WHITE,
        border: Border { radius: 12.0.into(), ..Default::default() },
        shadow: Shadow { color: alpha(Color::BLACK, 0.35), offset: iced::Vector::new(0.0, 10.0), blur_radius: 28.0 },
        snap: true,
    });
    let right = column![
        press,
        text(format!("Sync as saves change · {}", if ctx.watch_saves { "On" } else { "Off" })).size(13.0).color(theme::TEXT_SECONDARY),
    ]
    .spacing(10)
    .align_x(Alignment::End);

    row![left, right].spacing(40).align_y(Alignment::End).into()
}

fn last_run(ago: Option<String>) -> String {
    match ago {
        Some(ago) => format!("LAST RUN {}", ago.to_uppercase()),
        None => "LAST RUN".to_string(),
    }
}

/// "1 save" or "12 saves" — "saves and states", since the count holds both.
fn saves(n: usize) -> String {
    if n == 1 { "1 save or state".to_string() } else { format!("{n} saves and states") }
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// `words` with its first letter up and a full stop: the sync says its reasons as clauses.
fn sentence(words: &str) -> String {
    let words = words.trim().trim_end_matches('.');
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn progress_bar<'a>(fraction: f32) -> Element<'a, Message> {
    let done = container(Space::new()).width(Length::FillPortion((fraction.clamp(0.0, 1.0) * 1000.0) as u16 + 1)).height(Length::Fixed(6.0)).style(|_| {
        container::Style { background: Some(Background::Color(theme::ACCENT_400)), border: Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }
    });
    let rest = Space::new().width(Length::FillPortion(((1.0 - fraction.clamp(0.0, 1.0)) * 1000.0) as u16 + 1));
    container(row![done, rest])
        .width(Length::Fixed(520.0))
        .height(Length::Fixed(6.0))
        .style(|_| container::Style {
            background: Some(Background::Color(glass(0.08))),
            border: Border { radius: 3.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// What the run did, as filters with their counts, on the Library's glass strip with the triggers
/// that walk them.
fn filters<'a>(current: Filter, events: &[Event]) -> Element<'a, Message> {
    use crate::ui::browse::{chip, key, strip};
    let mut pills = row![container(key("L2")).padding([0, 4])].spacing(4).align_y(Alignment::Center);
    for filter in Filter::shown(events) {
        pills = pills.push(chip(format!("{}  {}", filter.label(), filter.count(events)), filter == current, false, Message::SyncFilter(filter)));
    }
    strip(pills.push(container(key("R2")).padding([0, 4])))
}

/// A game's card: its cover, name and system, and a line for each of its saves the filter takes. The
/// cursor's is lifted on the ink ring, as a Settings row is.
fn card<'a>(group: &Group<'a>, lit: bool, covers: &Covers, library: &Library) -> Element<'a, Message> {
    let (w, h) = COVER;
    let cover: Element<Message> = match group.game.and_then(|g| g.cover_url.as_deref()).and_then(|url| covers.handle(url)) {
        Some(handle) => image(handle.clone()).content_fit(ContentFit::Cover).width(Length::Fixed(w)).height(Length::Fixed(h)).border_radius(6.0).into(),
        None => container(Space::new()).width(Length::Fixed(w)).height(Length::Fixed(h)).style(|_| container::Style {
            background: Some(Background::Color(glass(0.06))),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into(),
    };
    let (title, about) = match group.game {
        Some(game) => {
            let played = game
                .last_played
                .as_deref()
                .and_then(|p| crate::library::short_date(p, crate::library::this_year()))
                .map(|d| format!(" · played {d}"))
                .unwrap_or_default();
            (game.title.clone(), format!("{}{played}", library.system_name(&game.system_id)))
        }
        None => ("Not in the library".to_string(), "Saves no game here is named for".to_string()),
    };
    let who = row![
        cover,
        column![
            text(crate::ui::home::truncate(&title, 30)).size(16.0).font(theme::SEMIBOLD).color(theme::INK),
            text(about).size(13.0).color(theme::TEXT_SECONDARY),
        ]
        .spacing(3),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .width(Length::Fixed(GAME_W));

    let mut lines = column![].spacing(10).width(Length::Fill);
    for event in &group.events {
        lines = lines.push(line(event));
    }
    let open = group.game.map(|g| Message::OpenDetail(g.rom_id));
    let mut press = button(row![who, lines].spacing(20).align_y(Alignment::Start)).width(Length::Fill).padding([14, 18]).style(crate::ui::style::glass_row(lit, 16.0));
    if let Some(open) = open {
        press = press.on_press(open);
    }
    let card = container(press);
    if lit { card.id(cursor_id()).into() } else { card.into() }
}

/// One save's line: an icon in its tone, what happened, and why or which.
fn line<'a>(event: &Event) -> Element<'a, Message> {
    let thing = thing(event);
    let (icon, tone, what, why) = match &event.happened {
        Happened::Up => (lucide::arrow_up(), theme::ACCENT_400, format!("{thing} sent up"), event.name.clone()),
        Happened::Down => (lucide::arrow_down(), theme::IN_SYNC, format!("{thing} brought down"), event.name.clone()),
        Happened::InSync => (lucide::check(), theme::TEXT_SECONDARY, format!("{thing} in sync"), event.name.clone()),
        Happened::Settled { kept_here: true, .. } => (
            lucide::git_merge(),
            theme::ACCENT_400,
            "Changed here and on RomM".to_string(),
            "Kept this device's; RomM keeps the other in its history".to_string(),
        ),
        Happened::Settled { kept_here: false, .. } => (
            lucide::git_merge(),
            theme::ACCENT_400,
            "Changed here and on RomM".to_string(),
            "Kept RomM's; this device's copy went to the backups".to_string(),
        ),
        Happened::SetAside(_) => (lucide::archive(), theme::TEXT_SECONDARY, "Removed on RomM".to_string(), "This device's copy went to the backups".to_string()),
        Happened::LeftAlone(why) => (lucide::circle_minus(), theme::TEXT_SECONDARY, format!("{thing} left as it is"), sentence(why)),
        Happened::Failed(why) => (lucide::triangle_alert(), theme::WARNING, format!("{thing} didn't sync"), sentence(why)),
    };
    let ink = if matches!(event.happened, Happened::Failed(_)) { theme::WARNING } else { theme::INK };
    let badge = container(icon.size(15.0).color(tone)).center(Length::Fixed(30.0)).style(move |_| container::Style {
        background: Some(Background::Color(alpha(tone, 0.14))),
        border: Border { radius: 15.0.into(), ..Default::default() },
        ..Default::default()
    });
    row![
        badge,
        column![
            text(what).size(15.0).font(theme::SEMIBOLD).color(ink),
            text(crate::ui::home::truncate(&why, 110)).size(13.0).color(theme::TEXT_SECONDARY),
        ]
        .spacing(1)
        .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// What the save is, in words: the game's save, or a state — and which slot, where its name says.
fn thing(event: &Event) -> String {
    if !event.state {
        return "Save".to_string();
    }
    let name = event.name.to_lowercase();
    let slot = name.split('.').find_map(|part| part.strip_prefix("slot").and_then(|n| n.parse::<u32>().ok()));
    match slot {
        Some(n) => format!("State in slot {n}"),
        None if name.contains(".auto.") => "Auto state".to_string(),
        None => "State".to_string(),
    }
}

/// What the run said about itself, beyond what it did to each save: why nothing moved, what it left
/// for the next sync. A line the list already says, about one save, is left out.
fn about_run(report: &Report) -> Vec<String> {
    report
        .notes
        .iter()
        .filter(|note| !report.events.iter().any(|e| note.starts_with(&format!("{}: ", e.name))))
        .map(|note| sentence(note))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(rom_id: Option<i64>, name: &str, happened: Happened) -> Event {
        Event { rom_id, name: name.into(), state: false, happened }
    }

    #[test]
    fn a_card_per_game_the_failed_first_and_the_saves_in_sync_only_counted() {
        let library = crate::fixtures::library(6);
        let ids: Vec<i64> = library.all_games().iter().map(|g| g.rom_id).collect();
        let events = vec![
            event(Some(ids[0]), "a.srm", Happened::Up),
            event(Some(ids[1]), "b.srm", Happened::InSync),
            event(Some(ids[0]), "a.state", Happened::Down),
            event(Some(ids[2]), "c.srm", Happened::Failed("RomM refused it".into())),
            event(None, "loose.srm", Happened::LeftAlone(crate::sync::NO_GAME.into())),
        ];
        let all = groups(&events, Filter::All, &library);
        let names: Vec<Option<i64>> = all.iter().map(|g| g.game.map(|g| g.rom_id)).collect();
        assert_eq!(names, [Some(ids[2]), Some(ids[0]), None], "failed first, then in the run's order; the save in sync not listed");
        assert_eq!(all[1].events.len(), 2, "a game's save and state on one card");
        assert_eq!(Filter::All.count(&events), 5, "All counts the saves in sync too");
        assert_eq!(Filter::shown(&events), [Filter::All, Filter::Up, Filter::Down, Filter::LeftAlone, Filter::Failed, Filter::InSync]);
        let in_sync = groups(&events, Filter::InSync, &library);
        assert_eq!(in_sync.len(), 1);
        assert_eq!(in_sync[0].game.map(|g| g.rom_id), Some(ids[1]));
    }

    #[test]
    fn a_state_says_its_slot() {
        let state = |name: &str| Event { rom_id: None, name: name.into(), state: true, happened: Happened::Up };
        assert_eq!(thing(&state("Game.slot2.retroarch.deckhand.state")), "State in slot 2");
        assert_eq!(thing(&state("Game.auto.retroarch.deckhand.state")), "Auto state");
        assert_eq!(thing(&state("ULUS10000_1.00_0.ppst")), "State");
        assert_eq!(thing(&event(None, "Game.srm", Happened::Up)), "Save");
        assert_eq!(sentence("could not zip it (disk full)."), "Could not zip it (disk full).");
    }
}

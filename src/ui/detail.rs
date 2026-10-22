//! The game detail page — a port of `browse/ui/DetailView.kt`.
//!
//! Full-bleed art behind a legibility scrim, the cover beside the game's name and its facts, the actions in a row under them, then the screenshots, the description and the
//! achievements. Every fact here comes off the library snapshot: the RomM list response already
//! carries the summary, the metadata, the size and the screenshot paths, so opening a game costs no
//! fetch and the page draws in the first frame.
//!
//! The pad's cursor walks it in rows: the actions, the screenshots — any one of which opens over the
//! whole window, see [`viewer`] — and the achievements' heading, which folds their list away.

use iced::widget::{self as widget, button, column, container, image, mouse_area, row, scrollable, stack, text, Space};
use iced::{Alignment, Background, Border, Color, ContentFit, Element, Gradient, Length, Radians};
use iced_fonts::lucide;

use crate::app::Message;
use crate::covers::Covers;
use crate::downloads::{Downloads, SyncState};
use crate::library::Game;
use crate::theme::{self, alpha};

/// How tall the cover down the left is: about as tall as the facts beside it, so a wide box's
/// cover does not take half the page.
pub const COVER_H: f32 = 190.0;

/// How tall the screenshots are drawn in their row.
pub const SCREENSHOT_H: f32 = 128.0;

/// How many of a game's screenshots the page shows.
pub const SHOTS_SHOWN: usize = 8;

/// Around each screenshot, inside the ring the cursor puts on it.
const SHOT_INSET: f32 = 3.0;

const SHOT_GAP: f32 = 12.0;

/// Between the page's parts: the cover and facts, the actions, and each section under them.
const SECTION_GAP: f32 = 28.0;

/// What a pad step up or down the page moves it by, where a step does not move the cursor.
pub const READ_STEP: f32 = 160.0;

/// Kept clear above or below a row the cursor is brought to.
pub const REVEAL_MARGIN: f32 = 24.0;

/// Around a screenshot opened over the window: room either side for the arrows to the next, and
/// above and below for its count and the way out.
pub const VIEWER_MARGIN: (f32, f32) = (96.0, 72.0);

pub fn detail_id() -> widget::Id {
    widget::Id::new("detail-page")
}

/// The row of actions, the screenshots and the achievements' heading, each as the part of the page
/// the cursor brings into view when it lands there.
pub fn part_id(zone: Zone) -> widget::Id {
    match zone {
        Zone::Actions => widget::Id::new("detail-actions"),
        Zone::Shots => widget::Id::new("detail-shots"),
        Zone::Achievements => widget::Id::new("detail-achievements"),
    }
}

/// The screenshots' own row, which scrolls sideways.
pub fn shots_strip_id() -> widget::Id {
    widget::Id::new("detail-shots-strip")
}

/// The detail page's own cursor: which row it is on, and where along it.
#[derive(Debug, Default)]
pub struct DetailFocus {
    pub zone: Zone,
    pub action: usize,
    pub shot: usize,
    /// The screenshot open over the whole window, if one is — see [`viewer`].
    pub viewing: Option<usize>,
}

/// The rows of the page the cursor stops on, top to bottom.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    #[default]
    Actions,
    Shots,
    /// The heading over the achievements, which opens and closes their list.
    Achievements,
}

impl Zone {
    /// The rows a page has: the actions always, the screenshots and the achievements where the
    /// game has any.
    pub fn of(shots: usize, achievements: u32) -> Vec<Zone> {
        let mut zones = vec![Zone::Actions];
        if shots > 0 {
            zones.push(Zone::Shots);
        }
        if achievements > 0 {
            zones.push(Zone::Achievements);
        }
        zones
    }
}

/// The actions the page offers. All four are drawn because the page is a poor thing without them —
/// it is where you decide what to do with a game — but what each one does depends on what this
/// device already has of it, and what this build can honestly do. See [`Action::pending_reason`].
///
/// Where the Kotlin page's fourth action is *Pin* — the exemption that keeps a game from being
/// evicted under the storage budget — this one is *Remove*. There is no budget here, so nothing
/// could ever evict a game and a pin would be a button with no opposite; removal is the actual
/// counterpart to a download, and it is what the space back is behind. Pin arrives with the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Download,
    /// The game's saves and states — see [`crate::ui::saves`].
    Saves,
    /// What RomM keeps beside the game — a manual, a hack, a translation — to put on this device or
    /// take off it. Only on a game that has any, and only once the game is here.
    Extras,
    Favourite,
    /// Keep it when a space budget makes room — see [`crate::downloads::storage`].
    Pin,
    /// Put it in Steam's library, or take it out — see [`crate::steam`].
    Steam,
    Remove,
}

impl Action {
    pub const ALL: [Action; 7] =
        [Action::Download, Action::Saves, Action::Extras, Action::Favourite, Action::Pin, Action::Steam, Action::Remove];

    /// The actions the page draws, in order: all of them, but Extras only for a game with extras on
    /// this device to offer. Every other action is drawn either way and says why it can't, when it
    /// can't — a game without extras is most games, and a button that is never live is just noise.
    pub fn shown(extras: bool) -> Vec<Action> {
        Self::ALL.into_iter().filter(|a| *a != Action::Extras || extras).collect()
    }

    fn label(self, sync: &SyncState, pinned: bool, missing: bool, in_steam: bool, favourite: bool) -> String {
        match self {
            Action::Download if missing && matches!(sync, SyncState::RemoteOnly | SyncState::Failed(_)) => {
                "Missing on RomM".into()
            }
            Action::Download => match sync {
                SyncState::Installed => "▶  Play".into(),
                SyncState::UpdateAvailable => "Update".into(),
                SyncState::Downloading(progress) => {
                    format!("Downloading… {}%", (progress * 100.0) as i32)
                }
                SyncState::Unpacking(progress) => format!("Unpacking… {}%", (progress * 100.0) as i32),
                SyncState::Failed(_) => "Try again".into(),
                SyncState::RemoteOnly => "Download".into(),
            },
            Action::Saves => "Saves & states".into(),
            Action::Extras => "Extras".into(),
            // The word carries it, as the Kotlin app has it: a heart glyph comes from a fallback
            // face, set tight against the word.
            Action::Favourite => if favourite { "Favourited".into() } else { "Favourite".into() },
            Action::Pin => if pinned { "Pinned".into() } else { "Pin".into() },
            Action::Steam => if in_steam { "In Steam".into() } else { "Add to Steam".into() },
            Action::Remove => "Remove".into(),
        }
    }

    /// What pressing this cannot do for a game in `sync`, and why. None means it is live.
    ///
    /// State-dependent because two of the four change meaning with it: Download becomes Play once the
    /// game is here, and Remove is live only then. Saying so on the button is the point — a button that lies about what it will do is
    /// worse than one that admits it.
    ///
    /// `missing` is a game RomM holds no file for in any build — gone from its disk since it was
    /// scanned, or never there — so a download has nothing to fetch.
    pub fn pending_reason(self, sync: &SyncState, missing: bool) -> Option<&'static str> {
        match (self, sync) {
            (Action::Download, SyncState::RemoteOnly | SyncState::Failed(_)) if missing => {
                Some("isn't on RomM's disk, so there is nothing to download — rescan its platform in RomM")
            }
            (Action::Download | Action::Saves | Action::Favourite, _) => None,
            // Not while the game itself is being fetched: both would write the one record of it.
            (Action::Extras, s) if s.is_busy() => Some("is downloading — its extras can be picked once that's done"),
            (Action::Extras, s) if s.is_here() => None,
            (Action::Extras, _) => Some("isn't on this device, and its extras go in beside it"),
            (Action::Steam, _) if !cfg!(target_os = "linux") => Some("can only be added to Steam on Linux in this build"),
            (Action::Steam, s) if s.is_here() => None,
            (Action::Steam, _) => Some("isn't on this device, so there's nothing for Steam to start"),
            (Action::Pin, s) if s.is_here() => None,
            (Action::Pin, _) => Some("isn't on this device, so there's nothing to keep"),
            (Action::Remove, SyncState::Installed | SyncState::UpdateAvailable | SyncState::Failed(_)) => None,
            (Action::Remove, _) => Some("isn't on this device, so there's nothing to remove"),
        }
    }
}

pub struct DetailContext<'a> {
    pub game: &'a Game,
    pub system_name: &'a str,
    pub system_total: usize,
    pub covers: &'a Covers,
    /// What this device has of each game — see [`crate::downloads`]. The whole action row reads
    /// off it, so the page is rebuilt as a download's progress arrives.
    pub downloads: &'a Downloads,
    pub focus: &'a DetailFocus,
    pub host: &'a str,
    /// Where the cursor sits in the system, so the header can say "3 of 374".
    pub position: Option<usize>,
    /// The game's saves and states on RomM, once asked — the stats strip counts them.
    pub saves: Option<&'a crate::game_saves::GameSaves>,
    /// One of its builds is in Steam's library, added from here.
    pub in_steam: bool,
    /// One of its builds is in RomM's favourites.
    pub favourite: bool,
    /// RetroAchievements' list for the game, once asked; None while it is on its way, or for a game
    /// with none.
    pub achievements: Option<&'a Result<std::sync::Arc<crate::achievements::GameAchievements>, String>>,
    /// RomM has extras of the build on this device to offer — see [`Action::Extras`].
    pub extras: bool,
    /// The achievements' list is open under its heading.
    pub achievements_open: bool,
}

impl<'a> DetailContext<'a> {
    fn asset(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.host.trim_end_matches('/'), path)
        }
    }
}

pub fn detail_view<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let game = ctx.game;

    // The actions under the cover, across the page, rather than beside it under the facts: a wide
    // box's cover leaves the facts too little room to hold a row of buttons as well, and they wrapped
    // into two and three rows there.
    let mut page = column![
        back_link(ctx),
        Space::new().height(Length::Fixed(12.0)),
        row![art_panel(ctx), Space::new().width(Length::Fixed(28.0)), facts(ctx)]
            .align_y(Alignment::Start),
        Space::new().height(Length::Fixed(20.0)),
        container(actions(ctx)).id(part_id(Zone::Actions)),
    ];
    for section in [screenshots(ctx), about(game), achievements(ctx)].into_iter().flatten() {
        page = page.push(Space::new().height(Length::Fixed(SECTION_GAP))).push(section);
    }
    let page = page.push(Space::new().height(Length::Fixed(SECTION_GAP))).padding([16, 28]);

    // The backdrop is the game's own banner, blurred, under a scrim heavy enough to read against.
    // Same treatment as Home's hero and for the same reason: it should read as this game's colour,
    // not as a picture competing with the text on top of it.
    let backdrop = game
        .marquee_path
        .as_deref()
        .map(|p| ctx.asset(p))
        .and_then(|url| ctx.covers.handle(&url).cloned());

    let mut layers = stack![];
    if let Some(handle) = backdrop {
        layers = layers.push(
            container(
                // Stretched, not cropped — blurred past telling, and a crop spills on tiny-skia; see
                // the same call on Home's hero.
                image(handle)
                    .content_fit(ContentFit::Fill)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true),
        );
    }
    layers = layers.push(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Gradient(Gradient::Linear(
                    iced::gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
                        .add_stop(0.0, alpha(theme::BACKGROUND, 0.94))
                        .add_stop(0.6, alpha(theme::BACKGROUND, 0.86))
                        .add_stop(1.0, alpha(theme::BACKGROUND, 0.78)),
                ))),
                ..Default::default()
            }),
    );
    layers = layers.push(
        scrollable(page)
            .id(detail_id())
            .on_scroll(|viewport| Message::DetailScrolled(viewport.absolute_offset().y))
            .width(Length::Fill)
            .height(Length::Fill),
    );

    layers.width(Length::Fill).height(Length::Fill).into()
}

fn back_link<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let where_ = match ctx.position {
        Some(i) => format!("←  {} · {} of {}", ctx.system_name, i + 1, ctx.system_total),
        None => format!("←  {} · {} games", ctx.system_name, ctx.system_total),
    };
    button(text(where_).size(theme::text_size::BODY_MEDIUM).color(theme::TEXT_SECONDARY))
        .padding([4, 2])
        .on_press(Message::Back)
        .style(crate::ui::style::link)
        .into()
}

/// The cover, with its install state called out across the top.
fn art_panel<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let game = ctx.game;
    let cover = game.cover_url.as_deref().and_then(|url| ctx.covers.feature(url));
    let ratio = cover.as_ref().map(|(_, meta)| meta.ratio).unwrap_or(0.71);
    let width = COVER_H * ratio;

    let art: Element<Message> = match cover {
        Some((handle, _)) => image(handle)
            .content_fit(ContentFit::Cover)
            .width(Length::Fixed(width))
            .height(Length::Fixed(COVER_H))
            .into(),
        None => Space::new().width(Length::Fill).height(Length::Fill).into(),
    };

    let badge = container(
        text(match ctx.downloads.state_of(game.builds()) {
            SyncState::Installed => "ON THIS DEVICE".to_string(),
            SyncState::UpdateAvailable => "UPDATE ON ROMM".to_string(),
            SyncState::Downloading(progress) => {
                format!("DOWNLOADING {}%", (progress * 100.0) as i32)
            }
            SyncState::Unpacking(progress) => format!("UNPACKING {}%", (progress * 100.0) as i32),
            SyncState::Failed(_) => "DOWNLOAD FAILED".to_string(),
            SyncState::RemoteOnly => "NOT DOWNLOADED".to_string(),
        })
        .size(10.0)
        .font(theme::BOLD)
        .color(theme::TEXT_PRIMARY),
    )
    .padding([4, 8])
    .style(|_| container::Style {
        background: Some(Background::Color(alpha(Color::BLACK, 0.72))),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    });

    container(
        stack![
            art,
            container(badge).padding(10),
        ]
        .width(Length::Fixed(width))
        .height(Length::Fixed(COVER_H)),
    )
    .style(|_| container::Style {
        background: Some(Background::Color(theme::SURFACE_VARIANT)),
        border: Border {
            color: alpha(theme::NEUTRAL_100, 0.12),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    })
    .clip(true)
    .into()
}

fn facts<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let game = ctx.game;

    // The chip row: what the game *is* — system, which build, year, genres — then where this player
    // stands with it, as marked in RomM.
    let mut chips = row![].spacing(10);
    for label in [Some(ctx.system_name.to_string()), game.build.clone(), game.year.map(|y| y.to_string())]
        .into_iter()
        .flatten()
        .chain(game.genres.iter().take(2).cloned())
        .chain(game.status.map(|s| s.label().to_string()))
        .chain(game.now_playing.then(|| "Now playing".to_string()))
        .chain(game.backlogged.then(|| "On the backlog".to_string()))
    {
        chips = chips.push(
            container(
                text(label).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY),
            )
            .padding([6, 12])
            .style(|_| container::Style {
                background: Some(Background::Color(alpha(theme::SURFACE, 0.75))),
                border: Border { color: theme::BORDER, width: 1.0, radius: 8.0.into() },
                ..Default::default()
            }),
        );
    }

    column![
        title(ctx),
        Space::new().height(Length::Fixed(10.0)),
        rating_line(game),
        Space::new().height(Length::Fixed(16.0)),
        chips.wrap().vertical_spacing(10.0),
        Space::new().height(Length::Fixed(20.0)),
        stats(ctx),
    ]
    .width(Length::Fill)
    .into()
}

/// The game's name, as text: RomM's title art beside the cover was the name a second time, in the
/// box's own lettering.
fn title<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    text(ctx.game.title.as_str())
        .size(theme::text_size::DISPLAY_MEDIUM)
        .font(theme::EXTRABOLD)
        .color(theme::TEXT_PRIMARY)
        .into()
}

/// Stars, the numeric rating, and when it was last played.
fn rating_line<'a>(game: &'a Game) -> Element<'a, Message> {
    let mut line = row![].spacing(8).align_y(Alignment::Center);

    if let Some(rating) = game.rating {
        // Five glyphs, filled up to the rating. A half star would need a third glyph; the number
        // beside it carries the precision that rounding loses.
        let filled = rating.round() as usize;
        line = line.push(
            text((0..5).map(|i| if i < filled { '★' } else { '☆' }).collect::<String>())
                .size(theme::text_size::TITLE_MEDIUM)
                .color(theme::ACCENT_SOFT),
        );
        line = line.push(
            text(format!("{rating:.1}"))
                .size(theme::text_size::BODY_MEDIUM)
                .color(theme::TEXT_SECONDARY),
        );
        line = line.push(text("·").size(theme::text_size::BODY_MEDIUM).color(theme::DOT_COLOR));
    }

    line = line.push(
        text(match game.played_date() {
            Some(date) => format!("Played {date}"),
            None => "Never played".to_string(),
        })
        .size(theme::text_size::BODY_MEDIUM)
        .color(theme::TEXT_SECONDARY),
    );
    line.into()
}

/// The stats strip. Each cell names a fact and gives it, and a fact this build cannot know says so
/// rather than showing a confident zero.
fn stats<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let game = ctx.game;
    let cells = [
        (
            "PUBLISHER",
            game.companies.first().cloned().unwrap_or_else(|| "Unknown".into()),
        ),
        ("PLAYERS", game.players.clone().unwrap_or_else(|| "-".into())),
        ("SIZE", game.size_label().unwrap_or_else(|| "-".into())),
        // RomM's counts, once it has answered. Until then a dash: a 0 would claim it holds none.
        ("SAVES", ctx.saves.map_or_else(|| "-".into(), |s| s.saves.len().to_string())),
        ("STATES", ctx.saves.map_or_else(|| "-".into(), |s| s.states.len().to_string())),
        // The main story, by HowLongToBeat's players; N/A when RomM has not matched the game there.
        ("HOW LONG TO BEAT", game.beat_label()),
    ];

    let mut strip = row![].spacing(36);
    for (label, value) in cells {
        strip = strip.push(
            column![
                text(label).size(10.0).font(theme::BOLD).color(theme::TEXT_MUTED),
                Space::new().height(Length::Fixed(4.0)),
                text(value)
                    .size(theme::text_size::TITLE_MEDIUM)
                    .font(theme::MEDIUM)
                    .color(theme::TEXT_PRIMARY),
            ],
        );
    }
    // Wrapped, for the facts beside a wide box's cover.
    strip.wrap().vertical_spacing(16.0).into()
}

fn actions<'a>(ctx: &DetailContext<'a>) -> Element<'a, Message> {
    let state = ctx.downloads.state_of(ctx.game.builds());
    let pinned = ctx.downloads.installed_builds(ctx.game.builds()).iter().any(|i| i.pinned);
    let mut buttons = row![].spacing(12);
    let shown = Action::shown(ctx.extras);
    for (i, action) in shown.iter().enumerate() {
        let primary = i == 0;
        // Held to the row: a button that went (Extras, with the game) can leave the cursor past it.
        let lit = ctx.focus.zone == Zone::Actions && ctx.focus.action.min(shown.len() - 1) == i;
        let label = action.label(&state, pinned, ctx.game.missing(), ctx.in_steam, ctx.favourite);
        // A running download is inert: the percentage is the progress, and a second press on the
        // one button the thumb is already resting on should not be a cancel found by accident.
        let running = *action == Action::Download && state.is_busy();
        let mut press = button(
            text(label)
                .size(theme::text_size::TITLE_MEDIUM)
                .font(if primary { theme::BOLD } else { theme::MEDIUM }),
        )
        .padding([13, 22]);
        if !running {
            press = press.on_press(Message::DetailAction(*action));
        }
        buttons = buttons.push(
            press
            // One bright button, the one under the cursor. The first action keeps a purple of its own
            // so it still reads as the main one, but a muted one: at full strength it out-shone a
            // focused neighbour, and from a couch the brightest button is the one you're on.
            .style(crate::ui::style::action(primary, lit)),
        );
    }
    // Wrapped, so the row never runs off a narrow window or a large UI scale.
    buttons.wrap().vertical_spacing(12.0).into()
}

fn about<'a>(game: &'a Game) -> Option<Element<'a, Message>> {
    let summary = game.summary.as_deref()?;
    Some(
        column![
            label("ABOUT"),
            Space::new().height(Length::Fixed(10.0)),
            text(summary.to_string())
                .size(theme::text_size::BODY_LARGE)
                .color(theme::TEXT_BODY),
        ]
        .into(),
    )
}

/// A section's name over it.
fn label<'a>(words: &'a str) -> Element<'a, Message> {
    text(words).size(10.0).font(theme::BOLD).color(theme::TEXT_MUTED).into()
}

/// A badge's size in the Achievements list, and how many rows it shows before saying how many more.
pub const BADGE: f32 = 48.0;
pub const ACHIEVEMENTS_SHOWN: usize = 60;

/// RetroAchievements' list for the game: each one's badge, title, what it asks, its points and
/// how many players have it — the last being the only rarity RomM carries — and, with an account
/// linked, which this player has: earned ones lit with the day they were earned, the rest locked.
///
/// Folded under a heading that says how many there are and how far along this player is, until it
/// is opened: a list that runs to dozens of rows is not what most visits to the page are for.
fn achievements<'a>(ctx: &DetailContext<'a>) -> Option<Element<'a, Message>> {
    let count = ctx.game.achievements as usize;
    if count == 0 {
        return None;
    }
    let open = ctx.achievements_open;
    let summary = match ctx.achievements {
        Some(Ok(game)) => {
            let points: i64 = game.list.iter().filter_map(|a| a.points).sum();
            match game.linked {
                // RetroAchievements' own count; earned points are not given, since a badge several
                // achievements share does not say which of them it was — see `achievements`.
                Some(true) => format!("{} of {} earned · {} points", game.earned_count(), game.list.len(), thousands(points)),
                _ => format!("{} · {} points", game.list.len(), thousands(points)),
            }
        }
        _ => count.to_string(),
    };
    let chevron = if open { lucide::chevron_up() } else { lucide::chevron_down() };
    let heading = button(
        row![
            lucide::trophy().size(18.0).color(theme::ACCENT_SOFT),
            text("Achievements").size(theme::text_size::TITLE_MEDIUM).font(theme::SEMIBOLD),
            text(summary).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY).width(Length::Fill),
            text(if open { "Hide" } else { "Show" }).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY),
            chevron.size(18.0).color(theme::TEXT_SECONDARY),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([14, 18])
    .on_press(Message::ToggleAchievements)
    .style(crate::ui::style::row(ctx.focus.zone == Zone::Achievements, 12.0));
    let heading = container(heading).id(part_id(Zone::Achievements));
    if !open {
        return Some(heading.into());
    }
    let body = match ctx.achievements {
        None => muted("Asking RomM…".into()),
        Some(Err(why)) => muted(why.clone()),
        Some(Ok(game)) => achievement_rows(ctx, game),
    };
    Some(column![heading, Space::new().height(Length::Fixed(14.0)), body].into())
}

/// The opened list: a note to link an account where none is, then a row per achievement.
fn achievement_rows<'a>(ctx: &DetailContext<'a>, game: &crate::achievements::GameAchievements) -> Element<'a, Message> {
    let list = &game.list;
    let mut rows = column![].spacing(8);
    for achievement in list.iter().take(ACHIEVEMENTS_SHOWN) {
        let earned = game.earned(achievement);
        let locked = game.locked(achievement);
        let badge: Element<Message> = match achievement_badge(achievement, locked, ctx.host).and_then(|url| ctx.covers.handle(&url).cloned()) {
            Some(handle) => image(handle)
                .content_fit(ContentFit::Contain)
                .width(Length::Fixed(BADGE))
                .height(Length::Fixed(BADGE))
                .opacity(if locked { 0.55_f32 } else { 1.0 })
                .into(),
            None => container(Space::new())
                .width(Length::Fixed(BADGE))
                .height(Length::Fixed(BADGE))
                .style(|_| container::Style {
                    background: Some(Background::Color(theme::SURFACE_VARIANT)),
                    border: Border { radius: 6.0.into(), ..Default::default() },
                    ..Default::default()
                })
                .into(),
        };
        let ink = if locked { theme::TEXT_SECONDARY } else { theme::TEXT_PRIMARY };
        let mut words = column![text(achievement.title.clone()).size(theme::text_size::BODY_LARGE).font(theme::MEDIUM).color(ink)].spacing(2);
        if let Some(description) = achievement.description.as_deref().filter(|d| !d.trim().is_empty()) {
            words = words.push(
                text(description.to_string())
                    .size(theme::text_size::BODY_MEDIUM)
                    .color(if locked { theme::TEXT_MUTED } else { theme::TEXT_SECONDARY }),
            );
        }
        let mut figures = column![].align_x(Alignment::End).spacing(2);
        if let Some(points) = achievement.points {
            figures = figures.push(
                text(format!("{points} pts"))
                    .size(theme::text_size::LABEL_LARGE)
                    .font(theme::SEMIBOLD)
                    .color(if locked { theme::TEXT_MUTED } else { theme::ACCENT_SOFT }),
            );
        }
        match earned {
            // The day is enough: the time is RetroAchievements' server's, not this device's.
            Some(earned) => {
                let day = earned.date.as_deref().map(|d| d.split([' ', 'T']).next().unwrap_or(d).to_string()).unwrap_or_default();
                let mode = if earned.date_hardcore.is_some() { " · hardcore" } else { "" };
                figures = figures.push(
                    text(format!("✓ earned {day}{mode}")).size(theme::text_size::LABEL_LARGE).color(theme::ACCENT),
                );
            }
            None => {
                if let Some(awarded) = achievement.num_awarded {
                    figures = figures.push(muted(format!("{} players", thousands(awarded))));
                }
            }
        }
        let lit = earned.is_some();
        rows = rows.push(
            container(row![badge, words.width(Length::Fill), figures].spacing(14).align_y(Alignment::Center))
                .padding([8, 12])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(alpha(theme::SURFACE, if locked { 0.45 } else { 0.75 }))),
                    border: Border {
                        color: if lit { alpha(theme::ACCENT, 0.55) } else { theme::BORDER },
                        width: 1.0,
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                }),
        );
    }
    if list.len() > ACHIEVEMENTS_SHOWN {
        rows = rows.push(muted(format!("and {} more", list.len() - ACHIEVEMENTS_SHOWN)));
    }
    if game.linked == Some(false) {
        return column![
            muted("Link a RetroAchievements account to your user in RomM to see which ones you have.".into()),
            Space::new().height(Length::Fixed(10.0)),
            rows,
        ]
        .into();
    }
    rows.into()
}

/// Where an achievement's badge is — the greyed one for a `locked` achievement: RomM's own copy
/// when it kept one, RetroAchievements' otherwise.
pub fn achievement_badge(achievement: &crate::romm::models::Achievement, locked: bool, host: &str) -> Option<String> {
    let (path, url) = if locked {
        (achievement.badge_path_lock.as_deref(), achievement.badge_url_lock.as_deref())
    } else {
        (achievement.badge_path.as_deref(), achievement.badge_url.as_deref())
    };
    match path.filter(|p| !p.is_empty()) {
        Some(path) if path.starts_with("http") => Some(path.to_string()),
        Some(path) => Some(format!("{}{}", host.trim_end_matches('/'), path)),
        None => url.filter(|u| !u.is_empty()).map(str::to_string),
    }
}

fn muted<'a>(line: String) -> Element<'a, Message> {
    text(line).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_MUTED).into()
}

/// 19293 → "19,293".
fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The screenshots the page shows: the first [`SHOTS_SHOWN`] of the game's, of those that have
/// arrived. What the cursor walks and the viewer steps through.
pub fn shown_shots<'g>(game: &'g Game, covers: &Covers) -> Vec<&'g String> {
    game.screenshots.iter().take(SHOTS_SHOWN).filter(|url| covers.handle(url).is_some()).collect()
}

/// How wide screenshot `url` is drawn in its row: its own shape, at the row's height.
fn shot_width(covers: &Covers, url: &str) -> f32 {
    SCREENSHOT_H * covers.meta(url).map_or(4.0 / 3.0, |meta| meta.ratio)
}

/// Where shown screenshot `index` sits along its row, left and right, ring and all — for scrolling
/// the row to it.
pub fn shot_span(game: &Game, covers: &Covers, index: usize) -> (f32, f32) {
    let shots = shown_shots(game, covers);
    let tile = |url: &String| shot_width(covers, url) + 2.0 * SHOT_INSET;
    let left: f32 = shots.iter().take(index).map(|url| tile(url) + SHOT_GAP).sum();
    (left, left + shots.get(index).map_or(0.0, |url| tile(url)))
}

/// The screenshots in a row that scrolls sideways, each one a button that opens it over the whole
/// window. The cursor's is ringed in the brightest ink, as a tile on Home is.
fn screenshots<'a>(ctx: &DetailContext<'a>) -> Option<Element<'a, Message>> {
    let shots = shown_shots(ctx.game, ctx.covers);
    if shots.is_empty() {
        return None;
    }
    let cursor = (ctx.focus.zone == Zone::Shots).then(|| ctx.focus.shot.min(shots.len() - 1));

    let mut strip = row![].spacing(SHOT_GAP);
    for (i, url) in shots.into_iter().enumerate() {
        let Some(handle) = ctx.covers.handle(url) else { continue };
        let lit = cursor == Some(i);
        strip = strip.push(
            button(
                image(handle.clone())
                    .content_fit(ContentFit::Cover)
                    .width(Length::Fixed(shot_width(ctx.covers, url)))
                    .height(Length::Fixed(SCREENSHOT_H))
                    .border_radius(6.0),
            )
            .padding(SHOT_INSET)
            .on_press(Message::ViewShot(i))
            .style(move |_, status| button::Style {
                background: None,
                border: Border {
                    color: if lit {
                        theme::INK
                    } else if matches!(status, button::Status::Hovered) {
                        theme::FOCUS_RING
                    } else {
                        Color::TRANSPARENT
                    },
                    width: 2.0,
                    radius: (6.0 + SHOT_INSET).into(),
                },
                ..Default::default()
            }),
        );
    }

    Some(
        container(column![
            label("SCREENSHOTS"),
            Space::new().height(Length::Fixed(10.0)),
            scrollable(strip)
                .id(shots_strip_id())
                .on_scroll(|_| Message::Scrolled)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(5).scroller_width(5).spacing(6),
                ))
                .width(Length::Fill),
        ])
        .id(part_id(Zone::Shots))
        .into(),
    )
}

/// Screenshot `index` of the page's over the whole window, as large as fits: the full-size copy
/// once it has come — see [`crate::covers::Kind::Full`] — and the row's until then.
///
/// Arrows either side step to the one before and after; Left and Right do the same on the pad.
/// A click anywhere else closes it, as B does.
pub fn viewer<'a>(game: &'a Game, covers: &'a Covers, index: usize) -> Element<'a, Message> {
    let shots = shown_shots(game, covers);
    let index = index.min(shots.len().saturating_sub(1));
    let picture: Element<Message> = match shots.get(index) {
        Some(url) => match covers.handle(&crate::covers::key(url, crate::covers::Kind::Full)).or_else(|| covers.handle(url)) {
            Some(handle) => image(handle.clone()).content_fit(ContentFit::Contain).width(Length::Fill).height(Length::Fill).into(),
            None => Space::new().into(),
        },
        None => Space::new().into(),
    };

    let arrow = |icon: iced::widget::Text<'a>, to: Option<usize>| -> Element<'a, Message> {
        let Some(to) = to else { return Space::new().width(Length::Fixed(48.0)).into() };
        button(container(icon.size(26.0).color(theme::INK)).center(Length::Fixed(48.0)))
            .padding(0)
            .on_press(Message::ViewShot(to))
            .style(|_, status| button::Style {
                background: Some(Background::Color(theme::glass(if matches!(status, button::Status::Hovered) { 0.16 } else { 0.08 }))),
                border: Border { color: theme::glass(0.14), width: 1.0, radius: 24.0.into() },
                text_color: theme::INK,
                ..Default::default()
            })
            .into()
    };
    let before = index.checked_sub(1);
    let after = (index + 1 < shots.len()).then_some(index + 1);
    let (side, end) = VIEWER_MARGIN;

    let count = text(format!("{} of {}", index + 1, shots.len())).size(theme::text_size::BODY_MEDIUM).color(theme::TEXT_SECONDARY);
    let close = button(
        row![crate::ui::tab_bar::glyph("B", false), text("Close").size(theme::text_size::BODY_MEDIUM)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([6, 12])
    .on_press(Message::CloseShot)
    .style(crate::ui::style::quiet);
    let top = row![count, Space::new().width(Length::Fill), close]
        .align_y(Alignment::Center)
        .height(Length::Fixed(end))
        .padding([0.0, side - 48.0]);
    let middle = row![
        container(arrow(lucide::chevron_left(), before)).center_x(Length::Fixed(side)),
        picture,
        container(arrow(lucide::chevron_right(), after)).center_x(Length::Fixed(side)),
    ]
    .align_y(Alignment::Center)
    .height(Length::Fill);

    mouse_area(
        container(column![top, middle, Space::new().height(Length::Fixed(end))])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style { background: Some(Background::Color(alpha(theme::GROUND, 0.98))), ..Default::default() }),
    )
    .on_press(Message::CloseShot)
    .into()
}

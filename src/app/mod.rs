//! The application: state, messages, and the async work behind them.
//!
//! Two screens. [`Screen::Login`] until a connection is stored, [`Screen::Browsing`] after — with a
//! loading state between, because a first library fetch on a self-hosted server is measured in
//! seconds per platform and a blank window for a minute reads as a hang.
//!
//! This file holds the state, the messages, the boot and [`App::update`]; the work is split by what
//! it is about, each an `impl App` of its own — [`dispatch`] routes every message, then [`library`]
//! (signing in, loading), [`art`], [`navigation`], [`settings`] and [`pickers`], [`browse`],
//! [`download`] and [`queue`], [`installs`], [`playing`], [`pairing`], [`release`], and [`view`].

use std::collections::HashMap;
use std::sync::Arc;

use futures::channel::mpsc::Sender;
use futures::{SinkExt, StreamExt};
use iced::keyboard::{self, key::Named};
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Background, Border, Element, Length, Subscription, Task};

use crate::animation::{Cursor, FocusFade};
use crate::ui::reveal;
use crate::cache;
use crate::collections::Scope;
use crate::frames::FrameLog;
use crate::config::{Connection, Settings};
use crate::covers::{self, Cover, CoverMeta, Covers};
use crate::downloads::{self, Downloads, QueueItem};
use crate::focus::HomeFocus;
use crate::library::{home_shelves, Game, Library, Shelf, System};
use crate::romm::models::Collection;
use crate::romm::{host_candidates, normalize_code, ApiError, RommClient, RomQuery, FETCH_PARALLELISM};
use crate::systems;
use crate::theme;
use crate::ui::home::{home_view, HomeContext};
use crate::ui::login::{login_view, LoginState};
use crate::ui::settings::{Check, Installs, Row as SettingRow, SaveSync};
use crate::ui::tab_bar::{tab_bar, Tab};

mod art;
mod browse;
mod dispatch;
mod download;
mod extras;
mod installs;
mod library;
mod navigation;
mod pairing;
mod pickers;
mod playing;
mod queue;
mod release;
mod settings;
mod view;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod round_trip;

/// How many covers are fetched at once. Higher than the platform gate because these are small and
/// mostly cache hits after a first run.
const COVER_PARALLELISM: usize = 8;

/// How many grid rows either side of the viewport count as "on screen".
///
/// This is the margin on both of the things the window decides — which covers are fetched and which
/// rows are built — so it buys the same thing twice: art that is already decoded when a scroll
/// reaches it, and tiles that already exist when it does.
const GRID_MARGIN_ROWS: usize = 3;

/// One decoded piece of art on its way back to the UI: the URL it was fetched from, and the handle
/// plus measurements, or None when the fetch or the decode failed.
///
/// Named because the channel's item type is otherwise long enough to bury the closure it annotates.
type ArtResult = (u64, String, Option<Arc<(iced::widget::image::Handle, CoverMeta)>>);

/// Filter chips plus sort chips — the Chips band's width, shared by the cursor and the view so the
/// two cannot disagree about how far Right goes.
const CHIP_COUNT: usize = crate::view_model::Filter::ALL.len() + crate::view_model::Sort::ALL.len();

#[derive(Debug, Clone)]
pub enum Message {
    // Login
    UrlChanged(String),
    CodeChanged(String),
    InsecureToggled(bool),
    Connect,
    Paired(Result<Box<Connection>, String>),
    /// Pair by QR code instead of a typed code — see [`crate::qr_pairing`].
    StartQr,
    /// RomM's answer to a request to pair by QR code, for the request with this generation.
    QrStarted(u64, Result<crate::qr_pairing::QrPairing, String>),
    /// Where a waiting QR request stands, after one more ask.
    QrPolled(u64, crate::qr_pairing::Poll),
    /// Give up on the QR code, back to the fields.
    CancelQr,

    // Library
    /// Read the whole server, platform by platform. The first pairing, and what Refresh does.
    LoadLibrary,
    /// Reconcile the cached library with the server, re-reading only the systems whose rom count
    /// moved. What a start with a usable cache does, behind an already-drawn Home.
    SyncLibrary,
    LibraryProgress(Progress),
    /// Decoded art on its way back, tagged with the art generation it was prepared for — see
    /// [`App::art_generation`].
    CoverLoaded(u64, String, Option<Arc<(iced::widget::image::Handle, CoverMeta)>>),

    /// RomM's smart and generated collections arrived, or could not — see [`crate::collections`].
    CollectionsLoaded(Result<crate::collections::Sources, String>),
    /// The player's own collections and smart ones, asked for again as the Collections page opened —
    /// see [`crate::collections::load_own`]. `edits` is [`App::favourite_edits`] when it was asked.
    OwnCollectionsLoaded { edits: u64, result: Result<(Vec<Collection>, Option<Vec<Collection>>), String> },
    /// A kind chip on the Collections page was clicked.
    CollectionKindPicked(usize),
    /// The Collections page was scrolled, carrying its absolute offset — what decides which cards'
    /// covers are worth fetching.
    CollectionsScrolled(f32),

    // Navigation
    TabSelected(Tab),
    /// Open a system's grid, by slug. From the Browse system list.
    OpenSystem(String),
    /// Open a collection's grid, by key. From a card on the Collections page or the Browse list.
    OpenCollection(String),
    OpenDetail(i64),
    /// Leave the current page for the one that opened it.
    Back,
    FilterSelected(crate::view_model::Filter),
    SortSelected(crate::view_model::Sort),
    /// Scroll the grid to a letter's first game, and put the cursor on it.
    JumpToLetter(usize),
    DetailAction(crate::ui::detail::Action),
    Nav(Direction),
    /// The window's size, as iced reports it — divided by the system's scale *and* by the scale this
    /// app gave it. From a resize, or from asking. The grid geometry follows its width, and the row
    /// window its height.
    Resized(f32, f32),
    /// The system's scale for the window changed — a move to another screen — so its size in the
    /// system's pixels did too.
    Rescaled,
    /// The panel's device-pixel ratio, as the windowing system reports it.
    PanelDensity(f32),
    /// The Browse grid was scrolled, carrying its absolute vertical offset.
    ///
    /// This is what makes the grid lazy: it is the only thing that tells the app which rows the
    /// user is actually looking at, and both the art fetch and the row build are windowed on it.
    GridScrolled(f32),
    /// Some other page or strip was scrolled. The app does not need to know where to — only that
    /// the next frame is a scroll; see [`App::style`].
    Scrolled,
    /// A frame went by while the focus cross-fade is running. Carries nothing and changes nothing:
    /// the view reads the fade's position from the clock, so all this has to do is get the view
    /// rebuilt. Only subscribed to while something is actually animating.
    Redraw,
    /// Walk every tab, wrapping. The gamepad binds this to the shoulder buttons, the keyboard to Tab.
    CycleTab(isize),
    /// Previous / next system, without leaving the page you are on.
    CycleSystem(isize),
    /// The mouse pointer came onto a game card, or left one.
    Hovered(Cursor),
    /// A card on Home was clicked: put the spotlight on it — or press it, if it was there already.
    HomeCardClicked(usize, usize),
    /// The wheel turned over Home's shelves, which walks them as Up and Down do.
    HomeWheel(iced::mouse::ScrollDelta),
    /// What this user has earned of their games' achievements, by RetroAchievements id — or why
    /// RomM could not say.
    AwardedLoaded(Result<Option<HashMap<i64, usize>>, String>),
    /// The mouse moved, clicked or scrolled — heard only while the cursor is up in the chrome.
    PointerUsed,
    Unhovered(Cursor),
    /// Confirm on whatever the cursor is on — a card, a button, a row.
    Activate,
    /// RomM's answer to a favourite being set or cleared: the favourites collection as it now is —
    /// or why not, in which case `ids` go back the way they were.
    FavouriteChanged { tile: i64, adding: bool, ids: Vec<i64>, result: Result<Collection, String> },
    Dismiss,
    SignOut,

    // Settings
    /// A settings row was clicked: put the cursor on it and press it.
    SettingsClicked(usize),
    /// Set a stepped setting — UI scale, grid density — to one of its options, by index.
    SettingsPick(SettingRow, usize),
    /// The connection test's answer: the account the token reads as, or why it could not.
    ConnectionTested(Result<String, String>),
    /// The art caches' size on disk, measured off the UI thread.
    ImageCacheMeasured(u64),
    /// The config save sync reads (None when there is none to read), and where it says RetroArch
    /// keeps saves, or why that could not be found.
    SaveFolderFound(Option<std::path::PathBuf>, Result<std::path::PathBuf, String>),
    /// Where ES-DE's hooks stand, read off the UI thread.
    HooksRead(crate::hooks::Status),
    /// A side of the waiting conflict was clicked.
    ConflictChosen(crate::sync::Choice),
    /// RomM registered this install as a device, or registered it again — see [`crate::sync::device`].
    DeviceReady(String),
    /// What installing or removing the hooks did, and where they stand after.
    HooksChanged(Result<Vec<String>, String>, crate::hooks::Status),
    /// A save sync finished: what it did, or why it could not start.
    SavesSynced(Result<crate::sync::Report, String>),
    /// Time to ask the repository again whether there is a newer Deckhand — see [`crate::release`].
    CheckRelease,
    /// Games found already in the downloads folder, to record — see [`downloads::detect`].
    GamesFound(Vec<downloads::Installed>),
    /// The banner's own Dismiss: hides the banner, and nothing else.
    DismissBanner,
    /// Time to see whether the banner has been up for [`BANNER_LASTS`]. Only while there is one.
    BannerTick,
    /// The detail page scrolled to this offset — which tells Up whether the page is at its top.
    DetailScrolled(f32),
    /// Time to look over the save folders for changes — see [`crate::watch`].
    WatchTick,
    /// What the save folders hold now, looked over off the UI thread.
    WatchScanned(Option<(Vec<std::path::PathBuf>, WatchFingerprint)>, crate::watch::Snapshot),
    /// The repository's newest release, or why it could not be read.
    ReleaseChecked(Result<crate::release::Release, String>),
    /// A new version being installed said something: how far it has got, or that it is done.
    ReleaseEvent(crate::release::Event),

    // Downloads
    /// Fetch this game onto the device — the spotlight's button, the detail page's, and a retry.
    Download(i64),
    /// A running download said something: how far it has got, or that it is done. One stream per
    /// worker, so an event can only be about the game that worker has.
    DownloadEvent(downloads::Event),
    /// Take a game off the device, dismiss a failure, or drop something from the queue.
    RemoveDownload(i64),
    /// Forget a failed download of a rom, leaving whatever of it is here — see [`Downloads::dismiss`].
    DismissDownload(i64),
    /// What removing a game's files actually did, off the UI thread.
    DownloadRemoved(String, Result<usize, String>),
    /// A Downloads row was clicked: put the cursor on it and press it.
    DownloadsClicked(usize),
    /// Remove whatever the Downloads cursor is on. Delete, the one key in this app that takes
    /// something away — which is why the page spells it out rather than leaving it to be found.
    RemoveFocusedDownload,
    /// RomM's detail of a game's own rom, for what Download offers — keyed by the tile's rom id.
    DownloadChoices(i64, Result<Box<crate::romm::models::RomDetail>, String>),
    /// RomM's detail of a downloaded build, for the extras its game's page offers — see
    /// [`App::load_files`].
    FilesLoaded(i64, Result<Box<crate::romm::models::RomDetail>, String>),
    /// An extra of a build taken off this device: the build, what the extra was, and the build's
    /// record without it.
    ExtraRemoved(i64, String, Result<Box<downloads::Installed>, String>),
    /// Time to ask RomM for the installs its web app queued for this device — see [`installs`].
    ClaimInstalls,
    /// The installs RomM holds taken by this device, or why it would not say.
    InstallsClaimed(Result<Vec<crate::romm::models::InstallRequest>, ApiError>),
    /// RomM's detail of a rom it asked this device to install that the library does not have yet.
    InstallDetail(crate::romm::models::InstallRequest, Result<Box<crate::romm::models::RomDetail>, String>),
    /// A row of the waiting picker was clicked.
    Picked(usize),
    /// Putting an older version of a save back went — see [`crate::sync::restore`]. Keyed by the
    /// game's tile, whose Saves page reads it again.
    SaveRestored(i64, Result<crate::sync::Report, String>),
    /// A state from RomM went into one of a game's slots, or why not — see [`crate::sync::put_state`].
    /// Keyed by the game's tile, whose Saves page reads RomM again.
    StatePut(i64, Result<(crate::sync::states::Slot, crate::sync::Report), String>),
    /// Games went into Steam's library (`add`) or came out, each with its shortcut's app id, and
    /// what art any went in without — or why not. See [`crate::steam`].
    SteamChanged { add: bool, games: Vec<(i64, u32)>, restarted: bool, result: Result<crate::steam::Gaps, String> },
    /// A key of the waiting text entry's keyboard was clicked.
    EntryKey(crate::ui::entry::Key),
    /// What the clipboard held, for the waiting text entry.
    Pasted(Option<String>),
    /// A game removed to make room: what deleting its files did.
    Evicted(String, Result<usize, String>),
    /// Where downloads go now, or why that folder could not be used.
    LocationSet(Result<std::path::PathBuf, String>),

    /// A gamepad asked for something — see [`crate::gamepad`].
    Pad(crate::gamepad::Pad),

    // Saves
    /// Open a game's Saves page, from its detail page.
    OpenSaves(i64),
    /// A game's saves and states, as RomM holds them — keyed by the tile's rom id.
    SavesLoaded(i64, Result<Arc<crate::game_saves::GameSaves>, String>),
    /// RetroAchievements' list for a game, from RomM's detail of it, for the detail page.
    AchievementsLoaded(i64, Result<Arc<crate::achievements::GameAchievements>, String>),
    /// A Saves page tab was clicked.
    SavesTab(crate::game_saves::Tab),
    /// A Saves page row was clicked: put the cursor on it.
    SavesRowClicked(usize),

    // Search
    /// Open the Search page over the current one: `/`, Ctrl+F, or the header's button.
    OpenSearch,
    /// A printable key, as typed: the query on the Search page, a shortcut everywhere else.
    /// Backspace arrives as [`crate::search::BACKSPACE`].
    KeyChar(String, bool),
    /// A key of the on-screen keyboard was pressed.
    SearchKey(char),
    /// A search result was clicked: put the cursor on it and open it.
    SearchResultClicked(usize),

    // Playing
    /// News from a running launch: the game has started, or it is over — see [`crate::launch`].
    LaunchEvent(crate::launch::Event),
    /// The window is asked to close by a signal: Steam's *Exit game* on Deckhand's own shortcut, or
    /// the session ending — see [`App::stop_asked`].
    StopAsked,
    /// A finished download's BIOS and save pull came back, with whatever is worth saying about it.
    Readied(i64, Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// `rom_count` per system slug, off the platform list — the stamp a cached library is
/// diffed against. Keyed by slug rather than by RomM's platform id because the slug is what a
/// `System` and every `Game` already carry, and it is stable across a server rebuild where the
/// ids are not.
fn platform_counts(platforms: &[crate::romm::models::Platform]) -> HashMap<String, i64> {
    platforms
        .iter()
        .map(|p| (p.slug.clone().unwrap_or_default(), p.rom_count))
        .collect()
}

/// The rom ids in RomM's own favourites collection.
///
/// The load already fetches collections, and RomM has no separate favourites endpoint, so the chip
/// reads them out of what is on hand. A free function because the cached start needs the same
/// answer from the same input.
fn favourites_of(collections: &[Collection]) -> std::collections::HashSet<i64> {
    collections
        .iter()
        .filter(|c| c.is_favorite)
        .flat_map(|c| c.rom_ids.iter().copied())
        .collect()
}

/// Progress from the library load. Reported per platform rather than at the end, because the whole
/// point of the report is that the end can be a minute away.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Platforms are known; the rom fetch is starting.
    Started { platforms: usize },
    Platform { done: usize, total: usize, name: String },
    Loaded(Arc<Loaded>),
    Failed(String),
}

/// Everything one library refresh produces.
#[derive(Debug)]
pub struct Loaded {
    pub library: Library,
    pub collections: Vec<Collection>,
    /// `rom_count` per system slug as the server reported it during this load — what the next
    /// start diffs against to decide which systems it has to re-read. A system that could not be
    /// read whole has none, so the next start reads it again.
    pub counts: HashMap<String, i64>,
    /// When this load began, for the next one's `updated_after` — see [`library`].
    pub read_at: Option<String>,
}

enum Screen {
    Login(LoginState),
    /// Paired, and the first library fetch is in flight.
    Loading { status: String },
    Browsing(Route),
}

/// Where in the browse UI the user is.
///
/// Detail carries where it was opened from rather than the app keeping a stack: the only way in is
/// from a grid or a shelf, and a one-deep return is all the page needs. A general stack would be
/// state to keep correct for a depth nothing reaches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Route {
    Home,
    /// Every collection, by kind — see [`crate::collections`].
    Collections,
    /// A grid of one system's games, or one collection's.
    Browse { scope: Scope },
    Detail { rom_id: i64, from: Box<Route> },
    Downloads,
    Settings,
    /// Search across every system, pushed over the page it was opened from, like a detail page.
    Search { from: Box<Route> },
    /// One game's saves and states, pushed over its detail page — see [`crate::game_saves`].
    Saves { rom_id: i64, from: Box<Route> },
}

impl Route {
    /// The tab this route belongs under, so the bar keeps showing where you are inside a detail
    /// page — which is a pushed destination, not a tab of its own.
    fn tab(&self) -> Tab {
        match self {
            Route::Home => Tab::Home,
            Route::Collections | Route::Browse { scope: Scope::Collection(_) } => Tab::Collections,
            Route::Browse { scope: Scope::System(_) } => Tab::Browse,
            Route::Search { .. } => Tab::Search,
            Route::Detail { from, .. } | Route::Saves { from, .. } => from.tab(),
            Route::Downloads => Tab::Downloads,
            Route::Settings => Tab::Settings,
        }
    }
}

pub struct App {
    settings: Settings,
    client: RommClient,
    screen: Screen,
    /// Shared, so a background job — writing the cache — can hold it without a copy of every game.
    library: Arc<Library>,
    collections: Vec<Collection>,
    shelves: Vec<Shelf>,
    focus: HomeFocus,
    /// The spotlight's picture, handing over from one game to the next — see [`Backdrop`].
    backdrop: crate::animation::Backdrop,
    /// Home's shelves sliding to the one the cursor is on — see [`Glide`].
    glide: crate::animation::Glide,
    /// What a touchpad has scrolled over Home's shelves and not yet spent on a step — see
    /// [`Message::HomeWheel`].
    wheel: f32,
    /// The spotlit game's name and description, cut to their lines, by rom id: measured once a game
    /// and kept, since measuring is a layout of the text — see [`crate::ui::home::clamp_lines`].
    spotlight_text: std::cell::RefCell<HashMap<i64, (String, String)>>,
    /// What this user has earned of each game's achievements, by RetroAchievements id, as RomM
    /// last said — for the spotlight. None until it has been asked, and for a user with no
    /// RetroAchievements account linked; a game missing from it has none earned.
    awarded: Option<HashMap<i64, usize>>,
    collections_focus: crate::collections::CollectionsFocus,
    /// The smart and generated collections as RomM last answered — None until the Collections tab
    /// first asks. The player's own arrive with the library, in [`App::collections`].
    collection_sources: Option<Result<crate::collections::Sources, String>>,
    /// All three, resolved against the library — rebuilt whenever either side changes.
    all_collections: crate::collections::Collections,
    /// Where the Collections page is scrolled to, mirrored the way [`App::grid_scroll`] is.
    collections_scroll: f32,
    browse_focus: crate::focus::BrowseFocus,
    detail_focus: crate::ui::detail::DetailFocus,
    /// The Browse page's filter and sort, kept across systems: they are a way of *reading* a
    /// library, not a property of one system, so walking to the next system keeps your view.
    filter: crate::view_model::Filter,
    sort: crate::view_model::Sort,
    /// Rom ids in RomM's favourites collection.
    favourites: std::collections::HashSet<i64>,
    /// Games whose favourite is on its way to RomM, by tile — a second press waits for the first.
    favouriting: std::collections::HashSet<i64>,
    /// Favourites pressed or answered this session, so a list of collections asked for before the
    /// latest can tell it is behind them.
    favourite_edits: u64,
    /// The window's current logical width, which the grid geometry divides up.
    ///
    /// Tracked rather than assumed because the whole point of the derivation is that a wider
    /// surface makes each poster bigger: pinned to the reference width, a maximised window would
    /// have shown reference-sized covers with the slack going to the right margin.
    content_width: f32,
    /// The window's current logical height. Used as a deliberate *over*-estimate of the Browse
    /// grid's viewport — the grid sits below the tab bar, the header and two bands of chrome, so
    /// the window is always taller. Erring high only builds a few more rows than are strictly
    /// visible, where erring low would leave a band of the page empty.
    content_height: f32,
    /// The window fills the screen — Game Mode, or `DECKHAND_FULLSCREEN` — so the layout is scaled
    /// to fit it rather than the window sized to fit the layout. See [`App::layout_scale`].
    fullscreen: bool,
    /// The window's size in the system's own logical pixels — the physical size over the system's
    /// scale, and *not* over this app's. What the screen is fitted against; see
    /// [`App::layout_scale`].
    window: (f32, f32),
    /// The scale factor iced is applying: what [`App::scale_factor`] last told it, which iced asks
    /// for at the end of every update. Every size iced reports is divided by it as well as by the
    /// system's scale, so it is what turns a report back into the system's pixels.
    applied_scale: std::cell::Cell<f32>,
    /// The panel's device-pixel ratio — 1 on an ordinary screen, 2 on a HiDPI one.
    ///
    /// Kept because it decides how much art is worth decoding: a cover drawn 250 units tall needs
    /// 250 rows on a 1x screen and 500 on a 2x one, and decoded art is essentially all of this
    /// app's memory. Starts at 1 and is corrected by [`Message::PanelDensity`] once the window can
    /// be asked — the safe direction to be wrong in briefly, since too *few* pixels only means one
    /// early cover is soft until it is drawn again.
    panel_density: f32,
    /// Bumped whenever art already held stops being valid — which today means one thing, a move to
    /// a denser panel. Work in flight carries the generation it was started under, so a decode that
    /// was already running at the old density is dropped on arrival instead of racing the
    /// replacement and sometimes winning.
    art_generation: u64,
    /// How far the Browse grid is scrolled, in layout units.
    ///
    /// Mirrored here rather than read back from the widget because the view has to know it *while*
    /// it is being built, and because half the scrolls are ours: `scroll_to` moves the grid without
    /// ever firing `on_scroll`, so every place that issues one records the offset it asked for.
    grid_scroll: f32,
    /// Counts scrolls, anywhere in the app. Only [`App::style`] reads it.
    scrolls: u32,
    covers: Covers,
    /// `rom_count` per system slug as the server last reported it. Carried so a sync can tell which
    /// systems actually moved — see [`crate::cache`]. Empty until a load or a cache read fills it,
    /// which makes the first sync a full read by construction.
    system_counts: HashMap<String, i64>,
    /// When the library was last read from RomM, give or take [`library::SKEW`] — what the next sync
    /// asks RomM for the changes since. None until a read records one.
    library_read_at: Option<String>,
    /// Frame-interval reporting, when `DECKHAND_FRAMETIMES` asked for it. `None` normally,
    /// and the only cost of that is this check — see [`crate::frames`].
    frame_log: Option<FrameLog>,
    /// The cross-fade between the card the cursor left and the one it landed on. One for the whole
    /// app, driven from [`App::update`] — see [`crate::animation`].
    fade: FocusFade,
    /// The game card under the mouse pointer. Kept apart from the cursor: pointing at a card
    /// highlights it, but Confirm still opens the one the cursor is on.
    hovered: Option<Cursor>,
    /// A transient message across the top of the window — a refresh failure, a game back from being
    /// played. Goes by itself once it has been up for [`BANNER_LASTS`], or by its Dismiss before
    /// that. Set through [`App::say`], which is what starts its time.
    banner: Option<String>,
    /// When the banner up now went up. None for one set without [`App::say`] — a test's — whose time
    /// starts at the next [`Message::BannerTick`].
    banner_since: Option<std::time::Instant>,
    /// The queue, what is running, and what is on disk — see [`crate::downloads`].
    downloads: Downloads,
    downloads_focus: crate::focus::DownloadsFocus,
    search_focus: crate::search::SearchFocus,
    /// The search index and the last query's results — worked out when the library or the query
    /// changes, not on every frame. Behind a `RefCell` because the view is what first asks.
    search_cache: std::cell::RefCell<SearchCache>,
    /// Browse's view of the scope last shown — see [`App::browse_entry`].
    browse_cache: std::cell::RefCell<Option<BrowseEntry>>,
    /// Each game's saves and states as RomM last answered, by tile rom id: asked for when its detail
    /// page opens, so the counts there and its Saves page are ready by the time they are looked at.
    game_saves: HashMap<i64, Result<Arc<crate::game_saves::GameSaves>, String>>,
    /// Each game's RetroAchievements list as RomM last answered, by rom id: asked for when its
    /// detail page opens, for a game that has any.
    achievements: HashMap<i64, Result<Arc<crate::achievements::GameAchievements>, String>>,
    /// What RomM holds of each downloaded build in a folder of its own — its files, and what each is
    /// to the game — by rom id: asked for when its game's page opens, for the extras it offers.
    rom_files: HashMap<i64, Arc<crate::romm::models::RomDetail>>,
    saves_focus: crate::game_saves::SavesFocus,
    /// A save that changed on both sides, waiting for the player to say which to keep. While there
    /// is one, the cursor keys and the pad answer it and nothing else.
    conflict: Option<PendingConflict>,
    /// A choice put to the player as a list — see [`crate::ui::picker`].
    pick: Option<PendingPick>,
    /// A setting being typed in — see [`crate::ui::entry`].
    entry: Option<PendingEntry>,
    /// Whether RetroDECK is installed, as Settings last looked.
    retrodeck: bool,
    /// Which of this device's games are in Steam's library — see [`crate::steam`].
    steam_bindings: crate::steam::Bindings,
    /// Whether Steam is installed, as Settings last looked.
    steam_found: bool,
    /// What the save folders held when last looked over, and what is waiting to settle — see
    /// [`crate::watch`].
    watcher: crate::watch::Watcher,
    /// The folders the watcher looks over, when they were worked out, and from what — see
    /// `WatchTick`.
    watch_folders: Option<(Vec<std::path::PathBuf>, std::time::Instant, WatchFingerprint)>,
    /// `watch.lock`, held while this is the Deckhand on the device that watches. None when another
    /// one does, or in fixture mode.
    watch_lock: Option<std::fs::File>,
    /// Each running download's worker, so *Cancel* can stop it.
    download_workers: HashMap<i64, iced::task::Handle>,
    /// Whether this device takes installs from RomM's web app — see [`installs`].
    installs: Installs,
    /// The installs RomM asked of this device that it has taken and not yet answered, by request id.
    install_requests: HashMap<String, installs::Taken>,
    /// A claim is on its way to RomM.
    claiming: bool,
    /// A look over the save folders is under way.
    looking: bool,
    /// The pad's cursor is up in the chrome — the tab bar, or the banner's Dismiss — rather than on
    /// the page. Up from a page's top row puts it there, Down or Back takes it back.
    chrome: Option<Chrome>,
    /// How far the detail page is scrolled, so Up at its top can go on to the tab bar.
    detail_offset: f32,
    /// The last Up on a page had nowhere to go — see [`App::navigate`].
    up_stopped: bool,
    /// Where ES-DE's hooks stand, as Settings last read it. None while it is being read.
    hooks: Option<crate::hooks::Status>,
    /// The game being played, from the press until its save has gone back up. One at a time: two
    /// emulators would fight over the screen and the pad, and two syncs over one save.
    playing: Option<i64>,
    /// How to ask the game being played to close, while there is one.
    stop: Option<crate::launch::Stop>,
    /// Asked to close while a game ran: the window goes once the game's closing is seen to.
    closing: bool,
    /// The game being handed to its emulator, or back, while its saves move — see [`Handover`].
    handover: Option<Handover>,
    settings_focus: crate::ui::settings::SettingsFocus,
    /// What the Settings page's connection test last found. Reset each time the page opens: the row
    /// carries no time, so a result from before a sign-out, or from an hour ago, would read as now.
    connection_check: Check,
    /// The art caches' size on disk, for the Settings page. None while it is being measured.
    image_cache_bytes: Option<u64>,
    /// Where RetroArch keeps saves, for the Settings page. None while it is being looked for.
    save_folder: Option<Result<std::path::PathBuf, String>>,
    /// The RetroArch config save sync reads, for the Settings page: Some(None) when there is none on
    /// this device. None while it is being looked for.
    retroarch_found: Option<Option<std::path::PathBuf>>,
    save_sync: SaveSync,
    /// Where Deckhand's own update stands — see [`crate::release`].
    release: crate::release::Status,
    /// The repository Deckhand's own releases are read from: [`crate::release::REPOSITORY`], or a
    /// test's.
    release_repository: &'static str,
    /// The version the banner last announced, so asking again every six hours does not say it again.
    release_announced: Option<String>,
    /// The AppImage this runs from, which an update replaces. None for any other build.
    appimage: Option<std::path::PathBuf>,
    /// Whether a settings change is written to disk. False in fixture mode: a synthetic session has
    /// no business rewriting the real settings file, and fixture mode is what the tests run in.
    writes_settings: bool,
}

impl App {
    /// [`App::with_client`] with a client of its own — for the tests.
    #[cfg(test)]
    pub fn new(fixtures: Option<usize>) -> (Self, Task<Message>) {
        Self::with_client(fixtures, RommClient::new().expect("the HTTP client is built from static config"))
    }

    /// `fixtures` is the `--fixtures=N` count: it short-circuits both pairing and the network and
    /// opens straight onto a synthetic Home, so the layout can be worked on without a server.
    /// `client` is built by `main`, which says why and stops if it cannot be.
    pub fn with_client(fixtures: Option<usize>, client: RommClient) -> (Self, Task<Message>) {
        let settings = Settings::load();
        let paired = fixtures.is_none() && settings.connection.is_some();

        let host = settings_host(&settings);
        let mut app = Self {
            screen: if paired {
                Screen::Loading { status: "Connecting...".into() }
            } else {
                Screen::Login(LoginState::new("https://".into()))
            },
            settings,
            client,
            library: Default::default(),
            collections: Vec::new(),
            shelves: Vec::new(),
            focus: HomeFocus::default(),
            backdrop: Default::default(),
            glide: Default::default(),
            wheel: 0.0,
            spotlight_text: Default::default(),
            awarded: None,
            collections_focus: Default::default(),
            collection_sources: None,
            all_collections: Default::default(),
            collections_scroll: 0.0,
            browse_focus: Default::default(),
            detail_focus: Default::default(),
            filter: Default::default(),
            sort: Default::default(),
            favourites: Default::default(),
            favouriting: Default::default(),
            favourite_edits: 0,
            content_width: crate::grid::REFERENCE_CONTENT_W,
            content_height: crate::grid::REFERENCE_CONTENT_H,
            fullscreen: false,
            window: (0.0, 0.0),
            applied_scale: std::cell::Cell::new(1.0),
            panel_density: 1.0,
            art_generation: 0,
            grid_scroll: 0.0,
            scrolls: 0,
            covers: Covers::default(),
            system_counts: HashMap::new(),
            library_read_at: None,
            frame_log: FrameLog::from_env(),
            fade: FocusFade::default(),
            hovered: None,
            banner: None,
            banner_since: None,
            downloads: match fixtures {
                Some(_) => Downloads::detached(Settings::config_dir(), &host),
                None => Downloads::load(Settings::config_dir(), &host),
            },
            downloads_focus: Default::default(),
            playing: None,
            stop: None,
            closing: false,
            handover: None,
            game_saves: HashMap::new(),
            achievements: HashMap::new(),
            rom_files: HashMap::new(),
            saves_focus: Default::default(),
            conflict: None,
            pick: None,
            entry: None,
            retrodeck: fixtures.is_none() && crate::launch::RetroDeck::detect().is_some(),
            steam_bindings: crate::steam::Bindings::load(),
            steam_found: false,
            watcher: crate::watch::Watcher::default(),
            watch_folders: None,
            watch_lock: fixtures.is_none().then(|| crate::watch::claim(&Settings::config_dir())).flatten(),
            download_workers: HashMap::new(),
            installs: Installs::default(),
            install_requests: HashMap::new(),
            claiming: false,
            looking: false,
            chrome: None,
            detail_offset: 0.0,
            up_stopped: false,
            hooks: None,
            search_focus: Default::default(),
            search_cache: Default::default(),
            browse_cache: Default::default(),
            settings_focus: Default::default(),
            connection_check: Check::default(),
            image_cache_bytes: None,
            save_folder: None,
            retroarch_found: None,
            save_sync: SaveSync::Idle,
            release: crate::release::Status::Unasked,
            release_repository: crate::release::REPOSITORY,
            release_announced: None,
            appimage: fixtures.is_none().then(crate::release::running_appimage).flatten(),
            writes_settings: fixtures.is_none(),
        };

        // Trimming the prepared-art cache is disk work with no deadline, so it goes to a blocking
        // thread and its result is thrown away — nothing on screen waits for it.
        let real = fixtures.is_none();
        let launch = app.launch_context();
        let prune = Task::future(async move {
            let _ = off_thread(covers::prune_caches).await;
            // ES-DE's hooks run a copy of this binary; an update leaves them on the old one until
            // this. Never in fixture mode, which is what the tests run in.
            if real && let Ok(Some(note)) = off_thread(crate::hooks::refresh_if_installed).await {
                eprintln!("deckhand: {note}");
            }
            // The saves and playtime of a game that ended along with the Deckhand that ran it — see
            // `crate::playing`. Not in fixture mode either.
            if real {
                crate::launch::finish_left(&launch).await;
            }
        })
        .discard();

        // Deckhand's own new versions, asked for at every launch — see `release`. Not in fixture mode,
        // which is what the tests run in.
        let release = if real { app.check_release() } else { Task::none() };

        // Measure the window rather than assuming it opened at the default size. No resize event
        // fires for the *initial* size, so a window restored wider than the default would lay out
        // its grid against a stale width until the first drag — and the density decides how much of
        // each cover is worth decoding, so it is asked for in the same breath.
        let measure = Task::batch([
            prune,
            release,
            iced::window::latest()
                .and_then(iced::window::size)
                .map(|size| Message::Resized(size.width, size.height)),
            iced::window::latest()
                .and_then(iced::window::scale_factor)
                .map(Message::PanelDensity),
        ]);

        if let Some(count) = fixtures {
            // A synthetic session is not paired with anything. The stored connection is dropped
            // rather than carried, because every path that reads one would otherwise reach a real
            // server with rom ids that only exist here — a download press most of all.
            app.settings.connection = None;
            app.library = Arc::new(crate::fixtures::library(count));
            app.rebuild_collections();
            app.rebuild_shelves();
            app.screen = Screen::Browsing(Route::Home);
            app.follow_spotlight();
            app.say(format!("Fixture mode — {count} synthetic games, no server."));
            return (app, measure);
        }

        // Deckhand's own shortcut in Steam, when the player added one, gets its art.
        let measure = Task::batch([measure, dress_own_shortcut()]);

        if !paired {
            return (app, measure);
        }

        // Whatever the last run left queued goes back on the wire now rather than waiting for a
        // press — see [`Downloads::load`]. It needs only the connection, not the library, because a
        // queue item carries its own title.
        let resume = app.pump_downloads();

        // A cached library opens Home in the first frame, and the server is reconciled behind it.
        // The alternative — showing the loading screen until the walk finishes — spends a minute
        // hiding a library that is almost always still correct.
        let cached = app.settings.connection.as_ref().and_then(|c| cache::load(&c.host));
        if let Some(cached) = cached {
            app.library = Arc::new(cached.library());
            app.match_downloads();
            app.favourites = favourites_of(&cached.collections);
            app.collections = cached.collections;
            // RomM's smart and generated collections as it last gave them: the Collections page opens
            // on these, and the sync behind Home asks for them again.
            if let Some(sources) = app.settings.connection.as_ref().and_then(|c| cache::load_sources(&c.host)) {
                app.collection_sources = Some(Ok(sources));
            }
            app.rebuild_collections();
            app.system_counts = cached.counts;
            app.library_read_at = cached.read_at;
            app.rebuild_shelves();
            app.screen = Screen::Browsing(Route::Home);
            app.follow_spotlight();
            // The cache's text and its parse are done with: given back — see `memory`.
            crate::memory::release();
            let device = app.ensure_device();
            let detect = app.detect_games();
            let awarded = app.load_awarded();
            let installs = Task::done(Message::ClaimInstalls);
            return (app, Task::batch([measure, resume, Task::done(Message::SyncLibrary), device, detect, awarded, installs]));
        }

        (app, Task::batch([measure, resume, Task::done(Message::LoadLibrary)]))
    }

    /// A short name for the current screen, for test failure messages.
    #[cfg(test)]
    fn screen_name(&self) -> String {
        match &self.screen {
            Screen::Login(_) => "login".into(),
            Screen::Loading { .. } => "loading".into(),
            Screen::Browsing(route) => format!("{route:?}"),
        }
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Browsing(_) if !self.library.is_empty() => {
                format!("Deckhand — {} games", self.library.total_games)
            }
            _ => "Deckhand".into(),
        }
    }

    pub fn theme(&self) -> iced::Theme {
        theme::deckhand()
    }

    /// The window's own colours: the theme's, except that every scroll nudges the background by
    /// less than one 8-bit step.
    ///
    /// The nudge is for tiny-skia. It redraws only what changed since the last frame, which suits a
    /// cursor moving over a still page — but a scroll moves everything in the scrolled area, and
    /// the renderer reports that as dozens of scattered rectangles rather than one. It then draws
    /// each separately, re-rasterising every cover that touches it: a Browse scroll frame measured
    /// ~230 ms drawn in pieces against ~35 ms drawn whole. A background that differs from the last
    /// frame's is the one thing it takes to mean "redraw the whole window", so a scroll makes it
    /// differ.
    ///
    /// Both backends round the nudge back to the same pixels, and wgpu redraws every frame whole
    /// anyway, so to it this is nothing. The count cycles through 64 values, not two: two scrolls
    /// between frames would put a parity back where it started, and that frame would go back to
    /// being drawn in pieces.
    pub fn style(&self, theme: &iced::Theme) -> iced::theme::Style {
        use iced::theme::Base;

        let mut style = theme.base();
        style.background_color.r += (self.scrolls % 64) as f32 * 1e-5;
        style
    }

    /// The UI-scale ladder is applied here rather than per-widget, so every size in `theme` stays
    /// the literal spec number and one setting moves all of them together.
    /// iced's hook: the scale to lay the page out at. Remembered as it is handed over, because
    /// iced applies it then — see [`App::applied_scale`].
    pub fn scale_factor(&self) -> f32 {
        let scale = self.layout_scale();
        self.applied_scale.set(scale);
        scale
    }

    /// Open as the whole screen — see [`App::layout_scale`].
    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
    }

    /// How many of the window's logical pixels a layout unit takes: the UI scale — or, filling the
    /// screen, whatever fits the 1280x800 the layout is designed against into it.
    ///
    /// A window of its own is sized to the design, 1280x800 times the UI scale, and a resize
    /// reflows the page. The screen is whatever size it is, in whatever units the system says:
    /// Game Mode reports a Deck's panel at a scale of about 2, from its density, which left half
    /// the design's width to lay out in and drew the page twice as big. Fitted, the Deck gets the
    /// page it was designed for, a 16:9 TV the same 800 units of height a little wider, and a 4K
    /// one the same at its size.
    ///
    /// At the default step the screen holds the design exactly — what Game Mode showed before the
    /// window filled the screen, when gamescope shrank a window of 1280x800 times the UI scale to
    /// fit — and the other steps draw larger or smaller from there.
    ///
    /// The fit is taken against the window in the *system's* pixels, which iced never reports: it
    /// divides every size by the scale it has applied, this app's included — see
    /// [`Message::Resized`]. Fitted against iced's sizes directly, the page fed back into itself: a
    /// Deck settled at 87% whatever the step, and a screen that sent one more resize swung to twice
    /// the size.
    pub(super) fn layout_scale(&self) -> f32 {
        let ui = self.settings.ui_scale.max(0.1);
        let (width, height) = self.window;
        if !self.fullscreen || width <= 0.0 || height <= 0.0 {
            return ui;
        }
        let fit = (width / crate::PANEL.0).min(height / crate::PANEL.1);
        fit * ui / crate::theme::ui_scale::DEFAULT
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Heard on every screen, from the first: listening is what stops a signal ending the window
        // on the spot, mid-sync — see `App::stop_asked`.
        let stops = Subscription::run(crate::launch::stop_signals).map(|()| Message::StopAsked);
        Subscription::batch([stops, self.screen_subscription()])
    }

    fn screen_subscription(&self) -> Subscription<Message> {
        // The login screen's text inputs need the keys for themselves — grabbing arrows there would
        // break moving the caret inside the URL field — so it hears only the pad, and the keys only
        // while the on-screen keyboard or a QR code is up.
        if let Screen::Login(state) = &self.screen {
            let pad = Subscription::run(crate::gamepad::stream).map(Message::Pad);
            if self.entry.is_none() && state.qr.is_none() {
                return pad;
            }
        } else if !matches!(self.screen, Screen::Browsing(_)) {
            return Subscription::none();
        }
        // Resizes matter even on the pages that do not lay out a grid: the next one that does
        // needs the current width, and there is no resize event waiting for it when it opens.
        let resizes =
            iced::window::resize_events().map(|(_id, size)| Message::Resized(size.width, size.height));
        let rescales = iced::event::listen_with(|event, _, _| {
            matches!(event, iced::Event::Window(iced::window::Event::Rescaled(_))).then_some(Message::Rescaled)
        });

        // 0.14 removed `on_key_press`; the key-press events come off the general listener now.
        let keys = keyboard::listen().filter_map(|event| {
            let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
                return None;
            };
            match key {
            keyboard::Key::Named(Named::ArrowUp) => Some(Message::Nav(Direction::Up)),
            keyboard::Key::Named(Named::ArrowDown) => Some(Message::Nav(Direction::Down)),
            keyboard::Key::Named(Named::ArrowLeft) => Some(Message::Nav(Direction::Left)),
            keyboard::Key::Named(Named::ArrowRight) => Some(Message::Nav(Direction::Right)),
            keyboard::Key::Named(Named::Enter) => Some(Message::Activate),
            keyboard::Key::Named(Named::Escape) => Some(Message::Dismiss),
            // The Downloads page's own key, and the only one in the app that takes something away —
            // which is why that page names it on screen rather than leaving it to be found. The
            // handler ignores it everywhere else.
            keyboard::Key::Named(Named::Delete) => Some(Message::RemoveFocusedDownload),
            // Tab walks every destination, as the pad's shoulder buttons do.
            keyboard::Key::Named(Named::Tab) => {
                Some(Message::CycleTab(if modifiers.shift() { -1 } else { 1 }))
            }
            // Every printable key goes through one message, because what it means depends on the
            // page — a letter of the query on Search, a shortcut anywhere else — and this closure
            // cannot see which page is open.
            keyboard::Key::Character(c) => Some(Message::KeyChar(c.to_string(), modifiers.control())),
            keyboard::Key::Named(Named::Space) => Some(Message::KeyChar(" ".into(), modifiers.control())),
            keyboard::Key::Named(Named::Backspace) => {
                Some(Message::KeyChar(crate::search::BACKSPACE.to_string(), false))
            }
                _ => None,
            }
        });

        let pad = Subscription::run(crate::gamepad::stream).map(Message::Pad);
        let mut subscriptions = vec![keys, resizes, rescales, pad];

        // Frames, but only while a fade is actually running. `subscription` is re-read after every
        // message, so this attaches itself when the cursor moves and drops off again on the frame
        // the ramp lands — an idle Home does not sit there redrawing at 60 fps for nothing, which
        // matters on a handheld running off a battery.
        // Measuring wants every frame the compositor will give, not only the ones a fade asked
        // for — an idle page that redraws twice a second measures nothing. A handover's sweep runs
        // for the few seconds it is up, which is what says the window has not hung.
        let now = std::time::Instant::now();
        if self.frame_log.is_some()
            || self.handover.is_some()
            || self.fade.is_animating(now)
            || self.backdrop.is_animating(now)
            || self.glide.is_animating(now)
        {
            subscriptions.push(iced::window::frames().map(|_| Message::Redraw));
        }

        // Saves synced as they change — see `watch`. Only for the Deckhand on the device that holds
        // the watch, and not while a game started here runs: its launch syncs when it ends.
        if self.watch_lock.is_some() && self.settings.watch_saves && self.settings.connection.is_some() && self.playing.is_none() {
            subscriptions.push(iced::time::every(crate::watch::EVERY).map(|_| Message::WatchTick));
        }

        // Installs queued for this device in RomM's web app — by the Deckhand that watches saves, so
        // two on one device never take the same one, and not while a game started here runs.
        if self.watch_lock.is_some() && self.playing.is_none() && self.installs.taking() {
            subscriptions.push(iced::time::every(installs::EVERY).map(|_| Message::ClaimInstalls));
        }

        // The banner's time, only while there is one up.
        if self.banner.is_some() {
            subscriptions.push(iced::time::every(BANNER_TICK).map(|_| Message::BannerTick));
        }

        // Asking again while the window stays open: a Deck can sit in Game Mode for days.
        {
            use crate::release::Status;
            if matches!(self.release, Status::Current | Status::Out(_) | Status::Failed(..)) {
                subscriptions.push(iced::time::every(crate::release::EVERY).map(|_| Message::CheckRelease));
            }
        }

        // While the cursor is in the bar, the mouse being used is the way back into the page — its
        // own events, not a card's hover: a card reports the pointer entering it when the page moves
        // under a pointer resting on it, which pulled the cursor back down off the bar as it got
        // there.
        if self.chrome.is_some() {
            subscriptions.push(iced::event::listen_with(|event, _status, _window| {
                use iced::mouse::Event as Mouse;
                match event {
                    iced::Event::Mouse(Mouse::CursorMoved { .. } | Mouse::ButtonPressed(_) | Mouse::WheelScrolled { .. }) => {
                        Some(Message::PointerUsed)
                    }
                    _ => None,
                }
            }));
        }

        Subscription::batch(subscriptions)
    }

    /// The games matching the Search page's query, best first. Ranked once per query and library,
    /// however many times a frame asks.
    fn search_results(&self) -> Vec<&Game> {
        let ids = {
            let mut cache = self.search_cache.borrow_mut();
            let cache = &mut *cache;
            let stale = cache.index.as_ref().is_none_or(|index| index.generation != self.library.generation());
            if stale {
                cache.index = Some(crate::search::Index::of(&self.library));
            }
            if stale || cache.query != self.search_focus.query {
                cache.query.clone_from(&self.search_focus.query);
                cache.results = cache.index.as_ref().map(|index| index.rank(&cache.query)).unwrap_or_default();
            }
            cache.results.clone()
        };
        ids.into_iter().filter_map(|id| self.library.game(id)).collect()
    }

    /// The game in Home's spotlight.
    fn spotlit_id(&self) -> Option<i64> {
        self.focus.spotlit(&self.shelves).map(|game| game.rom_id)
    }

    /// Point the backdrop at the spotlit game: up at once when its picture is in, otherwise the
    /// moment it lands, so a slow picture does not fade the old one out to nothing first.
    pub(super) fn follow_spotlight(&mut self) {
        let Some(id) = self.spotlit_id() else { return };
        if self.spotlight_ready(id) {
            self.backdrop.show(id, std::time::Instant::now());
        } else {
            self.backdrop.wait_for(id);
        }
    }

    /// Whether the spotlight's picture for `rom_id` has come back — with art, or without it, which
    /// is as final an answer.
    pub(super) fn spotlight_ready(&self, rom_id: i64) -> bool {
        use crate::ui::home::{picture_of, Picture};
        match self.library.game(rom_id).and_then(picture_of) {
            None => true,
            Some(Picture::Shot(url)) => self.covers.settled(&covers::key(url, covers::Kind::Spotlight)),
            Some(Picture::Cover(url)) => self.covers.settled(&covers::key(url, covers::Kind::Feature)),
        }
    }

    /// What the cursor is on, for the cross-fade.
    ///
    /// Derived from the focus models rather than tracked alongside them, so there is nothing to
    /// keep in step: every message that moves the cursor moves this with it, for free.
    fn cursor(&self) -> Cursor {
        match &self.screen {
            // The spotlit card, whether the cursor is on it or up on the buttons that are about it:
            // stepping up to them is not a move away from the card, which stays lit and lifted.
            Screen::Browsing(Route::Home) => {
                let shelf = self.focus.shelf_pos();
                match self.shelves.get(shelf) {
                    Some(s) => Cursor::HomeCard(shelf, self.focus.index_of(s)),
                    None => Cursor::Elsewhere,
                }
            }
            Screen::Browsing(Route::Browse { .. })
                if self.browse_focus.zone() == crate::focus::BrowseZone::Grid =>
            {
                Cursor::BrowseTile(self.browse_focus.index())
            }
            _ => Cursor::Elsewhere,
        }
    }

    /// Dispatch a message, then start a cross-fade if it moved the cursor.
    ///
    /// The move is detected here, by comparing before and after, rather than announced by each of
    /// the dozen arms that can cause one — `Nav`, `CycleTab`, `CycleSystem`, `JumpToLetter`,
    /// `OpenSystem`, `Back`, a filter change that re-seats the cursor, and a library refresh that
    /// reconciles it. Any arm that forgot to say so would leave that one move un-animated, and the
    /// bug would be invisible until someone happened to look.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        // A conflict on screen takes the answering keys for itself — the game is waiting on it.
        if self.conflict.is_some()
            && let Some(answer) = self.answer_conflict(&message)
        {
            return answer;
        }
        // A game being handed over has the window: a key pressed then is for no page under it — the
        // pad's are already the game's while it is being played.
        if self.handover.is_some()
            && matches!(
                message,
                Message::Nav(_)
                    | Message::Activate
                    | Message::Dismiss
                    | Message::KeyChar(..)
                    | Message::CycleTab(_)
                    | Message::CycleSystem(_)
                    | Message::RemoveFocusedDownload
                    | Message::OpenSearch
            )
        {
            return Task::none();
        }
        // So does a text entry, for every key — and a picker, for the keys that walk and answer it.
        if self.entry.is_some()
            && let Some(answer) = self.answer_entry(&message)
        {
            return answer;
        }
        if self.pick.is_some()
            && let Some(answer) = self.answer_pick(&message)
        {
            return answer;
        }
        if matches!(self.screen, Screen::Login(_))
            && let Some(answer) = self.login_pad(&message)
        {
            return answer;
        }
        let before = self.cursor();
        let route_before = self.route_hash();
        let spotlit_before = self.spotlit_id();
        let mut task = self.dispatch(message);
        // A banner that went while the pad's cursor was on its Dismiss leaves the cursor on the bar.
        if self.chrome == Some(Chrome::Banner) && self.banner.is_none() {
            self.chrome = Some(Chrome::Tabs);
        }
        let after = self.cursor();
        let now = std::time::Instant::now();
        if before != after {
            self.fade.moved(before, now);
        }
        // Home's column follows the shelf the cursor is on, however it got there — a step, a click,
        // the wheel, a refresh that took a shelf away. A no-op when it is already there; and off
        // Home it is simply there, rather than sliding where nobody can see it.
        if matches!(self.screen, Screen::Browsing(Route::Home)) {
            self.glide.go(self.focus.shelf_pos(), now);
        } else {
            self.glide.jump(self.focus.shelf_pos());
        }
        // And the spotlight follows the game: its picture, and its neighbours', fetched ahead.
        if self.spotlit_id() != spotlit_before {
            self.follow_spotlight();
            task = Task::batch([task, self.fetch_art()]);
        }

        // Landing on a different page asks for that page's art, here rather than in the dozen arms
        // that can move you. The budget drops art that has been off screen a while, so a page
        // returned to is routinely a page whose covers have to be fetched again - and three arms
        // did not: `Back` out of a detail page, and either way into Home, which is exactly the
        // walk that left a system you had already loaded showing empty boxes.
        //
        // Costs nothing when nothing was dropped: `Covers::claim` filters out everything held or
        // already in flight, and `fetch_art` returns an empty task when that leaves no jobs.
        if route_before != self.route_hash() {
            // The hover is keyed by position, and a position on the page just left names a
            // different card, or none, on this one. Whatever the pointer rests on says so again as
            // soon as it moves.
            self.hovered = None;
            return Task::batch([task, self.fetch_art()]);
        }
        task
    }

    /// The route being shown, hashed — what `update` compares across a message to tell a page
    /// change, without a copy of the route for every message (frames and covers included).
    fn route_hash(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        self.route().map(|route| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            route.hash(&mut hasher);
            hasher.finish()
        })
    }

    /// The route being shown, if the app is past the login and loading screens.
    fn route(&self) -> Option<&Route> {
        match &self.screen {
            Screen::Browsing(route) => Some(route),
            _ => None,
        }
    }

    /// Put `words` up in the banner, for [`BANNER_LASTS`] from now — however long the one it
    /// replaces had been up, the same words again included.
    pub(super) fn say(&mut self, words: String) {
        self.banner = Some(words);
        self.banner_since = Some(std::time::Instant::now());
    }

    /// Take the banner down once it has been up for [`BANNER_LASTS`].
    fn banner_tick(&mut self, now: std::time::Instant) {
        if self.banner.is_none() {
            return;
        }
        match self.banner_since {
            Some(since) if now.duration_since(since) >= BANNER_LASTS => {
                self.banner = None;
                self.banner_since = None;
            }
            Some(_) => {}
            None => self.banner_since = Some(now),
        }
    }
}

/// Run `work` on tokio's blocking pool — the disk, process and config work the app keeps off the UI
/// thread. A `work` that panicked answers why, for its caller to say or fall back from.
/// Give the shortcut that starts Deckhand in Steam Deckhand's art — see [`crate::steam::dress_own`].
/// Nothing is said either way: it is done again at the next start.
fn dress_own_shortcut() -> Task<Message> {
    Task::future(off_thread(|| {
        let (Some(steam), Some(deckhand)) = (crate::steam::Steam::find(), crate::steam::deckhand_path()) else { return };
        let _ = crate::steam::dress_own(&steam, &deckhand, !crate::steam::running());
    }))
    .discard()
}

async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("it stopped unexpectedly ({e})"))
}

/// The paired server's address, or "" when there is none — what keys this device's download
/// records, since a RomM rom id means nothing without the install it came from.
fn settings_host(settings: &Settings) -> String {
    settings.connection.as_ref().map(|c| c.host.clone()).unwrap_or_default()
}

/// Pair with a server, trying each host candidate in turn.
///
/// Standalone rather than inline in the task so the whole round trip can be tested against a stub
/// server — this is the one path in the app where a mistake means nobody can get in at all.
async fn pair(
    client: &RommClient,
    candidates: Vec<String>,
    code: String,
    insecure: bool,
) -> Result<Connection, String> {
    if candidates.is_empty() {
        return Err("Enter your RomM server address.".to_string());
    }
    let mut last: Option<ApiError> = None;
    for host in candidates {
        match client.exchange_pairing_code(&host, &code, insecure).await {
            Ok(paired) => {
                let connection = Connection {
                    host: host.clone(),
                    token: paired.raw_token.unwrap_or_default(),
                    insecure,
                    token_id: paired.id,
                    token_name: paired.name,
                    device_id: None,
                };
                // Prove the token reads before it is written to disk. A token that pairs but cannot
                // read is worse than none: the app would open on an empty library rather than back
                // on the login screen where it can be fixed.
                return match client.fetch_current_user(&connection).await {
                    Ok(_) => Ok(connection),
                    Err(e) => Err(e.to_string()),
                };
            }
            // A transport failure means this candidate was never reached, so the next scheme is
            // worth trying. An HTTP status means we reached the right server and it said no —
            // retrying the same code over plaintext would only burn a code valid once.
            Err(e) if e.is_transport() => last = Some(e),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(last
        .map(|e| e.to_string())
        .unwrap_or_else(|| "Could not reach the server.".into()))
}

/// Scroll the Browse system list so system `index` is in view.
///
/// With one more row showing beyond it on the side being walked toward, so the list always shows
/// where the next step goes — and, walking down, that there is a next step at all.
fn reveal_system_row(index: usize) -> Task<Message> {
    use crate::ui::browse::{system_list_id, system_row_span, SYSTEM_ROW_PITCH};
    let (top, bottom) = system_row_span(index);
    reveal::reveal(system_list_id(), top, bottom, SYSTEM_ROW_PITCH).discard()
}

fn loading_view(status: &str) -> Element<'_, Message> {
    container(
        column![
            iced::widget::image(theme::logo()).width(Length::Fixed(96.0)).height(Length::Fixed(96.0)),
            Space::new().height(Length::Fixed(14.0)),
            text("Deckhand").size(theme::text_size::DISPLAY_SMALL).font(theme::EXTRABOLD),
            Space::new().height(Length::Fixed(10.0)),
            text(status)
                .size(theme::text_size::BODY_LARGE)
                .color(theme::TEXT_SECONDARY),
        ]
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

/// A game on its way into its emulator, or back out of it, while its BIOS and saves move. The whole
/// window says so, as the Kotlin app's does: those seconds have nothing else moving, and the page
/// under them invites a second press — or, on the way out, closing Deckhand before the session's
/// save is up.
#[derive(Debug, Clone, Copy)]
pub(super) struct Handover {
    pub(super) rom_id: i64,
    /// Back from the game, its save going up — rather than on the way in.
    pub(super) closing: bool,
    /// When it began, which the bar's sweep is timed from.
    pub(super) since: std::time::Instant,
}

impl Handover {
    pub(super) fn new(rom_id: i64, closing: bool) -> Self {
        Handover { rom_id, closing, since: std::time::Instant::now() }
    }
}

/// A choice put to the player as a list, and what answering it does.
struct PendingPick {
    picker: crate::ui::picker::Picker,
    then: Then,
}

/// What a picker's answer does, with what it chooses among — row for row.
enum Then {
    /// Queue what was chosen of the game whose tile is `tile`.
    Download { tile: i64, choices: Vec<downloads::choice::Choice> },
    /// Play the build chosen.
    Play { builds: Vec<i64> },
    /// Remove the build chosen.
    Remove { builds: Vec<i64> },
    /// Send downloads to the place chosen — or, past the last of them, walk to another folder.
    Location { places: Vec<downloads::storage::Location> },
    /// One folder of a walk through this device's folders: go on into the one chosen, or take it.
    Browse { purpose: Browsing, dir: std::path::PathBuf, steps: Vec<crate::folders::Step> },
    /// Read the RetroArch config chosen.
    RetroArch { choices: Vec<RetroArchChoice> },
    /// RetroDECK starts games, or a command of the player's own does.
    Launcher,
    /// The system chosen, to give an emulator of its own: ES-DE system names, row for row.
    SystemEmulators { systems: Vec<String> },
    /// How `system`'s emulator is set.
    SystemEmulator { system: String, choices: Vec<EmulatorChoice> },
    /// Put this older version of a save back: the first row says yes.
    Restore { tile: i64, version: Box<crate::romm::models::SaveAsset> },
    /// Put this state from RomM into a slot: the next free one, slot 0, or the auto slot, row for
    /// row; the last row leaves it.
    PutState { tile: i64, state: Box<crate::romm::models::SaveAsset> },
    /// Close Steam, put these games in its library (or take them out), and open it again: the first
    /// row says yes.
    Steam { games: Vec<i64>, add: bool },
    /// Put the extra chosen of the build `rom_id` on this device, or take it off when it is here.
    Extras { rom_id: i64, extras: Vec<Extra> },
}

/// One of a build's extras, as its game's page offers it.
#[derive(Debug, Clone)]
struct Extra {
    /// RomM's id for the file.
    id: i64,
    /// Where it goes, relative to the platform folder — see [`downloads::Installed::files`].
    path: String,
    /// Its category: "Manual", "Hack".
    label: String,
    /// The file's name on RomM: a category alone doesn't tell two hacks apart.
    name: String,
    bytes: u64,
    here: bool,
}

/// What the watched folders were worked out from: the RetroArch config chosen, and the games here.
type WatchFingerprint = (Option<std::path::PathBuf>, u64);

/// How long the watched folders are kept before they are worked out again.
const WATCH_FOLDERS_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// What Browse's view of a scope was worked out from — see [`App::browse_entry`].
#[derive(Debug, Clone, PartialEq)]
struct BrowseKey {
    scope: Scope,
    filter: crate::view_model::Filter,
    sort: crate::view_model::Sort,
    library: u64,
    installed: u64,
    favourites: u64,
}

/// Browse's view of a scope, kept between frames: the games in order (as places in the library),
/// the A-Z index, the chips' counts and the grid's cover shape.
struct BrowseEntry {
    key: BrowseKey,
    games: Vec<usize>,
    letters: Vec<(char, usize)>,
    counts: crate::ui::browse::ChipCounts,
    ratio: f32,
}

/// What [`App::search_results`] keeps between asks.
#[derive(Default)]
struct SearchCache {
    index: Option<crate::search::Index>,
    /// The query the results are for.
    query: String,
    results: Vec<i64>,
}

/// Where the pad's cursor is when it is above the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chrome {
    /// On the tab bar: Left/Right go to the neighbouring destination, Up to the banner.
    Tabs,
    /// On the banner's Dismiss.
    Banner,
}

/// What a walk through the folders is choosing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Browsing {
    /// Where downloads go.
    RomsFolder,
    /// The `retroarch.cfg` save sync reads.
    RetroArchConfig,
    /// The program that runs this ES-DE system's games.
    Emulator(String),
}

/// A row of a system's emulator picker.
#[derive(Debug, Clone, PartialEq)]
enum EmulatorChoice {
    /// Walk to the emulator's file, from here.
    Pick(std::path::PathBuf),
    /// Type the command.
    Type,
    /// No command of its own: the system starts the way every game does.
    Shared,
}

/// A row of the RetroArch config picker.
#[derive(Debug, Clone, PartialEq)]
enum RetroArchChoice {
    /// Whichever is found first, as before anything was chosen.
    Auto,
    Use(std::path::PathBuf),
    /// Walk the folders to one.
    Browse,
}

/// A setting being typed in, and which.
struct PendingEntry {
    entry: crate::ui::entry::Entry,
    purpose: Typing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Typing {
    LaunchCommand,
    /// The command for one ES-DE system's games.
    SystemCommand(String),
    /// The login screen's two fields, from the pad.
    LoginUrl,
    LoginCode,
}

/// A save that changed on both sides, waiting on the player.
struct PendingConflict {
    rom_id: i64,
    conflict: crate::sync::Conflict,
    reply: crate::launch::Reply,
    /// The side the cursor is on.
    keep: crate::sync::Choice,
}

/// The chooser: the two copies side by side, when each was made and how big, the newer marked.
/// The whole window — the game is waiting on it, and nothing else can be done meanwhile.
fn conflict_view<'a>(pending: &PendingConflict, title: &str) -> Element<'a, Message> {
    use crate::sync::Choice;
    let when = |t: Option<std::time::SystemTime>| t.map_or_else(|| "unknown".to_string(), |t| crate::sync::format_timestamp(t).replace('T', " ").replace('Z', " UTC"));
    let side = |choice: Choice, label: &str, detail: String| {
        let lit = pending.keep == choice;
        let newer = pending.conflict.newer == choice;
        button(
            column![
                text(label.to_string()).size(theme::text_size::TITLE_MEDIUM).font(theme::MEDIUM).color(theme::TEXT_PRIMARY),
                text(detail).size(theme::text_size::LABEL_LARGE).color(theme::TEXT_SECONDARY),
                text(if newer { "newer" } else { "" }).size(theme::text_size::LABEL_MEDIUM).font(theme::BOLD).color(theme::ACCENT),
            ]
            .spacing(6),
        )
        .width(Length::Fixed(300.0))
        .padding(18)
        .on_press(Message::ConflictChosen(choice))
        .style(crate::ui::style::row(lit, 14.0))
    };
    let c = &pending.conflict;
    let card = column![
        text(format!("\"{title}\": {} changed here and on RomM", c.name))
            .size(theme::text_size::TITLE_MEDIUM)
            .font(theme::MEDIUM)
            .color(theme::TEXT_PRIMARY),
        text("Which one should the game use? The other is kept: RomM holds its copy in the game's history, and this device's is copied to save-backups first.")
            .size(theme::text_size::BODY_MEDIUM)
            .color(theme::TEXT_SECONDARY),
        row![
            side(Choice::Local, "This device", format!("{} · {}", when(c.local_time), crate::downloads::human_bytes(c.local_bytes))),
            side(Choice::Remote, "RomM", when(c.remote_time)),
        ]
        .spacing(16),
        text("Left / Right to choose  ·  Enter or A keeps it  ·  Esc or B keeps the newer")
            .size(theme::text_size::LABEL_MEDIUM)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(16)
    .max_width(680);
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(theme::BACKGROUND)), ..Default::default() })
        .into()
}

/// How long a banner stays up, as long as the Kotlin app's: enough to read a line, short enough
/// that the window is soon clear again without a press.
pub const BANNER_LASTS: std::time::Duration = std::time::Duration::from_secs(6);

/// How often the banner's time is looked at — so it goes within this of [`BANNER_LASTS`].
const BANNER_TICK: std::time::Duration = std::time::Duration::from_millis(500);

/// The banner, and its Dismiss — which hides it and does nothing else, whatever page is open.
/// `focused` is the pad's cursor on Dismiss, up from the tab bar.
fn banner_view(message: &str, focused: bool) -> Element<'_, Message> {
    let dismiss: Element<'_, Message> = if focused {
        button(text("Dismiss").size(theme::text_size::LABEL_LARGE))
            .padding([6, 12])
            .on_press(Message::DismissBanner)
            // Lit the way the page's main buttons are when the cursor is on them: a ring on this dark
            // strip was easy to miss from a couch.
            .style(crate::ui::style::action(true, true))
            .into()
    } else {
        quiet_button("Dismiss", Message::DismissBanner)
    };
    container(
        row![
            text(message)
                .size(theme::text_size::LABEL_LARGE)
                .color(theme::TEXT_PRIMARY),
            Space::new().width(Length::Fill),
            dismiss,
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([8, 18]),
    )
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(theme::ACCENT_900)),
        border: Border { color: theme::ACCENT_700, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    })
    .into()
}

fn quiet_button<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(theme::text_size::LABEL_LARGE)).padding([6, 12]).on_press(message).style(crate::ui::style::quiet).into()
}

//! Two-way save sync with RomM. Every game downloaded here has its saves found, packed and put back
//! by argosy-sigil, whatever runs it — see [`units`]; RetroArch's other `.srm` files, of games
//! played here that were not downloaded through this app, sync as the files they are.
//!
//! A port of the per-game half of the Kotlin app's `SaveSyncEngine` (`syncGameSaves`), the half
//! that decides from facts rather than from clocks:
//!
//! - **What changed is decided by content.** The index remembers, for each save, the hash of the
//!   content last on both sides and the RomM save that content is. A local file whose hash differs
//!   has changed here; a newest server save with a different id has changed there. An mtime says
//!   when a file was written, not whether it holds anything new — a core rewriting unchanged SRAM
//!   on exit moves it — and clocks drift between devices.
//! - **A conflict is both sides changing**, and only that reaches a decision. It is settled
//!   newest-first, the one place a clock is read, and only between two versions that each hold
//!   progress. Neither is lost: RomM keeps a history of ten, and a local copy that loses is copied
//!   aside before it is overwritten — which the Kotlin app does not do.
//! - **A sweep creates nothing.** [`run`] refreshes or pushes only saves already on this device: a
//!   game with a save on RomM and none here is left alone, because a sweep cannot tell whether that
//!   game is on this device at all. [`sync_game`] can — it is handed the game's content file — so it
//!   is the one that brings a save down to a device that has never had one.
//!
//! Where saves are and which core wrote them comes from [`retroarch`]; which game a save belongs to
//! comes from the library's file stems, so matching costs no request — see [`Matcher`]. The two
//! entry points are the Settings page's sweep and [`crate::cli`]'s launch wrapper.
//!
//! Save states sync per game alongside its save, in [`states`] — around a launch, and in a sweep
//! narrowed to what changed, for the games whose own states are among it ([`changed_states`]). A
//! whole-device sweep leaves them to a launch: RomM has no negotiation for states, so each game
//! would cost a request of its own.
//!
//! A conflict is put to a [`Resolver`] when one is in scope — the window's chooser, around a launch
//! ([`with_resolver`]) — and otherwise keeps the newer side.

mod index;
pub(crate) mod names;
pub mod device;
pub mod hash;
pub mod session;
pub mod standalone;
pub mod states;
pub mod units;
pub use names::{canonical, format_timestamp, parse_timestamp};
#[cfg(test)]
pub use names::format_timestamp_precise;
#[cfg(test)]
mod scenarios;
pub mod retroarch;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use md5::{Digest, Md5};

use crate::config::Connection;
use crate::library::{Library, Stamp};
use crate::romm::models::{PlaySession, SaveAsset};
use crate::romm::{ApiError, RommClient};
use index::{Entry, Index};
use retroarch::{GameSaves, Install, LocalSave};
use session::{Outcome, Place, Thread};

/// A save that changed on both sides since they last agreed, as it is put to whoever decides.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The save's file name — `Chrono Trigger (USA).srm`, `Chrono Trigger (USA).state2`.
    pub name: String,
    pub local_time: Option<SystemTime>,
    pub remote_time: Option<SystemTime>,
    pub local_bytes: u64,
    /// Which side is newer by the clocks — what is kept when nobody is asked.
    pub newer: Choice,
}

/// Which side of a [`Conflict`] is kept. The other is never lost: RomM keeps its copy in the game's
/// history, and this device's goes to `save-backups/` first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Local,
    Remote,
}

/// Something that answers a [`Conflict`] — the window's chooser, around a launch.
pub type Resolver = std::sync::Arc<dyn Fn(Conflict) -> std::pin::Pin<Box<dyn std::future::Future<Output = Choice> + Send>> + Send + Sync>;

tokio::task_local! {
    /// The resolver for a sync running inside [`with_resolver`]. Everything else — a sweep, the
    /// launch wrapper — keeps the newer side without asking: nobody is there to ask.
    static RESOLVER: Resolver;
}

/// Run `work` with conflicts put to `resolver`.
pub async fn with_resolver<F: std::future::Future>(resolver: Resolver, work: F) -> F::Output {
    RESOLVER.scope(resolver, work).await
}

/// Decide a conflict: the resolver in scope if there is one, else the newer side. Unknown times keep
/// this device's — RomM holds the other in its history regardless.
async fn choose(name: &str, local_time: Option<SystemTime>, remote_time: Option<SystemTime>, local_bytes: u64) -> Choice {
    let newer = if matches!((remote_time, local_time), (Some(r), Some(l)) if r > l) { Choice::Remote } else { Choice::Local };
    let conflict = Conflict { name: name.to_string(), local_time, remote_time, local_bytes, newer };
    match RESOLVER.try_with(|r| r.clone()) {
        Ok(resolver) => resolver(conflict).await,
        Err(_) => newer,
    }
}

/// The RomM saves and states this device holds right now: what its sync last agreed with RomM on,
/// for every save, state and standalone save set still on disk. Read from the three indexes, so it
/// costs no request — and it is exact, since an entry records the very save id the content came from
/// or went up as.
pub fn on_device(store: &Path) -> HashSet<i64> {
    ["save-index.json", "state-index.json", "standalone-index.json"]
        .iter()
        .flat_map(|file| {
            Index::load(store.join(file))
                .iter()
                .filter(|(key, entry)| {
                    // A set that lies flat on a card it shares records its own files: a Dolphin
                    // memory card's folder is there as long as Dolphin is, so without them the save
                    // would read as here long after its `.gci`s were deleted. A sigil unit on a PCSX2
                    // folder card records the card, which is a folder.
                    if let Some(files) = &entry.files {
                        return files.iter().any(|file| Path::new(file).exists());
                    }
                    let key = Path::new(key);
                    // A standalone set is keyed on a name no file has, beside or inside the folder it
                    // fills; that folder being there is what "on this device" can mean for it.
                    let synthetic = key.file_name().is_some_and(|n| n == ".deckhand-set") || key.extension().is_some_and(|e| e == "zip");
                    key.is_file() || (synthetic && key.parent().is_some_and(Path::is_dir))
                })
                .map(|(_, entry)| entry.save_id)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// One build of one game, as a save is matched against it: the rom, the file name RomM holds it
/// under (without extension), and the system it is filed under.
#[derive(Debug, Clone)]
pub struct Build {
    pub rom_id: i64,
    pub stem: String,
    pub system_id: String,
    /// When the rom entered RomM, for a tile's own rom — see [`Stamp::born`].
    pub born: Option<String>,
}

impl Build {
    /// Which rom this is — what a save synced as it records, and is held to after.
    pub fn stamp(&self) -> Stamp {
        Stamp { stem: self.stem.clone(), born: self.born.clone() }
    }
}

/// The stamp of the build `rom_id` among `builds`.
pub fn stamp_of(builds: &[Build], rom_id: i64) -> Option<Stamp> {
    builds.iter().find(|b| b.rom_id == rom_id).map(Build::stamp)
}

/// Every build of every game in `library` — siblings too, since a save made with a regional build
/// belongs to that build's rom, not the tile's.
pub fn builds(library: &Library) -> Vec<Build> {
    library
        .all_games().iter()
        .flat_map(|game| {
            game.file_stems.iter().map(|(rom_id, stem)| Build {
                rom_id: *rom_id,
                stem: stem.clone(),
                system_id: game.system_id.clone(),
                born: game.created_at.clone().filter(|_| *rom_id == game.rom_id),
            })
        })
        .collect()
}

/// What one run did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// The folder the saves were read from.
    pub folder: PathBuf,
    pub in_sync: usize,
    pub pushed: Vec<String>,
    pub pulled: Vec<String>,
    /// Saves that changed on both sides, each naming the side that was kept.
    pub conflicts: Vec<String>,
    /// Saves no game in the library is named for, or that several games are.
    pub unmatched: Vec<String>,
    /// Pushes RomM put off — see [`RommClient::upload_save`]. The next run tries them again.
    pub deferred: Vec<String>,
    /// Local copies of saves RomM removed, each saying where the copy was set aside.
    pub removed: Vec<String>,
    /// Saves RomM offered that this device has nowhere to put — no core known to file one under.
    pub homeless: Vec<String>,
    pub failed: Vec<String>,
    /// Save states newer than any this device's core has, which another core made and this one
    /// cannot load, left on RomM — each saying which core — see [`states::Core`].
    pub other_cores: Vec<String>,
    /// Anything else worth saying about the run that is not a count — why nothing happened, say.
    pub notes: Vec<String>,
    /// Play sessions RomM took with the sync session — see [`session::sync`].
    pub plays_recorded: usize,
    /// Every file on this device the run wrote or took away. What the window's watcher is told, so
    /// a sync's own writes are not read back as changes to sync again — and only those, leaving a
    /// save a game wrote while the sync ran the news it is. See [`crate::watch::Watcher::wrote`].
    pub written: Vec<PathBuf>,
}

impl Report {
    /// Fold another run's outcome into this one — a game's states into its save's report.
    fn absorb(&mut self, other: Report) {
        self.in_sync += other.in_sync;
        self.pushed.extend(other.pushed);
        self.pulled.extend(other.pulled);
        self.conflicts.extend(other.conflicts);
        self.unmatched.extend(other.unmatched);
        self.deferred.extend(other.deferred);
        self.removed.extend(other.removed);
        self.homeless.extend(other.homeless);
        self.failed.extend(other.failed);
        self.other_cores.extend(other.other_cores);
        self.notes.extend(other.notes);
        self.plays_recorded += other.plays_recorded;
        self.written.extend(other.written);
    }

    /// One line, for the Settings row: only what happened, in the order it matters.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (self.failed.len(), "failed"),
            (self.conflicts.len(), "settled"),
            (self.pushed.len(), "pushed"),
            (self.pulled.len(), "pulled"),
            (self.removed.len(), "removed"),
            (self.deferred.len(), "waiting on RomM"),
            (self.in_sync, "in sync"),
            (self.unmatched.len(), "not in the library"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        if parts.is_empty() { "no saves found".to_string() } else { parts.join(" · ") }
    }
}

/// Find RetroArch and sync every save on this device — the whole of what "Sync now" does: RetroArch's
/// saves, and every game downloaded here, whatever runs it.
///
/// `store` is where the index and the copies of overwritten saves are kept.
///
/// `changed`, when given, narrows it to the games those files are saves of — what the window's
/// watcher asks for after a save was written, rather than every save on the device again.
pub async fn sync_retroarch(
    client: RommClient,
    connection: Connection,
    settings: crate::config::Settings,
    builds: Vec<Build>,
    store: PathBuf,
    changed: Option<Vec<PathBuf>>,
) -> Result<Report, String> {
    let install = retroarch::locate(settings.retroarch_config.as_deref())?.with_content_dirs(crate::downloads::content_dirs(&store));
    let games = games_here(settings, store.clone(), connection.host.clone()).await;
    run(&client, &connection, &install, &builds, &store, &games, changed.as_deref()).await
}

/// Every game downloaded here from `host`, worked out as a launch works it out — which reads ES-DE's
/// tables, so off the async threads.
pub async fn games_here(settings: crate::config::Settings, store: PathBuf, host: String) -> Vec<crate::launch::Plan> {
    tokio::task::spawn_blocking(move || {
        crate::downloads::installed(&store, &host).iter().map(|installed| crate::launch::Plan::new(&settings, installed)).collect()
    })
    .await
    .unwrap_or_default()
}

/// Sync every save `install` holds against RomM, and every save of `games` — the games downloaded
/// here — through a sync session where RomM holds them ([`session`]); the saves `install` holds, the
/// way before that, where it does not.
///
/// In a session the downloaded games are what RomM is asked about beyond the saves on disk, so a save
/// one of them has on RomM and not here comes down — into the folder of the core that runs it, or the
/// standalone emulator's own.
///
/// An error is only for a run that could not start — RetroArch's folder unreadable, RomM's save
/// list unreachable. A single save that fails is reported in the [`Report`] and the run goes on.
pub async fn run(
    client: &RommClient,
    connection: &Connection,
    install: &Install,
    builds: &[Build],
    store: &Path,
    games: &[crate::launch::Plan],
    changed: Option<&[PathBuf]>,
) -> Result<Report, String> {
    let _one_at_a_time = session::exclusive(store).await?;
    let saves = install.saves();
    let matcher = Matcher::new(builds);
    let index = Index::load(store.join("save-index.json"));
    let mut unmatched = Vec::new();
    // One save per game: RomM pairs a game's save on its slot, and a game played under two cores has
    // a copy in each core's folder. The one played last is the one that syncs.
    let mut chosen: HashMap<i64, &LocalSave> = HashMap::new();
    for save in &saves {
        let Match::One(rom_id) = rom_of(&matcher, &index, save) else {
            unmatched.push(file_name(&save.path));
            continue;
        };
        let newer = |other: &&LocalSave| modified(&save.path) > modified(&other.path);
        if chosen.get(&rom_id).is_none_or(newer) {
            chosen.insert(rom_id, save);
        }
    }
    let mut others = saves.len() - unmatched.len() - chosen.len();
    let mut threads: Vec<Thread> =
        chosen.into_iter().map(|(rom_id, save)| retroarch_thread(rom_id, matcher.stamp(rom_id), save)).collect();

    // The downloaded games, each as its launch runs it: the core's own save over whichever copy the
    // folders turned up, and a standalone emulator's sets.
    let mut notes = Vec::new();
    let mut sets = HashMap::new();
    // Each held to what the library has under its id: a record whose id a rebuilt RomM gave to
    // another game is reconciled when the window loads the library, and until then its saves wait
    // rather than going up as that game's.
    let games: Vec<&crate::launch::Plan> = games.iter().filter(|g| matcher.holds(g)).collect();
    for game in &games {
        let stamp = matcher.stamp(game.rom_id).or_else(|| game.stamp.clone());
        // Through sigil, whatever runs it — see [`units`]. The engine before is what a game sigil
        // cannot take keeps.
        let found = runner(game).map(|runner| units::thread(units::Game::of(game, stamp.as_ref()), runner, Ok(install)));
        if let Some(found) = found {
            match found {
                Ok(thread) => {
                    // A copy the folders turned up for this game that is not this unit's — another
                    // core's folder — is set down again, and counted with the rest left as they are.
                    others += threads.iter().filter(|t| t.rom_id == game.rom_id && !thread.covers(&t.paths())).count();
                    threads.retain(|t| t.rom_id != game.rom_id);
                    threads.push(thread);
                    continue;
                }
                Err(why) => notes.push(format!("{}: synced as before sigil, since {why}", game.title)),
            }
        }
        match game.emulator.syncs() {
            Some(core) => {
                let saves = install.for_game(&game.rom, core).game_saves(&game.rom, core);
                if let Some(save) = launched(&saves) {
                    // A copy the folders turned up for this game that the launch would not read —
                    // another core's folder — is set down again, and counted with the rest that are
                    // left as they are rather than disappearing from the run without a word.
                    let elsewhere = |t: &Thread| !matches!(&t.place, Place::File(path) if *path == save.path);
                    others += threads.iter().filter(|t| t.rom_id == game.rom_id).filter(|t| elsewhere(t)).count();
                    threads.retain(|t| t.rom_id != game.rom_id);
                    threads.push(retroarch_thread(game.rom_id, stamp, save));
                }
            }
            None => {
                let crate::launch::Emulator::Other(name) = &game.emulator else { continue };
                let Some(emulator) = standalone::Emulator::for_launch(&game.system, name) else { continue };
                match standalone::threads(emulator, &game.rom, &game.title, game.rom_id, stamp.as_ref()) {
                    Ok(found) => threads.extend(found),
                    Err(why) => notes.push(format!("{}: {why}", game.title)),
                }
                sets.insert(game.rom_id, (emulator, *game, stamp));
            }
        }
    }
    let mut scope: Vec<i64> = games.iter().map(|g| g.rom_id).collect();
    // Only what changed: those games' saves, and RomM asked about those games alone. A watcher's
    // sync after one save was written once read, zipped and hashed every save on the device.
    //
    // Save states go with it, for the games whose own states are among what changed — see
    // [`changed_states`]. A state written while the window is open would otherwise wait for the next
    // launch of its game, and the watcher looks over RetroArch's state folder too.
    let mut states = Report::default();
    if let Some(changed) = changed {
        let touched: HashSet<i64> = threads.iter().filter(|t| t.covers(changed)).map(|t| t.rom_id).collect();
        threads.retain(|t| touched.contains(&t.rom_id));
        scope.retain(|id| touched.contains(id));
        unmatched.clear();
        states = changed_states(client, connection, install, store, &games, builds, changed).await;
        if threads.is_empty() && scope.is_empty() {
            let mut report = Report { folder: install.saves.clone(), ..Report::default() };
            report.absorb(states);
            return Ok(report);
        }
    }
    let others = if changed.is_some() { 0 } else { others };
    let late = |op: &crate::romm::models::SyncOperation| {
        let (emulator, game, stamp) = sets.get(&op.rom_id)?;
        standalone::late(*emulator, &game.rom, &game.title, game.rom_id, stamp.as_ref(), op)
    };

    match session::sync(client, connection, store, threads, &scope, Vec::new(), &late).await? {
        Outcome::Done(report) => {
            let mut report = *report;
            report.folder = install.saves.clone();
            report.unmatched = unmatched;
            report.notes.extend(notes);
            if !report.homeless.is_empty() {
                let more = report.homeless.len().saturating_sub(3);
                let named = report.homeless.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
                report.notes.push(format!("RomM has saves this device has nowhere to put yet: {named}{tail}"));
            }
            if others > 0 {
                report.notes.push(format!("{others} older copies kept under another core's folder were left as they are"));
            }
            report.absorb(states);
            Ok(report)
        }
        Outcome::Unsupported(why) => {
            let mut report = sweep(client, connection, install, builds, store, changed).await?;
            report.notes.push(format!("synced without a RomM sync session: {why}"));
            report.absorb(states);
            Ok(report)
        }
    }
}

/// Sync the save states of the games among `games` that one of `changed` is a state of — what a
/// [`run`] narrowed to what changed adds to the saves it moves.
///
/// Only a narrowed run does this. RomM has no negotiation for states ([`states`]), so each game
/// costs a request of its own, and asking after every game on the device whenever *Sync now* is
/// pressed would be hundreds of them for states nothing touched. A watcher's sync knows which game
/// wrote, so it is one request — and it is the one path that catches a state saved in a game started
/// outside Deckhand, which no launch of ours brackets.
async fn changed_states(
    client: &RommClient,
    connection: &Connection,
    install: &Install,
    store: &Path,
    games: &[&crate::launch::Plan],
    builds: &[Build],
    changed: &[PathBuf],
) -> Report {
    let mut report = Report::default();
    // Nothing among them is a state, which is most changes: no state folder is worked out and no
    // override config read, for any of the games on the device.
    let a_state = |path: &PathBuf| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase().contains(".state"));
    if !changed.iter().any(a_state) {
        return report;
    }
    for game in games {
        let Some(core) = game.emulator.syncs() else { continue };
        let install = install.for_game(&game.rom, core);
        let (Ok(dir), Some(stem)) = (install.game_states_dir(&game.rom, core), game.rom.file_stem().and_then(|s| s.to_str())) else {
            continue;
        };
        let theirs = |path: &PathBuf| {
            path.parent() == Some(dir.as_path())
                && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| states::is_slot_of(name, stem))
        };
        if !changed.iter().any(theirs) {
            continue;
        }
        let stamp = stamp_of(builds, game.rom_id).or_else(|| game.stamp.clone());
        let mut run = Run::with_index(client, connection, store, dir.clone(), "state-index.json").stamped(stamp);
        run.states(&dir, stem, game.rom_id, &states::Core::of(core, &install)).await;
        report.absorb(run.report);
    }
    report
}

/// What runs `game`, as [`units`] takes it — None for an emulator whose saves are not synced.
fn runner(game: &crate::launch::Plan) -> Option<units::Runner<'_>> {
    match &game.emulator {
        crate::launch::Emulator::Other(name) => standalone::Emulator::for_launch(&game.system, name).map(units::Runner::Standalone),
        emulator => emulator.syncs().map(units::Runner::RetroArch),
    }
}

/// The game a local save belongs to: the one it was matched to before, else by its name.
fn rom_of(matcher: &Matcher, index: &Index, save: &LocalSave) -> Match {
    // A save matched once stays with that game: a rename in the library must not move it. Only while
    // the id is still that game, though — a rebuilt RomM gives its ids to whatever it scans first.
    // An entry from before stamps were kept vouches for its id by the save's name, which RetroArch
    // takes from the rom's file.
    let remembered = index.get(&save.path).filter(|e| match (&e.stamp, matcher.build(e.rom_id)) {
        (Some(stamp), Some(build)) => stamp.same_rom(&build.stamp()),
        (None, Some(build)) => keys(&save.stem).iter().zip(keys(&build.stem)).any(|(a, b)| *a == b),
        (_, None) => false,
    });
    remembered.map(|e| Match::One(e.rom_id)).unwrap_or_else(|| matcher.find(&save.stem, save.content_folder.as_deref()))
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The saves a downloaded game has on this device, as a launch of it reads them: its core's battery
/// save, or its standalone emulator's sets.
///
/// `install` is RetroArch's, which only a game RetroArch runs needs — a standalone emulator's game
/// goes on without one.
pub fn threads_of(install: Result<&Install, &str>, game: &crate::launch::Plan) -> Result<Vec<Thread>, String> {
    if let Some(runner) = runner(game)
        && let Ok(thread) = units::thread(units::Game::of(game, game.stamp.as_ref()), runner, install)
    {
        return Ok(vec![thread]);
    }
    match game.emulator.syncs() {
        Some(core) => {
            let install = install.map_err(str::to_string)?;
            let saves = install.for_game(&game.rom, core).game_saves(&game.rom, core);
            Ok(launched(&saves).map(|save| retroarch_thread(game.rom_id, game.stamp.clone(), save)).into_iter().collect())
        }
        None => {
            let crate::launch::Emulator::Other(name) = &game.emulator else { return Ok(Vec::new()) };
            let emulator = standalone::Emulator::for_launch(&game.system, name)
                .ok_or_else(|| format!("{name}'s saves aren't synced by this build"))?;
            standalone::threads(emulator, &game.rom, &game.title, game.rom_id, game.stamp.as_ref())
        }
    }
}

/// Put RomM's `version` of one of `game`'s saves back as the current one — see [`session::restore`].
pub async fn restore(
    client: &RommClient,
    connection: &Connection,
    install: Result<&Install, &str>,
    store: &Path,
    game: &crate::launch::Plan,
    version: &SaveAsset,
) -> Result<Report, String> {
    let threads = threads_of(install, game)?;
    // A version from before the game moved onto sigil is in the shape before, which the engine
    // before puts back; the next sync sends the game's unit up from it.
    let mut places = threads.clone();
    for thread in &threads {
        if let Place::Sigil(unit) = &thread.place {
            places.extend(unit.legacy.iter().cloned());
            if let Some((emulator, rom, title)) = &unit.legacy_source {
                let name = names::canonical(&version.file_name);
                places.extend(standalone::legacy_threads(*emulator, rom, title, thread.rom_id, thread.stamp.as_ref(), &[name]));
            }
        }
    }
    let thread = places
        .iter()
        .find(|t| session::holds(t, version))
        .ok_or_else(|| format!("there is no place on this device that {} goes", version.file_name))?;
    session::restore(client, connection, store, thread, version).await
}

/// A RetroArch battery save as a thread of its game's `autosave` slot — the engine before sigil.
fn retroarch_thread(rom_id: i64, stamp: Option<Stamp>, save: &LocalSave) -> Thread {
    Thread {
        rom_id,
        stamp,
        slot: session::AUTOSAVE.into(),
        name: file_name(&save.path),
        emulator: save.emulator.clone(),
        place: Place::File(save.path.clone()),
        shared: false,
    }
}

/// [`run`] for a RomM without sync sessions: every save against the newest of its name on RomM.
async fn sweep(
    client: &RommClient,
    connection: &Connection,
    install: &Install,
    builds: &[Build],
    store: &Path,
    changed: Option<&[PathBuf]>,
) -> Result<Report, String> {
    let saves: Vec<LocalSave> = install.saves().into_iter().filter(|s| changed.is_none_or(|c| c.contains(&s.path))).collect();
    let mut run = Run::new(client, connection, store, install.saves.clone());
    if saves.is_empty() {
        return Ok(run.finish().await);
    }

    let mut server: HashMap<i64, Vec<SaveAsset>> = HashMap::new();
    for save in client
        .fetch_saves(connection, None)
        .await
        .map_err(|e| format!("Could not read the saves on RomM: {}", describe(&e)))?
    {
        if let Some(rom_id) = save.rom_id {
            server.entry(rom_id).or_default().push(save);
        }
    }

    let matcher = Matcher::new(builds);
    for save in &saves {
        let Match::One(rom_id) = rom_of(&matcher, &run.index, save) else {
            run.report.unmatched.push(file_name(&save.path));
            continue;
        };
        run.stamp = matcher.stamp(rom_id);
        run.save(save, rom_id, server.get(&rom_id).map(Vec::as_slice).unwrap_or_default()).await;
    }
    Ok(run.finish().await)
}

/// Sync the save of one game — the one whose content file is `rom`, run by `core` when the caller
/// knows which. What a launcher calls before the game starts and after it exits.
///
/// Unlike [`run`], this may create a save: being handed the content file is proof the game is on
/// this device, so a save RomM holds for it comes down even when there is none here yet — the first
/// launch on a new device. Where it goes needs the core whenever RetroArch sorts saves by core; see
/// [`Install::game_saves`] for why that is never guessed.
#[allow(clippy::too_many_arguments)]
pub async fn sync_game(
    client: &RommClient,
    connection: &Connection,
    install: &Install,
    builds: &[Build],
    store: &Path,
    rom: &Path,
    core: Option<&str>,
    plays: Vec<PlaySession>,
) -> Result<Report, String> {
    let rom_id = find_rom(builds, rom)?;
    let system = builds.iter().find(|b| b.rom_id == rom_id).map(|b| b.system_id.clone()).unwrap_or_default();
    sync_rom(client, connection, install, store, rom, &system, core, rom_id, stamp_of(builds, rom_id).as_ref(), plays).await
}

/// Whether RomM still holds, under `rom_id`, the game `is` recognises — asked of RomM itself before a
/// launch, or a launcher's hook, tells RomM anything under an id kept on this device.
///
/// What a launch goes by may be the library the window last read, days ago, and a RomM rebuilt since
/// gives the id to whatever it scanned into that place. Only another game there says no. A RomM that
/// cannot be asked — this device offline, say — or that has nothing under the id goes on as it would
/// have: there is no other game's slot to put the saves in, and the sync fails on its own if it
/// cannot reach RomM either.
pub async fn still_holds(client: &RommClient, conn: &Connection, rom_id: i64, is: &(dyn Fn(&Stamp) -> bool + Sync)) -> Result<(), String> {
    match client.fetch_rom_stamp(conn, rom_id).await {
        Ok(Some(now)) if !is(&now) => Err(format!(
            "RomM gives this game's id to \"{}\" now, so its saves and playtime stay off RomM — open Deckhand to find the game there again",
            now.stem
        )),
        _ => Ok(()),
    }
}

/// Which rom in the library `rom` is, by its file name and folder — for a caller handed only a file,
/// like a launcher's hook. The ROM's folder is the system's for any game not kept in a folder of its
/// own, which is what tells two games of one name on different systems apart.
pub fn find_rom(builds: &[Build], rom: &Path) -> Result<i64, String> {
    let name = file_name(rom);
    let stem = rom.file_stem().and_then(|s| s.to_str()).ok_or_else(|| format!("{name} is not a game file"))?;
    let folder = rom.parent().and_then(Path::file_name).and_then(|f| f.to_str());
    match Matcher::new(builds).find(stem, folder) {
        Match::One(id) => Ok(id),
        Match::None => Err(format!("{name} is not in the library Deckhand last loaded")),
        Match::Many => Err(format!("{name} matches more than one game in the library")),
    }
}

/// [`sync_game`] for a caller that already knows which rom `rom` is — a launch or a download from
/// the app, which have the rom id in hand. Skips the name match, which is what would otherwise turn
/// a game kept in a folder of its own, or one whose title two systems share, into "not in the
/// library".
///
/// The save goes through a sync session where RomM holds them ([`session`]), closing it with `plays`
/// — the play session a launch just ended — and the way before that where RomM does not. `system`
/// is the game's, ES-DE's name or RomM's: what tells sigil which console's saves these are.
#[allow(clippy::too_many_arguments)]
pub async fn sync_rom(
    client: &RommClient,
    connection: &Connection,
    install: &Install,
    store: &Path,
    rom: &Path,
    system: &str,
    core: Option<&str>,
    rom_id: i64,
    stamp: Option<&Stamp>,
    plays: Vec<PlaySession>,
) -> Result<Report, String> {
    let _one_at_a_time = session::exclusive(store).await?;
    // The game's overrides, if its core has any, decide where its saves and states are.
    let install = &install.for_game(rom, core);
    let name = file_name(rom);
    let saves = install.game_saves(rom, core);
    let folder = saves.target.as_ref().or(saves.existing.first()).and_then(|s| s.path.parent()).unwrap_or(&install.saves);

    let title = rom.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let game = units::Game { rom_id, rom, system, title: &title, stamp };
    let mut notes = Vec::new();
    let (threads, folder): (Vec<Thread>, PathBuf) = match units::thread(game, units::Runner::RetroArch(core), Ok(install)) {
        Ok(thread) => {
            let root = match &thread.place {
                Place::Sigil(unit) => unit.request.root.clone(),
                _ => folder.to_path_buf(),
            };
            (vec![thread], root)
        }
        Err(why) => {
            notes.push(format!("{name}: synced as before sigil, since {why}"));
            (launched(&saves).map(|save| retroarch_thread(rom_id, stamp.cloned(), save)).into_iter().collect(), folder.to_path_buf())
        }
    };
    let mut report = match session::sync(client, connection, store, threads, &[rom_id], plays, &|_| None).await? {
        Outcome::Done(report) => Report { folder: folder.clone(), ..*report },
        Outcome::Unsupported(why) => {
            let mut report = legacy_rom(client, connection, store, &folder, &saves, rom_id, stamp).await?;
            report.notes.push(format!("synced without a RomM sync session: {why}"));
            report
        }
    };
    report.notes.extend(notes);
    if saves.existing.is_empty() && saves.target.is_none() && !std::mem::take(&mut report.homeless).is_empty() {
        report.notes.push(format!(
            "{name} has no save here yet, and RetroArch files saves by core — without knowing which core \
             runs it there is nowhere to put the one RomM has"
        ));
    }

    // Its save states, beside — see [`states`]. Tagged with the core, which is what decides whether
    // a state loads anywhere else.
    match (install.game_states_dir(rom, core), rom.file_stem().and_then(|s| s.to_str())) {
        (Ok(dir), Some(stem)) => {
            let mut states = Run::with_index(client, connection, store, dir.clone(), "state-index.json").stamped(stamp.cloned());
            states.states(&dir, stem, rom_id, &states::Core::of(core, install)).await;
            report.absorb(states.report);
        }
        (Err(why), _) => report.notes.push(format!("{name}'s save states are not synced: {why}")),
        (_, None) => {}
    }
    Ok(report)
}

/// The save a launch syncs: the file the core about to run reads, when it is on disk; else the one
/// played last; else where the core will look, for RomM's to go.
fn launched(saves: &GameSaves) -> Option<&LocalSave> {
    let target = saves.target.as_ref();
    target
        .filter(|t| t.path.is_file())
        .or_else(|| saves.existing.iter().max_by_key(|s| modified(&s.path)))
        .or(target)
}

/// [`sync_rom`]'s save for a RomM without sync sessions: each copy against the newest of its name.
async fn legacy_rom(
    client: &RommClient,
    connection: &Connection,
    store: &Path,
    folder: &Path,
    saves: &GameSaves,
    rom_id: i64,
    stamp: Option<&Stamp>,
) -> Result<Report, String> {
    let mut run = Run::new(client, connection, store, folder.to_path_buf()).stamped(stamp.cloned());
    let server: Vec<SaveAsset> = client
        .fetch_saves(connection, Some(rom_id))
        .await
        .map_err(|e| format!("Could not read the saves on RomM: {}", describe(&e)))?
        .into_iter()
        .filter(|s| s.rom_id.is_none_or(|id| id == rom_id))
        .collect();
    for save in &saves.existing {
        run.save(save, rom_id, &server).await;
    }
    if saves.existing.is_empty()
        && let Some(target) = &saves.target
    {
        run.fetch_new(target, rom_id, &server).await;
    }
    if saves.existing.is_empty() && saves.target.is_none() && !server.is_empty() {
        run.report.homeless.push(String::new());
    }
    Ok(run.finish().await)
}

/// What one save needs, from what each side has done since they last agreed.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    InSync,
    Push,
    Pull,
    Conflict,
}

/// `hash` is the local content's; `last` what both sides last held; `server` the id of the newest
/// save RomM has under this save's name.
///
/// A save never seen before has "changed" locally, so it is pushed — or, when RomM has one too, it
/// is a conflict, which [`Run::settle`] resolves without a transfer when the two are the same.
fn plan(hash: &str, last: Option<&Entry>, server: Option<i64>) -> Plan {
    let local_changed = last.is_none_or(|e| e.hash != hash);
    let server_changed = server.is_some_and(|id| last.is_none_or(|e| e.save_id != id));
    match (local_changed, server_changed) {
        (false, false) => Plan::InSync,
        (true, false) => Plan::Push,
        (false, true) => Plan::Pull,
        (true, true) => Plan::Conflict,
    }
}

/// One run's state: the connection, the index it updates, and the report it fills.
struct Run<'a> {
    client: &'a RommClient,
    connection: &'a Connection,
    index: Index,
    backups: PathBuf,
    report: Report,
    /// RomM saves this run took or agreed on, to acknowledge for this device at the end — see
    /// [`device`].
    acks: Vec<i64>,
    /// Which rom the game whose saves these are is — see [`Entry::follows`]. None in a run over
    /// several games' that has not been told.
    stamp: Option<Stamp>,
}

impl<'a> Run<'a> {
    fn new(client: &'a RommClient, connection: &'a Connection, store: &Path, folder: PathBuf) -> Self {
        Self::with_index(client, connection, store, folder, "save-index.json")
    }

    /// A run over its own index — states keep theirs apart from saves', since a slot and a save
    /// are different files with different histories on RomM.
    fn with_index(client: &'a RommClient, connection: &'a Connection, store: &Path, folder: PathBuf, index: &str) -> Self {
        Self {
            client,
            connection,
            index: Index::load(store.join(index)),
            backups: store.join("save-backups"),
            report: Report { folder, ..Default::default() },
            acks: Vec::new(),
            stamp: None,
        }
    }

    /// The run, for the one game the rom stamped `stamp` is.
    fn stamped(mut self, stamp: Option<Stamp>) -> Self {
        self.stamp = stamp;
        self
    }

    /// Bring down a save this device has none of, if RomM has one under the name RetroArch will
    /// look for. Nothing on either side is not news, and is left unsaid.
    async fn fetch_new(&mut self, target: &LocalSave, rom_id: i64, server: &[SaveAsset]) {
        let name = file_name(&target.path);
        let Some(remote) = newest_on_server(server, &target.stem, &name) else { return };
        if let Some(bytes) = self.download(remote, &name).await
            && self.write(target, rom_id, remote, &bytes, &name)
        {
            self.report.pulled.push(name);
        }
    }

    async fn save(&mut self, save: &LocalSave, rom_id: i64, server: &[SaveAsset]) {
        let name = file_name(&save.path);
        let local = match fs::read(&save.path) {
            Ok(bytes) => bytes,
            Err(e) => return self.report.failed.push(format!("{name}: {e}")),
        };
        let hash = md5_hex(&local);
        let newest = newest_on_server(server, &save.stem, &name);

        match (plan(&hash, self.index.agreed(&save.path, rom_id, self.stamp.as_ref()), newest.map(|s| s.id)), newest) {
            (Plan::InSync, _) => self.report.in_sync += 1,
            (Plan::Push, _) => {
                if self.push(save, rom_id, &name, local, hash).await {
                    self.report.pushed.push(name);
                }
            }
            (Plan::Pull, Some(remote)) => {
                if let Some(bytes) = self.download(remote, &name).await
                    && self.write(save, rom_id, remote, &bytes, &name)
                {
                    self.report.pulled.push(name);
                }
            }
            (Plan::Conflict, Some(remote)) => self.settle(save, rom_id, remote, local, hash, name).await,
            // `plan` only answers Pull or Conflict when there is a server save.
            (Plan::Pull | Plan::Conflict, None) => unreachable!("a server change with no server save"),
        }
    }

    /// Both sides changed since they last agreed.
    async fn settle(
        &mut self,
        save: &LocalSave,
        rom_id: i64,
        remote: &SaveAsset,
        local: Vec<u8>,
        hash: String,
        name: String,
    ) {
        let Some(bytes) = self.download(remote, &name).await else { return };
        // The same progress on both sides — a save first seen on this device that another one had
        // already pushed, say. Agreeing on it costs nothing and moves no data.
        if md5_hex(&bytes) == hash {
            if self.remember(save, rom_id, hash, remote.id, &name) {
                self.report.in_sync += 1;
                self.acks.push(remote.id);
            }
            return;
        }

        let local_time = fs::metadata(&save.path).and_then(|m| m.modified()).ok();
        let remote_time = remote.updated_at.as_deref().and_then(names::parse_timestamp);
        if choose(&name, local_time, remote_time, local.len() as u64).await == Choice::Remote {
            // The local copy holds progress RomM has never seen, so it is set aside before it goes.
            let Some(kept) = self.back_up(&save.path, &local, &name) else { return };
            if self.write(save, rom_id, remote, &bytes, &name) {
                self.report.conflicts.push(format!("{name}: kept RomM's copy; this device's is at {}", kept.display()));
            }
        } else if self.push(save, rom_id, &name, local, hash).await {
            self.report.conflicts.push(format!("{name}: kept this device's copy; RomM keeps the other"));
        }
    }

    async fn push(&mut self, save: &LocalSave, rom_id: i64, name: &str, bytes: Vec<u8>, hash: String) -> bool {
        match self.client.upload_save(self.connection, rom_id, &save.emulator, name, bytes).await {
            Ok(Some(created)) => self.remember(save, rom_id, hash, created.id, name),
            Ok(None) => {
                self.report.deferred.push(name.to_string());
                false
            }
            Err(e) => {
                self.report.failed.push(format!("{name}: {}", describe(&e)));
                false
            }
        }
    }

    async fn download(&mut self, remote: &SaveAsset, name: &str) -> Option<Vec<u8>> {
        match self.client.download_save(self.connection, remote).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                self.report.failed.push(format!("{name}: {}", describe(&e)));
                None
            }
        }
    }

    /// Put `bytes` — `remote`'s content — in place of the local save.
    ///
    /// Written beside it and renamed over it, so a run cut short leaves the old save whole rather
    /// than half of the new one. The file then takes RomM's time rather than now, so a later
    /// conflict weighs when the progress was made, not when it was copied here.
    fn write(&mut self, save: &LocalSave, rom_id: i64, remote: &SaveAsset, bytes: &[u8], name: &str) -> bool {
        if let Err(e) = write_atomic(&save.path, bytes) {
            self.report.failed.push(format!("{name}: {e}"));
            return false;
        }
        if let Some(time) = remote.updated_at.as_deref().and_then(names::parse_timestamp)
            && let Ok(file) = fs::File::options().write(true).open(&save.path)
        {
            let _ = file.set_modified(time);
        }
        let ok = self.remember(save, rom_id, md5_hex(bytes), remote.id, name);
        if ok {
            self.acks.push(remote.id);
            self.report.written.push(save.path.clone());
        }
        ok
    }

    /// Tell RomM this device holds the saves this run took — what lets its next push through. A
    /// failed acknowledgement costs at most one deferred push, and is tried again next time.
    async fn finish(mut self) -> Report {
        if let Some(device) = self.connection.device_id.clone() {
            self.acks.sort_unstable();
            self.acks.dedup();
            for id in std::mem::take(&mut self.acks) {
                let _ = self.client.confirm_downloaded(self.connection, id, &device, None).await;
            }
        }
        session::prune_backups(&self.backups);
        self.report
    }

    fn remember(&mut self, save: &LocalSave, rom_id: i64, hash: String, save_id: i64, name: &str) -> bool {
        match self.index.set(&save.path, Entry { rom_id, hash, save_id, stamp: self.stamp.clone(), ..Default::default() }) {
            Ok(()) => true,
            Err(e) => {
                self.report.failed.push(format!("{name}: synced, but could not record it ({e})"));
                false
            }
        }
    }

    /// Copy a save that is about to be overwritten into the backups folder — see
    /// [`session::back_up`], which names the copies so they sort by when they were made and never
    /// writes over one already there.
    fn back_up(&mut self, path: &Path, bytes: &[u8], name: &str) -> Option<PathBuf> {
        match session::back_up(&self.backups, name, bytes) {
            Ok(kept) => Some(kept),
            Err(e) => {
                // Without the copy the overwrite would lose the only one, so it does not happen.
                self.report.failed.push(format!("{name}: could not keep a copy of {} ({e})", path.display()));
                None
            }
        }
    }
}

/// The newest save RomM holds under the name of the local file `name` (stem `stem`).
fn newest_on_server<'a>(server: &'a [SaveAsset], stem: &str, name: &str) -> Option<&'a SaveAsset> {
    server
        .iter()
        .filter(|s| names::local_name(&names::canonical(&s.file_name), stem).eq_ignore_ascii_case(name))
        .max_by_key(|s| (s.updated_at.as_deref().and_then(names::parse_timestamp), s.id))
}

/// Which game a save belongs to, by its name against every build's file name.
struct Matcher<'a> {
    builds: &'a [Build],
    /// Each build's place in `builds`, by rom id.
    roms: HashMap<i64, usize>,
    /// The same builds keyed three ways, tried in order, each looser than the last.
    forms: [HashMap<String, Vec<usize>>; 3],
}

enum Match {
    One(i64),
    None,
    Many,
}

impl<'a> Matcher<'a> {
    fn new(builds: &'a [Build]) -> Self {
        let mut forms: [HashMap<String, Vec<usize>>; 3] = Default::default();
        for (i, build) in builds.iter().enumerate() {
            for (form, key) in forms.iter_mut().zip(keys(&build.stem)) {
                form.entry(key).or_default().push(i);
            }
        }
        Self { builds, roms: builds.iter().enumerate().map(|(i, b)| (b.rom_id, i)).collect(), forms }
    }

    fn build(&self, rom_id: i64) -> Option<&Build> {
        self.roms.get(&rom_id).map(|&i| &self.builds[i])
    }

    fn stamp(&self, rom_id: i64) -> Option<Stamp> {
        self.build(rom_id).map(Build::stamp)
    }

    /// Whether `game`'s id is not another game's in the library — see
    /// [`crate::downloads::Installed::is`]. An id the library does not have at all is no sign of one:
    /// a system that failed to load says nothing about the games on it.
    fn holds(&self, game: &crate::launch::Plan) -> bool {
        self.build(game.rom_id).is_none_or(|b| game.is(&b.stamp()))
    }

    /// The exact name first, then without a disc tag, then without punctuation. A looser form is
    /// only tried because the stricter one named no game at all, so an exact match always wins.
    ///
    /// The same name on two systems is settled by `folder` — the content folder a save was sorted
    /// under, or a launched ROM's own folder, either of which is the system's for most games — and
    /// otherwise left unmatched rather than guessed.
    fn find(&self, stem: &str, folder: Option<&str>) -> Match {
        for (form, key) in self.forms.iter().zip(keys(stem)) {
            let Some(hits) = form.get(&key) else { continue };
            let roms = |filter: &dyn Fn(&Build) -> bool| {
                let mut ids: Vec<i64> =
                    hits.iter().map(|&i| &self.builds[i]).filter(|b| filter(b)).map(|b| b.rom_id).collect();
                ids.sort_unstable();
                ids.dedup();
                ids
            };
            let all = roms(&|_| true);
            if let [id] = all[..] {
                return Match::One(id);
            }
            if let Some(folder) = folder
                && let [id] = roms(&|b| b.system_id.eq_ignore_ascii_case(folder))[..]
            {
                return Match::One(id);
            }
            return Match::Many;
        }
        Match::None
    }
}

/// A name in the three forms [`Matcher`] compares.
fn keys(stem: &str) -> [String; 3] {
    [stem.to_lowercase(), names::without_disc(stem).to_lowercase(), names::punctuation_free(stem)]
}

/// A request failure, as one save's line in the report.
///
/// The client's messages are written for pairing, where a 404 means a stale code; here it means
/// RomM refused this save, and saying anything about pairing would send the user the wrong way.
fn describe(error: &ApiError) -> String {
    match error.status {
        400 | 404 | 422 => format!("RomM refused it (HTTP {})", error.status),
        _ => error.message.clone(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn md5_hex(bytes: &[u8]) -> String {
    Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `bytes` to `path` through a temporary file beside it, so a crash mid-write never leaves a
/// half-written save — or index — where the whole one was.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".deckhand-part");
    let temp = PathBuf::from(temp);
    // On the disk before it replaces the old copy: a rename that lands before the data would leave
    // an empty save after a power cut — a Deck's battery running out mid-sync, say.
    let mut file = fs::File::create(&temp)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })?;
    // The rename on the disk too, not only the bytes it points at. A save whose rename is still in
    // the air when a Deck's battery goes flat comes back as the old file — while the index entry
    // written just after it, in another folder and so another batch of the journal, can be there
    // saying the new one is. The next sync would read the old save as progress made here and push it
    // over the newer one on RomM. A filesystem that will not open a directory for this (Windows)
    // is left as it was.
    if let Some(dir) = path.parent() {
        let _ = fs::File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub_server;

    fn entry(hash: &str, save_id: i64) -> Entry {
        Entry { rom_id: 1, hash: hash.into(), save_id, ..Default::default() }
    }

    #[test]
    fn each_side_changing_alone_moves_the_save_toward_the_other() {
        let last = entry("aaa", 10);
        assert_eq!(plan("aaa", Some(&last), Some(10)), Plan::InSync);
        assert_eq!(plan("bbb", Some(&last), Some(10)), Plan::Push);
        assert_eq!(plan("aaa", Some(&last), Some(11)), Plan::Pull);
        assert_eq!(plan("bbb", Some(&last), Some(11)), Plan::Conflict);
    }

    #[test]
    fn a_save_seen_for_the_first_time_is_pushed_unless_romm_has_one_too() {
        assert_eq!(plan("aaa", None, None), Plan::Push);
        assert_eq!(plan("aaa", None, Some(10)), Plan::Conflict);
    }

    #[test]
    fn a_save_deleted_from_romm_is_not_pushed_back_unless_it_changes() {
        // Deleting a save on the server is a choice; re-uploading it unasked would undo it.
        assert_eq!(plan("aaa", Some(&entry("aaa", 10)), None), Plan::InSync);
        assert_eq!(plan("bbb", Some(&entry("aaa", 10)), None), Plan::Push);
    }

    fn build(rom_id: i64, stem: &str, system: &str) -> Build {
        Build { rom_id, stem: stem.into(), system_id: system.into(), born: None }
    }

    fn found(matcher: &Matcher, stem: &str, folder: Option<&str>) -> Option<i64> {
        match matcher.find(stem, folder) {
            Match::One(id) => Some(id),
            Match::None | Match::Many => None,
        }
    }

    #[test]
    fn a_save_finds_its_build_by_name_including_a_sibling_build() {
        let builds = [build(1, "Chrono Trigger (USA)", "snes"), build(2, "Chrono Trigger (Japan)", "snes")];
        let matcher = Matcher::new(&builds);
        assert_eq!(found(&matcher, "Chrono Trigger (USA)", None), Some(1));
        assert_eq!(found(&matcher, "chrono trigger (japan)", None), Some(2));
        assert_eq!(found(&matcher, "Secret of Mana (USA)", None), None);
    }

    const REBUILT: &str = "2026-10-12T19:00:00+00:00";
    const BEFORE: &str = "2026-08-01T22:26:48+00:00";

    fn born(rom_id: i64, stem: &str, at: &str) -> Build {
        Build { born: Some(at.into()), ..build(rom_id, stem, "n64") }
    }

    #[test]
    fn a_save_remembered_under_an_id_a_rebuilt_romm_gave_to_another_game_goes_by_its_name_again() {
        let builds = [born(1, "Zipang (Japan)", REBUILT), born(12, "Banjo-Kazooie (USA)", REBUILT), born(3, "New Name", BEFORE)];
        let matcher = Matcher::new(&builds);
        let dir = std::env::temp_dir().join(format!("deckhand-rom-of-{}", std::process::id()));
        let mut index = Index::load(dir.join("save-index.json"));
        let save = |stem: &str| LocalSave {
            path: dir.join(format!("{stem}.srm")),
            stem: stem.into(),
            content_folder: None,
            emulator: "mupen64plus_next".into(),
        };
        let remembered = |rom_id: i64, stamp: Option<Stamp>| Entry { rom_id, hash: "h".into(), save_id: 5, stamp, ..Default::default() };
        let of = |index: &Index, stem: &str| match rom_of(&matcher, index, &save(stem)) {
            Match::One(id) => Some(id),
            Match::None | Match::Many => None,
        };

        let banjo = Stamp { stem: "Banjo-Kazooie (USA)".into(), born: Some(BEFORE.into()) };
        index.set(&save("Banjo-Kazooie (USA)").path, remembered(1, Some(banjo))).unwrap();
        assert_eq!(of(&index, "Banjo-Kazooie (USA)"), Some(12), "not Zipang, which has its number now");
        index.set(&save("Banjo-Kazooie (USA)").path, remembered(1, None)).unwrap();
        assert_eq!(of(&index, "Banjo-Kazooie (USA)"), Some(12), "an entry from before stamps goes by the save's name");
        index.set(&save("Banjo-Kazooie (USA)").path, remembered(12, None)).unwrap();
        assert_eq!(of(&index, "Banjo-Kazooie (USA)"), Some(12));

        // Renamed in RomM since it was matched: the same row, so the save stays with it.
        let renamed = Stamp { stem: "Old Name".into(), born: Some(BEFORE.into()) };
        index.set(&save("Old Name").path, remembered(3, Some(renamed))).unwrap();
        assert_eq!(of(&index, "Old Name"), Some(3));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_downloaded_game_is_synced_by_a_sweep_unless_its_id_holds_another_game() {
        let game = |stamp: Option<Stamp>| crate::launch::Plan {
            rom_id: 1,
            title: "Banjo-Kazooie".into(),
            rom: PathBuf::from("Banjo-Kazooie (USA).z64"),
            system: "n64".into(),
            platform_id: None,
            launcher: Err("not needed".into()),
            emulator: crate::launch::Emulator::RetroArch,
            stamp,
            names: vec!["Banjo-Kazooie (USA)".into()],
        };
        let banjo = Some(Stamp { stem: "Banjo-Kazooie (USA)".into(), born: Some(BEFORE.into()) });
        assert!(!Matcher::new(&[born(1, "Zipang (Japan)", REBUILT)]).holds(&game(banjo.clone())));
        assert!(!Matcher::new(&[born(1, "Zipang (Japan)", REBUILT)]).holds(&game(None)), "nor by its files' names");
        assert!(Matcher::new(&[born(1, "Banjo-Kazooie (USA)", REBUILT)]).holds(&game(banjo.clone())), "the same file, scanned again");
        assert!(Matcher::new(&[]).holds(&game(banjo)), "a library without the id says nothing against it");
    }

    #[tokio::test]
    async fn romm_is_asked_whether_an_id_is_still_the_game_and_only_another_game_there_says_no() {
        let zipang = serde_json::json!({ "id": 1, "fs_name_no_ext": "Zipang (Japan)" }).to_string();
        let old_server = serde_json::json!({ "id": 2, "fs_name": "Chrono Trigger (USA).sfc", "fs_extension": "sfc", "files": [] }).to_string();
        let host = stub_server::start(vec![
            ("GET /api/roms/1/simple", 200, zipang),
            ("GET /api/roms/2/simple", 404, "{}".into()),
            ("GET /api/roms/2", 200, old_server),
        ])
        .await;
        let (client, conn) = (RommClient::new().unwrap(), connection(host));
        let is = |stem: &'static str| move |now: &Stamp| now.stem == stem;

        let refused = still_holds(&client, &conn, 1, &is("Banjo-Kazooie (USA)")).await.unwrap_err();
        assert!(refused.contains("\"Zipang (Japan)\""), "{refused}");
        assert!(still_holds(&client, &conn, 1, &is("Zipang (Japan)")).await.is_ok());
        assert!(still_holds(&client, &conn, 2, &is("Chrono Trigger (USA)")).await.is_ok(), "a RomM before `/simple` answers with the whole rom");
        assert!(still_holds(&client, &conn, 2, &is("Banjo-Kazooie (USA)")).await.is_err());
        assert!(still_holds(&client, &conn, 3, &is("Banjo-Kazooie (USA)")).await.is_ok(), "nothing under the id is no other game");
    }

    #[test]
    fn a_looser_form_is_tried_only_when_the_exact_name_names_nothing() {
        let builds = [
            build(1, "Final Fantasy VII (USA)", "psx"),
            build(2, "Pokemon Yellow (USA) (CGB+SGB Enhanced)", "gb"),
        ];
        let matcher = Matcher::new(&builds);
        // One disc of the set, against the set.
        assert_eq!(found(&matcher, "Final Fantasy VII (USA) (Disc 2)", None), Some(1));
        // A name an uploader stripped of punctuation.
        assert_eq!(found(&matcher, "Pokemon Yellow (USA) (CGBSGB Enhanced)", None), Some(2));
    }

    #[test]
    fn the_same_name_on_two_systems_is_settled_by_the_content_folder_or_not_at_all() {
        let builds = [
            build(1, "007 - The World Is Not Enough (USA)", "psx"),
            build(2, "007 - The World Is Not Enough (USA)", "n64"),
        ];
        let matcher = Matcher::new(&builds);
        assert_eq!(found(&matcher, "007 - The World Is Not Enough (USA)", Some("n64")), Some(2));
        assert_eq!(found(&matcher, "007 - The World Is Not Enough (USA)", None), None);
    }

    #[test]
    fn the_summary_names_only_what_happened() {
        let report = Report {
            in_sync: 3,
            pushed: vec!["a.srm".into()],
            unmatched: vec!["b.srm".into(), "c.srm".into()],
            ..Default::default()
        };
        assert_eq!(report.summary(), "1 pushed · 3 in sync · 2 not in the library");
        assert_eq!(Report::default().summary(), "no saves found");
    }

    // --- Against a server --------------------------------------------------------------------

    /// A flat RetroArch install with saves, and a store for the index, under the temp directory.
    struct Device {
        root: PathBuf,
    }

    impl Device {
        fn new(name: &str) -> Self {
            Self::with_config(name, "savefile_directory = \":/saves\"\n")
        }

        /// Stock RetroDECK's layout: saves sorted by content folder, then by core.
        fn sorted(name: &str) -> Self {
            let device = Self::with_config(
                name,
                "savefile_directory = \":/saves\"\n\
                 libretro_info_path = \":/info\"\n\
                 sort_savefiles_by_content_enable = \"true\"\n\
                 sort_savefiles_enable = \"true\"\n",
            );
            fs::create_dir_all(device.root.join("ra/info")).unwrap();
            fs::write(device.root.join("ra/info/snes9x_libretro.info"), "corename = \"Snes9x\"\n").unwrap();
            device
        }

        fn with_config(name: &str, config: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-sync-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("ra/saves")).unwrap();
            fs::write(root.join("ra/retroarch.cfg"), config).unwrap();
            Self { root }
        }

        /// A launch of `rom` (a path under `roms/`) through `core`, as `deckhand pre` runs one.
        async fn launch(&self, host: String, rom: &str, core: Option<&str>) -> Result<Report, String> {
            let install = retroarch::read(&self.root.join("ra/retroarch.cfg")).unwrap();
            let rom = self.root.join("roms").join(rom);
            sync_game(&RommClient::new().unwrap(), &connection(host), &install, &chrono(), &self.store(), &rom, core, Vec::new())
                .await
        }

        fn save(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.root.join("ra/saves").join(name);
            fs::write(&path, contents).unwrap();
            path
        }

        fn store(&self) -> PathBuf {
            self.root.join("store")
        }

        async fn sync(&self, host: String, builds: &[Build]) -> Report {
            self.sync_changed(host, builds, None).await
        }

        async fn sync_changed(&self, host: String, builds: &[Build], changed: Option<&[PathBuf]>) -> Report {
            let install = retroarch::read(&self.root.join("ra/retroarch.cfg")).unwrap();
            run(&RommClient::new().unwrap(), &connection(host), &install, builds, &self.store(), &[], changed).await.unwrap()
        }
    }

    fn connection(host: String) -> Connection {
        Connection { host, token: "rmm_test".into(), insecure: false, token_id: None, token_name: None, device_id: None }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const CHRONO: &str = "Chrono Trigger (USA)";

    fn chrono() -> Vec<Build> {
        vec![build(1, CHRONO, "snes")]
    }

    fn server_save(id: i64, stamp: &str, iso: &str) -> String {
        serde_json::json!({
            "id": id,
            "rom_id": 1,
            "file_name": format!("{CHRONO} [{stamp}].srm"),
            "emulator": "snes9x",
            "updated_at": iso,
            "download_path": format!("/api/raw/assets/saves/{id}.srm"),
        })
        .to_string()
    }

    #[tokio::test]
    async fn a_new_save_is_pushed_and_the_next_run_finds_it_in_sync() {
        let device = Device::new("push");
        device.save(&format!("{CHRONO}.srm"), "progress");
        let created = server_save(10, "2026-06-29_13-48-43", "2026-06-29T13:48:43Z");

        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("POST /api/saves", 201, created.clone()),
        ])
        .await;
        let report = device.sync(host, &chrono()).await;
        assert_eq!(report.pushed, vec![format!("{CHRONO}.srm")], "{report:?}");
        let upload = requests.lock().unwrap().iter().find(|r| r.starts_with("POST")).cloned().unwrap();
        assert!(upload.contains("rom_id=1") && upload.contains("slot=autosave"), "{upload}");

        // RomM now lists what was uploaded, under its own history name.
        let host = stub_server::start(vec![("GET /api/saves", 200, format!("[{created}]"))]).await;
        let report = device.sync(host, &chrono()).await;
        assert_eq!(report.in_sync, 1, "{report:?}");
        assert!(report.pushed.is_empty() && report.pulled.is_empty(), "{report:?}");
    }

    #[tokio::test]
    async fn a_sync_for_what_changed_moves_only_that_game_s_save() {
        let device = Device::new("narrow");
        let chrono = device.save(&format!("{CHRONO}.srm"), "progress");
        device.save("Secret of Mana (USA).srm", "other progress");
        let builds = vec![build(1, CHRONO, "snes"), build(2, "Secret of Mana (USA)", "snes")];
        let created = server_save(10, "2026-06-29_13-48-43", "2026-06-29T13:48:43Z");
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("POST /api/saves", 201, created),
        ])
        .await;
        let report = device.sync_changed(host, &builds, Some(&[chrono])).await;
        assert_eq!(report.pushed, vec![format!("{CHRONO}.srm")], "only the save that changed: {report:?}");
        assert_eq!(requests.lock().unwrap().iter().filter(|r| r.starts_with("POST")).count(), 1);
    }

    #[tokio::test]
    async fn a_sync_for_what_changed_takes_up_a_save_state_written_since() {
        // A game started from ES-DE without the hooks, which saved a state while the window sat
        // open: no launch of ours brackets it, so the watcher's sync is what has to carry it.
        let device = Device::sorted("changed-state");
        let roms = device.root.join("roms/snes");
        fs::create_dir_all(&roms).unwrap();
        fs::write(roms.join(format!("{CHRONO}.sfc")), "rom").unwrap();
        // This config names no savestate_directory, so RetroArch keeps states beside the games.
        let state = roms.join(format!("{CHRONO}.state"));
        fs::write(&state, "a state saved outside Deckhand").unwrap();

        let created = serde_json::json!({
            "id": 70, "rom_id": 1, "file_name": "created", "emulator": "snes9x",
            "updated_at": "2026-07-02T10:00:00Z", "download_path": "/api/raw/assets/states/70.state",
        });
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/states", 200, "[]".into()),
            ("POST /api/states", 201, created.to_string()),
        ])
        .await;
        let install = retroarch::read(&device.root.join("ra/retroarch.cfg")).unwrap();
        let plan = crate::launch::Plan {
            rom_id: 1,
            title: CHRONO.into(),
            rom: roms.join(format!("{CHRONO}.sfc")),
            system: "snes".into(),
            platform_id: None,
            launcher: Err("not needed to sync".into()),
            emulator: crate::launch::Emulator::Core("snes9x".into()),
            stamp: None,
            names: Vec::new(),
        };
        let report = run(
            &RommClient::new().unwrap(),
            &connection(host),
            &install,
            &chrono(),
            &device.store(),
            std::slice::from_ref(&plan),
            Some(&[state]),
        )
        .await
        .unwrap();

        assert_eq!(report.pushed, vec![format!("{CHRONO}.state")], "{report:?}");
        let lines = requests.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l.starts_with("POST /api/states?")), "{lines:?}");
        // The game's save is untouched: a state changing is not its save changing.
        assert!(!lines.iter().any(|l| l.starts_with("POST /api/saves")), "{lines:?}");
    }

    #[test]
    fn what_is_on_this_device_is_what_the_indexes_agreed_on_and_is_still_there() {
        let store = std::env::temp_dir().join(format!("deckhand-on-device-{}", std::process::id()));
        let _ = fs::remove_dir_all(&store);
        fs::create_dir_all(&store).unwrap();
        let here = store.join("here.srm");
        fs::write(&here, "progress").unwrap();
        let entry = |save_id| Entry { rom_id: 1, hash: "h".into(), save_id, ..Default::default() };
        let mut saves = Index::load(store.join("save-index.json"));
        saves.set(&here, entry(10)).unwrap();
        saves.set(&store.join("deleted.srm"), entry(11)).unwrap();
        // A standalone set is keyed on a name no file has, inside the folder it fills.
        let card = store.join("BASLUS-20946GTA50000");
        fs::create_dir_all(&card).unwrap();
        Index::load(store.join("standalone-index.json")).set(&card.join(".deckhand-set"), entry(20)).unwrap();
        Index::load(store.join("state-index.json")).set(&store.join("gone/x.state"), entry(30)).unwrap();
        // A set lying flat on a card it shares records its own files, because the card's folder is
        // there as long as the emulator is — one whose files are gone is not on this device.
        let gci = store.join("Card A");
        fs::create_dir_all(&gci).unwrap();
        fs::write(gci.join("01-GALE-here.gci"), "progress").unwrap();
        let flat = |save_id, file: &str| Entry {
            files: Some(vec![gci.join(file).to_string_lossy().into_owned()]),
            ..entry(save_id)
        };
        let mut sets = Index::load(store.join("standalone-index.json"));
        sets.set(&gci.join("GALE01.gci.zip"), flat(40, "01-GALE-here.gci")).unwrap();
        sets.set(&gci.join("GMSE01.gci.zip"), flat(41, "01-GMSE-deleted.gci")).unwrap();

        let ids = on_device(&store);
        assert_eq!(ids, [10, 20, 40].into_iter().collect(), "a deleted save, a missing folder or an emptied card is not here");
        let _ = fs::remove_dir_all(&store);
    }

    #[tokio::test]
    async fn a_registered_device_acknowledges_what_it_pulls_and_names_itself_on_a_push() {
        let device = Device::new("ack");
        let path = device.save(&format!("{CHRONO}.srm"), "old progress");
        Index::load(device.store().join("save-index.json"))
            .set(&path, Entry { rom_id: 1, hash: md5_hex(b"old progress"), save_id: 10, ..Default::default() })
            .unwrap();
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
            ("GET /api/raw/assets/saves/11.srm", 200, "new progress".into()),
            ("POST /api/saves/11/downloaded", 200, "{}".into()),
        ])
        .await;
        let install = retroarch::read(&device.root.join("ra/retroarch.cfg")).unwrap();
        let conn = Connection { device_id: Some("d-1".into()), ..connection(host) };
        let report = run(&RommClient::new().unwrap(), &conn, &install, &chrono(), &device.store(), &[], None).await.unwrap();
        assert_eq!(report.pulled.len(), 1, "{report:?}");
        assert!(requests.lock().unwrap().iter().any(|l| l.starts_with("POST /api/saves/11/downloaded")), "the pull is acknowledged");

        // Changed here now: the push says which device it is.
        fs::write(&path, "played after").unwrap();
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
            ("POST /api/saves", 201, server_save(12, "2026-07-02_10-00-00", "2026-07-02T10:00:00Z")),
        ])
        .await;
        let conn = Connection { device_id: Some("d-1".into()), ..connection(host) };
        let report = run(&RommClient::new().unwrap(), &conn, &install, &chrono(), &device.store(), &[], None).await.unwrap();
        assert_eq!(report.pushed.len(), 1, "{report:?}");
        let upload = requests.lock().unwrap().iter().find(|l| l.starts_with("POST /api/saves?")).cloned().unwrap();
        assert!(upload.contains("device_id=d-1"), "{upload}");
    }

    #[tokio::test]
    async fn a_save_changed_only_on_romm_is_pulled_over_the_local_one() {
        let device = Device::new("pull");
        let path = device.save(&format!("{CHRONO}.srm"), "old progress");
        Index::load(device.store().join("save-index.json"))
            .set(&path, Entry { rom_id: 1, hash: md5_hex(b"old progress"), save_id: 10, ..Default::default() })
            .unwrap();

        let saves = format!(
            "[{},{}]",
            server_save(10, "2026-06-29_13-48-43", "2026-06-29T13:48:43Z"),
            server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"),
        );
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, saves),
            ("GET /api/raw/assets/saves/11.srm", 200, "new progress".into()),
        ])
        .await;
        let report = device.sync(host, &chrono()).await;

        assert_eq!(report.pulled, vec![format!("{CHRONO}.srm")], "{report:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new progress");
        assert_eq!(Index::load(device.store().join("save-index.json")).get(&path).unwrap().save_id, 11);
    }

    #[tokio::test]
    async fn a_resolver_in_scope_decides_a_conflict_instead_of_the_clocks() {
        let device = Device::new("asked");
        let path = device.save(&format!("{CHRONO}.srm"), "played here, long ago");
        Index::load(device.store().join("save-index.json"))
            .set(&path, Entry { rom_id: 1, hash: md5_hex(b"what both last had"), save_id: 10, ..Default::default() })
            .unwrap();
        let long_ago = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        fs::File::options().write(true).open(&path).unwrap().set_modified(long_ago).unwrap();
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
            ("GET /api/raw/assets/saves/11.srm", 200, "played elsewhere, recently".into()),
            ("POST /api/saves", 201, server_save(12, "2026-07-02_10-00-00", "2026-07-02T10:00:00Z")),
        ])
        .await;

        // RomM's is newer, and the player keeps this device's anyway.
        let asked = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = asked.clone();
        let resolver: Resolver = std::sync::Arc::new(move |conflict: Conflict| {
            *seen.lock().unwrap() = Some(conflict);
            Box::pin(async { Choice::Local })
        });
        let report = with_resolver(resolver, device.sync(host, &chrono())).await;

        let conflict = asked.lock().unwrap().clone().expect("the conflict was put to the resolver");
        assert_eq!((conflict.name.as_str(), conflict.newer), (format!("{CHRONO}.srm").as_str(), Choice::Remote));
        assert!(report.conflicts[0].contains("kept this device's"), "{report:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "played here, long ago");
        assert!(requests.lock().unwrap().iter().any(|l| l.starts_with("POST /api/saves?")));
    }

    #[tokio::test]
    async fn when_both_changed_the_newer_wins_and_the_local_loser_is_kept() {
        let device = Device::new("conflict");
        let path = device.save(&format!("{CHRONO}.srm"), "played here, long ago");
        Index::load(device.store().join("save-index.json"))
            .set(&path, Entry { rom_id: 1, hash: md5_hex(b"what both last had"), save_id: 10, ..Default::default() })
            .unwrap();
        let long_ago = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        fs::File::options().write(true).open(&path).unwrap().set_modified(long_ago).unwrap();

        let host = stub_server::start(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
            ("GET /api/raw/assets/saves/11.srm", 200, "played elsewhere, recently".into()),
        ])
        .await;
        let report = device.sync(host, &chrono()).await;

        assert_eq!(report.conflicts.len(), 1, "{report:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "played elsewhere, recently");
        let kept: Vec<_> = fs::read_dir(device.store().join("save-backups")).unwrap().flatten().collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(kept[0].path()).unwrap(), "played here, long ago");
    }

    #[tokio::test]
    async fn a_save_first_seen_with_the_same_content_as_romm_is_agreed_without_a_transfer() {
        let device = Device::new("agree");
        let path = device.save(&format!("{CHRONO}.srm"), "same progress");
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(10, "2026-06-29_13-48-43", "2026-06-29T13:48:43Z"))),
            ("GET /api/raw/assets/saves/10.srm", 200, "same progress".into()),
        ])
        .await;
        let report = device.sync(host, &chrono()).await;

        assert_eq!(report.in_sync, 1, "{report:?}");
        assert!(report.conflicts.is_empty() && report.pushed.is_empty(), "{report:?}");
        assert_eq!(Index::load(device.store().join("save-index.json")).get(&path).unwrap().save_id, 10);
    }

    #[tokio::test]
    async fn a_save_no_game_is_named_for_is_reported_and_left_alone() {
        let device = Device::new("unmatched");
        device.save("Homebrew Thing.srm", "progress");
        let host = stub_server::start(vec![("GET /api/saves", 200, "[]".into())]).await;
        let report = device.sync(host, &chrono()).await;
        assert_eq!(report.unmatched, vec!["Homebrew Thing.srm".to_string()]);
    }

    // --- Around a launch ---------------------------------------------------------------------

    #[tokio::test]
    async fn a_first_launch_brings_romm_s_save_to_where_the_core_will_read_it() {
        // A device that has never had this save: the launch is what says the game is here.
        let device = Device::sorted("first-launch");
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
            ("GET /api/raw/assets/saves/11.srm", 200, "progress from the other device".into()),
        ])
        .await;
        let report = device.launch(host, &format!("snes/{CHRONO}.sfc"), Some("snes9x")).await.unwrap();

        assert_eq!(report.pulled, vec![format!("{CHRONO}.srm")], "{report:?}");
        let save = device.root.join("ra/saves/snes/Snes9x").join(format!("{CHRONO}.srm"));
        assert_eq!(fs::read_to_string(&save).unwrap(), "progress from the other device");
        assert_eq!(Index::load(device.store().join("save-index.json")).get(&save).unwrap().save_id, 11);
    }

    #[tokio::test]
    async fn a_first_launch_with_no_core_named_pulls_nothing_and_says_why() {
        let device = Device::sorted("no-core");
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, format!("[{}]", server_save(11, "2026-07-01_10-00-00", "2026-07-01T10:00:00Z"))),
        ])
        .await;
        let report = device.launch(host, &format!("snes/{CHRONO}.sfc"), None).await.unwrap();

        assert!(report.pulled.is_empty(), "{report:?}");
        assert!(report.notes.iter().any(|n| n.contains("no save here yet")), "{report:?}");
        assert!(!device.root.join("ra/saves/snes").exists(), "no folder made for a guessed core");
    }

    #[tokio::test]
    async fn after_playing_the_launched_game_s_save_is_pushed() {
        let device = Device::sorted("after");
        let save = device.root.join("ra/saves/snes/Snes9x").join(format!("{CHRONO}.srm"));
        fs::create_dir_all(save.parent().unwrap()).unwrap();
        fs::write(&save, "just played").unwrap();

        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("POST /api/saves", 201, server_save(12, "2026-07-02_10-00-00", "2026-07-02T10:00:00Z")),
        ])
        .await;
        // No core named: the save the core wrote is found in its folder all the same.
        let report = device.launch(host, &format!("snes/{CHRONO}.sfc"), None).await.unwrap();

        assert_eq!(report.pushed, vec![format!("{CHRONO}.srm")], "{report:?}");
        let upload = requests.lock().unwrap().iter().find(|r| r.starts_with("POST")).cloned().unwrap();
        assert!(upload.contains("emulator=snes9x"), "tagged with the core whose folder it is in: {upload}");
    }

    #[tokio::test]
    async fn a_launched_game_the_library_does_not_have_is_an_error() {
        let device = Device::sorted("not-in-library");
        let error = device.launch("http://127.0.0.1:9".into(), "snes/Homebrew Thing.sfc", Some("snes9x")).await;
        assert!(error.unwrap_err().contains("not in the library"));
    }
}

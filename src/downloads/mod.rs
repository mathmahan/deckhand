//! Downloading a game from RomM onto this device, and remembering what is here.
//!
//! A port of `browse/data/DownloadQueue.kt`, with the parts that only exist to feed RetroDECK left
//! out. What is here is the queue's own shape, which is what makes a download feel like one:
//!
//! - **The badge is optimistic.** A press marks the game *Downloading* before any request is made,
//!   so pressing it is obviously acknowledged even when the tile is not in view.
//! - **Two at a time.** [`CONCURRENCY`] workers, the same gate the Kotlin queue uses. More does not
//!   make a link faster, and it makes every bar crawl at once.
//! - **Progress is throttled to ~4 Hz.** A chunk arrives hundreds of times a second and every
//!   report rebuilds the page; 250 ms is as often as a percentage is worth redrawing.
//! - **The queue survives a restart.** Whatever was queued or in flight is written to
//!   `downloads.json` and asked for again on the next launch. A rom is re-fetched from the start:
//!   the partial file was removed when its worker died, because a `.part` of unknown age is worse
//!   than no file at all.
//! - **Nothing is marked installed until it is on disk under its real name.** Each file is streamed
//!   to `<name>.part` and renamed — see [`crate::romm::RommClient::download_file`].
//!
//! What a finished download leaves behind is recorded in `installed.json` rather than re-derived by
//! walking the rom folders: that record is what the *installed* filter, the Downloads page and
//! removal all read, and three answers derived three ways would disagree. Removal is reversible on
//! purpose — the files go, the record goes, and one press downloads the game again.
//!
//! Around it: what Download offers when a game has discs or other builds ([`choice`]), unpacking a
//! game that came as an archive ([`extract`]), and the space it all takes — the drive it goes on,
//! a budget, and what makes room ([`storage`]). Not here: ES-DE publishing, which belongs to a
//! feature this build does not have.

pub mod choice;
pub mod detect;
pub mod extract;
pub mod layout;
pub mod storage;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Connection;
use crate::library::{Fingerprint, Library, Stamp};
use crate::romm::RommClient;

/// How many downloads run at once. The Kotlin queue's own gate: a second request keeps the link
/// busy while the first is waiting on the server, and a third only divides the same bandwidth into
/// more bars that each move slower.
pub const CONCURRENCY: usize = 2;

/// How often a running download reports its progress. A rom arrives in thousands of chunks and each
/// report rebuilds the page, so this is a budget rather than a delay — four frames a second is as
/// finely as a percentage reads.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Per-game download status, as the tiles, the hero and the detail page render it.
///
/// `Downloading` carries 0.0-1.0 of the total bytes the server said the rom is; `Unpacking` the same
/// of an archive's contents.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SyncState {
    #[default]
    RemoteOnly,
    Downloading(f32),
    Unpacking(f32),
    Installed,
    /// On this device, and RomM's copy has changed since it was downloaded.
    ///
    /// Told honestly: not by comparing what is on disk with what RomM holds — an unpacked archive or
    /// a generated playlist never adds up to the server's figures — but by RomM's own fingerprint of
    /// the rom (its size and content hash), recorded when it was downloaded, against the one it gives
    /// now.
    UpdateAvailable,
    /// The download stopped, carrying why. Kept on the game rather than only in a banner: the reason
    /// is what decides whether a retry is worth pressing.
    Failed(String),
}

impl SyncState {
    /// On this device, current or not.
    pub fn is_here(&self) -> bool {
        matches!(self, SyncState::Installed | SyncState::UpdateAvailable)
    }

    /// A worker has it, fetching or unpacking.
    pub fn is_busy(&self) -> bool {
        matches!(self, SyncState::Downloading(_) | SyncState::Unpacking(_))
    }
}

/// One game waiting for, or occupying, a worker.
///
/// The title and system come along rather than being looked up when the worker starts, because a
/// queue read back from disk on the next launch has to be able to say what it is doing before the
/// library has loaded.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QueueItem {
    /// The server this was asked of — stamped by [`Downloads::enqueue`], so a caller cannot get it
    /// wrong. A rom id means nothing without it (see [`Installed::host`]), and the queue outlives
    /// both the run and the pairing: an item asked of one RomM must never be fetched from another,
    /// which is what would happen if a resumed queue were taken at face value.
    #[serde(default)]
    pub host: String,
    pub rom_id: i64,
    pub system_id: String,
    pub title: String,
    /// Only these of the rom's files, by RomM's id — one disc of a set. None for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<i64>>,
    /// What was chosen, when it was not the whole game: "Disc 2", "Europe · Rev 1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What it weighs, when known before the server is asked — what a space budget makes room for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// A build other than the one the game's tile stands for, by what sets it apart: "Europe".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Which rom `rom_id` was when this was asked for — see [`Stamp`]. A download whose id RomM has
    /// given to another game since is refused rather than fetched as that game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
}

/// One game on this device: where its files are, and what they are.
///
/// The file list is recorded rather than re-derived from the folder, so removal takes exactly what
/// the download put there and nothing a user dropped in beside it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Installed {
    /// The server this rom came from, as the stored connection spells it.
    ///
    /// A rom id means nothing without it: RomM's ids are per-install, so a device paired to a
    /// second server would otherwise show *that* server's game 10 as on disk because the first
    /// server's game 10 is. [`crate::cache`] keys the library the same way, for the same reason —
    /// the difference here is that nothing is dropped on a mismatch, because the files are real and
    /// the first server's records have to survive a visit to the second.
    #[serde(default)]
    pub host: String,
    pub rom_id: i64,
    pub title: String,
    pub system_id: String,
    /// RomM's id for the platform, which is what its BIOS is listed under. None on a record written
    /// before it was kept; a launch then asks RomM for it.
    #[serde(default)]
    pub platform_id: Option<i64>,
    /// The **platform** folder the files were written under — `<downloads>/psx`, never deeper.
    ///
    /// A rom RomM keeps as a directory of its own has that directory in each of its [`files`]
    /// instead. Keeping the split here is what lets [`delete`] clean up: it may remove the
    /// directories it emptied up to and including this one, and must never climb past it into the
    /// downloads folder the user chose.
    ///
    /// [`files`]: Installed::files
    pub dir: PathBuf,
    /// Every file written, relative to [`Installed::dir`], in the order it was written — so a
    /// folder-structured rom's entries carry its folder. Includes the generated `.m3u`, which is a
    /// file this app made and must therefore be a file it removes.
    pub files: Vec<String>,
    pub bytes: u64,
    /// RomM's fingerprint of the rom when it was downloaded — see [`SyncState::UpdateAvailable`].
    /// None on a record from before it was kept, until a library refresh can vouch for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    /// The RomM files this holds, by id — which discs of a set, for one fetched a disc at a time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_files: Vec<i64>,
    /// Only some of the rom's files are here — "Disc 1", "Disc 1, Disc 3" — or None for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subset: Option<String>,
    /// A build other than the one the game's tile stands for — "Europe · Rev 1" — or None for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Kept when a space budget makes room.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// When it was downloaded, and when last played here — what "least recently used" weighs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played_at: Option<String>,
    /// Which rom `rom_id` was when this was downloaded — see [`Stamp`]. None on a record from before
    /// it was kept, until a library load vouches for one ([`Downloads::reconcile`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
    /// The ids RomM kept this game under before a rebuild gave it the one it has — what a Steam
    /// shortcut made before then still starts it by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formerly: Vec<i64>,
    /// RomM gives this record's id to another game now, and has this one under no other: set aside
    /// until it does — see [`Downloads::reconcile`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub astray: bool,
}

impl Installed {
    /// Absolute paths of everything this install owns.
    pub fn paths(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.files.iter().map(|f| self.dir.join(f))
    }

    /// When this was last used here — played, or failing that downloaded. What a space budget's
    /// "least recently used" means: a game that has only just arrived is not first in line.
    pub fn used(&self, played: Option<&str>) -> Option<std::time::SystemTime> {
        [self.played_at.as_deref(), self.installed_at.as_deref(), played]
            .into_iter()
            .flatten()
            .filter_map(crate::sync::parse_timestamp)
            .max()
    }

    /// Whether `now` — what RomM holds under this record's id — is the rom it was downloaded from.
    ///
    /// A record from before stamps were kept goes by its files' names, which are RomM's names for the
    /// rom: `Secret of Mana (USA).sfc`, or the folder a game kept as one is in.
    pub fn is(&self, now: &Stamp) -> bool {
        holds(self.stamp.as_ref(), &self.names(), now)
    }

    /// What RomM called the rom, as far as this record knows: the stamp's file name, or the names of
    /// the files it wrote.
    pub fn names(&self) -> Vec<String> {
        if let Some(stamp) = &self.stamp {
            return vec![stamp.stem.clone()];
        }
        let mut names: Vec<String> = self
            .files
            .iter()
            .flat_map(|file| {
                let path = Path::new(file);
                let first = path.components().next().filter(|_| path.components().count() > 1);
                [path.file_stem(), first.map(|c| c.as_os_str())]
            })
            .flatten()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The title with what part of the game this is, for a list: "Final Fantasy VII — Disc 1".
    pub fn name(&self) -> String {
        match (&self.build, &self.subset) {
            (Some(build), Some(part)) => format!("{} — {build}, {part}", self.title),
            (Some(build), None) => format!("{} — {build}", self.title),
            (None, Some(part)) => format!("{} — {part}", self.title),
            (None, None) => self.title.clone(),
        }
    }
}

/// Whether `now` is the rom a record stamped `stamp`, whose files are named `names`, was downloaded
/// from — see [`Installed::is`].
///
/// With neither to go by there is nothing that says it is another game, and it is taken as this one.
pub fn holds(stamp: Option<&Stamp>, names: &[String], now: &Stamp) -> bool {
    match stamp {
        Some(then) => then.same_rom(now),
        None => names.is_empty() || names.iter().any(|n| n.eq_ignore_ascii_case(&now.stem)),
    }
}

/// What a running download sends back. One stream per worker, so a message can only ever be about
/// the game that worker has.
#[derive(Debug, Clone)]
pub enum Event {
    Progress(i64, f32),
    /// Unpacking an archive that arrived, 0.0-1.0 of its contents.
    Unpacking(i64, f32),
    /// Boxed because this is the one large variant of a message enum that is cloned on every send.
    Finished(i64, Result<Box<Installed>, String>),
}

/// The queue, what is running, what failed, and what is on disk.
///
/// Holds no client and starts nothing itself: [`crate::app`] owns the async side, asks for the next
/// item through [`Downloads::start_next`] and hands results back through [`Downloads::finish`]. That
/// keeps every decision here synchronous and testable, and keeps the one place that can spawn work
/// in the one place that can also stop it.
///
/// There is no separate map of per-game states. A game's state *is* which of these four lists it is
/// in, so [`Downloads::state`] cannot disagree with what is actually queued or on disk — which an
/// overlay beside them could, and did in the shape this replaced.
#[derive(Debug)]
pub struct Downloads {
    /// Where `installed.json` and `downloads.json` live — the config directory, beside the pairing.
    store: PathBuf,
    /// The server every rom id here belongs to — see [`Installed::host`]. Empty before pairing,
    /// and changed by [`Downloads::set_host`] when the pairing does.
    /// Shared, so a record's key is built from it without copying the address — `state` is asked
    /// once per tile per frame.
    host: std::sync::Arc<str>,
    /// Asked for, not yet started, in the order asked.
    queue: VecDeque<QueueItem>,
    /// Has a worker, with how far it has got — and whether that is unpacking rather than fetching.
    active: Vec<(QueueItem, f32, bool)>,
    /// Stopped, with why. Kept with the item rather than as a bare reason per id, because the page
    /// still has to name a game whose worker is gone — and a queue resumed on the next launch may
    /// hold games the library has not loaded yet.
    failed: Vec<(QueueItem, String)>,
    /// Games on disk, keyed by server **and** rom id, so two servers' libraries cannot be confused
    /// for each other. Every host's records are held and written, and the reads below answer only
    /// for [`Downloads::host`]. Ordered, so the Downloads page and the counts are stable between
    /// runs rather than following a hash.
    installed: BTreeMap<(std::sync::Arc<str>, i64), Installed>,
    /// Records whose rom RomM has changed since — see [`Downloads::check_updates`].
    stale: HashSet<(std::sync::Arc<str>, i64)>,
    /// Records on a drive that is not here right now — an SD card not in. Not shown, not counted,
    /// and kept, so the games are back the moment it is.
    away: Vec<Installed>,
    /// Records whose game RomM no longer has under any id it gave — see [`Downloads::reconcile`].
    /// Not shown, not synced, not launched, and kept with their files: every library load looks for
    /// the game again, so one put back in RomM comes back here too.
    astray: Vec<Installed>,
    /// Whether the two files are written. False in fixture mode, which has no business recording a
    /// synthetic library's downloads over a real one's.
    writes: bool,
}

impl Downloads {
    /// Read what the last run left: what is on disk, and what it had not finished fetching.
    ///
    /// An entry whose files are gone is dropped, so the *installed* filter never offers a game that
    /// is not there — unless its whole folder is gone, which is a drive that is not mounted (an SD
    /// card not in) rather than files deleted: that one is set aside and kept, so the games come
    /// back with the card. A resumed queue item goes back in the queue, which is what makes a
    /// download survive a restart; it shows its badge from the first frame, exactly as a fresh press
    /// does.
    pub fn load(store: PathBuf, host: &str) -> Self {
        let records: Vec<Installed> = read_json(&store.join(INSTALLED_FILE)).unwrap_or_default();
        let queue: VecDeque<QueueItem> = read_json(&store.join(PENDING_FILE)).unwrap_or_default();
        let (mut installed, mut away, mut astray) = (BTreeMap::new(), Vec::new(), Vec::new());
        for record in records {
            if record.paths().all(|p| p.exists()) {
                if record.astray {
                    astray.push(record);
                } else {
                    installed.insert((record.host.as_str().into(), record.rom_id), record);
                }
            } else if !record.dir.exists() {
                away.push(record);
            }
        }
        Self { queue, installed, away, astray, writes: true, ..Self::detached(store, host) }
    }

    /// An empty set of downloads that neither reads nor writes the records on disk.
    ///
    /// What fixture mode and the tests get. A synthetic library's rom ids are its own, so reading
    /// the real `installed.json` would show games the fixture library does not contain — and
    /// writing it would record a synthetic session over a real one's.
    pub fn detached(store: PathBuf, host: &str) -> Self {
        Self {
            store,
            host: host.trim_end_matches('/').into(),
            queue: VecDeque::new(),
            active: Vec::new(),
            failed: Vec::new(),
            installed: BTreeMap::new(),
            stale: HashSet::new(),
            away: Vec::new(),
            astray: Vec::new(),
            writes: false,
        }
    }

    /// Point this at another server — a fresh pairing, or a sign-out, within one session.
    ///
    /// Nothing is thrown away. Every queue item and every record carries the server it belongs to,
    /// so the other server's simply stop being visible and stop being handed to a worker; a worker
    /// already running keeps the server it started against, and records against that one when it
    /// lands. Pairing back finds the queue where it was left.
    pub fn set_host(&mut self, host: &str) {
        self.host = host.trim_end_matches('/').into();
    }

    /// Whether `item` belongs to the server this is currently answering for.
    fn mine_item(&self, item: &QueueItem) -> bool {
        same_host(&item.host, &self.host)
    }

    /// This server's key for a rom id.
    fn key(&self, rom_id: i64) -> (std::sync::Arc<str>, i64) {
        (std::sync::Arc::clone(&self.host), rom_id)
    }

    /// The state to draw for one rom: which of the four lists it is in.
    pub fn state(&self, rom_id: i64) -> SyncState {
        let here = |item: &QueueItem| item.rom_id == rom_id && self.mine_item(item);
        if let Some((_, progress, unpacking)) = self.active.iter().find(|(i, _, _)| here(i)) {
            return if *unpacking { SyncState::Unpacking(*progress) } else { SyncState::Downloading(*progress) };
        }
        if self.queue.iter().any(here) {
            return SyncState::Downloading(0.0);
        }
        if let Some((_, why)) = self.failed.iter().find(|(i, _)| here(i)) {
            return SyncState::Failed(why.clone());
        }
        let key = self.key(rom_id);
        if self.installed.contains_key(&key) {
            return if self.stale.contains(&key) { SyncState::UpdateAvailable } else { SyncState::Installed };
        }
        SyncState::RemoteOnly
    }

    /// The state to draw for a game — every build under its tile, `builds` with its own first. A
    /// build being fetched shows while it runs; otherwise a game with a build here is here (an
    /// update waiting first), and a failure shows only for a game with nothing here — a failed
    /// attempt at another build is the Downloads page's to retry, not a reason to stop playing.
    pub fn state_of(&self, builds: impl IntoIterator<Item = i64>) -> SyncState {
        let states: Vec<SyncState> = builds.into_iter().map(|id| self.state(id)).collect();
        let pick = |want: &dyn Fn(&SyncState) -> bool| states.iter().find(|s| want(s)).cloned();
        pick(&SyncState::is_busy)
            .or_else(|| pick(&|s| matches!(s, SyncState::UpdateAvailable)))
            .or_else(|| pick(&|s| matches!(s, SyncState::Installed)))
            .or_else(|| pick(&|s| matches!(s, SyncState::Failed(_))))
            .unwrap_or_default()
    }

    /// The records of every build in `builds` that is on this device, in the order given.
    pub fn installed_builds(&self, builds: impl IntoIterator<Item = i64>) -> Vec<&Installed> {
        builds.into_iter().filter_map(|id| self.installed(id)).collect()
    }

    /// Hold every record of this server to what RomM keeps under its id now, in `library`.
    ///
    /// A rom id is only RomM's row number, and a RomM rebuilt from an empty database hands them out
    /// again as it scans — so a record kept from before names whichever game was scanned into its
    /// place, and syncing, launching or updating it by that id would treat one game as another.
    ///
    /// - A record whose id still holds its rom ([`Installed::is`]), or a rom of the very content it
    ///   downloaded by RomM's hash, takes that rom's [`Stamp`] — which is what lets the next load tell
    ///   even a file renamed in RomM from another game.
    /// - One whose id holds another game is looked for by its file name among its system's builds.
    ///   Found, it moves to that id, keeping the old one in [`Installed::formerly`].
    /// - Otherwise it is set aside with its files, out of every list, sync and launch, and looked for
    ///   again at the next load.
    ///
    /// An id the library does not hold at all is left as it is: a system that failed to load says
    /// nothing about the games on it.
    pub fn reconcile(&mut self, library: &Library) -> Reconciled {
        let host = self.host.clone();
        let mut done = Reconciled::default();
        let mut changed = false;
        let mut strays = Vec::new();
        let keys: Vec<_> = self.installed.keys().filter(|(h, _)| same_host(h, &host)).cloned().collect();
        for key in keys {
            let Some(now) = library.stamp(key.1) else { continue };
            let Some(record) = self.installed.get_mut(&key) else { continue };
            let same_content = record.fingerprint.as_ref().is_some_and(|then| {
                then.md5.is_some() && library.game(key.1).and_then(|g| g.fingerprint.as_ref()) == Some(then)
            });
            if record.is(&now) || same_content {
                if record.stamp.as_ref() != Some(&now) {
                    record.stamp = Some(now);
                    changed = true;
                }
            } else if let Some(record) = self.installed.remove(&key) {
                strays.push((record, false));
            }
        }
        // Set aside by an earlier load, and RomM's to find again — this server's.
        let (looking, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.astray).into_iter().partition(|r| same_host(&r.host, &host));
        self.astray = others;
        for (mut record, before) in strays.into_iter().chain(looking.into_iter().map(|r| (r, true))) {
            let found = match library.builds_named(&record.system_id, &record.names())[..] {
                [id] => Some(id),
                _ => None,
            };
            let key = |id: i64| (host.clone(), id);
            match found {
                Some(id) if !self.installed.contains_key(&key(id)) => {
                    let was = record.rom_id;
                    if was != id {
                        if !record.formerly.contains(&was) {
                            record.formerly.push(was);
                        }
                        done.moved.push((was, id));
                    }
                    record.rom_id = id;
                    record.stamp = library.stamp(id);
                    record.astray = false;
                    self.installed.insert(key(id), record);
                    changed = true;
                }
                // Downloaded again under its new id into the same files: this one is only the old
                // record of them.
                Some(id) if self.installed.get(&key(id)).is_some_and(|r| r.dir == record.dir && record.files.iter().all(|f| r.files.contains(f))) => {
                    changed = true;
                }
                _ => {
                    if !before {
                        done.astray.push(record.name());
                        changed = true;
                    }
                    record.astray = true;
                    self.astray.push(record);
                }
            }
        }
        if changed {
            self.persist_installed();
        }
        done
    }

    /// The record a Steam shortcut's id starts: the game under that id, or failing that the one RomM
    /// kept under it before a rebuild moved it ([`Installed::formerly`]).
    pub fn installed_as(&self, rom_id: i64) -> Option<&Installed> {
        self.installed(rom_id).or_else(|| self.mine().find(|i| i.formerly.contains(&rom_id)))
    }

    /// Note which records RomM's copy has moved on from, against `library` — RomM's fingerprint of
    /// each game's own rom now, against the one recorded at download.
    ///
    /// A record from before fingerprints were kept takes the current one when what it holds is
    /// exactly RomM's size — the same files, as far as anything here can tell — and otherwise stays
    /// unknown rather than guessed at. Builds a tile does not stand for (siblings) are not in the
    /// library's figures and are left as they are.
    pub fn check_updates(&mut self, library: &Library) {
        let current: std::collections::HashMap<i64, &Fingerprint> =
            library.all_games().iter().filter_map(|g| g.fingerprint.as_ref().map(|f| (g.rom_id, f))).collect();
        let host = self.host.clone();
        let mut adopted = false;
        self.stale.retain(|(h, _)| !same_host(h, &host));
        for ((h, rom_id), record) in self.installed.iter_mut() {
            if !same_host(h, &host) {
                continue;
            }
            let Some(now) = current.get(rom_id) else { continue };
            match &record.fingerprint {
                Some(then) if !then.same(now) => {
                    self.stale.insert((h.clone(), *rom_id));
                }
                Some(_) => {}
                None if record.subset.is_none() && record.build.is_none() && on_disk_bytes(record) == Some(now.bytes) => {
                    record.fingerprint = Some((*now).clone());
                    adopted = true;
                }
                None => {}
            }
        }
        if adopted {
            self.persist_installed();
        }
    }

    /// Mark a game kept, or not, when a space budget makes room. False when it is not here.
    pub fn set_pinned(&mut self, rom_id: i64, pinned: bool) -> bool {
        let key = self.key(rom_id);
        let Some(record) = self.installed.get_mut(&key) else { return false };
        record.pinned = pinned;
        self.persist_installed();
        true
    }

    /// Note that a game was played here, ending at `at`.
    pub fn mark_played(&mut self, rom_id: i64, at: &str) {
        let key = self.key(rom_id);
        if let Some(record) = self.installed.get_mut(&key) {
            record.played_at = Some(at.to_string());
            self.persist_installed();
        }
    }

    /// The item that failed for a rom, as it was asked for — what a retry asks for again.
    pub fn failed_item(&self, rom_id: i64) -> Option<QueueItem> {
        self.failed.iter().find(|(i, _)| i.rom_id == rom_id && self.mine_item(i)).map(|(i, _)| i.clone())
    }

    /// Every installed game of this server as making room weighs it: `kept` are never removed (a
    /// game being played, or one of these builds being fetched), and pinned ones neither.
    pub fn candidates(&self, played: &dyn Fn(i64) -> Option<String>, kept: &HashSet<i64>) -> Vec<storage::Candidate> {
        self.mine()
            .map(|i| storage::Candidate {
                rom_id: i.rom_id,
                title: i.name(),
                bytes: i.bytes,
                used: i.used(played(i.rom_id).as_deref()),
                kept: i.pinned || kept.contains(&i.rom_id) || self.is_queued(i.rom_id),
            })
            .collect()
    }

    /// What the downloads running now will weigh, as far as they said — what a budget counts as
    /// already spoken for.
    pub fn active_bytes(&self) -> u64 {
        self.active.iter().filter(|(item, _, _)| self.mine_item(item)).map(|(item, _, _)| item.bytes.unwrap_or(0)).sum()
    }

    /// This server's records, in id order.
    fn mine(&self) -> impl Iterator<Item = &Installed> {
        self.installed.values().filter(|i| same_host(&i.host, &self.host))
    }

    /// Record games found already on disk — see [`detect`] — for this server, skipping any a record
    /// already names. Answers how many were new.
    pub fn adopt(&mut self, found: Vec<Installed>) -> usize {
        let mut added = 0;
        for mut record in found {
            // Only a game that is nothing but on the server here: one queued, downloading or
            // unpacking since the look began has files of its own on the way — half-written, or
            // left by a failure — and its download records it when it is done.
            if self.state(record.rom_id) != SyncState::RemoteOnly {
                continue;
            }
            let key = (self.host.clone(), record.rom_id);
            record.host = self.host.to_string();
            self.installed.insert(key, record);
            added += 1;
        }
        if added > 0 {
            self.persist_installed();
        }
        added
    }

    /// Every file a record holds, by full path — what [`detect`] must not claim for another game.
    pub fn claimed(&self) -> HashSet<PathBuf> {
        self.installed.values().chain(&self.away).chain(&self.astray).flat_map(|i| i.files.iter().map(|f| i.dir.join(f))).collect()
    }

    /// A number that changes whenever which games are on disk here does — for keeping something
    /// worked out from [`Downloads::installed_ids`] without building the set to compare.
    pub fn installed_fingerprint(&self) -> u64 {
        self.mine().fold(self.host.len() as u64, |h, i| h.wrapping_mul(0x100_0000_01b3).wrapping_add(i.rom_id as u64))
    }

    /// The rom ids on disk — the *installed* filter, its chip count, and Home's "On this device"
    /// shelf.
    pub fn installed_ids(&self) -> HashSet<i64> {
        self.mine().map(|i| i.rom_id).collect()
    }

    /// What is on disk, largest first — the Downloads page's lower half, where the question being
    /// asked is which game to remove to get the space back.
    pub fn installed_by_size(&self) -> Vec<&Installed> {
        let mut all: Vec<&Installed> = self.mine().collect();
        all.sort_by_key(|i| std::cmp::Reverse(i.bytes));
        all
    }

    /// One game's record, for naming it after its library entry has gone.
    pub fn installed(&self, rom_id: i64) -> Option<&Installed> {
        self.installed.get(&self.key(rom_id))
    }

    pub fn installed_count(&self) -> usize {
        self.mine().count()
    }

    pub fn total_bytes(&self) -> u64 {
        self.mine().map(|i| i.bytes).sum()
    }

    /// Whether a worker is fetching or unpacking anything now.
    pub fn working(&self) -> bool {
        !self.active.is_empty()
    }

    /// Everything a worker is on or waiting for, running first and then in the order asked for.
    pub fn in_progress(&self) -> impl Iterator<Item = (&QueueItem, f32)> {
        self.all_pending().filter(|(item, _)| self.mine_item(item))
    }

    /// Every queue item, this server's and any other's — what gets persisted, and what the gate
    /// counts against. Only the reads above narrow to one server.
    fn all_pending(&self) -> impl Iterator<Item = (&QueueItem, f32)> {
        self.active
            .iter()
            .map(|(item, progress, _)| (item, *progress))
            .chain(self.queue.iter().map(|item| (item, 0.0)))
    }

    /// What stopped, and why, in the order it stopped.
    pub fn failures(&self) -> impl Iterator<Item = (&QueueItem, &str)> {
        self.failed
            .iter()
            .filter(|(item, _)| self.mine_item(item))
            .map(|(item, why)| (item, why.as_str()))
    }

    /// Whether this game already has a worker or a place in the queue — so a second press on a tile
    /// is "it is already coming" rather than a second download of the same rom.
    pub fn is_queued(&self, rom_id: i64) -> bool {
        self.in_progress().any(|(item, _)| item.rom_id == rom_id)
    }

    /// Queue a game, stamping it with the server this is paired to. False when it was already
    /// queued or in flight.
    ///
    /// A game that failed before is cleared on the way in: this *is* the retry, and leaving the old
    /// reason beside a running download would be two answers at once.
    pub fn enqueue(&mut self, mut item: QueueItem) -> bool {
        if self.is_queued(item.rom_id) {
            return false;
        }
        item.host = self.host.to_string();
        let (host, rom_id) = (self.host.clone(), item.rom_id);
        self.failed.retain(|(i, _)| !(i.rom_id == rom_id && same_host(&i.host, &host)));
        self.queue.push_back(item);
        self.persist_queue();
        true
    }

    /// The next item to start, or None when the gate is full or nothing is waiting. Moves it into
    /// the active set, so the caller spawning work and this agreeing on what is running is one step.
    ///
    /// Only this server's items are handed out: the caller fetches with the connection it has, so
    /// giving it an item asked of another RomM would download whatever *that* server calls rom 10.
    /// Such an item keeps its place and is picked up if the device pairs back.
    pub fn start_next(&mut self) -> Option<QueueItem> {
        if self.active.len() >= CONCURRENCY {
            return None;
        }
        let at = self.queue.iter().position(|i| self.mine_item(i))?;
        let item = self.queue.remove(at)?;
        self.active.push((item.clone(), 0.0, false));
        self.persist_queue();
        Some(item)
    }

    /// Note how far a running download has got.
    ///
    /// Ignored for a game with no worker: a stream that still had a throttled report in it when its
    /// game was removed must not put the badge back.
    pub fn progress(&mut self, rom_id: i64, fraction: f32) {
        self.advance(rom_id, fraction, false);
    }

    /// Note how far a running download's unpacking has got.
    pub fn unpacking(&mut self, rom_id: i64, fraction: f32) {
        self.advance(rom_id, fraction, true);
    }

    fn advance(&mut self, rom_id: i64, fraction: f32, unpacking: bool) {
        let host = self.host.clone();
        if let Some((_, held, phase)) = self
            .active
            .iter_mut()
            .find(|(i, _, _)| i.rom_id == rom_id && same_host(&i.host, &host))
        {
            *held = fraction.clamp(0.0, 1.0);
            *phase = unpacking;
        }
    }

    /// Record what a worker finished with, and free its slot.
    pub fn finish(&mut self, rom_id: i64, result: Result<Installed, String>) {
        let item = self
            .active
            .iter()
            .position(|(i, _, _)| i.rom_id == rom_id)
            .map(|at| self.active.remove(at).0);
        self.persist_queue();
        // The *worker's* server, not whatever this is paired to now: a download that was running
        // when the device signed out or re-paired landed on the first server's disk, and has to be
        // recorded against it. Falls back to the current one for a result nothing was running.
        let host = item.as_ref().map(|i| i.host.clone()).unwrap_or_else(|| self.host.to_string());
        match result {
            Ok(mut installed) => {
                // Stamped here rather than in `fetch`, which knows the connection it used but not
                // how this session spells it — and those have to be the same answer.
                installed.host.clone_from(&host);
                let key: (std::sync::Arc<str>, i64) = (host.as_str().into(), rom_id);
                // What the player set on the record while it downloaded stays theirs.
                if let Some(before) = self.installed.get(&key) {
                    installed.pinned |= before.pinned;
                    installed.played_at = installed.played_at.take().or_else(|| before.played_at.clone());
                }
                self.stale.remove(&key);
                self.installed.insert(key, installed);
                self.persist_installed();
            }
            Err(why) => {
                // A result for a game nothing was running still has to be sayable, so the item is
                // reconstructed rather than dropped on the floor.
                let item = item.unwrap_or_else(|| QueueItem {
                    host,
                    rom_id,
                    system_id: String::new(),
                    title: format!("#{rom_id}"),
                    ..Default::default()
                });
                self.failed.push((item, why));
            }
        }
    }

    /// Stop asking for a game: take it out of the queue, or out of the running set — the caller stops
    /// its worker. Answers whether there was anything to stop.
    pub fn cancel(&mut self, rom_id: i64) -> bool {
        let host = self.host.clone();
        let mine = |i: &QueueItem| i.rom_id == rom_id && same_host(&i.host, &host);
        let (queued, running) = (self.queue.len(), self.active.len());
        self.queue.retain(|i| !mine(i));
        self.active.retain(|(i, _, _)| !mine(i));
        let stopped = self.queue.len() != queued || self.active.len() != running;
        if stopped {
            self.persist_queue();
        }
        stopped
    }

    /// Forget a game: drop its record and hand back what was on disk, for the caller to delete.
    ///
    /// Splitting the two is what keeps the file removal off the UI thread while the page updates in
    /// the same frame as the press. A failed delete leaves files behind rather than a record of
    /// files that are gone, which is the harmless direction: the next download overwrites them.
    ///
    /// On a game that only failed this is "dismiss": the reason goes and nothing was on disk.
    pub fn remove(&mut self, rom_id: i64) -> Option<Installed> {
        self.failed.retain(|(i, _)| !(i.rom_id == rom_id && same_host(&i.host, &self.host)));
        let gone = self.installed.remove(&self.key(rom_id))?;
        self.stale.remove(&self.key(rom_id));
        self.persist_installed();
        Some(gone)
    }

    fn persist_queue(&self) {
        if self.writes {
            // Active items included: a worker that dies with the app has to be asked for again.
            // Every server's, not only this one's — an item is not abandoned by pairing elsewhere.
            let pending: Vec<&QueueItem> = self.all_pending().map(|(item, _)| item).collect();
            write_json(&self.store.join(PENDING_FILE), &pending);
        }
    }

    /// Write the records — merged with what another Deckhand on this device wrote since this one
    /// read them.
    ///
    /// `deckhand play`, started by Steam, reads the records as its game starts and notes when it
    /// was played as the game ends, perhaps hours later, perhaps with this window open throughout.
    /// Which games are here is this process's to say — it is the one that downloads and removes
    /// them — but a newer "last played" on disk is kept, or the window would write it away again
    /// and the space budget would think a game just played was the one to make room with. The two
    /// take turns through `installed.lock`.
    fn persist_installed(&self) {
        if !self.writes {
            return;
        }
        let path = self.store.join(INSTALLED_FILE);
        let _turn = lock(&self.store.join("installed.lock"));
        // By the files as well as the id: a rebuilt RomM's ids move between games, and a game's time
        // is its own files' — see `reconcile`.
        let played_elsewhere: HashMap<(String, i64, PathBuf, Vec<String>), String> = read_json::<Vec<Installed>>(&path)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| Some(((r.host, r.rom_id, r.dir, r.files), r.played_at?)))
            .collect();
        // A record downloaded again while its drive was away replaces the one set aside.
        let away = self.away.iter().filter(|a| !self.installed.contains_key(&(a.host.as_str().into(), a.rom_id)));
        let mut all: Vec<Installed> = self.installed.values().chain(away).chain(&self.astray).cloned().collect();
        for record in &mut all {
            let key = (record.host.clone(), record.rom_id, record.dir.clone(), record.files.clone());
            if let Some(there) = played_elsewhere.get(&key)
                && record.played_at.as_ref().is_none_or(|here| here < there)
            {
                record.played_at = Some(there.clone());
            }
        }
        write_json(&path, &all);
    }
}

/// What [`Downloads::reconcile`] did that is worth saying.
#[derive(Debug, Default, PartialEq)]
pub struct Reconciled {
    /// Records moved to the id RomM keeps their game under now, as (was, is).
    pub moved: Vec<(i64, i64)>,
    /// Records set aside, by name: RomM gives their id to another game, and has theirs under none.
    pub astray: Vec<String>,
}

const INSTALLED_FILE: &str = "installed.json";
const PENDING_FILE: &str = "downloads.json";

/// Whether two stored server addresses are the same one. Trailing slash only, as [`crate::cache`]
/// compares them — both come from the same stored connection, not from user input.
fn same_host(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Write `value` as JSON, through a temp file — flushed to the disk before it replaces the old one —
/// so neither a crash nor a power cut mid-write leaves a truncated record.
///
/// Best effort: a record that cannot be written costs the next launch its knowledge of one
/// download, which is not worth failing a working session over.
fn write_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_string_pretty(value) else { return };
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    let written = fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(json.as_bytes())?;
        file.sync_all()
    });
    if written.is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Hold `path`'s lock until the answer is dropped — the turn-taking between processes over one file.
/// None where the filesystem cannot lock, which then goes ahead as it would have without.
fn lock(path: &Path) -> Option<fs::File> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    file.lock().ok()?;
    Some(file)
}

/// How far a worker has got: fetching the files, then unpacking any that came as an archive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Fetching(f32),
    Unpacking(f32),
}

/// Fetch one game — or the part of it `item` chose — under `roms_root`, and answer with the record
/// of what is on disk now.
///
/// `existing` is this rom's record when some of it is already here: another disc of a set joins the
/// discs it has, and the playlist is rebuilt over all of them; an update replaces what changed. It is
/// also where the files go — the drive they are on — whatever the downloads folder has since become.
///
/// `progress` is called with how far it has got, already throttled to [`PROGRESS_INTERVAL`] — the
/// throttle is here rather than at the call site because this is what knows how many chunks are
/// behind each percent.
pub async fn fetch(
    client: &RommClient,
    conn: &Connection,
    roms_root: &Path,
    item: &QueueItem,
    existing: Option<Installed>,
    mut progress: impl FnMut(Phase),
) -> Result<Installed, String> {
    let detail = client.fetch_rom_detail(conn, item.rom_id).await.map_err(|e| e.to_string())?;
    // The id still the game it was asked for, and still the one on disk: a queue resumed after RomM
    // was rebuilt would otherwise fetch whichever game was scanned into that place, over this one.
    let stamp = detail.stamp();
    if let Some(now) = &stamp
        && (item.stamp.as_ref().is_some_and(|asked| !asked.same_rom(now)) || existing.as_ref().is_some_and(|e| !e.is(now)))
    {
        let other = detail.name.as_deref().unwrap_or(&now.stem);
        return Err(format!("RomM has given this game's id to \"{other}\" since — open the game again to download it"));
    }
    // A metadata-only rom: RomM knows this build of the game — often a sibling variant — but has no
    // file uploaded for it, so there is genuinely nothing to fetch. Said plainly, because a retry
    // cannot fix it.
    if detail.files.is_empty() {
        return Err("RomM has no file for this build of the game, so there is nothing to download".into());
    }

    let slug = detail.platform_slug.clone().unwrap_or_else(|| item.system_id.clone());
    // In RetroDECK's roms folder, ES-DE's name for the system: the folders it scans are its own.
    let folder = if crate::launch::retrodeck_roms().as_deref() == Some(roms_root) { crate::launch::es_system(&slug) } else { slug.clone() };
    let dir = match &existing {
        Some(record) => record.dir.clone(),
        None => roms_root.join(layout::safe_name(&folder)),
    };
    // Naming the directory is the difference between "download failed" and something the user can
    // act on. A downloads folder pointed at a drive that is no longer mounted fails here first and
    // fails here always, and without this it surfaces as a bare "No such file or directory" naming
    // a file rather than the folder.
    fs::create_dir_all(&dir).map_err(|e| {
        format!("can't create {} — check where downloads go in Settings: {e}", dir.display())
    })?;

    // A rom RomM keeps as its own directory gets that directory here too, as a prefix on every one
    // of its files rather than as a deeper `dir` — see [`Installed::dir`].
    let sub = layout::local_subdirectory(&detail);
    let base = layout::base_path(&detail);
    let relative_of = |file: &crate::romm::models::RomFile| match &sub {
        Some(sub) => format!("{sub}/{}", layout::relative_path(file, &base)),
        None => layout::relative_path(file, &base),
    };

    let wanted: Vec<&crate::romm::models::RomFile> = match &item.files {
        Some(ids) => {
            let picked: Vec<_> = detail.files.iter().filter(|f| ids.contains(&f.id)).collect();
            if picked.is_empty() {
                return Err("RomM no longer has the files that were picked — choose again".into());
            }
            picked
        }
        None => detail.files.iter().collect(),
    };
    let fingerprint = Fingerprint::of(detail.fs_size_bytes, detail.md5_hash.as_deref());
    // An update replaces what is here; another disc of a set only adds to it, and a file already
    // here at RomM's size is the same file.
    let update = existing.as_ref().is_some_and(|e| match (&e.fingerprint, &fingerprint) {
        (Some(then), Some(now)) => !then.same(now),
        _ => false,
    });
    let have: HashSet<String> = existing.iter().flat_map(|e| e.files.iter().cloned()).collect();
    let needed: Vec<&crate::romm::models::RomFile> = wanted
        .iter()
        .copied()
        .filter(|f| {
            let relative = relative_of(f);
            update || !have.contains(&relative) || fs::metadata(dir.join(&relative)).map_or(true, |m| m.len() != f.size())
        })
        .collect();

    // Room for the files, and for unpacking an archive among them before the archive goes.
    let unpacks = |f: &crate::romm::models::RomFile| f.is_game() && extract::is_archive(&f.file_name) && !extract::keeps_archives(&slug);
    let total = needed.iter().map(|f| f.size()).sum::<u64>();
    let room = total + needed.iter().filter(|f| unpacks(f)).map(|f| f.size()).sum::<u64>();
    if let Some(free) = storage::available(&dir)
        && free < room
    {
        return Err(format!(
            "not enough space on the drive for {}: it needs {}, and {} is free",
            dir.display(),
            human_bytes(room),
            human_bytes(free)
        ));
    }

    let total = total.max(1);
    let mut done = 0u64;
    let mut fetched: Vec<(String, bool)> = Vec::new();
    let mut last_report = Instant::now() - PROGRESS_INTERVAL;
    for file in &needed {
        let relative = relative_of(file);
        client
            .download_file(conn, detail.id, file, &dir.join(&relative), |copied| {
                if last_report.elapsed() >= PROGRESS_INTERVAL {
                    last_report = Instant::now();
                    progress(Phase::Fetching(((done + copied) as f32 / total as f32).clamp(0.0, 1.0)));
                }
            })
            .await
            .map_err(|e| format!("{}: {e}", file.file_name))?;
        // The server's figure, not the bytes that arrived, so the running total stays comparable
        // with `total` even on a server that under-reports a file's size.
        done += file.size();
        fetched.push((relative, unpacks(file)));
    }
    progress(Phase::Fetching(1.0));

    // Everything this rom has on disk now: what was here, what arrived. Which are the game itself
    // decides the playlist — an update or a manual beside two discs must not become a third disc.
    let game_files: HashSet<String> = detail.files.iter().filter(|f| f.is_game()).map(&relative_of).collect();
    let playlist_name = match &sub {
        Some(sub) => format!("{sub}/{}.m3u", layout::safe_name(&item.title)),
        None => format!("{}.m3u", layout::safe_name(&item.title)),
    };
    let mut files: Vec<String> = existing.iter().flat_map(|e| e.files.iter().cloned()).filter(|f| *f != playlist_name).collect();
    let mut extracted: HashSet<String> = HashSet::new();
    for (relative, _) in &fetched {
        if !files.contains(relative) {
            files.push(relative.clone());
        }
    }

    // Unpack what came as an archive, where it landed — see [`extract`].
    for (relative, _) in fetched.iter().filter(|(_, unpack)| *unpack) {
        let archive = dir.join(relative);
        let ours: HashSet<PathBuf> = files.iter().map(|f| dir.join(f)).collect();
        let mut last = Instant::now() - PROGRESS_INTERVAL;
        let unpacked = tokio::task::block_in_place(|| {
            extract::unpack(&archive, &|p| ours.contains(p), &mut |fraction| {
                if last.elapsed() >= PROGRESS_INTERVAL {
                    last = Instant::now();
                    progress(Phase::Unpacking(fraction));
                }
            })
        });
        let unpacked = match unpacked {
            Ok(unpacked) => unpacked,
            // Kept as RomM sent it, as every download was before anything here unpacked: an emulator
            // that reads archives may still open it, and fetching it again would read no better.
            Err(extract::Failure::Unreadable(why)) => {
                eprintln!("deckhand: kept {relative} as it came — {why}");
                continue;
            }
            Err(extract::Failure::Device(why)) => return Err(why),
        };
        let _ = fs::remove_file(&archive);
        files.retain(|f| f != relative);
        for path in unpacked {
            if let Ok(inside) = path.strip_prefix(&dir) {
                let inside = inside.to_string_lossy().replace('\\', "/");
                extracted.insert(inside.clone());
                if !files.contains(&inside) {
                    files.push(inside);
                }
            }
        }
    }

    // An update's files RomM no longer has: this install's, so they go.
    if update {
        let now: HashSet<&String> = files.iter().collect();
        let fresh: HashSet<String> = fetched.iter().map(|(f, _)| f.clone()).chain(extracted.iter().cloned()).collect();
        for old in existing.iter().flat_map(|e| e.files.iter()) {
            if now.contains(old) && !fresh.contains(old) && !wanted.iter().any(|f| relative_of(f) == *old) {
                let _ = fs::remove_file(dir.join(old));
            }
        }
        files.retain(|f| fresh.contains(f) || wanted.iter().any(|w| relative_of(w) == *f));
    }

    // The multi-disc playlist, over every disc here now — relative to the playlist, which sits in the
    // rom's own folder when it has one.
    let discs: Vec<String> = files
        .iter()
        .filter(|f| game_files.contains(*f) || extracted.contains(*f))
        .filter(|f| layout::extension(f) != "m3u")
        .map(|path| match &sub {
            Some(sub) => path.strip_prefix(&format!("{sub}/")).unwrap_or(path).to_string(),
            None => path.clone(),
        })
        .collect();
    let playlist = dir.join(&playlist_name);
    match layout::m3u_content(&discs) {
        Some(content) => {
            fs::write(&playlist, &content).map_err(|e| format!("can't write {}: {e}", playlist.display()))?;
            files.push(playlist_name);
        }
        // Down to one disc, or none: no playlist, and none left over from before.
        None => {
            let _ = fs::remove_file(&playlist);
        }
    }

    // What of the rom this holds, and what that is called when it is not all of it.
    let mut source_files: Vec<i64> = if update { Vec::new() } else { existing.as_ref().map(|e| e.source_files.clone()).unwrap_or_default() };
    for file in &wanted {
        if !source_files.contains(&file.id) {
            source_files.push(file.id);
        }
    }
    let complete = detail.files.iter().filter(|f| f.is_game()).all(|f| source_files.contains(&f.id));
    let subset = (!complete).then(|| {
        let mut parts: Vec<String> = existing.as_ref().and_then(|e| e.subset.clone()).into_iter().filter(|_| !update).collect();
        if let Some(label) = &item.label
            && !parts.iter().any(|p| p.split(", ").any(|x| x == label))
        {
            parts.push(label.clone());
        }
        parts.join(", ")
    });

    let bytes = files.iter().filter_map(|f| fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum();
    progress(Phase::Unpacking(1.0));
    Ok(Installed {
        // Left blank here and stamped by `Downloads::finish`: this knows which server it talked
        // to, but not which one the session is on, and those have to be the same answer.
        host: String::new(),
        rom_id: item.rom_id,
        title: item.title.clone(),
        system_id: item.system_id.clone(),
        platform_id: detail.platform_id,
        dir,
        files,
        bytes,
        fingerprint,
        source_files,
        subset: subset.filter(|s| !s.is_empty()),
        build: item.build.clone(),
        pinned: existing.as_ref().is_some_and(|e| e.pinned),
        formerly: existing.as_ref().map(|e| e.formerly.clone()).unwrap_or_default(),
        stamp,
        astray: false,
        installed_at: existing
            .as_ref()
            .and_then(|e| e.installed_at.clone())
            .or_else(|| Some(crate::sync::format_timestamp(std::time::SystemTime::now()))),
        played_at: existing.and_then(|e| e.played_at),
    })
}

/// What a record's files weigh on disk now, or None when one is missing.
fn on_disk_bytes(record: &Installed) -> Option<u64> {
    record
        .paths()
        .filter(|p| layout::extension(&p.to_string_lossy()) != "m3u")
        .map(|p| fs::metadata(p).ok().map(|m| m.len()))
        .sum()
}

/// The games downloaded from `host` whose files are all still here, from `installed.json` in `store`
/// — what a save sweep syncs beyond the saves already on disk.
pub fn installed(store: &Path, host: &str) -> Vec<Installed> {
    let installed: Vec<Installed> = read_json(&store.join(INSTALLED_FILE)).unwrap_or_default();
    installed.into_iter().filter(|i| same_host(&i.host, host) && i.paths().all(|p| p.exists())).collect()
}

/// Every folder a downloaded game's files are in, from `installed.json` in `store`, for every server.
///
/// What a save sweep reads when RetroArch keeps saves beside the games: these are the only folders
/// this app knows hold games. Every server's, because a save's game is decided by its name against
/// the library, not by which record pointed at the folder.
pub fn content_dirs(store: &Path) -> Vec<PathBuf> {
    let installed: Vec<Installed> = read_json(&store.join(INSTALLED_FILE)).unwrap_or_default();
    let mut dirs: Vec<PathBuf> =
        installed.iter().flat_map(|i| i.paths().filter_map(|p| p.parent().map(Path::to_path_buf)).collect::<Vec<_>>()).collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Delete everything an install owns, then the directories it emptied.
///
/// Children before parents, and only a directory this left empty: the rom's own folder goes, the
/// platform folder it sat in goes if that was the last game in it, and anything a user put there
/// keeps the folder alive. Returns how many files went, so the banner can say.
pub fn delete(installed: &Installed) -> std::io::Result<usize> {
    let paths: Vec<PathBuf> = installed.paths().collect();
    let mut removed = 0;
    let mut first_error = None;
    for path in &paths {
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            // Already gone is the outcome asked for, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => first_error = first_error.or(Some(e)),
        }
    }

    // `fs::remove_dir` refuses a non-empty directory, which is exactly the test wanted: a folder
    // still holding a disc image a second game needs, or something the user dropped in, is left
    // alone and the attempt costs nothing. Deepest first, so a rom's own folder is tried before the
    // platform folder holding it, and never past `dir` — above it is the downloads folder the user
    // chose, which is not this app's to tidy away.
    let mut dirs: Vec<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
    dirs.push(&installed.dir);
    dirs.sort_unstable_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for dir in dirs {
        if dir.starts_with(&installed.dir) {
            let _ = fs::remove_dir(dir);
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(removed),
    }
}

/// "412 MB", "1.2 GB" — a figure read at a glance, not accounted to the byte.
pub fn human_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= 1024.0 * 1024.0 * MB {
        format!("{:.1} TB", bytes / (1024.0 * 1024.0 * MB))
    } else if bytes >= 1024.0 * MB {
        format!("{:.1} GB", bytes / (1024.0 * MB))
    } else if bytes >= MB {
        format!("{:.0} MB", bytes / MB)
    } else {
        format!("{:.0} KB", (bytes / 1024.0).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store in a temp directory of its own, removed when the test ends.
    struct Store(PathBuf);

    impl Store {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deckhand-downloads-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn downloads(&self) -> Downloads {
            self.paired_to(HOST)
        }

        fn paired_to(&self, host: &str) -> Downloads {
            Downloads::load(self.0.clone(), host)
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const HOST: &str = "https://romm.local";

    /// `host` is left blank: `enqueue` stamps it, which is what every caller relies on.
    fn item(rom_id: i64) -> QueueItem {
        QueueItem {
            host: String::new(),
            rom_id,
            system_id: "snes".into(),
            title: format!("Game {rom_id}"),
            ..Default::default()
        }
    }

    fn installed_in(dir: &Path, rom_id: i64, files: &[&str]) -> Installed {
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"rom").unwrap();
        }
        Installed {
            // Stamped by `finish`, which is the only thing that knows which server this is.
            host: String::new(),
            rom_id,
            title: format!("Game {rom_id}"),
            system_id: "snes".into(),
            platform_id: None,
            dir: dir.to_path_buf(),
            files: files.iter().map(|f| f.to_string()).collect(),
            bytes: 3 * files.len() as u64,
            ..Default::default()
        }
    }

    #[test]
    fn the_window_and_a_launch_from_steam_keep_each_others_news() {
        let store = Store::new("two-writers");
        let games = store.0.join("roms");
        let mut window = store.downloads();
        for rom_id in [1, 2] {
            window.enqueue(item(rom_id));
            window.start_next();
            window.finish(rom_id, Ok(installed_in(&games.join(format!("g{rom_id}")), rom_id, &["game.sfc"])));
        }
        // `deckhand play 1` starts with the records as they are, and the game runs for a while.
        let launched = store.downloads();
        // Meanwhile the window removes game 2…
        assert!(window.remove(2).is_some());
        // …and the game ends: the launch reads the records afresh, as `cli::play` does, and notes it.
        drop(launched);
        store.downloads().mark_played(1, "2026-10-06T12:00:00Z");
        // The window writes again, knowing nothing of that play.
        window.set_pinned(1, true);

        let after = store.downloads();
        assert!(after.installed(2).is_none(), "the window's removal stands");
        assert_eq!(after.installed(1).and_then(|i| i.played_at.as_deref()), Some("2026-10-06T12:00:00Z"), "and so does the play");
    }

    #[test]
    fn a_press_shows_before_anything_is_asked_of_the_server() {
        let store = Store::new("optimistic");
        let mut downloads = store.downloads();
        assert_eq!(downloads.state(1), SyncState::RemoteOnly);
        assert!(downloads.enqueue(item(1)));
        assert_eq!(downloads.state(1), SyncState::Downloading(0.0), "queued, nothing asked yet");
    }

    #[test]
    fn a_second_press_on_the_same_game_is_not_a_second_download() {
        let store = Store::new("dedupe");
        let mut downloads = store.downloads();
        assert!(downloads.enqueue(item(1)));
        assert!(!downloads.enqueue(item(1)), "already queued");
        downloads.start_next();
        assert!(!downloads.enqueue(item(1)), "already in flight");
        assert_eq!(downloads.in_progress().count(), 1);
    }

    #[test]
    fn only_two_run_at_once_and_a_finish_frees_the_next() {
        let store = Store::new("gate");
        let mut downloads = store.downloads();
        for id in 1..=4 {
            downloads.enqueue(item(id));
        }
        assert_eq!(downloads.start_next().map(|i| i.rom_id), Some(1));
        assert_eq!(downloads.start_next().map(|i| i.rom_id), Some(2));
        assert_eq!(downloads.start_next(), None, "the gate is full at {CONCURRENCY}");

        downloads.finish(1, Err("no".into()));
        assert_eq!(downloads.start_next().map(|i| i.rom_id), Some(3), "in the order asked for");
    }

    #[test]
    fn a_finished_download_is_on_disk_and_a_failed_one_says_why() {
        let store = Store::new("finish");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.enqueue(item(2));
        downloads.start_next();
        downloads.start_next();

        let landed = installed_in(&store.0, 1, &["game.sfc"]);
        downloads.finish(1, Ok(landed));
        downloads.finish(2, Err("the download stopped early".into()));

        assert_eq!(downloads.state(1), SyncState::Installed);
        assert_eq!(downloads.state(2), SyncState::Failed("the download stopped early".into()));
        assert_eq!(downloads.installed_ids(), HashSet::from([1]));
        assert_eq!(downloads.total_bytes(), 3);
        assert_eq!(downloads.in_progress().count(), 0, "both slots are free again");
        // The page can still name the game that failed, though its worker is gone.
        assert_eq!(
            downloads.failures().map(|(i, _)| i.title.clone()).collect::<Vec<_>>(),
            vec!["Game 2"]
        );
    }

    #[test]
    fn progress_from_a_removed_game_does_not_put_its_badge_back() {
        let store = Store::new("stale-progress");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.start_next();
        downloads.finish(1, Ok(installed_in(&store.0, 1, &["game.sfc"])));
        downloads.remove(1);

        // The worker's stream can still have a throttled report in it when the game goes.
        downloads.progress(1, 0.5);
        assert_eq!(downloads.state(1), SyncState::RemoteOnly, "no worker, no badge");
    }

    #[test]
    fn a_running_download_reports_how_far_it_has_got() {
        let store = Store::new("progress");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.start_next();
        downloads.progress(1, 0.42);
        assert_eq!(downloads.state(1), SyncState::Downloading(0.42));
        // A server that under-reports a file's size can push the fraction past the end.
        downloads.progress(1, 1.9);
        assert_eq!(downloads.state(1), SyncState::Downloading(1.0));
    }

    #[test]
    fn the_queue_and_what_is_on_disk_survive_a_restart() {
        let store = Store::new("restart");
        {
            let mut downloads = store.downloads();
            downloads.enqueue(item(1));
            downloads.enqueue(item(2));
            // One was in flight when the app went away: it has to be asked for again, because its
            // partial file went with its worker.
            downloads.start_next();
            downloads.finish(3, Ok(installed_in(&store.0, 3, &["other.sfc"])));
        }

        let next_run = store.downloads();
        assert_eq!(
            next_run.in_progress().map(|(i, _)| i.rom_id).collect::<Vec<_>>(),
            vec![1, 2],
            "both the started one and the waiting one come back"
        );
        assert_eq!(next_run.state(1), SyncState::Downloading(0.0), "and show their badge at once");
        assert_eq!(next_run.state(3), SyncState::Installed);
    }

    #[test]
    fn an_install_whose_files_went_missing_is_not_offered_as_installed() {
        let store = Store::new("pruned");
        {
            let mut downloads = store.downloads();
            downloads.finish(1, Ok(installed_in(&store.0, 1, &["here.sfc"])));
            downloads.finish(2, Ok(installed_in(&store.0, 2, &["gone.sfc"])));
        }
        fs::remove_file(store.0.join("gone.sfc")).unwrap();

        let next_run = store.downloads();
        assert_eq!(next_run.installed_ids(), HashSet::from([1]), "deleted outside the app");
    }

    #[test]
    fn removing_a_game_takes_its_files_and_the_folders_it_emptied() {
        let store = Store::new("remove");
        let psx = store.0.join("psx");
        let installed = installed_in(
            &psx,
            1,
            &["Driver 2 (USA)/disc1.cue", "Driver 2 (USA)/disc1.bin", "Driver 2 (USA)/Driver 2.m3u"],
        );
        // A folder with something of the user's in it survives.
        let snes = store.0.join("snes");
        let sibling = installed_in(&snes, 2, &["other.sfc"]);
        fs::write(snes.join("notes.txt"), b"mine").unwrap();

        assert_eq!(delete(&installed).unwrap(), 3);
        assert!(!psx.join("Driver 2 (USA)").exists(), "the rom's own folder went with it");
        assert!(!psx.exists(), "and the platform folder it emptied");
        assert!(store.0.exists(), "but never the downloads folder itself");

        assert_eq!(delete(&sibling).unwrap(), 1);
        assert!(snes.exists(), "a folder still holding something is left alone");
        assert!(snes.join("notes.txt").exists());
    }

    #[test]
    fn removing_a_game_already_gone_from_disk_still_succeeds() {
        let store = Store::new("remove-missing");
        let installed = installed_in(&store.0, 1, &["game.sfc"]);
        fs::remove_file(store.0.join("game.sfc")).unwrap();
        assert_eq!(delete(&installed).unwrap(), 0, "already gone is the outcome asked for");
    }

    #[test]
    fn a_second_server_s_game_ten_is_not_this_server_s_game_ten() {
        let store = Store::new("two-servers");
        {
            let mut first = store.downloads();
            first.finish(10, Ok(installed_in(&store.0, 10, &["first.sfc"])));
            assert_eq!(first.installed_ids(), HashSet::from([10]));
        }

        // Same device, a different RomM. Its rom 10 is a different game entirely, and must not
        // show as already downloaded.
        let mut second = store.paired_to("https://other.example.com");
        assert_eq!(second.state(10), SyncState::RemoteOnly);
        assert_eq!(second.installed_count(), 0);
        assert_eq!(second.total_bytes(), 0);
        second.finish(10, Ok(installed_in(&store.0, 10, &["second.sfc"])));
        assert_eq!(second.installed_count(), 1);

        // And coming back to the first server finds its own record still there, not overwritten.
        let first = store.downloads();
        assert_eq!(first.state(10), SyncState::Installed);
        assert_eq!(first.installed_by_size()[0].files, vec!["first.sfc"]);
    }

    #[test]
    fn a_queue_asked_of_one_server_is_never_handed_to_another() {
        let store = Store::new("queue-host");
        {
            let mut first = store.downloads();
            first.enqueue(item(1));
        }

        // Fetching this with the other server's connection would download whatever *that* server
        // calls rom 1, so it is neither shown nor started — but it keeps its place.
        let mut other = store.paired_to("https://other.example.com");
        assert_eq!(other.in_progress().count(), 0);
        assert_eq!(other.state(1), SyncState::RemoteOnly);
        assert_eq!(other.start_next(), None, "and no worker is given it");

        // Pairing back picks it up where it was left.
        other.set_host(HOST);
        assert_eq!(other.start_next().map(|i| i.rom_id), Some(1));
    }

    #[test]
    fn signing_out_hides_the_queue_rather_than_abandoning_it() {
        let store = Store::new("sign-out");
        let mut downloads = store.downloads();
        downloads.finish(1, Ok(installed_in(&store.0, 1, &["kept.sfc"])));
        downloads.enqueue(item(2));
        downloads.finish(3, Err("the download stopped early".into()));

        downloads.set_host("");
        assert_eq!(downloads.in_progress().count(), 0, "nothing signed out can fetch it");
        assert_eq!(downloads.failures().count(), 0);
        assert_eq!(downloads.installed_count(), 0, "and no other server's library shows through");
        assert!(store.0.join("kept.sfc").is_file(), "but the files are real and stay");

        // Signing back in finds all three again.
        downloads.set_host(HOST);
        assert_eq!(downloads.state(1), SyncState::Installed);
        assert_eq!(downloads.in_progress().count(), 1);
        assert_eq!(downloads.failures().count(), 1);
    }

    #[test]
    fn a_download_that_lands_after_a_sign_out_is_recorded_against_the_server_it_came_from() {
        let store = Store::new("late-finish");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.start_next();
        // The worker is still talking to the first server when the device signs out.
        downloads.set_host("");
        downloads.finish(1, Ok(installed_in(&store.0, 1, &["landed.sfc"])));

        assert_eq!(downloads.state(1), SyncState::RemoteOnly, "not this server's game");
        downloads.set_host(HOST);
        assert_eq!(
            downloads.state(1),
            SyncState::Installed,
            "but recorded against the one that served it"
        );
    }

    #[test]
    fn a_retry_replaces_the_failure_it_is_retrying() {
        let store = Store::new("retry");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.start_next();
        downloads.finish(1, Err("the download stopped early".into()));

        assert!(downloads.enqueue(item(1)));
        assert_eq!(downloads.state(1), SyncState::Downloading(0.0));
        assert_eq!(downloads.failures().count(), 0, "one answer at a time, not two");
    }

    #[test]
    fn dismissing_a_failure_puts_the_game_back_to_fetchable() {
        let store = Store::new("dismiss");
        let mut downloads = store.downloads();
        downloads.enqueue(item(1));
        downloads.start_next();
        downloads.finish(1, Err("not connected to RomM".into()));

        assert!(downloads.remove(1).is_none(), "nothing was on disk to delete");
        assert_eq!(downloads.state(1), SyncState::RemoteOnly);
    }

    #[test]
    fn what_is_on_disk_is_listed_largest_first() {
        let store = Store::new("by-size");
        let mut downloads = store.downloads();
        downloads.finish(1, Ok(installed_in(&store.0, 1, &["small.sfc"])));
        downloads.finish(2, Ok(installed_in(&store.0, 2, &["a.cue", "a.bin", "b.cue"])));
        assert_eq!(
            downloads.installed_by_size().iter().map(|i| i.rom_id).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn sizes_read_at_a_glance() {
        assert_eq!(human_bytes(300), "1 KB");
        assert_eq!(human_bytes(412 * 1024 * 1024), "412 MB");
        assert_eq!(human_bytes(1288 * 1024 * 1024), "1.3 GB");
    }

    #[test]
    fn a_rom_romm_changed_since_it_was_downloaded_is_an_update_and_an_old_record_adopts_a_fingerprint() {
        let store = Store::new("updates");
        let mut downloads = store.downloads();
        let mut games = crate::fixtures::library(2).all_games().to_vec();
        games[0].fingerprint = Some(Fingerprint { bytes: 3, md5: Some("new".into()) });
        games[1].fingerprint = Some(Fingerprint { bytes: 3, md5: Some("same".into()) });
        let (a, b) = (games[0].rom_id, games[1].rom_id);
        let library = Library::new(crate::fixtures::library(2).systems, games);

        let dir = store.0.join("roms");
        let mut changed = installed_in(&dir, a, &["a.sfc"]);
        changed.fingerprint = Some(Fingerprint { bytes: 3, md5: Some("old".into()) });
        // From before fingerprints: its file is exactly RomM's size, so it is taken to be RomM's.
        let legacy = installed_in(&dir, b, &["b.sfc"]);
        for (id, record) in [(a, changed), (b, legacy)] {
            downloads.enqueue(item(id));
            downloads.start_next();
            downloads.finish(id, Ok(record));
        }

        downloads.check_updates(&library);
        assert_eq!(downloads.state(a), SyncState::UpdateAvailable);
        assert_eq!(downloads.state(b), SyncState::Installed);
        assert_eq!(downloads.installed(b).unwrap().fingerprint.as_ref().and_then(|f| f.md5.as_deref()), Some("same"));
        assert_eq!(downloads.state_of([999, a]), SyncState::UpdateAvailable, "a game's state covers every build of it");
    }

    /// A library of SNES games, each (rom id, file name, when RomM made it).
    fn rebuilt(games: &[(i64, &str, Option<&str>)]) -> Library {
        let games = games
            .iter()
            .map(|(id, stem, born)| crate::library::Game {
                rom_id: *id,
                title: stem.to_string(),
                system_id: "snes".into(),
                file_stems: vec![(*id, stem.to_string())],
                created_at: born.map(str::to_string),
                ..Default::default()
            })
            .collect();
        Library::new(crate::fixtures::library(1).systems, games)
    }

    fn stamp(stem: &str, born: &str) -> Option<Stamp> {
        Some(Stamp { stem: stem.into(), born: Some(born.into()) })
    }

    /// What is on disk, as `finish` records it.
    fn on_disk(downloads: &mut Downloads, record: Installed) {
        let id = record.rom_id;
        downloads.enqueue(item(id));
        downloads.start_next();
        downloads.finish(id, Ok(record));
    }

    const BEFORE: &str = "2026-08-01T22:26:48+00:00";
    const AFTER: &str = "2026-10-12T19:00:00+00:00";

    #[test]
    fn a_game_a_rebuilt_romm_keeps_under_another_id_follows_it_there_and_leaves_its_old_one_to_the_game_now_there() {
        let store = Store::new("rebuilt-moved");
        let mut downloads = store.downloads();
        let mut mana = installed_in(&store.0.join("roms"), 1, &["Secret of Mana (USA).sfc"]);
        mana.stamp = stamp("Secret of Mana (USA)", BEFORE);
        on_disk(&mut downloads, mana);

        let done = downloads.reconcile(&rebuilt(&[(1, "Zipang (Japan)", Some(AFTER)), (7, "Secret of Mana (USA)", Some(AFTER))]));

        assert_eq!(done, Reconciled { moved: vec![(1, 7)], astray: vec![] });
        let record = downloads.installed(7).expect("under the id RomM keeps it under now");
        assert_eq!(record.formerly, [1]);
        assert_eq!(record.stamp, stamp("Secret of Mana (USA)", AFTER), "and held to that rom from now on");
        assert_eq!(downloads.state(1), SyncState::RemoteOnly, "Zipang is not on this device");
        assert_eq!(downloads.installed_as(1).map(|r| r.rom_id), Some(7), "a Steam shortcut made with the old id still starts it");
        assert!(store.downloads().installed(7).is_some(), "and it is written down");
    }

    #[test]
    fn a_game_romm_no_longer_has_is_set_aside_with_its_files_and_comes_back_when_romm_has_it_again() {
        let store = Store::new("rebuilt-astray");
        let mut downloads = store.downloads();
        let mut banjo = installed_in(&store.0.join("roms"), 1, &["Banjo-Kazooie (USA).z64"]);
        banjo.stamp = stamp("Banjo-Kazooie (USA)", BEFORE);
        on_disk(&mut downloads, banjo);

        let without = rebuilt(&[(1, "Zipang (Japan)", Some(AFTER))]);
        assert_eq!(downloads.reconcile(&without), Reconciled { moved: vec![], astray: vec!["Game 1".into()] });
        assert!(downloads.installed(1).is_none() && downloads.installed_ids().is_empty(), "out of every list");
        assert!(downloads.claimed().contains(&store.0.join("roms/Banjo-Kazooie (USA).z64")), "its files still its own");
        assert_eq!(downloads.reconcile(&without), Reconciled::default(), "said once");

        let mut reopened = store.downloads();
        assert!(reopened.installed(1).is_none(), "set aside across a restart too");
        let back = rebuilt(&[(1, "Zipang (Japan)", Some(AFTER)), (12, "Banjo-Kazooie (USA)", Some(AFTER))]);
        assert_eq!(reopened.reconcile(&back).moved, [(1, 12)]);
        assert!(reopened.installed(12).is_some_and(|r| !r.astray));
    }

    #[test]
    fn a_record_from_before_stamps_is_vouched_for_by_its_files_or_romm_s_hash_and_a_renamed_file_is_still_its_game() {
        let store = Store::new("rebuilt-vouched");
        let mut downloads = store.downloads();
        let dir = store.0.join("roms");
        on_disk(&mut downloads, installed_in(&dir, 1, &["Chrono Trigger (USA).sfc"]));
        let mut hashed = installed_in(&dir, 2, &["unpacked.sfc"]);
        hashed.fingerprint = Some(Fingerprint { bytes: 3, md5: Some("abc".into()) });
        on_disk(&mut downloads, hashed);
        let mut renamed = installed_in(&dir, 3, &["Old Name.sfc"]);
        renamed.stamp = stamp("Old Name", BEFORE);
        on_disk(&mut downloads, renamed);
        on_disk(&mut downloads, installed_in(&dir, 4, &["Not Loaded.sfc"]));

        let mut library = rebuilt(&[(1, "Chrono Trigger (USA)", Some(BEFORE)), (2, "Earthbound (USA)", None), (3, "New Name", Some(BEFORE))]);
        let mut games = library.all_games().to_vec();
        games[1].fingerprint = Some(Fingerprint { bytes: 3, md5: Some("abc".into()) });
        library = Library::new(library.systems.clone(), games);

        assert_eq!(downloads.reconcile(&library), Reconciled::default());
        assert_eq!(downloads.installed(1).unwrap().stamp, stamp("Chrono Trigger (USA)", BEFORE), "taken from the library");
        assert!(downloads.installed(2).unwrap().stamp.is_some(), "the same content is the same rom");
        assert_eq!(downloads.installed(3).unwrap().stamp, stamp("New Name", BEFORE), "the same row in RomM, renamed");
        assert_eq!(downloads.installed(4).unwrap().stamp, None, "a game the library does not hold is left as it was");
    }

    #[test]
    fn two_games_a_rebuild_swapped_the_ids_of_each_follow_their_own() {
        let store = Store::new("rebuilt-swapped");
        let mut downloads = store.downloads();
        let dir = store.0.join("roms");
        for (id, stem) in [(1, "Alpha (USA)"), (2, "Beta (USA)")] {
            let mut record = installed_in(&dir, id, &[&format!("{stem}.sfc")]);
            record.stamp = stamp(stem, BEFORE);
            on_disk(&mut downloads, record);
        }

        let done = downloads.reconcile(&rebuilt(&[(1, "Beta (USA)", Some(AFTER)), (2, "Alpha (USA)", Some(AFTER))]));

        assert_eq!(done.moved.len(), 2);
        assert_eq!(downloads.installed(1).unwrap().files, ["Beta (USA).sfc"]);
        assert_eq!(downloads.installed(2).unwrap().files, ["Alpha (USA).sfc"]);
    }

    #[test]
    fn a_pinned_game_and_one_being_fetched_are_never_what_makes_room() {
        let store = Store::new("room");
        let mut downloads = store.downloads();
        let dir = store.0.join("roms");
        for (id, played) in [(1, "2026-01-01T00:00:00Z"), (2, "2026-02-01T00:00:00Z"), (3, "2026-03-01T00:00:00Z")] {
            let mut record = installed_in(&dir, id, &[&format!("{id}.sfc")]);
            record.played_at = Some(played.into());
            downloads.enqueue(item(id));
            downloads.start_next();
            downloads.finish(id, Ok(record));
        }
        downloads.set_pinned(1, true);
        let kept: HashSet<i64> = [2].into_iter().collect();
        let candidates = downloads.candidates(&|_| None, &kept);
        let gone: Vec<i64> = storage::make_room(&candidates, 7, 3).iter().map(|c| c.rom_id).collect();
        assert_eq!(gone, [3], "1 is pinned and 2 is being played: 3 goes, though it was played last");
    }

    #[test]
    fn a_game_on_a_drive_that_is_not_in_comes_back_with_it() {
        let store = Store::new("away");
        let card = store.0.join("card");
        {
            let mut downloads = store.downloads();
            downloads.enqueue(item(5));
            downloads.start_next();
            downloads.finish(5, Ok(installed_in(&card.join("snes"), 5, &["Game.sfc"])));
        }
        // The card is out: its folder is not there.
        fs::rename(&card, store.0.join("card-out")).unwrap();
        let mut downloads = store.downloads();
        assert_eq!(downloads.state(5), SyncState::RemoteOnly, "not offered while it is away");
        // Something else is recorded meanwhile, which writes the records back out.
        downloads.enqueue(item(6));
        downloads.start_next();
        downloads.finish(6, Ok(installed_in(&store.0.join("home/snes"), 6, &["Other.sfc"])));

        fs::rename(store.0.join("card-out"), &card).unwrap();
        let downloads = store.downloads();
        assert_eq!(downloads.state(5), SyncState::Installed, "back with the card");
        assert_eq!(downloads.state(6), SyncState::Installed);
    }
}

/// End-to-end tests against a stub RomM server.
///
/// `fetch` is almost entirely *translation*: RomM's library-root paths into local ones, its file
/// list into a write order, and a multi-disc set into a playlist. None of that is exercised by
/// testing [`layout`] alone, because the bug that mattered was in how the two were joined — so
/// these talk HTTP and then look at the disk.
#[cfg(test)]
mod round_trip {
    use super::*;
    use crate::stub_server;

    /// A download target in a temp directory of its own, removed when the test ends.
    struct Root(PathBuf);

    impl Root {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deckhand-fetch-{name}"));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn connection(host: &str) -> Connection {
        Connection {
            host: host.to_string(),
            token: "rmm_test".into(),
            insecure: false,
            token_id: None,
            token_name: None,
            device_id: None,
        }
    }

    fn item(rom_id: i64, system: &str, title: &str) -> QueueItem {
        QueueItem {
            host: String::new(),
            rom_id,
            system_id: system.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_plain_rom_lands_in_its_platform_folder_under_the_name_romm_knows() {
        let detail = serde_json::json!({
            "id": 7,
            "fs_name": "Chrono Trigger (USA).sfc",
            "fs_path": "roms/snes",
            "platform_slug": "snes",
            "files": [{
                "id": 70,
                "file_name": "Chrono Trigger (USA).sfc",
                "file_size_bytes": 11,
                "file_path": "roms/snes",
                "category": "game"
            }]
        })
        .to_string();
        // The content route goes first: the detail route's path is a prefix of it, and the stub
        // answers with the first pattern the request line contains.
        let host = stub_server::start(vec![
            ("GET /api/roms/7/content/", 200, "rom-content".into()),
            ("GET /api/roms/7", 200, detail),
        ])
        .await;

        let root = Root::new("flat");
        let client = RommClient::new().unwrap();
        let mut seen: Vec<Phase> = Vec::new();
        let installed = fetch(
            &client,
            &connection(&host),
            &root.0,
            &item(7, "snes", "Chrono Trigger"),
            None,
            |f| seen.push(f),
        )
        .await
        .expect("the download succeeds");

        let landed = root.0.join("snes").join("Chrono Trigger (USA).sfc");
        assert_eq!(fs::read_to_string(&landed).unwrap(), "rom-content");
        assert_eq!(installed.dir, root.0.join("snes"), "the platform folder, nothing deeper");
        assert_eq!(installed.files, vec!["Chrono Trigger (USA).sfc"]);
        assert_eq!(installed.bytes, 11);
        assert!(seen.contains(&Phase::Fetching(1.0)), "and it reports finishing");
        // Nothing is marked installed until it is under its real name.
        assert!(!root.0.join("snes").join("Chrono Trigger (USA).sfc.part").exists());
    }

    #[tokio::test]
    async fn a_multi_disc_rom_keeps_its_own_folder_and_gets_a_playlist() {
        let files = |id: i64, name: &str| {
            serde_json::json!({
                "id": id,
                "file_name": name,
                "file_size_bytes": 4,
                // Filed from RomM's library root, *including* the rom's own folder — the shape
                // that was once read as a subpath and rebuilt the whole library under the
                // platform directory.
                "file_path": "roms/psx/Driver 2 (USA)",
                "category": "game"
            })
        };
        let detail = serde_json::json!({
            "id": 8,
            "fs_name": "Driver 2 (USA)",
            "fs_path": "roms/psx",
            "platform_slug": "psx",
            "files": [
                files(80, "disc1.cue"),
                files(81, "disc1.bin"),
                files(82, "disc2.cue"),
                files(83, "disc2.bin"),
            ]
        })
        .to_string();
        let host = stub_server::start(vec![
            ("GET /api/roms/8/content/", 200, "disc".into()),
            ("GET /api/roms/8", 200, detail),
        ])
        .await;

        let root = Root::new("multi-disc");
        let client = RommClient::new().unwrap();
        let installed =
            fetch(&client, &connection(&host), &root.0, &item(8, "psx", "Driver 2"), None, |_| {})
                .await
                .expect("the download succeeds");

        let game_dir = root.0.join("psx").join("Driver 2 (USA)");
        assert!(game_dir.join("disc1.cue").is_file());
        assert!(game_dir.join("disc2.bin").is_file());
        assert_eq!(
            installed.files,
            vec![
                "Driver 2 (USA)/disc1.cue",
                "Driver 2 (USA)/disc1.bin",
                "Driver 2 (USA)/disc2.cue",
                "Driver 2 (USA)/disc2.bin",
                "Driver 2 (USA)/Driver 2.m3u",
            ],
            "the rom's folder is a prefix on each file, not a deeper `dir`"
        );
        assert_eq!(installed.dir, root.0.join("psx"));
        // The playlist sits beside the discs and names them without the folder they share with it.
        assert_eq!(
            fs::read_to_string(game_dir.join("Driver 2.m3u")).unwrap(),
            "disc1.cue\ndisc2.cue\n"
        );
        // And nothing from the library root above the platform folder was rebuilt underneath it.
        assert!(!root.0.join("psx").join("roms").exists());

        // Removing it takes the files, the rom's folder and the platform folder it emptied.
        assert_eq!(delete(&installed).unwrap(), 5);
        assert!(!root.0.join("psx").exists());
        assert!(root.0.exists(), "but never the downloads folder itself");
    }

    #[tokio::test]
    async fn a_download_asked_for_before_romm_was_rebuilt_is_not_fetched_as_the_game_now_under_its_id() {
        let detail = serde_json::json!({
            "id": 7,
            "name": "Zipang",
            "fs_name": "Zipang (Japan).sfc",
            "fs_name_no_ext": "Zipang (Japan)",
            "created_at": "2026-10-12T19:00:00+00:00",
            "platform_slug": "snes",
            "files": [{ "id": 70, "file_name": "Zipang (Japan).sfc", "file_size_bytes": 11, "file_path": "roms/snes" }]
        })
        .to_string();
        let (host, requests) = stub_server::start_recording(vec![("GET /api/roms/7", 200, detail)]).await;

        let root = Root::new("rebuilt-queue");
        let asked = QueueItem {
            stamp: Some(Stamp { stem: "Secret of Mana (USA)".into(), born: Some("2026-08-01T22:26:48+00:00".into()) }),
            ..item(7, "snes", "Secret of Mana")
        };
        let error = fetch(&RommClient::new().unwrap(), &connection(&host), &root.0, &asked, None, |_| {})
            .await
            .expect_err("another game is under that id now");

        assert!(error.contains("\"Zipang\""), "{error}");
        assert!(!requests.lock().unwrap().iter().any(|r| r.contains("/content/")), "nothing of Zipang's was fetched");
    }

    #[tokio::test]
    async fn a_build_with_no_file_on_the_server_says_so_rather_than_failing_obscurely() {
        // RomM knows this build — often a sibling variant — but has no file uploaded for it, so
        // there is genuinely nothing to fetch and a retry cannot fix it.
        let detail = serde_json::json!({ "id": 9, "platform_slug": "snes", "files": [] }).to_string();
        let host = stub_server::start(vec![("GET /api/roms/9", 200, detail)]).await;

        let root = Root::new("no-files");
        let client = RommClient::new().unwrap();
        let error = fetch(&client, &connection(&host), &root.0, &item(9, "snes", "Ghost"), None, |_| {})
            .await
            .expect_err("there is nothing to download");

        assert!(error.contains("no file for this build"), "{error}");
        assert!(!root.0.join("snes").exists(), "and nothing was created for it");
    }

    #[tokio::test]
    async fn a_refused_file_leaves_no_partial_behind_and_names_the_file() {
        let detail = serde_json::json!({
            "id": 6,
            "fs_name": "Gone.sfc",
            "fs_path": "roms/snes",
            "platform_slug": "snes",
            "files": [{ "id": 60, "file_name": "Gone.sfc", "file_size_bytes": 9, "file_path": "roms/snes" }]
        })
        .to_string();
        let host = stub_server::start(vec![
            ("GET /api/roms/6/content/", 404, "{\"detail\":\"gone\"}".into()),
            ("GET /api/roms/6", 200, detail),
        ])
        .await;

        let root = Root::new("refused");
        let client = RommClient::new().unwrap();
        let error = fetch(&client, &connection(&host), &root.0, &item(6, "snes", "Gone"), None, |_| {})
            .await
            .expect_err("the server refused the file");

        assert!(error.starts_with("Gone.sfc:"), "the reason names the file: {error}");
        assert!(!root.0.join("snes").join("Gone.sfc").exists());
        assert!(
            !root.0.join("snes").join("Gone.sfc.part").exists(),
            "a partial of unknown age is worse than no file at all"
        );
    }

    #[tokio::test]
    async fn one_disc_of_a_set_is_selected_by_id_rather_than_by_name_alone() {
        // Without `file_ids` RomM zips *every* file of a multi-file rom and serves that, so asking
        // by name alone would download the whole set under one disc's name.
        let detail = serde_json::json!({
            "id": 5,
            "fs_name": "Set",
            "fs_path": "roms/psx",
            "platform_slug": "psx",
            "files": [{ "id": 55, "file_name": "a b.chd", "file_size_bytes": 1, "file_path": "roms/psx/Set" }]
        })
        .to_string();
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/roms/5/content/", 200, "x".into()),
            ("GET /api/roms/5", 200, detail),
        ])
        .await;

        let root = Root::new("file-ids");
        let client = RommClient::new().unwrap();
        fetch(&client, &connection(&host), &root.0, &item(5, "psx", "Set"), None, |_| {})
            .await
            .expect("the download succeeds");

        let asked = requests.lock().unwrap().clone();
        let content = asked.iter().find(|l| l.contains("/content/")).expect("a content request");
        assert!(content.contains("file_ids=55"), "{content}");
        // The name travels percent-encoded — it is what RomM knows this file as.
        assert!(content.contains("/content/a%20b.chd"), "{content}");
    }

    // --- Choosing, unpacking, updating ----------------------------------------------------------

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn rom_detail(id: i64, platform: &str, files: &[(i64, &str, usize)]) -> Vec<u8> {
        serde_json::json!({
            "id": id,
            "fs_name": files[0].1,
            "fs_path": format!("roms/{platform}"),
            "platform_slug": platform,
            "fs_size_bytes": files.iter().map(|f| f.2).sum::<usize>(),
            "md5_hash": "abc",
            "files": files.iter().map(|(fid, name, size)| serde_json::json!({
                "id": fid, "file_name": name, "file_size_bytes": size, "file_path": format!("roms/{platform}"), "category": "game"
            })).collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes()
    }

    // Unpacking runs beside the async runtime rather than on it, which wants a runtime with threads.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_zipped_rom_is_unpacked_where_it_landed_and_the_zip_goes() {
        let archive = zip_bytes(&[("Game (USA).cue", b"FILE \"Game (USA).bin\""), ("Game (USA).bin", &[3u8; 64])]);
        let host = stub_server::start_bytes(vec![
            ("GET /api/roms/30/content/", 200, archive.clone()),
            ("GET /api/roms/30", 200, rom_detail(30, "psx", &[(300, "Game (USA).zip", archive.len())])),
        ])
        .await;
        let root = Root::new("unzip");
        let mut phases = Vec::new();
        let installed = fetch(&RommClient::new().unwrap(), &connection(&host), &root.0, &item(30, "psx", "Game"), None, |p| phases.push(p))
            .await
            .expect("downloads and unpacks");
        let mut files = installed.files.clone();
        files.sort();
        assert_eq!(files, ["Game (USA).bin", "Game (USA).cue"]);
        assert!(!root.0.join("psx/Game (USA).zip").exists(), "the archive is gone once unpacked");
        assert_eq!(installed.bytes, 64 + 21);
        assert!(phases.iter().any(|p| matches!(p, Phase::Unpacking(_))));
        assert_eq!(crate::launch::target(&installed), root.0.join("psx/Game (USA).cue"), "and it launches from what was inside");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_arcade_set_stays_zipped() {
        let archive = zip_bytes(&[("rom.bin", b"set")]);
        let host = stub_server::start_bytes(vec![
            ("GET /api/roms/31/content/", 200, archive.clone()),
            ("GET /api/roms/31", 200, rom_detail(31, "arcade", &[(310, "pacman.zip", archive.len())])),
        ])
        .await;
        let root = Root::new("arcade");
        let installed = fetch(&RommClient::new().unwrap(), &connection(&host), &root.0, &item(31, "arcade", "Pac-Man"), None, |_| {})
            .await
            .unwrap();
        assert_eq!(installed.files, ["pacman.zip"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_archive_it_cannot_read_is_kept_as_it_came() {
        let archive = b"PK\x03\x04 and then nothing a zip would have".to_vec();
        let host = stub_server::start_bytes(vec![
            ("GET /api/roms/32/content/", 200, archive.clone()),
            ("GET /api/roms/32", 200, rom_detail(32, "psx", &[(320, "Game.zip", archive.len())])),
        ])
        .await;
        let root = Root::new("unreadable");
        let installed = fetch(&RommClient::new().unwrap(), &connection(&host), &root.0, &item(32, "psx", "Game"), None, |_| {})
            .await
            .expect("the download stands, archived");
        assert_eq!(installed.files, ["Game.zip"]);
        assert_eq!(fs::read(root.0.join("psx/Game.zip")).unwrap(), archive, "as RomM sent it");
    }

    #[tokio::test]
    async fn one_disc_then_the_other_makes_the_whole_set_with_its_playlist() {
        let host = stub_server::start_bytes(vec![
            ("GET /api/roms/40/content/", 200, b"disc".to_vec()),
            ("GET /api/roms/40", 200, rom_detail(40, "psx", &[(401, "FF (Disc 1).chd", 4), (402, "FF (Disc 2).chd", 4)])),
        ])
        .await;
        let root = Root::new("discs");
        let client = RommClient::new().unwrap();
        let one = QueueItem { files: Some(vec![401]), label: Some("Disc 1".into()), ..item(40, "psx", "FF") };
        let first = fetch(&client, &connection(&host), &root.0, &one, None, |_| {}).await.unwrap();
        assert_eq!(first.files, ["FF (Disc 1).chd"], "only the disc picked, and no playlist for one disc");
        assert_eq!((first.subset.as_deref(), first.name()), (Some("Disc 1"), "FF — Disc 1".to_string()));

        let two = QueueItem { files: Some(vec![402]), label: Some("Disc 2".into()), ..item(40, "psx", "FF") };
        let both = fetch(&client, &connection(&host), &root.0, &two, Some(first), |_| {}).await.unwrap();
        assert_eq!(both.files, ["FF (Disc 1).chd", "FF (Disc 2).chd", "FF.m3u"], "the set, and its playlist over both");
        assert_eq!(both.subset, None, "the whole game is here now");
        assert_eq!(both.source_files, [401, 402]);
        assert_eq!(fs::read_to_string(root.0.join("psx/FF.m3u")).unwrap(), "FF (Disc 1).chd\nFF (Disc 2).chd\n");
    }

}

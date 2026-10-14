//! Saves synced as they change, not only around a launch: a game played from ES-DE without the hooks,
//! a save copied in by hand, RetroArch writing its save while the window sits open.
//!
//! The window looks over RetroArch's save and state folders every few seconds — a walk of a few
//! hundred small files, off the UI thread — and a file that changed is synced once it has stopped
//! changing: three seconds for a battery save, which is written in one go, fifteen for anything a
//! megabyte or over, since a console writing a memory card goes quiet mid-write for longer than
//! three. The sync is the one *Sync now* runs, so RomM's sync session decides what moves.
//!
//! A sync's own writes are not news: once one finishes, the files it says it wrote become the new
//! baseline rather than a change to sync again — and only those, so a save a game wrote while the
//! sync was running is still synced rather than swallowed. Nothing is watched while a game started from the window
//! runs — its launch syncs when it ends — and only one Deckhand on a device watches: the first takes
//! `watch.lock` and holds it, and two processes syncing the same files would race each other's
//! uploads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often the folders are looked over.
pub const EVERY: Duration = Duration::from_secs(4);
const SETTLE: Duration = Duration::from_secs(3);
const LARGE: u64 = 1024 * 1024;
const LARGE_SETTLE: Duration = Duration::from_secs(15);
/// The most files one look counts, so a save folder pointed somewhere huge stays cheap.
const MOST: usize = 20_000;

/// Every file under some folders, by when it was written and how big it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot(HashMap<PathBuf, (Option<SystemTime>, u64)>);

/// Look over `folders`, three levels down — enough for RetroArch's by-core and by-content sorting.
pub fn snapshot(folders: &[PathBuf]) -> Snapshot {
    let mut files = HashMap::new();
    for folder in folders {
        walk(folder, 3, &mut files);
    }
    Snapshot(files)
}

fn walk(dir: &Path, depth: usize, files: &mut HashMap<PathBuf, (Option<SystemTime>, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if files.len() >= MOST {
            return;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if depth > 0 {
                walk(&entry.path(), depth - 1, files);
            }
        } else {
            files.insert(entry.path(), (meta.modified().ok(), meta.len()));
        }
    }
}

/// What the watcher has seen, and what it is waiting on to settle.
#[derive(Debug, Default)]
pub struct Watcher {
    last: Option<Snapshot>,
    /// Changed files not yet settled: how big each was, and when it last changed.
    pending: HashMap<PathBuf, (u64, Instant)>,
    /// The next look is a new baseline rather than news — a sync has just written, and what it
    /// wrote is not known.
    rebase: bool,
    /// Files a sync wrote: whatever is in them at the next look is where they stand, not news.
    written: std::collections::HashSet<PathBuf>,
}

impl Watcher {
    /// Take a new look, taken at `now`. Answers the files whose change has settled — a sync of
    /// their games is due — or none.
    pub fn observe(&mut self, seen: Snapshot, now: Instant) -> Vec<PathBuf> {
        let Some(mut last) = self.last.take().filter(|_| !self.rebase) else {
            self.rebase = false;
            self.written.clear();
            self.pending.clear();
            self.last = Some(seen);
            return Vec::new();
        };
        // What a sync wrote is where those files stand now, not a change to sync again — and only
        // those: a save a game wrote while the sync ran is still news, where taking the whole look
        // as a new baseline would have swallowed it along with the sync's own writes.
        for path in std::mem::take(&mut self.written) {
            match seen.0.get(&path) {
                Some(stamp) => last.0.insert(path.clone(), *stamp),
                None => last.0.remove(&path),
            };
            self.pending.remove(&path);
        }
        for (path, stamp) in &seen.0 {
            if last.0.get(path) != Some(stamp) {
                self.pending.insert(path.clone(), (stamp.1, now));
            }
        }
        // Gone before it settled: nothing to wait for.
        self.pending.retain(|path, _| seen.0.contains_key(path));
        let settled: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, (size, since))| now.duration_since(*since) >= if *size >= LARGE { LARGE_SETTLE } else { SETTLE })
            .map(|(path, _)| path.clone())
            .collect();
        for path in &settled {
            self.pending.remove(path);
        }
        self.last = Some(seen);
        settled
    }

    /// What is on disk after the next look is where things stand, not a change: a sync just wrote,
    /// and what it wrote is not known — a sync that failed, or one this app did not watch happen.
    pub fn rebase(&mut self) {
        self.rebase = true;
    }

    /// [`Watcher::rebase`] for the files a sync says it wrote ([`crate::sync::Report::written`]).
    /// Everything else keeps the baseline it had, so a save written while the sync ran is not taken
    /// for one of its own writes.
    pub fn wrote(&mut self, paths: &[PathBuf]) {
        self.written.extend(paths.iter().cloned());
    }
}

/// Hold `watch.lock` in `store` for as long as the answer lives — None when another Deckhand here
/// holds it already. A filesystem that cannot lock lets this one watch.
pub fn claim(store: &Path) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(store);
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(store.join("watch.lock")).ok()?;
    match file.try_lock() {
        Ok(()) | Err(std::fs::TryLockError::Error(_)) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(files: &[(&str, u64, u64)]) -> Snapshot {
        Snapshot(
            files
                .iter()
                .map(|(name, secs, size)| (PathBuf::from(name), (Some(SystemTime::UNIX_EPOCH + Duration::from_secs(*secs)), *size)))
                .collect(),
        )
    }

    #[test]
    fn a_change_is_synced_once_it_has_settled_and_not_before() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut watcher = Watcher::default();
        assert!(watcher.observe(snap(&[("a.srm", 1, 8)]), at(0)).is_empty(), "the first look is the baseline");
        assert!(watcher.observe(snap(&[("a.srm", 2, 8)]), at(4)).is_empty(), "changed — waiting for it to settle");
        assert!(watcher.observe(snap(&[("a.srm", 3, 8)]), at(6)).is_empty(), "changed again: the wait starts over");
        assert_eq!(watcher.observe(snap(&[("a.srm", 3, 8)]), at(10)), [PathBuf::from("a.srm")], "quiet for three seconds: due");
        assert!(watcher.observe(snap(&[("a.srm", 3, 8)]), at(14)).is_empty(), "and only once");

        // A memory card waits longer.
        assert!(watcher.observe(snap(&[("a.srm", 3, 8), ("card.ps2", 5, LARGE * 8)]), at(20)).is_empty());
        assert!(watcher.observe(snap(&[("a.srm", 3, 8), ("card.ps2", 5, LARGE * 8)]), at(26)).is_empty());
        assert_eq!(watcher.observe(snap(&[("a.srm", 3, 8), ("card.ps2", 5, LARGE * 8)]), at(36)), [PathBuf::from("card.ps2")]);
    }

    #[test]
    fn what_a_sync_wrote_is_the_new_baseline_and_what_it_did_not_is_still_news() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut watcher = Watcher::default();
        watcher.observe(snap(&[("pulled.srm", 1, 8), ("played.srm", 1, 8)]), at(0));

        // A sync took `pulled.srm` down; while it ran, a game started outside Deckhand wrote
        // `played.srm`. Only the first is the sync's own work.
        watcher.wrote(&[PathBuf::from("pulled.srm")]);
        let seen = snap(&[("pulled.srm", 9, 8), ("played.srm", 9, 8)]);
        assert!(watcher.observe(seen.clone(), at(4)).is_empty(), "waiting for the game's write to settle");
        assert_eq!(
            watcher.observe(seen, at(10)),
            [PathBuf::from("played.srm")],
            "the save the game wrote is synced; the one the sync wrote is not synced again"
        );
    }

    #[test]
    fn a_sync_s_own_writes_are_the_new_baseline_not_news() {
        let start = Instant::now();
        let mut watcher = Watcher::default();
        watcher.observe(snap(&[("a.srm", 1, 8)]), start);
        watcher.rebase();
        assert!(watcher.observe(snap(&[("a.srm", 9, 8), ("b.srm", 9, 8)]), start + Duration::from_secs(4)).is_empty());
        assert!(watcher.observe(snap(&[("a.srm", 9, 8), ("b.srm", 9, 8)]), start + Duration::from_secs(60)).is_empty(), "nothing pending from it");
    }

    #[test]
    fn folders_are_looked_over_and_only_one_deckhand_watches() {
        let dir = std::env::temp_dir().join(format!("deckhand-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("saves/snes9x")).unwrap();
        std::fs::write(dir.join("saves/snes9x/Chrono.srm"), "save").unwrap();
        let seen = snapshot(&[dir.join("saves"), dir.join("missing")]);
        assert_eq!(seen.0.len(), 1);
        assert!(seen.0.contains_key(&dir.join("saves/snes9x/Chrono.srm")));

        let first = claim(&dir).expect("the first takes it");
        assert!(claim(&dir).is_none(), "the second does not");
        drop(first);
        assert!(claim(&dir).is_some(), "free again once the first is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

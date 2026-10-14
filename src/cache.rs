//! The library on disk, so a second launch does not re-read the whole server.
//!
//! A first pairing has to walk every platform, and against a self-hosted RomM that is measured in
//! seconds per platform — thirty systems is a minute of staring at a progress line. Nothing about
//! that library changes between launches unless the collection itself changed, so it is written out
//! and read back: the app opens on Home immediately, and reconciles with the server behind it.
//!
//! What "changed" means here is two things. RomM reports a `rom_count` per platform, and a system
//! whose count moved is re-read: games added and removed. And RomM answers which roms it changed
//! since the library was last read (`read_at`): a rom edited in place — rescraped art, a corrected
//! name, a file swapped one-for-one — leaves the count where it was, and is caught by that instead.
//! Refresh still does the full walk.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::library::{Game, Library, System};
use crate::romm::models::{Collection, VirtualCollection};

/// Bumped whenever the shape below changes. A cache written by an older build is dropped rather
/// than migrated: it is derived data that costs one slow start to rebuild, and a library that
/// deserialises into the wrong shape is worse than no library at all.
///
/// 2: games carry `file_stems`, which save sync matches saves against. A version-1 cache would load
/// with every list empty and every save on disk unmatched until a Refresh, so it is dropped instead.
/// 3: games carry HowLongToBeat's time. The count-based sync only re-reads systems that changed, so
/// a version-2 cache would say N/A on every game until a Refresh.
/// 4: games hidden in RomM are left out, and games carry the status, backlog, build, other names,
/// achievement count and whether RomM still has the file. A version-3 cache would show hidden games.
/// 5: games carry their RetroAchievements id, which this user's earned achievements are matched by.
const VERSION: u32 = 5;

/// The whole library as it is written to disk.
#[derive(Debug, Serialize, Deserialize)]
pub struct Cached {
    version: u32,
    /// The server this was read from.
    ///
    /// Checked on load, because the cache is keyed by *file*, not by host: pointing the app at a
    /// second RomM must not open on the first one's library. A mismatch is a miss, not an error —
    /// the new server just gets the full walk it would have had anyway.
    host: String,
    pub systems: Vec<System>,
    pub games: Vec<Game>,
    pub collections: Vec<Collection>,
    /// `rom_count` per system slug, as the server reported it when this was written. The next
    /// start compares against this to decide which systems to re-read.
    pub counts: HashMap<String, i64>,
    /// When the library was read, for the next start to ask RomM what changed since. Absent from a
    /// cache written before it was kept, which costs only that one start's look.
    #[serde(default)]
    pub read_at: Option<String>,
}

impl Cached {
    /// The library, rebuilt with its indexes.
    pub fn library(&self) -> Library {
        Library::new(self.systems.clone(), self.games.clone())
    }
}

/// `$XDG_CACHE_HOME/deckhand-rust/library.json`.
///
/// The cache directory rather than the config one: this is derived from the server and can be
/// deleted at any time without touching the pairing.
pub fn path() -> PathBuf {
    Settings::cache_dir().join("library.json")
}

/// Read the cached library for `host`, or `None` when there is nothing usable.
///
/// Every failure is a miss: a missing file, a partial write, a cache from an older build, or one
/// belonging to a different server. None of those are worth reporting — the caller's fallback is
/// the full load, which is what would have happened without a cache at all.
pub fn load(host: &str) -> Option<Cached> {
    let text = fs::read_to_string(path()).ok()?;
    let cached: Cached = serde_json::from_str(&text).ok()?;
    (cached.version == VERSION && same_host(&cached.host, host)).then_some(cached)
}

/// Write the library out. Best effort: a cache that cannot be written costs a slow start next
/// time, which is not worth failing a working session over.
///
/// Written to a temp file and renamed, so a crash mid-write leaves the previous cache intact
/// rather than a truncated one that would deserialise into a half library.
pub fn save(host: &str, library: &Library, collections: &[Collection], counts: &HashMap<String, i64>, read_at: Option<&str>) {
    // Written from borrows: the shape [`Cached`] reads back, without a copy of every game to say it.
    #[derive(Serialize)]
    struct Writing<'a> {
        version: u32,
        host: &'a str,
        systems: &'a [System],
        games: &'a [Game],
        collections: &'a [Collection],
        counts: &'a HashMap<String, i64>,
        read_at: Option<&'a str>,
    }
    let cached = Writing { version: VERSION, host, systems: &library.systems, games: library.all_games(), collections, counts, read_at };
    let Ok(json) = serde_json::to_string(&cached) else { return };

    let path = path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    // Its own temp file per write: two refreshes close together write from two threads.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("json.{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Drop the cached library and collections — for signing out, where they belong to a pairing that
/// is gone.
pub fn clear() {
    let _ = fs::remove_file(path());
    let _ = fs::remove_file(sources_path());
}

/// Bumped whenever [`CachedSources`] changes shape; an older file is a miss, as the library's is.
const SOURCES_VERSION: u32 = 1;

/// RomM's smart and generated collections as it last answered.
///
/// Kept apart from the library because they arrive apart: RomM builds the generated ones — every
/// genre, franchise, series, company and mode, 900 of them and a megabyte on the reference server —
/// when they are asked for, which takes it a second or more, and the Collections page waited on that
/// at the first visit of every launch. Kept, the page opens on what RomM said last time, and the
/// answer to asking again replaces it.
#[derive(Debug, Serialize, Deserialize)]
struct CachedSources {
    version: u32,
    host: String,
    smart: Vec<Collection>,
    generated: Vec<VirtualCollection>,
}

/// `$XDG_CACHE_HOME/deckhand-rust/collections.json`, beside the library.
fn sources_path() -> PathBuf {
    Settings::cache_dir().join("collections.json")
}

/// The smart and generated collections RomM at `host` last gave, or `None` when there is nothing
/// usable — every failure a miss, as for the library.
pub fn load_sources(host: &str) -> Option<(Vec<Collection>, Vec<VirtualCollection>)> {
    let text = fs::read_to_string(sources_path()).ok()?;
    let cached: CachedSources = serde_json::from_str(&text).ok()?;
    (cached.version == SOURCES_VERSION && same_host(&cached.host, host)).then_some((cached.smart, cached.generated))
}

/// Keep RomM's answer for the next launch. Best effort, and written through a temp file, as the
/// library is.
pub fn save_sources(host: &str, smart: &[Collection], generated: &[VirtualCollection]) {
    #[derive(Serialize)]
    struct Writing<'a> {
        version: u32,
        host: &'a str,
        smart: &'a [Collection],
        generated: &'a [VirtualCollection],
    }
    let Ok(json) = serde_json::to_string(&Writing { version: SOURCES_VERSION, host, smart, generated }) else { return };
    let path = sources_path();
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("json.{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Two host strings naming the same server. Compared without a trailing slash because the login
/// screen keeps whatever the user typed, and `https://romm.lan` and `https://romm.lan/` are the
/// same server with two spellings — treating them as different would silently disable the cache
/// for anyone who typed the slash.
fn same_host(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Which systems the next load has to actually read.
///
/// `cached` is the stamp written with the library; `fresh` is slug -> `rom_count` from the server
/// now. A system is stale when its count moved or when the cache has never seen it.
///
/// Systems the server no longer has need no entry here: the sync rebuilds its system list from the
/// platforms the server just reported, so one that has gone is dropped by not being rebuilt.
pub fn stale(cached: &HashMap<String, i64>, fresh: &HashMap<String, i64>) -> Vec<String> {
    fresh
        .iter()
        .filter(|(slug, count)| cached.get(*slug) != Some(*count))
        .map(|(slug, _)| slug.clone())
        .collect()
}

/// Re-stamp a cached game's `last_played` from a freshly fetched played map.
///
/// The same rule [`Game::from_rom`] applies, and for the same reason: play history lives on
/// whichever rom was launched, so a game played through one of its sibling regional builds has to
/// light up the tile that stands for it. A cached game has no rom to consult, which is why it
/// carries [`Game::sibling_ids`].
///
/// A game the map does not mention keeps what it had, rather than being cleared: the map is empty
/// when RomM could not be asked, and a RomM too old to filter by play answers only its first page —
/// so absence from it never means "never played".
pub fn restamp(game: &mut Game, played: &HashMap<i64, String>) {
    let newest = std::iter::once(game.rom_id)
        .chain(game.sibling_ids.iter().copied())
        .filter_map(|id| played.get(&id).cloned())
        .max();
    if let Some(newest) = newest {
        game.last_played = Some(newest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(s, n)| ((*s).to_string(), *n)).collect()
    }

    #[test]
    fn the_collections_romm_gave_are_kept_for_the_next_launch_and_only_for_its_server() {
        let dir = std::env::temp_dir().join(format!("deckhand-sources-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        crate::config::TEST_CACHE.with(|cache| *cache.borrow_mut() = Some(dir.join("deckhand-rust")));

        assert!(load_sources("https://romm.lan").is_none(), "nothing kept yet");
        let generated = vec![VirtualCollection { id: "genre-rpg".into(), name: Some("RPG".into()), kind: "genre".into(), rom_ids: vec![1, 2] }];
        save_sources("https://romm.lan/", &[], &generated);
        let (smart, kept) = load_sources("https://romm.lan").expect("kept");
        assert!(smart.is_empty());
        assert_eq!(kept[0].rom_ids, [1, 2]);
        assert!(load_sources("https://other.lan").is_none(), "another server's are not this one's");

        clear();
        assert!(load_sources("https://romm.lan").is_none(), "signing out drops them with the library");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_library_that_has_not_moved_needs_no_fetching() {
        let before = counts(&[("snes", 120), ("psx", 40)]);
        assert!(stale(&before, &before.clone()).is_empty());
    }

    #[test]
    fn a_system_that_gained_or_lost_games_is_the_only_one_re_read() {
        let before = counts(&[("snes", 120), ("psx", 40), ("n64", 12)]);
        let after = counts(&[("snes", 121), ("psx", 40), ("n64", 9)]);
        let mut stale = stale(&before, &after);
        stale.sort();
        assert_eq!(stale, vec!["n64", "snes"], "psx did not move and must not be re-read");
    }

    #[test]
    fn a_system_the_cache_has_never_seen_is_read() {
        let before = counts(&[("snes", 120)]);
        let after = counts(&[("snes", 120), ("psp", 3)]);
        assert_eq!(stale(&before, &after), vec!["psp"]);
    }

    #[test]
    fn a_system_the_server_dropped_is_never_fetched() {
        // It is not in `fresh`, so it cannot be stale; the sync drops it by rebuilding its system
        // list from what the server reported rather than from the cache.
        let before = counts(&[("snes", 120), ("dos", 7)]);
        let after = counts(&[("snes", 120)]);
        assert!(stale(&before, &after).is_empty());
    }

    #[test]
    fn a_host_keeps_its_own_cache_whatever_the_trailing_slash() {
        assert!(same_host("https://romm.lan", "https://romm.lan/"));
        assert!(same_host("https://romm.lan/", "https://romm.lan"));
        assert!(!same_host("https://romm.lan", "https://other.lan"));
    }

    #[test]
    fn a_game_played_through_a_sibling_still_lights_up_its_tile() {
        let mut game = Game { rom_id: 1, sibling_ids: vec![2, 3], ..Default::default() };
        let played: HashMap<i64, String> = [(3, "2026-09-08".to_string())].into_iter().collect();
        restamp(&mut game, &played);
        assert_eq!(game.last_played.as_deref(), Some("2026-09-08"));
    }

    #[test]
    fn re_stamping_takes_the_newest_across_the_group() {
        let mut game = Game { rom_id: 1, sibling_ids: vec![2], ..Default::default() };
        let played: HashMap<i64, String> =
            [(1, "2026-01-01".to_string()), (2, "2026-09-08".to_string())].into_iter().collect();
        restamp(&mut game, &played);
        assert_eq!(game.last_played.as_deref(), Some("2026-09-08"));
    }

    #[test]
    fn a_game_missing_from_the_played_page_keeps_the_date_it_had() {
        // The played query returns a page of recent roms, not the whole history, so absence from
        // it says nothing — clearing on absence would empty the shelf for anyone with more played
        // games than fit one page.
        let mut game = Game {
            rom_id: 1,
            last_played: Some("2025-04-02".to_string()),
            ..Default::default()
        };
        restamp(&mut game, &HashMap::new());
        assert_eq!(game.last_played.as_deref(), Some("2025-04-02"));
    }

    #[test]
    fn a_saved_library_reads_back_whole() {
        let dir = crate::config::test_root().join("cache-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        crate::config::TEST_CACHE.with(|cache| *cache.borrow_mut() = Some(dir.clone()));
        let library = crate::fixtures::library(20);
        save("https://romm.local", &library, &[], &HashMap::from([("snes".to_string(), 5)]), Some("2026-10-12T09:00:00Z"));
        let back = load("https://romm.local/").expect("the cache this build writes, read back");
        assert_eq!(back.library().total_games, library.total_games);
        assert_eq!(back.library().all_games().iter().map(|g| g.rom_id).collect::<Vec<_>>(), library.all_games().iter().map(|g| g.rom_id).collect::<Vec<_>>());
        assert_eq!(back.counts.get("snes"), Some(&5));
        assert_eq!(back.read_at.as_deref(), Some("2026-10-12T09:00:00Z"), "what the next sync asks RomM for changes since");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
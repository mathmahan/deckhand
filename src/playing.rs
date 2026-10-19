//! A game being played, on disk, so its time played outlives the process that timed it.
//!
//! Every way a game starts writes a marker for it in `<store>/playing/` as it starts, and takes it
//! once the game has closed and RomM has the session: the window's Play and a game's own Steam
//! shortcut (both [`crate::launch::play`]), and ES-DE's hooks (`deckhand pre` and `post`). What the
//! marker is for is the session that never gets that far. Steam's *Exit game* ends Deckhand and the
//! emulator together, and so does a crash, a dead battery or a power cut.
//!
//! Three things make a marker worth trusting once whatever wrote it is gone:
//!
//! - **It is held.** Whatever is timing the session keeps the file locked: the window and `deckhand
//!   play` themselves, and for ES-DE `deckhand keep` ([`keep`]), which the game-start hook leaves
//!   running in RetroDECK's sandbox. The kernel lets go of a lock when its process dies, however it
//!   dies, so a marker nobody holds is a session nobody is timing any more.
//! - **It is touched.** Its holder sets its modified time every [`TICK`], and once more as the game
//!   closes, so a session whose holder died ended then — not whenever its marker is found.
//! - **It says it was kept.** A marker that was never held — a `pre` from a game's Steam launch
//!   options, with nothing alive between it and its `post` — cannot be told from a game still
//!   running, so only that `post` takes it, as it always has.
//!
//! A kept marker that nobody holds is a session RomM has not had: [`settle_left`] sends it from
//! whichever Deckhand starts next. A session RomM could not take as its game closed — no network,
//! say — is left the same way, on purpose.

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Connection;
use crate::romm::RommClient;
use crate::romm::models::PlaySession;

/// How often a held marker is touched.
pub const TICK: Duration = Duration::from_secs(30);

/// A kept marker untouched for this long has lost its holder, even if no other process can see the
/// lock go: a `post` reading it takes the last touch as the end.
const QUIET: Duration = Duration::from_secs(2 * TICK.as_secs());

/// A session longer than this is a marker left behind, not play.
const LONGEST: Duration = Duration::from_secs(24 * 3600);

/// How often [`keep`] looks for its marker being taken: the game has closed, and ES-DE may be about
/// to start another.
const LOOK: Duration = Duration::from_secs(1);

/// What a marker says. A build before these markers wrote the start alone, as seconds; that still
/// reads, as a marker never kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Note {
    /// When the game started, in seconds since the epoch.
    start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rom_id: Option<i64>,
    /// Whether anything has ever held it — see the module's header.
    #[serde(default)]
    kept: bool,
}

impl Note {
    fn read(text: &str) -> Option<Self> {
        let text = text.trim();
        serde_json::from_str(text)
            .ok()
            .or_else(|| text.parse().ok().map(|start| Note { start, rom_id: None, kept: false }))
    }

    fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// `rom`'s marker: one per game, named by its path.
fn path(store: &Path, rom: &Path) -> PathBuf {
    store.join("playing").join(format!("{}.start", crate::sync::hash::file(rom.to_string_lossy().as_bytes())))
}

fn secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// The play session from `start` to `end`, if it was one: long enough to be play (see
/// [`crate::launch::MIN_SESSION`]) and not so long it can only be a marker left behind.
fn session(rom_id: i64, start: SystemTime, end: SystemTime) -> Option<PlaySession> {
    (end.duration_since(start).ok()? <= LONGEST).then(|| crate::launch::play_session(rom_id, start, end)).flatten()
}

/// Whether `path` still names the file `file` has open — not one a later start of the game put there.
fn names(file: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), fs::metadata(path)) {
            (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        path.exists()
    }
}

/// A marker this process holds while its game runs — see [`hold`].
#[derive(Debug)]
pub struct Held {
    file: File,
    path: PathBuf,
    start: SystemTime,
}

/// Note that `rom` starts now, held by this process until [`Held::end`]. Dropped without that, the
/// marker stays for [`settle_left`]. None when it cannot be written: the game still plays, its time
/// just has nothing behind it.
pub fn hold(store: &Path, rom: &Path, rom_id: i64) -> Option<Held> {
    let path = path(store, rom);
    let dir = path.parent()?;
    fs::create_dir_all(dir).ok()?;
    // Locked before it has a name, so no other Deckhand ever sees it unheld.
    let staging = dir.join(format!(".{}.{}", std::process::id(), path.file_name()?.to_string_lossy()));
    let mut file = OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&staging).ok()?;
    match file.try_lock() {
        Ok(()) | Err(fs::TryLockError::Error(_)) => {}
        Err(fs::TryLockError::WouldBlock) => return None,
    }
    let start = SystemTime::now();
    let written = file.write_all(&Note { start: secs(start), rom_id: Some(rom_id), kept: true }.bytes());
    if written.and_then(|()| fs::rename(&staging, &path)).is_err() {
        let _ = fs::remove_file(&staging);
        return None;
    }
    Some(Held { file, path, start })
}

impl Held {
    pub fn start(&self) -> SystemTime {
        self.start
    }

    /// Still playing, as of now.
    pub fn touch(&self) {
        let _ = self.file.set_modified(SystemTime::now());
    }

    /// The session is done with — RomM has it, or there was none to have. The marker goes, unless a
    /// later start of the game has put its own in its place.
    pub fn end(self) {
        if names(&self.file, &self.path) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Note that `rom` starts now with nothing to hold the marker: a `pre`, whose `post` takes it. ES-DE's
/// hook then starts [`keep`] to hold it.
pub fn start(store: &Path, rom: &Path, rom_id: Option<i64>) {
    let note = Note { start: secs(SystemTime::now()), rom_id, kept: false };
    let _ = crate::sync::write_atomic(&path(store, rom), &note.bytes());
}

/// A session's marker as a `post` took it.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    pub rom_id: Option<i64>,
    pub start: SystemTime,
    pub end: SystemTime,
}

impl Taken {
    /// The play session it was, under `rom_id` when the marker does not say.
    pub fn session(&self, rom_id: Option<i64>) -> Option<PlaySession> {
        session(self.rom_id.or(rom_id)?, self.start, self.end)
    }
}

/// The session `rom`'s marker holds, ended now — taken, so a second `post` finds none, and so a
/// [`keep`] holding it lets go. A kept marker that has gone quiet lost its holder when it did, so its
/// session ended at its last touch.
pub fn take(store: &Path, rom: &Path) -> Option<Taken> {
    let path = path(store, rom);
    let note = Note::read(&fs::read_to_string(&path).ok()?);
    let touched = fs::metadata(&path).and_then(|m| m.modified()).ok();
    let _ = fs::remove_file(&path);
    let note = note?;
    let now = SystemTime::now();
    let end = match touched {
        Some(touched) if note.kept && now.duration_since(touched).is_ok_and(|quiet| quiet > QUIET) => touched,
        _ => now,
    };
    Some(Taken { rom_id: note.rom_id, start: at(note.start), end })
}

/// Put a taken session back, for [`settle_left`] to send: RomM could not have it when its game closed.
/// A start of the game since has its own marker there, and wins.
pub fn owe(store: &Path, rom: &Path, taken: &Taken, rom_id: Option<i64>) {
    let path = path(store, rom);
    let Some(rom_id) = taken.rom_id.or(rom_id) else { return };
    if path.exists() {
        return;
    }
    let note = Note { start: secs(taken.start), rom_id: Some(rom_id), kept: true };
    if crate::sync::write_atomic(&path, &note.bytes()).is_ok()
        && let Ok(file) = OpenOptions::new().write(true).open(&path)
    {
        let _ = file.set_modified(taken.end);
    }
}

/// Hold `rom`'s marker, touching it, until its `post` takes it — what `deckhand keep` does, left
/// running by ES-DE's game-start hook. Returns at once when there is no marker, or something else
/// holds it.
///
/// Its life is RetroDECK's: started inside the sandbox, it goes when the sandbox does, which is when
/// ES-DE goes — quit, or ended by Steam with the game. A game that closes as games do runs `post`,
/// which takes the marker, and this notices within [`LOOK`].
pub fn keep(store: &Path, rom: &Path) {
    let path = path(store, rom);
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&path) else { return };
    if file.try_lock().is_err() {
        return;
    }
    let Some(mut note) = fs::read_to_string(&path).ok().and_then(|text| Note::read(&text)) else { return };
    note.kept = true;
    // Rewritten through the held file, so the lock stays on the file the path names.
    if file.set_len(0).and_then(|()| file.rewind()).and_then(|()| file.write_all(&note.bytes())).is_err() {
        return;
    }
    let born = Instant::now();
    let mut touched = Instant::now();
    let _ = file.set_modified(SystemTime::now());
    while names(&file, &path) && born.elapsed() < LONGEST {
        std::thread::sleep(LOOK);
        if touched.elapsed() >= TICK {
            let _ = file.set_modified(SystemTime::now());
            touched = Instant::now();
        }
    }
}

/// Send RomM every session whose marker was kept and that nobody holds any more, and let those
/// markers go. A marker RomM does not take stays for the next time; one that is not a session — too
/// short to be play, or too long to be anything but left behind — just goes. Answers how many went.
pub async fn settle_left(client: &RommClient, conn: &Connection, store: &Path) -> usize {
    let Ok(entries) = fs::read_dir(store.join("playing")) else { return 0 };
    let mut sent = 0;
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|e| e != "start") || path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else { continue };
        // Held: a game is running on it. Unlockable: nothing here can tell, so it is left alone.
        if file.try_lock().is_err() {
            continue;
        }
        let Some(note) = fs::read_to_string(&path).ok().and_then(|text| Note::read(&text)) else { continue };
        if !note.kept {
            continue;
        }
        let Ok(end) = file.metadata().and_then(|m| m.modified()) else { continue };
        let play = note.rom_id.and_then(|rom_id| session(rom_id, at(note.start), end));
        if let Some(play) = &play {
            if client.post_play_session(conn, play).await.is_err() {
                continue;
            }
            sent += 1;
        }
        // Still holding it, so no other Deckhand sends it too.
        if names(&file, &path) {
            let _ = fs::remove_file(&path);
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Store(PathBuf);

    impl Store {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-playing-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn marker(&self) -> PathBuf {
            path(&self.0, Path::new(ROM))
        }

        /// A marker as one left by a session that started `ago` and was last touched `quiet` ago.
        fn left(&self, kept: bool, ago: u64, quiet: u64) {
            let now = SystemTime::now();
            let note = Note { start: secs(now - Duration::from_secs(ago)), rom_id: Some(7), kept };
            crate::sync::write_atomic(&self.marker(), &note.bytes()).unwrap();
            OpenOptions::new().write(true).open(self.marker()).unwrap().set_modified(now - Duration::from_secs(quiet)).unwrap();
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ROM: &str = "/games/snes/Some Game (USA).sfc";

    fn connection(host: String) -> Connection {
        Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None }
    }

    fn sessions(requests: &crate::stub_server::Requests) -> usize {
        requests.lock().unwrap().iter().filter(|r| r.starts_with("POST /api/play-sessions")).count()
    }

    #[tokio::test]
    async fn a_held_marker_is_a_game_still_running_and_is_sent_once_its_holder_is_gone() {
        let store = Store::new("held");
        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/play-sessions", 200, "{}".into())]).await;
        let (client, conn) = (RommClient::new().unwrap(), connection(host));

        let held = hold(&store.0, Path::new(ROM), 7).unwrap();
        assert!(fs::read_dir(store.0.join("playing")).unwrap().count() == 1, "nothing staged is left beside it");
        assert_eq!(settle_left(&client, &conn, &store.0).await, 0);
        assert!(store.marker().is_file(), "a game still running keeps its marker");

        // Played ten minutes, then ended with Deckhand: last touched as it went.
        held.file.set_modified(held.start + Duration::from_secs(600)).unwrap();
        drop(held);
        assert_eq!(settle_left(&client, &conn, &store.0).await, 1);
        assert!(!store.marker().exists());
        assert_eq!(sessions(&requests), 1);
        assert_eq!(settle_left(&client, &conn, &store.0).await, 0, "sent once");
    }

    #[tokio::test]
    async fn a_session_romm_cannot_take_waits_for_the_next_time_and_one_that_is_not_play_just_goes() {
        let store = Store::new("refused");
        let refusing = crate::stub_server::start(vec![("POST /api/play-sessions", 500, "{}".into())]).await;
        let client = RommClient::new().unwrap();

        store.left(true, 900, 300);
        assert_eq!(settle_left(&client, &connection(refusing), &store.0).await, 0);
        assert!(store.marker().is_file(), "kept for when RomM can take it");

        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/play-sessions", 200, "{}".into())]).await;
        store.left(true, 30, 25);
        assert_eq!(settle_left(&client, &connection(host.clone()), &store.0).await, 0);
        assert!(!store.marker().exists(), "five seconds is a game that never got going");
        store.left(true, 3 * 24 * 3600, 24 * 3600);
        assert_eq!(settle_left(&client, &connection(host), &store.0).await, 0);
        assert!(!store.marker().exists(), "a day long is a marker left behind");
        assert_eq!(sessions(&requests), 0);
    }

    #[tokio::test]
    async fn a_marker_nothing_ever_held_is_left_for_its_own_post() {
        let store = Store::new("plain");
        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/play-sessions", 200, "{}".into())]).await;
        store.left(false, 900, 900);
        assert_eq!(settle_left(&RommClient::new().unwrap(), &connection(host), &store.0).await, 0);
        assert!(store.marker().is_file(), "it may be a game launched through Steam's launch options, still running");
        assert_eq!(sessions(&requests), 0);
    }

    #[test]
    fn a_post_ends_the_session_now_unless_its_keeper_went_quiet_before() {
        let store = Store::new("take");
        let rom = Path::new(ROM);

        start(&store.0, rom, Some(7));
        let taken = take(&store.0, rom).unwrap();
        assert!(SystemTime::now().duration_since(taken.end).unwrap() < Duration::from_secs(5));
        assert_eq!(taken.rom_id, Some(7));
        assert_eq!(take(&store.0, rom), None, "taken, so a second post finds none");

        // ES-DE ended mid-game an hour after it began; this post is for a later start.
        store.left(true, 7200, 3600);
        let taken = take(&store.0, rom).unwrap();
        let played = taken.end.duration_since(taken.start).unwrap().as_secs();
        assert!((3595..=3605).contains(&played), "{played}");

        // A marker never kept has nothing to go quiet: its post is the end, as before.
        store.left(false, 7200, 7200);
        let taken = take(&store.0, rom).unwrap();
        assert!(SystemTime::now().duration_since(taken.end).unwrap() < Duration::from_secs(5));
    }

    #[test]
    fn a_marker_from_before_these_reads_as_its_start() {
        let store = Store::new("legacy");
        crate::sync::write_atomic(&store.marker(), b"1782900000").unwrap();
        let taken = take(&store.0, Path::new(ROM)).unwrap();
        assert_eq!((taken.start, taken.rom_id), (at(1_782_900_000), None));
        assert!(taken.session(Some(7)).is_none(), "and, ended a day later, is no session");
    }

    #[test]
    fn ending_a_session_leaves_a_later_start_of_the_game_alone() {
        let store = Store::new("end");
        let rom = Path::new(ROM);
        hold(&store.0, rom, 7).unwrap().end();
        assert!(!store.marker().exists());

        let first = hold(&store.0, rom, 7).unwrap();
        start(&store.0, rom, Some(7));
        first.end();
        assert!(store.marker().is_file());
    }

    #[test]
    fn an_owed_session_goes_back_for_the_next_time_unless_the_game_started_again() {
        let store = Store::new("owe");
        let rom = Path::new(ROM);
        let now = SystemTime::now();
        let taken = Taken { rom_id: None, start: now - Duration::from_secs(900), end: now - Duration::from_secs(60) };

        owe(&store.0, rom, &taken, Some(7));
        let note = Note::read(&fs::read_to_string(store.marker()).unwrap()).unwrap();
        assert_eq!((note.rom_id, note.kept), (Some(7), true));
        let touched = fs::metadata(store.marker()).unwrap().modified().unwrap();
        assert_eq!(secs(touched), secs(taken.end), "it ended when the game closed, not when it was put back");

        start(&store.0, rom, Some(7));
        owe(&store.0, rom, &taken, Some(7));
        assert!(!Note::read(&fs::read_to_string(store.marker()).unwrap()).unwrap().kept);
    }

    #[test]
    fn a_keeper_holds_the_marker_until_its_post_takes_it() {
        let store = Store::new("keep");
        let rom = Path::new(ROM);
        start(&store.0, rom, Some(7));
        let keeper = {
            let store = store.0.clone();
            std::thread::spawn(move || keep(&store, Path::new(ROM)))
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let held = || OpenOptions::new().read(true).open(store.marker()).is_ok_and(|f| f.try_lock().is_err());
        while !held() {
            assert!(Instant::now() < deadline, "the keeper never took hold");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(Note::read(&fs::read_to_string(store.marker()).unwrap()).unwrap().kept);

        // A second keeper finds it held, and leaves.
        keep(&store.0, rom);

        assert!(take(&store.0, rom).is_some());
        let deadline = Instant::now() + 2 * LOOK + Duration::from_secs(1);
        while !keeper.is_finished() {
            assert!(Instant::now() < deadline, "the keeper outlived its marker");
            std::thread::sleep(Duration::from_millis(20));
        }
        keep(&store.0, rom);
    }
}

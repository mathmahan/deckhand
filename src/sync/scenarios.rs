//! The save sync end to end: devices playing and syncing against [`crate::fake_romm`], a RomM that
//! keeps state and decides the way RomM 5.4 was seen to.
//!
//! Each test is a story told in launches — a Deck and a PC, progress made on one and taken up on the
//! other, offline play on both, a clock that is wrong, a save deleted on the server — with what must
//! be true after each step: what moved, what RomM now holds, and that no progress was lost.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::session::{self, AUTOSAVE, Outcome, Place, Thread};
use super::{Choice, Conflict, Report, Resolver, retroarch, sync_rom, with_resolver};
use crate::config::Connection;
use crate::fake_romm::FakeRomm;
use crate::romm::RommClient;
use crate::romm::models::PlaySession;

const GAME: &str = "Chrono Trigger (USA)";
const ROM_ID: i64 = 1;

/// One device: a flat RetroArch install with the game on it, and the app's own store.
struct Device {
    root: PathBuf,
    conn: Connection,
}

impl Device {
    fn new(name: &str, server: &FakeRomm) -> Self {
        let root = std::env::temp_dir().join(format!("deckhand-scenario-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ra/saves")).unwrap();
        fs::create_dir_all(root.join("roms/snes")).unwrap();
        fs::write(root.join("ra/retroarch.cfg"), "savefile_directory = \":/saves\"\n").unwrap();
        fs::write(root.join("roms/snes").join(format!("{GAME}.sfc")), "rom").unwrap();
        let device = format!("device-{name}");
        server.device(&device);
        let conn = Connection {
            host: server.host.clone(),
            token: "rmm_test".into(),
            insecure: false,
            token_id: None,
            token_name: None,
            device_id: Some(device),
        };
        Self { root, conn }
    }

    fn id(&self) -> &str {
        self.conn.device_id.as_deref().unwrap()
    }

    fn save_path(&self) -> PathBuf {
        self.root.join("ra/saves").join(format!("{GAME}.srm"))
    }

    /// Play: the game writes `progress` at `at`, by this device's clock.
    fn play(&self, progress: &str, at: SystemTime) {
        fs::write(self.save_path(), progress).unwrap();
        fs::File::options().write(true).open(self.save_path()).unwrap().set_modified(at).unwrap();
    }

    fn save(&self) -> Option<String> {
        fs::read_to_string(self.save_path()).ok()
    }

    fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    fn backups(&self) -> Vec<String> {
        let mut kept: Vec<String> = fs::read_dir(self.store().join("save-backups"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| fs::read_to_string(e.path()).unwrap())
            .collect();
        kept.sort();
        kept
    }

    /// The sync around a launch of the game, as the app runs it.
    async fn launch(&self) -> Report {
        self.launch_with(Vec::new()).await
    }

    async fn launch_with(&self, plays: Vec<PlaySession>) -> Report {
        let install = retroarch::read(&self.root.join("ra/retroarch.cfg")).unwrap();
        let rom = self.root.join("roms/snes").join(format!("{GAME}.sfc"));
        sync_rom(&RommClient::new().unwrap(), &self.conn, &install, &self.store(), &rom, "snes", Some("snes9x"), ROM_ID, None, plays)
            .await
            .unwrap()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A conflict chooser that always answers `choice`, and remembers what it was asked.
fn chooser(choice: Choice) -> (Resolver, std::sync::Arc<std::sync::Mutex<Vec<Conflict>>>) {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asked.clone();
    let resolver: Resolver = std::sync::Arc::new(move |conflict: Conflict| {
        seen.lock().unwrap().push(conflict);
        Box::pin(async move { choice })
    });
    (resolver, asked)
}

#[tokio::test]
async fn a_save_goes_up_once_and_then_nothing_moves() {
    let server = FakeRomm::start().await;
    let deck = Device::new("once", &server);
    deck.play("progress", server.now());

    let report = deck.launch().await;
    assert_eq!(report.pushed, [format!("{GAME}.srm")], "{report:?}");
    let row = server.newest(ROM_ID, AUTOSAVE).expect("pushed into the autosave slot");
    assert_eq!((row.bytes.as_slice(), row.origin.as_deref()), (b"progress".as_slice(), Some(deck.id())));
    assert!(server.holds(deck.id(), row.id), "RomM records the pushing device as holding it");

    let report = deck.launch().await;
    assert_eq!((report.in_sync, report.pushed.len(), report.pulled.len()), (1, 0, 0), "{report:?}");
    assert_eq!(server.requests("POST /api/saves?"), 1, "the second sync sends nothing");
    assert_eq!(server.requests("POST /api/saves/"), 0, "and acknowledges nothing it had already");
    assert_eq!(server.open_sessions(), 0, "every session is closed");
}

#[tokio::test]
async fn progress_hops_between_devices_without_a_conflict() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("hop-deck", &server), Device::new("hop-pc", &server));

    deck.play("chapter 1", server.now());
    deck.launch().await;

    // A device that has never had the save takes RomM's on its first launch, and says so.
    let report = pc.launch().await;
    assert_eq!(report.pulled, [format!("{GAME}.srm")], "{report:?}");
    assert_eq!(pc.save().as_deref(), Some("chapter 1"));
    let first = server.newest(ROM_ID, AUTOSAVE).unwrap();
    assert!(server.holds(pc.id(), first.id), "acknowledged once the bytes were in place");

    // Played there, then back here: each side moved alone, so each sync is a plain transfer.
    server.advance(Duration::from_secs(600));
    pc.play("chapter 2", server.now());
    let report = pc.launch().await;
    assert_eq!(report.pushed.len(), 1, "not refused: the PC holds the version it pushes over — {report:?}");
    let report = deck.launch().await;
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert!(report.conflicts.is_empty(), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 2"));
    assert!(deck.backups().is_empty(), "nothing here had progress to keep");
}

#[tokio::test]
async fn offline_play_on_two_devices_is_put_to_the_player() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("offline-deck", &server), Device::new("offline-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;
    pc.launch().await;

    // Both play from chapter 1; the PC syncs first.
    server.advance(Duration::from_secs(600));
    deck.play("deck: chapter 2", server.now());
    server.advance(Duration::from_secs(600));
    pc.play("pc: chapter 3", server.now());
    pc.launch().await;

    // RomM 5.3 compares the Deck's file against a version it never synced, by clock alone — and the
    // PC's is newer, so RomM says download. That would overwrite the Deck's chapter 2.
    let (resolver, asked) = chooser(Choice::Local);
    let report = with_resolver(resolver, deck.launch()).await;
    assert_eq!(server.answered().last().map(String::as_str), Some("download"), "what RomM said");
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "the player is asked: {report:?}");
    assert_eq!(asked[0].newer, Choice::Remote, "the PC's is newer, and still the player's call");
    assert!(report.conflicts[0].contains("kept this device's"), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("deck: chapter 2"));

    // Kept here, and pushed past RomM's refusal on the player's say-so — the PC's stays in history.
    let rows: Vec<Vec<u8>> = server.rows(ROM_ID).into_iter().map(|r| r.bytes).collect();
    assert_eq!(rows, [b"chapter 1".to_vec(), b"pc: chapter 3".to_vec(), b"deck: chapter 2".to_vec()]);
}

#[tokio::test]
async fn with_nobody_to_ask_the_newer_side_wins_and_the_loser_is_kept() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("newer-deck", &server), Device::new("newer-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;
    pc.launch().await;

    server.advance(Duration::from_secs(600));
    deck.play("deck: chapter 2", server.now());
    server.advance(Duration::from_secs(600));
    pc.play("pc: chapter 3", server.now());
    pc.launch().await;

    let report = deck.launch().await;
    assert!(report.conflicts[0].contains("kept RomM's"), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("pc: chapter 3"));
    assert_eq!(deck.backups(), ["deck: chapter 2"], "the Deck's progress is set aside, not lost");
}

#[tokio::test]
async fn a_change_still_goes_up_when_this_device_s_clock_is_behind() {
    let server = FakeRomm::start().await;
    let deck = Device::new("skew", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    // An hour behind RomM's clock: the new save looks older than the last sync.
    server.advance(Duration::from_secs(60));
    deck.play("chapter 2", server.now() - Duration::from_secs(3600));
    let report = deck.launch().await;
    assert_eq!(report.pushed.len(), 1, "{report:?}");
    // The time sent was moved past the last sync, so RomM itself saw the change.
    assert_eq!(server.answered().last().map(String::as_str), Some("upload"));
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"chapter 2");
}

#[tokio::test]
async fn a_save_deleted_on_romm_is_not_sent_back_until_it_changes() {
    let server = FakeRomm::start().await;
    let deck = Device::new("deleted", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    server.delete_all(ROM_ID);
    let report = deck.launch().await;
    assert!(report.pushed.is_empty(), "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("deleted on RomM")), "{report:?}");
    assert!(server.rows(ROM_ID).is_empty());

    server.advance(Duration::from_secs(60));
    deck.play("chapter 2", server.now());
    let report = deck.launch().await;
    assert_eq!(report.pushed.len(), 1, "new progress goes up: {report:?}");
}

#[tokio::test]
async fn a_save_untracked_on_this_device_is_left_alone() {
    let server = FakeRomm::start().await;
    let deck = Device::new("untracked", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;
    server.untrack(deck.id(), server.newest(ROM_ID, AUTOSAVE).unwrap().id);

    server.advance(Duration::from_secs(60));
    deck.play("chapter 2", server.now());
    let report = deck.launch().await;
    assert!(report.pushed.is_empty(), "{report:?}");
    assert_eq!(server.rows(ROM_ID).len(), 1);
}

#[tokio::test]
async fn a_device_whose_sync_romm_turned_off_moves_nothing() {
    let server = FakeRomm::start().await;
    let deck = Device::new("disabled", &server);
    server.sync_off(deck.id());
    deck.play("chapter 1", server.now());
    let report = deck.launch().await;
    assert!(report.pushed.is_empty() && report.failed.is_empty(), "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("turned off")), "{report:?}");
    assert!(server.rows(ROM_ID).is_empty());
}

#[tokio::test]
async fn romm_s_own_player_saving_in_place_comes_down() {
    let server = FakeRomm::start().await;
    let deck = Device::new("in-place", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;
    let row = server.newest(ROM_ID, AUTOSAVE).unwrap();

    // The web player rewrites the version it loaded: same id, new bytes.
    server.rewrite(row.id, b"chapter 2, in the browser");
    let report = deck.launch().await;
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 2, in the browser"));
}

#[tokio::test]
async fn a_download_that_is_not_what_romm_described_is_not_written() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("corrupt-deck", &server), Device::new("corrupt-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;
    // The bytes on RomM's disk no longer match the hash it recorded.
    server.garble(server.rows(ROM_ID)[0].id, b"garbled");

    let report = pc.launch().await;
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(pc.save(), None, "nothing written");
    let row = server.newest(ROM_ID, AUTOSAVE).unwrap();
    assert!(!server.holds(pc.id(), row.id), "and RomM is not told it arrived");
}

#[tokio::test]
async fn a_push_racing_another_device_s_is_settled_not_forced() {
    let server = FakeRomm::start().await;
    let deck = Device::new("race", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    server.advance(Duration::from_secs(60));
    deck.play("deck: chapter 2", server.now());
    // Between the Deck's negotiation and its push, another device's lands.
    server.race(ROM_ID, AUTOSAVE, b"racer: chapter 3");
    let (resolver, asked) = chooser(Choice::Remote);
    let report = with_resolver(resolver, deck.launch()).await;
    assert_eq!(asked.lock().unwrap().len(), 1, "the refusal became a question: {report:?}");
    assert_eq!(deck.save().as_deref(), Some("racer: chapter 3"));
    assert_eq!(deck.backups(), ["deck: chapter 2"]);
}

#[tokio::test]
async fn a_romm_without_sync_sessions_still_syncs_the_way_before() {
    let server = FakeRomm::start().await;
    server.without_sync_sessions();
    let deck = Device::new("legacy", &server);
    deck.play("chapter 1", server.now());
    let report = deck.launch().await;
    assert_eq!(report.pushed.len(), 1, "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("without a RomM sync session")), "{report:?}");
}

#[tokio::test]
async fn the_play_session_goes_with_the_sync_session_after_the_game() {
    let server = FakeRomm::start().await;
    let deck = Device::new("plays", &server);
    deck.play("chapter 1", server.now());
    let start = server.now() - Duration::from_secs(1800);
    let play = crate::launch::play_session(ROM_ID, start, server.now()).unwrap();
    let report = deck.launch_with(vec![play.clone()]).await;
    assert_eq!(report.plays_recorded, 1, "{report:?}");
    let plays = server.plays();
    assert_eq!(plays.len(), 1);
    assert!(plays[0].session.is_some(), "taken as part of the sync session");

    // Sent again — a retry — it is the same session, not a second one.
    let report = deck.launch_with(vec![play]).await;
    assert_eq!(report.plays_recorded, 1);
    assert_eq!(server.plays().len(), 1);
}

#[tokio::test]
async fn a_set_travels_as_one_zip_hashed_as_romm_hashes_it() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("set-deck", &server), Device::new("set-pc", &server));
    let card = |device: &Device| device.root.join("memcards/Mcd001.ps2");
    let folder = "BASLUS-20946GTA5";
    let write = |device: &Device, contents: &str| {
        let dir = card(device).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("icon.sys"), "icon").unwrap();
        fs::write(dir.join("SAVE.DAT"), contents).unwrap();
    };
    let thread = |device: &Device| {
        let target = card(device).join(folder);
        let files = ["SAVE.DAT", "icon.sys"]
            .into_iter()
            .map(|n| (n.to_string(), target.join(n)))
            .filter(|(_, p)| p.exists())
            .collect();
        Thread {
            rom_id: ROM_ID,
            stamp: None,
            slot: AUTOSAVE.into(),
            name: format!("{folder}.zip"),
            emulator: "pcsx2".into(),
            shared: true,
            place: Place::Set(super::standalone::Bundle {
                asset: format!("{folder}.zip"),
                key: target.join(".deckhand-set"),
                files,
                target,
                folder: true,
            }),
        }
    };
    let sync = |device: &Device| {
        let (client, conn, store, thread) = (RommClient::new().unwrap(), device.conn.clone(), device.store(), thread(device));
        async move {
            match session::sync(&client, &conn, &store, vec![thread], &[ROM_ID], Vec::new(), &|_| None).await.unwrap() {
                Outcome::Done(report) => *report,
                Outcome::Unsupported(why) => panic!("{why}"),
            }
        }
    };

    write(&deck, "slot 1");
    assert_eq!(sync(&deck).await.pushed.len(), 1);
    let row = server.newest(ROM_ID, AUTOSAVE).expect("in the autosave slot, where every client keeps it");
    let expected = super::hash::set(&[("SAVE.DAT".into(), b"slot 1".to_vec()), ("icon.sys".into(), b"icon".to_vec())]);
    assert_eq!(row.content_hash, expected, "RomM's hash of the zip is the set's");
    assert_eq!(sync(&deck).await.in_sync, 1, "so the next sync sees the same save on both sides");

    let report = sync(&pc).await;
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert_eq!(fs::read_to_string(card(&pc).join(folder).join("SAVE.DAT")).unwrap(), "slot 1");
}

#[tokio::test]
async fn a_library_bigger_than_one_negotiation_is_scoped_in_turns() {
    let server = FakeRomm::start().await;
    let deck = Device::new("chunks", &server);
    let scope: Vec<i64> = (1..=1_200).collect();
    let outcome = session::sync(&RommClient::new().unwrap(), &deck.conn, &deck.store(), Vec::new(), &scope, Vec::new(), &|_| None)
        .await
        .unwrap();
    assert!(matches!(outcome, Outcome::Done(_)));
    assert_eq!(server.requests("POST /api/sync/negotiate"), 3, "500 games at a time");
    assert_eq!(server.open_sessions(), 0, "each closed before the next opened");
}

#[tokio::test]
async fn a_sweep_romm_stops_answering_part_way_still_says_what_it_moved() {
    let server = FakeRomm::start().await;
    let deck = Device::new("chunks-cut-short", &server);
    deck.play("chapter 1", server.now());
    // The game's save goes up in the first negotiation; RomM then goes away for the rest.
    server.fail_negotiations_after(1);
    let scope: Vec<i64> = (1..=1_200).collect();
    let threads = vec![Thread {
        rom_id: ROM_ID,
        stamp: None,
        slot: AUTOSAVE.into(),
        name: format!("{GAME}.srm"),
        emulator: "snes9x".into(),
        place: Place::File(deck.save_path()),
        shared: false,
    }];
    let outcome = session::sync(&RommClient::new().unwrap(), &deck.conn, &deck.store(), threads, &scope, Vec::new(), &|_| None)
        .await
        .unwrap();

    let Outcome::Done(report) = outcome else { panic!("{outcome:?}") };
    assert_eq!(report.pushed, [format!("{GAME}.srm")], "what the first turn moved is not thrown away: {report:?}");
    assert!(report.notes.iter().any(|n| n.contains("left for the next sync")), "{report:?}");
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"chapter 1");
}

#[tokio::test]
async fn one_sync_runs_at_a_time() {
    let store = std::env::temp_dir().join(format!("deckhand-lock-{}", std::process::id()));
    let first = session::exclusive(&store).await.unwrap();
    let waiting = tokio::spawn({
        let store = store.clone();
        async move { session::exclusive(&store).await.map(|_| ()) }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!waiting.is_finished(), "the second waits while the first holds the lock");
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap().unwrap();
    let _ = fs::remove_dir_all(&store);
}

#[tokio::test]
async fn a_save_made_in_romm_s_own_player_comes_down() {
    let server = FakeRomm::start().await;
    let deck = Device::new("web", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    // RomM's web player — or Argosy, or Grout — saves into the same slot.
    server.advance(Duration::from_secs(600));
    server.push_from(None, ROM_ID, AUTOSAVE, &format!("{GAME}.srm"), b"chapter 2, in the browser");
    let report = deck.launch().await;
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 2, in the browser"));
}

/// The slot a phone app keeps a game's save in, rather than `autosave`.
const BATTERY: &str = "battery";

#[tokio::test]
async fn a_save_another_client_keeps_in_a_slot_of_its_own_is_followed_there() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("follow-deck", &server), Device::new("follow-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;

    // A phone app plays on from it, into a slot of its own, under a name of its own.
    server.advance(Duration::from_secs(600));
    server.push_from(Some("phone"), ROM_ID, BATTERY, "1.srm", b"chapter 2, on the phone");
    let report = deck.launch().await;
    assert_eq!((report.pulled.len(), report.conflicts.len()), (1, 0), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 2, on the phone"));

    // Played on here, it goes up where the phone looks — and nothing is taken out of autosave.
    server.advance(Duration::from_secs(600));
    deck.play("chapter 3", server.now());
    let report = deck.launch().await;
    assert_eq!(report.pushed.len(), 1, "{report:?}");
    assert_eq!(server.newest(ROM_ID, BATTERY).unwrap().bytes, b"chapter 3");
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"chapter 1");

    // A device new to the game takes the newest, and autosave's older one is not news to it.
    let report = pc.launch().await;
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert!(!report.notes.iter().any(|n| n.contains("nowhere")), "{report:?}");
    assert_eq!(pc.save().as_deref(), Some("chapter 3"));
    let again = deck.launch().await;
    assert!(again.pushed.is_empty() && again.pulled.is_empty(), "{again:?}");
}

#[tokio::test]
async fn a_save_goes_back_to_autosave_when_that_is_newer_and_leaves_the_older_slot_without_a_word() {
    let server = FakeRomm::start().await;
    let deck = Device::new("follow-back", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;
    server.advance(Duration::from_secs(600));
    server.push_from(Some("phone"), ROM_ID, BATTERY, "1.srm", b"chapter 2, on the phone");
    deck.launch().await;

    // The phone plays on in its slot, then a client that keeps autosave plays on after it.
    server.advance(Duration::from_secs(600));
    server.push_from(Some("phone"), ROM_ID, BATTERY, "1.srm", b"chapter 3, on the phone");
    server.advance(Duration::from_secs(600));
    server.push_from(Some("tv"), ROM_ID, AUTOSAVE, &format!("{GAME}.srm"), b"chapter 4, on the tv");
    let report = deck.launch().await;
    assert_eq!((report.pulled.len(), report.conflicts.len()), (1, 0), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 4, on the tv"));
    assert!(!report.notes.iter().any(|n| n.contains("nowhere")), "{report:?}");

    server.advance(Duration::from_secs(600));
    deck.play("chapter 5", server.now());
    deck.launch().await;
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"chapter 5");
    assert_eq!(server.newest(ROM_ID, BATTERY).unwrap().bytes, b"chapter 3, on the phone");
}

#[tokio::test]
async fn progress_here_and_in_another_client_s_slot_is_put_to_the_player() {
    let server = FakeRomm::start().await;
    let deck = Device::new("follow-conflict", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    server.advance(Duration::from_secs(600));
    deck.play("deck: chapter 2", server.now());
    server.advance(Duration::from_secs(600));
    server.push_from(Some("phone"), ROM_ID, BATTERY, "1.srm", b"phone: chapter 2");

    let (resolver, asked) = chooser(Choice::Local);
    let report = with_resolver(resolver, deck.launch()).await;
    assert_eq!(asked.lock().unwrap().len(), 1, "the player is asked: {report:?}");
    assert_eq!(deck.save().as_deref(), Some("deck: chapter 2"));
    // Kept here, and sent up where the phone looks; the phone's stays in that slot's history.
    let battery: Vec<Vec<u8>> = server.rows(ROM_ID).into_iter().filter(|r| r.slot.as_deref() == Some(BATTERY)).map(|r| r.bytes).collect();
    assert_eq!(battery, [b"phone: chapter 2".to_vec(), b"deck: chapter 2".to_vec()]);
}

#[tokio::test]
async fn a_version_restored_goes_into_the_slot_the_save_follows() {
    let server = FakeRomm::start().await;
    let deck = Device::new("follow-restore", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;
    server.advance(Duration::from_secs(600));
    server.push_from(Some("phone"), ROM_ID, BATTERY, "1.srm", b"chapter 2, on the phone");
    deck.launch().await;

    // Chapter 1, from autosave's history, put back from the Saves page.
    let client = RommClient::new().unwrap();
    let first = client.slot_saves(&deck.conn, ROM_ID, AUTOSAVE).await.unwrap().remove(0);
    let install = retroarch::read(&deck.root.join("ra/retroarch.cfg")).unwrap();
    let rom = deck.root.join("roms/snes").join(format!("{GAME}.sfc"));
    let save = install.game_saves(&rom, Some("snes9x")).existing.into_iter().next().unwrap();
    let thread = Thread { rom_id: ROM_ID, stamp: None, slot: AUTOSAVE.into(), name: format!("{GAME}.srm"), emulator: save.emulator, place: Place::File(save.path), shared: false };
    server.advance(Duration::from_secs(60));
    let report = session::restore(&client, &deck.conn, &deck.store(), &thread, &first).await.unwrap();
    assert_eq!(report.pushed.len(), 1, "{report:?}");
    assert_eq!(server.newest(ROM_ID, BATTERY).unwrap().bytes, b"chapter 1", "the newest where the phone looks");
    assert_eq!(server.rows(ROM_ID).len(), 3, "and nowhere else");
}

#[tokio::test]
async fn a_first_sync_with_different_saves_on_both_sides_asks() {
    let server = FakeRomm::start().await;
    let deck = Device::new("first-both", &server);
    server.push_from(None, ROM_ID, AUTOSAVE, &format!("{GAME}.srm"), b"from somewhere else");
    // Older than RomM's by the clocks, and never agreed with it: the clocks are no answer.
    deck.play("played here before Deckhand", server.now() - Duration::from_secs(86_400));
    let (resolver, asked) = chooser(Choice::Local);
    let report = with_resolver(resolver, deck.launch()).await;
    assert_eq!(asked.lock().unwrap().len(), 1, "{report:?}");
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"played here before Deckhand");
}

#[tokio::test]
async fn the_same_save_already_on_both_sides_is_agreed_without_moving_it() {
    let server = FakeRomm::start().await;
    let deck = Device::new("same", &server);
    let id = server.push_from(None, ROM_ID, AUTOSAVE, &format!("{GAME}.srm"), b"the same progress");
    deck.play("the same progress", server.now() - Duration::from_secs(86_400));
    let report = deck.launch().await;
    assert_eq!((report.in_sync, report.pushed.len(), report.pulled.len()), (1, 0, 0), "{report:?}");
    assert!(server.holds(deck.id(), id), "RomM is told this device holds it, so its next push is not refused");
    assert_eq!(server.requests("GET /api/saves/"), 0, "nothing downloaded");
}

#[tokio::test]
async fn an_agreement_from_before_sync_sessions_carries_over() {
    let server = FakeRomm::start().await;
    let deck = Device::new("migrated", &server);
    // What the engine before sync sessions left: a push it made as this device, and its index entry.
    let id = server.push_from(Some(deck.id()), ROM_ID, AUTOSAVE, &format!("{GAME}.srm"), b"chapter 1");
    deck.play("chapter 1", server.now());
    let mut index = super::index::Index::load(deck.store().join("save-index.json"));
    let legacy = super::index::Entry { rom_id: ROM_ID, hash: super::hash::file(b"chapter 1"), save_id: id, ..Default::default() };
    index.set(&deck.save_path(), legacy).unwrap();

    let report = deck.launch().await;
    assert_eq!((report.in_sync, report.pushed.len()), (1, 0), "{report:?}");
    let entry = super::index::Index::load(deck.store().join("save-index.json")).get(&deck.save_path()).cloned().unwrap();
    assert_eq!((entry.slot.as_deref(), entry.romm_hash, entry.save_id), (Some(AUTOSAVE), true, id));
}

#[tokio::test]
async fn a_device_romm_forgot_registers_again_rather_than_syncing_blind() {
    let server = FakeRomm::start().await;
    let deck = Device::new("forgotten", &server);
    server.forget_devices();
    deck.play("chapter 1", server.now());
    let install = retroarch::read(&deck.root.join("ra/retroarch.cfg")).unwrap();
    let rom = deck.root.join("roms/snes").join(format!("{GAME}.sfc"));
    let error = sync_rom(&RommClient::new().unwrap(), &deck.conn, &install, &deck.store(), &rom, "snes", Some("snes9x"), ROM_ID, None, Vec::new())
        .await
        .unwrap_err();
    assert!(error.contains("no longer knows this device"), "{error}");
    assert!(server.rows(ROM_ID).is_empty());
}

#[tokio::test]
async fn the_settings_sweep_syncs_every_game_in_one_session() {
    let server = FakeRomm::start().await;
    let deck = Device::new("sweep", &server);
    deck.play("chrono progress", server.now());
    fs::write(deck.root.join("ra/saves/Secret of Mana (USA).srm"), "mana progress").unwrap();
    fs::write(deck.root.join("ra/saves/Homebrew Thing.srm"), "homebrew").unwrap();
    let builds = vec![
        super::Build { rom_id: ROM_ID, stem: GAME.into(), system_id: "snes".into(), born: None },
        super::Build { rom_id: 2, stem: "Secret of Mana (USA)".into(), system_id: "snes".into(), born: None },
    ];
    let install = retroarch::read(&deck.root.join("ra/retroarch.cfg")).unwrap();
    let report = super::run(&RommClient::new().unwrap(), &deck.conn, &install, &builds, &deck.store(), &[], None).await.unwrap();

    let mut pushed = report.pushed.clone();
    pushed.sort();
    assert_eq!(pushed, [format!("{GAME}.srm"), "Secret of Mana (USA).srm".to_string()], "{report:?}");
    assert_eq!(report.unmatched, ["Homebrew Thing.srm"]);
    assert_eq!(server.requests("POST /api/sync/negotiate"), 1);
    assert_eq!(server.newest(2, AUTOSAVE).unwrap().bytes, b"mana progress");
}

#[tokio::test]
async fn a_sync_for_one_changed_save_sends_only_that_game_in_its_session() {
    let server = FakeRomm::start().await;
    let deck = Device::new("narrow-session", &server);
    deck.play("chrono progress", server.now());
    let mana = deck.root.join("ra/saves/Secret of Mana (USA).srm");
    fs::write(&mana, "mana progress").unwrap();
    let builds = vec![
        super::Build { rom_id: ROM_ID, stem: GAME.into(), system_id: "snes".into(), born: None },
        super::Build { rom_id: 2, stem: "Secret of Mana (USA)".into(), system_id: "snes".into(), born: None },
    ];
    let install = retroarch::read(&deck.root.join("ra/retroarch.cfg")).unwrap();
    let report =
        super::run(&RommClient::new().unwrap(), &deck.conn, &install, &builds, &deck.store(), &[], Some(&[mana])).await.unwrap();
    assert_eq!(report.pushed, ["Secret of Mana (USA).srm"], "{report:?}");
    assert!(server.rows(ROM_ID).is_empty(), "the save that did not change was not sent");
    assert_eq!(server.newest(2, AUTOSAVE).unwrap().bytes, b"mana progress");
}

#[tokio::test]
async fn the_sweep_brings_down_the_saves_of_games_downloaded_here() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("sweep-pull-deck", &server), Device::new("sweep-pull-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;

    // The PC has the game downloaded and has never played it: no save on disk to find by name.
    let game = crate::launch::Plan {
        rom_id: ROM_ID,
        title: GAME.into(),
        rom: pc.root.join("roms/snes").join(format!("{GAME}.sfc")),
        system: "snes".into(),
        platform_id: None,
        launcher: Err("not needed to sync".into()),
        emulator: crate::launch::Emulator::Core("snes9x".into()),
        stamp: None,
        names: Vec::new(),
    };
    let install = retroarch::read(&pc.root.join("ra/retroarch.cfg")).unwrap();
    let report = super::run(&RommClient::new().unwrap(), &pc.conn, &install, &[], &pc.store(), &[game], None).await.unwrap();
    assert_eq!(report.pulled, [format!("{GAME}.srm")], "{report:?}");
    assert_eq!(pc.save().as_deref(), Some("chapter 1"));
}

/// A PSP-style save folder on `device`, as a thread of the game's shared `autosave` slot.
fn folder_thread(device: &Device, folder: &str) -> Thread {
    let target = device.root.join("SAVEDATA").join(folder);
    let files = ["DATA.BIN", "PARAM.SFO"]
        .into_iter()
        .map(|n| (n.to_string(), target.join(n)))
        .filter(|(_, p)| p.exists())
        .collect();
    Thread {
        rom_id: ROM_ID,
        stamp: None,
        slot: AUTOSAVE.into(),
        name: format!("{folder}.zip"),
        emulator: "ppsspp".into(),
        place: Place::Set(super::standalone::Bundle {
            asset: format!("{folder}.zip"),
            key: target.join(".deckhand-set"),
            files,
            target,
            folder: true,
        }),
        shared: true,
    }
}

fn write_folder(device: &Device, folder: &str, data: &str) {
    let dir = device.root.join("SAVEDATA").join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("PARAM.SFO"), "sfo").unwrap();
    fs::write(dir.join("DATA.BIN"), data).unwrap();
}

fn read_folder(device: &Device, folder: &str) -> Option<String> {
    fs::read_to_string(device.root.join("SAVEDATA").join(folder).join("DATA.BIN")).ok()
}

async fn sync_folders(device: &Device, folders: &[&str]) -> Report {
    let threads = folders.iter().map(|f| folder_thread(device, f)).collect();
    match session::sync(&RommClient::new().unwrap(), &device.conn, &device.store(), threads, &[ROM_ID], Vec::new(), &|_| None)
        .await
        .unwrap()
    {
        Outcome::Done(report) => *report,
        Outcome::Unsupported(why) => panic!("{why}"),
    }
}

#[tokio::test]
async fn a_slot_another_client_keeps_a_different_file_in_is_settled_by_name() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("shared-deck", &server), Device::new("shared-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;
    // Another client keeps the game's memory card as `.mcd`, in the same slot, newer.
    server.advance(Duration::from_secs(60));
    server.push_from(Some("tv"), ROM_ID, AUTOSAVE, &format!("{GAME}.mcd"), b"another client's card");

    // The slot's newest is the `.mcd`: what comes down is still the `.srm`.
    let report = pc.launch().await;
    assert_eq!(report.pulled, [format!("{GAME}.srm")], "{report:?}");
    assert_eq!(pc.save().as_deref(), Some("chapter 1"));

    // Played on the PC: RomM refuses the push, since the PC never synced the `.mcd` — which is not
    // this save, so it goes up past the check, and the `.mcd` stays.
    server.advance(Duration::from_secs(60));
    pc.play("chapter 2", server.now());
    let report = pc.launch().await;
    assert_eq!(report.pushed.len(), 1, "{report:?}");
    assert!(report.conflicts.is_empty(), "{report:?}");
    let names: Vec<String> = server.rows(ROM_ID).iter().map(|r| crate::sync::canonical(&r.file_name)).collect();
    assert!(names.contains(&format!("{GAME}.mcd")), "the other client's card is untouched: {names:?}");

    // And back on the Deck it is a plain download.
    let report = deck.launch().await;
    assert_eq!((report.pulled.len(), report.conflicts.len()), (1, 0), "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 2"));
}

#[tokio::test]
async fn save_folders_that_share_their_slot_stay_in_autosave() {
    let server = FakeRomm::start().await;
    let deck = Device::new("folders-stay", &server);
    let a = "ULUS10041DATA00";
    write_folder(&deck, a, "a1");
    sync_folders(&deck, &[a]).await;

    // A newer copy of the folder in another slot is not where the game's other folders are.
    server.advance(Duration::from_secs(60));
    server.push_from(Some("phone"), ROM_ID, BATTERY, &format!("{a}.zip"), b"not read");
    server.advance(Duration::from_secs(60));
    write_folder(&deck, a, "a2");
    let report = sync_folders(&deck, &[a]).await;
    assert_eq!((report.pushed.len(), report.pulled.len()), (1, 0), "{report:?}");
    assert_eq!(read_folder(&deck, a).as_deref(), Some("a2"));
    let battery = server.rows(ROM_ID).into_iter().filter(|r| r.slot.as_deref() == Some(BATTERY)).count();
    assert_eq!(battery, 1, "nothing went into the other slot");
}

#[tokio::test]
async fn several_save_folders_in_one_slot_each_keep_their_own_history() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("folders-deck", &server), Device::new("folders-pc", &server));
    let (a, b) = ("ULUS10041DATA00", "ULUS10041DATA01");
    write_folder(&deck, a, "a1");
    write_folder(&deck, b, "b1");
    let report = sync_folders(&deck, &[a, b]).await;
    assert_eq!(report.pushed.len(), 2, "{report:?}");

    // RomM's negotiation offers only the slot's newest; the other folder is found by name.
    let report = sync_folders(&pc, &[a, b]).await;
    assert_eq!(report.pulled.len(), 2, "{report:?}");
    assert_eq!((read_folder(&pc, a).as_deref(), read_folder(&pc, b).as_deref()), (Some("a1"), Some("b1")));

    // One folder changes on each side; neither is a conflict, and neither drags the other along.
    server.advance(Duration::from_secs(60));
    write_folder(&deck, a, "a2");
    let report = sync_folders(&deck, &[a, b]).await;
    assert_eq!((report.pushed.len(), report.in_sync, report.conflicts.len()), (1, 1, 0), "{report:?}");
    server.advance(Duration::from_secs(60));
    write_folder(&pc, b, "b2");
    let report = sync_folders(&pc, &[a, b]).await;
    assert_eq!((report.pushed.len(), report.pulled.len(), report.conflicts.len()), (1, 1, 0), "{report:?}");
    assert_eq!(read_folder(&pc, a).as_deref(), Some("a2"));

    let report = sync_folders(&deck, &[a, b]).await;
    assert_eq!((report.pulled.len(), report.conflicts.len()), (1, 0), "{report:?}");
    assert_eq!((read_folder(&deck, a).as_deref(), read_folder(&deck, b).as_deref()), (Some("a2"), Some("b2")));
    assert!(server.log().iter().filter(|l| l.starts_with("POST /api/saves?")).all(|l| !l.contains("autocleanup")),
        "a slot several saves share is never trimmed");
}

#[tokio::test]
async fn a_first_launch_with_no_core_known_says_why_nothing_came_down() {
    let server = FakeRomm::start().await;
    let deck = Device::new("no-core-deck", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;

    // RetroDECK's layout files saves by core, and nothing says which core runs the game.
    let pc = Device::new("no-core-pc", &server);
    fs::write(
        pc.root.join("ra/retroarch.cfg"),
        "savefile_directory = \":/saves\"\nsort_savefiles_by_content_enable = \"true\"\nsort_savefiles_enable = \"true\"\n",
    )
    .unwrap();
    let install = retroarch::read(&pc.root.join("ra/retroarch.cfg")).unwrap();
    let rom = pc.root.join("roms/snes").join(format!("{GAME}.sfc"));
    let report = sync_rom(&RommClient::new().unwrap(), &pc.conn, &install, &pc.store(), &rom, "snes", None, ROM_ID, None, Vec::new()).await.unwrap();
    assert!(report.pulled.is_empty(), "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("no save here yet")), "{report:?}");
}

#[tokio::test]
async fn a_device_that_is_only_behind_takes_the_newer_save_without_a_question() {
    let server = FakeRomm::start().await;
    let (deck, pc) = (Device::new("behind-deck", &server), Device::new("behind-pc", &server));
    deck.play("chapter 1", server.now());
    deck.launch().await;
    server.advance(Duration::from_secs(600));
    deck.play("chapter 2", server.now());
    deck.launch().await;

    // The PC has chapter 1 from somewhere — a copy made before Deckhand — and no record of syncing.
    pc.play("chapter 1", server.now() + Duration::from_secs(3600));
    let (resolver, asked) = chooser(Choice::Local);
    let report = with_resolver(resolver, pc.launch()).await;
    assert!(asked.lock().unwrap().is_empty(), "nothing to ask: chapter 1 is in RomM's history — {report:?}");
    assert_eq!(report.pulled.len(), 1, "{report:?}");
    assert_eq!(pc.save().as_deref(), Some("chapter 2"));
    assert!(pc.backups().is_empty(), "and nothing to keep: RomM has it");
}

#[tokio::test]
async fn restoring_one_save_of_a_shared_slot_does_not_trim_the_others_off_romm() {
    let server = FakeRomm::start().await;
    let deck = Device::new("restore-shared", &server);
    // Two of the game's save folders: RomM pairs both on its one `autosave` slot, and its trimming
    // counts the slot, so a push that asks for it would count another folder's save as an old
    // version of this one.
    write_folder(&deck, "ULUS10041DATA00", "the folder being restored");
    write_folder(&deck, "ULUS10041SYSTEM", "another folder's only save");
    let report = sync_folders(&deck, &["ULUS10041DATA00", "ULUS10041SYSTEM"]).await;
    assert_eq!(report.pushed.len(), 2, "{report:?}");
    let restoring = server.rows(ROM_ID).into_iter().find(|r| r.file_name.contains("ULUS10041DATA00")).unwrap();

    // The slot filled past the ten a trimming push keeps.
    for n in 0..9 {
        server.advance(Duration::from_secs(60));
        server.push_from(None, ROM_ID, AUTOSAVE, "ULUS10041DATA00.zip", format!("version {n}").as_bytes());
    }
    server.advance(Duration::from_secs(60));
    let client = RommClient::new().unwrap();
    let version = client.slot_saves(&deck.conn, ROM_ID, AUTOSAVE).await.unwrap().into_iter().find(|v| v.id == restoring.id).unwrap();
    let thread = folder_thread(&deck, "ULUS10041DATA00");
    let report = session::restore(&client, &deck.conn, &deck.store(), &thread, &version).await.unwrap();
    assert_eq!(report.pushed, ["ULUS10041DATA00.zip"], "{report:?}");

    let left: Vec<String> = server.rows(ROM_ID).into_iter().map(|r| r.file_name).collect();
    assert!(left.iter().any(|n| n.contains("ULUS10041SYSTEM")), "the other folder's only save is still on RomM: {left:?}");
    assert_eq!(read_folder(&deck, "ULUS10041DATA00").as_deref(), Some("the folder being restored"));
}

#[tokio::test]
async fn an_older_version_restored_becomes_the_newest_everywhere_and_this_device_s_copy_is_kept() {
    let server = FakeRomm::start().await;
    let deck = Device::new("restore", &server);
    let pc = Device::new("restore-pc", &server);
    deck.play("chapter 1", server.now());
    deck.launch().await;
    server.advance(Duration::from_secs(600));
    deck.play("chapter 2", server.now());
    deck.launch().await;
    pc.launch().await;
    assert_eq!(pc.save().as_deref(), Some("chapter 2"));

    // Chapter 1, from RomM's history of the slot, put back from the Saves page.
    let client = RommClient::new().unwrap();
    let versions = client.slot_saves(&deck.conn, ROM_ID, AUTOSAVE).await.unwrap();
    let first = versions.iter().min_by_key(|v| v.updated_at.clone()).unwrap();
    let install = retroarch::read(&deck.root.join("ra/retroarch.cfg")).unwrap();
    let rom = deck.root.join("roms/snes").join(format!("{GAME}.sfc"));
    let save = install.game_saves(&rom, Some("snes9x")).existing.into_iter().next().unwrap();
    let thread = Thread { rom_id: ROM_ID, stamp: None, slot: AUTOSAVE.into(), name: format!("{GAME}.srm"), emulator: save.emulator, place: Place::File(save.path), shared: false };
    assert!(session::holds(&thread, first));
    server.advance(Duration::from_secs(60));
    let report = session::restore(&client, &deck.conn, &deck.store(), &thread, first).await.unwrap();
    assert_eq!(report.pushed, [format!("{GAME}.srm")], "{report:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 1"));
    assert_eq!(deck.backups(), ["chapter 2"], "what it replaced is kept");
    assert_eq!(server.newest(ROM_ID, AUTOSAVE).unwrap().bytes, b"chapter 1", "RomM's newest is the restored one");

    // It sticks: the deck's next launch moves nothing — an old version on disk is not read as being
    // behind — and the PC takes it.
    server.advance(Duration::from_secs(60));
    let again = deck.launch().await;
    assert!(again.pushed.is_empty() && again.pulled.is_empty(), "{again:?}");
    assert_eq!(deck.save().as_deref(), Some("chapter 1"));
    pc.launch().await;
    assert_eq!(pc.save().as_deref(), Some("chapter 1"));
}

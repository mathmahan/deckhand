//! The saves of the standalone emulators RetroDECK runs by default — Dolphin for GameCube and Wii,
//! PCSX2 for PlayStation 2, PPSSPP for PSP — kept in step with RomM one game at a time.
//!
//! **Since sigil**, a game's saves go through [`super::units`], and what this module packs itself is
//! the shape before: the one a game's first sync through sigil settles RomM's copies in, and the one
//! a game sigil cannot take still syncs in — see [`sigil_request`] for where each emulator is found.
//! The rest of these notes are that shape's.
//!
//! A port of the Kotlin app's `GameCubeHandler`, `Pcsx2Handler` and `PspHandler`, in the same model
//! and the same server format, so the two apps sync with each other: **a game's saves are a set of
//! files, and the set travels as one zip**. RomM gets `<asset>.zip` in the game's save history; what
//! changed is decided by a hash over the set's names and bytes, so a re-zip or a touched mtime is
//! never mistaken for progress. Otherwise it is the battery-save rule — see [`super::plan`] — in an
//! index of its own, `standalone-index.json`, with the newer side winning a conflict and the losing
//! local set copied into `save-backups/` as a zip first.
//!
//! What a game's set is, per emulator:
//!
//! - **Dolphin**, GCI-folder memory cards: every `.gci` on `Card A` / `Card B` whose header carries
//!   the game's 6-character id, read from the disc header — no catalogue needed. `<ID>.gci.zip`,
//!   flat.
//! - **PCSX2**, folder memory cards: each `<BA|BE|BI><serial>…` folder on the cards `PCSX2.ini`
//!   inserts, one zip per folder. A file memory card (`Mcd001.ps2`) holds every game in one 8 MB
//!   image and is not split here.
//! - **PPSSPP**: each `SAVEDATA/<serial>…` folder, one zip per folder.
//! - **RPCS3**: each `savedata/<TITLEID>…` folder whose `PARAM.SFO` titles this game — a PS3 save
//!   carries its game's name, so no catalogue — under the `dev_hdd0` RPCS3's `vfs.yml` maps.
//! - **Vita3K**: the `savedata/<TITLEID>` folder named by the id in the game's own file name,
//!   under the `pref-path` its `config.yml` names.
//! - **Azahar**: the game's `title/00040000/<id>/data` folder on the emulated SD card, by the program
//!   id in the ROM's NCCH header.
//! - **Cemu**: `usr/save/<high>/<low>/user` under its `mlc_path`, by the title id in an unpacked
//!   game's `meta/meta.xml`.
//! - **Dolphin running a Wii disc**: the game's `title/00010000/<code in hex>/data/` folder in the
//!   emulated NAND, `<ID>.wii.zip`. The Kotlin app has no Wii handler, so this name is this app's.
//!
//! Save states sync too, one file per slot under its own name — the Kotlin app's
//! `SerialStateHandler`: PCSX2's, PPSSPP's and DuckStation's, found by serial, and Dolphin's, by the
//! game's id. A Dolphin state loads only in a Dolphin that saves states the same way, which a
//! version apart often does not; one that will not load is refused by Dolphin with a message, and
//! the game's own save, synced separately, is untouched.
//!
//! A PS2 or PSP folder is named after the disc serial, which nothing in RomM carries, so the serial
//! is read out of the game's own disc — a CHD or a CSO too — by [`crate::sigil`]. A folder RomM has
//! for the game and this device does not comes down too.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::index::Entry;
use super::session::{self, Outcome, Place, Thread};
use super::{Plan, Report, Run, describe, md5_hex, names, plan};
use crate::config::Connection;
use crate::library::Stamp;
use crate::romm::RommClient;
use crate::romm::models::{PlaySession, SaveAsset, SyncOperation};

/// A standalone emulator this build syncs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emulator {
    Dolphin,
    /// Dolphin running a Wii disc: the save is in its emulated NAND, not on a memory card.
    DolphinWii,
    Pcsx2,
    Ppsspp,
    Rpcs3,
    Vita3k,
    Azahar,
    Cemu,
    /// PS1 memory cards, synced through sigil only: the engine before it never kept them. Its save
    /// states as well.
    DuckStation,
    /// Switch saves under Ryubing, Ryujinx's fork, or Ryujinx itself. Synced through sigil only.
    Ryujinx,
    /// Switch saves under Eden, a yuzu fork. Synced through sigil only.
    Eden,
}

impl Emulator {
    /// The emulator a launch of an ES-DE `system` runs, by the name ES-DE gives its command — for the
    /// pairs this build can sync, and only those.
    pub fn for_launch(system: &str, label: &str) -> Option<Self> {
        let label = label.to_lowercase();
        match system {
            "gc" if label.contains("dolphin") => Some(Emulator::Dolphin),
            "wii" if label.contains("dolphin") => Some(Emulator::DolphinWii),
            "ps2" if label.contains("pcsx2") => Some(Emulator::Pcsx2),
            "psp" if label.contains("ppsspp") => Some(Emulator::Ppsspp),
            "ps3" if label.contains("rpcs3") => Some(Emulator::Rpcs3),
            "psvita" if label.contains("vita3k") => Some(Emulator::Vita3k),
            "n3ds" if label.contains("azahar") || label.contains("citra") => Some(Emulator::Azahar),
            "wiiu" if label.contains("cemu") => Some(Emulator::Cemu),
            "psx" if label.contains("duckstation") => Some(Emulator::DuckStation),
            "switch" if label.contains("ryujinx") || label.contains("ryubing") => Some(Emulator::Ryujinx),
            "switch" if label.contains("eden") => Some(Emulator::Eden),
            _ => None,
        }
    }

    /// RomM's `emulator` for an upload — what the Kotlin app tags these with.
    pub(super) fn tag(self) -> &'static str {
        match self {
            Emulator::Dolphin | Emulator::DolphinWii => "dolphin",
            Emulator::Pcsx2 => "pcsx2",
            Emulator::Ppsspp => "ppsspp",
            Emulator::Rpcs3 => "rpcs3",
            Emulator::Vita3k => "vita3k",
            Emulator::Azahar => "azahar",
            Emulator::Cemu => "cemu",
            Emulator::DuckStation => "duckstation",
            Emulator::Ryujinx => "ryujinx",
            Emulator::Eden => "eden",
        }
    }
}

/// One game's save set, as one RomM save.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Bundle {
    /// The server file name, `GALE01.gci.zip`.
    pub(super) asset: String,
    /// What the index is keyed by: a path no file has, unique to the set.
    pub(super) key: PathBuf,
    /// The set's files on disk, by their name in the zip.
    pub(super) files: Vec<(String, PathBuf)>,
    /// Where a pull unpacks to.
    pub(super) target: PathBuf,
    /// A folder set is replaced whole on a pull; a flat one only overwrites its own files, since the
    /// card it shares holds every other game's too.
    pub(super) folder: bool,
}

/// Sync the saves `emulator` keeps for the game launched from `rom`, titled `title` — through a sync
/// session where RomM holds them ([`super::session`]), closing it with `plays`; the way before that
/// where it does not.
///
/// In a session each set is a thread of the game's `autosave` slot, where every client of this library
/// keeps it. A PS2, PSP or PS3 game's save folders are several sets in one slot, which RomM's pairing
/// cannot tell apart — [`super::session`] settles those by name.
#[allow(clippy::too_many_arguments)]
pub async fn sync(
    client: &RommClient,
    connection: &Connection,
    store: &Path,
    emulator: Emulator,
    rom: &Path,
    title: &str,
    rom_id: i64,
    stamp: Option<&Stamp>,
    plays: Vec<PlaySession>,
) -> Result<Report, String> {
    let _one_at_a_time = session::exclusive(store).await?;
    let keys = title_keys(title, rom);
    // Through sigil — see [`super::units`] — and the engine before for a game sigil cannot take.
    let game = super::units::Game { rom_id, rom, system: "", title, stamp };
    let mut notes = Vec::new();
    let threads = match super::units::thread(game, super::units::Runner::Standalone(emulator), Err("")) {
        Ok(thread) => vec![thread],
        Err(why) => {
            notes.push(format!("{title}: synced as before sigil, since {why}"));
            threads(emulator, rom, title, rom_id, stamp)?
        }
    };
    let folder = threads.iter().find_map(|t| match &t.place {
        Place::Set(bundle) => Some(bundle.target.clone()),
        Place::Sigil(unit) => Some(unit.request.root.clone()),
        Place::File(_) => None,
    });
    let late = |op: &SyncOperation| late(emulator, rom, title, rom_id, stamp, op);
    let mut report = match session::sync(client, connection, store, threads, &[rom_id], plays, &late).await? {
        Outcome::Done(report) => Report { folder: folder.unwrap_or_default(), ..*report },
        Outcome::Unsupported(why) => {
            let mut report = legacy(client, connection, store, emulator, rom, title, rom_id, stamp, &keys).await?;
            report.notes.push(format!("synced without a RomM sync session: {why}"));
            report
        }
    };
    report.notes.extend(notes);

    // Save states: one file per slot, each a save of its own, uploaded under its own name — the
    // Kotlin app's `SerialStateHandler`.
    if let Some(states) = state_files(emulator, rom) {
        match states {
            Ok((dir, local, id)) => {
                let remote: Vec<SaveAsset> = client
                    .fetch_states(connection, rom_id)
                    .await
                    .map_err(|e| format!("Could not read the states on RomM: {}", describe(&e)))?
                    .into_iter()
                    .filter(|s| s.rom_id.is_none_or(|id| id == rom_id) && is_state_of(emulator, &names::canonical(&s.file_name)))
                    // Named by the game's own id, a state of another game's never belongs here.
                    .filter(|s| id.as_deref().is_none_or(|id| names::canonical(&s.file_name).starts_with(id)))
                    .collect();
                let mut names: Vec<String> = local.iter().cloned().chain(remote.iter().map(|s| names::canonical(&s.file_name))).collect();
                names.sort();
                names.dedup();
                let mut run = Run::with_index(client, connection, store, dir.clone(), "state-index.json").stamped(stamp.cloned());
                for name in names {
                    let newest = remote
                        .iter()
                        .filter(|s| names::canonical(&s.file_name) == name)
                        .max_by_key(|s| (s.updated_at.as_deref().and_then(names::parse_timestamp), s.id));
                    run.state_file(dir.join(&name), newest, rom_id, emulator.tag()).await;
                }
                report.absorb(run.report);
            }
            Err(why) => report.notes.push(format!("save states not synced: {why}")),
        }
    }
    Ok(report)
}

/// The game's save sets on this device, as threads of a sync session — and where RomM's goes when
/// this device has none yet.
pub(super) fn threads(emulator: Emulator, rom: &Path, title: &str, rom_id: i64, stamp: Option<&Stamp>) -> Result<Vec<Thread>, String> {
    let keys = title_keys(title, rom);
    Ok(discover(emulator, rom, title, &keys, &[])?.into_iter().map(|b| thread(emulator, rom_id, stamp, b)).collect())
}

/// The engine before sigil's threads for the game, as they were, and — named in `server`, RomM's
/// files for it — the sets RomM has that no card here holds: what is settled in the old shape before
/// the game's first sync through sigil. See [`super::units`].
pub(super) fn legacy_threads(
    emulator: Emulator,
    rom: &Path,
    title: &str,
    rom_id: i64,
    stamp: Option<&Stamp>,
    server: &[String],
) -> Vec<Thread> {
    let keys = title_keys(title, rom);
    discover(emulator, rom, title, &keys, server).unwrap_or_default().into_iter().map(|b| thread(emulator, rom_id, stamp, b)).collect()
}

/// A place for a set RomM offers that no thread names: a save folder for the game that no card here
/// holds yet.
pub(super) fn late(emulator: Emulator, rom: &Path, title: &str, rom_id: i64, stamp: Option<&Stamp>, op: &SyncOperation) -> Option<Thread> {
    let name = names::canonical(&op.file_name);
    if op.rom_id != rom_id || !name.to_lowercase().ends_with(".zip") {
        return None;
    }
    let found = discover(emulator, rom, title, &title_keys(title, rom), std::slice::from_ref(&name)).ok()?;
    found.into_iter().find(|b| b.asset.eq_ignore_ascii_case(&name)).map(|b| thread(emulator, rom_id, stamp, b))
}

/// The game's save sets on this device, and — named in `server` — the ones RomM has that a card here
/// could take.
fn discover(emulator: Emulator, rom: &Path, title: &str, keys: &[String], server: &[String]) -> Result<Vec<Bundle>, String> {
    match emulator {
        Emulator::Dolphin => dolphin(rom),
        Emulator::DolphinWii => wii(rom),
        Emulator::Pcsx2 => pcsx2(rom, server),
        Emulator::Ppsspp => ppsspp(rom, server),
        Emulator::Rpcs3 => rpcs3(keys, server),
        Emulator::Vita3k => vita3k(rom, title, server),
        Emulator::Azahar => azahar(rom),
        Emulator::Cemu => cemu(rom),
        Emulator::DuckStation | Emulator::Ryujinx | Emulator::Eden => Ok(Vec::new()),
    }
}

/// A set as a thread of the game's `autosave` slot, where every client of this library keeps it — one
/// of several there for an emulator that keeps a folder per save.
pub(super) fn thread(emulator: Emulator, rom_id: i64, stamp: Option<&Stamp>, bundle: Bundle) -> Thread {
    let shared = matches!(emulator, Emulator::Pcsx2 | Emulator::Ppsspp | Emulator::Rpcs3);
    Thread {
        rom_id,
        stamp: stamp.cloned(),
        slot: session::AUTOSAVE.to_string(),
        name: bundle.asset.clone(),
        emulator: emulator.tag().to_string(),
        place: Place::Set(bundle),
        shared,
    }
}

/// [`sync`]'s sets for a RomM without sync sessions: each against the newest of its name.
#[allow(clippy::too_many_arguments)]
async fn legacy(
    client: &RommClient,
    connection: &Connection,
    store: &Path,
    emulator: Emulator,
    rom: &Path,
    title: &str,
    rom_id: i64,
    stamp: Option<&Stamp>,
    keys: &[String],
) -> Result<Report, String> {
    let server: Vec<SaveAsset> = client
        .fetch_saves(connection, Some(rom_id))
        .await
        .map_err(|e| format!("Could not read the saves on RomM: {}", describe(&e)))?
        .into_iter()
        .filter(|s| s.rom_id.is_none_or(|id| id == rom_id))
        .collect();
    let server_names: Vec<String> = server.iter().map(|s| names::canonical(&s.file_name)).collect();
    let bundles = discover(emulator, rom, title, keys, &server_names)?;
    let folder = bundles.first().map(|b| b.target.clone()).unwrap_or_default();
    let mut run = Run::with_index(client, connection, store, folder, "standalone-index.json").stamped(stamp.cloned());
    for bundle in &bundles {
        run.bundle(bundle, rom_id, emulator.tag(), &server).await;
    }
    Ok(run.finish().await)
}

/// Whether `name` is a state file of `emulator`'s, by its exact shape — a Syncthing copy or a
/// `.backup` sibling does not fit.
fn is_state_of(emulator: Emulator, name: &str) -> bool {
    match emulator {
        // `SLUS-20946 (2C6BE434).01.p2s`
        Emulator::Pcsx2 => {
            ps2_serial(name).is_some()
                && name.ends_with(".p2s")
                && name.split_once(" (").is_some_and(|(_, rest)| rest.contains(")."))
        }
        // `ULUS10538_2.00_0.ppst`
        Emulator::Ppsspp => {
            let parts: Vec<&str> = name.strip_suffix(".ppst").map(|s| s.split('_').collect()).unwrap_or_default();
            parts.len() == 3
                && psp_serial(parts[0]).is_some()
                && parts[0].len() == 9
                && parts[1].split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                && !parts[2].is_empty()
                && parts[2].chars().all(|c| c.is_ascii_digit())
        }
        // `GALE01.s01`: the game's id, then the slot. Dolphin numbers its slots from 01.
        Emulator::Dolphin | Emulator::DolphinWii => name.split_once(".s").is_some_and(|(id, slot)| {
            id.len() == 6
                && id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                && slot.len() == 2
                && slot.bytes().all(|b| b.is_ascii_digit())
                && slot != "00"
        }),
        // `SLUS-00594_1.sav`, and `SLUS-00594_resume.sav` — what "save state on exit" writes.
        Emulator::DuckStation => name.strip_suffix(".sav").and_then(|stem| stem.rsplit_once('_')).is_some_and(|(serial, slot)| {
            serial.len() == 10
                && ps2_serial(serial).is_some()
                && (slot == "resume" || (!slot.is_empty() && slot.bytes().all(|b| b.is_ascii_digit())))
        }),
        _ => false,
    }
}

/// A state folder, the names in it that are one game's, and — for an emulator that names its states
/// by the game's own id — that id, which RomM's states are matched by as well.
type StateFiles = (PathBuf, Vec<String>, Option<String>);

/// The folder an emulator keeps states in, and the ones there that are this game's — by the serial
/// on its disc, like its saves, or for Dolphin by the id in the disc's header. None for an emulator
/// whose states are not synced.
fn state_files(emulator: Emulator, rom: &Path) -> Option<Result<StateFiles, String>> {
    if matches!(emulator, Emulator::Dolphin | Emulator::DolphinWii) {
        return Some(dolphin_states(emulator, rom));
    }
    let dir = match emulator {
        Emulator::Pcsx2 => pcsx2_states(),
        Emulator::Ppsspp => first_dir(installs(
            "config/ppsspp/PSP/PPSSPP_STATE",
            ("org.ppsspp.PPSSPP", "config/ppsspp/PSP/PPSSPP_STATE"),
            ".config/ppsspp/PSP/PPSSPP_STATE",
        ))
        .ok_or_else(|| "no PPSSPP_STATE folder found".to_string()),
        Emulator::DuckStation => duckstation_ini().map(|ini| duckstation_states(&ini)),
        // None of the others has a state format worth carrying to another machine.
        _ => return None,
    };
    Some(dir.and_then(|dir| {
        let serials = disc_serials(emulator, rom)?;
        let mut ours: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| is_state_of(emulator, name) && of_serials(emulator, name, &serials))
            .collect();
        ours.sort();
        Ok((dir, ours, None))
    }))
}

/// The serials on the game's discs — `SLUS-20946` for PCSX2's names, `ULUS10538` for PPSSPP's,
/// `SLUS-00594` for DuckStation's — read out of each disc by sigil: every disc an `.m3u` lists, else
/// the file itself.
fn disc_serials(emulator: Emulator, rom: &Path) -> Result<Vec<String>, String> {
    let platform = match emulator {
        Emulator::Pcsx2 => "ps2",
        Emulator::Ppsspp => "psp",
        Emulator::DuckStation => "psx",
        _ => return Ok(Vec::new()),
    };
    let mut serials = Vec::new();
    let mut why = None;
    for disc in discs(rom) {
        match crate::sigil::identify(&disc, Some(platform)) {
            Ok(game) if !game.title_id().is_empty() => serials.push(game.title_id()),
            Ok(_) => {}
            Err(e) => why = Some(e),
        }
    }
    serials.sort();
    serials.dedup();
    match (serials.is_empty(), why) {
        (false, _) => Ok(serials),
        (true, Some(e)) => Err(format!("its disc serial could not be read: {e}")),
        (true, None) => Err(format!("{} carries no disc serial", rom.display())),
    }
}

/// Whether a save folder or state named `name` is of one of `serials`' games, by the serial its name
/// starts with.
fn of_serials(emulator: Emulator, name: &str, serials: &[String]) -> bool {
    match emulator {
        // A PS1 disc's serial has a PS2 one's shape, `SLUS-00594`.
        Emulator::Pcsx2 | Emulator::DuckStation => ps2_serial(name).is_some_and(|s| serials.contains(&s)),
        Emulator::Ppsspp => psp_serial(name).is_some_and(|s| serials.iter().any(|id| id.replace('-', "") == s.replace('-', ""))),
        _ => false,
    }
}

/// Dolphin's `StateSaves` folder, and the game's states in it, named by the id in its disc's header.
///
/// The folder is beside Dolphin's `GC` and `Wii` ones, and Dolphin makes it only when it first saves
/// a state — so where Dolphin is set up and has not saved one yet, this is where RomM's go.
fn dolphin_states(emulator: Emulator, rom: &Path) -> Result<StateFiles, String> {
    let user = first_dir(installs("data/dolphin-emu", ("org.DolphinEmu.dolphin-emu", "data/dolphin-emu"), ".local/share/dolphin-emu"))
        .ok_or("no Dolphin folder found")?;
    let disc = entry_point(rom);
    let id = match emulator {
        Emulator::DolphinWii => wii_id(&disc),
        _ => gamecube_id(&disc),
    }
    .ok_or_else(|| format!("{} has no game id Deckhand can read", disc.display()))?;
    Ok(states_named(emulator, user.join("StateSaves"), &id))
}

/// The states in `dir` that are the game `id`'s, by name.
fn states_named(emulator: Emulator, dir: PathBuf, id: &str) -> StateFiles {
    let mut ours: Vec<String> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_state_of(emulator, name) && name.starts_with(id))
        .collect();
    ours.sort();
    (dir, ours, Some(id.to_string()))
}

/// PCSX2's state folder: `SaveStates` in its `PCSX2.ini`, else `sstates` beside it.
fn pcsx2_states() -> Result<PathBuf, String> {
    let (ini, settings) = pcsx2_ini()?;
    let base = ini.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
    Ok(settings.get("SaveStates").map_or_else(|| base.join("sstates"), |s| base.join(s)))
}

/// DuckStation's `settings.ini`.
fn duckstation_ini() -> Result<PathBuf, String> {
    installs(
        "config/duckstation/settings.ini",
        ("org.duckstation.DuckStation", "config/duckstation/settings.ini"),
        ".local/share/duckstation/settings.ini",
    )
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| "no DuckStation settings.ini found".to_string())
}

/// DuckStation's state folder: `SaveStates` in its `settings.ini`, else `savestates` beside it — under
/// RetroDECK a link into `states/psx/duckstation`.
fn duckstation_states(ini: &Path) -> PathBuf {
    let base = ini.parent().unwrap_or(Path::new("."));
    ini_value(ini, "SaveStates").map_or_else(|| base.join("savestates"), |s| base.join(s))
}

impl Run<'_> {
    async fn bundle(&mut self, bundle: &Bundle, rom_id: i64, tag: &str, server: &[SaveAsset]) {
        let name = bundle.asset.clone();
        let remote = server
            .iter()
            .filter(|s| names::canonical(&s.file_name).eq_ignore_ascii_case(&bundle.asset))
            .max_by_key(|s| (s.updated_at.as_deref().and_then(names::parse_timestamp), s.id));
        let local = match read_set(&bundle.files) {
            Ok(set) => set,
            Err(e) => return self.report.fail(Some(rom_id), &name, false, e),
        };
        if local.is_empty() {
            // Nothing here: a fresh device takes RomM's. Nothing on either side is not news.
            if let Some(remote) = remote {
                self.pull_bundle(bundle, rom_id, remote, &name).await;
            }
            return;
        }
        let hash = set_hash(&local);
        match (plan(&hash, self.index.agreed(&bundle.key, rom_id, self.stamp.as_ref()), remote.map(|r| r.id)), remote) {
            (Plan::InSync, _) => self.report.agreed(Some(rom_id), &name, false),
            (Plan::Push, _) => {
                self.push_bundle(bundle, rom_id, tag, &local, hash, &name).await;
            }
            (Plan::Pull, Some(remote)) => {
                self.pull_bundle(bundle, rom_id, remote, &name).await;
            }
            (Plan::Conflict, Some(remote)) => {
                let local_time = bundle.files.iter().filter_map(|(_, p)| fs::metadata(p).and_then(|m| m.modified()).ok()).max();
                let remote_time = remote.updated_at.as_deref().and_then(names::parse_timestamp);
                let bytes = local.iter().map(|(_, b)| b.len() as u64).sum();
                if super::choose(&name, local_time, remote_time, bytes).await == super::Choice::Remote {
                    let Ok(zipped) = pack(&local) else { return self.report.fail(Some(rom_id), &name, false, "could not zip a copy") };
                    let Some(kept) = self.back_up(&bundle.target, &zipped, &name) else { return };
                    if self.pull_bundle(bundle, rom_id, remote, &name).await {
                        let said = format!("{name}: kept RomM's saves; this device's are at {}", kept.display());
                        self.report.settled(Some(rom_id), &name, false, false, Some(kept), said);
                    }
                } else if self.push_bundle(bundle, rom_id, tag, &local, hash, &name).await {
                    self.report.settled(Some(rom_id), &name, false, true, None, format!("{name}: kept this device's saves; RomM keeps the other"));
                }
            }
            (Plan::Pull | Plan::Conflict, None) => unreachable!("a server change with no server save"),
        }
    }

    async fn push_bundle(&mut self, bundle: &Bundle, rom_id: i64, tag: &str, set: &[(String, Vec<u8>)], hash: String, name: &str) -> bool {
        let zipped = match pack(set) {
            Ok(zipped) => zipped,
            Err(e) => {
                self.report.fail(Some(rom_id), name, false, format!("could not zip it ({e})"));
                return false;
            }
        };
        match self.client.upload_save(self.connection, rom_id, tag, &bundle.asset, zipped).await {
            Ok(Some(created)) => {
                let ok = self.remember_bundle(bundle, rom_id, hash, created.id, name, None);
                if ok {
                    self.report.up(Some(rom_id), name, false);
                }
                ok
            }
            Ok(None) => {
                self.report.put_off(Some(rom_id), name, false);
                false
            }
            Err(e) => {
                self.report.fail(Some(rom_id), name, false, describe(&e));
                false
            }
        }
    }

    async fn pull_bundle(&mut self, bundle: &Bundle, rom_id: i64, remote: &SaveAsset, name: &str) -> bool {
        let bytes = match self.client.download_save(self.connection, remote).await {
            Ok(bytes) => bytes,
            Err(e) => {
                self.report.fail(Some(rom_id), name, false, describe(&e));
                return false;
            }
        };
        let set = match unpack(&bytes, &bundle.target, bundle.folder, &bundle.files) {
            Ok(set) => set,
            Err(e) => {
                self.report.fail(Some(rom_id), name, false, e);
                return false;
            }
        };
        // What the pull wrote, not what was here before it: on a device taking the set for the first
        // time there was nothing, and a flat set's own files are what say whether it is still here.
        let written: Vec<PathBuf> = set.iter().map(|(relative, _)| bundle.target.join(relative)).collect();
        let ok = self.remember_bundle(bundle, rom_id, set_hash(&set), remote.id, name, Some(&written));
        if ok {
            self.report.down(Some(rom_id), name, false);
            self.report.written.extend(bundle.files.iter().map(|(_, path)| path.clone()));
            self.report.written.extend(written);
            self.acks.push(remote.id);
        }
        ok
    }

    /// `written` is what a pull just put on disk, in place of the files the bundle was built from.
    fn remember_bundle(
        &mut self,
        bundle: &Bundle,
        rom_id: i64,
        hash: String,
        save_id: i64,
        name: &str,
        written: Option<&[PathBuf]>,
    ) -> bool {
        // A flat set records its own files: see [`Entry::files`].
        let path_name = |path: &PathBuf| path.to_string_lossy().into_owned();
        let files = (!bundle.folder).then(|| match written {
            Some(written) => written.iter().map(path_name).collect(),
            None => bundle.files.iter().map(|(_, path)| path_name(path)).collect(),
        });
        match self.index.set(&bundle.key, Entry { rom_id, hash, save_id, files, stamp: self.stamp.clone(), ..Default::default() }) {
            Ok(()) => true,
            Err(e) => {
                self.report.fail(Some(rom_id), name, false, format!("synced, but could not record it ({e})"));
                false
            }
        }
    }
}

// ---- The set as bytes: read, hash, zip, unzip -------------------------------------------------

pub(super) fn read_set(files: &[(String, PathBuf)]) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut set: Vec<(String, Vec<u8>)> = files.iter().map(|(n, p)| Ok((n.clone(), fs::read(p)?))).collect::<std::io::Result<_>>()?;
    set.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(set)
}

/// MD5 over each file's name and bytes in name order — the same set always hashes the same.
pub(super) fn set_hash(set: &[(String, Vec<u8>)]) -> String {
    let mut all = Vec::new();
    let mut sorted: Vec<&(String, Vec<u8>)> = set.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, bytes) in sorted {
        all.extend_from_slice(name.as_bytes());
        all.extend_from_slice(bytes);
    }
    md5_hex(&all)
}

pub(super) fn pack(set: &[(String, Vec<u8>)]) -> zip::result::ZipResult<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in set {
        zip.start_file(name.as_str(), options)?;
        zip.write_all(bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Unpack `bytes` into `target`, answering what was written.
///
/// A folder set is staged beside the target and swapped in whole, so a failure leaves the old save
/// rather than half of the new one. A flat set writes each file through a staged sibling, and `mine`
/// — the set's own files as they were found — says which of what is already there is this save's, so
/// that a file the incoming set no longer names goes with it. Either way an entry naming a path
/// outside the target is refused.
pub(super) fn unpack(
    bytes: &[u8],
    target: &Path,
    folder: bool,
    mine: &[(String, PathBuf)],
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a zip RomM sent: {e}"))?;
    let mut set = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let relative = entry.enclosed_name().ok_or_else(|| format!("{} would land outside the save folder", entry.name()))?;
        // Flat sets live straight in the card folder, whatever path the zip gave them.
        let relative = if folder { relative } else { PathBuf::from(relative.file_name().unwrap_or_default()) };
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(|e| e.to_string())?;
        set.push((relative.to_string_lossy().replace('\\', "/"), content));
    }

    let parent = target.parent().ok_or("no folder to unpack into")?;
    fs::create_dir_all(parent).map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    if folder {
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let staging = parent.join(format!(".{name}.deckhand-tmp"));
        let _ = fs::remove_dir_all(&staging);
        let aside = parent.join(format!(".{name}.deckhand-old"));
        let written = (|| -> std::io::Result<()> {
            for (relative, content) in &set {
                let path = staging.join(relative);
                fs::create_dir_all(path.parent().unwrap_or(&staging))?;
                fs::write(path, content)?;
            }
            // The folder there now is moved aside rather than deleted until the new one is in its
            // place, and put back if that fails — so there is never a moment with neither.
            let _ = fs::remove_dir_all(&aside);
            let had = target.exists();
            if had {
                fs::rename(target, &aside)?;
            }
            if let Err(e) = fs::rename(&staging, target) {
                if had {
                    let _ = fs::rename(&aside, target);
                }
                return Err(e);
            }
            if had {
                let _ = fs::remove_dir_all(&aside);
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        written.map_err(|e| format!("can't write {}: {e}", target.display()))?;
    } else {
        fs::create_dir_all(target).map_err(|e| format!("can't create {}: {e}", target.display()))?;
        for (relative, content) in &set {
            super::write_atomic(&target.join(relative), content).map_err(|e| format!("can't write {relative}: {e}"))?;
        }
        // A file of this save that RomM's copy of the set does not name was deleted on the device
        // that pushed it, and goes here too — left behind, it would be read as a change made here
        // and pushed straight back, so the deletion would never take anywhere. Only files already
        // known to be this save's, and only in the folder being written: a flat set shares its card
        // with every other game's saves, and a copy of this one on another card is not this run's to
        // touch. The whole set is in `save-backups/` first — see [`super::session::Run::pull`].
        for (name, path) in mine {
            let gone = !set.iter().any(|(relative, _)| relative.eq_ignore_ascii_case(name));
            if gone && path.parent() == Some(target) {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(set)
}

/// Every file under `dir` that is part of the save, by its path relative to `dir` with `/`
/// separators.
fn walk(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if !of_the_set(&entry.file_name().to_string_lossy()) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(dir) {
                out.push((relative.to_string_lossy().replace('\\', "/"), path));
            }
        }
    }
    out
}

/// Whether a file or folder in a save folder is part of the save: not one of this app's own working
/// files, left behind by a write that failed, and not a copy a file syncer put beside it.
///
/// Either would go into the zip and change the set's hash, so the same save would read as different
/// on this device from every other — a push, and a pull on the other side, every sync.
fn of_the_set(name: &str) -> bool {
    !name.contains(".deckhand-") && !name.contains("sync-conflict")
}

/// The first of `candidates` that is a directory.
fn first_dir(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|p| p.is_dir())
}

/// `~/.var/app/<app>/<rest>`, `~/<native>` — where an emulator keeps something under RetroDECK, its
/// own Flatpak, and a native install, in that order.
fn installs(retrodeck: &str, flatpak: (&str, &str), native: &str) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    vec![
        home.join(".var/app").join(crate::launch::RETRODECK).join(retrodeck),
        home.join(".var/app").join(flatpak.0).join(flatpak.1),
        home.join(native),
    ]
}

// ---- Dolphin ------------------------------------------------------------------------------------

/// The game's `.gci` set. Dolphin's `GC` folder holds a `USA`, `EUR` and `JAP` folder, each with a
/// `Card A` and `Card B` — under RetroDECK each region folder is a link into `saves/gc/dolphin/`.
fn dolphin(rom: &Path) -> Result<Vec<Bundle>, String> {
    let root = first_dir(installs(
        "data/dolphin-emu/GC",
        ("org.DolphinEmu.dolphin-emu", "data/dolphin-emu/GC"),
        ".local/share/dolphin-emu/GC",
    ))
    .ok_or("no Dolphin GC folder found")?;
    let disc = entry_point(rom);
    let id = gamecube_id(&disc).ok_or_else(|| format!("{} has no GameCube game id Deckhand can read", disc.display()))?;

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut home: Option<PathBuf> = None;
    for region in ["USA", "EUR", "JAP"] {
        for card in ["Card A", "Card B"] {
            let dir = root.join(region).join(card);
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_gci = name.to_lowercase().ends_with(".gci") && !name.contains("sync-conflict");
                if !is_gci || gci_id(&path).is_none_or(|g| !g.eq_ignore_ascii_case(&id)) {
                    continue;
                }
                if seen.insert(fs::canonicalize(&path).unwrap_or_else(|_| path.clone())) {
                    home.get_or_insert_with(|| dir.clone());
                    files.push((name, path));
                }
            }
        }
    }
    let asset = format!("{id}.gci.zip");
    Ok(vec![Bundle {
        key: root.join(&asset),
        target: home.unwrap_or_else(|| root.join(gamecube_region(&id)).join("Card A")),
        asset,
        files,
        folder: false,
    }])
}

/// The game's Wii save: `title/00010000/<game code in hex>/data/` in Dolphin's emulated NAND — under
/// RetroDECK a link into `saves/wii/dolphin`. One folder, synced as `<ID>.wii.zip`.
fn wii(rom: &Path) -> Result<Vec<Bundle>, String> {
    let nand = first_dir(installs(
        "data/dolphin-emu/Wii",
        ("org.DolphinEmu.dolphin-emu", "data/dolphin-emu/Wii"),
        ".local/share/dolphin-emu/Wii",
    ))
    .ok_or("no Dolphin Wii NAND found")?;
    let disc = entry_point(rom);
    let id = wii_id(&disc).ok_or_else(|| format!("{} has no Wii game id Deckhand can read", disc.display()))?;
    let title: String = id[..4].bytes().map(|b| format!("{b:02x}")).collect();
    let target = nand.join("title/00010000").join(title).join("data");
    Ok(vec![Bundle {
        asset: format!("{id}.wii.zip"),
        key: target.join(".deckhand-set"),
        files: walk(&target),
        target,
        folder: true,
    }])
}

/// The 6-character id from a Wii disc's header: raw ISO (Wii magic at 0x18), RVZ/WIA (the header
/// embedded at 0x58), WBFS (a copy of it at 0x200). Anything else is None.
pub fn wii_id(disc: &Path) -> Option<String> {
    let mut head = vec![0u8; 0x400];
    let mut file = fs::File::open(disc).ok()?;
    let read = read_up_to(&mut file, &mut head);
    let head = &head[..read];
    let offset = if head.starts_with(b"RVZ\x01") || head.starts_with(b"WIA\x01") {
        0x58
    } else if head.starts_with(b"WBFS") {
        0x200
    } else {
        0
    };
    let magic = head.get(offset + 0x18..offset + 0x1C)?;
    (magic == [0x5D, 0x1C, 0x9E, 0xA3]).then(|| game_id_at(head, offset)).flatten()
}

/// The disc a launch target is: the first entry of a multi-disc `.m3u`, else the file itself.
fn entry_point(rom: &Path) -> PathBuf {
    discs(rom).into_iter().next().unwrap_or_else(|| rom.to_path_buf())
}

/// Every disc a launch target is: each entry of a multi-disc `.m3u`, else the file itself.
pub(crate) fn discs(rom: &Path) -> Vec<PathBuf> {
    if rom.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u"))
        && let Ok(text) = fs::read_to_string(rom)
    {
        let listed: Vec<PathBuf> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| rom.parent().unwrap_or(Path::new("")).join(l))
            .collect();
        if !listed.is_empty() {
            return listed;
        }
    }
    vec![rom.to_path_buf()]
}

/// The 6-character id from a GameCube disc's header: raw ISO/GCM (header at 0, disc magic at
/// 0x1C), RVZ/WIA (the header embedded at 0x58), CISO (at a fixed 0x8000). Anything else is None.
pub fn gamecube_id(disc: &Path) -> Option<String> {
    let mut head = vec![0u8; 0x8000 + 0x20];
    let mut file = fs::File::open(disc).ok()?;
    let read = read_up_to(&mut file, &mut head);
    let head = &head[..read];
    if head.len() >= 0x60 && (head.starts_with(b"RVZ\x01") || head.starts_with(b"WIA\x01")) {
        return game_id_at(head, 0x58);
    }
    let offset = if head.starts_with(b"CISO") { 0x8000 } else { 0 };
    let magic = head.get(offset + 0x1C..offset + 0x20)?;
    (magic == [0xC2, 0x33, 0x9F, 0x3D]).then(|| game_id_at(head, offset)).flatten()
}

fn gci_id(gci: &Path) -> Option<String> {
    let mut head = [0u8; 0x40];
    let mut file = fs::File::open(gci).ok()?;
    (read_up_to(&mut file, &mut head) == head.len()).then(|| game_id_at(&head, 0)).flatten()
}

fn read_up_to(file: &mut fs::File, buf: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    filled
}

/// Six printable bytes, the first four a game code — rejects a zeroed or garbage header.
fn game_id_at(bytes: &[u8], at: usize) -> Option<String> {
    let id = bytes.get(at..at + 6)?;
    if !id.iter().all(|b| (0x20..=0x7e).contains(b)) || !id[..4].iter().all(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some(String::from_utf8_lossy(id).to_uppercase())
}

/// The region folder a game id's fourth character puts its saves in.
fn gamecube_region(id: &str) -> &'static str {
    match id.chars().nth(3) {
        Some('P' | 'X' | 'Y' | 'D' | 'F' | 'S' | 'I' | 'H') => "EUR",
        Some('J' | 'K' | 'T') => "JAP",
        _ => "USA",
    }
}

// ---- PCSX2 --------------------------------------------------------------------------------------

/// The prefixes PCSX2 gives a folder card's save folders, by region.
const MEMCARD_PREFIXES: [&str; 3] = ["BA", "BE", "BI"];

/// `SLUS-20946` from `SLUS-20946…`, or from a folder card's `BASLUS-20946GTA50000`.
fn ps2_serial(name: &str) -> Option<String> {
    let serial = |s: &str| {
        let b = s.as_bytes();
        (b.len() >= 10 && b[..4].iter().all(u8::is_ascii_uppercase) && b[4] == b'-' && b[5..10].iter().all(u8::is_ascii_digit))
            .then(|| s[..10].to_string())
    };
    serial(name).or_else(|| MEMCARD_PREFIXES.iter().find_map(|p| name.strip_prefix(p).and_then(serial)))
}

/// PCSX2's `PCSX2.ini`, and what it says.
fn pcsx2_ini() -> Result<(PathBuf, HashMap<String, String>), String> {
    let ini = installs(
        "config/PCSX2/inis/PCSX2.ini",
        ("net.pcsx2.PCSX2", "config/PCSX2/inis/PCSX2.ini"),
        ".config/PCSX2/inis/PCSX2.ini",
    )
    .into_iter()
    .find(|p| p.is_file())
    .ok_or("no PCSX2.ini found")?;
    let settings: HashMap<String, String> = fs::read_to_string(&ini)
        .map_err(|e| format!("could not read {}: {e}", ini.display()))?
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Ok((ini, settings))
}

/// The folder cards PCSX2 has inserted, from its own `PCSX2.ini`.
fn pcsx2_cards() -> Result<Vec<PathBuf>, String> {
    let (ini, settings) = pcsx2_ini()?;
    // Relative to PCSX2's own folder, the one holding `inis/`.
    let base = ini.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
    let folder = settings.get("MemoryCards").map_or_else(|| base.join("memcards"), |m| base.join(m));
    let cards: Vec<PathBuf> = ["Slot1", "Slot2"]
        .iter()
        .filter(|slot| settings.get(&format!("{slot}_Enable")).is_none_or(|v| v.eq_ignore_ascii_case("true")))
        .filter_map(|slot| settings.get(&format!("{slot}_Filename")))
        .map(|name| folder.join(name))
        .filter(|card| card.is_dir())
        .collect();
    if cards.is_empty() {
        return Err("PCSX2's memory card is a single file holding every game, which can't be synced one game at a time — \
                    convert it to a folder card in PCSX2 (Settings ▸ Memory Cards ▸ Convert) and it will be"
            .into());
    }
    Ok(cards)
}

fn pcsx2(rom: &Path, server: &[String]) -> Result<Vec<Bundle>, String> {
    let cards = pcsx2_cards()?;
    let serials = disc_serials(Emulator::Pcsx2, rom)?;
    let ours = |folder: &str| of_serials(Emulator::Pcsx2, folder, &serials);
    let mut bundles = Vec::new();
    for card in &cards {
        for entry in fs::read_dir(card).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_save = entry.path().is_dir() && MEMCARD_PREFIXES.iter().any(|p| name.starts_with(p)) && !name.contains("sync-conflict");
            if is_save && ours(&name) {
                bundles.push(folder_bundle(card, &name));
            }
        }
    }
    // What RomM has for this game and no card here does: onto the first card.
    for asset in server {
        if let Some(folder) = asset.strip_suffix(".zip")
            && MEMCARD_PREFIXES.iter().any(|p| folder.starts_with(p))
            && ps2_serial(folder).is_some()
            && !bundles.iter().any(|b| b.asset.eq_ignore_ascii_case(asset))
        {
            bundles.push(folder_bundle(&cards[0], folder));
        }
    }
    Ok(bundles)
}

// ---- PPSSPP -------------------------------------------------------------------------------------

/// `ULUS-10041` from a `SAVEDATA` folder `ULUS10041DATA00`.
fn psp_serial(name: &str) -> Option<String> {
    let b = name.as_bytes();
    (b.len() >= 9 && b[..4].iter().all(u8::is_ascii_uppercase) && b[4..9].iter().all(u8::is_ascii_digit))
        .then(|| format!("{}-{}", &name[..4], &name[4..9]))
}

fn ppsspp(rom: &Path, server: &[String]) -> Result<Vec<Bundle>, String> {
    let savedata = first_dir(installs(
        "config/ppsspp/PSP/SAVEDATA",
        ("org.ppsspp.PPSSPP", "config/ppsspp/PSP/SAVEDATA"),
        ".config/ppsspp/PSP/SAVEDATA",
    ))
    .ok_or("no PPSSPP SAVEDATA folder found")?;
    let serials = disc_serials(Emulator::Ppsspp, rom)?;
    let mut bundles = Vec::new();
    for entry in fs::read_dir(&savedata).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = of_serials(Emulator::Ppsspp, &name, &serials);
        if entry.path().is_dir() && ours && !name.contains("sync-conflict") {
            bundles.push(folder_bundle(&savedata, &name));
        }
    }
    for asset in server {
        if let Some(folder) = asset.strip_suffix(".zip")
            && psp_serial(folder).is_some()
            && !bundles.iter().any(|b| b.asset.eq_ignore_ascii_case(asset))
        {
            bundles.push(folder_bundle(&savedata, folder));
        }
    }
    Ok(bundles)
}

// ---- RPCS3 --------------------------------------------------------------------------------------

/// A PS3 or Vita title id at the start of a name: four letters, five digits.
fn title_id(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    (b.len() >= 9 && b[..4].iter().all(u8::is_ascii_uppercase) && b[4..9].iter().all(u8::is_ascii_digit)).then(|| &name[..9])
}

/// RPCS3's save folders: `dev_hdd0/home/00000001/savedata`, where `dev_hdd0` is wherever RPCS3's own
/// `vfs.yml` maps it — under RetroDECK, into its storage.
fn rpcs3_savedata() -> Result<PathBuf, String> {
    let vfs = installs("config/rpcs3/vfs.yml", ("net.rpcs3.RPCS3", "config/rpcs3/vfs.yml"), ".config/rpcs3/vfs.yml")
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("no RPCS3 vfs.yml found")?;
    let text = fs::read_to_string(&vfs).map_err(|e| format!("could not read {}: {e}", vfs.display()))?;
    let value = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().trim_matches('"').to_string()));
    let emulator_dir = value("$(EmulatorDir):")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vfs.parent().unwrap_or(Path::new(".")).display().to_string() + "/");
    let hdd0 = value("/dev_hdd0/:").unwrap_or_else(|| "$(EmulatorDir)dev_hdd0/".into()).replace("$(EmulatorDir)", &emulator_dir);
    Ok(PathBuf::from(hdd0).join("home/00000001/savedata"))
}

/// Every save folder whose `PARAM.SFO` names this game — a PS3 save carries its game's title, so no
/// catalogue is needed — and what RomM has besides.
fn rpcs3(keys: &[String], server: &[String]) -> Result<Vec<Bundle>, String> {
    let savedata = rpcs3_savedata()?;
    let mut bundles = Vec::new();
    for entry in fs::read_dir(&savedata).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let title = sfo_title(&entry.path().join("PARAM.SFO"));
        if title_id(&name).is_some() && !name.contains("sync-conflict") && title.is_some_and(|t| keys.contains(&normalize(&t))) {
            bundles.push(folder_bundle(&savedata, &name));
        }
    }
    for asset in server {
        if let Some(folder) = asset.strip_suffix(".zip")
            && title_id(folder).is_some()
            && !bundles.iter().any(|b| b.asset.eq_ignore_ascii_case(asset))
        {
            bundles.push(folder_bundle(&savedata, folder));
        }
    }
    Ok(bundles)
}

/// `TITLE` from a `PARAM.SFO`: a header, a table of 16-byte entries, then key and data tables. Any
/// malformed file is None — a save that cannot say whose it is is one not attributed.
pub fn sfo_title(path: &Path) -> Option<String> {
    let b = fs::read(path).ok().filter(|b| b.len() >= 0x14 && b.len() <= 1 << 20 && b.starts_with(b"\0PSF"))?;
    let u16_at = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize);
    let u32_at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let (keys, data, entries) = (u32_at(8)?, u32_at(12)?, u32_at(16)?.min(1024));
    (0..entries).find_map(|i| {
        let e = 0x14 + i * 16;
        let key_start = keys + u16_at(e)?;
        let key_end = b.get(key_start..)?.iter().position(|c| *c == 0)? + key_start;
        // 0x0204 is UTF-8 with its terminator; 0x0004 without.
        if &b[key_start..key_end] != b"TITLE" || !matches!(u16_at(e + 2)?, 0x0204 | 0x0004) {
            return None;
        }
        let start = data + u32_at(e + 12)?;
        let value = b.get(start..start + u32_at(e + 4)?)?;
        let text = String::from_utf8_lossy(value.split(|c| *c == 0).next()?).trim().to_string();
        (!text.is_empty()).then_some(text)
    })
}

// ---- Vita3K -------------------------------------------------------------------------------------

/// Vita3K's save folders: `ux0/user/00/savedata` under the `pref-path` its `config.yml` names.
fn vita3k_savedata() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let config = [home.join(".var/app").join(crate::launch::RETRODECK).join("config/Vita3K/config.yml"), home.join(".config/Vita3K/config.yml")]
        .into_iter()
        .find(|p| p.is_file());
    let pref = config
        .and_then(|c| fs::read_to_string(c).ok())
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("pref-path:").map(|v| v.trim().trim_matches('"').to_string())))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/Vita3K/Vita3K"));
    pref.join("ux0/user/00/savedata")
}

/// The game's one save folder, named by its title id — which a Vita dump carries in its file name,
/// conventionally bracketed: `TxK [PCSE00397] [USA].zip`.
fn vita3k(rom: &Path, title: &str, server: &[String]) -> Result<Vec<Bundle>, String> {
    let savedata = vita3k_savedata();
    let name = rom.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let id = [name.as_str(), title]
        .iter()
        .find_map(|s| (0..s.len()).filter(|i| s.is_char_boundary(*i)).find_map(|i| title_id(&s[i..]).map(str::to_string)))
        .ok_or_else(|| format!("no title id in \"{name}\" to find its save folder by"))?;
    let mut bundles = vec![folder_bundle(&savedata, &id)];
    for asset in server {
        if let Some(folder) = asset.strip_suffix(".zip")
            && folder.len() == 9
            && title_id(folder).is_some()
            && !bundles.iter().any(|b| b.asset.eq_ignore_ascii_case(asset))
        {
            bundles.push(folder_bundle(&savedata, folder));
        }
    }
    Ok(bundles)
}

// ---- Azahar -------------------------------------------------------------------------------------

/// A setting from an INI-style file — `key=value`, the last spelling winning.
fn ini_value(path: &Path, key: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .filter_map(|l| l.split_once('='))
        .rfind(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The game's 3DS save: `sdmc/Nintendo 3DS/<0×32>/<0×32>/title/00040000/<id>/data`, `<id>` being the
/// low half of the program id in the ROM's NCCH header. Azahar, like Citra before it, uses the
/// all-zero console ids. One folder, `<program id>.3ds.zip`.
fn azahar(rom: &Path) -> Result<Vec<Bundle>, String> {
    let sdmc = azahar_sdmc();
    let id = program_id(&entry_point(rom)).ok_or_else(|| format!("{} has no 3DS program id Deckhand can read (a .3ds, .cci or .cxi is needed)", rom.display()))?;
    let zeros = "0".repeat(32);
    let target = sdmc
        .join("Nintendo 3DS")
        .join(&zeros)
        .join(&zeros)
        .join("title")
        .join(format!("{:08x}", id >> 32))
        .join(format!("{:08x}", id & 0xffff_ffff))
        .join("data");
    Ok(vec![Bundle {
        asset: format!("{id:016X}.3ds.zip"),
        key: target.join(".deckhand-set"),
        files: walk(&target),
        target,
        folder: true,
    }])
}

/// Azahar's emulated SD card: the one its `qt-config.ini` names when told to use custom storage,
/// else its own.
fn azahar_sdmc() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let config = [
        home.join(".var/app").join(crate::launch::RETRODECK).join("config/azahar-emu/qt-config.ini"),
        home.join(".var/app/org.azahar_emu.Azahar/config/azahar-emu/qt-config.ini"),
        home.join(".config/azahar-emu/qt-config.ini"),
    ]
    .into_iter()
    .find(|p| p.is_file());
    config
        .as_deref()
        .filter(|c| ini_value(c, "use_custom_storage").is_some_and(|v| v == "true"))
        .and_then(|c| ini_value(c, "sdmc_directory"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/azahar-emu/sdmc"))
}

/// The program id of a 3DS ROM: an NCSD image (`.3ds`, `.cci`) holds its game as partition 0, an
/// NCCH (`.cxi`) is that partition itself; either way it is the u64 at 0x118 of the NCCH header.
/// Anything else — a `.cia`, a compressed image — is None.
pub fn program_id(rom: &Path) -> Option<u64> {
    let mut head = vec![0u8; 0x200];
    let mut file = fs::File::open(rom).ok()?;
    let read = read_up_to(&mut file, &mut head);
    let u32_at = |b: &[u8], i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let ncch_at: u64 = match head.get(0x100..0x104)? {
        b"NCCH" => 0,
        // The partition table: offset and size in 0x200-byte media units, partition 0 first.
        b"NCSD" => u64::from(u32_at(&head[..read], 0x120)?) * 0x200,
        _ => return None,
    };
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(ncch_at)).ok()?;
    let mut ncch = vec![0u8; 0x200];
    if read_up_to(&mut file, &mut ncch) < 0x120 || &ncch[0x100..0x104] != b"NCCH" {
        return None;
    }
    let id = u64::from_le_bytes(ncch[0x118..0x120].try_into().ok()?);
    (id != 0).then_some(id)
}

// ---- Cemu ---------------------------------------------------------------------------------------

/// The game's Wii U save: `<mlc>/usr/save/<high>/<low>/user`, by the title id in an unpacked game's
/// `meta/meta.xml`. A `.wud`, `.wux` or `.wua` keeps it encrypted or compressed, and is not read.
fn cemu(rom: &Path) -> Result<Vec<Bundle>, String> {
    let mlc = cemu_mlc();
    let id = wiiu_title_id(rom).ok_or_else(|| format!("no meta/meta.xml beside {} to read its title id from", rom.display()))?;
    let target = mlc.join("usr/save").join(&id[..8]).join(&id[8..]).join("user");
    Ok(vec![Bundle {
        asset: format!("{}.wiiu.zip", id.to_uppercase()),
        key: target.join(".deckhand-set"),
        files: walk(&target),
        target,
        folder: true,
    }])
}

/// Cemu's `mlc01`: the `mlc_path` its `settings.xml` names, else its own.
fn cemu_mlc() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    [
        home.join(".var/app").join(crate::launch::RETRODECK).join("config/Cemu/settings.xml"),
        home.join(".var/app/info.cemu.Cemu/config/Cemu/settings.xml"),
        home.join(".config/Cemu/settings.xml"),
    ]
    .into_iter()
    .filter_map(|c| fs::read_to_string(c).ok())
    .find_map(|xml| {
        let start = xml.find("<mlc_path>")? + "<mlc_path>".len();
        let end = xml[start..].find("</mlc_path>")? + start;
        Some(xml[start..end].trim().to_string()).filter(|p| !p.is_empty())
    })
    .map(PathBuf::from)
    .unwrap_or_else(|| home.join(".local/share/Cemu/mlc01"))
}

/// `0005000010101c00` from `<title_id …>0005000010101C00</title_id>` in the game's `meta/meta.xml`,
/// found from the launch target — the game folder itself, or a file inside its `code/`.
pub fn wiiu_title_id(rom: &Path) -> Option<String> {
    let meta = rom.ancestors().take(4).map(|dir| dir.join("meta/meta.xml")).find(|m| m.is_file())?;
    let xml = fs::read_to_string(meta).ok()?;
    let open = xml.find("<title_id")?;
    let start = xml[open..].find('>')? + open + 1;
    let end = xml[start..].find("</title_id>")? + start;
    let id = xml[start..end].trim().to_lowercase();
    (id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit())).then_some(id)
}

pub(super) fn folder_bundle(parent: &Path, folder: &str) -> Bundle {
    let target = parent.join(folder);
    Bundle {
        asset: format!("{folder}.zip"),
        key: target.join(".deckhand-set"),
        files: walk(&target),
        target,
        folder: true,
    }
}

// ---- What sigil is asked -------------------------------------------------------------------------

/// What sigil is asked about the game `rom` runs as on `emulator`: the emulator's layout, the folder
/// its paths are under — the one holding its `memcards/`, `PSP/`, `dev_hdd0/`, `ux0/`, `sdmc/`,
/// `mlc01/`, `bis/` or `nand/`, as sigil names them — the settings that move a save, and the game's
/// ids off its discs, a Switch game's read with the user's keys.
///
/// Every emulator is found where [`installs`] looks, RetroDECK's first. A folder the emulator was
/// told to keep somewhere sigil cannot name — memory cards in a folder not called `memcards` — is
/// an error, and the game keeps the engine before sigil.
pub(super) fn sigil_request(emulator: Emulator, rom: &Path) -> Result<crate::sigil::Request, String> {
    let named = |dir: &Path, name: &str, what: &str| -> Result<PathBuf, String> {
        match (dir.file_name().and_then(|n| n.to_str()), dir.parent()) {
            (Some(n), Some(parent)) if n == name => Ok(parent.to_path_buf()),
            _ => Err(format!("{what} is in {}, a folder not named {name}, which sigil can't take", dir.display())),
        }
    };
    let (layout, platform, root, settings, profile): (&str, &str, PathBuf, Vec<PathBuf>, Option<String>) = match emulator {
        Emulator::Dolphin | Emulator::DolphinWii => {
            let user = first_dir(installs("data/dolphin-emu", ("org.DolphinEmu.dolphin-emu", "data/dolphin-emu"), ".local/share/dolphin-emu"))
                .ok_or("no Dolphin folder found")?;
            let ini = installs(
                "config/dolphin-emu/Dolphin.ini",
                ("org.DolphinEmu.dolphin-emu", "config/dolphin-emu/Dolphin.ini"),
                ".config/dolphin-emu/Dolphin.ini",
            );
            let platform = if emulator == Emulator::DolphinWii { "wii" } else { "gamecube" };
            ("dolphin_standalone", platform, user, ini, None)
        }
        Emulator::Pcsx2 => {
            let (ini, settings) = pcsx2_ini()?;
            let base = ini.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
            let cards = settings.get("MemoryCards").map_or_else(|| base.join("memcards"), |m| base.join(m));
            ("pcsx2_standalone", "ps2", named(&cards, "memcards", "PCSX2's memory cards folder")?, vec![ini], None)
        }
        Emulator::Ppsspp => {
            let savedata = first_dir(installs(
                "config/ppsspp/PSP/SAVEDATA",
                ("org.ppsspp.PPSSPP", "config/ppsspp/PSP/SAVEDATA"),
                ".config/ppsspp/PSP/SAVEDATA",
            ))
            .ok_or("no PPSSPP SAVEDATA folder found")?;
            let psp = named(&savedata, "SAVEDATA", "PPSSPP's SAVEDATA")?;
            ("ppsspp_standalone", "psp", named(&psp, "PSP", "PPSSPP's memory stick")?, Vec::new(), None)
        }
        Emulator::Rpcs3 => {
            let savedata = rpcs3_savedata()?;
            // `dev_hdd0/home/<profile>/savedata`
            let hdd0 = savedata.ancestors().nth(3).ok_or("RPCS3's dev_hdd0 has no folder above it")?.to_path_buf();
            ("rpcs3", "ps3", named(&hdd0, "dev_hdd0", "RPCS3's dev_hdd0")?, Vec::new(), Some("00000001".into()))
        }
        Emulator::Vita3k => {
            // `ux0/user/<profile>/savedata`
            let ux0 = vita3k_savedata().ancestors().nth(3).ok_or("Vita3K's ux0 has no folder above it")?.to_path_buf();
            ("vita3k", "psvita", named(&ux0, "ux0", "Vita3K's ux0")?, Vec::new(), Some("00".into()))
        }
        Emulator::Azahar => ("azahar", "3ds", named(&azahar_sdmc(), "sdmc", "Azahar's SD card")?, Vec::new(), None),
        Emulator::Cemu => ("cemu", "wiiu", named(&cemu_mlc(), "mlc01", "Cemu's mlc01")?, Vec::new(), None),
        Emulator::DuckStation => {
            let file = duckstation_ini()?;
            let cards = ini_value(&file, "Directory")
                .map(PathBuf::from)
                .unwrap_or_else(|| file.parent().unwrap_or(Path::new(".")).join("memcards"));
            ("duckstation", "psx", named(&cards, "memcards", "DuckStation's memory cards folder")?, vec![file], None)
        }
        Emulator::Ryujinx => {
            let base = ryujinx_base().ok_or("no Ryujinx folder found")?;
            let profile = ryujinx_profile(&base);
            ("ryujinx", "switch", base, Vec::new(), profile)
        }
        Emulator::Eden => {
            let data = eden_data().ok_or("no Eden folder found")?;
            ("eden", "switch", eden_root(&data, Some(&eden_config(&data)))?, Vec::new(), None)
        }
    };
    // The settings sigil says this layout's files turn on, from the emulator's own config.
    let file = settings.iter().find(|p| p.is_file());
    let options = crate::sigil::layout(layout)
        .map(|row| row.options)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|option| Some((option.key.clone(), ini_value(file?, &option.key)?)))
        .collect();
    let disc = entry_point(rom);
    let keys = switch_keys(emulator);
    let identify = |d: &Path| crate::sigil::identify_with(d, Some(platform), keys.as_deref());
    let identity = identify(&disc).map_err(|e| match &keys {
        None if platform == "switch" => format!("its game id could not be read without the Switch's prod.keys: {e}"),
        _ => format!("its game id could not be read: {e}"),
    })?;
    let game_ids = discs(rom)
        .iter()
        .filter_map(|d| identify(d).ok())
        .map(|i| i.title_id())
        .filter(|id| !id.is_empty())
        .collect();
    Ok(crate::sigil::Request {
        layout: layout.into(),
        platform: Some(platform.into()),
        content: disc,
        identity: Some(identity),
        options,
        root,
        profile,
        game_ids,
    })
}

// ---- Switch -------------------------------------------------------------------------------------

/// Of an emulator's installs that are here, the one that ran last, by when it last wrote `log` — a
/// log folder it writes on every start. A Flatpak, an AppImage's native folder and RetroDECK's,
/// which once shipped Ryujinx, can all be here at once; the earlier of two that never logged wins.
fn last_used(candidates: Vec<PathBuf>, log: &str) -> Option<PathBuf> {
    candidates
        .into_iter()
        .rev()
        .filter(|p| p.is_dir())
        .max_by_key(|p| fs::metadata(p.join(log)).and_then(|m| m.modified()).ok())
}

/// Ryujinx's folder, the one holding its `bis/` and `system/`: Ryubing's Flatpak's, else the
/// native one an AppImage uses, else RetroDECK's or the old Ryujinx Flatpak's.
fn ryujinx_base() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    last_used(
        vec![
            home.join(".var/app/io.github.ryubing.Ryujinx/config/Ryujinx"),
            home.join(".config/Ryujinx"),
            home.join(".var/app").join(crate::launch::RETRODECK).join("config/Ryujinx"),
            home.join(".var/app/org.ryujinx.Ryujinx/config/Ryujinx"),
        ],
        "Logs",
    )
}

/// The user Ryujinx last opened, from its `system/Profiles.json` — whose saves a game's are where
/// there are several users. None with one user, whom sigil takes without being told.
fn ryujinx_profile(base: &Path) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(base.join("system/Profiles.json")).ok()?).ok()?;
    let users: Vec<&str> = json.get("profiles")?.as_array()?.iter().filter_map(|p| p.get("user_id")?.as_str()).collect();
    let last = json.get("last_opened")?.as_str()?;
    (users.len() > 1 && users.contains(&last)).then(|| last.to_string())
}

/// Eden's data folder, the one holding its `keys/` and, unless it was told otherwise, its `nand/`.
fn eden_data() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    last_used(vec![home.join(".var/app/dev.eden_emu.eden/data/eden"), home.join(".local/share/eden")], "log")
}

/// Eden's `qt-config.ini` for its data folder `data`: the Flatpak's own, else the native one.
fn eden_config(data: &Path) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let flatpak = home.join(".var/app/dev.eden_emu.eden");
    if data.starts_with(&flatpak) { flatpak.join("config/eden/qt-config.ini") } else { home.join(".config/eden/qt-config.ini") }
}

/// The folder holding the `nand/` of the Eden whose data folder is `data`, as its settings put it.
pub(crate) fn eden_nand_root(data: &Path) -> Result<PathBuf, String> {
    eden_root(data, Some(&eden_config(data)))
}

/// The folder holding Eden's `nand/`: the one its settings name, else its data folder's. A settings
/// file that isn't there names none.
fn eden_root(data: &Path, config: Option<&Path>) -> Result<PathBuf, String> {
    let nand = config.and_then(|c| ini_value(c, "nand_directory")).map_or_else(|| data.join("nand"), PathBuf::from);
    match (nand.file_name().and_then(|n| n.to_str()), nand.parent()) {
        (Some("nand"), Some(parent)) => Ok(parent.to_path_buf()),
        _ => Err(format!("Eden's NAND is in {}, a folder not named nand, which sigil can't take", nand.display())),
    }
}

/// The user's `prod.keys` where the Switch emulator keeps them, which reading an NSP or XCI takes.
fn switch_keys(emulator: Emulator) -> Option<PathBuf> {
    match emulator {
        Emulator::Ryujinx => ryujinx_base().map(|base| base.join("system/prod.keys")),
        Emulator::Eden => eden_data().map(|data| data.join("keys/prod.keys")),
        _ => None,
    }
    .filter(|keys| keys.is_file())
}

// ---- Matching a PS3 save's title to the game ----------------------------------------------------

/// A title as a PS3 save's `PARAM.SFO` and the library are compared: lowercase, no `(…)` tags, every
/// run of anything else one space — the Kotlin app's `normalizeTitle`.
pub fn normalize(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in title.to_lowercase().chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if depth == 0 && c.is_ascii_alphanumeric() => out.push(c),
            _ if depth == 0 => out.push(' '),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a save's title may equal for this game: its title, and its file's name.
fn title_keys(title: &str, rom: &Path) -> Vec<String> {
    let stem = rom.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    [title, &stem].iter().map(|t| normalize(t)).filter(|k| !k.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Connection;
    use crate::romm::RommClient;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deckhand-standalone-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_gamecube_id_comes_out_of_every_disc_shape() {
        let dir = temp("discs");
        let mut iso = vec![0u8; 0x100];
        iso[..6].copy_from_slice(b"GALE01");
        iso[0x1C..0x20].copy_from_slice(&[0xC2, 0x33, 0x9F, 0x3D]);
        fs::write(dir.join("melee.iso"), &iso).unwrap();
        assert_eq!(gamecube_id(&dir.join("melee.iso")).as_deref(), Some("GALE01"));

        let mut rvz = vec![0u8; 0x100];
        rvz[..4].copy_from_slice(b"RVZ\x01");
        rvz[0x58..0x5E].copy_from_slice(b"GZLP01");
        fs::write(dir.join("zelda.rvz"), &rvz).unwrap();
        assert_eq!(gamecube_id(&dir.join("zelda.rvz")).as_deref(), Some("GZLP01"));

        let mut ciso = vec![0u8; 0x8100];
        ciso[..4].copy_from_slice(b"CISO");
        ciso[0x8000..0x8006].copy_from_slice(b"G8ME01");
        ciso[0x801C..0x8020].copy_from_slice(&[0xC2, 0x33, 0x9F, 0x3D]);
        fs::write(dir.join("ttyd.ciso"), &ciso).unwrap();
        assert_eq!(gamecube_id(&dir.join("ttyd.ciso")).as_deref(), Some("G8ME01"));

        fs::write(dir.join("not.iso"), vec![0u8; 0x100]).unwrap();
        assert_eq!(gamecube_id(&dir.join("not.iso")), None, "no disc magic, no id");
        assert_eq!(gamecube_region("GZLP01"), "EUR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wii_id_comes_out_of_iso_rvz_and_wbfs_and_a_gamecube_disc_is_not_one() {
        let dir = temp("wii");
        let wii = |offset: usize, prefix: &[u8], id: &[u8]| {
            let mut bytes = vec![0u8; 0x400];
            bytes[..prefix.len()].copy_from_slice(prefix);
            bytes[offset..offset + 6].copy_from_slice(id);
            bytes[offset + 0x18..offset + 0x1C].copy_from_slice(&[0x5D, 0x1C, 0x9E, 0xA3]);
            bytes
        };
        fs::write(dir.join("a.iso"), wii(0, b"", b"RMGE01")).unwrap();
        fs::write(dir.join("b.rvz"), wii(0x58, b"RVZ\x01", b"SMNP01")).unwrap();
        fs::write(dir.join("c.wbfs"), wii(0x200, b"WBFS", b"RSBE01")).unwrap();
        assert_eq!(wii_id(&dir.join("a.iso")).as_deref(), Some("RMGE01"));
        assert_eq!(wii_id(&dir.join("b.rvz")).as_deref(), Some("SMNP01"));
        assert_eq!(wii_id(&dir.join("c.wbfs")).as_deref(), Some("RSBE01"));

        let mut gc = vec![0u8; 0x100];
        gc[..6].copy_from_slice(b"GALE01");
        gc[0x1C..0x20].copy_from_slice(&[0xC2, 0x33, 0x9F, 0x3D]);
        fs::write(dir.join("melee.iso"), &gc).unwrap();
        assert_eq!(wii_id(&dir.join("melee.iso")), None);
        // Mario Galaxy's save folder is its game code in hex.
        let title: String = "RMGE"[..4].bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(title, "524d4745");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_ps3_save_names_its_game_in_its_param_sfo() {
        // A real layout: one TITLE entry, UTF-8 with its terminator.
        let key = b"TITLE\0";
        let value = "UFC® Undisputed™ 3\0".as_bytes();
        let keys = 0x14 + 16;
        let data = keys + key.len();
        let mut sfo = vec![0u8; data + value.len()];
        sfo[..4].copy_from_slice(b"\0PSF");
        sfo[8..12].copy_from_slice(&(keys as u32).to_le_bytes());
        sfo[12..16].copy_from_slice(&(data as u32).to_le_bytes());
        sfo[16..20].copy_from_slice(&1u32.to_le_bytes());
        sfo[0x14..0x16].copy_from_slice(&0u16.to_le_bytes());
        sfo[0x16..0x18].copy_from_slice(&0x0204u16.to_le_bytes());
        sfo[0x18..0x1C].copy_from_slice(&(value.len() as u32).to_le_bytes());
        sfo[0x20..0x24].copy_from_slice(&0u32.to_le_bytes());
        sfo[keys..data].copy_from_slice(key);
        sfo[data..].copy_from_slice(value);
        let dir = temp("sfo");
        fs::write(dir.join("PARAM.SFO"), &sfo).unwrap();
        let title = sfo_title(&dir.join("PARAM.SFO")).unwrap();
        assert_eq!(title, "UFC® Undisputed™ 3");
        assert_eq!(normalize(&title), "ufc undisputed 3", "the marks fold away like any punctuation");
        fs::write(dir.join("bad.SFO"), b"\0PSFshort").unwrap();
        assert_eq!(sfo_title(&dir.join("bad.SFO")), None);
        assert_eq!(title_id("BLUS30738HLRQUE_01"), Some("BLUS30738"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_3ds_program_id_comes_out_of_an_ncsd_or_a_bare_ncch() {
        let dir = temp("3ds");
        let ncch = |id: u64| {
            let mut b = vec![0u8; 0x200];
            b[0x100..0x104].copy_from_slice(b"NCCH");
            b[0x118..0x120].copy_from_slice(&id.to_le_bytes());
            b
        };
        let pokemon = 0x0004_0000_0016_4800u64;
        fs::write(dir.join("game.cxi"), ncch(pokemon)).unwrap();
        assert_eq!(program_id(&dir.join("game.cxi")), Some(pokemon));

        let mut cci = vec![0u8; 0x4000 + 0x200];
        cci[0x100..0x104].copy_from_slice(b"NCSD");
        cci[0x120..0x124].copy_from_slice(&0x20u32.to_le_bytes()); // partition 0 at 0x20 * 0x200
        cci[0x4000..].copy_from_slice(&ncch(pokemon));
        fs::write(dir.join("game.3ds"), &cci).unwrap();
        assert_eq!(program_id(&dir.join("game.3ds")), Some(pokemon));
        fs::write(dir.join("game.cia"), vec![0u8; 0x400]).unwrap();
        assert_eq!(program_id(&dir.join("game.cia")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wii_u_title_id_comes_from_the_games_meta_xml() {
        let dir = temp("wiiu");
        fs::create_dir_all(dir.join("Game/meta")).unwrap();
        fs::create_dir_all(dir.join("Game/code")).unwrap();
        fs::write(
            dir.join("Game/meta/meta.xml"),
            r#"<?xml version="1.0"?><menu><title_id type="hexBinary" length="8">0005000010101C00</title_id></menu>"#,
        )
        .unwrap();
        assert_eq!(wiiu_title_id(&dir.join("Game")).as_deref(), Some("0005000010101c00"));
        assert_eq!(wiiu_title_id(&dir.join("Game/code/game.rpx")).as_deref(), Some("0005000010101c00"));
        assert_eq!(wiiu_title_id(&dir.join("elsewhere.wua")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn serials_come_out_of_folder_names_in_both_spellings() {
        assert_eq!(ps2_serial("BASLUS-20946GTA50000").as_deref(), Some("SLUS-20946"));
        assert_eq!(ps2_serial("BESLES-50330KH").as_deref(), Some("SLES-50330"));
        assert_eq!(ps2_serial("_pcsx2_superblock"), None);
        assert_eq!(psp_serial("ULUS10041DATA00").as_deref(), Some("ULUS-10041"));
        assert_eq!(psp_serial("SYSTEM"), None);
    }

    #[test]
    fn titles_compare_without_tags_or_punctuation() {
        assert_eq!(normalize("Grand Theft Auto: San Andreas (USA) (v1.03)"), "grand theft auto san andreas");
        assert_eq!(normalize("Grand Theft Auto - San Andreas"), "grand theft auto san andreas");
    }

    #[test]
    fn a_save_folder_or_state_is_a_games_by_the_serial_on_its_disc() {
        let san_andreas = ["SLUS-20946".to_string()];
        assert!(of_serials(Emulator::Pcsx2, "BASLUS-20946GTA50000", &san_andreas));
        assert!(of_serials(Emulator::Pcsx2, "BESLUS-20946GTA50000", &san_andreas), "whatever region prefix the card gave it");
        assert!(of_serials(Emulator::Pcsx2, "SLUS-20946 (2C6BE434).01.p2s", &san_andreas), "a state");
        assert!(!of_serials(Emulator::Pcsx2, "BASLUS-20947GTA50000", &san_andreas));
        let dj_max = ["ULUS10538".to_string()];
        assert!(of_serials(Emulator::Ppsspp, "ULUS10538", &dj_max));
        assert!(of_serials(Emulator::Ppsspp, "ULUS10538DATA00", &dj_max));
        assert!(of_serials(Emulator::Ppsspp, "ULUS10538_2.00_0.ppst", &dj_max));
        assert!(!of_serials(Emulator::Ppsspp, "ULUS10539", &dj_max));
        let ape_escape = ["SCUS-94423".to_string()];
        assert!(of_serials(Emulator::DuckStation, "SCUS-94423_1.sav", &ape_escape));
        assert!(of_serials(Emulator::DuckStation, "SCUS-94423_resume.sav", &ape_escape));
        assert!(!of_serials(Emulator::DuckStation, "SCUS-94424_1.sav", &ape_escape));
    }

    #[test]
    fn a_multi_disc_playlist_is_every_disc_it_lists() {
        let dir = temp("m3u");
        fs::write(dir.join("Game.m3u"), "# discs\nGame (Disc 1).chd\n\nGame (Disc 2).chd\n").unwrap();
        assert_eq!(discs(&dir.join("Game.m3u")), [dir.join("Game (Disc 1).chd"), dir.join("Game (Disc 2).chd")]);
        assert_eq!(entry_point(&dir.join("Game.m3u")), dir.join("Game (Disc 1).chd"));
        assert_eq!(discs(&dir.join("Game.chd")), [dir.join("Game.chd")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_survives_a_zip_round_trip_and_a_hostile_entry_is_refused() {
        let dir = temp("zip");
        let set = vec![("a/ICON0.PNG".to_string(), b"icon".to_vec()), ("PARAM.SFO".to_string(), b"sfo".to_vec())];
        let zipped = pack(&set).unwrap();
        let target = dir.join("ULUS10041DATA00");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale"), "old").unwrap();
        let back = unpack(&zipped, &target, true, &[]).unwrap();
        assert_eq!(set_hash(&back), set_hash(&set));
        assert!(!target.join("stale").exists(), "a folder set is replaced whole");
        assert_eq!(fs::read(target.join("a/ICON0.PNG")).unwrap(), b"icon");

        let mut evil = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        evil.start_file("../escape.gci", zip::write::SimpleFileOptions::default()).unwrap();
        evil.write_all(b"x").unwrap();
        let evil = evil.finish().unwrap().into_inner();
        assert!(unpack(&evil, &dir.join("card"), false, &[]).is_err());
        assert!(!dir.join("escape.gci").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_flat_set_taken_by_a_device_without_it_is_then_on_that_device() {
        let dir = temp("flat-first-pull");
        let card = dir.join("USA/Card A");
        fs::create_dir_all(&card).unwrap();
        // Nothing of this game on the card yet, so the set is built from no files at all.
        let bundle = Bundle {
            asset: "GALE01.gci.zip".into(),
            key: dir.join("GALE01.gci.zip"),
            files: Vec::new(),
            target: card.clone(),
            folder: false,
        };
        let zipped = pack(&[("01-GALE-save.gci".to_string(), b"progress".to_vec())]).unwrap();
        let host = crate::stub_server::start_bytes(vec![("GET /api/raw/assets/saves/40.zip", 200, zipped)]).await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let remote: SaveAsset = serde_json::from_value(serde_json::json!({
            "id": 40, "rom_id": 1, "file_name": "GALE01.gci [2026-07-01_10-00-00].zip",
            "updated_at": "2026-07-01T10:00:00Z", "download_path": "/api/raw/assets/saves/40.zip",
        }))
        .unwrap();
        let store = dir.join("store");
        let client = RommClient::new().unwrap();
        let mut run = Run::with_index(&client, &conn, &store, card.clone(), "standalone-index.json");
        assert!(run.pull_bundle(&bundle, 1, &remote, "GALE01.gci.zip").await, "{:?}", run.report);

        assert_eq!(fs::read(card.join("01-GALE-save.gci")).unwrap(), b"progress");
        // Against the file it landed in, not the none it was built from — otherwise the Saves page
        // would say the set it has just taken is not on this device.
        assert_eq!(super::super::on_device(&store), [40].into_iter().collect());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flat_sets_file_romm_no_longer_has_goes_with_the_pull_and_another_games_stays() {
        let dir = temp("flat-stale");
        let card = dir.join("Card A");
        fs::create_dir_all(&card).unwrap();
        // This game's two saves, and another game's on the card they share.
        for name in ["01-GALE-one.gci", "01-GALE-two.gci"] {
            fs::write(card.join(name), "ours").unwrap();
        }
        fs::write(card.join("01-GMSE-theirs.gci"), "another game's").unwrap();
        let mine: Vec<(String, PathBuf)> =
            ["01-GALE-one.gci", "01-GALE-two.gci"].into_iter().map(|n| (n.to_string(), card.join(n))).collect();

        // RomM's copy of the set has only the first: the second was deleted where it was pushed from.
        let zipped = pack(&[("01-GALE-one.gci".to_string(), b"newer".to_vec())]).unwrap();
        let set = unpack(&zipped, &card, false, &mine).unwrap();

        assert_eq!(set.len(), 1);
        assert_eq!(fs::read(card.join("01-GALE-one.gci")).unwrap(), b"newer");
        assert!(!card.join("01-GALE-two.gci").exists(), "gone from the set, so gone from the card");
        assert!(card.join("01-GMSE-theirs.gci").is_file(), "another game's save is not this set's to touch");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_apps_own_working_files_are_not_part_of_a_set() {
        let dir = temp("walk");
        let target = dir.join("ULUS10041DATA00");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("DATA.BIN"), "save").unwrap();
        // A write that failed part way, and a copy a file syncer left beside the save.
        fs::write(target.join("DATA.BIN.deckhand-part"), "half").unwrap();
        fs::write(target.join("DATA.sync-conflict-20260101.BIN"), "someone else's").unwrap();
        let found: Vec<String> = walk(&target).into_iter().map(|(name, _)| name).collect();
        assert_eq!(found, ["DATA.BIN"], "only the save itself, so the set hashes the same everywhere");
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_set_goes_up_as_one_zip_and_comes_down_on_a_device_without_it() {
        let dir = temp("e2e");
        let card = dir.join("USA/Card A");
        fs::create_dir_all(&card).unwrap();
        let mut gci = vec![0u8; 0x40];
        gci[..6].copy_from_slice(b"GALE01");
        fs::write(card.join("01-GALE-SuperSmashBros0110290334.gci"), &gci).unwrap();
        let bundle = Bundle {
            asset: "GALE01.gci.zip".into(),
            key: dir.join("GALE01.gci.zip"),
            files: vec![("01-GALE-SuperSmashBros0110290334.gci".into(), card.join("01-GALE-SuperSmashBros0110290334.gci"))],
            target: card.clone(),
            folder: false,
        };
        let created = serde_json::json!({
            "id": 40, "rom_id": 1, "file_name": "GALE01.gci [2026-07-01_10-00-00].zip",
            "updated_at": "2026-07-01T10:00:00Z", "download_path": "/api/raw/assets/saves/40.zip",
        });
        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/saves", 201, created.to_string())]).await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let client = RommClient::new().unwrap();
        let store = dir.join("store");

        let mut run = Run::with_index(&client, &conn, &store, card.clone(), "standalone-index.json");
        run.bundle(&bundle, 1, "dolphin", &[]).await;
        assert_eq!(run.report.pushed, ["GALE01.gci.zip"], "{:?}", run.report);
        let upload = requests.lock().unwrap().iter().find(|l| l.starts_with("POST")).cloned().unwrap();
        assert!(upload.contains("emulator=dolphin"), "{upload}");

        // The same set, unchanged, against RomM's record of it: in sync.
        let server: SaveAsset = serde_json::from_value(created.clone()).unwrap();
        let mut run = Run::with_index(&client, &conn, &store, card.clone(), "standalone-index.json");
        run.bundle(&bundle, 1, "dolphin", std::slice::from_ref(&server)).await;
        assert_eq!(run.report.in_sync, 1, "{:?}", run.report);

        // A second device, no card yet: what went up lands on its card, flat. (Unpacked directly — the
        // stub server answers text, and a zip is not.)
        let zipped = pack(&read_set(&bundle.files).unwrap()).unwrap();
        let other = dir.join("other/USA/Card A");
        let set = unpack(&zipped, &other, false, &[]).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(fs::read(other.join("01-GALE-SuperSmashBros0110290334.gci")).unwrap(), gci, "lands on the card, flat");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dolphin_state_folder_gives_up_only_the_games_own_slots() {
        let dir = temp("dolphin-states");
        for name in ["GALE01.s01", "GALE01.s07", "GALE01.s01.dtm", "GALJ01.s01", "GMSE01.s02", "lastState.sav"] {
            fs::write(dir.join(name), b"state").unwrap();
        }
        let (folder, ours, id) = states_named(Emulator::Dolphin, dir.clone(), "GALE01");
        assert_eq!(folder, dir);
        assert_eq!(ours, ["GALE01.s01", "GALE01.s07"], "this disc's slots, not another region's or another game's");
        assert_eq!(id.as_deref(), Some("GALE01"));
        // Dolphin makes the folder only when it first saves a state: none yet is no states yet.
        let (_, none, _) = states_named(Emulator::Dolphin, dir.join("StateSaves"), "GALE01");
        assert!(none.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_dolphin_state_goes_up_under_its_own_name_as_dolphins() {
        let dir = temp("dolphin-push");
        fs::write(dir.join("GALE01.s03"), b"a dolphin state").unwrap();
        let created = serde_json::json!({
            "id": 77, "rom_id": 1, "file_name": "GALE01.s03", "emulator": "dolphin",
            "updated_at": "2026-07-01T10:00:00Z", "download_path": "/api/raw/assets/states/77.s03",
        });
        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/states", 201, created.to_string())]).await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let client = RommClient::new().unwrap();
        let mut run = Run::with_index(&client, &conn, &dir.join("store"), dir.clone(), "state-index.json");
        run.state_file(dir.join("GALE01.s03"), None, 1, Emulator::Dolphin.tag()).await;
        assert_eq!(run.report.pushed.len(), 1, "{:?}", run.report);
        let upload = requests.lock().unwrap().iter().find(|l| l.starts_with("POST")).cloned().unwrap();
        assert!(upload.contains("emulator=dolphin") && upload.contains("rom_id=1"), "{upload}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_state_is_known_by_its_exact_shape_and_a_backup_is_not_one() {
        assert!(is_state_of(Emulator::Pcsx2, "SLUS-20946 (2C6BE434).01.p2s"));
        assert!(!is_state_of(Emulator::Pcsx2, "SLUS-20946 (399A49CA).01.p2s.backup"));
        assert!(!is_state_of(Emulator::Pcsx2, "SLUS-20946.p2s"));
        assert!(is_state_of(Emulator::Ppsspp, "ULUS10538_2.00_0.ppst"));
        assert!(!is_state_of(Emulator::Ppsspp, "ULUS10538_2.00_0.jpg"));
        assert!(!is_state_of(Emulator::Ppsspp, "ULUS10538_2.00_0.sync-conflict-x.ppst"));
        assert!(is_state_of(Emulator::Dolphin, "GALE01.s01"));
        assert!(is_state_of(Emulator::DolphinWii, "RMGE01.s10"));
        assert!(!is_state_of(Emulator::Dolphin, "GALE01.s01.dtm"), "a recording beside a state is not one");
        assert!(!is_state_of(Emulator::Dolphin, "lastState.sav"), "nor the state Dolphin keeps to undo a load");
        assert!(!is_state_of(Emulator::Dolphin, "GALE01.s00"));
        assert!(!is_state_of(Emulator::Dolphin, "gale01.s01"));
        assert!(is_state_of(Emulator::DuckStation, "SCUS-94423_1.sav"));
        assert!(is_state_of(Emulator::DuckStation, "SCUS-94423_10.sav"));
        assert!(is_state_of(Emulator::DuckStation, "SCUS-94423_resume.sav"), "the state saved on exit");
        assert!(!is_state_of(Emulator::DuckStation, "SCUS-94423_1.sav.backup"));
        assert!(!is_state_of(Emulator::DuckStation, "savestate_1.sav"), "a state of no game in particular");
        assert!(!is_state_of(Emulator::DuckStation, "resume.sav"));
        assert!(!is_state_of(Emulator::DuckStation, "Ape Escape (USA).state"), "RetroArch's, not DuckStation's");
        assert!(!is_state_of(Emulator::DuckStation, "SCUS-94423_1.sync-conflict-x.sav"));
    }

    #[test]
    fn duckstations_states_are_where_its_settings_say() {
        let dir = temp("duckstation-states");
        let ini = dir.join("settings.ini");
        fs::write(&ini, "[Main]\nSaveStateOnExit = true\n").unwrap();
        assert_eq!(duckstation_states(&ini), dir.join("savestates"), "beside the settings, by default");
        fs::write(&ini, "[Main]\nSaveStateOnExit = true\n[Folders]\nSaveStates = states\n").unwrap();
        assert_eq!(duckstation_states(&ini), dir.join("states"));
        fs::write(&ini, "[Folders]\nSaveStates = /media/states/psx\n").unwrap();
        assert_eq!(duckstation_states(&ini), PathBuf::from("/media/states/psx"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_launch_label_picks_the_emulator_only_for_its_own_system() {
        assert_eq!(Emulator::for_launch("gc", "Dolphin (Standalone)"), Some(Emulator::Dolphin));
        assert_eq!(Emulator::for_launch("wii", "Dolphin (Standalone)"), Some(Emulator::DolphinWii));
        assert_eq!(Emulator::for_launch("ps2", "PCSX2 (Standalone)"), Some(Emulator::Pcsx2));
        assert_eq!(Emulator::for_launch("psp", "PPSSPP (Standalone)"), Some(Emulator::Ppsspp));
        assert_eq!(Emulator::for_launch("ps3", "RPCS3 Shortcut (Standalone)"), Some(Emulator::Rpcs3));
        assert_eq!(Emulator::for_launch("psvita", "Vita3K (Standalone)"), Some(Emulator::Vita3k));
        assert_eq!(Emulator::for_launch("switch", "Ryujinx (Standalone)"), Some(Emulator::Ryujinx));
        assert_eq!(Emulator::for_launch("switch", "Ryubing (Standalone)"), Some(Emulator::Ryujinx));
        assert_eq!(Emulator::for_launch("switch", "io.github.ryubing.Ryujinx"), Some(Emulator::Ryujinx));
        assert_eq!(Emulator::for_launch("switch", "Eden (Standalone)"), Some(Emulator::Eden));
        assert_eq!(Emulator::for_launch("switch", "dev.eden_emu.eden"), Some(Emulator::Eden));
        assert_eq!(Emulator::for_launch("switch", "Citron (Standalone)"), None);
        assert_eq!(Emulator::for_launch("psx", "Eden (Standalone)"), None);
    }

    #[test]
    fn the_install_that_ran_last_is_the_one_whose_saves_sync() {
        let dir = temp("last-used");
        let (flatpak, native, gone) = (dir.join("flatpak"), dir.join("native"), dir.join("gone"));
        fs::create_dir_all(flatpak.join("Logs")).unwrap();
        fs::create_dir_all(native.join("Logs")).unwrap();
        let candidates = || vec![flatpak.clone(), native.clone(), gone.clone()];
        let at = |p: &Path, secs: u64| {
            fs::File::open(p).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        };
        at(&flatpak.join("Logs"), 2_000_000_000);
        at(&native.join("Logs"), 1_000_000_000);
        assert_eq!(last_used(candidates(), "Logs"), Some(flatpak.clone()));
        at(&native.join("Logs"), 2_100_000_000);
        assert_eq!(last_used(candidates(), "Logs"), Some(native.clone()), "the AppImage ran since");
        assert_eq!(last_used(candidates(), "log"), Some(flatpak.clone()), "neither logged: the first that is here");
        assert_eq!(last_used(vec![gone.clone()], "Logs"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edens_nand_is_where_its_settings_put_it() {
        let dir = temp("eden");
        let data = dir.join("eden");
        assert_eq!(eden_root(&data, None), Ok(data.clone()), "beside its keys, by default");
        assert_eq!(eden_root(&data, Some(&dir.join("qt-config.ini"))), Ok(data.clone()), "nor set up yet");
        let ini = dir.join("qt-config.ini");
        fs::write(&ini, "[Data%20Storage]\nnand_directory\\default=false\nnand_directory=/run/media/sd/switch/nand/\n").unwrap();
        assert_eq!(eden_root(&data, Some(&ini)), Ok(PathBuf::from("/run/media/sd/switch")));
        fs::write(&ini, "[Data%20Storage]\nnand_directory=/run/media/sd/switch-nand\n").unwrap();
        assert!(eden_root(&data, Some(&ini)).is_err(), "sigil finds the NAND by its name");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ryujinx_names_its_last_user_only_where_there_are_several() {
        let base = temp("ryujinx");
        fs::create_dir_all(base.join("system")).unwrap();
        let profiles = |users: &[&str], last: &str| {
            let profiles: Vec<serde_json::Value> = users.iter().map(|u| serde_json::json!({ "user_id": u, "name": "Player" })).collect();
            fs::write(base.join("system/Profiles.json"), serde_json::json!({ "profiles": profiles, "last_opened": last }).to_string()).unwrap();
        };
        let (ryu, guest) = ("00000000000000010000000000000000", "7A4C6C1D2E3F40518293A4B5C6D7E8F9");
        profiles(&[ryu], ryu);
        assert_eq!(ryujinx_profile(&base), None, "one user, whom sigil takes by itself");
        profiles(&[ryu, guest], guest);
        assert_eq!(ryujinx_profile(&base).as_deref(), Some(guest));
        profiles(&[ryu, guest], "00000000000000000000000000000099");
        assert_eq!(ryujinx_profile(&base), None, "a user since removed is no one's");
        let _ = fs::remove_dir_all(&base);
    }
}

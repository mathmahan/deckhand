//! Save sync through RomM's own sync protocol: sessions negotiated with the server, which is where
//! every device's progress meets.
//!
//! **RomM decides what moves.** This device says what it holds — each save's game, slot, content
//! hash, time and size (`POST /api/sync/negotiate`) — and RomM answers per save: *upload*,
//! *download*, *conflict* or *no-op*, from what it holds and from what it last saw this device sync.
//! The transfers that follow are made inside the session RomM opened for them and as this device, so
//! RomM's own record of who holds what stays true, and the session is closed with what came of it and
//! with the play session it ends (`POST /api/sync/sessions/{id}/complete`). Saves are paired on
//! (game, slot) — the same pairing RomM's own web player, Argosy and Grout use, so a save made in any
//! of them comes down here, and the other way.
//!
//! Four things are this engine's own, on top of what RomM says:
//!
//! - **A save goes on in the slot its newest version is in** ([`follow`]). Every save starts in
//!   `autosave`, but a client may keep a game's save in a slot of its own, and RomM's web player goes
//!   on in whichever slot holds the game's newest save. Each slot is a history of its own to RomM, so
//!   a save left in `autosave` would never see that progress: it goes where the newest version is,
//!   and stays there until another slot has a newer one.
//!
//! - **Hashes are RomM's to the bit** ([`super::hash`]): a save that is the same on both sides is a
//!   no-op whatever its timestamps say, and moves no data.
//! - **This device's memory of the last agreement is a second opinion.** RomM 5.3 weighs a save's
//!   time against when this device last synced the slot's *current* version — so a version another
//!   device pushed since is judged by clocks alone, and progress made here offline could be
//!   overwritten by a download, or pushed over a version it never saw. The index ([`super::index`])
//!   remembers which content and which RomM save each local save last agreed on, and an answer that
//!   would lose something becomes the conflict it is ([`decide`]). The times reported are translated
//!   the same way ([`reported_time`]): content unchanged since the last agreement is reported as no
//!   newer than it, changed content as newer, so RomM's clock rules reach the content's answer even
//!   when this device's clock is off.
//! - **Nothing is lost.** A local copy is set aside in `save-backups/` before anything overwrites or
//!   removes it, unless the bytes it replaces are a version RomM holds in the slot's history. A push
//!   never goes over a version this device has not seen unless a person chose that — RomM's refusal
//!   (409) is settled as a conflict, not retried over the top. A download is checked against the hash
//!   RomM described before it is written, and RomM is told this device holds it only once it is in
//!   place. A save RomM says to remove is removed only when what is here is what the last agreement
//!   recorded: anything else holds progress RomM's own copy did not, and stays.
//!
//! RomM pairs one save per slot, and judges it by the slot's newest version — but a game's `autosave`
//! slot can hold several different saves: a PSP game's save folders, a Saturn game's three files, a
//! PS1 memory card one client keeps as `.srm` and another as `.mcd`. Every client of this library
//! writes them there. So an answer counts only when RomM's file is this save's ([`same_save`]); a save
//! whose slot's newest version is another save's is settled by name against the slot's versions
//! instead ([`decide`]'s `by_name`), the same content rules, one request for every such game in the
//! negotiation. And a push into a slot other saves share keeps RomM's history-trimming off, since the
//! trimming counts versions across the whole slot and would drop another save's only copy.
//!
//! Save states have no negotiation in RomM 5.3 (they carry no slot and no hash), so they keep the
//! engine in [`super::states`]; and a RomM without the protocol keeps the engine before this one
//! ([`Outcome::Unsupported`]).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::index::{Entry, Index};
use super::standalone::{self, Bundle};
use super::units::Unit;
use super::{Choice, Report, hash, names};
use crate::config::Connection;
use crate::library::Stamp;
use crate::romm::models::{ClientSave, PlaySession, SaveAsset, SyncOperation};
use crate::romm::{ApiError, Push, Pushed, RommClient};

/// The slot a game's rolling save starts in — RomM's own convention, which its web player, Argosy,
/// Grout and this app all keep to. It goes on in another when RomM's newest version of it is there —
/// see [`follow`].
pub const AUTOSAVE: &str = "autosave";

/// The most games one negotiation can be scoped to.
const SCOPE_LIMIT: usize = 500;

/// One save as RomM pairs it — a game's slot — and where it lives on this device.
#[derive(Debug, Clone)]
pub struct Thread {
    pub rom_id: i64,
    /// Which rom that is, as the library has it now — see [`Stamp`]. What the save's history is held
    /// to, and recorded with. None where nothing said.
    pub stamp: Option<Stamp>,
    pub slot: String,
    /// The name it goes up under: `Chrono Trigger (USA).srm`, `GALE01.gci.zip`.
    pub name: String,
    /// RomM's `emulator` for it: the core, or the standalone emulator.
    pub emulator: String,
    pub place: Place,
    /// Other saves of the game live in its slot too — one of several save folders. Pushed without
    /// RomM's history-trimming, which counts the whole slot.
    pub shared: bool,
}

impl Thread {
    /// Whether any of `changed` is this save: its file, one of its set's, or — for a set kept as a
    /// folder of its own — anything in that folder.
    pub fn covers(&self, changed: &[PathBuf]) -> bool {
        match &self.place {
            Place::File(path) => changed.contains(path),
            Place::Set(bundle) => changed
                .iter()
                .any(|p| (bundle.folder && p.starts_with(&bundle.target)) || bundle.files.iter().any(|(_, file)| file == p)),
            Place::Sigil(unit) => unit.covers(changed),
        }
    }
}

/// Where a [`Thread`] lives.
#[derive(Debug, Clone)]
pub enum Place {
    /// One file — a RetroArch battery save. It need not exist yet: it is also where one from RomM
    /// goes.
    File(PathBuf),
    /// A standalone emulator's set of files, which travels as one zip — the shape before sigil.
    Set(Bundle),
    /// A game's saves as sigil keeps them — see [`super::units`].
    Sigil(Box<Unit>),
}

/// A save as it is on this device right now.
#[derive(Debug, Clone)]
struct Held {
    /// RomM's hash of it.
    hash: String,
    /// A set's hash the way the engine before this one took it — what an index entry from then holds.
    legacy: Option<String>,
    /// For a sigil unit that is one file of its own, RomM's hash of the file as it is on disk. sigil
    /// sends some files in a form of its own — a PS1 card with its free blocks zeroed, where
    /// DuckStation leaves them 0xFF — so the unit's hash is not the file's, and an agreement this
    /// device or another client recorded with the file as it was is still this content.
    raw: Option<String>,
    bytes: u64,
    modified: Option<SystemTime>,
}

impl Held {
    /// Whether this is the content RomM hashes `hash`: the unit, or the file it was read from.
    fn is(&self, hash: &str) -> bool {
        self.hash == hash || self.raw.as_deref() == Some(hash)
    }
}

impl Thread {
    fn key(&self) -> PathBuf {
        match &self.place {
            Place::File(path) => path.clone(),
            Place::Set(bundle) => bundle.key.clone(),
            Place::Sigil(unit) => unit.key(),
        }
    }

    /// A unit that is one file of its own keeps the entry a battery save always had; any other is
    /// kept with the sets.
    fn index_file(&self) -> &'static str {
        match &self.place {
            Place::File(_) => "save-index.json",
            Place::Sigil(unit) if unit.file().is_some() => "save-index.json",
            Place::Set(_) | Place::Sigil(_) => "standalone-index.json",
        }
    }

    /// Every file on this device this save is held in — what a sync that writes it changes, for
    /// [`Report::written`].
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        match &self.place {
            Place::File(path) => vec![path.clone()],
            Place::Set(bundle) => bundle.files.iter().map(|(_, path)| path.clone()).collect(),
            Place::Sigil(unit) => unit.paths(),
        }
    }

    /// The files an index entry for this save records as holding it — see [`Entry::files`]. Only a
    /// flat set needs them: a file is its own answer, and a folder set has a folder of its own.
    ///
    /// `written` is what a pull just put on disk, which is what the set is then held in — the files
    /// the thread was built from are the ones that were there before it, and on a device taking the
    /// set for the first time there were none.
    fn set_files(&self, written: Option<&[PathBuf]>) -> Option<Vec<String>> {
        let name = |path: &PathBuf| path.to_string_lossy().into_owned();
        match &self.place {
            Place::Set(bundle) if !bundle.folder => {
                Some(written.map_or_else(|| bundle.files.iter().map(|(_, path)| name(path)).collect(), |paths| paths.iter().map(name).collect()))
            }
            // A unit with no file of its own: what says it is still here is its files being there.
            Place::Sigil(unit) if unit.file().is_none() => Some(match written {
                Some(paths) => paths.iter().filter(|p| p.is_file()).map(name).collect(),
                None => unit.paths().iter().map(name).collect(),
            }),
            _ => None,
        }
    }

    /// What is here, or None when there is nothing.
    fn held(&self) -> Result<Option<Held>, String> {
        match &self.place {
            Place::File(path) => match fs::read(path) {
                Ok(bytes) => Ok(Some(Held { hash: hash::file(&bytes), legacy: None, raw: None, bytes: bytes.len() as u64, modified: modified(path) })),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
            },
            Place::Set(bundle) => {
                let set = standalone::read_set(&bundle.files).map_err(|e| e.to_string())?;
                if set.is_empty() {
                    return Ok(None);
                }
                Ok(Some(Held {
                    hash: hash::set(&set),
                    legacy: Some(standalone::set_hash(&set)),
                    raw: None,
                    bytes: set.iter().map(|(_, b)| b.len() as u64).sum(),
                    modified: bundle.files.iter().filter_map(|(_, p)| modified(p)).max(),
                }))
            }
            Place::Sigil(unit) => Ok(unit.held()?.map(|(hash, bytes, modified)| Held { hash, legacy: None, raw: unit.raw(), bytes, modified })),
        }
    }

    /// What goes up, and RomM's hash of it — read at the moment of the push, so what is sent is what
    /// is there then.
    fn body(&self) -> Result<(Vec<u8>, String), String> {
        match &self.place {
            Place::File(path) => {
                let bytes = fs::read(path).map_err(|e| e.to_string())?;
                let hash = hash::file(&bytes);
                Ok((bytes, hash))
            }
            Place::Set(bundle) => {
                let set = standalone::read_set(&bundle.files).map_err(|e| e.to_string())?;
                let zipped = standalone::pack(&set).map_err(|e| format!("could not zip it ({e})"))?;
                Ok((zipped, hash::set(&set)))
            }
            Place::Sigil(unit) => unit.body(),
        }
    }

    /// Put `bytes` from RomM — its file `server_name` — in place, made at `made` by RomM's clock,
    /// answering the hash of what is here now — what this device's next sync will read — and the
    /// files it was written to.
    fn put(&self, bytes: &[u8], made: Option<SystemTime>, server_name: &str) -> Result<(String, Vec<PathBuf>), String> {
        match &self.place {
            Place::File(path) => {
                super::write_atomic(path, bytes).map_err(|e| e.to_string())?;
                // RomM's time rather than now: a later conflict weighs when the progress was made,
                // not when it was copied here.
                if let Some(time) = made
                    && let Ok(file) = fs::File::options().write(true).open(path)
                {
                    let _ = file.set_modified(time);
                }
                Ok((hash::file(bytes), vec![path.clone()]))
            }
            Place::Set(bundle) => {
                let set = standalone::unpack(bytes, &bundle.target, bundle.folder, &bundle.files)?;
                let written = set.iter().map(|(relative, _)| bundle.target.join(relative)).collect();
                Ok((hash::set(&set), written))
            }
            Place::Sigil(unit) => unit.put(bytes, made, server_name),
        }
    }

    /// The local copy as one blob, for `save-backups/`: the file, or the set zipped.
    fn snapshot(&self) -> Result<Vec<u8>, String> {
        match &self.place {
            Place::File(path) => fs::read(path).map_err(|e| e.to_string()),
            Place::Set(bundle) => {
                let set = standalone::read_set(&bundle.files).map_err(|e| e.to_string())?;
                standalone::pack(&set).map_err(|e| format!("could not zip a copy ({e})"))
            }
            Place::Sigil(unit) => unit.snapshot(),
        }
    }

    /// Take the local copy away — only ever after [`Thread::snapshot`] went into the backups.
    fn clear(&self) -> Result<(), String> {
        match &self.place {
            Place::File(path) => fs::remove_file(path).map_err(|e| e.to_string()),
            Place::Set(bundle) if bundle.folder => fs::remove_dir_all(&bundle.target).map_err(|e| e.to_string()),
            // A flat set shares its card with every other game's saves: only its own files go.
            Place::Set(bundle) => bundle.files.iter().try_for_each(|(_, p)| fs::remove_file(p)).map_err(|e| e.to_string()),
            Place::Sigil(unit) => unit.clear(),
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a sync came to.
#[derive(Debug)]
pub enum Outcome {
    Done(Box<Report>),
    /// This RomM does not speak the protocol, or not with this install — the engine before this one
    /// takes over. Says why.
    Unsupported(String),
}

/// Finds a place for a save RomM offers that no thread names.
pub type Late<'a> = &'a (dyn Fn(&SyncOperation) -> Option<Thread> + Sync);

/// Bring `threads` into step with RomM, through one negotiation per [`SCOPE_LIMIT`] games.
///
/// `scope` is the games on this device beyond those a thread names — RomM offers their saves for
/// download too; `late` finds a place for one no thread names, a save folder this device has none
/// of yet. `plays` are the play sessions this sync ends, closed with its session. An error is a sync
/// that could not start or reach RomM; one save failing is in the [`Report`].
pub async fn sync(
    client: &RommClient,
    conn: &Connection,
    store: &Path,
    threads: Vec<Thread>,
    scope: &[i64],
    plays: Vec<PlaySession>,
    late: Late<'_>,
) -> Result<Outcome, String> {
    let Some(device) = conn.device_id.clone() else {
        return Ok(Outcome::Unsupported("this install is not registered with RomM as a device".into()));
    };
    let mut run = Run::new(client, conn, store, device);
    // Games whose saves go through sigil for the first time: RomM's copies in the shape before are
    // settled first — see [`Run::migrate`].
    let threads = run.migrate(threads).await;

    // What is here, read once: the negotiation states it, and the decisions weigh it.
    let mut local: Vec<(Thread, Option<Held>)> = Vec::with_capacity(threads.len());
    for mut thread in threads {
        run.recall(&mut thread);
        match thread.held() {
            Ok(held) => local.push((thread, held)),
            Err(e) => run.report.failed.push(format!("{}: {e}", thread.name)),
        }
    }

    let mut roms: Vec<i64> = scope.iter().copied().chain(local.iter().map(|(t, _)| t.rom_id)).collect();
    roms.sort_unstable();
    roms.dedup();
    let mut plays = Some(plays);
    let mut homeless = Vec::new();
    for (n, chunk) in roms.chunks(SCOPE_LIMIT).enumerate() {
        let in_chunk = |rom_id: i64| chunk.binary_search(&rom_id).is_ok();
        let sent: Vec<usize> = (0..local.len()).filter(|&i| in_chunk(local[i].0.rom_id) && local[i].1.is_some()).collect();
        let mut saves = Vec::with_capacity(sent.len());
        for &i in &sent {
            let baseline = run.baseline(&local[i].0);
            saves.push(client_save(&local[i].0, local[i].1.as_ref().expect("sent only what is here"), baseline.as_ref()));
        }
        let negotiation = match client.negotiate(conn, &run.device, &saves, chunk).await {
            Ok(negotiation) => negotiation,
            Err(e) => {
                if n == 0 && disabled(&e) {
                    run.report.notes.push("sync is turned off for this device in RomM, so nothing moved".into());
                    return Ok(Outcome::Done(Box::new(run.report)));
                }
                if n == 0 && forgotten(&e) {
                    super::device::forget();
                    return Err("RomM no longer knows this device; it registers again on the next sync".into());
                }
                if n == 0
                    && let Some(why) = unsupported(&e)
                {
                    return Ok(Outcome::Unsupported(why));
                }
                if n == 0 {
                    return Err(format!("Could not negotiate with RomM: {}", super::describe(&e)));
                }
                // Not the first turn of a library too big for one negotiation. The games before this
                // one are synced and their indexes written, so what moved is said rather than thrown
                // away with the error; the rest is left for the next sync.
                if forgotten(&e) {
                    super::device::forget();
                }
                let left = roms.len().saturating_sub(n * SCOPE_LIMIT);
                run.report.notes.push(format!(
                    "{left} more games were left for the next sync: RomM stopped answering ({})",
                    super::describe(&e)
                ));
                break;
            }
        };
        run.session = Some(negotiation.session_id);
        run.counts = (0, 0);

        // Which answer is about which save. RomM answers each save sent, in the order sent, then lists
        // what it has that nothing sent names — checked rather than assumed, and matched by game,
        // slot and file where the order does not hold.
        let ops = negotiation.operations;
        let in_order = ops.len() >= sent.len()
            && sent.iter().zip(&ops).all(|(&i, op)| op.rom_id == local[i].0.rom_id && op.slot.as_deref() == Some(local[i].0.slot.as_str()));
        let (answers, offered) = if in_order { ops.split_at(sent.len()) } else { (&ops[..0], &ops[..]) };
        // Each save goes on in the slot of RomM's newest version of it, which need not be the one it
        // was sent in — see [`follow`].
        let own: HashMap<usize, &SyncOperation> = sent.iter().copied().zip(answers).collect();
        for (i, (thread, _)) in local.iter_mut().enumerate() {
            if !in_chunk(thread.rom_id) {
                continue;
            }
            let baseline = run.baseline(thread);
            if let Some(slot) = follow(thread, baseline.as_ref(), own.get(&i).copied().into_iter().chain(offered)) {
                thread.slot = slot;
            }
        }
        let mut todo: Vec<(usize, Option<SyncOperation>)> = Vec::new();
        let mut decided: HashSet<usize> = HashSet::new();
        for (&i, op) in sent.iter().zip(answers) {
            // A save that goes on in another slot is settled by that slot's answer, below.
            if op.slot.as_deref() != Some(local[i].0.slot.as_str()) {
                continue;
            }
            decided.insert(i);
            // An answer about another save in the slot says nothing about this one.
            let ours = op.save_id.is_none() || same_save(&local[i].0, &op.file_name);
            todo.push((i, ours.then(|| op.clone())));
        }
        for op in offered {
            let slot = op.slot.as_deref().unwrap_or_default();
            let found = local.iter().position(|(t, _)| t.rom_id == op.rom_id && t.slot == slot && same_save(t, &op.file_name));
            match found {
                Some(i) if decided.insert(i) => todo.push((i, Some(op.clone()))),
                Some(_) => {}
                // An older version of a save that goes on in another slot: history, not news.
                None if local.iter().any(|(t, _)| !t.shared && t.rom_id == op.rom_id && t.slot != slot && same_save(t, &op.file_name)) => {}
                // A save RomM has that no thread names: a place for it if there is one.
                None => match late(op).filter(|t| !local.iter().any(|(other, _)| other.key() == t.key())) {
                    Some(thread) => match thread.held() {
                        Ok(held) => {
                            local.push((thread, held));
                            decided.insert(local.len() - 1);
                            todo.push((local.len() - 1, Some(op.clone())));
                        }
                        Err(e) => run.report.failed.push(format!("{}: {e}", thread.name)),
                    },
                    // Nowhere to put it: worth saying when it is this game's kind of save, or the game
                    // has no place here for one at all — not for another client's other files.
                    None => {
                        let mine = local.iter().filter(|(t, _)| t.rom_id == op.rom_id).collect::<Vec<_>>();
                        if op.action == "download" && (mine.is_empty() || mine.iter().any(|(t, _)| same_kind(t, &op.file_name))) {
                            homeless.push(names::canonical(&op.file_name));
                        }
                    }
                },
            }
        }
        // What RomM's answer said nothing about — a place with nothing here yet, whose slot's newest
        // version may be another save's and hide this one's — is looked up by name too.
        for (i, (thread, _)) in local.iter().enumerate() {
            if in_chunk(thread.rom_id) && !decided.contains(&i) {
                todo.push((i, None));
            }
        }

        // The by-name ones, against every version of their games' slots: a request per hundred games,
        // which keeps the address short enough for any proxy in front of RomM.
        let mut by_name: Vec<i64> = todo.iter().filter(|(_, op)| op.is_none()).map(|(i, _)| local[*i].0.rom_id).collect();
        by_name.sort_unstable();
        by_name.dedup();
        let mut versions = Ok(Vec::new());
        for batch in by_name.chunks(100) {
            match client.fetch_saves_of(conn, batch).await {
                Ok(found) => {
                    if let Ok(all) = &mut versions {
                        all.extend(found);
                    }
                }
                Err(e) => versions = Err(super::describe(&e)),
            }
        }
        for (i, op) in todo {
            let (thread, held) = &local[i];
            match (op, &versions) {
                (Some(op), _) => run.apply(&op, thread, held.as_ref(), false).await,
                (None, Ok(versions)) => run.apply(&by_name_op(thread, versions), thread, held.as_ref(), true).await,
                // Nothing here, nothing known: not news. Something here that could not be weighed is.
                (None, Err(_)) if held.is_none() => {}
                (None, Err(why)) => {
                    run.fail(thread, format!("RomM's other saves for it could not be read ({why})"));
                }
            }
        }

        let (done, failed) = run.counts;
        let carried = plays.take().unwrap_or_default();
        match client.complete_sync(conn, negotiation.session_id, done, failed, &carried).await {
            Ok(recorded) => run.report.plays_recorded += recorded,
            Err(e) => run.report.notes.push(format!("RomM's sync session was left open: {}", super::describe(&e))),
        }
    }
    if !homeless.is_empty() {
        homeless.sort();
        homeless.dedup();
        run.report.homeless = homeless;
    }
    prune_backups(&run.store.join("save-backups"));
    Ok(Outcome::Done(Box::new(run.report)))
}

/// Whether the RomM file `server_name` is `thread`'s save, rather than another save of the game in
/// the same slot: a battery save is any of the game's files of its type (another client may have
/// named it after a different build of the same game); a set is the set of its name.
fn same_save(thread: &Thread, server_name: &str) -> bool {
    let name = names::canonical(server_name);
    match &thread.place {
        Place::File(_) => same_kind(thread, &name),
        Place::Set(_) => name.eq_ignore_ascii_case(&thread.name),
        Place::Sigil(unit) => unit.holds(&name),
    }
}

/// The slot `thread`'s save goes on in, when that is not the one it is in: the slot RomM's newest
/// version of it is in, among `ops` — a negotiation's answers.
///
/// Another client may keep a game's save in a slot of its own (a phone app's `battery`), and RomM's
/// web player goes on in whichever slot the game's newest save is in, so going on there too is what
/// keeps every client on one line of progress. The slot it is in wins a tie. Its newest version there
/// is what RomM answered about it, or else — RomM leaves out a version this device has synced and
/// nobody has changed since — the one `baseline` agreed on. A save that shares its slot with the
/// game's others stays put: only clients of sigil write those, and all of them in `autosave`.
///
/// Nothing moves on RomM: the slot left behind keeps its history.
fn follow<'o>(thread: &Thread, baseline: Option<&Entry>, ops: impl IntoIterator<Item = &'o SyncOperation>) -> Option<String> {
    if thread.shared {
        return None;
    }
    let made = |op: &SyncOperation| op.server_updated_at.as_deref().and_then(names::parse_timestamp);
    // Here: None while RomM has said nothing of this slot, Some(None) when it holds none of the save.
    let mut here: Option<Option<SystemTime>> = None;
    let mut elsewhere: Option<(SystemTime, &str)> = None;
    for op in ops.into_iter().filter(|op| op.rom_id == thread.rom_id) {
        let slot = op.slot.as_deref().unwrap_or_default();
        match (slot == thread.slot, op.save_id.is_some() && same_save(thread, &op.file_name)) {
            // RomM holds nothing in this slot.
            (true, _) if op.save_id.is_none() => here = Some(here.flatten()),
            (true, true) => here = Some(here.flatten().max(made(op))),
            (false, true) => {
                if let Some(at) = made(op).filter(|at| elsewhere.is_none_or(|(newest, _)| *at > newest)) {
                    elsewhere = Some((at, slot));
                }
            }
            // Another save's newest: it says nothing of where this one is.
            (_, false) => {}
        }
    }
    let (at, slot) = elsewhere?;
    let here = here.unwrap_or_else(|| {
        baseline.filter(|b| b.slot.as_deref().unwrap_or(AUTOSAVE) == thread.slot).and_then(|b| b.synced_at.as_deref()).and_then(names::parse_timestamp)
    });
    here.is_none_or(|here| at > here).then(|| slot.to_string())
}

/// Put RomM's `version` of `thread`'s save back as the current one, everywhere.
///
/// This device's copy goes into `save-backups/` first, then the version is put in place and sent up
/// again as the slot's newest — past RomM's check that this device holds the newest, since going
/// back on it is the point. Sending it up is what makes it stick: left only on disk, an old version
/// is exactly what the sync reads as this device being behind, and the next sync would take the
/// newest back. Pushed, every device takes it, and this one's index agrees with RomM on it. It goes
/// into the slot the save goes on in, wherever `version` itself was: that is where every client
/// looks for the newest.
pub async fn restore(client: &RommClient, conn: &Connection, store: &Path, thread: &Thread, version: &SaveAsset) -> Result<Report, String> {
    let _one_at_a_time = exclusive(store).await?;
    let mut run = Run::new(client, conn, store, conn.device_id.clone().unwrap_or_default());
    let mut thread = thread.clone();
    run.recall(&mut thread);
    let thread = &thread;
    let failed = |run: &Run| run.report.failed.join("; ");
    if thread.held()?.is_some() && run.set_aside(thread).is_none() {
        return Err(failed(&run));
    }
    let bytes = client.pull_save(conn, version.id, None).await.map_err(|e| super::describe(&e))?;
    if let Some(expected) = version.content_hash.as_deref()
        && hash::of_upload(&bytes) != expected
    {
        return Err("what came down is not the version RomM described, so nothing was changed".into());
    }
    // Made now, not when RomM first had it: it is this device's newest from here on.
    thread.put(&bytes, None, &version.file_name)?;
    run.report.written.extend(thread.paths());
    let held = thread.held()?;
    let op = SyncOperation {
        action: "upload".into(),
        rom_id: thread.rom_id,
        save_id: Some(version.id),
        file_name: thread.name.clone(),
        slot: Some(thread.slot.clone()),
        emulator: Some(thread.emulator.clone()),
        reason: "restored".into(),
        server_updated_at: version.updated_at.clone(),
        server_content_hash: version.content_hash.clone(),
    };
    match run.push(&op, thread, held.as_ref(), true).await {
        Went::Up => {
            run.report.pushed.push(thread.name.clone());
            Ok(run.report)
        }
        _ => Err(failed(&run)),
    }
}

/// Whether `version` — one of RomM's records — is a version of `thread`'s save.
pub fn holds(thread: &Thread, version: &SaveAsset) -> bool {
    version.rom_id.is_none_or(|id| id == thread.rom_id) && same_save(thread, &version.file_name)
}

/// Whether `server_name` is a file of `thread`'s type.
fn same_kind(thread: &Thread, server_name: &str) -> bool {
    if let Place::Sigil(unit) = &thread.place {
        return unit.holds(server_name);
    }
    let ext = |n: &str| n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    ext(&names::canonical(server_name)) == ext(&thread.name)
}

/// RomM's newest version of `thread`'s save among `versions`, as an answer to settle by name.
fn by_name_op(thread: &Thread, versions: &[SaveAsset]) -> SyncOperation {
    let mine: Vec<&SaveAsset> = versions
        .iter()
        .filter(|v| v.rom_id == Some(thread.rom_id) && v.slot.as_deref() == Some(thread.slot.as_str()) && same_save(thread, &v.file_name))
        .collect();
    // A slot can hold several saves of one kind: each disc of a multi-disc set keeps its own `.srm`
    // under its own name, and one game is one rom on RomM. The versions spelled as this save is are
    // this save's history; one that only shares its kind — another build's name for it — stands in
    // just when none of them does, which is what brings a save made in another client down at all.
    let exact: Vec<&SaveAsset> =
        mine.iter().copied().filter(|v| names::canonical(&v.file_name).eq_ignore_ascii_case(&thread.name)).collect();
    let newest = if exact.is_empty() { &mine } else { &exact }
        .iter()
        .copied()
        .max_by_key(|v| (v.updated_at.as_deref().and_then(names::parse_timestamp), v.id));
    SyncOperation {
        action: BY_NAME.into(),
        rom_id: thread.rom_id,
        save_id: newest.map(|v| v.id),
        file_name: newest.map_or_else(|| thread.name.clone(), |v| v.file_name.clone()),
        slot: Some(thread.slot.clone()),
        emulator: newest.and_then(|v| v.emulator.clone()),
        reason: "settled by name: the slot's newest version is another save's".into(),
        server_updated_at: newest.and_then(|v| v.updated_at.clone()),
        server_content_hash: newest.and_then(|v| v.content_hash.clone()),
    }
}

/// The answer this device gives itself for a save it settles by name — see [`by_name_op`].
const BY_NAME: &str = "by_name";

/// What one save as RomM put it needs, from what this device knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The same on both sides. `confirm`: RomM has not been told this device holds its version.
    Agree { confirm: bool },
    Upload,
    Download,
    Conflict,
    /// RomM removed this version (a server after 5.3 says `delete`): set the local copy aside.
    Remove,
    /// Leave both sides as they are, for the reason given.
    Leave(&'static str),
}

/// What to do about `op` — RomM's answer for one save in `slot` — given what is here (`held`) and
/// the last agreement this device remembers for it (`baseline`).
///
/// RomM's answer stands where this device has nothing to add. What it adds is content: whether the
/// local copy changed since the last agreement (`here`), and — when that agreement was about this
/// very slot — whether RomM's copy did (`there`). Those can only turn an answer that would lose
/// progress into a conflict, or a conflict that loses nothing into the transfer it really is.
fn decide(op: &SyncOperation, slot: &str, held: Option<&Held>, baseline: Option<&Entry>) -> Step {
    let server_hash = op.server_content_hash.as_deref();
    if let (Some(held), Some(server_hash)) = (held, server_hash)
        && held.is(server_hash)
    {
        // An agreement from before sync sessions was never acknowledged as this device: once, now.
        let confirm =
            op.save_id.is_some() && baseline.is_none_or(|b| Some(b.save_id) != op.save_id || b.unconfirmed || b.slot.is_none());
        return Step::Agree { confirm };
    }
    // Changed here since the last agreement — any slot's, since it is the content that is weighed.
    let here = baseline.map(|b| held.is_none_or(|h| !same_content(h, b)));
    // RomM's copy moved since the last agreement — only knowable when that was about this slot. A
    // version is rewritten in place by RomM's own player, so the id alone does not say it is the same.
    let there = baseline.filter(|b| b.slot.as_deref().unwrap_or(AUTOSAVE) == slot).map(|b| match op.save_id {
        None => true,
        Some(id) => id != b.save_id || matches!((server_hash, theirs(b)), (Some(now), Some(then)) if now != then),
    });

    match op.action.as_str() {
        "no_op" if op.reason.to_lowercase().contains("untracked") => Step::Leave("untracked on this device in RomM"),
        "no_op" => match (held, here, there) {
            (None, ..) => Step::Agree { confirm: false },
            (Some(_), Some(true), Some(false)) => Step::Upload,
            (Some(_), Some(false), Some(true)) if op.save_id.is_some() => Step::Download,
            (Some(_), Some(true), Some(true)) if op.save_id.is_some() => Step::Conflict,
            // Different content, and never an agreement to tell which side moved.
            (Some(_), None, _) if server_hash.is_some() && op.save_id.is_some() => Step::Conflict,
            _ => Step::Agree { confirm: false },
        },
        "upload" if held.is_none() => Step::Leave("nothing here to send"),
        "upload" => match (op.save_id, here, there) {
            // RomM's slot is empty, and what is here is what was last agreed on: it was deleted on
            // RomM, and sending it back unchanged would undo that.
            (None, Some(false), Some(true)) => Step::Leave("deleted on RomM, so not sent back until it changes"),
            (None, ..) => Step::Upload,
            (Some(_), Some(false), Some(false)) => Step::Agree { confirm: false },
            (Some(_), _, Some(false)) => Step::Upload,
            (Some(_), Some(false), Some(true)) => Step::Download,
            // Both sides hold something, and either RomM moved past what this device last saw or
            // the two never agreed at all: RomM chose by the clocks, and that is no answer.
            (Some(_), ..) => Step::Conflict,
        },
        "download" => match (held, here, there) {
            (None, ..) | (Some(_), Some(false), _) => Step::Download,
            (Some(_), Some(true), Some(false)) => Step::Upload,
            (Some(_), ..) => Step::Conflict,
        },
        "conflict" => match (held, here, there) {
            (None, ..) | (Some(_), Some(false), _) => Step::Download,
            (Some(_), Some(true), Some(false)) => Step::Upload,
            (Some(_), ..) => Step::Conflict,
        },
        "delete" if held.is_none() => Step::Agree { confirm: false },
        // Removed on RomM, and what is here is not what the last agreement recorded — progress made
        // since, or no agreement to go by at all. RomM's own copy of those bytes is gone, so taking
        // this device's away would leave them nowhere: it stays, and its next change sends it up.
        "delete" if here != Some(false) => {
            Step::Leave("removed on RomM, but this device's copy has changed since, so it was left as it is")
        }
        "delete" => Step::Remove,
        // Settled by content alone, against RomM's newest version of this very save.
        BY_NAME => match (held, op.save_id, here, there) {
            (None, None, ..) => Step::Agree { confirm: false },
            // Gone from here since the last agreement, and unchanged on RomM: deleted here on purpose.
            (None, Some(_), _, Some(false)) => Step::Leave(""),
            (None, Some(_), ..) => Step::Download,
            // None of this save's versions is left in a slot other saves fill: trimmed away by their
            // pushes far more likely than deleted, and the other devices need a copy.
            (Some(_), None, ..) => Step::Upload,
            (Some(_), Some(_), Some(false), Some(false)) => Step::Agree { confirm: false },
            (Some(_), Some(_), Some(true), Some(false)) => Step::Upload,
            (Some(_), Some(_), Some(false), Some(true)) => Step::Download,
            (Some(_), Some(_), ..) => Step::Conflict,
        },
        _ => Step::Leave("RomM asked for something this version of Deckhand does not know"),
    }
}

/// Whether what is here is what `baseline` recorded.
fn same_content(held: &Held, baseline: &Entry) -> bool {
    held.is(&baseline.hash) || (!baseline.romm_hash && held.legacy.as_deref() == Some(baseline.hash.as_str()))
}

/// RomM's hash of the version `baseline` agreed on, when it is known.
fn theirs(baseline: &Entry) -> Option<&str> {
    baseline.server_hash.as_deref().or(baseline.romm_hash.then_some(baseline.hash.as_str()))
}

/// The time this device reports for `held`: its modification time, moved to the right side of the
/// last agreement — no newer than it when the content is what was agreed, newer when it is not. RomM
/// weighs the time against when this device last synced the slot, by RomM's clock, so this is what
/// keeps a skewed clock here from reading as no change, or as one.
fn reported_time(held: &Held, baseline: Option<&Entry>) -> SystemTime {
    let modified = held.modified.unwrap_or(SystemTime::UNIX_EPOCH);
    let agreed = baseline.and_then(|b| b.synced_at.as_deref().and_then(names::parse_timestamp).map(|t| (b, t)));
    match agreed {
        Some((b, at)) if same_content(held, b) => modified.min(at),
        Some((_, at)) => modified.max(at + Duration::from_secs(1)),
        None => modified,
    }
}

fn client_save(thread: &Thread, held: &Held, baseline: Option<&Entry>) -> ClientSave {
    ClientSave {
        rom_id: thread.rom_id,
        file_name: thread.name.clone(),
        slot: Some(thread.slot.clone()),
        emulator: Some(thread.emulator.clone()),
        content_hash: Some(held.hash.clone()),
        updated_at: names::format_timestamp_precise(reported_time(held, baseline)),
        file_size_bytes: held.bytes,
    }
}

/// RomM turned this device's sync off.
fn disabled(e: &ApiError) -> bool {
    e.status == 400 && e.detail.as_deref().is_some_and(|d| d.contains("disabled"))
}

/// RomM has no device by this install's id any more.
fn forgotten(e: &ApiError) -> bool {
    e.status == 404 && e.detail.as_deref().is_some_and(|d| d.starts_with("Device"))
}

/// Why this RomM cannot hold a sync session with this install, when that is what `e` says.
fn unsupported(e: &ApiError) -> Option<String> {
    match (e.status, e.detail.as_deref()) {
        (404 | 405, _) => Some("this RomM has no sync sessions".into()),
        (400, Some(d)) if d.contains("device_id") => Some("this install is not registered with RomM as a device".into()),
        (401 | 403, _) => Some("the pairing token cannot use RomM's sync".into()),
        _ => None,
    }
}

/// What RomM holds for a slot, as the agreement records it.
struct Version {
    save_id: i64,
    synced_at: Option<String>,
    server_hash: Option<String>,
}

/// One sync's state.
struct Run<'a> {
    client: &'a RommClient,
    conn: &'a Connection,
    device: String,
    indexes: HashMap<&'static str, Index>,
    store: PathBuf,
    report: Report,
    session: Option<i64>,
    /// Operations done and failed in the current session.
    counts: (usize, usize),
    /// The save being settled shares its slot with other saves of the game.
    shared: bool,
}

impl<'a> Run<'a> {
    fn new(client: &'a RommClient, conn: &'a Connection, store: &Path, device: String) -> Self {
        Self {
            client,
            conn,
            device,
            indexes: HashMap::new(),
            store: store.to_path_buf(),
            report: Report::default(),
            session: None,
            counts: (0, 0),
            shared: false,
        }
    }

    fn index(&mut self, thread: &Thread) -> &mut Index {
        let file = thread.index_file();
        let store = &self.store;
        self.indexes.entry(file).or_insert_with(|| Index::load(store.join(file)))
    }

    fn baseline(&mut self, thread: &Thread) -> Option<Entry> {
        let key = thread.key();
        self.index(thread).agreed(&key, thread.rom_id, thread.stamp.as_ref()).cloned()
    }

    /// Put `thread` in the slot its save last agreed with RomM in: the one it goes on in, until RomM
    /// holds a newer version of it in another — see [`follow`].
    fn recall(&mut self, thread: &mut Thread) {
        if thread.shared {
            return;
        }
        if let Some(slot) = self.baseline(thread).and_then(|b| b.slot) {
            thread.slot = slot;
        }
    }

    /// The first sync of games whose saves sigil now keeps ([`Place::Sigil`]) while RomM may still
    /// hold them in the shape before it: a PS2 game's save folders zipped one by one, a PSP game's, a
    /// core's `.srm` that sigil keeps as a zip of save types. For each such game with no agreement of
    /// its own yet:
    ///
    /// - RomM already holds the unit, under the name this app gives it: another device moved the
    ///   game over. When what is here is what this device last agreed with RomM on in the old shape,
    ///   it is only behind, and its unit is recorded as agreeing with no version RomM holds — so the
    ///   sync takes RomM's. Anything else here may be progress RomM lacks, and is weighed against
    ///   the unit as any save is.
    /// - Otherwise the old shape is settled first, by the engine that wrote it — what a device on it
    ///   pushed comes down, what is here and newer goes up, a conflict is decided as one — and the
    ///   unit goes up after, in this sync. Where the old shape's own zip had the unit's name, the
    ///   agreement just reached is recorded as the unit's, so the unit only goes up once it changes.
    ///
    /// A game RomM's copies could not be read for, or whose old shape did not settle, is left out of
    /// this sync, rather than weighed against a copy in another shape.
    async fn migrate(&mut self, threads: Vec<Thread>) -> Vec<Thread> {
        let firsts: Vec<usize> = (0..threads.len())
            .filter(|&i| matches!(&threads[i].place, Place::Sigil(u) if !u.legacy.is_empty() || u.legacy_source.is_some()))
            .filter(|&i| self.baseline(&threads[i]).is_none())
            .collect();
        if firsts.is_empty() {
            return threads;
        }
        let mut roms: Vec<i64> = firsts.iter().map(|&i| threads[i].rom_id).collect();
        roms.sort_unstable();
        roms.dedup();
        let mut versions: Vec<SaveAsset> = Vec::new();
        for batch in roms.chunks(100) {
            match self.client.fetch_saves_of(self.conn, batch).await {
                Ok(found) => versions.extend(found),
                Err(e) => {
                    self.report.notes.push(format!(
                        "{} games were left for the next sync: RomM's copies of their saves could not be read ({})",
                        roms.len(),
                        super::describe(&e)
                    ));
                    return without(threads, &firsts);
                }
            }
        }

        let mut old: Vec<Thread> = Vec::new();
        let mut settling: Vec<(usize, Vec<Thread>)> = Vec::new();
        for &i in &firsts {
            let thread = &threads[i];
            let Place::Sigil(unit) = &thread.place else { continue };
            let theirs: Vec<&SaveAsset> = versions
                .iter()
                .filter(|v| v.rom_id == Some(thread.rom_id) && v.slot.as_deref().unwrap_or(AUTOSAVE) == thread.slot)
                .collect();
            let moved = theirs.iter().any(|v| names::canonical(&v.file_name).eq_ignore_ascii_case(&thread.name));
            if moved {
                if let Ok(Some(held)) = thread.held()
                    && self.unchanged(&unit.legacy)
                {
                    self.remember(thread, held.hash, Version { save_id: 0, synced_at: None, server_hash: None }, true, None);
                }
                continue;
            }
            let names: Vec<String> = theirs.iter().map(|v| names::canonical(&v.file_name)).collect();
            let legacy = match &unit.legacy_source {
                Some((emulator, rom, title)) => standalone::legacy_threads(*emulator, rom, title, thread.rom_id, thread.stamp.as_ref(), &names),
                None => unit.legacy.clone(),
            };
            if legacy.is_empty() {
                continue;
            }
            old.extend(legacy.iter().cloned());
            settling.push((i, legacy));
        }
        if old.is_empty() {
            return threads;
        }

        let outcome = Box::pin(sync(self.client, self.conn, &self.store.clone(), old, &[], Vec::new(), &|_| None)).await;
        // That sync wrote the indexes on disk; what this one read of them before is stale now.
        self.indexes.clear();
        let report = match outcome {
            Ok(Outcome::Done(report)) => *report,
            Ok(Outcome::Unsupported(why)) | Err(why) => {
                self.report.notes.push(format!("{} games were left for the next sync: their saves from before could not be settled ({why})", settling.len()));
                return without(threads, &settling.iter().map(|(i, _)| *i).collect::<Vec<_>>());
            }
        };
        let failed: Vec<usize> = settling
            .iter()
            .filter(|(_, legacy)| report.failed.iter().any(|f| legacy.iter().any(|t| f.starts_with(&format!("{}:", t.name)))))
            .map(|(i, _)| *i)
            .collect();
        self.report.absorb(report);

        // The old shape's zip under the unit's own name: its agreement is the unit's.
        for (i, legacy) in &settling {
            let thread = &threads[*i];
            let Place::Sigil(unit) = &thread.place else { continue };
            let same = legacy.iter().find(|t| t.name.eq_ignore_ascii_case(&unit.artifact));
            let Some(same) = same.filter(|_| !failed.contains(i) && unit.name != unit.artifact) else { continue };
            let (Some(agreed), Ok(Some(held))) = (self.baseline(same), thread.held()) else { continue };
            let version = Version { save_id: agreed.save_id, synced_at: agreed.synced_at.clone(), server_hash: theirs(&agreed).map(str::to_string) };
            self.remember(thread, held.hash, version, !agreed.unconfirmed, None);
        }
        if !failed.is_empty() {
            self.report.notes.push(format!("{} games were left for the next sync: their saves from before did not all settle", failed.len()));
        }
        without(threads, &failed)
    }

    /// Whether every one of `legacy` is, here, what this device last agreed with RomM on — or not
    /// here at all.
    fn unchanged(&mut self, legacy: &[Thread]) -> bool {
        legacy.iter().all(|thread| match thread.held() {
            Ok(None) => true,
            Ok(Some(held)) => self.baseline(thread).is_some_and(|b| same_content(&held, &b)),
            Err(_) => false,
        })
    }

    /// Act on RomM's answer `op` about `thread` — `by_name` when it is this device's own, against the
    /// versions of a slot other saves share.
    async fn apply(&mut self, op: &SyncOperation, thread: &Thread, held: Option<&Held>, by_name: bool) {
        let baseline = self.baseline(thread);
        self.shared = by_name || thread.shared;
        match decide(op, &thread.slot, held, baseline.as_ref()) {
            Step::Agree { confirm } => self.agree(op, thread, held, baseline.as_ref(), confirm).await,
            Step::Upload => {
                if self.push(op, thread, held, false).await == Went::Up {
                    self.report.pushed.push(thread.name.clone());
                }
            }
            Step::Download => {
                if self.pull(op, thread, held).await {
                    self.report.pulled.push(thread.name.clone());
                }
            }
            Step::Conflict => self.settle(op, thread, held).await,
            Step::Remove => self.remove(thread),
            Step::Leave("") => {}
            Step::Leave(why) => self.report.notes.push(format!("{}: {why}", thread.name)),
        }
    }

    /// The same on both sides. Recorded only when that is known — the same hash, or nothing moved
    /// on either side since the last agreement — and acknowledged to RomM when it has not been.
    async fn agree(&mut self, op: &SyncOperation, thread: &Thread, held: Option<&Held>, baseline: Option<&Entry>, confirm: bool) {
        let Some(held) = held else { return };
        self.report.in_sync += 1;
        let identical = op.server_content_hash.as_deref().is_some_and(|h| held.is(h));
        let version = match (identical, op.save_id, baseline) {
            (true, Some(save_id), _) => {
                Version { save_id, synced_at: op.server_updated_at.clone(), server_hash: op.server_content_hash.clone() }
            }
            // An agreement from before sync sessions takes RomM's hash and slot here.
            (false, _, Some(b)) if same_content(held, b) => {
                Version { save_id: b.save_id, synced_at: b.synced_at.clone(), server_hash: b.server_hash.clone() }
            }
            _ => return,
        };
        let confirmed = if confirm {
            self.client.confirm_downloaded(self.conn, version.save_id, &self.device, Some(&held.hash)).await.is_ok()
        } else {
            baseline.is_none_or(|b| !b.unconfirmed || b.save_id != version.save_id)
        };
        self.remember(thread, held.hash.clone(), version, confirmed, None);
    }

    /// Push this device's copy into its slot. `overwrite`, past a version this device has not seen,
    /// is only ever a settled conflict: a person's choice, or the newer side when nobody was asked.
    async fn push(&mut self, op: &SyncOperation, thread: &Thread, held: Option<&Held>, overwrite: bool) -> Went {
        let (mut overwrite, mut retried) = (overwrite, false);
        loop {
            let (bytes, hash) = match thread.body() {
                Ok(body) => body,
                Err(e) => return self.fail(thread, e),
            };
            let push = Push {
                rom_id: thread.rom_id,
                emulator: &thread.emulator,
                slot: &thread.slot,
                file_name: &thread.name,
                bytes,
                content_hash: &hash,
                session_id: self.session,
                overwrite,
                // Off for a slot other saves share, whether this run worked that out or the thread
                // itself says so — a restore pushes straight through `push`, and RomM's trimming
                // counts the whole slot, so leaving it on would drop another save's only copy.
                autocleanup: !(self.shared || thread.shared),
            };
            match self.client.push_save(self.conn, push).await {
                Ok(Pushed::Saved(created)) => {
                    let created = *created;
                    // RomM 5.3 answers a push of bytes an older version of the slot already holds
                    // with that version, storing nothing — which would leave the newer one current.
                    if !overwrite && op.save_id.is_some_and(|id| id != created.id) && older(&created.updated_at, &op.server_updated_at) {
                        overwrite = true;
                        continue;
                    }
                    self.counts.0 += 1;
                    let server_hash = created.content_hash.filter(|h| *h != hash);
                    self.remember(thread, hash, Version { save_id: created.id, synced_at: created.updated_at, server_hash }, true, None);
                    return Went::Up;
                }
                Ok(Pushed::Refused) if !overwrite => {
                    // RomM holds a version of the slot this device has not synced. Someone else's
                    // progress on this save, landed since — settled as the conflict it is. Or not
                    // this save's at all: another save sharing the slot, or the very version this
                    // device decided against, which RomM was never told it holds. Then nothing of
                    // this save's is in the way, and it goes up past the check.
                    let current = match self.newest(thread).await {
                        Ok(current) => current,
                        Err(e) => return self.fail(thread, e),
                    };
                    if !retried && current.save_id == op.save_id {
                        (overwrite, retried) = (true, true);
                        continue;
                    }
                    Box::pin(self.settle(&current, thread, held)).await;
                    return Went::Settled;
                }
                Ok(Pushed::Refused) => return self.fail(thread, "RomM refused it"),
                Err(e) => return self.fail(thread, super::describe(&e)),
            }
        }
    }

    /// RomM's newest version of `thread`'s save — not of its slot, which other saves may share — as
    /// an answer to settle against.
    async fn newest(&self, thread: &Thread) -> Result<SyncOperation, String> {
        let versions = self
            .client
            .slot_saves(self.conn, thread.rom_id, &thread.slot)
            .await
            .map_err(|e| format!("RomM has a newer save, which could not be read ({})", super::describe(&e)))?;
        Ok(SyncOperation { reason: "RomM has a newer version since this device last synced".into(), ..by_name_op(thread, &versions) })
    }

    /// Take RomM's copy: check it is what RomM described, put it in place, then say so.
    ///
    /// `keep` is what is here now, when it has not already been set aside. Bytes a download writes
    /// over are RomM's to hand back when they are a version of this very save, and this device's copy
    /// goes to `save-backups/` first when they may not be. Two ways they may not: a version only of
    /// this save's kind can be another save of a slot several share — each disc of a set keeps its own
    /// `.srm` — and a flat set's pull takes away the set's files RomM's copy of it does not name.
    async fn pull(&mut self, op: &SyncOperation, thread: &Thread, keep: Option<&Held>) -> bool {
        let replaced = keep.is_some_and(|held| op.server_content_hash.as_deref().is_none_or(|h| !held.is(h)));
        // A unit that is more than one file of its own can take files away, or sit on a card every
        // game shares: what it replaces is set aside first, whatever it is called.
        let unaccounted = !names::canonical(&op.file_name).eq_ignore_ascii_case(&thread.name)
            || matches!(&thread.place, Place::Set(bundle) if !bundle.folder)
            || matches!(&thread.place, Place::Sigil(unit) if unit.file().is_none());
        if replaced && unaccounted && self.set_aside(thread).is_none() {
            return false;
        }
        let Some(save_id) = op.save_id else { return self.failed(thread, "RomM named no save to take".into()) };
        let bytes = match self.client.pull_save(self.conn, save_id, self.session).await {
            Ok(bytes) => bytes,
            Err(e) => return self.failed(thread, super::describe(&e)),
        };
        if let Some(expected) = op.server_content_hash.as_deref()
            && hash::of_upload(&bytes) != expected
        {
            return self.failed(thread, "what came down is not the save RomM described, so it was left as it was".into());
        }
        let made = op.server_updated_at.as_deref().and_then(names::parse_timestamp);
        let (hash, written) = match thread.put(&bytes, made, &op.file_name) {
            Ok(put) => put,
            Err(e) => return self.failed(thread, e),
        };
        let confirmed = self.client.confirm_downloaded(self.conn, save_id, &self.device, Some(&hash)).await.is_ok();
        self.counts.0 += 1;
        // What the pull wrote, and what it replaced: a flat set's pull takes files away, and the
        // watcher has to know about those as well as the ones now there.
        self.report.written.extend(thread.paths());
        self.report.written.extend(written.iter().cloned());
        let server_hash = op.server_content_hash.clone().filter(|h| *h != hash);
        let version = Version { save_id, synced_at: op.server_updated_at.clone(), server_hash };
        self.remember(thread, hash, version, confirmed, Some(&written));
        true
    }

    /// Both sides hold progress the other lacks. A person decides when one is there to ask, the newer
    /// side otherwise; the loser is never lost — RomM keeps its copy in the slot's history, and this
    /// device's goes to `save-backups/` before anything replaces it.
    async fn settle(&mut self, op: &SyncOperation, thread: &Thread, held: Option<&Held>) {
        let Some(held) = held else {
            if self.pull(op, thread, None).await {
                self.report.pulled.push(thread.name.clone());
            }
            return;
        };
        // What is here is, byte for byte, a version RomM's history already moved past: this device is
        // only behind, which nobody needs asking about — a first sync on a device that has not played
        // since, say. Its copy is in that history, so nothing is set aside.
        if self.behind(op, thread, held).await {
            if self.pull(op, thread, None).await {
                self.report.pulled.push(thread.name.clone());
            }
            return;
        }
        let remote_time = op.server_updated_at.as_deref().and_then(names::parse_timestamp);
        let name = thread.name.clone();
        match super::choose(&name, held.modified, remote_time, held.bytes).await {
            Choice::Local => {
                if self.push(op, thread, Some(held), true).await == Went::Up {
                    self.report.conflicts.push(format!("{name}: kept this device's copy; RomM keeps the other in its history"));
                }
            }
            Choice::Remote => {
                let Some(kept) = self.set_aside(thread) else { return };
                if self.pull(op, thread, None).await {
                    self.report.conflicts.push(format!("{name}: kept RomM's copy; this device's is at {}", kept.display()));
                }
            }
        }
    }

    /// Whether `held` is an older version of `thread`'s save than `op`'s, by RomM's history of the slot.
    async fn behind(&self, op: &SyncOperation, thread: &Thread, held: &Held) -> bool {
        let Some(current) = op.save_id else { return false };
        let Ok(versions) = self.client.slot_saves(self.conn, thread.rom_id, &thread.slot).await else { return false };
        let made = |v: &SaveAsset| v.updated_at.as_deref().and_then(names::parse_timestamp);
        let newest = versions.iter().find(|v| v.id == current).and_then(made);
        versions.iter().any(|v| {
            v.id != current
                && same_save(thread, &v.file_name)
                && v.content_hash.as_deref().is_some_and(|h| held.is(h))
                && matches!((made(v), newest), (Some(then), Some(now)) if then < now)
        })
    }

    /// RomM removed this version: the local copy goes too, set aside first.
    fn remove(&mut self, thread: &Thread) {
        let Some(kept) = self.set_aside(thread) else { return };
        if let Err(e) = thread.clear() {
            self.fail(thread, format!("could not remove it ({e})"));
            return;
        }
        self.report.written.extend(thread.paths());
        let key = thread.key();
        if let Err(e) = self.index(thread).remove(&key) {
            self.report.failed.push(format!("{}: removed, but could not record it ({e})", thread.name));
        }
        self.counts.0 += 1;
        self.report.removed.push(format!("{}: removed on RomM; this device's copy is at {}", thread.name, kept.display()));
    }

    /// Copy the local save into the backups. None — and so nothing overwritten — when that fails.
    fn set_aside(&mut self, thread: &Thread) -> Option<PathBuf> {
        match thread.snapshot().and_then(|bytes| back_up(&self.store.join("save-backups"), &thread.name, &bytes)) {
            Ok(path) => Some(path),
            Err(e) => {
                self.fail(thread, format!("could not keep a copy of this device's, so it was left as it was ({e})"));
                None
            }
        }
    }

    /// `written` is what a pull just put on disk — see [`Thread::set_files`].
    fn remember(&mut self, thread: &Thread, hash: String, version: Version, confirmed: bool, written: Option<&[PathBuf]>) {
        let entry = Entry {
            rom_id: thread.rom_id,
            hash,
            save_id: version.save_id,
            slot: Some(thread.slot.clone()),
            synced_at: version.synced_at,
            server_hash: version.server_hash,
            romm_hash: true,
            unconfirmed: !confirmed,
            files: thread.set_files(written),
            stamp: thread.stamp.clone(),
        };
        let key = thread.key();
        if let Err(e) = self.index(thread).set(&key, entry) {
            self.report.failed.push(format!("{}: synced, but could not record it ({e})", thread.name));
        }
    }

    fn fail(&mut self, thread: &Thread, why: impl std::fmt::Display) -> Went {
        self.counts.1 += 1;
        self.report.failed.push(format!("{}: {why}", thread.name));
        Went::Not
    }

    fn failed(&mut self, thread: &Thread, why: String) -> bool {
        self.fail(thread, why);
        false
    }
}

/// `threads` but those at `left`.
fn without(threads: Vec<Thread>, left: &[usize]) -> Vec<Thread> {
    threads.into_iter().enumerate().filter(|(i, _)| !left.contains(i)).map(|(_, t)| t).collect()
}

/// How a push went.
#[derive(Debug, PartialEq, Eq)]
enum Went {
    Up,
    /// Refused, and settled as the conflict it was — already reported.
    Settled,
    Not,
}

/// Whether RomM time `a` is before `b`; false when either is unreadable.
fn older(a: &Option<String>, b: &Option<String>) -> bool {
    match (a.as_deref().and_then(names::parse_timestamp), b.as_deref().and_then(names::parse_timestamp)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

/// Write `bytes` into `dir` as `<milliseconds> <name>`, so the copies sort by when they were made.
///
/// Never over a copy already there: a second one of the same save in the same millisecond — a
/// conflict settled for two of a game's save folders in one run — takes the next free name, because
/// the whole point of the folder is that what it holds is the only copy left of something.
pub(super) fn back_up(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for n in 0..=1000u32 {
        let kept = match n {
            0 => dir.join(format!("{stamp} {name}")),
            n => dir.join(format!("{stamp} ({n}) {name}")),
        };
        match fs::OpenOptions::new().create_new(true).write(true).open(&kept) {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, bytes).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
                return Ok(kept);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("the backups folder already holds a thousand copies made this millisecond".into())
}

/// How many copies of one save `save-backups/` keeps, and how long every copy is kept whatever else
/// is in there.
const KEEP_COPIES: usize = 10;
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

/// Trim `save-backups/`: of one save's copies, the newest [`KEEP_COPIES`] stay, and so does every
/// copy made within the last [`KEEP_FOR`] however many there are.
///
/// Nothing else ever cleared the folder, and a device that settles a conflict or takes a download
/// over something now leaves a copy each time — on a Deck's drive that adds up. But what is in there
/// can be the last copy of a save, so one goes only once ten newer copies of that same save are
/// there and a week has passed. Called at the end of a sync, where a failure costs nothing.
pub(super) fn prune_backups(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut per_save: HashMap<String, Vec<(u128, PathBuf)>> = HashMap::new();
    for entry in entries.flatten() {
        let Some((made, save)) = copy_of(&entry.file_name().to_string_lossy()) else { continue };
        per_save.entry(save).or_default().push((made, entry.path()));
    }
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let keep_after = now.saturating_sub(KEEP_FOR.as_millis());
    for copies in per_save.values_mut() {
        copies.sort_unstable_by_key(|(made, _)| std::cmp::Reverse(*made));
        for (made, path) in copies.iter().skip(KEEP_COPIES) {
            if *made < keep_after {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// A copy's name read back: when it was made, and which save it is a copy of. None for anything in
/// the folder this app did not put there, which is then left alone.
fn copy_of(name: &str) -> Option<(u128, String)> {
    let (stamp, rest) = name.split_once(' ')?;
    let made: u128 = stamp.parse().ok()?;
    // `<millis> (2) <name>`, the second copy of one save made in one millisecond.
    let save = rest.strip_prefix('(').and_then(|r| r.split_once(") ")).map_or(rest, |(_, name)| name);
    Some((made, save.to_string()))
}

/// The one sync running at a time on this device, across processes: the window, a launch's
/// `deckhand pre`/`post`, ES-DE's hooks. RomM 5.3 closes a device's open session when the device
/// negotiates another, and two runs writing one index would each record half of what happened.
pub struct Exclusive(#[allow(dead_code)] fs::File);

/// Wait for the sync lock in `store`, for up to two minutes. A filesystem that cannot lock is let
/// through unlocked rather than stopping every sync.
pub async fn exclusive(store: &Path) -> Result<Exclusive, String> {
    let path = store.join("sync.lock");
    tokio::task::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("can't open {}: {e}", path.display()))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Exclusive(file)),
                Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(fs::TryLockError::WouldBlock) => return Err("another Deckhand sync is still running".to_string()),
                Err(fs::TryLockError::Error(_)) => return Ok(Exclusive(file)),
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(action: &str, save_id: Option<i64>, server_hash: Option<&str>) -> SyncOperation {
        SyncOperation {
            action: action.into(),
            rom_id: 1,
            save_id,
            file_name: "Game [2026-07-01_10-00-00].srm".into(),
            slot: Some(AUTOSAVE.into()),
            emulator: Some("snes9x".into()),
            reason: String::new(),
            server_updated_at: Some("2026-07-01T10:00:00Z".into()),
            server_content_hash: server_hash.map(str::to_string),
        }
    }

    fn held(hash: &str) -> Held {
        Held { hash: hash.into(), legacy: None, raw: None, bytes: 8, modified: None }
    }

    #[test]
    fn a_unit_sigil_sends_in_a_form_of_its_own_is_still_the_file_an_agreement_recorded() {
        // A PS1 card: the file on disk hashes "raw", the unit sigil makes of it "canonical".
        let card = Held { raw: Some("raw".into()), ..held("canonical") };
        let last = agreed("raw", 10);
        assert_eq!(decided(&op("no_op", Some(10), Some("raw")), Some(&card), Some(&last)), Step::Agree { confirm: false });
        assert_eq!(decided(&op("upload", Some(10), Some("raw")), Some(&card), Some(&last)), Step::Agree { confirm: false });
        // Another device moved on: unchanged here, so it is only a download — never a conflict.
        assert_eq!(decided(&op("download", Some(11), Some("newer")), Some(&card), Some(&last)), Step::Download);
        assert_eq!(decided(&op("no_op", Some(11), Some("newer")), Some(&card), Some(&last)), Step::Download);
        // No agreement at all, and RomM holds this very file: the same.
        assert_eq!(decided(&op("no_op", Some(10), Some("raw")), Some(&card), None), Step::Agree { confirm: true });
        // Changed here since: still sent.
        let played = Held { raw: Some("raw2".into()), ..held("canonical2") };
        assert_eq!(decided(&op("no_op", Some(10), Some("raw")), Some(&played), Some(&last)), Step::Upload);
    }

    fn agreed(hash: &str, save_id: i64) -> Entry {
        Entry {
            rom_id: 1,
            hash: hash.into(),
            save_id,
            slot: Some(AUTOSAVE.into()),
            synced_at: Some("2026-06-01T10:00:00Z".into()),
            server_hash: None,
            romm_hash: true,
            unconfirmed: false,
            files: None,
            stamp: None,
        }
    }

    fn decided(op: &SyncOperation, held: Option<&Held>, baseline: Option<&Entry>) -> Step {
        decide(op, AUTOSAVE, held, baseline)
    }

    #[test]
    fn the_same_content_on_both_sides_is_agreed_and_acknowledged_once() {
        let here = held("aaa");
        assert_eq!(decided(&op("conflict", Some(10), Some("aaa")), Some(&here), None), Step::Agree { confirm: true });
        assert_eq!(decided(&op("no_op", Some(10), Some("aaa")), Some(&here), Some(&agreed("aaa", 10))), Step::Agree { confirm: false });
        let unacknowledged = Entry { unconfirmed: true, ..agreed("aaa", 10) };
        assert_eq!(decided(&op("no_op", Some(10), Some("aaa")), Some(&here), Some(&unacknowledged)), Step::Agree { confirm: true });
    }

    #[test]
    fn romm_s_answer_stands_where_this_device_has_nothing_to_add() {
        let last = agreed("aaa", 10);
        assert_eq!(decided(&op("upload", Some(10), Some("aaa")), Some(&held("bbb")), Some(&last)), Step::Upload);
        assert_eq!(decided(&op("download", Some(11), Some("ccc")), Some(&held("aaa")), Some(&last)), Step::Download);
        assert_eq!(decided(&op("download", Some(11), Some("ccc")), None, None), Step::Download, "nothing here yet");
        assert_eq!(decided(&op("upload", None, None), Some(&held("aaa")), None), Step::Upload, "nothing on RomM yet");
        assert_eq!(decided(&op("conflict", Some(11), Some("ccc")), Some(&held("bbb")), Some(&last)), Step::Conflict);
    }

    #[test]
    fn a_download_over_progress_romm_never_saw_is_a_conflict() {
        // Another device pushed version 11 while this one played offline: RomM 5.3 compares by clock
        // against a version this device never synced, and may say download.
        let last = agreed("aaa", 10);
        assert_eq!(decided(&op("download", Some(11), Some("ccc")), Some(&held("bbb")), Some(&last)), Step::Conflict);
        // Or upload, over a version this device never saw.
        assert_eq!(decided(&op("upload", Some(11), Some("ccc")), Some(&held("bbb")), Some(&last)), Step::Conflict);
        // A first sync with something on both sides and no agreement ever: the clocks are no answer.
        assert_eq!(decided(&op("download", Some(11), Some("ccc")), Some(&held("bbb")), None), Step::Conflict);
        assert_eq!(decided(&op("upload", Some(11), Some("ccc")), Some(&held("bbb")), None), Step::Conflict);
    }

    #[test]
    fn a_conflict_only_one_side_made_is_the_transfer_it_really_is() {
        let last = agreed("aaa", 10);
        assert_eq!(decided(&op("conflict", Some(11), Some("ccc")), Some(&held("aaa")), Some(&last)), Step::Download);
        assert_eq!(decided(&op("conflict", Some(10), Some("aaa")), Some(&held("bbb")), Some(&last)), Step::Upload);
    }

    #[test]
    fn a_no_op_over_a_change_the_clocks_hid_still_moves_it() {
        // This device's clock is behind RomM's, so its change reads as older than the last sync.
        let last = agreed("aaa", 10);
        assert_eq!(decided(&op("no_op", Some(10), Some("aaa")), Some(&held("bbb")), Some(&last)), Step::Upload);
        // A no-op RomM gives for a save the player untracked is left alone whatever changed.
        let untracked = SyncOperation { reason: "Save is untracked on this device".into(), ..op("no_op", Some(10), Some("aaa")) };
        assert_eq!(decided(&untracked, Some(&held("bbb")), Some(&last)), Step::Leave("untracked on this device in RomM"));
    }

    #[test]
    fn a_save_deleted_on_romm_is_not_sent_back_unless_it_changes() {
        let last = agreed("aaa", 10);
        assert!(matches!(decided(&op("upload", None, None), Some(&held("aaa")), Some(&last)), Step::Leave(_)));
        assert_eq!(decided(&op("upload", None, None), Some(&held("bbb")), Some(&last)), Step::Upload);
        // An agreement about another slot says nothing about this one being emptied.
        let elsewhere = Entry { slot: Some("BASLUS-20946GTA5".into()), ..agreed("aaa", 10) };
        assert_eq!(decided(&op("upload", None, None), Some(&held("aaa")), Some(&elsewhere)), Step::Upload);
    }

    #[test]
    fn a_set_hashed_the_old_way_is_still_recognised_as_unchanged() {
        let legacy = Entry { romm_hash: false, ..agreed("old-way", 10) };
        let set = Held { legacy: Some("old-way".into()), ..held("romm-way") };
        assert_eq!(decided(&op("download", Some(11), Some("ccc")), Some(&set), Some(&legacy)), Step::Download);
    }

    #[test]
    fn newer_servers_deletions_set_the_local_copy_aside_and_unknown_answers_are_left() {
        assert_eq!(decided(&op("delete", None, None), Some(&held("aaa")), Some(&agreed("aaa", 10))), Step::Remove);
        assert_eq!(decided(&op("delete", None, None), None, None), Step::Agree { confirm: false });
        assert!(matches!(decided(&op("teleport", Some(10), None), Some(&held("aaa")), None), Step::Leave(_)));
    }

    #[test]
    fn a_deletion_does_not_take_progress_romm_never_had() {
        // Played here since the last agreement, and removed on RomM in between: RomM has no copy of
        // what is here, so this is the one answer that must not be carried out.
        let changed_here = decided(&op("delete", None, None), Some(&held("bbb")), Some(&agreed("aaa", 10)));
        assert!(matches!(changed_here, Step::Leave(_)), "{changed_here:?}");
        // Nor when there is no agreement to say which side moved.
        assert!(matches!(decided(&op("delete", None, None), Some(&held("bbb")), None), Step::Leave(_)));
    }

    #[test]
    fn a_slot_several_of_a_games_saves_share_is_settled_against_the_one_of_its_own_name() {
        let thread = Thread {
            rom_id: 1,
            stamp: None,
            slot: AUTOSAVE.into(),
            name: "Driver 2 (USA) (Disc 1).srm".into(),
            emulator: "swanstation".into(),
            place: Place::File(PathBuf::from("Driver 2 (USA) (Disc 1).srm")),
            shared: false,
        };
        let version = |id: i64, name: &str, at: &str| -> SaveAsset {
            serde_json::from_value(serde_json::json!({
                "id": id, "rom_id": 1, "slot": AUTOSAVE, "file_name": name, "updated_at": at,
            }))
            .unwrap()
        };
        // Both discs of the set are one rom on RomM, and both saves are `.srm`s of its autosave slot.
        let versions = [
            version(10, "Driver 2 (USA) (Disc 1) [2026-07-01_10-00-00].srm", "2026-07-01T10:00:00Z"),
            version(11, "Driver 2 (USA) (Disc 2) [2026-07-02_10-00-00].srm", "2026-07-02T10:00:00Z"),
        ];
        let op = by_name_op(&thread, &versions);
        assert_eq!(op.save_id, Some(10), "disc 2's newer save is not a version of disc 1's: {op:?}");

        // With none of its own name there, another spelling of the game's save still comes down —
        // which is what brings one made in RomM's own player here.
        let elsewhere = [version(11, "Driver 2 (Europe) [2026-07-02_10-00-00].srm", "2026-07-02T10:00:00Z")];
        assert_eq!(by_name_op(&thread, &elsewhere).save_id, Some(11));
    }

    #[test]
    fn a_copy_set_aside_never_goes_over_one_already_there() {
        let dir = std::env::temp_dir().join(format!("deckhand-backups-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = back_up(&dir, "Chrono Trigger (USA).srm", b"the first copy").unwrap();
        let second = back_up(&dir, "Chrono Trigger (USA).srm", b"the second copy").unwrap();
        assert_ne!(first, second, "two copies in the same millisecond are two files");
        assert_eq!(fs::read(&first).unwrap(), b"the first copy");
        assert_eq!(fs::read(&second).unwrap(), b"the second copy");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_backups_keep_ten_of_each_save_and_everything_from_the_last_week() {
        let dir = std::env::temp_dir().join(format!("deckhand-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
        let week = KEEP_FOR.as_millis();
        let copy = |name: &str| {
            fs::write(dir.join(name), "a copy").unwrap();
            dir.join(name)
        };
        // Fifteen copies of one save from a month ago, and fifteen from today.
        let old: Vec<PathBuf> = (0..15).map(|n| copy(&format!("{} Chrono.srm", now - 4 * week + n))).collect();
        let today: Vec<PathBuf> = (0..15).map(|n| copy(&format!("{} Secret.srm", now - 1000 + n))).collect();
        // Another save with only a few, and a file this app did not put there.
        let few: Vec<PathBuf> = (0..3).map(|n| copy(&format!("{} Mana.srm", now - 4 * week + n))).collect();
        let theirs = copy("notes about my saves.txt");

        prune_backups(&dir);

        assert!(old[5..].iter().all(|p| p.is_file()), "the newest ten of that save stay");
        assert!(old[..5].iter().all(|p| !p.exists()), "the five older ones go");
        assert!(today.iter().all(|p| p.is_file()), "nothing from the last week goes, however many there are");
        assert!(few.iter().all(|p| p.is_file()), "a save with fewer than ten keeps all of them");
        assert!(theirs.is_file(), "a file this app did not write is left alone");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_settled_by_name_follows_the_content_rules() {
        let last = agreed("aaa", 10);
        let by = |save_id, hash| SyncOperation { action: BY_NAME.into(), ..op("", save_id, hash) };
        assert_eq!(decided(&by(Some(10), Some("aaa")), Some(&held("bbb")), Some(&last)), Step::Upload);
        assert_eq!(decided(&by(Some(11), Some("ccc")), Some(&held("aaa")), Some(&last)), Step::Download);
        assert_eq!(decided(&by(Some(11), Some("ccc")), Some(&held("bbb")), Some(&last)), Step::Conflict);
        assert_eq!(decided(&by(Some(11), Some("ccc")), Some(&held("bbb")), None), Step::Conflict, "never agreed");
        // Gone from here and unchanged on RomM: deleted here, and not brought back.
        assert_eq!(decided(&by(Some(10), Some("aaa")), None, Some(&last)), Step::Leave(""));
        assert_eq!(decided(&by(Some(11), Some("ccc")), None, Some(&last)), Step::Download, "newer progress comes back");
        // In a slot other saves fill, a version gone from RomM was most likely trimmed by their pushes.
        assert_eq!(decided(&by(None, None), Some(&held("aaa")), Some(&last)), Step::Upload);
        assert_eq!(decided(&by(None, None), Some(&held("aaa")), None), Step::Upload);
    }

    #[test]
    fn only_a_file_of_the_save_s_own_kind_is_its_version() {
        let srm = Thread {
            rom_id: 1,
            stamp: None,
            slot: AUTOSAVE.into(),
            name: "Chrono Trigger (USA).srm".into(),
            emulator: "snes9x".into(),
            place: Place::File(PathBuf::from("Chrono Trigger (USA).srm")),
            shared: false,
        };
        assert!(same_save(&srm, "Chrono Trigger (USA) [2026-07-01_10-00-00].srm"));
        assert!(same_save(&srm, "chrono trigger (japan).SRM"), "another build's name, the same game's save");
        assert!(!same_save(&srm, "Chrono Trigger (USA) [2026-07-01_10-00-00].mcd"), "another client's card");
    }

    #[test]
    fn the_reported_time_lands_on_the_side_of_the_last_agreement_the_content_is_on() {
        let synced = names::parse_timestamp("2026-06-01T10:00:00Z").unwrap();
        let last = agreed("aaa", 10);
        // A clock running ahead: unchanged content still reads as no newer than the agreement.
        let ahead = Held { modified: Some(synced + Duration::from_secs(3600)), ..held("aaa") };
        assert_eq!(reported_time(&ahead, Some(&last)), synced);
        // A clock running behind: changed content still reads as newer.
        let behind = Held { modified: Some(synced - Duration::from_secs(3600)), ..held("bbb") };
        assert_eq!(reported_time(&behind, Some(&last)), synced + Duration::from_secs(1));
        // No agreement: the file's own time.
        assert_eq!(reported_time(&behind, None), synced - Duration::from_secs(3600));
    }
}

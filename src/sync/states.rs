//! RetroArch save states, kept one-to-one with RomM for one game: every slot RetroArch writes is a
//! state on RomM, and a slot's state on RomM is what the slot holds.
//!
//! RetroArch keeps slot 0 in `<rom>.state`, slot N in `<rom>.stateN`, and what "save state on close"
//! writes in `<rom>.state.auto` — here a slot of its own, [`Slot::Auto`]. Each goes up under one name
//! for good, which says the slot and the core that made it: `<rom>.slot3.snes9x.deckhand.state`. RomM
//! keeps one state per name: an upload under a name the game has already replaces that state where
//! it is — the same id, new bytes and time, and a new picture when one comes — whichever core sends
//! it, which is why the core is in the name. So slot 3 is the same state on every device that runs
//! the game under that core, and there are as many slots as RetroArch writes: one that numbers each
//! new state one past the last (`savestate_auto_index`) has every one of them on RomM.
//!
//! What changed is decided the way saves decide it — content against the index — in an index of its
//! own, `state-index.json`, keyed by the slot's path. RomM keeps no hash of a state, and a state
//! replaced keeps its id, so RomM's side has moved when its time has: the index keeps the time of the
//! version last agreed on too ([`Entry::synced_at`]). A conflict keeps the newer side, and whichever
//! copy loses is kept in `save-backups/` first.
//!
//! **Nothing is deleted on RomM.** That is the player's to do, on RomM's own page. A state deleted
//! here stays on RomM, and one deleted on RomM stays here, and neither comes back by itself: a slot
//! this device had and no longer has is not downloaded again until RomM's changes, and a state gone
//! from RomM is not sent up again until the slot changes. A state on RomM that no slot here holds —
//! the web player's, another app's, another build's of the core — is on the game's Saves page, which
//! puts one into a slot ([`Target`]).
//!
//! **Only the core's own states take part.** A state is a core's memory, and RetroArch refuses one
//! another core wrote, even for the same console — a game played under SwanStation here and PCSX
//! ReARMed elsewhere has two lines of states on RomM, and each device follows its own core's. Which
//! core wrote a state is read from its `emulator` tag ([`Core::loads`]); one whose tag names no core
//! is offered as it always was.
//!
//! Before this, slot 0 was a rolling history of this app's uploads, `<rom> [2026-07-01
//! 10-00-00-000].deckhand.state`, and slots 1 to 4 were `<rom>.slotN.deckhand.state`, the same for
//! every core. Those stay on RomM. Until a slot has a state under its own name, the old one stands in
//! for it ([`stand_in`]), and a slot that holds one of the old history's states as it came down is
//! not sent up again until it changes.
//!
//! Thumbnails travel both ways: RetroArch's `<state>.png` goes up as the state's screenshot, and
//! RomM's comes down beside the slot. A standalone emulator's state sends the picture it took too
//! ([`picture`]), and PPSSPP's takes RomM's back as its `.jpg` ([`picture_beside`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::index::Entry;
use super::retroarch::Install;
use super::{Run, describe, file_name, md5_hex, names, write_atomic};
use crate::romm::models::SaveAsset;

/// The core a game runs under on this device: what the states it makes are tagged with on RomM,
/// and what tells the states it can load from the ones it cannot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Core {
    /// Its id, `swanstation` — the library's file name, the spelling RomM's web player and its
    /// streams use too. None when the launch does not say, and then every state is offered.
    id: Option<String>,
    /// RetroArch's folder name for it, `SwanStation`, for saying which core this is.
    folder: Option<String>,
    /// Every tag that names this core, folded — see [`fold`]: its id, its folder name (what
    /// RetroArch Cloud Sync files a core RomM has no id for under), and the ids of the other builds
    /// filed in the same folder, whose states it loads.
    tags: Vec<String>,
}

impl Core {
    /// The core `id` as `install` knows it — its folder, and the builds that share it.
    pub fn of(id: Option<&str>, install: &Install) -> Self {
        let Some(id) = id else { return Core::default() };
        let folder = install.core_name(id).map(str::to_string);
        let mut tags = vec![fold(id)];
        if let Some(folder) = folder.as_deref() {
            tags.push(fold(folder));
            tags.extend(install.core_builds(folder).into_iter().map(fold));
        }
        tags.sort_unstable();
        tags.dedup();
        Core { id: Some(id.to_string()), folder, tags }
    }

    /// The core a downloaded game runs under here, as a launch of it would. Reads ES-DE's tables and
    /// RetroArch's config, so off the UI thread.
    pub fn for_game(settings: &crate::config::Settings, installed: &crate::downloads::Installed) -> Self {
        let plan = crate::launch::Plan::new(settings, installed);
        match (plan.emulator.syncs(), super::retroarch::locate(settings.retroarch_config.as_deref())) {
            (Some(Some(id)), Ok(install)) => Core::of(Some(id), &install),
            _ => Core::default(),
        }
    }

    /// What an upload is tagged with: the core, or RetroArch when it is not known.
    pub fn tag(&self) -> &str {
        self.id.as_deref().unwrap_or("retroarch")
    }

    /// The core by the name RetroArch gives it, for saying which it is.
    pub fn name(&self) -> Option<&str> {
        self.folder.as_deref().or(self.id.as_deref())
    }

    /// Whether this core loads `state`. Its `emulator` says which core made it; RomM's own streams
    /// file a capture under `retroarch` when they cannot name the core's folder, and name the core
    /// apart, in `core`. A state that names no core at all — an upload whose core was not known,
    /// tagged `retroarch` — may be anyone's, and is offered.
    pub fn loads(&self, state: &SaveAsset) -> bool {
        self.id.is_none() || made_by(state).is_none_or(|tag| self.tags.contains(&fold(tag)))
    }
}

/// The core a RomM state names as its maker, if it names one.
pub fn made_by(state: &SaveAsset) -> Option<&str> {
    fn named(tag: Option<&str>) -> Option<&str> {
        tag.map(str::trim).filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("retroarch"))
    }
    named(state.emulator.as_deref()).or_else(|| named(state.core.as_deref()))
}

/// One core's tag as every client spells it: case aside, a library's `_libretro` suffix aside, and
/// the Beetle cores under the `mednafen_` ids their libraries carry, which RomM files their
/// `beetle_` spellings with.
fn fold(tag: &str) -> String {
    let tag = tag.trim().to_lowercase();
    let tag = tag.strip_suffix("_libretro").unwrap_or(&tag);
    match tag.strip_prefix("beetle_") {
        Some(rest) => format!("mednafen_{rest}"),
        None => tag.to_string(),
    }
}

/// One of RetroArch's save state slots for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// Slot N: `<rom>.state` for 0, `<rom>.stateN` past it.
    Number(u32),
    /// `<rom>.state.auto`: what "save state on close" writes, and "load state on start" reads.
    Auto,
}

impl Slot {
    /// The slot `name` is, of the game whose content stem is `stem` — `<stem>.state`, `<stem>.stateN`,
    /// `<stem>.state.auto` — if it is one. Not a thumbnail beside one, nor a `.state0` or `.state07`,
    /// which RetroArch never writes.
    pub fn of_file(name: &str, stem: &str) -> Option<Slot> {
        let rest = name.strip_prefix(stem)?.to_lowercase();
        match rest.strip_prefix(".state")? {
            "" => Some(Slot::Number(0)),
            ".auto" => Some(Slot::Auto),
            digits => digits.parse::<u32>().ok().filter(|n| *n > 0 && n.to_string() == digits).map(Slot::Number),
        }
    }

    /// The file RetroArch keeps it in.
    pub fn file(self, stem: &str) -> String {
        match self {
            Slot::Number(0) => format!("{stem}.state"),
            Slot::Number(n) => format!("{stem}.state{n}"),
            Slot::Auto => format!("{stem}.state.auto"),
        }
    }

    /// The name it goes up to RomM under, made by the core tagged `core`: the same every time.
    pub fn remote_name(self, stem: &str, core: &str) -> String {
        match self {
            Slot::Number(n) => format!("{stem}.slot{n}.{core}{OWN}"),
            Slot::Auto => format!("{stem}.auto.{core}{OWN}"),
        }
    }

    /// The slot a state's name on RomM says it is, and the core it says made it, when it is named
    /// the way [`Slot::remote_name`] names one.
    pub fn of_remote(name: &str) -> Option<(Slot, &str)> {
        let rest = strip_suffix_ci(name, OWN)?;
        let (rest, core) = rest.rsplit_once('.')?;
        let (_, slot) = rest.rsplit_once('.')?;
        // A core's tag is a library's id: letters, digits, `_` and `-`. Anything else in that place
        // is a name of another shape — the old `<rom>.slot2.deckhand.state` among them.
        let tag = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if core.is_empty() || !core.chars().all(tag) || numbered(core).is_some() {
            return None;
        }
        let slot = if slot.eq_ignore_ascii_case("auto") { Slot::Auto } else { Slot::Number(numbered(slot)?) };
        Some((slot, core))
    }

    /// How the Saves page names it.
    pub fn label(self) -> String {
        match self {
            Slot::Number(n) => format!("Slot {n}"),
            Slot::Auto => "Auto slot".into(),
        }
    }

    /// How a sentence names it: `slot 3`, `the auto slot`.
    pub fn in_words(self) -> String {
        match self {
            Slot::Number(n) => format!("slot {n}"),
            Slot::Auto => "the auto slot".into(),
        }
    }

    /// The slot as RetroArch's menu numbers it: its number, or `auto`.
    pub fn number(self) -> String {
        match self {
            Slot::Number(n) => n.to_string(),
            Slot::Auto => "auto".into(),
        }
    }
}

/// N from `slotN`, whatever its case.
fn numbered(part: &str) -> Option<u32> {
    let digits = part.get(..4).filter(|p| p.eq_ignore_ascii_case("slot")).and(part.get(4..))?;
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
}

/// Where the Saves page puts a state from RomM, on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The slot past every numbered one here and on RomM: one nothing is in, on either side.
    Next,
    /// Slot 0.
    Zero,
    /// The auto slot, which RetroArch loads by itself on start when "load state on start" is on.
    Auto,
}

/// Where RetroArch keeps a state file's thumbnail: beside it, `.png` appended.
fn thumbnail(state: &Path) -> PathBuf {
    let mut name = state.file_name().unwrap_or_default().to_os_string();
    name.push(".png");
    state.with_file_name(name)
}

/// Where a pulled state's picture goes for its emulator to show: RetroArch's [`thumbnail`] beside a
/// `.state`, and PPSSPP's `<stem>.jpg` beside a `.ppst`. PPSSPP tells a PNG from a JPEG by its
/// content, so RomM's picture is written as it comes, whichever it is. None for an emulator that
/// reads no picture beside its state: there it would only be a stray file.
fn picture_beside(state: &Path) -> Option<PathBuf> {
    let name = file_name(state);
    if name.contains(".state") {
        Some(thumbnail(state))
    } else if name.ends_with(".ppst") {
        Some(state.with_extension("jpg"))
    } else {
        None
    }
}

/// The biggest picture read out of a state: PCSX2's is a few hundred KB.
const MOST_PICTURE: u64 = 16 << 20;

/// The picture an emulator took with `state`, and its extension: PCSX2's `Screenshot.png` inside the
/// `.p2s`, which is a zip, PPSSPP's `.jpg` beside the `.ppst`, DuckStation's in the `.sav`'s own
/// header ([`duckstation_picture`]), and otherwise RetroArch's [`thumbnail`]. Sent with every upload:
/// without one RomM keeps the slot's old picture, and a state that synced looks as if it had not.
fn picture(state: &Path) -> Option<(Vec<u8>, &'static str)> {
    let name = file_name(state);
    if name.ends_with(".sav") {
        return duckstation_picture(&fs::read(state).ok()?).map(|png| (png, "png"));
    }
    if name.ends_with(".p2s") {
        let mut zip = zip::ZipArchive::new(fs::File::open(state).ok()?).ok()?;
        let entry = zip.by_name("Screenshot.png").ok()?;
        let mut png = Vec::new();
        entry.take(MOST_PICTURE).read_to_end(&mut png).ok()?;
        return Some((png, "png"));
    }
    if name.ends_with(".ppst") {
        return fs::read(state.with_extension("jpg")).ok().map(|jpg| (jpg, "jpg"));
    }
    fs::read(thumbnail(state)).ok().map(|png| (png, "png"))
}

/// `DUCC`, which a DuckStation state starts with.
const DUCKSTATION_MAGIC: u32 = 0x4343_5544;

/// The first version whose header says how its picture is compressed. Before it, the picture is
/// stored raw, and the field is unused.
const DUCKSTATION_COMPRESSED_PICTURE: u32 = 69;

/// DuckStation's picture in `state`, a PNG. DuckStation stores it as raw RGBA pixels after its header
/// (`SAVE_STATE_HEADER` in its `save_state_version.h`): little-endian `u32`s for its magic and
/// version, then the title (128 bytes) and serial (32), then the media path's length, offset and
/// subimage, then the picture's compression, width, height, stored size and offset. Compressed with
/// zstd by default, or with zlib. A state compressed with XZ, or anything unexpected, gives none.
fn duckstation_picture(state: &[u8]) -> Option<Vec<u8>> {
    let word = |at: usize| state.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // The fields after the title and serial.
    let field = |n: usize| word(8 + 128 + 32 + 4 * n);
    if word(0)? != DUCKSTATION_MAGIC {
        return None;
    }
    let (width, height) = (field(4)?, field(5)?);
    if width == 0 || height == 0 || width >= 32768 || height >= 32768 {
        return None;
    }
    let raw = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if raw as u64 > MOST_PICTURE * 4 {
        return None;
    }
    let (compression, stored) =
        if word(4)? >= DUCKSTATION_COMPRESSED_PICTURE { (field(3)?, field(6)? as usize) } else { (0, raw) };
    let offset = field(7)? as usize;
    let stored = state.get(offset..offset.checked_add(stored)?)?;
    let mut pixels = Vec::with_capacity(raw);
    let limit = raw as u64 + 1;
    match compression {
        0 => pixels.extend_from_slice(stored),
        1 => {
            flate2::read::ZlibDecoder::new(stored).take(limit).read_to_end(&mut pixels).ok()?;
        }
        2 => {
            ruzstd::decoding::StreamingDecoder::new(stored).ok()?.take(limit).read_to_end(&mut pixels).ok()?;
        }
        _ => return None,
    }
    if pixels.len() != raw {
        return None;
    }
    // Its alpha is not meant to be seen through.
    let rgb = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, pixels)?).into_rgb8();
    let mut png = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// What marks a state this app uploaded — and its name before the rename, still recognised.
const OWN: &str = ".deckhand.state";
const LEGACY: &str = ".romsync.state";

/// Whether a server state is one RetroArch can be handed at all. A standalone emulator's states
/// (`.p2s`, `.sav`, a serial-keyed bundle) are not.
fn is_state(name: &str) -> bool {
    name.to_lowercase().ends_with(".state")
}

fn is_own(name: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with(OWN) || name.ends_with(LEGACY)
}

/// `name` without `suffix` on its end, whatever the case of either.
fn strip_suffix_ci<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    (name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(suffix)).then(|| &name[..cut])
}

/// Whether `name` is a save state RetroArch keeps for the content `stem`: one of its [`Slot`]s.
///
/// What tells a changed file in the state folder from another game's — see [`super::changed_states`].
pub(super) fn is_slot_of(name: &str, stem: &str) -> bool {
    Slot::of_file(name, stem).is_some()
}

/// The slot an old name gives a state — `<rom>.slotN.deckhand.state`, slot 1 to 4 — from when the
/// player's slots were four, named the same for every core.
fn old_slot(name: &str) -> Option<u32> {
    let rest = strip_suffix_ci(name, OWN).or_else(|| strip_suffix_ci(name, LEGACY)).or_else(|| strip_suffix_ci(name, ".state"))?;
    let (_, slot) = rest.rsplit_once('.')?;
    numbered(slot).filter(|n| (1..=4).contains(n))
}

/// Whether a state is one of this app's old slot-0 history, `<rom> [stamp].deckhand.state`.
fn old_history(name: &str) -> bool {
    is_own(name) && Slot::of_remote(name).is_none() && old_slot(name).is_none()
}

/// The slot a state on RomM is, for the core tagged `core`: by its own name, or by an old one that
/// stands in for it ([`stand_in`]).
fn slot_of(state: &SaveAsset, core: &str) -> Option<Slot> {
    match Slot::of_remote(&state.file_name) {
        Some((slot, by)) => (fold(by) == fold(core)).then_some(slot),
        None if old_history(&state.file_name) => Some(Slot::Number(0)),
        None => old_slot(&state.file_name).map(Slot::Number),
    }
}

/// RomM's state for `slot`, among `server` — the states the core tagged `core` loads: the newest
/// under the slot's own name, or while there is none, the one that stands in for it.
fn remote_for<'s>(server: &'s [SaveAsset], slot: Slot, core: &str) -> Option<&'s SaveAsset> {
    let own = server.iter().filter(|s| Slot::of_remote(&s.file_name).is_some_and(|(of, by)| of == slot && fold(by) == fold(core)));
    newest(own).or_else(|| stand_in(server, slot))
}

/// The state that stands in for `slot` while RomM has none under the slot's own name: for slots 1
/// to 4 the newest under the old name they shared, and for slot 0 the newest of this app's old
/// history, which slot 0 was. None for any other.
fn stand_in(server: &[SaveAsset], slot: Slot) -> Option<&SaveAsset> {
    match slot {
        Slot::Number(0) => newest(server.iter().filter(|s| old_history(&s.file_name))),
        Slot::Number(n) => newest(server.iter().filter(|s| old_slot(&s.file_name) == Some(n))),
        Slot::Auto => None,
    }
}

/// The newest of `states`, by RomM's time and then id.
fn newest<'a>(states: impl Iterator<Item = &'a SaveAsset>) -> Option<&'a SaveAsset> {
    states.max_by_key(|s| (s.updated_at.as_deref().and_then(names::parse_timestamp), s.id))
}

/// Whether RomM's `remote` is still the version `agreed` records. A state sent up again keeps its
/// id, so its time is what says it moved — to the second, since RomM answers an upload to the
/// microsecond and keeps the whole seconds of it. An entry from before the time was kept goes by the
/// id alone.
fn same_version(agreed: &Entry, remote: &SaveAsset) -> bool {
    let second = |t: Option<&str>| {
        let t = t.and_then(names::parse_timestamp)?;
        Some(t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    };
    agreed.save_id == remote.id
        && match (second(agreed.synced_at.as_deref()), second(remote.updated_at.as_deref())) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// The game's slots on this device that hold a state, with when RetroArch wrote each, in slot order.
pub fn on_disk(dir: &Path, stem: &str) -> Vec<(Slot, SystemTime)> {
    let mut found: Vec<(Slot, SystemTime)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let slot = Slot::of_file(&entry.file_name().to_string_lossy(), stem)?;
            let meta = fs::metadata(entry.path()).ok().filter(|m| m.is_file() && m.len() > 0)?;
            Some((slot, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    found.sort();
    found
}

/// Which of RetroArch's slots each of a game's states on RomM is here, by id — for the Saves page,
/// which says where to load one: the pairing a sync makes ([`remote_for`]). `states` newest first,
/// across the game's builds. Another core's and another emulator's are in none.
pub fn layout(states: &[SaveAsset], core: &Core) -> HashMap<i64, Slot> {
    let tag = core.tag();
    let ours: Vec<&SaveAsset> = states.iter().filter(|s| is_state(&s.file_name) && core.loads(s)).collect();
    // A slot's own state first; an old one stands in only for a slot that has none.
    let own = ours.iter().filter_map(|s| Some((*s, Slot::of_remote(&s.file_name).filter(|(_, by)| fold(by) == fold(tag))?.0)));
    let old = ours.iter().filter_map(|s| Some((*s, Slot::Number(old_slot(&s.file_name)?))));
    let history = ours.iter().filter(|s| old_history(&s.file_name)).map(|s| (*s, Slot::Number(0)));
    let mut claimed = HashSet::new();
    let mut out = HashMap::new();
    for (state, slot) in own.chain(old).chain(history) {
        if claimed.insert(slot) {
            out.insert(state.id, slot);
        }
    }
    out
}

/// One slot, as a sync of it sees it: the file here, and the state on RomM it is paired with.
struct Pair<'s> {
    /// The file the index is keyed by, and the one a pull writes.
    key: PathBuf,
    /// The same file when it holds a state.
    live: Option<PathBuf>,
    /// The name it goes up under.
    upload_as: String,
    remote: Option<&'s SaveAsset>,
}

impl Run<'_> {
    /// Sync the states of the game whose content stem is `stem`, kept in `dir`, run by `core` — the
    /// one whose states are synced, and what this device's are tagged with.
    pub(super) async fn states(&mut self, dir: &Path, stem: &str, rom_id: i64, core: &Core) {
        let (server, foreign) = match self.fetch_states(rom_id, core).await {
            Ok(states) => states,
            Err(why) => return self.report.fail(Some(rom_id), &format!("{stem}.state"), true, why),
        };
        let tag = core.tag();
        let here = on_disk(dir, stem);

        // Another core's newest state, when it is newer than anything this one has here or on RomM:
        // progress made elsewhere that cannot come here, which the player may be looking for.
        let time = |s: &SaveAsset| s.updated_at.as_deref().and_then(names::parse_timestamp);
        let newest_here = here.iter().map(|(_, at)| *at).max();
        if let Some(other) = newest(foreign.iter())
            && let Some(at) = time(other)
            && newest(server.iter()).and_then(time).is_none_or(|t| at > t)
            && newest_here.is_none_or(|t| at > t)
        {
            let maker = made_by(other).unwrap_or("another core");
            let here = core.name().unwrap_or("this core");
            self.report.left(Some(rom_id), &other.file_name, true, format!("Made with {maker}, which {here} can't load, so it stays on RomM"));
            self.report.other_cores.push(format!("a newer save state made with {maker} stays on RomM — {here} can't load it"));
        }

        // What the slots here agreed on with this app's old history: content RomM has already.
        let old: HashSet<i64> = server.iter().filter(|s| old_history(&s.file_name)).map(|s| s.id).collect();
        let held: Vec<Entry> = self.index.iter().filter(|(_, e)| e.rom_id == rom_id && old.contains(&e.save_id)).map(|(_, e)| e.clone()).collect();

        // Every slot on either side.
        let slots: BTreeSet<Slot> = here.iter().map(|(slot, _)| *slot).chain(server.iter().filter_map(|s| slot_of(s, tag))).collect();
        for slot in slots {
            let key = dir.join(slot.file(stem));
            let live = here.iter().any(|(s, _)| *s == slot).then(|| key.clone());
            let pair = Pair { key, live, upload_as: slot.remote_name(stem, tag), remote: remote_for(&server, slot, tag) };
            self.state(pair, rom_id, tag, &held).await;
        }
    }

    /// The game's RetroArch states on RomM: those `core` loads, and the rest.
    async fn fetch_states(&mut self, rom_id: i64, core: &Core) -> Result<(Vec<SaveAsset>, Vec<SaveAsset>), String> {
        let all = self.client.fetch_states(self.connection, rom_id).await.map_err(|e| describe(&e))?;
        Ok(all.into_iter().filter(|s| s.rom_id.is_none_or(|id| id == rom_id) && is_state(&s.file_name)).partition(|s| core.loads(s)))
    }

    /// Put RomM's `state` into `target`, one of this device's slots for the game whose content stem
    /// is `stem` — the Saves page's doing — and send it up as that slot's own. What the slot held is
    /// kept in `save-backups/` first. Answers the slot it went into.
    pub(super) async fn put(&mut self, dir: &Path, stem: &str, rom_id: i64, core: &Core, state: &SaveAsset, target: Target) -> Result<Slot, String> {
        let tag = core.tag();
        let (server, _) = self.fetch_states(rom_id, core).await?;
        let slot = match target {
            Target::Zero => Slot::Number(0),
            Target::Auto => Slot::Auto,
            Target::Next => {
                let here = on_disk(dir, stem).into_iter().map(|(slot, _)| slot);
                let there = server.iter().filter_map(|s| slot_of(s, tag));
                let top = here.chain(there).filter_map(|slot| match slot {
                    Slot::Number(n) => Some(n),
                    Slot::Auto => None,
                });
                Slot::Number(top.max().map_or(1, |n| n + 1))
            }
        };
        let key = dir.join(slot.file(stem));
        let name = file_name(&key);
        let bytes = self.client.download_state(self.connection, state).await.map_err(|e| describe(&e))?;
        if let Ok(held) = fs::read(&key)
            && !held.is_empty()
            && held != bytes
        {
            let kept = self.back_up(&key, &held, &name).ok_or_else(|| format!("could not keep a copy of what {name} held"))?;
            self.report.notes.push(format!("what {name} held is at {}", kept.display()));
        }
        fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        write_atomic(&key, &bytes).map_err(|e| format!("could not write {name}: {e}"))?;
        self.report.written.push(key.clone());
        // RomM's picture of it beside it — or none, rather than the picture of what it replaced.
        if state.screenshot.is_some() {
            self.pull_thumbnail(&key, state).await;
        } else if fs::remove_file(thumbnail(&key)).is_ok() {
            self.report.written.push(thumbnail(&key));
        }
        let pair = Pair { live: Some(key.clone()), key, upload_as: slot.remote_name(stem, tag), remote: remote_for(&server, slot, tag) };
        self.state(pair, rom_id, tag, &[]).await;
        Ok(slot)
    }

    /// One state file that is a save of its own — a standalone emulator's slot, uploaded under its
    /// own name every time. See [`super::standalone`].
    pub(super) async fn state_file(&mut self, path: PathBuf, remote: Option<&SaveAsset>, rom_id: i64, emulator: &str) {
        let live = fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0).then(|| path.clone());
        let pair = Pair { upload_as: file_name(&path), key: path, live, remote };
        self.state(pair, rom_id, emulator, &[]).await;
    }

    /// Sync one slot. `held` is what slots of this game's agreed on with RomM states of this app's
    /// old history — content RomM has already, which a slot holding it as it is does not send again.
    async fn state(&mut self, pair: Pair<'_>, rom_id: i64, emulator: &str, held: &[Entry]) {
        let name = file_name(&pair.key);
        let agreed = self.index.agreed(&pair.key, rom_id, self.stamp.as_ref()).cloned();
        let Some(live) = pair.live.clone() else {
            match pair.remote {
                // Deleted here since the two last agreed. RomM's stays, and stays there, unless it
                // changes: then it is another state, which comes down as one would to a fresh device.
                Some(remote) if agreed.as_ref().is_some_and(|e| same_version(e, remote)) => {}
                Some(remote) => self.pull_state(&pair, rom_id, remote, &name, None).await,
                None => {}
            }
            return;
        };
        let bytes = match fs::read(&live) {
            Ok(bytes) => bytes,
            Err(e) => return self.report.fail(Some(rom_id), &name, true, e),
        };
        let hash = md5_hex(&bytes);
        let local_changed = agreed.as_ref().is_none_or(|e| e.hash != hash);
        let server_changed = pair.remote.is_some_and(|r| agreed.as_ref().is_none_or(|e| !same_version(e, r)));
        match (local_changed, server_changed, pair.remote) {
            (false, false, _) => self.report.agreed(Some(rom_id), &name, true),
            (true, false, remote) => {
                // A slot new to the index that holds one of the old history's states as it came: RomM
                // has it, and this slot sends its own up the next time it changes.
                if agreed.is_none()
                    && remote.is_none()
                    && let Some(old) = held.iter().find(|e| e.hash == hash)
                {
                    if self.remember_state(&pair.key, rom_id, hash, old.save_id, old.synced_at.clone(), &name) {
                        self.report.agreed(Some(rom_id), &name, true);
                    }
                    return;
                }
                self.push_state(&pair, rom_id, emulator, bytes, hash, &name).await;
            }
            (false, true, Some(remote)) => self.pull_state(&pair, rom_id, remote, &name, Some(&hash)).await,
            (true, true, Some(remote)) => {
                let Some(theirs) = self.download_state(remote, rom_id, &name).await else { return };
                if md5_hex(&theirs) == hash {
                    if self.remember_state(&pair.key, rom_id, hash, remote.id, remote.updated_at.clone(), &name) {
                        self.report.agreed(Some(rom_id), &name, true);
                    }
                    return;
                }
                let local_time = fs::metadata(&live).and_then(|m| m.modified()).ok();
                let remote_time = remote.updated_at.as_deref().and_then(names::parse_timestamp);
                if super::choose(&name, local_time, remote_time, bytes.len() as u64).await == super::Choice::Remote {
                    let Some(kept) = self.back_up(&live, &bytes, &name) else { return };
                    if self.write_state(&pair, rom_id, remote, &theirs, &name) {
                        let said = format!("{name}: kept RomM's state; this device's is at {}", kept.display());
                        self.report.settled(Some(rom_id), &name, true, false, Some(kept), said);
                    }
                } else if remote.file_name.eq_ignore_ascii_case(&pair.upload_as) {
                    // The push replaces RomM's where it is, so its copy is kept here first.
                    let Some(kept) = self.back_up(&live, &theirs, &name) else { return };
                    if self.push_state(&pair, rom_id, emulator, bytes, hash, &name).await {
                        let said = format!("{name}: kept this device's state; RomM's is at {}", kept.display());
                        self.report.settled(Some(rom_id), &name, true, true, None, said);
                    }
                } else if self.push_state(&pair, rom_id, emulator, bytes, hash, &name).await {
                    self.report.settled(Some(rom_id), &name, true, true, None, format!("{name}: kept this device's state; RomM keeps the other"));
                }
            }
            (_, true, None) => unreachable!("a server change with no server state"),
        }
    }

    /// Send a slot up under its name, answering whether it went.
    #[allow(clippy::too_many_arguments)]
    async fn push_state(&mut self, pair: &Pair<'_>, rom_id: i64, emulator: &str, bytes: Vec<u8>, hash: String, name: &str) -> bool {
        // RetroArch writes the thumbnail beside whichever file it saved: `.state.png`, `.state.auto.png`.
        let shot = pair.live.as_ref().and_then(|live| picture(live));
        // RetroArch's own state with none beside it: its state pictures are off.
        let pictureless = shot.is_none() && pair.live.as_ref().is_some_and(|live| file_name(live).contains(".state"));
        let created = match self.client.upload_state(self.connection, rom_id, emulator, &pair.upload_as, bytes, shot).await {
            Ok(created) => created,
            Err(e) => {
                self.report.fail(Some(rom_id), name, true, describe(&e));
                return false;
            }
        };
        if pictureless {
            self.report.pictureless += 1;
        }
        if self.remember_state(&pair.key, rom_id, hash, created.id, created.updated_at.clone(), name) {
            self.report.up(Some(rom_id), name, true);
            return true;
        }
        false
    }

    /// Bring RomM's state for a slot down into it. `here` is the hash of what the slot holds, when it
    /// holds something: the same content come back under a newer time — sent up again elsewhere — is
    /// agreed on, and not written over itself.
    async fn pull_state(&mut self, pair: &Pair<'_>, rom_id: i64, remote: &SaveAsset, name: &str, here: Option<&str>) {
        let Some(bytes) = self.download_state(remote, rom_id, name).await else { return };
        let hash = md5_hex(&bytes);
        if here == Some(hash.as_str()) {
            if self.remember_state(&pair.key, rom_id, hash, remote.id, remote.updated_at.clone(), name) {
                self.report.agreed(Some(rom_id), name, true);
            }
            return;
        }
        if self.write_state(pair, rom_id, remote, &bytes, name) {
            self.report.down(Some(rom_id), name, true);
            self.pull_thumbnail(&pair.key, remote).await;
        }
    }

    async fn download_state(&mut self, remote: &SaveAsset, rom_id: i64, name: &str) -> Option<Vec<u8>> {
        match self.client.download_state(self.connection, remote).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                self.report.fail(Some(rom_id), name, true, describe(&e));
                None
            }
        }
    }

    /// `remote`'s content into the slot, dated as RomM dates it.
    fn write_state(&mut self, pair: &Pair<'_>, rom_id: i64, remote: &SaveAsset, bytes: &[u8], name: &str) -> bool {
        if let Err(e) = write_atomic(&pair.key, bytes) {
            self.report.fail(Some(rom_id), name, true, e);
            return false;
        }
        if let Some(time) = remote.updated_at.as_deref().and_then(names::parse_timestamp)
            && let Ok(file) = fs::File::options().write(true).open(&pair.key)
        {
            let _ = file.set_modified(time);
        }
        self.report.written.push(pair.key.clone());
        self.remember_state(&pair.key, rom_id, md5_hex(bytes), remote.id, remote.updated_at.clone(), name)
    }

    /// The state's thumbnail from RomM, beside the slot file, where RetroArch's menu shows it. A
    /// thumbnail is not progress: one that fails is skipped without a word.
    async fn pull_thumbnail(&mut self, key: &Path, remote: &SaveAsset) {
        let Some(shot) = picture_beside(key) else { return };
        let Some(path) = remote.screenshot.as_ref().and_then(|s| s.download_path.as_deref()).filter(|p| !p.trim().is_empty()) else {
            // PPSSPP would go on showing the replaced state's picture over this one.
            if file_name(key).ends_with(".ppst") && fs::remove_file(&shot).is_ok() {
                self.report.written.push(shot);
            }
            return;
        };
        let host = self.connection.host.trim_end_matches('/');
        let url = if path.starts_with("http") { path.to_string() } else { format!("{host}/{}", path.trim_start_matches('/')) };
        if let Ok(bytes) = self.client.fetch_asset(self.connection, &url).await
            && write_atomic(&shot, &bytes).is_ok()
        {
            // The watcher sees the state folder, and `<state>.png` holds `.state` in its name:
            // left unsaid, a thumbnail this sync fetched would read back as a state to sync.
            self.report.written.push(shot);
        }
    }

    /// The slot and RomM agree on `hash`, RomM's state `save_id` as it was at `synced_at`.
    fn remember_state(&mut self, key: &Path, rom_id: i64, hash: String, save_id: i64, synced_at: Option<String>, name: &str) -> bool {
        match self.index.set(key, Entry { rom_id, hash, save_id, synced_at, stamp: self.stamp.clone(), ..Default::default() }) {
            Ok(()) => true,
            Err(e) => {
                self.report.fail(Some(rom_id), name, true, format!("synced, but could not record it ({e})"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Connection;
    use crate::fake_romm::FakeRomm;
    use crate::romm::RommClient;
    use crate::stub_server;

    const CHRONO: &str = "Chrono Trigger (USA)";

    /// A RetroArch that keeps states by content folder, and a game in `snes/`.
    struct Device(PathBuf);

    impl Device {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-states-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("ra")).unwrap();
            fs::write(
                root.join("ra/retroarch.cfg"),
                format!(
                    "savefile_directory = \"{0}/saves\"\nsavestate_directory = \"{0}/states\"\n\
                     sort_savestates_by_content_enable = \"true\"\n",
                    root.display()
                ),
            )
            .unwrap();
            Self(root)
        }

        fn state(&self, name: &str) -> PathBuf {
            self.0.join("states/snes").join(name)
        }

        async fn sync(&self, host: String) -> super::super::Report {
            let install = super::super::retroarch::read(&self.0.join("ra/retroarch.cfg")).unwrap();
            let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
            let rom = self.0.join(format!("games/snes/{CHRONO}.sfc"));
            super::super::sync_rom(&RommClient::new().unwrap(), &conn, &install, &self.0.join("store"), &rom, "snes", Some("snes9x"), 1, None, Vec::new())
                .await
                .unwrap()
        }

        /// The Saves page's "put into a slot", for `state`.
        async fn put(&self, host: String, state: &SaveAsset, target: Target) -> Result<(Slot, super::super::Report), String> {
            let install = super::super::retroarch::read(&self.0.join("ra/retroarch.cfg")).unwrap();
            let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
            let (client, dir) = (RommClient::new().unwrap(), self.0.join("states/snes"));
            let mut run = Run::with_index(&client, &conn, &self.0.join("store"), dir.clone(), "state-index.json");
            let slot = run.put(&dir, CHRONO, 1, &Core::of(Some("snes9x"), &install), state, target).await?;
            Ok((slot, run.report))
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The name this app sends `slot` up under, from snes9x.
    fn own(slot: &str) -> String {
        format!("{CHRONO}.{slot}.snes9x.deckhand.state")
    }

    fn write(path: PathBuf, bytes: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn named(romm: &FakeRomm, name: &str) -> crate::fake_romm::State {
        romm.states(1).into_iter().find(|s| s.file_name == name).unwrap_or_else(|| panic!("{name} on RomM"))
    }

    #[tokio::test]
    async fn every_slot_goes_up_under_a_name_of_its_own_and_comes_down_into_the_same_slot() {
        let romm = FakeRomm::start().await;
        let a = Device::new("own-a");
        for (file, bytes) in [("state", "zero"), ("state.auto", "on close"), ("state3", "three"), ("state12", "twelve")] {
            write(a.state(&format!("{CHRONO}.{file}")), bytes);
        }
        fs::write(a.state(&format!("{CHRONO}.state3.png")), b"\x89PNG three").unwrap();
        let report = a.sync(romm.host.clone()).await;
        assert_eq!(report.pushed.len(), 4, "{report:?}");
        assert_eq!(report.pictureless, 3, "the three with no picture beside them: RetroArch's are off");
        let mut names: Vec<String> = romm.states(1).into_iter().map(|s| s.file_name).collect();
        names.sort();
        assert_eq!(names, [own("auto"), own("slot0"), own("slot12"), own("slot3")], "past slot 4 too");
        assert!(romm.states(1).iter().all(|s| s.emulator.as_deref() == Some("snes9x")), "tagged with the core");
        assert_eq!(named(&romm, &own("slot3")).picture.map(|(_, p)| p), Some(b"\x89PNG three".to_vec()));

        let again = a.sync(romm.host.clone()).await;
        assert!(again.pushed.is_empty() && again.pulled.is_empty(), "{again:?}");
        assert_eq!(again.in_sync, 4);

        // Another device takes each into the slot it was in.
        let b = Device::new("own-b");
        let report = b.sync(romm.host.clone()).await;
        assert_eq!(report.pulled.len(), 4, "{report:?}");
        assert_eq!(fs::read_to_string(b.state(&format!("{CHRONO}.state12"))).unwrap(), "twelve");
        assert_eq!(fs::read_to_string(b.state(&format!("{CHRONO}.state.auto"))).unwrap(), "on close");
        assert_eq!(fs::read(b.state(&format!("{CHRONO}.state3.png"))).unwrap(), b"\x89PNG three");
        assert_eq!(romm.requests("POST /api/states/delete"), 0, "nothing is deleted on RomM");
    }

    #[tokio::test]
    async fn a_slot_saved_again_replaces_its_state_where_it_is_and_the_other_device_takes_it() {
        let romm = FakeRomm::start().await;
        let (a, b) = (Device::new("again-a"), Device::new("again-b"));
        write(a.state(&format!("{CHRONO}.state2")), "first");
        a.sync(romm.host.clone()).await;
        b.sync(romm.host.clone()).await;
        let id = named(&romm, &own("slot2")).id;

        write(a.state(&format!("{CHRONO}.state2")), "second");
        let report = a.sync(romm.host.clone()).await;
        assert_eq!(report.pushed, [format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(romm.states(1).len(), 1, "one state for the slot, however often it is saved");
        assert_eq!(named(&romm, &own("slot2")).id, id, "replaced where it is");

        // The id did not move; the time did, and that is what the other device goes by.
        let report = b.sync(romm.host.clone()).await;
        assert_eq!(report.pulled, [format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(fs::read_to_string(b.state(&format!("{CHRONO}.state2"))).unwrap(), "second");
    }

    #[tokio::test]
    async fn a_state_deleted_on_either_side_stays_on_the_other_and_does_not_come_back() {
        let romm = FakeRomm::start().await;
        let (a, b) = (Device::new("deleted-a"), Device::new("deleted-b"));
        write(a.state(&format!("{CHRONO}.state1")), "one");
        write(a.state(&format!("{CHRONO}.state2")), "two");
        a.sync(romm.host.clone()).await;

        // Deleted here: RomM keeps it, and the next sync does not bring it back.
        fs::remove_file(a.state(&format!("{CHRONO}.state1"))).unwrap();
        let report = a.sync(romm.host.clone()).await;
        assert!(report.pulled.is_empty() && report.pushed.is_empty(), "{report:?}");
        assert!(!a.state(&format!("{CHRONO}.state1")).exists());
        assert_eq!(romm.states(1).len(), 2);
        // Until another device saves into that slot: that is a state of its own, and comes down.
        b.sync(romm.host.clone()).await;
        write(b.state(&format!("{CHRONO}.state1")), "one, played on");
        b.sync(romm.host.clone()).await;
        let report = a.sync(romm.host.clone()).await;
        assert_eq!(report.pulled, [format!("{CHRONO}.state1")], "{report:?}");

        // Deleted on RomM: kept here, and not sent up again while it is as it was.
        romm.delete_state(named(&romm, &own("slot2")).id);
        let report = a.sync(romm.host.clone()).await;
        assert!(report.pushed.is_empty(), "{report:?}");
        assert_eq!(fs::read_to_string(a.state(&format!("{CHRONO}.state2"))).unwrap(), "two");
        // Saved into again, it goes up as a new state.
        write(a.state(&format!("{CHRONO}.state2")), "two again");
        let report = a.sync(romm.host.clone()).await;
        assert_eq!(report.pushed, [format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(romm.requests("POST /api/states/delete"), 0, "nothing is deleted on RomM");
    }

    #[tokio::test]
    async fn the_old_names_stand_in_until_a_slot_has_its_own_and_stay_on_romm() {
        let romm = FakeRomm::start().await;
        // What this app sent up before: a history for slot 0, and slot 2 under the name every core shared.
        romm.state_from(1, &format!("{CHRONO} [2026-06-01 10-00-00-000].deckhand.state"), "snes9x", b"older", None);
        romm.state_from(1, &format!("{CHRONO} [2026-07-01 10-00-00-000].deckhand.state"), "snes9x", b"newest", None);
        romm.state_from(1, &format!("{CHRONO}.slot2.deckhand.state"), "snes9x", b"slot two", None);
        // The web player's: in no slot, on the Saves page.
        romm.state_from(1, &format!("{CHRONO}.state"), "snes9x", b"web", None);

        let device = Device::new("old-names");
        let report = device.sync(romm.host.clone()).await;
        assert_eq!(report.pulled, [format!("{CHRONO}.state"), format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "newest", "slot 0 was the history");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state2"))).unwrap(), "slot two");
        assert!(!device.state(&format!("{CHRONO}.state5")).exists(), "the older history stays on RomM");

        // What slot 0 holds is on RomM already, and so is the same content in the auto slot.
        fs::copy(device.state(&format!("{CHRONO}.state")), device.state(&format!("{CHRONO}.state.auto"))).unwrap();
        let report = device.sync(romm.host.clone()).await;
        assert!(report.pushed.is_empty() && report.pulled.is_empty(), "{report:?}");
        assert_eq!(report.in_sync, 3);

        // Saved into, a slot goes up under its own name; the old state stays where it is.
        write(device.state(&format!("{CHRONO}.state2")), "slot two, played on");
        let report = device.sync(romm.host.clone()).await;
        assert_eq!(report.pushed, [format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(named(&romm, &own("slot2")).bytes, b"slot two, played on");
        assert_eq!(named(&romm, &format!("{CHRONO}.slot2.deckhand.state")).bytes, b"slot two");
        assert_eq!(romm.states(1).len(), 5);
        assert_eq!(romm.requests("POST /api/states/delete"), 0, "nothing is deleted on RomM");
    }

    #[tokio::test]
    async fn a_conflict_kept_here_keeps_romms_copy_here_before_replacing_it() {
        let romm = FakeRomm::start().await;
        let (a, b) = (Device::new("conflict-a"), Device::new("conflict-b"));
        write(a.state(&format!("{CHRONO}.state1")), "agreed");
        a.sync(romm.host.clone()).await;
        b.sync(romm.host.clone()).await;
        write(b.state(&format!("{CHRONO}.state1")), "b's");
        b.sync(romm.host.clone()).await;
        // Saved here since, and after RomM's: this side is the newer.
        write(a.state(&format!("{CHRONO}.state1")), "a's");
        let report = a.sync(romm.host.clone()).await;
        assert_eq!(report.conflicts.len(), 1, "{report:?}");
        assert!(report.conflicts[0].contains("RomM's is at"), "{report:?}");
        assert_eq!(named(&romm, &own("slot1")).bytes, b"a's");
        let backups: Vec<String> = fs::read_dir(a.0.join("store/save-backups"))
            .unwrap()
            .flatten()
            .map(|e| fs::read_to_string(e.path()).unwrap())
            .collect();
        assert_eq!(backups, ["b's"], "the copy RomM no longer has");
    }

    #[tokio::test]
    async fn another_cores_states_stay_on_romm_and_its_newer_one_is_mentioned() {
        let romm = FakeRomm::start().await;
        romm.state_from(1, &own("slot0"), "snes9x", b"snes9x's", None);
        romm.advance(std::time::Duration::from_secs(60));
        romm.state_from(1, &format!("{CHRONO}.slot0.bsnes.deckhand.state"), "bsnes", b"bsnes's", None);
        romm.state_from(1, &format!("{CHRONO}.slot2.bsnes.deckhand.state"), "bsnes", b"bsnes's two", None);
        let device = Device::new("other-core");
        let report = device.sync(romm.host.clone()).await;

        assert_eq!(report.pulled, [format!("{CHRONO}.state")], "{report:?}");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "snes9x's", "this core's own");
        assert!(!device.state(&format!("{CHRONO}.state2")).exists(), "bsnes's slot is not snes9x's");
        assert_eq!(report.other_cores.len(), 1, "{report:?}");
        assert!(report.other_cores[0].contains("bsnes"), "{report:?}");
    }

    #[tokio::test]
    async fn a_state_put_into_a_slot_lands_past_every_slot_and_goes_up_as_that_slot() {
        let romm = FakeRomm::start().await;
        let device = Device::new("put");
        write(device.state(&format!("{CHRONO}.state1")), "one");
        write(device.state(&format!("{CHRONO}.state3")), "three");
        // Slot 5 is on RomM, from another device, and not here.
        romm.state_from(1, &own("slot5"), "snes9x", b"five", None);
        let web = romm.state_from(1, &format!("{CHRONO}.state"), "snes9x", b"web", Some(b"\x89PNG web"));
        let web: SaveAsset = serde_json::from_value(serde_json::json!({
            "id": web, "rom_id": 1, "file_name": format!("{CHRONO}.state"), "emulator": "snes9x",
            "download_path": format!("/api/states/{web}/content"),
            "screenshot": { "download_path": format!("/api/screenshots/{}/content", named(&romm, &format!("{CHRONO}.state")).picture.unwrap().0) },
        }))
        .unwrap();

        let (slot, report) = device.put(romm.host.clone(), &web, Target::Next).await.unwrap();
        assert_eq!(slot, Slot::Number(6), "past slot 5 too, which is RomM's");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state6"))).unwrap(), "web");
        assert_eq!(fs::read(device.state(&format!("{CHRONO}.state6.png"))).unwrap(), b"\x89PNG web");
        assert_eq!(report.pushed, [format!("{CHRONO}.state6")], "{report:?}");
        assert_eq!(named(&romm, &own("slot6")).bytes, b"web");

        // Into a slot that holds something: that goes to save-backups first.
        let (slot, report) = device.put(romm.host.clone(), &web, Target::Auto).await.unwrap();
        assert_eq!(slot, Slot::Auto);
        assert!(report.notes.is_empty(), "nothing was there: {report:?}");
        write(device.state(&format!("{CHRONO}.state")), "zero");
        let (_, report) = device.put(romm.host.clone(), &web, Target::Zero).await.unwrap();
        assert!(report.notes.iter().any(|n| n.contains("save-backups")), "{report:?}");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "web");
        assert_eq!(romm.requests("POST /api/states/delete"), 0, "nothing is deleted on RomM");
    }

    #[test]
    fn the_saves_page_says_which_slot_each_state_is_in() {
        let by = |id: i64, name: String, emulator: &str| -> SaveAsset {
            serde_json::from_value(serde_json::json!({ "id": id, "file_name": name, "emulator": emulator })).unwrap()
        };
        // Newest first, as the page holds them.
        let states = vec![
            by(9, own("slot2"), "snes9x"),
            by(8, format!("{CHRONO}.slot2.deckhand.state"), "snes9x"),
            by(7, format!("{CHRONO}.slot3.deckhand.state"), "snes9x"),
            by(6, format!("{CHRONO} [2026-07-03 10-00-00-000].deckhand.state"), "snes9x"),
            by(5, format!("{CHRONO} [2026-07-02 10-00-00-000].deckhand.state"), "snes9x"),
            by(4, own("auto"), "snes9x"),
            by(3, format!("{CHRONO}.slot7.bsnes.deckhand.state"), "bsnes"),
            by(2, format!("{CHRONO}.state"), "snes9x"),
            by(1, "SLUS-20946 (2C6BE434).01.p2s".into(), "pcsx2"),
        ];
        let dir = std::env::temp_dir().join(format!("deckhand-states-layout-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("retroarch.cfg"), "").unwrap();
        let install = super::super::retroarch::read(&dir.join("retroarch.cfg")).unwrap();
        let _ = fs::remove_dir_all(&dir);
        let slots = layout(&states, &Core::of(Some("snes9x"), &install));
        assert_eq!(slots.get(&9), Some(&Slot::Number(2)), "the slot's own");
        assert_eq!(slots.get(&8), None, "an old name only stands in for a slot with none");
        assert_eq!(slots.get(&7), Some(&Slot::Number(3)));
        assert_eq!(slots.get(&6), Some(&Slot::Number(0)), "the old history's newest was slot 0");
        assert_eq!(slots.get(&5), None);
        assert_eq!(slots.get(&4), Some(&Slot::Auto));
        assert_eq!((slots.get(&3), slots.get(&2), slots.get(&1)), (None, None, None), "another core's, the web player's, PCSX2's");
    }

    #[test]
    fn a_state_loads_in_the_core_its_tag_names_whatever_the_spelling() {
        let root = std::env::temp_dir().join(format!("deckhand-states-cores-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("info")).unwrap();
        fs::write(root.join("retroarch.cfg"), format!("libretro_info_path = \"{}/info\"\n", root.display())).unwrap();
        for (id, name) in [
            ("swanstation", "SwanStation"),
            ("pcsx_rearmed", "PCSX-ReARMed"),
            ("mednafen_psx_hw", "Beetle PSX HW"),
            ("mupen64plus_next", "Mupen64Plus-Next"),
            ("mupen64plus_next_gles3", "Mupen64Plus-Next"),
        ] {
            fs::write(root.join(format!("info/{id}_libretro.info")), format!("corename = \"{name}\"\n")).unwrap();
        }
        let install = super::super::retroarch::read(&root.join("retroarch.cfg")).unwrap();
        let _ = fs::remove_dir_all(&root);
        let tagged = |emulator: &str, core: Option<&str>| -> SaveAsset {
            serde_json::from_value(serde_json::json!({ "id": 1, "file_name": "Game.state", "emulator": emulator, "core": core })).unwrap()
        };

        let swan = Core::of(Some("swanstation"), &install);
        assert_eq!((swan.tag(), swan.name()), ("swanstation", Some("SwanStation")));
        assert!(swan.loads(&tagged("swanstation", None)));
        assert!(swan.loads(&tagged("SwanStation", None)), "RetroArch's folder name, as Cloud Sync files a core");
        assert!(!swan.loads(&tagged("pcsx_rearmed", None)), "another PlayStation core's");
        assert!(swan.loads(&tagged("retroarch", None)), "an upload whose core was not known may be anyone's");
        assert!(swan.loads(&tagged("", None)));
        assert!(!swan.loads(&tagged("retroarch", Some("pcsx_rearmed"))), "a stream's capture names its core apart");
        assert!(swan.loads(&tagged("retroarch", Some("swanstation"))));

        let beetle = Core::of(Some("mednafen_psx_hw"), &install);
        assert!(beetle.loads(&tagged("beetle_psx_hw", None)), "RomM's other spelling of the Beetle cores");
        assert!(beetle.loads(&tagged("Beetle PSX HW", None)));

        let mupen = Core::of(Some("mupen64plus_next"), &install);
        assert!(mupen.loads(&tagged("mupen64plus_next_gles3", None)), "another build filed in the same folder");

        let unknown = Core::default();
        assert!(unknown.loads(&tagged("pcsx_rearmed", None)), "no core known: nothing to tell them apart by");
        assert_eq!(unknown.tag(), "retroarch");
    }

    #[test]
    fn a_slot_is_named_for_its_file_and_its_core_and_read_back_from_either() {
        let stem = "Chrono Trigger (USA)";
        for (file, slot) in [("state", Slot::Number(0)), ("state3", Slot::Number(3)), ("state120", Slot::Number(120)), ("state.auto", Slot::Auto)] {
            let name = format!("{stem}.{file}");
            assert_eq!(Slot::of_file(&name, stem), Some(slot), "{name}");
            assert_eq!(slot.file(stem), name);
            assert_eq!(Slot::of_remote(&slot.remote_name(stem, "mupen64plus_next")), Some((slot, "mupen64plus_next")));
        }
        assert_eq!(Slot::Number(3).remote_name(stem, "snes9x"), "Chrono Trigger (USA).slot3.snes9x.deckhand.state");
        assert_eq!(Slot::Auto.remote_name("Game v1.1", "snes9x"), "Game v1.1.auto.snes9x.deckhand.state");
        assert_eq!(Slot::of_remote("Game v1.1.auto.snes9x.deckhand.state"), Some((Slot::Auto, "snes9x")), "dots in the stem");

        for not_a_slot in ["state.png", "state3.png", "state.auto.png", "state0", "state07", "srm"] {
            assert_eq!(Slot::of_file(&format!("{stem}.{not_a_slot}"), stem), None, "{not_a_slot}");
        }
        assert!(is_slot_of("Chrono Trigger (USA).state7", stem));
        assert!(!is_slot_of("Secret of Mana (USA).state", stem), "another game's");

        // Names of other shapes are not read as a slot's own.
        for other in [
            "Game.slot2.deckhand.state",
            "Game [2026-07-01 10-00-00-000].deckhand.state",
            "Game.state",
            "Game.slot2.snes 9x.deckhand.state",
            "Game.slotmachine.snes9x.deckhand.state",
        ] {
            assert_eq!(Slot::of_remote(other), None, "{other}");
        }
    }

    #[test]
    fn the_old_names_are_read_for_the_slot_they_were() {
        assert_eq!(old_slot("Chrono Trigger (USA).slot2.deckhand.state"), Some(2));
        assert_eq!(old_slot("Game.slot4.romsync.state"), Some(4));
        assert_eq!(old_slot("Game.slot4.state"), Some(4));
        assert_eq!(old_slot("Game.slot7.deckhand.state"), None, "there were four");
        assert_eq!(old_slot("Game.slot2.snes9x.deckhand.state"), None, "a slot's own name");
        assert!(old_history("Game [2026-07-01 10-00-00-000].deckhand.state"));
        assert!(!old_history("Game.slot2.deckhand.state"));
        assert!(!old_history("Game.slot2.snes9x.deckhand.state"));
        assert!(!old_history("Game.state"), "the web player's");
        assert!(!is_state("Game.p2s"));
    }

    #[tokio::test]
    async fn a_pulled_ppsspp_state_brings_its_picture_where_ppsspp_shows_it() {
        let dir = std::env::temp_dir().join(format!("deckhand-ppsspp-pull-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let png = b"\x89PNG a picture".to_vec();
        let host = stub_server::start_bytes(vec![
            ("GET /api/raw/assets/states/7.ppst", 200, b"romm's state".to_vec()),
            ("GET /api/screenshots/5/content", 200, png.clone()),
        ])
        .await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let client = RommClient::new().unwrap();
        let remote = |screenshot: serde_json::Value| -> SaveAsset {
            serde_json::from_value(serde_json::json!({
                "id": 7, "rom_id": 1, "file_name": "ULUS99999_1.00_0.ppst", "emulator": "ppsspp",
                "updated_at": "2026-07-01T10:00:00Z", "download_path": "/api/raw/assets/states/7.ppst",
                "screenshot": screenshot,
            }))
            .unwrap()
        };
        let state = dir.join("ULUS99999_1.00_0.ppst");
        let jpg = dir.join("ULUS99999_1.00_0.jpg");

        let with = remote(serde_json::json!({ "download_path": "/api/screenshots/5/content" }));
        let mut run = Run::with_index(&client, &conn, &dir.join("store"), dir.clone(), "state-index.json");
        run.state_file(state.clone(), Some(&with), 1, "ppsspp").await;
        assert_eq!(run.report.pulled.len(), 1, "{:?}", run.report);
        assert_eq!(fs::read(&state).unwrap(), b"romm's state");
        assert_eq!(fs::read(&jpg).unwrap(), png, "PPSSPP's name, RomM's picture as it came");
        assert!(!dir.join("ULUS99999_1.00_0.ppst.png").exists(), "and no stray one");

        // A state with no picture on RomM does not keep the one it replaced.
        fs::remove_file(&state).unwrap();
        let without = remote(serde_json::Value::Null);
        let mut run = Run::with_index(&client, &conn, &dir.join("store2"), dir.clone(), "state-index.json");
        run.state_file(state.clone(), Some(&without), 1, "ppsspp").await;
        assert_eq!(run.report.pulled.len(), 1, "{:?}", run.report);
        assert!(!jpg.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pulled_states_picture_goes_where_its_emulator_reads_one() {
        let dir = Path::new("/states");
        assert_eq!(picture_beside(&dir.join("Game.state")), Some(dir.join("Game.state.png")));
        assert_eq!(picture_beside(&dir.join("Game.state3")), Some(dir.join("Game.state3.png")));
        assert_eq!(picture_beside(&dir.join("ULUS99999_1.00_0.ppst")), Some(dir.join("ULUS99999_1.00_0.jpg")));
        assert_eq!(picture_beside(&dir.join("SLUS-99999 (0000ABCD).01.p2s")), None, "PCSX2's is inside the state");
        assert_eq!(picture_beside(&dir.join("SCUS-99999_1.sav")), None, "and DuckStation's");
        assert_eq!(picture_beside(&dir.join("GALE01.s01")), None, "Dolphin reads none");
    }

    /// A DuckStation state of `version` whose picture is `width` by `height`, stored as `stored`
    /// under `compression`, followed by a little state data.
    fn duckstation_state(version: u32, compression: u32, width: u32, height: u32, stored: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(DUCKSTATION_MAGIC.to_le_bytes());
        b.extend(version.to_le_bytes());
        b.extend([0u8; 128 + 32]);
        let header = b.len() + 12 * 4;
        let data = header + stored.len();
        for field in [0, 0, 0, compression, width, height, stored.len() as u32, header as u32, 0, 4, 4, data as u32] {
            b.extend(field.to_le_bytes());
        }
        b.extend(stored);
        b.extend(b"data");
        b
    }

    #[test]
    fn a_state_goes_up_with_the_picture_its_emulator_took() {
        let dir = std::env::temp_dir().join(format!("deckhand-state-pictures-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // PCSX2's, inside the state.
        let p2s = dir.join("SLUS-20946 (399A49CA).01.p2s");
        let mut zip = zip::ZipWriter::new(fs::File::create(&p2s).unwrap());
        let stored = zip::write::SimpleFileOptions::default();
        zip.start_file("eeMemory.bin", stored).unwrap();
        std::io::Write::write_all(&mut zip, b"memory").unwrap();
        zip.start_file("Screenshot.png", stored).unwrap();
        std::io::Write::write_all(&mut zip, b"pcsx2 png").unwrap();
        zip.finish().unwrap();
        assert_eq!(picture(&p2s), Some((b"pcsx2 png".to_vec(), "png")));

        // PPSSPP's, beside it under the same stem.
        let ppst = dir.join("ULUS10538_2.00_0.ppst");
        fs::write(&ppst, b"state").unwrap();
        fs::write(dir.join("ULUS10538_2.00_0.jpg"), b"ppsspp jpg").unwrap();
        assert_eq!(picture(&ppst), Some((b"ppsspp jpg".to_vec(), "jpg")));

        // RetroArch's, beside it with `.png` appended.
        let state = dir.join("Chrono Trigger (USA).state1");
        fs::write(&state, b"state").unwrap();
        assert_eq!(picture(&state), None, "no thumbnail, no picture");
        fs::write(dir.join("Chrono Trigger (USA).state1.png"), b"retroarch png").unwrap();
        assert_eq!(picture(&state), Some((b"retroarch png".to_vec(), "png")));

        // DuckStation's, raw or compressed in its own header, as a PNG.
        let pixels: Vec<u8> = (0..2 * 3).flat_map(|i| [i as u8 * 40, 200, 10, 0]).collect();
        let deflated = {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            std::io::Write::write_all(&mut z, &pixels).unwrap();
            z.finish().unwrap()
        };
        let zstd = ruzstd::encoding::compress_to_vec(&pixels[..], ruzstd::encoding::CompressionLevel::Fastest);
        for (version, compression, stored) in [(66, 7, &pixels), (88, 0, &pixels), (88, 1, &deflated), (88, 2, &zstd)] {
            let sav = dir.join("SCUS-94423_1.sav");
            fs::write(&sav, duckstation_state(version, compression, 2, 3, stored)).unwrap();
            let (png, ext) = picture(&sav).unwrap_or_else(|| panic!("version {version}, compression {compression}"));
            assert_eq!(ext, "png");
            let shown = image::load_from_memory(&png).unwrap().into_rgba8();
            assert_eq!((shown.width(), shown.height()), (2, 3));
            assert_eq!(shown.get_pixel(1, 0).0, [40, 200, 10, 255], "opaque, whatever its alpha said");
        }
        let xz = dir.join("SCUS-94423_2.sav");
        fs::write(&xz, duckstation_state(88, 3, 2, 3, &pixels)).unwrap();
        assert_eq!(picture(&xz), None, "XZ is not read");
        let short = duckstation_state(88, 0, 2, 3, &pixels);
        assert_eq!(duckstation_picture(&short[..short.len() - b"data".len() - 1]), None, "a cut-off picture");
        assert_eq!(duckstation_picture(&duckstation_state(88, 0, 3, 3, &pixels)), None, "fewer pixels than its size");

        // A state that is no zip, or holds no picture, goes up without one.
        let bare = dir.join("SLUS-20946 (399A49CA).02.p2s");
        fs::write(&bare, b"not a zip").unwrap();
        assert_eq!(picture(&bare), None);
        let _ = fs::remove_dir_all(&dir);
    }
}

//! RetroArch save states, kept in step with RomM for one game — the slot model the Kotlin app and
//! DeckhandTV share, so all three read and write one history.
//!
//! RetroArch keeps slot 0 in `<rom>.state` and slot N in `<rom>.stateN`. Two bands of that sync:
//!
//! - **Slot 0 is the game's current state**, two-way against the newest of RomM's *rolling* states.
//!   RomM keeps no history of states and prunes none, so a push is a new state under a fresh
//!   timestamped name, `<rom> [2026-07-01 10-00-00-000].deckhand.state`, and this app prunes its
//!   own down to [`HISTORY`] afterwards. `.state.auto` — what "save state on close" writes — is the
//!   same slot: whichever of the two is newer is the one that syncs, and a pull overwrites both.
//! - **Slots 1 to [`SLOTS`] are the player's own**, each two-way against one fixed server state,
//!   `<rom>.slotN.deckhand.state`, uploaded under that name every time. Never pruned, never part of
//!   slot 0's history, so slot 2 is the same save on every device.
//!
//! What changed is decided the way saves decide it — by content against the index, see
//! [`super::plan`] — in an index of its own, `state-index.json`, keyed by the slot's path. A
//! conflict keeps the newer side, and a local state that loses is copied into `save-backups/` first.
//!
//! **Only the core's own states take part.** A state is a core's memory, and RetroArch refuses one
//! another core wrote, even for the same console — a game played under SwanStation here and PCSX
//! ReARMed elsewhere has two lines of states on RomM, and each device follows its own core's. Which
//! core wrote a state is read from its `emulator` tag ([`Core::loads`]); one whose tag names no core
//! is offered as it always was.
//!
//! Beyond the two-way bands, RomM's older rolling states are laid into slots [`FIRST_HISTORY`] and
//! up, newest first and download-only, as the Kotlin app does — a state from a few sessions back is
//! one menu away. Only into a slot this app last wrote, though: RetroArch's slot number is one the
//! player can wind as far as they like, so a state they saved there themselves is theirs and is
//! never traded for a copy of something RomM holds. Thumbnails travel both ways: RetroArch's
//! `<state>.png` goes up as the state's screenshot, and RomM's comes down beside the slot. A
//! standalone emulator's state sends the picture it took too ([`picture`]), and takes none back.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::index::Entry;
use super::retroarch::Install;
use super::{Plan, Run, describe, file_name, md5_hex, names, plan, write_atomic};
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

/// The player's own slots, shared with DeckhandTV's slot model.
pub const SLOTS: u32 = 4;

/// How many of its own rolling states this app keeps on RomM for a game.
pub const HISTORY: usize = 10;

/// The first slot the older rolling history is laid into, after the player's own.
pub const FIRST_HISTORY: u32 = SLOTS + 1;

/// Where RetroArch keeps a state file's thumbnail: beside it, `.png` appended.
fn thumbnail(state: &Path) -> PathBuf {
    let mut name = state.file_name().unwrap_or_default().to_os_string();
    name.push(".png");
    state.with_file_name(name)
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

/// Whether `name` is a save state RetroArch keeps for the content `stem`: `<stem>.state` for slot 0,
/// `<stem>.state.auto` for the one "save state on close" writes, `<stem>.stateN` for slot N.
///
/// What tells a changed file in the state folder from another game's — see [`super::changed_states`].
pub(super) fn is_slot_of(name: &str, stem: &str) -> bool {
    let Some(rest) = name.strip_prefix(stem).map(str::to_lowercase) else { return false };
    let Some(slot) = rest.strip_prefix(".state") else { return false };
    slot.is_empty() || slot == ".auto" || slot.chars().all(|c| c.is_ascii_digit())
}

/// The manual slot a server state's name declares: `.slotN` ending its stem, or followed by a dot.
pub fn slot_of(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(OWN).or_else(|| lower.strip_suffix(LEGACY)).unwrap_or(&lower);
    let at = stem.rfind(".slot")? + ".slot".len();
    let digits: String = stem[at..].chars().take_while(char::is_ascii_digit).collect();
    let rest = &stem[at + digits.len()..];
    let slot: u32 = digits.parse().ok()?;
    ((rest.is_empty() || rest.starts_with('.')) && (1..=SLOTS).contains(&slot)).then_some(slot)
}

/// The name a slot-0 push goes up under: unique, and sorting by when it was made.
fn rolling_name(stem: &str, now: SystemTime) -> String {
    let millis = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    // `2026-07-01T10:00:00Z` → `2026-07-01 10-00-00-123`, the Kotlin app's spelling.
    let stamp = names::format_timestamp_no_zone(now).replace('T', " ").replace(':', "-");
    format!("{stem} [{stamp}-{millis:03}]{OWN}")
}

/// The newest of `states`, by RomM's time and then id.
fn newest<'a>(states: impl Iterator<Item = &'a SaveAsset>) -> Option<&'a SaveAsset> {
    states.max_by_key(|s| (s.updated_at.as_deref().and_then(names::parse_timestamp), s.id))
}

/// One slot, as a sync sees it.
struct Slot<'s> {
    /// The file the index is keyed by, and the one a pull writes.
    key: PathBuf,
    /// The file holding the slot's current content, if any.
    live: Option<PathBuf>,
    /// Other files that hold this slot and must follow a pull: `.state.auto` for slot 0.
    mirrors: Vec<PathBuf>,
    upload_as: String,
    remote: Option<&'s SaveAsset>,
    rolling: bool,
}

impl Run<'_> {
    /// Sync the states of the game whose content stem is `stem`, kept in `dir`, run by `core` — the
    /// one whose states are synced, and what this device's are tagged with.
    pub(super) async fn states(&mut self, dir: &Path, stem: &str, rom_id: i64, core: &Core) {
        let emulator = core.tag();
        let (server, foreign): (Vec<SaveAsset>, Vec<SaveAsset>) = match self.client.fetch_states(self.connection, rom_id).await {
            Ok(all) => all
                .into_iter()
                .filter(|s| s.rom_id.is_none_or(|id| id == rom_id) && is_state(&s.file_name))
                .partition(|s| core.loads(s)),
            Err(e) => return self.report.failed.push(format!("{stem}.state: {}", describe(&e))),
        };
        let history: Vec<&SaveAsset> = server.iter().filter(|s| slot_of(&s.file_name).is_none()).collect();

        let state = dir.join(format!("{stem}.state"));
        let auto = dir.join(format!("{stem}.state.auto"));
        let live = [&auto, &state]
            .into_iter()
            .filter(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0))
            .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
            .cloned();

        // Another core's newest state, when it is newer than anything this one has here or on RomM:
        // progress made elsewhere that cannot come here, which the player may be looking for.
        let time = |s: &SaveAsset| s.updated_at.as_deref().and_then(names::parse_timestamp);
        let local_time = live.as_ref().and_then(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        if let Some(other) = newest(foreign.iter().filter(|s| slot_of(&s.file_name).is_none()))
            && let Some(at) = time(other)
            && newest(history.iter().copied()).and_then(time).is_none_or(|t| at > t)
            && local_time.is_none_or(|t| at > t)
        {
            let maker = made_by(other).unwrap_or("another core");
            let here = core.name().unwrap_or("this core");
            self.report.other_cores.push(format!("a newer save state made with {maker} stays on RomM — {here} can't load it"));
        }
        let slot_zero = Slot {
            key: state,
            live,
            mirrors: vec![auto],
            upload_as: rolling_name(stem, SystemTime::now()),
            remote: newest(history.iter().copied()),
            rolling: true,
        };
        self.state(slot_zero, rom_id, emulator, &history).await;

        for n in 1..=SLOTS {
            let path = dir.join(format!("{stem}.state{n}"));
            let live = fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0).then(|| path.clone());
            let slot = Slot {
                key: path,
                live,
                mirrors: Vec::new(),
                upload_as: format!("{stem}.slot{n}{OWN}"),
                remote: newest(server.iter().filter(|s| slot_of(&s.file_name) == Some(n))),
                rolling: false,
            };
            self.state(slot, rom_id, emulator, &history).await;
        }
        self.lay_out_history(dir, stem, rom_id, &history).await;
    }

    /// One state file that is a save of its own — a standalone emulator's slot, uploaded under its
    /// own name every time and never pruned. See [`super::standalone`].
    pub(super) async fn state_file(&mut self, path: PathBuf, remote: Option<&SaveAsset>, rom_id: i64, emulator: &str) {
        let live = fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0).then(|| path.clone());
        let slot = Slot { upload_as: file_name(&path), key: path, live, mirrors: Vec::new(), remote, rolling: false };
        self.state(slot, rom_id, emulator, &[]).await;
    }

    async fn state(&mut self, slot: Slot<'_>, rom_id: i64, emulator: &str, history: &[&SaveAsset]) {
        let name = file_name(&slot.key);
        let Some(live) = slot.live.clone() else {
            // Nothing here: a fresh device takes RomM's. Nothing on either side is not news.
            if let Some(remote) = slot.remote {
                self.pull_state(&slot, rom_id, remote, &name).await;
            }
            return;
        };
        let bytes = match fs::read(&live) {
            Ok(bytes) => bytes,
            Err(e) => return self.report.failed.push(format!("{name}: {e}")),
        };
        let hash = md5_hex(&bytes);
        match (plan(&hash, self.index.agreed(&slot.key, rom_id, self.stamp.as_ref()), slot.remote.map(|r| r.id)), slot.remote) {
            (Plan::InSync, _) => self.report.in_sync += 1,
            (Plan::Push, _) => self.push_state(&slot, rom_id, emulator, bytes, hash, &name, history).await,
            (Plan::Pull, Some(remote)) => self.pull_state(&slot, rom_id, remote, &name).await,
            (Plan::Conflict, Some(remote)) => {
                let Some(theirs) = self.download_state(remote, &name).await else { return };
                if md5_hex(&theirs) == hash {
                    if self.remember_state(&slot.key, rom_id, hash, remote.id, &name) {
                        self.report.in_sync += 1;
                    }
                    return;
                }
                let local_time = fs::metadata(&live).and_then(|m| m.modified()).ok();
                let remote_time = remote.updated_at.as_deref().and_then(names::parse_timestamp);
                if super::choose(&name, local_time, remote_time, bytes.len() as u64).await == super::Choice::Remote {
                    let Some(kept) = self.back_up(&live, &bytes, &name) else { return };
                    if self.write_state(&slot, rom_id, remote, &theirs, &name) {
                        self.report.conflicts.push(format!("{name}: kept RomM's state; this device's is at {}", kept.display()));
                    }
                } else {
                    self.push_state(&slot, rom_id, emulator, bytes, hash, &name, history).await;
                    self.report.conflicts.push(format!("{name}: kept this device's state; RomM keeps the other"));
                }
            }
            (Plan::Pull | Plan::Conflict, None) => unreachable!("a server change with no server state"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_state(
        &mut self,
        slot: &Slot<'_>,
        rom_id: i64,
        emulator: &str,
        bytes: Vec<u8>,
        hash: String,
        name: &str,
        history: &[&SaveAsset],
    ) {
        // RetroArch writes the thumbnail beside whichever file it saved: `.state.png`, `.state.auto.png`.
        let shot = slot.live.as_ref().and_then(|live| picture(live));
        let created = match self.client.upload_state(self.connection, rom_id, emulator, &slot.upload_as, bytes, shot).await {
            Ok(created) => created,
            Err(e) => return self.report.failed.push(format!("{name}: {}", describe(&e))),
        };
        if self.remember_state(&slot.key, rom_id, hash, created.id, name) {
            self.report.pushed.push(name.to_string());
        }
        if slot.rolling {
            self.prune(history, &created).await;
        }
    }

    /// Keep this app's newest [`HISTORY`] rolling states for the game and drop the rest. Only its
    /// own: a state the web player or another app uploaded is not this app's to delete.
    async fn prune(&mut self, history: &[&SaveAsset], created: &SaveAsset) {
        // The one just uploaded is this app's whatever name RomM gave it back.
        let mut own: Vec<&SaveAsset> =
            history.iter().copied().filter(|s| is_own(&s.file_name)).chain(std::iter::once(created)).collect();
        own.sort_by_key(|s| std::cmp::Reverse((s.updated_at.as_deref().and_then(names::parse_timestamp), s.id)));
        own.dedup_by_key(|s| s.id);
        let stale: Vec<i64> = own.iter().skip(HISTORY).map(|s| s.id).filter(|id| *id != created.id).collect();
        if !stale.is_empty()
            && let Err(e) = self.client.delete_states(self.connection, &stale).await
        {
            self.report.notes.push(format!("older states not pruned from RomM: {}", describe(&e)));
        }
    }

    async fn pull_state(&mut self, slot: &Slot<'_>, rom_id: i64, remote: &SaveAsset, name: &str) {
        if let Some(bytes) = self.download_state(remote, name).await
            && self.write_state(slot, rom_id, remote, &bytes, name)
        {
            self.report.pulled.push(name.to_string());
            self.pull_thumbnail(&slot.key, remote).await;
        }
    }

    async fn download_state(&mut self, remote: &SaveAsset, name: &str) -> Option<Vec<u8>> {
        match self.client.download_state(self.connection, remote).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                self.report.failed.push(format!("{name}: {}", describe(&e)));
                None
            }
        }
    }

    /// `remote`'s content into the slot, and into each mirror already on disk — a `.state.auto` left
    /// older than the pulled `.state` would be the newer of the two by its old time, and push back.
    fn write_state(&mut self, slot: &Slot<'_>, rom_id: i64, remote: &SaveAsset, bytes: &[u8], name: &str) -> bool {
        let time = remote.updated_at.as_deref().and_then(names::parse_timestamp);
        let targets: Vec<PathBuf> =
            std::iter::once(&slot.key).chain(slot.mirrors.iter().filter(|m| m.is_file())).cloned().collect();
        for path in &targets {
            if let Err(e) = write_atomic(path, bytes) {
                self.report.failed.push(format!("{name}: {e}"));
                return false;
            }
            if let Some(time) = time
                && let Ok(file) = fs::File::options().write(true).open(path)
            {
                let _ = file.set_modified(time);
            }
        }
        self.report.written.extend(targets);
        self.remember_state(&slot.key, rom_id, md5_hex(bytes), remote.id, name)
    }

    /// The state's thumbnail from RomM, beside the slot file, where RetroArch's menu shows it. A
    /// thumbnail is not progress: one that fails is skipped without a word.
    async fn pull_thumbnail(&mut self, key: &Path, remote: &SaveAsset) {
        // A standalone emulator reads no `<state>.png`: there it would only be a stray file.
        if !file_name(key).contains(".state") {
            return;
        }
        let Some(path) = remote.screenshot.as_ref().and_then(|s| s.download_path.as_deref()).filter(|p| !p.trim().is_empty()) else {
            return;
        };
        let host = self.connection.host.trim_end_matches('/');
        let url = if path.starts_with("http") { path.to_string() } else { format!("{host}/{}", path.trim_start_matches('/')) };
        if let Ok(bytes) = self.client.fetch_asset(self.connection, &url).await {
            let shot = thumbnail(key);
            if write_atomic(&shot, &bytes).is_ok() {
                // The watcher sees the state folder, and `<state>.png` holds `.state` in its name:
                // left unsaid, a thumbnail this sync fetched would read back as a state to sync.
                self.report.written.push(shot);
            }
        }
    }

    /// RomM's older rolling states, laid into slots [`FIRST_HISTORY`] and up, newest first — so a
    /// state from a few sessions back is one menu away, as in the Kotlin app. Download-only: they are
    /// RomM's history, not the player's slots. One already holding that state is left as it is.
    ///
    /// And so is one holding anything else this app did not put there. RetroArch's slot is a number
    /// the player can wind up as far as they like from its own menu, so a `.state7` may be a state
    /// they saved themselves; it is never traded for a copy of RomM's history, which is on RomM.
    async fn lay_out_history(&mut self, dir: &Path, stem: &str, rom_id: i64, history: &[&SaveAsset]) {
        let mut newest_first: Vec<&SaveAsset> = history.to_vec();
        newest_first.sort_by_key(|s| std::cmp::Reverse((s.updated_at.as_deref().and_then(names::parse_timestamp), s.id)));
        let mut theirs: Vec<String> = Vec::new();
        // The newest is slot 0's; the next ones go after the player's own slots.
        for (rank, remote) in newest_first.iter().skip(1).take(HISTORY - 1).enumerate() {
            let key = dir.join(format!("{stem}.state{}", FIRST_HISTORY + rank as u32));
            let here = self.index.agreed(&key, rom_id, self.stamp.as_ref()).is_some_and(|e| e.save_id == remote.id) && key.is_file();
            if here {
                continue;
            }
            if !self.laid_this_down(&key) {
                theirs.push(file_name(&key));
                continue;
            }
            let name = file_name(&key);
            let Some(bytes) = self.download_state(remote, &name).await else { continue };
            let slot = Slot { key: key.clone(), live: None, mirrors: Vec::new(), upload_as: String::new(), remote: None, rolling: false };
            if self.write_state(&slot, rom_id, remote, &bytes, &name) {
                self.pull_thumbnail(&key, remote).await;
            }
        }
        if !theirs.is_empty() {
            self.report.notes.push(format!(
                "RomM's older states were kept out of {} — this device has a state of its own there",
                theirs.join(", ")
            ));
        }
    }

    /// Whether a history slot is this app's to write: empty, or still holding the very bytes this
    /// app last laid in it. Anything else is the player's own state, saved into that slot since.
    fn laid_this_down(&self, key: &Path) -> bool {
        match fs::read(key) {
            // Nothing there (or nothing readable, which a write would fail on anyway).
            Err(_) => true,
            Ok(bytes) => self.index.get(key).is_some_and(|e| e.hash == md5_hex(&bytes)),
        }
    }

    fn remember_state(&mut self, key: &Path, rom_id: i64, hash: String, save_id: i64, name: &str) -> bool {
        match self.index.set(key, Entry { rom_id, hash, save_id, stamp: self.stamp.clone(), ..Default::default() }) {
            Ok(()) => true,
            Err(e) => {
                self.report.failed.push(format!("{name}: synced, but could not record it ({e})"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Connection;
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
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn state(id: i64, name: &str, iso: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "rom_id": 1, "file_name": name, "emulator": "snes9x", "updated_at": iso,
            "download_path": format!("/api/raw/assets/states/{id}.state"),
        })
    }

    #[tokio::test]
    async fn a_fresh_device_gets_the_newest_state_in_slot_zero_and_each_slot_in_its_own() {
        let device = Device::new("fresh");
        let states = serde_json::json!([
            state(5, &format!("{CHRONO} [2026-06-01 10-00-00-000].deckhand.state"), "2026-06-01T10:00:00Z"),
            state(6, &format!("{CHRONO} [2026-07-01 10-00-00-000].deckhand.state"), "2026-07-01T10:00:00Z"),
            state(7, &format!("{CHRONO}.slot2.deckhand.state"), "2026-07-02T10:00:00Z"),
        ]);
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("GET /api/raw/assets/states/6.state", 200, "newest rolling".into()),
            ("GET /api/raw/assets/states/7.state", 200, "slot two".into()),
            ("GET /api/raw/assets/states/5.state", 200, "older rolling".into()),
            ("GET /api/states?rom_id=1", 200, states.to_string()),
        ])
        .await;
        let report = device.sync(host.clone()).await;
        assert_eq!(report.pulled, [format!("{CHRONO}.state"), format!("{CHRONO}.state2")], "{report:?}");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "newest rolling");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state2"))).unwrap(), "slot two");
        assert_eq!(
            fs::read_to_string(device.state(&format!("{CHRONO}.state5"))).unwrap(),
            "older rolling",
            "the history after the newest goes after the player's slots"
        );
        assert!(report.failed.is_empty(), "{report:?}");

        let again = device.sync(host).await;
        assert!(again.pulled.is_empty() && again.pushed.is_empty(), "{again:?}");
        assert_eq!(again.in_sync, 2);
    }

    #[tokio::test]
    async fn new_states_go_up_under_the_shared_names_and_old_rolling_ones_are_pruned() {
        let device = Device::new("push");
        fs::create_dir_all(device.state("")).unwrap();
        fs::write(device.state(&format!("{CHRONO}.state.auto")), "played on close").unwrap();
        fs::write(device.state(&format!("{CHRONO}.state3")), "my slot").unwrap();
        // Ten of this app's rolling states already, the oldest first, and one the web player made.
        let mut existing: Vec<serde_json::Value> = (1..=10)
            .map(|i| state(i, &format!("{CHRONO} [2026-01-{i:02} 10-00-00-000].deckhand.state"), &format!("2026-01-{i:02}T10:00:00Z")))
            .collect();
        existing.push(state(50, &format!("{CHRONO}.state"), "2025-01-01T00:00:00Z"));
        let created = state(99, "created", "2026-08-01T10:00:00Z");
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("GET /api/raw/assets/states/", 200, "another device's".into()),
            ("POST /api/states/delete", 200, "{}".into()),
            ("POST /api/states", 201, created.to_string()),
            ("GET /api/states?rom_id=1", 200, serde_json::Value::Array(existing).to_string()),
        ])
        .await;
        // Both sides changed for slot 0 (RomM has states this device never agreed on): the local
        // one is newer, so it goes up and RomM keeps the rest.
        let report = device.sync(host).await;
        assert_eq!(report.pushed, [format!("{CHRONO}.state"), format!("{CHRONO}.state3")], "{report:?}");
        assert_eq!(report.conflicts.len(), 1, "{report:?}");

        let lines = requests.lock().unwrap().clone();
        let uploads: Vec<&String> = lines.iter().filter(|l| l.starts_with("POST /api/states?")).collect();
        assert_eq!(uploads.len(), 2);
        assert!(uploads.iter().all(|l| l.contains("emulator=snes9x")), "tagged with the core: {uploads:?}");
        assert!(lines.iter().any(|l| l.starts_with("POST /api/states/delete")), "the oldest own state goes: {lines:?}");
    }

    #[tokio::test]
    async fn another_cores_states_stay_on_romm_and_its_newer_one_is_mentioned() {
        let device = Device::new("other-core");
        let by = |id: i64, name: String, iso: &str, core: &str| {
            let mut s = state(id, &name, iso);
            s["emulator"] = core.into();
            s
        };
        let states = serde_json::json!([
            state(5, &format!("{CHRONO} [2026-06-01 10-00-00-000].deckhand.state"), "2026-06-01T10:00:00Z"),
            by(6, format!("{CHRONO} [2026-07-01 10-00-00-000].deckhand.state"), "2026-07-01T10:00:00Z", "bsnes"),
            by(7, format!("{CHRONO}.slot2.deckhand.state"), "2026-07-02T10:00:00Z", "bsnes"),
        ]);
        let (host, requests) = stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("GET /api/raw/assets/states/5.state", 200, "snes9x's".into()),
            ("GET /api/raw/assets/states/", 200, "bsnes's".into()),
            ("GET /api/states?rom_id=1", 200, states.to_string()),
        ])
        .await;
        let report = device.sync(host).await;

        assert_eq!(report.pulled, [format!("{CHRONO}.state")], "{report:?}");
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "snes9x's", "this core's newest");
        assert!(!device.state(&format!("{CHRONO}.state2")).exists(), "bsnes's slot is not snes9x's");
        assert!(!device.state(&format!("{CHRONO}.state5")).exists(), "nor is its history");
        let lines = requests.lock().unwrap().clone();
        assert!(!lines.iter().any(|l| l.contains("/states/6.state") || l.contains("/states/7.state")), "never fetched: {lines:?}");
        assert_eq!(report.other_cores.len(), 1, "{report:?}");
        assert!(report.other_cores[0].contains("bsnes"), "{report:?}");
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
    fn a_slot_is_read_from_its_name_and_only_within_the_shared_range() {
        assert_eq!(slot_of("Chrono Trigger (USA).slot2.deckhand.state"), Some(2));
        assert_eq!(slot_of("Game.slot4.state"), Some(4));
        assert_eq!(slot_of("Game.slot7.deckhand.state"), None, "past the family's four");
        assert_eq!(slot_of("Game [2026-07-01 10-00-00-000].deckhand.state"), None);
        assert_eq!(slot_of("Game.slotmachine.state"), None);
    }

    #[tokio::test]
    async fn a_state_the_player_saved_in_a_history_slot_is_not_traded_for_romms() {
        let device = Device::new("history-slot");
        fs::create_dir_all(device.state("")).unwrap();
        // RetroArch's slot is a number the player can wind as far as they like: slot 5 is theirs.
        fs::write(device.state(&format!("{CHRONO}.state5")), "my own slot five").unwrap();
        let states = serde_json::json!([
            state(6, &format!("{CHRONO} [2026-07-01 10-00-00-000].deckhand.state"), "2026-07-01T10:00:00Z"),
            state(5, &format!("{CHRONO} [2026-06-01 10-00-00-000].deckhand.state"), "2026-06-01T10:00:00Z"),
        ]);
        let host = stub_server::start(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("GET /api/raw/assets/states/6.state", 200, "newest rolling".into()),
            ("GET /api/raw/assets/states/5.state", 200, "older rolling".into()),
            ("GET /api/states?rom_id=1", 200, states.to_string()),
        ])
        .await;
        let report = device.sync(host).await;

        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state5"))).unwrap(), "my own slot five");
        assert!(report.notes.iter().any(|n| n.contains(".state5")), "and it is said: {report:?}");
        // Slot 0 is still RomM's newest — the player's own slots are the only ones held back.
        assert_eq!(fs::read_to_string(device.state(&format!("{CHRONO}.state"))).unwrap(), "newest rolling");
    }

    #[test]
    fn a_state_file_is_recognised_as_its_games_slot() {
        assert!(is_slot_of("Chrono Trigger (USA).state", "Chrono Trigger (USA)"));
        assert!(is_slot_of("Chrono Trigger (USA).state.auto", "Chrono Trigger (USA)"));
        assert!(is_slot_of("Chrono Trigger (USA).state7", "Chrono Trigger (USA)"));
        assert!(!is_slot_of("Chrono Trigger (USA).state.png", "Chrono Trigger (USA)"), "a thumbnail is not a state");
        assert!(!is_slot_of("Chrono Trigger (USA).srm", "Chrono Trigger (USA)"));
        assert!(!is_slot_of("Secret of Mana (USA).state", "Chrono Trigger (USA)"));
    }

    #[test]
    fn a_rolling_name_is_the_kotlin_apps_spelling() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_782_900_000_123);
        assert_eq!(rolling_name("Chrono", t), "Chrono [2026-07-01 10-00-00-123].deckhand.state");
        assert!(is_own(&rolling_name("x", t)) && is_state(&rolling_name("x", t)));
        assert!(!is_state("Game.p2s"));
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

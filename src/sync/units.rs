//! Each game's saves as one unit, read and written through argosy-sigil — see [`crate::sigil`].
//!
//! A game downloaded here has its saves found by sigil, whatever runs it: a RetroArch core's `.srm`
//! or memory card files, a PCSX2 memory card's folders for it, a PPSSPP game's save folders, a
//! Dolphin GCI folder's files with its id. They travel to RomM as the unit sigil packs — one file as
//! it is, or a zip — in the game's `autosave` slot, the shape RomM's Android app syncs; and a unit
//! from RomM goes back through sigil, which writes only this game's files, into a memory card every
//! other game shares as well.
//!
//! A [`Unit`] is a place for [`super::session`]'s engine, beside a single file and the sets of the
//! engine before: what changed, what moves and what a conflict keeps are decided there, the same for
//! every place.
//!
//! **The shapes before sigil.** This app and the Kotlin app put a standalone emulator's saves on
//! RomM one zip per save folder, the folder's files at the zip's top; a RetroArch core's save went up
//! as its `.srm`. Where a game's unit is something else, its first sync through sigil settles what
//! RomM holds in the old shape first, with the engine that wrote it ([`Unit::legacy`]), and the unit
//! goes up after. A unit is never put up under a name the old shape gave a save of another shape: a
//! PSP game's unit is `ULUS10538.zip`, the name that game's first save folder had as a zip of its
//! own, and an app on the old shape would unpack the one into the other's place.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::names;
use super::retroarch::Install;
use super::session::{self, Place, Thread};
use super::standalone;
use crate::library::Stamp;
use crate::sigil::{self, Shape};

/// A game, as much of it as finding its saves takes.
#[derive(Debug, Clone, Copy)]
pub struct Game<'a> {
    pub rom_id: i64,
    /// The file a launch hands the emulator.
    pub rom: &'a Path,
    /// ES-DE's system: `psx`, `gc`.
    pub system: &'a str,
    pub title: &'a str,
    pub stamp: Option<&'a Stamp>,
}

impl<'a> Game<'a> {
    /// A downloaded game, as a launch works it out, held to `stamp`.
    pub fn of(plan: &'a crate::launch::Plan, stamp: Option<&'a Stamp>) -> Self {
        Game { rom_id: plan.rom_id, rom: &plan.rom, system: &plan.system, title: &plan.title, stamp }
    }
}

/// What runs a game.
#[derive(Debug, Clone, Copy)]
pub enum Runner<'a> {
    /// RetroArch, with this core when it is known.
    RetroArch(Option<&'a str>),
    Standalone(standalone::Emulator),
}

/// One game's saves, as sigil keeps them.
#[derive(Debug, Clone)]
pub struct Unit {
    pub request: sigil::Request,
    /// The name sigil gives the unit: `Chrono Trigger (USA).srm`, `ULUS10538.zip`.
    pub artifact: String,
    /// The name it goes up under: the artifact, unless that is a name the shape before sigil gave a
    /// save of another shape — see the module's notes.
    pub name: String,
    pub shape: Shape,
    /// The game's files that are here, and where the ones not here yet would go.
    members: Vec<PathBuf>,
    expected: Vec<PathBuf>,
    /// Files every game shares that this game's saves are on: a memory card, a backup RAM volume.
    shared: Vec<PathBuf>,
    /// The extensions a file of RomM's may have and still be this save, from any client: a PS1 card
    /// is an `.srm` to one and an `.mcd` to another, and sigil reads either.
    kinds: Vec<String>,
    /// The names this game's saves had on RomM in a shape before sigil that sigil does not read: a
    /// standalone emulator's save folders zipped one by one.
    legacy_names: Vec<String>,
    /// The engine before sigil's threads for the same saves, settled before the game's first sync
    /// through sigil — see [`super::session::sync`]. Empty when the shape did not change.
    pub legacy: Vec<Thread>,
    /// For a standalone emulator's game: how to find those threads again once RomM has said which
    /// sets it holds, so a set only RomM has comes down in the old shape too.
    pub legacy_source: Option<(standalone::Emulator, PathBuf, String)>,
}

impl Unit {
    /// One file, of its own: what the save is keyed on, and the same index entry the engine before
    /// sigil kept for a core's `.srm`.
    pub fn file(&self) -> Option<&Path> {
        (self.shape == Shape::Single || (self.shape == Shape::None && self.expected.len() == 1 && self.members.is_empty()))
            .then(|| self.members.first().or(self.expected.first()).map(PathBuf::as_path))
            .flatten()
            .filter(|_| self.shared.is_empty())
    }

    /// What the index keys this save on: its one file, else a name no file has under its save root.
    pub fn key(&self) -> PathBuf {
        match self.file() {
            Some(file) => file.to_path_buf(),
            None => self.request.root.join(".deckhand-sigil").join(&self.artifact),
        }
    }

    /// Whether `server_name`, one of RomM's files of this game's slot, is a version of this save.
    pub fn holds(&self, server_name: &str) -> bool {
        let name = names::canonical(server_name);
        if name.eq_ignore_ascii_case(&self.name) || name.eq_ignore_ascii_case(&self.artifact) {
            return true;
        }
        if self.legacy_names.iter().any(|l| l.eq_ignore_ascii_case(&name)) {
            return false;
        }
        extension(&name).is_some_and(|ext| self.kinds.contains(&ext))
    }

    /// Every file this save is held in, as far as is known.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.members.iter().chain(&self.shared).cloned().collect()
    }

    /// Whether any of `changed` is this save's: one of its files, a card it is on, or anything in
    /// one of its save folders.
    pub fn covers(&self, changed: &[PathBuf]) -> bool {
        let folders: Vec<PathBuf> = match self.shape {
            Shape::Folder => self.members.iter().chain(&self.expected).filter_map(|p| p.parent().map(Path::to_path_buf)).collect(),
            _ => Vec::new(),
        };
        let watched: Vec<&PathBuf> = self.members.iter().chain(&self.expected).chain(&self.shared).chain(&folders).collect();
        changed.iter().any(|p| watched.iter().any(|w| p == *w || p.starts_with(w)))
    }

    /// What is here: RomM's hash of the unit, its size, and when its files last changed. None when
    /// the game has no saves here.
    pub fn held(&self) -> Result<Option<(String, u64, Option<SystemTime>)>, String> {
        let unit = self.request.collect(None).map_err(|e| e.to_string())?;
        let Some(data) = unit.data else { return Ok(None) };
        let files = if self.members.is_empty() { &self.shared } else { &self.members };
        let modified = files.iter().filter_map(|p| fs::metadata(p).and_then(|m| m.modified()).ok()).max();
        Ok(Some((unit.content_hash, data.len() as u64, modified)))
    }

    /// RomM's hash of the save's one file as it is on disk, for a unit that is one — see
    /// `session::Held::raw`. None for any other.
    pub fn raw(&self) -> Option<String> {
        fs::read(self.file()?).ok().map(|bytes| super::hash::file(&bytes))
    }

    /// The unit as it goes up, and RomM's hash of it.
    pub fn body(&self) -> Result<(Vec<u8>, String), String> {
        let unit = self.request.collect(None).map_err(|e| e.to_string())?;
        let data = unit.data.ok_or("there is nothing here to send")?;
        Ok((data, unit.content_hash))
    }

    /// Put `bytes` — RomM's `server_name`, made at `made` — in place of the game's saves here,
    /// answering RomM's hash of what is here then and every file written or taken away.
    pub fn put(&self, bytes: &[u8], made: Option<SystemTime>, server_name: &str) -> Result<(String, Vec<PathBuf>), String> {
        let unit = self.adapt(bytes, server_name);
        let restored = self.request.restore(&unit, None).map_err(|e| e.to_string())?;
        // A file of its own takes RomM's time rather than now, as a battery save always has: a later
        // conflict weighs when the progress was made, not when it was copied here.
        if let (Some(made), Some(file)) = (made, self.file()) {
            let _ = fs::File::options().write(true).open(file).and_then(|f| f.set_modified(made));
        }
        let now = self.request.collect(None).map_err(|e| e.to_string())?;
        let mut touched = restored.written;
        touched.extend(restored.removed);
        Ok((now.content_hash, touched))
    }

    /// RomM's bytes as sigil reads them. A PSP, PS3 or Vita game's save folder uploaded in the shape
    /// before sigil, by this app or the Kotlin app, is the folder's files at the zip's top, under the
    /// folder's name — the name the game's unit can have too. Put under that folder, it is the unit
    /// sigil reads. Any other zip is sigil's already: an N64 game's save types sit at its top.
    fn adapt(&self, bytes: &[u8], server_name: &str) -> Vec<u8> {
        if !matches!(self.request.platform.as_deref(), Some("psp" | "ps3" | "psvita")) {
            return bytes.to_vec();
        }
        let Some(entries) = super::hash::zip_entries(bytes) else { return bytes.to_vec() };
        if entries.is_empty() || entries.iter().any(|(name, _)| name.contains('/')) {
            return bytes.to_vec();
        }
        let canonical = names::canonical(server_name);
        let Some(folder) = canonical.strip_suffix(".zip").or_else(|| canonical.strip_suffix(".ZIP")) else { return bytes.to_vec() };
        let rooted: Vec<(String, Vec<u8>)> = entries.into_iter().map(|(name, data)| (format!("{folder}/{name}"), data)).collect();
        standalone::pack(&rooted).unwrap_or_else(|_| bytes.to_vec())
    }

    /// The unit as one blob, for `save-backups/`.
    pub fn snapshot(&self) -> Result<Vec<u8>, String> {
        self.body().map(|(data, _)| data)
    }

    /// Take the game's saves away. Not where they share a memory card with other games' — those are
    /// left, and their next change sends them up again.
    pub fn clear(&self) -> Result<(), String> {
        if !self.shared.is_empty() {
            return Err("its saves are on a memory card other games share, so they were left there".into());
        }
        self.members.iter().try_for_each(|p| match fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        })
    }
}

/// The game's saves as a thread of its `autosave` slot, kept through sigil.
///
/// An error says why sigil cannot take them — no core known where RetroArch sorts saves by core, a
/// disc whose id cannot be read — and the caller keeps the engine before sigil for the game.
pub fn thread(game: Game<'_>, runner: Runner<'_>, install: Result<&Install, &str>) -> Result<Thread, String> {
    let (request, tag, old) = match runner {
        Runner::RetroArch(core) => retroarch(game, core, install?)?,
        Runner::Standalone(emulator) => {
            let request = standalone::sigil_request(emulator, game.rom)?;
            let old = standalone::threads(emulator, game.rom, game.title, game.rom_id, game.stamp).unwrap_or_default();
            (request, emulator.tag().to_string(), old)
        }
    };
    let source = match runner {
        Runner::Standalone(emulator) => Some((emulator, game.rom.to_path_buf(), game.title.to_string())),
        Runner::RetroArch(_) => None,
    };
    assemble(game, request, tag, old, source)
}

/// The thread for `request`: what sigil resolves it to, named so it never takes a name of `old` —
/// the engine before sigil's threads for the same game — that held another shape.
fn assemble(
    game: Game<'_>,
    request: sigil::Request,
    tag: String,
    old: Vec<Thread>,
    legacy_source: Option<(standalone::Emulator, PathBuf, String)>,
) -> Result<Thread, String> {
    let mut resolved = request.resolve().map_err(|e| format!("sigil could not find its saves: {e}"))?;
    // The name is what collecting packs, which is not always what resolving names: an N64 game's
    // `.srm` resolves as itself and goes up as a zip of its save types — under its own name, never
    // as a zip called `.srm` that another client would load as the save.
    if !resolved.members.is_empty() || !resolved.shared.is_empty() {
        let packed = request.collect(None).map_err(|e| format!("sigil could not read its saves: {e}"))?;
        if !packed.artifact.is_empty() {
            resolved.artifact = packed.artifact;
        }
    }
    if resolved.artifact.is_empty() {
        resolved.artifact = unnamed(&resolved, request.platform.as_deref()).ok_or("it has no save here yet for sigil to name")?;
    }
    let members: Vec<PathBuf> = resolved.members.iter().map(|m| m.path.clone()).collect();
    let expected: Vec<PathBuf> = resolved.expected.iter().map(|m| m.path.clone()).collect();
    // A core's `.srm` that sigil keeps as it is, where it is, is the same save under the same name:
    // nothing to settle first, and the index entry it had is its own still.
    let same = |t: &Thread| {
        matches!(&t.place, Place::File(path) if members.iter().chain(&expected).any(|m| m == path))
            && t.name.eq_ignore_ascii_case(&resolved.artifact)
            && resolved.shape != Shape::Multi
    };
    let legacy: Vec<Thread> = old.into_iter().filter(|t| !same(t)).collect();
    // The standalone emulators' zips from before are a shape sigil does not read, so none of them is
    // ever a version of this save. A core's `.srm` from before it does read — a libretro `.srm` is
    // what RomM's web player keeps too — and it stays one.
    let legacy_names: Vec<String> = legacy.iter().filter(|t| matches!(t.place, Place::Set(_))).map(|t| t.name.clone()).collect();
    let name = if legacy_names.iter().any(|n| n.eq_ignore_ascii_case(&resolved.artifact)) {
        format!("sigil {}", resolved.artifact)
    } else {
        resolved.artifact.clone()
    };
    let kinds = kinds(request.platform.as_deref(), &resolved.artifact);
    let shared_slot = resolved.shape != Shape::Single;
    let unit = Unit {
        request,
        artifact: resolved.artifact,
        name: name.clone(),
        shape: resolved.shape,
        members,
        expected,
        shared: resolved.shared,
        kinds,
        legacy_names,
        legacy,
        legacy_source,
    };
    Ok(Thread {
        rom_id: game.rom_id,
        stamp: game.stamp.cloned(),
        slot: session::AUTOSAVE.into(),
        name,
        emulator: tag,
        place: Place::Sigil(Box::new(unit)),
        // A zip of a game's save folders shares its slot with the ones before it and other clients'
        // files, which RomM's history-trimming would count against it.
        shared: shared_slot,
    })
}

/// A RetroArch game's request: the core's layout, the folder its save is in — or for the cores that
/// keep theirs in the system folder, that — and the core options its files turn on.
fn retroarch(game: Game<'_>, core: Option<&str>, install: &Install) -> Result<(sigil::Request, String, Vec<Thread>), String> {
    let install = install.for_game(game.rom, core);
    let saves = install.game_saves(game.rom, core);
    let save = super::launched(&saves).ok_or("RetroArch files saves by core, and which core runs this game is not known")?;
    // Without a core named, the one whose folder the save was found in.
    let core = core.map(str::to_string).or_else(|| (save.emulator != "retroarch").then(|| save.emulator.clone()));
    let layout = core.as_deref().map(layout_of_core).unwrap_or_else(|| "libretro".into());
    let system = install.system_dir();
    let root = match (layout.as_str(), &system) {
        ("pcsx2", Some(system)) => system.join("pcsx2/memcards"),
        ("flycast", Some(system)) => system.join("dc"),
        _ => save.path.parent().map(Path::to_path_buf).ok_or("its save has no folder")?,
    };
    let platform = sigil::platform_of(&crate::launch::es_system(game.system)).map(str::to_string);
    let core_options = core.as_deref().map(|c| install.core_options(game.rom, c)).unwrap_or_default();
    let options = sigil::layout(&layout)
        .map(|row| row.options)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|o| core_options.get(&o.key).map(|v| (o.key.clone(), v.clone())))
        .collect();
    let discs = standalone::discs(game.rom);
    let ids: Vec<sigil::Identity> = discs.iter().filter_map(|d| sigil::identify(d, platform.as_deref()).ok()).collect();
    let request = sigil::Request {
        layout,
        platform,
        content: game.rom.to_path_buf(),
        identity: ids.first().copied(),
        options,
        root,
        profile: None,
        game_ids: ids.iter().map(sigil::Identity::title_id).filter(|id| !id.is_empty()).collect(),
    };
    let old = super::retroarch_thread(game.rom_id, game.stamp.cloned(), save);
    Ok((request, save.emulator.clone(), vec![old]))
}

/// The name a unit with nothing here yet goes by, by sigil's rule for naming one: the file it
/// expects, as it is or the first of several zipped, else `<save id>.zip` for a game of save folders
/// and `<stem>.ps2` for a PS2 game's card. None where the rule needs a file that is not here — a
/// GameCube save's, which its own name says — and the game keeps the engine before sigil until it
/// has one.
fn unnamed(resolved: &sigil::Resolved, platform: Option<&str>) -> Option<String> {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
    match resolved.expected.as_slice() {
        [one] => return name(&one.path),
        [first, ..] => return name(&first.path).map(|n| format!("{n}.zip")),
        [] => {}
    }
    let key = resolved.key.as_str();
    if key.is_empty() || key.contains('/') {
        return None;
    }
    match platform? {
        "psp" | "ps3" | "psvita" | "wii" | "wiiu" | "switch" => Some(format!("{key}.zip")),
        "ps2" => Some(format!("{key}.ps2")),
        _ => None,
    }
}

/// sigil's layout for a core: its id, with the builds of one core that share a row folded into it —
/// RetroDECK's `mupen64plus_next_gles3` is `mupen64plus_next`.
fn layout_of_core(core: &str) -> String {
    let id = core.trim_end_matches("_libretro");
    if id.starts_with("mupen64plus_next") { "mupen64plus_next".into() } else { id.to_string() }
}

/// What else a file of RomM's may be called and be this save — see [`Unit::kinds`]. A zip is a
/// folder unit, which only its name says is this game's, so it never counts by extension.
fn kinds(platform: Option<&str>, artifact: &str) -> Vec<String> {
    let more: &[&str] = match platform {
        Some("psx") => &["srm", "mcd", "mcr"],
        Some("n64") => &["srm", "eep", "sra", "fla", "mpk"],
        Some("gb" | "gbc") => &["srm", "sav", "rtc"],
        Some("saturn" | "segacd") => &["srm", "bkr", "bcr", "brm", "smpc", "ram"],
        Some("dreamcast") => &["bin"],
        _ => &[],
    };
    let mut kinds: Vec<String> = extension(artifact).filter(|e| e != "zip").into_iter().chain(more.iter().map(|s| s.to_string())).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deckhand-units-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn unit(request: sigil::Request, legacy_names: Vec<String>) -> Unit {
        let resolved = request.resolve().unwrap();
        let name = if legacy_names.iter().any(|n| n.eq_ignore_ascii_case(&resolved.artifact)) {
            format!("sigil {}", resolved.artifact)
        } else {
            resolved.artifact.clone()
        };
        Unit {
            kinds: kinds(request.platform.as_deref(), &resolved.artifact),
            request,
            artifact: resolved.artifact,
            name,
            shape: resolved.shape,
            members: resolved.members.iter().map(|m| m.path.clone()).collect(),
            expected: resolved.expected.iter().map(|m| m.path.clone()).collect(),
            shared: resolved.shared,
            legacy_names,
            legacy: Vec::new(),
            legacy_source: None,
        }
    }

    fn psp(root: &Path) -> sigil::Request {
        sigil::Request {
            layout: "ppsspp_standalone".into(),
            platform: Some("psp".into()),
            content: PathBuf::from("/roms/psp/DJ Max - Portable 3.chd"),
            identity: Some(sigil::Identity::made_up("psp", "ULUS10538", "ULUS10538", 1)),
            options: Vec::new(),
            root: root.to_path_buf(),
            profile: None,
            game_ids: vec!["ULUS10538".into()],
        }
    }

    #[test]
    fn a_ps1_card_is_this_save_whatever_a_client_called_it_but_an_old_shape_never_is() {
        let root = temp("kinds");
        fs::write(root.join("Ape Escape (USA).srm"), vec![0u8; 128]).unwrap();
        let request = sigil::Request {
            layout: "swanstation".into(),
            platform: Some("psx".into()),
            content: PathBuf::from("/roms/psx/Ape Escape (USA).chd"),
            identity: None,
            options: Vec::new(),
            root: root.clone(),
            profile: None,
            game_ids: Vec::new(),
        };
        let card = unit(request, vec!["Ape Escape (USA).old".into()]);
        assert_eq!(card.name, "Ape Escape (USA).srm");
        assert!(card.holds("Ape Escape (USA) [2026-08-04_20-10-10].srm"));
        assert!(card.holds("Ape Escape (USA)_1 [2026-08-04_20-10-10].mcd"), "another client's card");
        assert!(!card.holds("Ape Escape (USA).zip"));
        assert_eq!(card.file(), Some(root.join("Ape Escape (USA).srm").as_path()), "keyed on its file, as before sigil");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_unit_never_goes_up_under_a_name_the_old_shape_gave_another_and_reads_that_shape_back() {
        let root = temp("collide");
        fs::create_dir_all(root.join("PSP/SAVEDATA/ULUS10538")).unwrap();
        fs::write(root.join("PSP/SAVEDATA/ULUS10538/SD.BIN"), b"dj max, here").unwrap();
        let folders = unit(psp(&root), vec!["ULUS10538.zip".into()]);
        assert_eq!(folders.artifact, "ULUS10538.zip");
        assert_eq!(folders.name, "sigil ULUS10538.zip", "the name the folder's own zip had before sigil is not taken");
        assert!(folders.holds("sigil ULUS10538 [2026-10-14_10-00-00].zip"));
        assert!(folders.holds("ULUS10538 [2026-08-06_11-11-06].zip"), "the old shape, and Argosy's: read and adapted");
        assert!(!folders.holds("ULUS10538DATA00.zip"), "another folder's zip from before");

        // The Kotlin app's zip of the folder: its files at the top.
        let old = standalone::pack(&[("SD.BIN".to_string(), b"dj max, from the tv".to_vec())]).unwrap();
        let (hash, written) = folders.put(&old, None, "ULUS10538 [2026-08-06_11-11-06].zip").unwrap();
        assert_eq!(fs::read(root.join("PSP/SAVEDATA/ULUS10538/SD.BIN")).unwrap(), b"dj max, from the tv");
        assert!(!root.join("PSP/SAVEDATA/ULUS10538/ULUS10538").exists(), "not a folder inside the folder");
        assert!(written.contains(&root.join("PSP/SAVEDATA/ULUS10538/SD.BIN")));
        assert_eq!(hash, folders.held().unwrap().unwrap().0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_unit_goes_up_and_comes_back_whole() {
        let root = temp("round-trip");
        fs::create_dir_all(root.join("PSP/SAVEDATA/ULUS10538")).unwrap();
        fs::create_dir_all(root.join("PSP/SAVEDATA/ULUS10538DATA00")).unwrap();
        fs::write(root.join("PSP/SAVEDATA/ULUS10538/SD.BIN"), b"one").unwrap();
        fs::write(root.join("PSP/SAVEDATA/ULUS10538DATA00/DATA.BIN"), b"two").unwrap();
        let here = unit(psp(&root), Vec::new());
        let (body, hash) = here.body().unwrap();
        assert_eq!(hash, super::super::hash::of_upload(&body));
        assert!(here.covers(&[root.join("PSP/SAVEDATA/ULUS10538DATA00/DATA.BIN")]));
        assert!(!here.covers(&[root.join("PSP/SAVEDATA/UCUS98711/DATA.BIN")]));

        let there = temp("round-trip-there");
        fs::create_dir_all(there.join("PSP/SAVEDATA")).unwrap();
        let fresh = unit(psp(&there), Vec::new());
        let (now, _) = fresh.put(&body, None, "ULUS10538.zip").unwrap();
        assert_eq!(now, hash, "the same saves, the same hash");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&there);
    }

    // ---- Moving a game onto sigil, against a RomM that keeps state --------------------------------

    use crate::config::Connection;
    use crate::fake_romm::FakeRomm;
    use crate::romm::RommClient;
    use super::super::session::{Outcome, sync};

    const ROM: i64 = 7;

    /// A device with PPSSPP's memory stick and the app's own store.
    struct Device {
        root: PathBuf,
        conn: Connection,
    }

    impl Device {
        fn new(name: &str, server: &FakeRomm) -> Device {
            let root = temp(&format!("move-{name}"));
            fs::create_dir_all(root.join("stick/PSP/SAVEDATA")).unwrap();
            let id = format!("device-{name}");
            server.device(&id);
            let conn = Connection { host: server.host.clone(), token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: Some(id) };
            Device { root, conn }
        }

        fn savedata(&self) -> PathBuf {
            self.root.join("stick/PSP/SAVEDATA")
        }

        fn play(&self, folder: &str, progress: &str) {
            fs::create_dir_all(self.savedata().join(folder)).unwrap();
            fs::write(self.savedata().join(folder).join("SD.BIN"), progress).unwrap();
        }

        fn save(&self, folder: &str) -> Option<String> {
            fs::read_to_string(self.savedata().join(folder).join("SD.BIN")).ok()
        }

        /// The engine before sigil's thread for one save folder.
        fn old(&self, folder: &str) -> Thread {
            standalone::thread(standalone::Emulator::Ppsspp, ROM, None, standalone::folder_bundle(&self.savedata(), folder))
        }

        /// The game through sigil, its folders before sigil `folders`.
        fn unit(&self, folders: &[&str]) -> Thread {
            let game = Game { rom_id: ROM, rom: Path::new("/roms/psp/DJ Max - Portable 3.chd"), system: "psp", title: "DJ Max", stamp: None };
            let old = folders.iter().map(|f| self.old(f)).collect();
            assemble(game, psp(&self.root.join("stick")), "ppsspp".into(), old, None).unwrap()
        }

        async fn sync(&self, threads: Vec<Thread>) -> super::super::Report {
            match sync(&RommClient::new().unwrap(), &self.conn, &self.root.join("store"), threads, &[ROM], Vec::new(), &|_| None).await.unwrap() {
                Outcome::Done(report) => *report,
                Outcome::Unsupported(why) => panic!("{why}"),
            }
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn entries(bytes: &[u8]) -> Vec<String> {
        let mut names: Vec<String> = super::super::hash::zip_entries(bytes).unwrap().into_iter().map(|(n, _)| n).collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_psp_game_moves_onto_sigil_without_a_conflict_on_any_device_and_the_old_shape_still_comes_through() {
        let server = FakeRomm::start().await;
        let (deck, laptop, pc) = (Device::new("deck", &server), Device::new("laptop", &server), Device::new("pc", &server));

        // Before sigil: the Deck's save folder goes up as a zip of its own files, and the laptop
        // takes it.
        deck.play("ULUS10538", "chapter 1");
        deck.sync(vec![deck.old("ULUS10538")]).await;
        laptop.sync(vec![laptop.old("ULUS10538")]).await;
        assert_eq!(laptop.save("ULUS10538").as_deref(), Some("chapter 1"));
        assert_eq!(entries(&server.newest(ROM, "autosave").unwrap().bytes), ["SD.BIN"], "the old shape");

        // The Deck moves onto sigil. Nothing changed, so nothing goes up — and nobody is asked.
        let report = deck.sync(vec![deck.unit(&["ULUS10538"])]).await;
        assert!(report.conflicts.is_empty() && report.pushed.is_empty(), "{report:?}");
        assert_eq!(server.rows(ROM).len(), 1);
        let report = deck.sync(vec![deck.unit(&["ULUS10538"])]).await;
        assert_eq!((report.in_sync, report.pushed.len()), (1, 0), "and it stays agreed: {report:?}");

        // Played on, its unit goes up — under a name the old shape never gave anything.
        deck.play("ULUS10538", "chapter 2");
        let report = deck.sync(vec![deck.unit(&["ULUS10538"])]).await;
        assert_eq!(report.pushed, ["sigil ULUS10538.zip"]);
        let newest = server.newest(ROM, "autosave").unwrap();
        assert_eq!(names::canonical(&newest.file_name), "sigil ULUS10538.zip");
        assert_eq!(entries(&newest.bytes), ["ULUS10538/SD.BIN"], "sigil's shape");

        // A PC with nothing takes the unit.
        let report = pc.sync(vec![pc.unit(&["ULUS10538"])]).await;
        assert_eq!(pc.save("ULUS10538").as_deref(), Some("chapter 2"), "{report:?}");

        // The laptop, unchanged since it agreed in the old shape, moves over and is only behind.
        let report = laptop.sync(vec![laptop.unit(&["ULUS10538"])]).await;
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert_eq!(laptop.save("ULUS10538").as_deref(), Some("chapter 2"));

        // A TV still on the Kotlin app pushes the old shape under the old name. Read, adapted, taken.
        let flat = standalone::pack(&[("SD.BIN".to_string(), b"chapter 3".to_vec())]).unwrap();
        server.push_from(Some("device-tv"), ROM, "autosave", "ULUS10538.zip", &flat);
        let report = deck.sync(vec![deck.unit(&["ULUS10538"])]).await;
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert_eq!(deck.save("ULUS10538").as_deref(), Some("chapter 3"));
        assert!(!deck.savedata().join("ULUS10538/ULUS10538").exists());
    }

    #[tokio::test]
    async fn what_another_device_pushed_in_the_old_shape_comes_down_before_the_game_moves() {
        let server = FakeRomm::start().await;
        let deck = Device::new("deck-behind", &server);
        deck.play("ULUS10538", "chapter 1");
        deck.sync(vec![deck.old("ULUS10538")]).await;
        // A TV on the old shape moves on.
        let flat = standalone::pack(&[("SD.BIN".to_string(), b"chapter 2, on the tv".to_vec())]).unwrap();
        server.push_from(Some("device-tv"), ROM, "autosave", "ULUS10538.zip", &flat);

        let report = deck.sync(vec![deck.unit(&["ULUS10538"])]).await;
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert_eq!(deck.save("ULUS10538").as_deref(), Some("chapter 2, on the tv"));
        assert!(report.pushed.is_empty(), "the same saves, not sent back: {report:?}");
    }

    #[tokio::test]
    async fn a_game_whose_old_names_were_never_the_units_sends_its_unit_once_they_are_settled() {
        let server = FakeRomm::start().await;
        let deck = Device::new("deck-data", &server);
        deck.play("ULUS10538DATA00", "chapter 1");
        deck.sync(vec![deck.old("ULUS10538DATA00")]).await;

        let report = deck.sync(vec![deck.unit(&["ULUS10538DATA00"])]).await;
        assert_eq!(report.pushed, ["ULUS10538.zip"], "{report:?}");
        assert_eq!(entries(&server.newest(ROM, "autosave").unwrap().bytes), ["ULUS10538DATA00/SD.BIN"]);
        let report = deck.sync(vec![deck.unit(&["ULUS10538DATA00"])]).await;
        assert_eq!((report.in_sync, report.pushed.len()), (1, 0), "{report:?}");
    }

    /// Every game downloaded on this machine, as sigil finds its saves: read only, RomM never asked.
    /// `cargo test real_units -- --ignored --nocapture` on a device with games on it.
    #[test]
    #[ignore = "reads this machine's games and saves; run explicitly"]
    fn real_units() {
        // The real store, not the one tests are given.
        let store = dirs::home_dir().unwrap().join(".config/deckhand-rust");
        let settings: crate::config::Settings = serde_json::from_str(&fs::read_to_string(store.join("settings.json")).unwrap()).unwrap();
        let host = settings.connection.as_ref().map(|c| c.host.clone()).unwrap_or_default();
        let install = super::super::retroarch::locate(settings.retroarch_config.as_deref());
        for installed in crate::downloads::installed(&store, &host) {
            let plan = crate::launch::Plan::new(&settings, &installed);
            let Some(runner) = super::super::runner(&plan) else {
                println!("{:<44} {:?}: not synced", plan.title, plan.emulator);
                continue;
            };
            match thread(Game::of(&plan, plan.stamp.as_ref()), runner, install.as_ref().map_err(String::as_str)) {
                Ok(thread) => {
                    let Place::Sigil(unit) = &thread.place else { unreachable!() };
                    let held = unit.held().map(|h| h.map(|(hash, bytes, _)| format!("{bytes} bytes, {hash}")));
                    println!(
                        "{:<44} {} in {}: \"{}\" ({:?}, {} files, {} on shared cards, {} old) — {:?}",
                        plan.title,
                        unit.request.layout,
                        unit.request.root.display(),
                        thread.name,
                        unit.shape,
                        unit.members.len(),
                        unit.shared.len(),
                        unit.legacy.len(),
                        held
                    );
                }
                Err(why) => println!("{:<44} {:?}: not through sigil: {why}", plan.title, plan.emulator),
            }
        }
    }

    #[test]
    fn an_n64_save_goes_up_as_the_zip_sigil_packs_and_a_cores_srm_from_before_is_still_it() {
        let dir = temp("n64");
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        rom[0x20..0x34].copy_from_slice(b"SUPER MARIO 64      ");
        rom[0x3B..0x3F].copy_from_slice(b"NSME");
        let rom_path = dir.join("Super Mario 64 (USA).z64");
        fs::write(&rom_path, &rom).unwrap();
        fs::create_dir_all(dir.join("saves")).unwrap();
        let mut srm = vec![0u8; 0x48800];
        srm[..8].copy_from_slice(b"PROGRESS");
        fs::write(dir.join("saves/Super Mario 64 (USA).srm"), &srm).unwrap();
        let identity = sigil::identify(&rom_path, Some("n64")).unwrap();
        let request = sigil::Request {
            layout: "mupen64plus_next".into(),
            platform: Some("n64".into()),
            content: rom_path.clone(),
            identity: Some(identity),
            options: Vec::new(),
            root: dir.join("saves"),
            profile: None,
            game_ids: vec![identity.title_id()],
        };
        let old = super::super::retroarch_thread(
            7,
            None,
            &super::super::retroarch::LocalSave {
                path: dir.join("saves/Super Mario 64 (USA).srm"),
                stem: "Super Mario 64 (USA)".into(),
                content_folder: None,
                emulator: "mupen64plus_next".into(),
            },
        );
        let game = Game { rom_id: 7, rom: &rom_path, system: "n64", title: "Super Mario 64", stamp: None };
        let thread = assemble(game, request, "mupen64plus_next".into(), vec![old], None).unwrap();
        assert_eq!(thread.name, "Super Mario 64 (USA).zip", "the zip sigil packs, under a zip's name");
        let Place::Sigil(unit) = &thread.place else { unreachable!() };
        assert_eq!(unit.legacy.len(), 1, "the .srm before is settled first");
        assert!(unit.holds("Super Mario 64 (USA) [2026-10-01_10-00-00].srm"), "and RomM's web player's .srm is still this save");
        let (body, _) = unit.body().unwrap();
        assert!(super::super::hash::zip_entries(&body).is_some());
        // A zip of save types put back is not taken for a save folder.
        let (_, written) = unit.put(&body, None, "Super Mario 64 (USA).zip").unwrap();
        assert!(written.iter().all(|p| !p.to_string_lossy().contains("Super Mario 64 (USA)/")), "{written:?}");
        assert_eq!(&fs::read(dir.join("saves/Super Mario 64 (USA).srm")).unwrap()[..8], b"PROGRESS");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cores_builds_share_its_layout() {
        assert_eq!(layout_of_core("mupen64plus_next_gles3"), "mupen64plus_next");
        assert_eq!(layout_of_core("mednafen_psx_hw"), "mednafen_psx_hw");
        assert_eq!(kinds(Some("gb"), "Pokemon Yellow.srm"), ["rtc", "sav", "srm"]);
        assert_eq!(kinds(Some("psp"), "ULUS10538.zip"), Vec::<String>::new());
        assert_eq!(kinds(None, "Sonic.srm"), ["srm"]);
    }
}

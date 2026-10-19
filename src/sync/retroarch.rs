//! Where RetroArch keeps battery saves, read from its own config rather than assumed.
//!
//! A port of the Kotlin app's `RetroArchConfig`, widened from RetroDECK to any RetroArch: the
//! RetroDECK and RetroArch Flatpaks, a native Linux install, Windows and macOS.
//!
//! RetroArch does not write saves to one fixed place. It composes the directory from settings the
//! user can change from its own menu:
//!
//! ```text
//! savefile_directory               = ".../saves"
//! sort_savefiles_by_content_enable = "true"   adds the content folder's name
//! sort_savefiles_enable            = "true"   adds the core's name
//! ```
//!
//! so a save lands in `<savefile_directory>/<content folder>/<core>/`, each segment present only if
//! its flag is on — `saves/snes/Snes9x/Chrono Trigger (USA).srm` on a stock RetroDECK. The Kotlin
//! app once hardcoded the flat shape; the day sorting by core was switched on, every save moved one
//! level down and its sync went on reporting success over a folder nothing wrote to any more.
//!
//! Saves kept beside the games — `savefiles_in_content_dir`, or no `savefile_directory` at all — are
//! found in the game's own folder around a launch, and in the folders `installed.json` records in a
//! sweep. A save beside a game this app did not download is not found by a sweep: nothing says where
//! that game is.
//!
//! Override configs — `config/<Core>/<Core>.cfg`, then the content folder's and the game's — are laid
//! over the main config for one game's sync ([`Install::for_game`]), so a core that files its saves
//! differently is followed. A sweep has no core for a save it has not matched yet, so it reads the
//! main config's folders only.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A RetroArch install, as far as its battery saves are concerned.
#[derive(Debug, Clone)]
pub struct Install {
    /// `savefile_directory`, resolved to a real path — or, when RetroArch keeps saves beside the
    /// games, the folder its config is in, which only names where the run looked.
    pub saves: PathBuf,
    /// Saves beside the games: `savefiles_in_content_dir`, or no `savefile_directory` at all, which
    /// is RetroArch's own default. Each game's save is then `<its folder>/<stem>.srm`, the sort flags
    /// do not apply, and only a list of the games' folders says where to look — see
    /// [`Install::with_content_dirs`].
    beside: bool,
    /// The folders a sweep reads when saves are beside the games: where the downloaded games are.
    content_dirs: Vec<PathBuf>,
    by_content: bool,
    by_core: bool,
    /// Where save states go, composed the same way from `savestate_directory` and its own two sort
    /// flags. None when RetroArch keeps them beside the games.
    states: Option<StateFolders>,
    /// The config this was read from, and what it said — kept so a game's overrides can be laid over.
    config: PathBuf,
    settings: HashMap<String, String>,
    /// Where RetroArch keeps override configs: `<this>/<Core>/<Core>.cfg` and the narrower two.
    overrides: PathBuf,
    /// A core's id (`snes9x`, `genesis_plus_gx`) to its folder name (`Snes9x`, `Genesis Plus GX`),
    /// from the cores' `.info` files. Keyed by id because builds of one core share a folder:
    /// RetroDECK ships four `mupen64plus_next` builds, every one of them `Mupen64Plus-Next`.
    cores: HashMap<String, String>,
}

/// `savestate_directory` and how RetroArch sorts inside it — the same three settings as saves, with
/// names of their own, so the two can be filed differently.
#[derive(Debug, Clone)]
struct StateFolders {
    root: PathBuf,
    by_content: bool,
    by_core: bool,
}

/// One battery save on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalSave {
    pub path: PathBuf,
    /// The file name without `.srm` — the stem of the content RetroArch loaded.
    pub stem: String,
    /// The content folder the save was sorted under, when RetroArch sorts by content. For a game
    /// kept in its system's folder that is the system's folder name, which is what tells two games
    /// of the same name on different systems apart.
    pub content_folder: Option<String>,
    /// The core that wrote it, as RomM's `emulator` field names it — or `retroarch` when nothing on
    /// disk says which core that was.
    pub emulator: String,
}

/// One game's saves under the current settings.
#[derive(Debug, Default, PartialEq)]
pub struct GameSaves {
    /// Its saves already on disk — more than one when it has been played under more than one core.
    pub existing: Vec<LocalSave>,
    /// Where RetroArch will write its save, when that is known: where a save from RomM goes when
    /// this device has none yet.
    pub target: Option<LocalSave>,
}

/// Where a `retroarch.cfg` may be, most specific first. The first that exists is the one used.
///
/// RetroDECK leads on Linux: on a Deck that has it, it is the RetroArch games are launched through,
/// and a stray desktop install alongside is the one *not* being played.
pub fn candidates() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
        for app in ["net.retrodeck.retrodeck", "org.libretro.RetroArch"] {
            out.push(home.join(".var/app").join(app).join("config/retroarch/retroarch.cfg"));
        }
        if let Some(config) = dirs::config_dir() {
            out.push(config.join("retroarch/retroarch.cfg"));
        }
    } else if cfg!(windows) {
        if let Some(roaming) = dirs::config_dir() {
            out.push(roaming.join("RetroArch").join("retroarch.cfg"));
        }
        // The portable zip keeps its config beside the executable, wherever it was unpacked; these
        // are the folders its own instructions and Steam use.
        out.push(PathBuf::from(r"C:\RetroArch-Win64\retroarch.cfg"));
        out.push(PathBuf::from(r"C:\RetroArch\retroarch.cfg"));
        if let Some(programs) = std::env::var_os("ProgramFiles(x86)") {
            out.push(PathBuf::from(programs).join(r"Steam\steamapps\common\RetroArch\retroarch.cfg"));
        }
    } else if cfg!(target_os = "macos") {
        out.push(home.join("Library/Application Support/RetroArch/config/retroarch.cfg"));
    }
    out
}

/// The config found when the settings name none: the first of [`candidates`] that exists.
pub fn found() -> Option<PathBuf> {
    candidates().into_iter().find(|c| c.is_file())
}

/// Find RetroArch's config — `chosen` when the settings name one, otherwise [`found`] — and read
/// where it keeps saves.
pub fn locate(chosen: Option<&Path>) -> Result<Install, String> {
    if let Some(config) = chosen {
        return read(config);
    }
    match found() {
        Some(config) => read(&config),
        None => Err("No RetroArch found. Pick its retroarch.cfg in Deckhand's Settings ▸ Saves ▸ RetroArch config.".into()),
    }
}

/// Where RetroArch looks for BIOS files: `system_directory` in the same config [`locate`] finds.
///
/// Read on its own rather than off an [`Install`], because a BIOS has nothing to do with where saves
/// go, and a RetroArch with no save folder at all still needs its BIOS. On RetroDECK this is its shared `bios/` folder, which its standalone emulators read too.
pub fn bios_dir(chosen: Option<&Path>) -> Result<PathBuf, String> {
    let config = match chosen {
        Some(config) => config.to_path_buf(),
        None => found().ok_or("No RetroArch found, so there is no BIOS folder to fill.")?,
    };
    let text = fs::read_to_string(&config).map_err(|e| format!("Could not read {}: {e}", config.display()))?;
    parse(&text)
        .get("system_directory")
        .and_then(|raw| resolve(raw, &config))
        // Unset means "beside the content", which differs per game and is no one folder to fill.
        .ok_or_else(|| format!("{} sets no system_directory, so there is no BIOS folder to fill.", config.display()))
}

/// Read one `retroarch.cfg`.
pub fn read(config: &Path) -> Result<Install, String> {
    let text = fs::read_to_string(config)
        .map_err(|e| format!("Could not read {}: {e}", config.display()))?;
    Ok(build(parse(&text), config))
}

/// An install from settings already read — the main config's, or those with a game's overrides laid
/// over them.
fn build(settings: HashMap<String, String>, config: &Path) -> Install {
    let flag = |key: &str| settings.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"));

    // An unset directory is RetroArch's built-in default, which is also "beside the content".
    let directory = settings
        .get("savefile_directory")
        .filter(|_| !flag("savefiles_in_content_dir"))
        .and_then(|raw| resolve(raw, config));
    let beside = directory.is_none();
    let saves = directory.unwrap_or_else(|| config.parent().unwrap_or(Path::new(".")).to_path_buf());

    let cores = settings
        .get("libretro_info_path")
        .and_then(|raw| resolve(raw, config))
        .map(|dir| cores(&dir))
        .unwrap_or_default();

    let states = settings
        .get("savestate_directory")
        .filter(|_| !flag("savestates_in_content_dir"))
        .and_then(|raw| resolve(raw, config))
        .map(|root| StateFolders {
            root,
            by_content: flag("sort_savestates_by_content_enable"),
            by_core: flag("sort_savestates_enable"),
        });

    // Overrides live under the menu's config folder, `config/` beside retroarch.cfg unless moved.
    let overrides = settings
        .get("rgui_config_directory")
        .and_then(|raw| resolve(raw, config))
        .unwrap_or_else(|| config.parent().unwrap_or(Path::new(".")).join("config"));

    Install {
        saves,
        beside,
        content_dirs: Vec::new(),
        by_content: flag("sort_savefiles_by_content_enable"),
        by_core: flag("sort_savefiles_enable"),
        states,
        cores,
        config: config.to_path_buf(),
        settings,
        overrides,
    }
}

impl Install {
    /// This install as it is for one game run by `core` (an id): the main config with RetroArch's
    /// overrides laid over it, broadest first — the core's own (`<Core>/<Core>.cfg`), the content
    /// folder's (`<Core>/<folder>.cfg`), the game's (`<Core>/<stem>.cfg`). Any of them can move or
    /// re-sort saves for that core alone; RetroDECK's PPSSPP core turns sorting by content off.
    ///
    /// Without a core there is no override to find, and the main config is the answer.
    pub fn for_game(&self, rom: &Path, core: Option<&str>) -> Install {
        let Some(name) = core.and_then(|id| self.core_name(id)) else { return self.clone() };
        let dir = self.overrides.join(name);
        let content = rom.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
        let stem = rom.file_stem().map(|n| n.to_string_lossy().into_owned());
        let mut settings = self.settings.clone();
        let mut any = false;
        for file in [Some(name.to_string()), content, stem].into_iter().flatten() {
            if let Ok(text) = fs::read_to_string(dir.join(format!("{file}.cfg"))) {
                settings.extend(parse(&text));
                any = true;
            }
        }
        if !any {
            return self.clone();
        }
        let mut install = build(settings, &self.config);
        install.cores.clone_from(&self.cores);
        install.content_dirs.clone_from(&self.content_dirs);
        install
    }

    /// The folders downloaded games are in, which is where a sweep looks when saves are beside them.
    pub fn with_content_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.content_dirs = dirs;
        self
    }

    /// Every folder a save or state of this install is written in — what the window watches for
    /// changes: the save folder, or each game's own when saves sit beside the games, and the state
    /// folder likewise.
    pub fn watched(&self) -> Vec<PathBuf> {
        let mut out = if self.beside { self.content_dirs.clone() } else { vec![self.saves.clone()] };
        match &self.states {
            Some(folders) => out.push(folders.root.clone()),
            None => out.extend(self.content_dirs.iter().cloned()),
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every `.srm` in the folders RetroArch writes to under its current settings, in path order.
    ///
    /// Exactly that depth and no other. A save sitting one level up was written before sorting was
    /// switched on, and RetroArch no longer opens it — refreshing it would put progress where the
    /// game cannot see it, and pushing it would offer RomM a copy the player has since moved on from.
    pub fn saves(&self) -> Vec<LocalSave> {
        if self.beside {
            // Beside the games: each game folder is its own content folder, and no core is on disk.
            let mut saves: Vec<LocalSave> = self
                .content_dirs
                .iter()
                .flat_map(|dir| {
                    let content = dir.file_name().map(|n| n.to_string_lossy().into_owned());
                    battery_saves(dir).into_iter().map(move |(path, stem)| LocalSave {
                        path,
                        stem,
                        content_folder: content.clone(),
                        emulator: "retroarch".to_string(),
                    })
                })
                .collect();
            saves.sort_by(|a, b| a.path.cmp(&b.path));
            saves.dedup_by(|a, b| a.path == b.path);
            return saves;
        }
        // (folder, content folder, core folder)
        let mut folders = vec![(self.saves.clone(), None::<String>, None::<String>)];
        if self.by_content {
            folders = folders
                .into_iter()
                .flat_map(|(dir, _, _)| subfolders(&dir).into_iter().map(|(path, name)| (path, Some(name), None)))
                .collect();
        }
        if self.by_core {
            folders = folders
                .into_iter()
                .flat_map(|(dir, content, _)| {
                    subfolders(&dir).into_iter().map(move |(path, name)| (path, content.clone(), Some(name)))
                })
                .collect();
        }

        let mut saves: Vec<LocalSave> = folders
            .into_iter()
            .flat_map(|(dir, content, core)| {
                let emulator = core
                    .and_then(|name| self.core_id(&name).map(str::to_string))
                    .unwrap_or_else(|| "retroarch".to_string());
                battery_saves(&dir)
                    .into_iter()
                    .map(move |(path, stem)| LocalSave {
                        path,
                        stem,
                        content_folder: content.clone(),
                        emulator: emulator.clone(),
                    })
            })
            .collect();
        saves.sort_by(|a, b| a.path.cmp(&b.path));
        saves
    }

    /// The saves of the game whose content file is `rom`, run by `core` (an id, `snes9x`) if known.
    ///
    /// The save is `<rom stem>.srm` in the folder RetroArch composes for this content, whose content
    /// segment is the ROM's own folder. The core segment needs the core. Without it the game's
    /// existing saves are still found, by looking in every core's folder, but there is nowhere to put
    /// a new one: a guessed core files the save where the core that actually runs never looks.
    pub fn game_saves(&self, rom: &Path, core: Option<&str>) -> GameSaves {
        let Some(stem) = rom.file_stem().and_then(|s| s.to_str()) else { return GameSaves::default() };
        let content = rom.parent().and_then(Path::file_name).and_then(|n| n.to_str());
        if self.beside {
            let Some(dir) = rom.parent() else { return GameSaves::default() };
            let target = LocalSave {
                path: dir.join(format!("{stem}.srm")),
                stem: stem.to_string(),
                content_folder: content.map(str::to_string),
                emulator: core.unwrap_or("retroarch").to_string(),
            };
            let here = fs::metadata(&target.path).is_ok_and(|m| m.is_file() && m.len() > 0);
            return GameSaves { existing: here.then(|| target.clone()).into_iter().collect(), target: Some(target) };
        }
        let mut dir = self.saves.clone();
        if self.by_content {
            let Some(content) = content else { return GameSaves::default() };
            dir.push(content);
        }
        let save = |dir: &Path, emulator: &str| LocalSave {
            path: dir.join(format!("{stem}.srm")),
            stem: stem.to_string(),
            content_folder: content.filter(|_| self.by_content).map(str::to_string),
            emulator: emulator.to_string(),
        };
        let on_disk = |save: &LocalSave| fs::metadata(&save.path).is_ok_and(|m| m.is_file() && m.len() > 0);

        let target = if !self.by_core {
            Some(save(&dir, core.unwrap_or("retroarch")))
        } else {
            core.and_then(|id| self.core_name(id).map(|name| save(&dir.join(name), id)))
        };
        let existing = match &target {
            Some(target) => on_disk(target).then(|| target.clone()).into_iter().collect(),
            None => subfolders(&dir)
                .into_iter()
                .map(|(folder, name)| save(&folder, self.core_id(&name).unwrap_or("retroarch")))
                .filter(on_disk)
                .collect(),
        };
        GameSaves { existing, target }
    }

    /// Whether RetroArch loads a game's auto slot state by itself when the game starts —
    /// `savestate_auto_load`, which naming a slot to start in overrides.
    pub fn loads_auto_state(&self) -> bool {
        self.settings.get("savestate_auto_load").is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// The folder RetroArch keeps `rom`'s save states in, run by `core` (an id) if known.
    ///
    /// Err says why there is none: states kept beside the games, or filed by a core nobody named —
    /// a guessed core would put a pulled state where the core that runs never looks.
    pub fn game_states_dir(&self, rom: &Path, core: Option<&str>) -> Result<PathBuf, String> {
        // Beside the games: the game's own folder, sorted by nothing.
        let Some(folders) = self.states.as_ref() else {
            return rom.parent().map(Path::to_path_buf).ok_or_else(|| "the game has no folder".to_string());
        };
        let mut dir = folders.root.clone();
        if folders.by_content {
            dir.push(rom.parent().and_then(Path::file_name).ok_or("the game has no folder to sort its states by")?);
        }
        if folders.by_core {
            let name = core
                .and_then(|id| self.core_name(id))
                .ok_or("RetroArch files save states by core, and which core runs this game is not known")?;
            dir.push(name);
        }
        Ok(dir)
    }

    /// RetroArch's `system_directory`, where some cores keep their saves too: LRPS2 its memory cards
    /// under `pcsx2/memcards`, Flycast its VMUs under `dc`.
    pub fn system_dir(&self) -> Option<PathBuf> {
        self.settings.get("system_directory").and_then(|raw| resolve(raw, &self.config))
    }

    /// The core options `core` (an id) runs `rom` with, in RetroArch's own order: the global file,
    /// then the core's `.opt`, the content folder's and the game's, each over the last. Which file a
    /// save is in can turn on them — a PS1 core's memory card method, a Sega CD core's backup RAM.
    pub fn core_options(&self, rom: &Path, core: &str) -> HashMap<String, String> {
        let global = self
            .settings
            .get("core_options_path")
            .and_then(|raw| resolve(raw, &self.config))
            .unwrap_or_else(|| self.config.parent().unwrap_or(Path::new(".")).join("retroarch-core-options.cfg"));
        let mut options = fs::read_to_string(global).map(|text| parse(&text)).unwrap_or_default();
        if let Some(name) = self.core_name(core) {
            let dir = self.overrides.join(name);
            let content = rom.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
            let stem = rom.file_stem().map(|n| n.to_string_lossy().into_owned());
            for file in [Some(name.to_string()), content, stem].into_iter().flatten() {
                if let Ok(text) = fs::read_to_string(dir.join(format!("{file}.opt"))) {
                    options.extend(parse(&text));
                }
            }
        }
        options
    }

    /// The folder name RetroArch sorts a core's saves under, from its id.
    pub fn core_name(&self, id: &str) -> Option<&str> {
        self.cores.get(id).map(String::as_str)
    }

    /// The id a save found in a core's folder is tagged with. Of the builds sharing the folder, the
    /// plainest — the shortest id, `mupen64plus_next` over its `_gles3` — so the answer is the same
    /// on every run.
    fn core_id(&self, folder: &str) -> Option<&str> {
        self.core_builds(folder).into_iter().min_by_key(|id| (id.len(), *id))
    }

    /// Every id filed under RetroArch's folder `folder`: the builds of one core, which load each
    /// other's states.
    pub fn core_builds(&self, folder: &str) -> Vec<&str> {
        self.cores.iter().filter(|(_, name)| name.as_str() == folder).map(|(id, _)| id.as_str()).collect()
    }
}

/// The visible folders directly inside `dir`, with their names.
fn subfolders(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.')).then(|| (e.path(), name))
        })
        .collect()
}

/// The non-empty `.srm` files directly inside `dir`, with their stems. An empty file is a save the
/// core created and never wrote; there is nothing in it to keep.
fn battery_saves(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && m.len() > 0))
        .filter_map(|e| {
            let path = e.path();
            let is_srm = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("srm"));
            let stem = path.file_stem()?.to_str()?.to_string();
            is_srm.then_some((path, stem))
        })
        .collect()
}

/// Core id → `corename` for every `.info` file in `dir`. RetroArch names a core's save folder after
/// the `corename` field, not the file — `genesis_plus_gx_libretro.info` files under `Genesis Plus
/// GX` — so this is a lookup, and no table here can go stale when a core is renamed upstream.
fn cores(dir: &Path) -> HashMap<String, String> {
    let Ok(entries) = fs::read_dir(dir) else { return HashMap::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.extension().is_some_and(|x| x == "info") {
                return None;
            }
            let id = path.file_stem()?.to_str()?.trim_end_matches("_libretro").to_lowercase();
            let name = parse(&fs::read_to_string(&path).ok()?).remove("corename")?;
            (!name.is_empty()).then_some((id, name))
        })
        .collect()
}

/// RetroArch's config format, shared by `retroarch.cfg` and the `.info` files: `key = "value"`, one
/// per line, `#` comments, quotes optional.
fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_string(), value.trim().trim_matches('"').to_string()))
        })
        .collect()
}

/// A directory named in the config, as a path on this machine. None for RetroArch's `default`.
///
/// - `:` is RetroArch's own directory. Taken as the config's folder, which it is for the portable
///   install that writes it.
/// - `~` is the home directory.
/// - Inside a Flatpak, RetroArch sees its sandbox: `/var/config/…` there is
///   `~/.var/app/<id>/config/…` from outside, which is where this is read from.
fn resolve(raw: &str, config: &Path) -> Option<PathBuf> {
    let value = raw.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("default") {
        return None;
    }
    let config_dir = config.parent().unwrap_or(Path::new("."));
    if let Some(rest) = value.strip_prefix(':') {
        return Some(config_dir.join(rest.trim_start_matches(['/', '\\'])));
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        return Some(dirs::home_dir().unwrap_or_default().join(rest));
    }
    if let (Some(sandbox), Some(rest)) = (flatpak_root(config), value.strip_prefix("/var/")) {
        return Some(sandbox.join(rest));
    }
    let path = PathBuf::from(value);
    Some(if path.is_absolute() { path } else { config_dir.join(path) })
}

/// `~/.var/app/<id>` when `config` is inside a Flatpak's data, else None.
fn flatpak_root(config: &Path) -> Option<PathBuf> {
    config.ancestors().find_map(|dir| {
        let app = dir.parent()?;
        let var = app.parent()?;
        (app.file_name()? == "app" && var.file_name()? == ".var").then(|| dir.to_path_buf())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A RetroArch install laid out under the temp directory: its config, and the saves under it.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, config: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-ra-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("retroarch.cfg"), config).unwrap();
            Self { root }
        }

        fn file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }

        fn install(&self) -> Install {
            read(&self.root.join("retroarch.cfg")).expect("the fixture config reads")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_save_is_found_under_its_content_and_core_folders_and_tagged_with_the_core() {
        let ra = Fixture::new(
            "sorted",
            "savefile_directory = \":/saves\"\n\
             libretro_info_path = \":/info\"\n\
             sort_savefiles_by_content_enable = \"true\"\n\
             sort_savefiles_enable = \"true\"\n",
        );
        ra.file("info/snes9x_libretro.info", "corename = \"Snes9x\"\n");
        let save = ra.file("saves/snes/Snes9x/Chrono Trigger (USA).srm", "progress");

        let saves = ra.install().saves();
        assert_eq!(
            saves,
            vec![LocalSave {
                path: save,
                stem: "Chrono Trigger (USA)".into(),
                content_folder: Some("snes".into()),
                emulator: "snes9x".into(),
            }]
        );
    }

    #[test]
    fn builds_of_one_core_each_find_the_folder_they_share() {
        let ra = Fixture::new(
            "builds",
            "savefile_directory = \":/saves\"\n\
             libretro_info_path = \":/info\"\n\
             sort_savefiles_enable = \"true\"\n",
        );
        // RetroDECK ships these four, every one filed under the same folder.
        for build in ["mupen64plus_next_gles3", "mupen64plus_next", "mupen64plus_next_develop", "mupen64plus_next_gles2"] {
            ra.file(&format!("info/{build}_libretro.info"), "corename = \"Mupen64Plus-Next\"\n");
        }
        let install = ra.install();
        for build in ["mupen64plus_next", "mupen64plus_next_gles3", "mupen64plus_next_develop", "mupen64plus_next_gles2"] {
            assert_eq!(install.core_name(build), Some("Mupen64Plus-Next"), "{build}");
        }
        let mut builds = install.core_builds("Mupen64Plus-Next");
        builds.sort_unstable();
        assert_eq!(builds, ["mupen64plus_next", "mupen64plus_next_develop", "mupen64plus_next_gles2", "mupen64plus_next_gles3"]);

        // A save found in the folder is tagged with the plainest build, the same on every run.
        ra.file("saves/Mupen64Plus-Next/Super Mario 64 (USA).srm", "progress");
        assert_eq!(install.saves()[0].emulator, "mupen64plus_next");
    }

    #[test]
    fn a_launched_game_s_save_is_where_its_core_will_write_it() {
        let ra = Fixture::new(
            "game",
            "savefile_directory = \":/saves\"\n\
             libretro_info_path = \":/info\"\n\
             sort_savefiles_by_content_enable = \"true\"\n\
             sort_savefiles_enable = \"true\"\n",
        );
        ra.file("info/snes9x_libretro.info", "corename = \"Snes9x\"\n");
        let install = ra.install();
        let rom = Path::new("roms").join("snes").join("Chrono Trigger (USA).sfc");
        let expected = ra.root.join("saves").join("snes").join("Snes9x").join("Chrono Trigger (USA).srm");

        // Nothing written yet, and the core named: the folder RetroArch will use is known.
        let fresh = install.game_saves(&rom, Some("snes9x"));
        assert!(fresh.existing.is_empty());
        assert_eq!(fresh.target.map(|t| t.path), Some(expected.clone()));

        // No core named: nowhere to put a new save…
        assert_eq!(install.game_saves(&rom, None).target, None);
        // …but one a core already wrote is found, and tagged with that core.
        ra.file("saves/snes/Snes9x/Chrono Trigger (USA).srm", "progress");
        let found = install.game_saves(&rom, None);
        assert_eq!(found.existing.len(), 1);
        assert_eq!(found.existing[0].path, expected);
        assert_eq!(found.existing[0].emulator, "snes9x");
    }

    #[test]
    fn a_launched_game_s_save_in_a_flat_folder_needs_no_core() {
        let ra = Fixture::new("flat-game", "savefile_directory = \":/saves\"\n");
        let target = ra.install().game_saves(Path::new("roms/snes/Game (USA).sfc"), None).target.unwrap();
        assert_eq!(target.path, ra.root.join("saves").join("Game (USA).srm"));
        assert_eq!(target.emulator, "retroarch");
    }

    #[test]
    fn a_save_at_a_depth_retroarch_no_longer_writes_to_is_left_alone() {
        // Written before sorting by core was switched on, so RetroArch no longer opens it.
        let ra = Fixture::new("stale", "savefile_directory = \":/saves\"\nsort_savefiles_enable = \"true\"\n");
        ra.file("saves/Old Game (USA).srm", "stale");
        let current = ra.file("saves/mGBA/New Game (USA).srm", "current");

        let saves: Vec<PathBuf> = ra.install().saves().into_iter().map(|s| s.path).collect();
        assert_eq!(saves, vec![current]);
    }

    #[test]
    fn only_non_empty_battery_saves_are_listed() {
        let ra = Fixture::new("flat", "savefile_directory = \":/saves\"\n");
        let save = ra.file("saves/Game (USA).srm", "progress");
        ra.file("saves/Never Written (USA).srm", "");
        ra.file("saves/Game (USA).state", "a state, not a save");
        ra.file("saves/Game (USA).srm.deckhand-part", "a download in flight");

        let saves = ra.install().saves();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].path, save);
        // No core folder says which core wrote it.
        assert_eq!(saves[0].emulator, "retroarch");
        assert_eq!(saves[0].content_folder, None);
    }

    #[test]
    fn a_cores_overrides_move_its_saves_broadest_first() {
        let ra = Fixture::new(
            "overrides",
            "savefile_directory = \":/saves\"\nsort_savefiles_by_content_enable = \"true\"\n\
             savestate_directory = \":/states\"\nlibretro_info_path = \":/info\"\n",
        );
        ra.file("info/ppsspp_libretro.info", "corename = \"PPSSPP\"\n");
        let game = ra.root.join("games/psp/Lumines (USA).cso");
        let other = ra.root.join("games/psp/Patapon (USA).cso");
        let install = read(&ra.root.join("retroarch.cfg")).unwrap();
        assert_eq!(install.game_saves(&game, Some("ppsspp")).target.unwrap().path, ra.root.join("saves/psp/Lumines (USA).srm"));

        // RetroDECK's own: the PPSSPP core files its saves flat.
        ra.file("config/PPSSPP/PPSSPP.cfg", "sort_savefiles_by_content_enable = \"false\"\n");
        let path = |rom: &Path| install.for_game(rom, Some("ppsspp")).game_saves(rom, Some("ppsspp")).target.unwrap().path;
        assert_eq!(path(&game), ra.root.join("saves/Lumines (USA).srm"));
        // One game's own override, narrower, wins over the core's — for that game alone.
        ra.file("config/PPSSPP/Lumines (USA).cfg", "savefile_directory = \":/elsewhere\"\n");
        assert_eq!(path(&game), ra.root.join("elsewhere/Lumines (USA).srm"));
        assert_eq!(path(&other), ra.root.join("saves/Patapon (USA).srm"));
        // Another core, or none, sees no override at all.
        assert_eq!(install.for_game(&game, None).game_saves(&game, None).target.unwrap().path, ra.root.join("saves/psp/Lumines (USA).srm"));
    }

    #[test]
    fn saves_kept_beside_the_games_are_found_there_by_a_launch_and_a_sweep() {
        for (name, config) in [
            ("beside", "savefile_directory = \":/saves\"\nsavefiles_in_content_dir = \"true\"\nsort_savefiles_enable = \"true\"\n"),
            ("default", "savefile_directory = \"default\"\n"),
        ] {
            let ra = Fixture::new(name, config);
            let game = ra.file("games/snes/Chrono Trigger (USA).sfc", "rom");
            let install = read(&ra.root.join("retroarch.cfg")).unwrap();

            // A first launch: the save goes beside the game, and no core is needed to say where.
            let saves = install.game_saves(&game, None);
            assert_eq!(saves.target.unwrap().path, ra.root.join("games/snes/Chrono Trigger (USA).srm"));
            assert!(saves.existing.is_empty());
            assert_eq!(install.game_states_dir(&game, None).unwrap(), ra.root.join("games/snes"));

            // A sweep finds it only in a folder it is told holds games.
            let save = ra.file("games/snes/Chrono Trigger (USA).srm", "progress");
            assert!(install.saves().is_empty(), "no game folders, nowhere to look");
            let install = install.with_content_dirs(vec![ra.root.join("games/snes")]);
            let found = install.saves();
            assert_eq!(found.len(), 1);
            assert_eq!((found[0].path.clone(), found[0].content_folder.as_deref()), (save, Some("snes")));
        }
    }

    #[test]
    fn a_flatpak_sandbox_path_is_read_from_outside_the_sandbox() {
        let config = Path::new("/home/deck/.var/app/net.retrodeck.retrodeck/config/retroarch/retroarch.cfg");
        assert_eq!(
            resolve("/var/config/retroarch/saves", config),
            Some(PathBuf::from("/home/deck/.var/app/net.retrodeck.retrodeck/config/retroarch/saves"))
        );
        // A host path — RetroDECK's own storage — is already one. (Only absolute where `/` is root.)
        if cfg!(unix) {
            assert_eq!(
                resolve("/home/deck/retrodeck/saves", config),
                Some(PathBuf::from("/home/deck/retrodeck/saves"))
            );
        }
    }

    #[test]
    fn the_config_format_tolerates_comments_and_missing_quotes() {
        let parsed = parse("# a comment\nsavefile_directory = \"/saves\"\nsort_savefiles_enable = true\n\n");
        assert_eq!(parsed.get("savefile_directory").map(String::as_str), Some("/saves"));
        assert_eq!(parsed.get("sort_savefiles_enable").map(String::as_str), Some("true"));
        assert_eq!(parsed.len(), 2);
    }
}

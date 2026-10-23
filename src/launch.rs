//! Starting a downloaded game, and what has to happen around it.
//!
//! A port of the Kotlin app's `GameLauncher`, kept to its launch order:
//!
//! 1. **BIOS**, so a system that will not boot without one has it — see [`crate::bios`].
//! 2. **The save comes down**, so progress made on another device is what the game opens.
//! 3. **The game runs**, tracked rather than detached: waiting on it is what tells the app when to
//!    come back, and when the save it wrote is ready to go up.
//! 4. **The save goes up**, so the next device starts from here.
//!
//! What runs it is RetroDECK, handed the file and the system: `flatpak run net.retrodeck.retrodeck
//! -s <system> <rom>`. One command for every system, because RetroDECK's own launcher then picks
//! the emulator from ES-DE's configuration — the user's per-system and per-game choices made in
//! ES-DE apply here unchanged. The `-s` is what lets a game live outside RetroDECK's `roms/` tree:
//! without it RetroDECK reads the system out of a `roms/<system>/` segment in the path, which a game
//! under `~/Games/Deckhand` does not have. A command of the player's own, set in Settings ▸ Playing,
//! replaces all of it.
//!
//! The command names only RetroDECK, so which emulator — and which core — will actually run is read
//! back out of ES-DE's own files the same way RetroDECK reads them ([`RetroDeck::emulator`]). Save
//! sync needs that answer: RetroArch may file saves by core, and a standalone emulator keeps saves
//! this build does not sync at all.
//!
//! Steps 1, 2 and 4 are best-effort. None of them is worth not playing over, so each one that fails
//! becomes a line in the banner afterwards and the launch goes on.
//!
//! A launch can be cut short. Steam's *Exit game* — a signal to `deckhand play`, or to the window — is
//! a [`Stop`]: the game is asked to close where it runs, the way its own Quit would, and step 4
//! follows once it has. Whatever ends Deckhand before step 4 is done — a second signal, a crash, a
//! dead battery — leaves the game's marker, and the next Deckhand to start does step 4 for it
//! ([`finish_left`]; see [`crate::playing`]).
//!
//! Step 2 alone is also what a download ends with ([`ready`]): a game's existing progress is laid out
//! before its first launch, which is what makes "download it here, carry on from where I was" work
//! without a launch in between.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use crate::config::{Connection, Settings};
use crate::downloads::Installed;
use crate::downloads::layout::{PLAYABLE, extension};
use crate::library::Stamp;
use crate::romm::{RommClient, HEARTBEAT_LASTS};
use crate::romm::models::PlaySession;
use crate::sync::{self, Report};

/// RetroDECK's Flatpak id.
pub const RETRODECK: &str = "net.retrodeck.retrodeck";

/// Why a game cannot start on a device with neither RetroDECK nor an emulator of the player's own —
/// which the window answers by offering to install RetroDECK.
pub const NO_EMULATOR: &str = "no emulator is set up for it — install RetroDECK, or choose an emulator in Settings ▸ Playing";

/// The ES-DE system a RomM platform slug is, which is the name RetroDECK knows it by.
///
/// The two mostly agree; these are the ones that do not, from the Kotlin app's `romFolderFor`. It
/// matters for more than a folder name here — `-s ngc` is a system RetroDECK has no emulator for.
/// An unknown slug passes through unchanged, so a platform added to RomM still launches if ES-DE
/// happens to call it the same thing.
pub fn es_system(slug: &str) -> String {
    let slug = slug.trim().to_lowercase();
    match slug.as_str() {
        "famicom" | "fc" | "family-computer" => "nes",
        "sfc" | "super-famicom" | "superfamicom" | "super-nintendo-entertainment-system" | "super_nintendo" => "snes",
        "game-boy" | "gameboy" => "gb",
        "game-boy-color" | "gameboycolor" => "gbc",
        "game-boy-advance" | "gameboyadvance" => "gba",
        "nintendo-64" | "nintendo64" => "n64",
        "ngc" | "gamecube" | "nintendo-gamecube" => "gc",
        "wii-u" | "nintendo-wii-u" => "wiiu",
        "3ds" | "nintendo-3ds" | "new-nintendo-3ds" => "n3ds",
        "megadrive" | "md" | "sega-genesis" | "sega-mega-drive" | "genesis-slash-megadrive"
        | "sega-mega-drive-genesis" | "mega-drive" => "genesis",
        "sms" | "master-system" | "sega-master-system" => "mastersystem",
        "game-gear" | "sega-game-gear" => "gamegear",
        "dc" | "sega-dreamcast" => "dreamcast",
        "ps" | "ps1" | "playstation" => "psx",
        "playstation-2" | "playstation2" | "sony-playstation-2" => "ps2",
        "vita" | "playstation-vita" | "ps-vita" => "psvita",
        "jaguar" | "atari-jaguar" => "atarijaguar",
        _ => return slug,
    }
    .to_string()
}

/// The file an emulator should be handed for a downloaded game.
///
/// The `.m3u` when the download wrote one — a multi-disc set is played through its playlist, which
/// is what lets the emulator swap discs, and RetroArch names the save after it. Otherwise the one
/// file, or the one entry point (`.cue`, `.chd`…) among several — never a `.bin` track — or the one
/// file among notes, pictures and patches that can be the game (a cartridge rom). A folder of
/// files with no entry point (a PS3 game's tree) is the folder, which RPCS3 boots as it is;
/// RetroDECK is handed what is in it ([`for_retrodeck`]).
///
/// Never a file beside the game — its update, its DLC, an extra, each in a folder of its own below
/// the game's ([`crate::downloads::layout::beside_the_game`]): an update is not the game, and an
/// emulator handed one starts nothing, or the wrong thing.
pub fn target(installed: &Installed) -> PathBuf {
    let game: Vec<String> = installed.files.iter().filter(|f| !crate::downloads::layout::beside_the_game(f)).cloned().collect();
    // A folder of nothing but extras was fetched whole (see `downloads::fetch`): its files are the game.
    let files = if game.is_empty() { &installed.files } else { &game };
    let pick = files
        .iter()
        .find(|f| extension(f) == "m3u")
        .or_else(|| (files.len() == 1).then(|| &files[0]))
        .or_else(|| files.iter().find(|f| PLAYABLE.contains(&extension(f).as_str())))
        .or_else(|| {
            // A cartridge rom among notes, pictures or a patch: the one file that can be the game, at
            // the top or in the rom's own folder. Never that folder — RetroDECK would look in it for
            // a file named like the folder, and find none.
            let mut games = files.iter().filter(|f| f.matches('/').count() <= 1 && !crate::downloads::detect::not_a_game(f));
            games.next().filter(|_| games.next().is_none())
        });
    if let Some(file) = pick {
        return installed.dir.join(file);
    }
    // Every file under one folder of the rom's own: that folder is the game.
    let first = files.first().and_then(|f| f.split_once('/')).map(|(dir, _)| dir);
    match first {
        Some(dir) if files.iter().all(|f| f.starts_with(&format!("{dir}/"))) => installed.dir.join(dir),
        _ => installed.dir.join(files.first().map(String::as_str).unwrap_or_default()),
    }
}

/// A command line as the words a program is started with — split at spaces, except inside quotes,
/// and with a backslash taking the next character as it is: what the Settings field holds, written
/// the way a shell would take it.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"') | None, '\\') => {
                let next = chars.next().ok_or("the command ends with a lone backslash")?;
                word.get_or_insert_default().push(next);
            }
            (Some(_), c) => word.get_or_insert_default().push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return Err("a quote in the command is never closed".into());
    }
    words.extend(word);
    Ok(words)
}

/// Words back into a line [`split_command`] reads the same: quoted where they have to be.
pub fn join_command(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            if !w.is_empty() && !w.chars().any(|c| c.is_whitespace() || "\"'\\".contains(c)) {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What will run a game, as far as its saves are concerned.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Emulator {
    /// RetroArch, running this libretro core (`snes9x`, `swanstation`).
    Core(String),
    /// RetroArch, with no way to tell which core. Fine for a RetroArch that does not file saves by
    /// core; one that does gets its existing saves synced and no new one pulled.
    RetroArch,
    /// Something else, by the name ES-DE gives it. Its saves are not RetroArch's, so not synced.
    Other(String),
}

impl Emulator {
    /// Whether this build's save sync covers it, and with which core.
    pub fn syncs(&self) -> Option<Option<&str>> {
        match self {
            Emulator::Core(core) => Some(Some(core)),
            Emulator::RetroArch => Some(None),
            Emulator::Other(_) => None,
        }
    }
}

/// What starts games.
#[derive(Debug, Clone)]
pub enum Launcher {
    RetroDeck(RetroDeck),
    /// The player's own command (`launch_command`): argv words, with `{rom}` and `{system}` filled in.
    Custom(Vec<String>),
}

impl Launcher {
    /// What starts a game of ES-DE `system`: the player's command for that system, else their command
    /// for every game, else RetroDECK if it is installed.
    pub fn find(settings: &Settings, system: &str) -> Result<Self, String> {
        let own = settings.system_commands.get(system).into_iter().chain(&settings.launch_command);
        if let Some(command) = own.into_iter().find(|c| !c.is_empty()) {
            return Ok(Launcher::Custom(command.clone()));
        }
        RetroDeck::detect().map(Launcher::RetroDeck).ok_or_else(|| NO_EMULATOR.to_string())
    }

    /// The argv that plays `rom` as `system`. RetroDECK chooses the command itself, unless it would
    /// choose one that cannot run the game ([`in_place_of_first`]): then it is handed the one that can.
    pub fn command(&self, system: &str, game: &Path) -> Vec<String> {
        let rom = game.to_string_lossy();
        match self {
            Launcher::RetroDeck(rd) => {
                let mut command = ["flatpak", "run", RETRODECK].map(String::from).to_vec();
                if let Some(text) = rd.instead(system, game) {
                    command.extend(["-e".into(), text]);
                }
                command.extend(["-s".into(), system.into(), for_retrodeck(game).to_string_lossy().into_owned()]);
                command
            }
            Launcher::Custom(words) => {
                let mut command: Vec<String> =
                    words.iter().map(|w| w.replace("{rom}", &rom).replace("{system}", system)).collect();
                // A command that never says where the game goes gets it last, which is where every
                // emulator's command line takes it.
                if !words.iter().any(|w| w.contains("{rom}")) {
                    command.push(rom.into_owned());
                }
                command
            }
        }
    }

    /// The argv that plays `rom` as `system` in RetroArch with `extra` on its command line: a state
    /// slot to boot into, a config to lay over its own. None when what runs it is not RetroArch,
    /// which has no such switches.
    ///
    /// RetroDECK passes nothing on to the emulator, but takes the command itself with `-e`, filling
    /// in its placeholders as it does for its own choice and running it through the shell: so the
    /// command it would choose, read from the same tables, goes with `extra` quoted on the end.
    pub fn retroarch_command(&self, system: &str, rom: &Path, extra: &[String]) -> Option<Vec<String>> {
        match self {
            Launcher::RetroDeck(rd) => {
                let text = rd.command_text(system, rom).filter(|t| t.contains("%EMULATOR_RETROARCH%"))?;
                let rom = rom.to_string_lossy().into_owned();
                let text = format!("{text} {}", join_command(extra));
                Some(["flatpak", "run", RETRODECK, "-e", &text, "-s", system, &rom].map(String::from).to_vec())
            }
            Launcher::Custom(_) => {
                // RetroArch, by a core or not — nothing else boots into a slot.
                self.emulator(system, rom).syncs()?;
                let mut command = self.command(system, rom);
                command.extend(extra.iter().cloned());
                Some(command)
            }
        }
    }

    /// Which emulator `command` will run `rom` with.
    pub fn emulator(&self, system: &str, rom: &Path) -> Emulator {
        match self {
            Launcher::RetroDeck(rd) => rd.emulator(system, rom),
            Launcher::Custom(_) => {
                // Read filled in, so a core named by `{system}` is the system's own.
                let words = self.command(system, rom);
                if let Some(core) = crate::cli::core_path(&words).as_deref().and_then(crate::cli::core_id) {
                    return Emulator::Core(core);
                }
                let program = program_name(&words);
                if program.to_lowercase().contains("retroarch") {
                    Emulator::RetroArch
                } else {
                    Emulator::Other(program)
                }
            }
        }
    }
}

/// What a command runs, by name: its program's file name — or, for `flatpak run`, the app it runs
/// (`net.pcsx2.PCSX2`), since `flatpak` says nothing about which emulator it is.
fn program_name(words: &[String]) -> String {
    let file_name = |word: &str| Path::new(word).file_name().map_or_else(|| word.to_string(), |n| n.to_string_lossy().into_owned());
    let program = words.first().map(String::as_str).unwrap_or_default();
    if file_name(program) == "flatpak"
        && let Some(run) = words.iter().position(|w| w == "run")
        && let Some(app) = words[run + 1..].iter().find(|w| !w.starts_with('-'))
    {
        return app.clone();
    }
    file_name(program)
}

/// What a Flatpak is granted of a path for one launch — see [`with_access`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    /// The game's folder: an emulator may keep its save beside the game, and one the Flatpak already
    /// writes to must not turn read-only for the launch, which a narrower grant would make it.
    ReadWrite,
}

/// `command` with `paths` granted, when what it runs is a Flatpak.
///
/// A Flatpak sees only what its manifest grants, and some grant nothing of the host at all: handed a
/// game on another drive, such an emulator cannot open it, and says so as a format it does not
/// support. So the launch runs it through `flatpak run --filesystem=<path>`, for this launch alone.
/// A Flatpak's exported launcher (`…/flatpak/exports/bin/<app id>`, what *Emulator for each system*
/// fills in) is a script of one `flatpak run` line, which is read out of it and run instead, its
/// branch and architecture kept. Anything else runs as it is, as does a launcher that cannot be read.
fn with_access(command: Vec<String>, paths: &[(PathBuf, Access)]) -> Vec<String> {
    let Some(program) = command.first() else { return command };
    // `flatpak run`, then what follows it: the app, the options it runs with and the game's.
    let (run, rest): (Vec<String>, Vec<String>) = if is_flatpak(program) && command.get(1).is_some_and(|w| w == "run") {
        (command[..2].to_vec(), command[2..].to_vec())
    } else if Path::new(program).parent().is_some_and(|dir| dir.ends_with("flatpak/exports/bin")) {
        let Some(mut run) = exported(program) else { return command };
        let rest = run.split_off(2).into_iter().chain(command[1..].iter().cloned()).collect();
        (run, rest)
    } else {
        return command;
    };
    let grant = |(path, access): &(PathBuf, Access)| {
        format!("--filesystem={}:{}", path.display(), if *access == Access::Read { "ro" } else { "rw" })
    };
    run.into_iter().chain(paths.iter().map(grant)).chain(rest).collect()
}

fn is_flatpak(word: &str) -> bool {
    Path::new(word).file_name().is_some_and(|n| n == "flatpak")
}

/// The `flatpak run` line a Flatpak's exported launcher runs, up to where the launcher's own
/// arguments go: `/usr/bin/flatpak run --branch=stable --arch=x86_64 <app id>`.
fn exported(launcher: &str) -> Option<Vec<String>> {
    let script = fs::read_to_string(launcher).ok()?;
    let line = script.lines().filter_map(|l| split_command(l).ok()).find(|words| words.iter().any(|w| is_flatpak(w)))?;
    let run: Vec<String> = line.into_iter().skip_while(|w| !is_flatpak(w)).take_while(|w| w != "$@" && !w.starts_with("@@")).collect();
    (run.len() > 2 && run[1] == "run").then_some(run)
}

/// A command for an emulator picked by its file — a program, an AppImage, or a Flatpak's launcher
/// in `exports/bin`: the options ES-DE starts it with, for those Deckhand knows, and `{rom}` last.
/// RetroArch's needs a core as well, which is the player's to add.
pub fn preset(program: &Path) -> Vec<String> {
    let name = program.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    // ES-DE's own options for each, from its `es_systems.xml`.
    const OPTIONS: &[(&str, &[&str])] = &[
        ("pcsx2", &["-batch"]),
        ("duckstation", &["-batch"]),
        ("primehack", &["-b", "-e"]),
        ("dolphin", &["-b", "-e"]),
        ("rpcs3", &["--no-gui"]),
        ("cemu", &["-g"]),
        ("melonds", &["-f"]),
        ("gopher64", &["-f"]),
        ("rosalie", &["--nogui", "-q"]),
        ("mupen64plus", &["--nogui"]),
        ("simple64", &["--nogui"]),
        ("bsnes", &["--fullscreen"]),
        ("xemu", &["-dvd_path"]),
        ("eden", &["-f", "-g"]),
    ];
    let options = OPTIONS.iter().find(|(key, _)| name.contains(key)).map_or(&[][..], |(_, options)| *options);
    std::iter::once(program.to_string_lossy().into_owned())
        .chain(options.iter().map(|o| o.to_string()))
        .chain(["{rom}".to_string()])
        .collect()
}

/// A folder RetroDECK's own config, `retrodeck.json`, names under `paths` — where its storage is.
fn retrodeck_path(key: &str) -> Option<PathBuf> {
    let config = dirs::home_dir()?.join(".var/app").join(RETRODECK).join("config/retrodeck/retrodeck.json");
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(config).ok()?).ok()?;
    json.pointer(&format!("/paths/{key}")).and_then(|v| v.as_str()).filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// RetroDECK's roms folder, the one ES-DE scans: where downloads go by default when RetroDECK is here.
pub fn retrodeck_roms() -> Option<PathBuf> {
    retrodeck_path("roms_path").or_else(|| retrodeck_path("rd_home_path").map(|home| home.join("roms")))
}

/// A RetroDECK install, as far as choosing an emulator goes: ES-DE's data folder, holding the
/// gamelists that carry per-game and per-system choices, and the `es_systems.xml` tables that turn a
/// choice into a command, most specific first.
#[derive(Debug, Clone)]
pub struct RetroDeck {
    es_de: PathBuf,
    systems: Vec<PathBuf>,
}

impl RetroDeck {
    /// ES-DE's data folder: gamelists, custom systems, scripts.
    pub fn es_de(&self) -> &Path {
        &self.es_de
    }

    /// RetroDECK on this machine, found by the config it writes — `retrodeck.json`, which also says
    /// where its storage is.
    pub fn detect() -> Option<Self> {
        let home = dirs::home_dir()?;
        let rd_home = retrodeck_path("rd_home_path")?;
        let es_de = rd_home.join("ES-DE");
        // The user's own table, then the one RetroDECK ships, which is readable from outside the
        // Flatpak. System-wide first, since that is how a Deck has it; a per-user install is legal.
        let bundled = ["/var/lib/flatpak/app".into(), home.join(".local/share/flatpak/app")]
            .into_iter()
            .map(|root: PathBuf| {
                root.join(RETRODECK)
                    .join("current/active/files/retrodeck/components/es-de/share/es-de/resources/systems/linux/es_systems.xml")
            });
        let systems = std::iter::once(es_de.join("custom_systems/es_systems.xml")).chain(bundled).collect();
        Some(Self { es_de, systems })
    }

    /// The emulator RetroDECK will pick for `rom` as `system`, decided the way its `run_game.sh`
    /// decides it: the game's own `<altemulator>` in ES-DE's gamelist, else the system's
    /// `<alternativeEmulator>`, else the system's first command — or, where that one cannot run the
    /// game, the one that can, which RetroDECK is then told ([`in_place_of_first`]). The label picks
    /// a `<command>` out of the first table that defines the system at all — tables are searched per
    /// system rather than first-file-wins, because RetroDECK ships a custom table that is all
    /// comments.
    pub fn emulator(&self, system: &str, rom: &Path) -> Emulator {
        let Chosen { command, label, .. } = self.chosen(system, rom);
        match (command, label) {
            (Some((label, text)), _) => from_command(&label, &text),
            // No table could be read. ES-DE calls every standalone command "<Name> (Standalone)" and
            // none of its RetroArch ones that, so the label alone is a fair answer; no label at all
            // is the system's default, which is a core for most systems.
            (None, Some(label)) if label.to_lowercase().contains("(standalone)") => Emulator::Other(label),
            (None, _) => Emulator::RetroArch,
        }
    }

    /// The ES-DE command RetroDECK will run `rom` with, as written in its table —
    /// `%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so %ROM%` — chosen as [`Self::emulator`]
    /// chooses it. What RetroDECK's `-e` takes, filled in by RetroDECK itself.
    pub fn command_text(&self, system: &str, rom: &Path) -> Option<String> {
        self.chosen(system, rom).command.map(|(_, text)| text)
    }

    /// The command to hand RetroDECK with `-e` for `rom`, where it would take another itself — see
    /// [`in_place_of_first`]. None where its own choice stands.
    pub fn instead(&self, system: &str, rom: &Path) -> Option<String> {
        let chosen = self.chosen(system, rom);
        chosen.replaced.then_some(chosen.command).flatten().map(|(_, text)| text)
    }

    /// The command chosen for `rom`, and the label asked for.
    fn chosen(&self, system: &str, rom: &Path) -> Chosen {
        let gamelist = fs::read_to_string(self.es_de.join("gamelists").join(system).join("gamelist.xml")).unwrap_or_default();
        let gamelist = strip_comments(&gamelist);
        let name = rom.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let label = game_label(&gamelist, &name).or_else(|| system_label(&gamelist));

        let commands = self
            .systems
            .iter()
            .filter_map(|file| fs::read_to_string(file).ok())
            .map(|xml| commands_for(&xml, system))
            .find(|commands| !commands.is_empty())
            .unwrap_or_default();
        let instead = label.is_none().then(|| in_place_of_first(&commands, rom)).flatten();
        let command = match &label {
            Some(label) => commands.iter().find(|(l, _)| l == label),
            None => instead.or(commands.first()),
        };
        Chosen { command: command.cloned(), label, replaced: instead.is_some() }
    }
}

/// A command out of ES-DE's tables, as [`RetroDeck::chosen`] chooses it.
struct Chosen {
    /// Its label and its text.
    command: Option<(String, String)>,
    /// The label ES-DE's gamelist asks for, if it asks for one.
    label: Option<String>,
    /// Chosen in place of the one RetroDECK would take itself — see [`in_place_of_first`].
    replaced: bool,
}

/// Where a PS3 game's tree keeps the program RPCS3 boots.
const EBOOT: &str = "PS3_GAME/USRDIR/EBOOT.BIN";

/// The command to run `rom` with in place of its system's first, where that one would run it as a
/// script and it is none. RetroDECK lists *RPCS3 Shortcut*, `%EMULATOR_OS-SHELL% %ROM%`, first for
/// PS3, for the `.desktop` files RPCS3 writes, and with no emulator chosen in ES-DE it hands that
/// command a disc image all the same, which then runs as `/bin/sh <game>.iso`. The one whose label
/// names what `rom` is goes instead: *RPCS3 ISO* for an `.iso`, *RPCS3 Directory* for a game's
/// folder or its `EBOOT.BIN`. None where the first is fine, or no label says.
fn in_place_of_first<'a>(commands: &'a [(String, String)], rom: &Path) -> Option<&'a (String, String)> {
    let (_, first) = commands.first()?;
    let kind = if rom.is_dir() || rom.ends_with(EBOOT) { "directory".to_string() } else { extension(&rom.to_string_lossy()) };
    if !first.contains("%EMULATOR_OS-SHELL%") || ["desktop", "sh", "appimage"].contains(&kind.as_str()) {
        return None;
    }
    commands.iter().find(|(label, _)| label.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|word| word == kind))
}

/// The file RetroDECK is handed for `rom`. A PS3 game's folder goes as its `EBOOT.BIN`, which RPCS3
/// boots: RetroDECK takes any folder for an ES-DE "directory as a file" and looks inside it for a
/// file named like the folder, which a PS3 game's tree does not have.
fn for_retrodeck(rom: &Path) -> PathBuf {
    let eboot = rom.join(EBOOT);
    if eboot.is_file() { eboot } else { rom.to_path_buf() }
}

/// An ES-DE command read for what it runs: `%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so
/// %ROM%` is RetroArch with `snes9x`.
fn from_command(label: &str, text: &str) -> Emulator {
    if !text.contains("%EMULATOR_RETROARCH%") {
        return Emulator::Other(label.to_string());
    }
    let core = text.split_once("%CORE_RETROARCH%/").and_then(|(_, rest)| rest.split_once("_libretro")).map(|(core, _)| core);
    match core {
        Some(core) if !core.is_empty() => Emulator::Core(core.to_lowercase()),
        _ => Emulator::RetroArch,
    }
}

/// Every `<command label="…">` of `system` in one `es_systems.xml`, in ES-DE's order.
fn commands_for(xml: &str, system: &str) -> Vec<(String, String)> {
    let xml = strip_comments(xml);
    blocks(&xml, "system")
        .find(|block| element(block, "name").as_deref() == Some(system))
        .map(|block| {
            let mut out = Vec::new();
            let mut rest = block;
            while let Some(start) = rest.find("<command") {
                rest = &rest[start..];
                let Some(open_end) = rest.find('>') else { break };
                let open = &rest[..open_end];
                let Some(close) = rest.find("</command>") else { break };
                let label = attribute(open, "label").unwrap_or_default();
                out.push((label, unescape(rest[open_end + 1..close].trim())));
                rest = &rest[close..];
            }
            out
        })
        .unwrap_or_default()
}

/// The `<altemulator>` of the gamelist entry whose path is `./<name>`.
fn game_label(gamelist: &str, name: &str) -> Option<String> {
    blocks(gamelist, "game")
        .find(|game| element(game, "path").is_some_and(|p| p.rsplit('/').next() == Some(name)))
        .and_then(|game| element(game, "altemulator"))
        .filter(|l| !l.is_empty())
}

/// The `<label>` inside a gamelist's `<alternativeEmulator>` header.
fn system_label(gamelist: &str) -> Option<String> {
    blocks(gamelist, "alternativeEmulator").next().and_then(|header| element(header, "label")).filter(|l| !l.is_empty())
}

// ES-DE's files are read with a few string scans rather than an XML parser. They are small and
// regular, and a gamelist with an emulator override is not well-formed XML anyway — it has two
// top-level elements, which a strict parser rejects outright.

/// The insides of every `<tag>…</tag>`, in order.
fn blocks<'a>(xml: &'a str, tag: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut rest = xml;
    std::iter::from_fn(move || {
        let start = rest.find(&open)? + open.len();
        let end = rest[start..].find(&close)? + start;
        let block = &rest[start..end];
        rest = &rest[end + close.len()..];
        Some(block)
    })
}

/// The text of the first `<tag>…</tag>` in `xml`, trimmed and unescaped.
fn element(xml: &str, tag: &str) -> Option<String> {
    blocks(xml, tag).next().map(|t| unescape(t.trim()))
}

/// `name="value"` from an opening tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[start..].find('"')? + start;
    Some(unescape(&tag[start..end]))
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn strip_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// One downloaded game, worked out for playing: the file, the system, what runs it.
#[derive(Debug, Clone)]
pub struct Plan {
    pub rom_id: i64,
    pub title: String,
    pub rom: PathBuf,
    pub system: String,
    pub platform_id: Option<i64>,
    /// Why there is no launcher, when there is none. A download's save pull goes ahead without
    /// one, assuming RetroArch, since that is all this build can sync anyway.
    pub launcher: Result<Launcher, String>,
    pub emulator: Emulator,
    /// Which rom the record was downloaded from, and what its files are called — what a sync holds
    /// the id to before it moves the game's saves under it. See [`Installed::is`].
    pub stamp: Option<Stamp>,
    pub names: Vec<String>,
}

impl Plan {
    /// What syncing its saves needs, as a marker keeps it — see [`crate::playing::Game`].
    pub fn game(&self) -> crate::playing::Game {
        crate::playing::Game {
            title: self.title.clone(),
            rom: self.rom.clone(),
            system: self.system.clone(),
            emulator: self.emulator.clone(),
            stamp: self.stamp.clone(),
        }
    }

    /// Reads ES-DE's tables, so off the UI thread.
    pub fn new(settings: &Settings, installed: &Installed) -> Self {
        let rom = target(installed);
        let system = es_system(&installed.system_id);
        let launcher = Launcher::find(settings, &system);
        let emulator = match &launcher {
            Ok(launcher) => launcher.emulator(&system, &rom),
            Err(_) => Emulator::RetroArch,
        };
        Plan {
            rom_id: installed.rom_id,
            title: installed.title.clone(),
            rom,
            system,
            platform_id: installed.platform_id,
            launcher,
            emulator,
            stamp: installed.stamp.clone(),
            names: installed.names(),
        }
    }

    /// Whether `now` — what RomM holds under the game's id — is the rom it was downloaded from.
    pub fn is(&self, now: &Stamp) -> bool {
        crate::downloads::holds(self.stamp.as_ref(), &self.names, now)
    }
}

/// What the BIOS and save steps need from the app: the pairing, and where its own records live.
#[derive(Clone)]
pub struct Context {
    pub client: RommClient,
    pub connection: Option<Connection>,
    pub retroarch_config: Option<PathBuf>,
    pub store: PathBuf,
}

/// Steps 1 and 2: the platform's BIOS, then the game's save down from RomM. What a launch does first
/// and what a finished download does. Answers what is worth telling the user — usually nothing.
pub async fn ready(ctx: &Context, plan: &Plan) -> Vec<String> {
    let Some(conn) = &ctx.connection else { return Vec::new() };
    let mut notes = Vec::new();

    // Only the platform's id finds its firmware, and a record written before it was kept has none,
    // so that costs one request for the rom's own.
    let platform_id = match plan.platform_id {
        Some(id) => Some(id),
        None => ctx.client.fetch_rom_detail(conn, plan.rom_id).await.ok().and_then(|d| d.platform_id),
    };
    // A RetroArch without a BIOS folder, or a RomM with no firmware, is the common case and not news.
    let bios = sync::retroarch::bios_dir(ctx.retroarch_config.as_deref());
    if let (Some(platform_id), Ok(dir)) = (platform_id, &bios)
        && let Ok(fetched) = crate::bios::sync(&ctx.client, conn, platform_id, dir).await
        && !fetched.is_empty()
    {
        notes.push(BIOS_SYNCED.into());
    }
    // A Switch emulator of its own takes RomM's keys and firmware into its own folders, from where
    // the BIOS sync put them — or a folder of Deckhand's, with no RetroArch to have one. Before the
    // save sync, which reads the keys to tell the game.
    if let (Some(platform_id), Some(target)) = (platform_id, crate::switch_firmware::target(plan)) {
        let source = match bios {
            Ok(dir) => dir,
            Err(_) => {
                let own = ctx.store.join("firmware/switch");
                let _ = crate::bios::sync(&ctx.client, conn, platform_id, &own).await;
                own
            }
        };
        if let Ok(firmware) = ctx.client.fetch_firmware(conn, platform_id).await {
            let store = ctx.store.clone();
            // Unknown reads as running: a firmware waits for later rather than going in under an
            // emulator that has it open.
            let put = tokio::task::spawn_blocking(move || {
                crate::switch_firmware::install(&target, &source, &firmware, &store, |name| crate::processes::running(name).unwrap_or(true))
            });
            notes.extend(put.await.unwrap_or_default());
        }
    }

    notes.extend(sync_any(ctx, conn, plan.rom_id, &plan.game(), true, Vec::new()).await.notes);
    said(notes)
}

/// Step 4: the save the session wrote, up to RomM, with the play session it ends.
pub async fn settle(ctx: &Context, plan: &Plan, play: Option<PlaySession>) -> Synced {
    match &ctx.connection {
        Some(conn) => sync_any(ctx, conn, plan.rom_id, &plan.game(), false, play.into_iter().collect()).await,
        None => Synced { notes: Vec::new(), recorded: 0, whole: true, report: None },
    }
}

/// What a sync of one game's saves did.
pub struct Synced {
    /// What is worth saying.
    pub notes: Vec<String>,
    /// How many of the play sessions handed to it RomM took with the sync.
    pub recorded: usize,
    /// Whether every save that could move did — nothing failed, and RomM was there to ask. A game
    /// whose saves this build does not sync has nothing to fail.
    pub whole: bool,
    /// What the sync did, when it ran.
    pub report: Option<Box<Report>>,
}

/// The game's saves, whichever emulator keeps them: RetroArch's battery saves and states, or a
/// standalone emulator's own. `before` says so once, before play, for one this build cannot sync.
async fn sync_any(ctx: &Context, conn: &Connection, rom_id: i64, game: &crate::playing::Game, before: bool, plays: Vec<PlaySession>) -> Synced {
    if let Some(core) = game.emulator.syncs() {
        return sync_save(ctx, conn, rom_id, game, core, plays).await;
    }
    let Emulator::Other(name) = &game.emulator else { return Synced { notes: Vec::new(), recorded: 0, whole: true, report: None } };
    let Some(emulator) = sync::standalone::Emulator::for_launch(&game.system, name) else {
        let notes = if before { vec![format!("it runs on {name}, whose saves this build doesn't sync")] } else { Vec::new() };
        return Synced { notes, recorded: 0, whole: true, report: None };
    };
    let result = sync::standalone::sync(&ctx.client, conn, &ctx.store, emulator, &game.rom, &game.title, rom_id, game.stamp.as_ref(), plays).await;
    reported(result, &format!("{name} saves not synced"))
}

async fn sync_save(ctx: &Context, conn: &Connection, rom_id: i64, game: &crate::playing::Game, core: Option<&str>, plays: Vec<PlaySession>) -> Synced {
    let install = match sync::retroarch::locate(ctx.retroarch_config.as_deref()) {
        Ok(install) => install,
        Err(why) => return Synced { notes: vec![format!("saves not synced: {why}")], recorded: 0, whole: false, report: None },
    };
    let result = sync::sync_rom(&ctx.client, conn, &install, &ctx.store, &game.rom, &game.system, core, rom_id, game.stamp.as_ref(), plays).await;
    reported(result, "saves not synced")
}

/// A sync's result as [`Synced`], `failed` heading the reason it gave up.
fn reported(result: Result<Report, String>, failed: &str) -> Synced {
    match result {
        Ok(report) => Synced {
            notes: save_notes(&report),
            recorded: report.plays_recorded,
            whole: report.failed.is_empty(),
            report: Some(Box::new(report)),
        },
        Err(why) => Synced { notes: vec![format!("{failed}: {why}")], recorded: 0, whole: false, report: None },
    }
}

/// Finish every game whose closing was never seen through — Deckhand ended along with it, or RomM
/// was out of reach as it closed (see [`crate::playing`]): its saves synced, and its time played sent
/// with them. One RomM still cannot take stays for the next time. Answers how many were finished.
pub async fn finish_left(ctx: &Context) -> usize {
    let left = crate::playing::left(&ctx.store);
    // RetroArch's own setting first, RomM or not: putting it back needs nothing else.
    for game in &left {
        if let Some(laid) = game.auto_load() {
            laid.put_back();
            game.auto_load_put_back();
        }
    }
    let Some(conn) = &ctx.connection else { return 0 };
    let mut finished = 0;
    for left in left {
        let Some(rom_id) = left.rom_id() else {
            left.done();
            continue;
        };
        let play = left.session();
        let synced = match left.game() {
            Some(game) => sync_any(ctx, conn, rom_id, game, false, play.clone().into_iter().collect()).await,
            None => Synced { notes: Vec::new(), recorded: 0, whole: true, report: None },
        };
        let taken = synced.recorded > 0 || match &play {
            Some(play) => ctx.client.post_play_session(conn, play).await.is_ok(),
            None => true,
        };
        match (synced.whole, taken) {
            (true, true) => {
                left.done();
                finished += 1;
            }
            (false, true) => left.sent(),
            _ => {}
        }
    }
    finished
}

/// What a note says of a BIOS fetched, whichever files it was.
const BIOS_SYNCED: &str = "BIOS synced";
/// What a note says of saves or states moved, either way, however many and whichever they were.
const SAVES_SYNCED: &str = "saves synced";

/// A sync's report, cut to what a banner should say: that saves moved, and what went wrong — never
/// "in sync", and never which saves: the game they are is the banner's own subject.
fn save_notes(report: &Report) -> Vec<String> {
    let mut notes = Vec::new();
    // A save changed on both sides is one that moved too: one side of it is now on the other. The
    // player chose which, or it was the newer.
    if !(report.pulled.is_empty() && report.pushed.is_empty() && report.conflicts.is_empty()) {
        notes.push(SAVES_SYNCED.into());
    }
    notes.extend(report.other_cores.iter().cloned());
    notes.extend(report.removed.iter().cloned());
    notes.extend(report.failed.iter().map(|f| format!("save sync failed: {f}")));
    notes
}

/// `notes` with the BIOS and the saves said once, in one line where the first of them was: "BIOS
/// and saves synced" for a launch that fetched a BIOS, pulled a save before play and pushed one
/// after.
fn said(notes: Vec<String>) -> Vec<String> {
    let bios = notes.iter().any(|n| n == BIOS_SYNCED);
    let saves = notes.iter().any(|n| n == SAVES_SYNCED);
    let line = match (bios, saves) {
        (true, true) => "BIOS and saves synced",
        (true, false) => BIOS_SYNCED,
        (false, true) => SAVES_SYNCED,
        (false, false) => return notes,
    };
    let mut out = Vec::with_capacity(notes.len());
    for note in notes {
        if note != BIOS_SYNCED && note != SAVES_SYNCED {
            out.push(note);
        } else if !out.iter().any(|n| n == line) {
            out.push(line.into());
        }
    }
    out
}

/// What a launch sends back.
#[derive(Debug, Clone)]
pub enum Event {
    /// The BIOS and the save are in place and the game is starting.
    Running(i64),
    /// The game has closed, and its save is on its way up to RomM.
    Saving(i64),
    /// It is over — the game closed and its save went up, or it never started, and why.
    Ended(i64, Result<Session, String>),
    /// A save changed both here and on RomM: which to keep. The sync waits on the answer.
    Conflict(i64, sync::Conflict, Reply),
}

/// Where a [`Event::Conflict`]'s answer goes — sent once; an event is cloned, an answer is not.
#[derive(Clone)]
pub struct Reply(std::sync::Arc<std::sync::Mutex<Option<futures::channel::oneshot::Sender<sync::Choice>>>>);

impl Reply {
    pub fn new(sender: futures::channel::oneshot::Sender<sync::Choice>) -> Self {
        Reply(std::sync::Arc::new(std::sync::Mutex::new(Some(sender))))
    }

    /// Answer. A second answer, or one nobody waits for any more, goes nowhere.
    pub fn send(&self, choice: sync::Choice) {
        if let Some(sender) = self.0.lock().ok().and_then(|mut s| s.take()) {
            let _ = sender.send(choice);
        }
    }
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reply")
    }
}

/// A finished play session.
#[derive(Debug, Clone)]
pub struct Session {
    pub exit: Exit,
    /// When the game closed, if it ran long enough to count as play — see [`MIN_SESSION`].
    pub played_at: Option<String>,
    /// What the steps around it did that is worth saying — a BIOS fetched, a save moved.
    pub notes: Vec<String>,
    /// What the sync after it did, when one ran: where the window's saves icon stands from then on.
    pub report: Option<Box<Report>>,
}

/// Where a launch has got to, for whoever started it to show — see [`play`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Step 3: the game is starting.
    Running,
    /// Step 4: the game has closed, and its save is going up to RomM.
    Saving,
}

/// The whole of a launch, steps 1 to 4. `tell` hears [`Stage::Running`] once, just before the game
/// starts, and [`Stage::Saving`] once after it closes — only when its save goes anywhere.
///
/// An error is a game that never started: no launcher, files gone, a command that would not run.
/// Anything that goes wrong around a game that did start is a note instead.
pub async fn play(ctx: Context, settings: Settings, installed: Installed, stop: Stop, mut tell: impl FnMut(Stage)) -> Result<Session, String> {
    let resume = settings.resume_state;
    let plan = tokio::task::spawn_blocking(move || Plan::new(&settings, &installed))
        .await
        .map_err(|e| e.to_string())?;
    let launcher = plan.launcher.clone()?;
    if !plan.rom.exists() {
        return Err(format!("{} is gone — remove the game and download it again", plan.rom.display()));
    }
    // The game's id held to RomM before anything goes there under it — a Steam shortcut can start
    // a game long after the library was last read, and a RomM rebuilt since keeps another game
    // there. Then the game plays and RomM hears nothing of it: not its save, not that it is being
    // played, not for how long, all of which would land on that other game.
    let mut notes = Vec::new();
    let ctx = match &ctx.connection {
        Some(conn) => match sync::still_holds(&ctx.client, conn, plan.rom_id, &|now| plan.is(now)).await {
            Ok(()) => ctx,
            Err(why) => {
                notes.push(why);
                Context { connection: None, ..ctx }
            }
        },
        None => ctx,
    };
    // A game an earlier launch never saw close — Deckhand ended along with it, or RomM out of reach
    // as it closed. Before this one's marker can take the place of the game's own.
    finish_left(&ctx).await;
    notes.extend(ready(&ctx, &plan).await);
    // Into the newest state when asked to, from the start when not, whatever RetroArch's own Auto
    // Load State says — see [`start_in`]. The save sync has just brought down any newer state from
    // elsewhere.
    let here = states_here(&ctx, &plan);
    let start = here.as_ref().map_or_else(Start::default, |h| start_in(resume, h.newest, h.auto, h.install.loads_auto_state()));
    let laid = start.auto_load.zip(here.as_ref()).and_then(|(on, h)| AutoLoad::lay(&ctx.store, h.install.config(), on));
    let mut extra = Vec::new();
    if let Some(slot) = start.slot {
        extra.extend(["--entryslot".to_string(), slot.to_string()]);
    }
    if laid.is_some() {
        extra.extend(["--appendconfig".to_string(), auto_load_file(&ctx.store).to_string_lossy().into_owned()]);
    }
    let special = (!extra.is_empty()).then(|| launcher.retroarch_command(&plan.system, &plan.rom, &extra)).flatten();
    // Laid over RetroArch's own only when the command carries it.
    let laid = laid.filter(|_| special.is_some());
    let resumed = match start {
        Start { slot: Some(_), .. } => special.is_some(),
        Start { auto_load: Some(on), .. } => on && laid.is_some(),
        // RetroArch starts in the auto slot's state by itself.
        Start { .. } => resume && here.as_ref().is_some_and(|h| h.newest == Some(Resume::Auto)),
    };
    if resumed {
        notes.push("resumed from the newest save state".into());
    }
    let mut command = special.unwrap_or_else(|| launcher.command(&plan.system, &plan.rom));
    // A Flatpak of the player's own sees only what it was granted: the game's folder, and the config
    // laid over RetroArch's, for this launch — see [`with_access`]. RetroDECK sees every folder.
    if matches!(launcher, Launcher::Custom(_)) {
        let folder = plan.rom.parent().map(|dir| (dir.to_path_buf(), Access::ReadWrite));
        let config = laid.is_some().then(|| (auto_load_file(&ctx.store), Access::Read));
        command = with_access(command, &folder.into_iter().chain(config).collect::<Vec<_>>());
    }
    // Asked to stop before the game is up — Steam closing the shortcut while its save came down — so
    // it is not started only to be closed again.
    if stop.asked() {
        return Err("asked to stop before it started".into());
    }
    tell(Stage::Running);
    // On disk for as long as the game runs, so what its closing owes outlives this process — see
    // `crate::playing`.
    let held = crate::playing::hold(&ctx.store, plan.rom_id, plan.game(), laid.clone());
    let start = held.as_ref().map_or_else(SystemTime::now, crate::playing::Held::start);
    let pid = Arc::new(AtomicU32::new(0));
    let game = tokio::task::spawn_blocking({
        let pid = pid.clone();
        move || run(&command, &pid)
    });
    let exit = match while_playing(&ctx, plan.rom_id, held.as_ref(), unless_stopped(&stop, &pid, GRACE, game)).await {
        Some(ran) => match ran.map_err(|e| e.to_string()).and_then(|ran| ran) {
            Ok(exit) => exit,
            Err(why) => {
                // It never started, so there is nothing its closing owes.
                if let Some(held) = held {
                    held.end();
                }
                return Err(why);
            }
        },
        None => {
            // Asked to stop, and the game did not close in time. Its saves and time played wait in its
            // marker for the next Deckhand to start.
            if let Some(held) = &held {
                held.touch();
            }
            let exit = Exit { success: false, code: None, last_line: None };
            return Ok(Session { exit, played_at: None, notes: vec![NOT_CLOSED.into()], report: None });
        }
    };
    // When it closed, for a sync this process does not live to finish.
    if let Some(held) = &held {
        held.touch();
    }
    if let Some(laid) = &laid {
        laid.put_back();
        if let Some(held) = &held {
            held.auto_load_put_back();
        }
    }
    let play = play_session(plan.rom_id, start, SystemTime::now());
    if ctx.connection.is_some() {
        tell(Stage::Saving);
    }
    let synced = settle(&ctx, &plan, play.clone()).await;
    notes.extend(synced.notes);
    let (played_at, owed) = record_session(&ctx, play, synced.recorded > 0, &mut notes).await;
    // Saves that did not all go, or a session RomM could not take, keep the marker, unheld, for the
    // next Deckhand to finish — saying so when it is only the saves, so the session never goes twice.
    match held {
        Some(held) if synced.whole && !owed => held.end(),
        Some(held) if !owed => held.sent(),
        _ => {}
    }
    Ok(Session { exit, played_at, notes: said(notes), report: synced.report })
}

/// What a launch says of a game asked to stop that did not close within [`GRACE`].
const NOT_CLOSED: &str = "asked to stop, it had not closed in time — its saves and playtime go to RomM the next time Deckhand starts";

/// A request for a running game to close, from whatever started it: a signal to `deckhand play`
/// (Steam's *Exit game*), or the window being asked to close. A clone is the same request.
#[derive(Debug, Clone)]
pub struct Stop(Arc<tokio::sync::watch::Sender<bool>>);

impl Default for Stop {
    fn default() -> Self {
        Stop(Arc::new(tokio::sync::watch::channel(false).0))
    }
}

impl Stop {
    pub fn ask(&self) {
        self.0.send_replace(true);
    }

    pub fn asked(&self) -> bool {
        *self.0.borrow()
    }

    async fn wait(&self) {
        let _ = self.0.subscribe().wait_for(|asked| *asked).await;
    }
}

/// Every SIGTERM, SIGHUP and SIGINT this process gets from the first poll on: how Steam's *Exit game*,
/// the session ending or a terminal's Ctrl+C asks it to stop. Listening replaces dying on the spot,
/// so whoever listens decides what stopping means.
pub fn stop_signals() -> impl futures::Stream<Item = ()> {
    use futures::StreamExt;
    // Set up on the first poll, which is inside the runtime the listeners need.
    futures::stream::once(async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let kinds = [SignalKind::terminate(), SignalKind::hangup(), SignalKind::interrupt()];
            let each = kinds
                .into_iter()
                .filter_map(|kind| signal(kind).ok())
                .map(|mut listener| futures::stream::poll_fn(move |cx| listener.poll_recv(cx)).boxed());
            futures::stream::select_all(each).boxed()
        }
        #[cfg(not(unix))]
        futures::stream::pending::<()>().boxed()
    })
    .flatten()
}

/// How long a game asked to stop is given to close: an emulator writing its saves as it quits takes
/// a moment, and whoever asked will not wait for ever. Past it, its closing is left to its marker.
const GRACE: Duration = Duration::from_secs(10);

/// How often a running game's processes are looked over, so a request to stop can still find them
/// once the process Deckhand started has gone — see [`crate::processes`].
const LOOK_OVER: Duration = Duration::from_secs(2);

/// `game` to its end — unless `stop` is asked while it runs. Then every process of the game is asked
/// to close (`pid`, the one Deckhand started, and what is under it) and given `grace` — [`GRACE`] —
/// to: the game has closed once all of them have, those its launcher left behind included. None when
/// they did not in time.
async fn unless_stopped<T>(stop: &Stop, pid: &AtomicU32, grace: Duration, game: impl Future<Output = T>) -> Option<T> {
    let mut game = std::pin::pin!(game);
    let mut tree = crate::processes::Tree::default();
    let mut looks = tokio::time::interval(LOOK_OVER);
    loop {
        tokio::select! {
            out = &mut game => return Some(out),
            _ = looks.tick() => tree.look(pid.load(Ordering::Relaxed)),
            () = stop.wait() => break,
        }
    }
    tree.look(pid.load(Ordering::Relaxed));
    tree.ask_to_close();
    let closed = async {
        let out = game.await;
        while !tree.gone() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        out
    };
    tokio::time::timeout(grace, closed).await.ok()
}

/// How often RomM hears that a game is still running: a third of [`HEARTBEAT_LASTS`], so one lost on
/// the way does not show the game as stopped.
const HEARTBEAT: Duration = Duration::from_secs(HEARTBEAT_LASTS.as_secs() / 3);

/// Await `game` while telling RomM, every [`HEARTBEAT`], that this device is playing `rom_id`, so its
/// web app shows the game as being played here — and, once it ends, that it has stopped. `held`, the
/// game's marker, is touched every [`crate::playing::TICK`] meanwhile, RomM or not.
///
/// Best effort, like every step around a game. A RomM that has no such thing (before 5.4), does not
/// know this device or does not let it say so is not asked again this session; any other failure is
/// one heartbeat missed, and the next goes as usual.
async fn while_playing<T>(ctx: &Context, rom_id: i64, held: Option<&crate::playing::Held>, game: impl Future<Output = T>) -> T {
    let ticks = async {
        let Some(held) = held else { return std::future::pending::<std::convert::Infallible>().await };
        loop {
            tokio::time::sleep(crate::playing::TICK).await;
            held.touch();
        }
    };
    let Some((conn, device)) = ctx.connection.as_ref().and_then(|c| Some((c, c.device_id.as_deref()?))) else {
        return tokio::select! {
            out = game => out,
            never = ticks => match never {},
        };
    };
    let beats = async {
        loop {
            if let Err(e) = ctx.client.heartbeat(conn, device, rom_id).await
                && matches!(e.status, 401 | 403 | 404 | 405 | 422)
            {
                return std::future::pending::<std::convert::Infallible>().await;
            }
            tokio::time::sleep(HEARTBEAT).await;
        }
    };
    let out = tokio::select! {
        out = game => out,
        never = beats => match never {},
        never = ticks => match never {},
    };
    let _ = ctx.client.clear_heartbeat(conn, device).await;
    out
}

/// Where a resumed game starts: in RetroArch's state slot N, or in the auto slot's state, which
/// RetroArch loads by itself on start when its Auto Load State is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    Slot(u32),
    Auto,
}

/// A game RetroArch runs, as far as starting it in a state goes.
struct Here {
    /// Its newest save state on this device that RetroArch can start in: any numbered slot
    /// `--entryslot` takes, or the auto slot.
    newest: Option<Resume>,
    /// It has a state in the auto slot, which RetroArch's Auto Load State would start it in.
    auto: bool,
    /// RetroArch as the game sees it, its overrides laid over.
    install: sync::retroarch::Install,
}

/// The game's save states on this device, if RetroArch runs it and they can be found.
fn states_here(ctx: &Context, plan: &Plan) -> Option<Here> {
    let core = plan.emulator.syncs()?;
    let install = sync::retroarch::locate(ctx.retroarch_config.as_deref()).ok()?.for_game(&plan.rom, core);
    let dir = install.game_states_dir(&plan.rom, core).ok()?;
    let stem = plan.rom.file_stem()?.to_string_lossy().into_owned();
    let mut states = sync::states::on_disk(&dir, &stem);
    states.sort_by_key(|(_, written)| std::cmp::Reverse(*written));
    let auto = states.iter().any(|(slot, _)| *slot == sync::states::Slot::Auto);
    let newest = states.into_iter().find_map(|(slot, _)| match slot {
        // `--entryslot` takes 0 to 999.
        sync::states::Slot::Number(n) => (n <= 999).then_some(Resume::Slot(n)),
        sync::states::Slot::Auto => Some(Resume::Auto),
    });
    Some(Here { newest, auto, install })
}

/// How RetroArch is started: in a numbered state slot, and with its own Auto Load State laid over
/// for this launch alone — None leaves it as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Start {
    slot: Option<u32>,
    auto_load: Option<bool>,
}

/// How a game RetroArch runs starts, so that *Resume from the newest state* (`resume`) decides
/// whether it resumes — not RetroArch's own Auto Load State (`loads_auto`), which on its own loads
/// the auto slot's state (`auto`) at every start, whatever Deckhand asked.
///
/// Resuming, a numbered slot is named, which RetroArch loads instead of the auto slot's state; the
/// auto slot's state, when it is the newest (`newest`), needs RetroArch's Auto Load State on. Not
/// resuming, it needs it off where there is an auto state to load. Either is laid over RetroArch's
/// own only where it differs, so a launch where nothing would load is left as it always was. Only
/// into a state that is there: RetroArch refuses to start at all over a missing one.
fn start_in(resume: bool, newest: Option<Resume>, auto: bool, loads_auto: bool) -> Start {
    match (resume, newest) {
        (true, Some(Resume::Slot(slot))) => Start { slot: Some(slot), auto_load: None },
        (true, Some(Resume::Auto)) => Start { slot: None, auto_load: (!loads_auto).then_some(true) },
        (false, _) if auto && loads_auto => Start { slot: None, auto_load: Some(false) },
        _ => Start::default(),
    }
}

/// The config a launch lays over RetroArch's own: its Auto Load State, one line.
fn auto_load_file(store: &Path) -> PathBuf {
    store.join("retroarch-auto-load.cfg")
}

/// RetroArch's own Auto Load State in `config`, which a launch laid `set` over for itself
/// (`--appendconfig`), `was` before — put back once the game has closed. RetroArch, saving its
/// settings as it quits, writes what was laid over them into its own config; a launch's marker
/// carries this, so it is put back even when Deckhand did not see the game close.
///
/// A core's or a game's own override that sets it goes over what a launch lays, and RetroArch then
/// saves nothing as it quits: such a game starts the way its override says.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoLoad {
    config: PathBuf,
    was: bool,
    set: bool,
}

impl AutoLoad {
    /// Write the config that turns RetroArch's Auto Load State `set` for a launch. None when it
    /// cannot be written: the launch then leaves RetroArch's setting as it is.
    fn lay(store: &Path, config: &Path, set: bool) -> Option<AutoLoad> {
        let line = format!("{} = \"{set}\"\n", sync::retroarch::AUTO_LOAD);
        sync::write_atomic(&auto_load_file(store), line.as_bytes()).ok()?;
        let was = sync::retroarch::setting(config, sync::retroarch::AUTO_LOAD).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        Some(AutoLoad { config: config.to_path_buf(), was, set })
    }

    /// Put RetroArch's own setting back where it is what the launch laid over it. Anything else there
    /// is the player's doing since, and stays.
    pub fn put_back(&self) {
        let key = sync::retroarch::AUTO_LOAD;
        let now = sync::retroarch::setting(&self.config, key).map(|v| v.eq_ignore_ascii_case("true"));
        if self.was != self.set && now == Some(self.set) {
            let _ = sync::retroarch::set_setting(&self.config, key, if self.was { "true" } else { "false" });
        }
    }
}

/// A session shorter than this is a game that crashed on boot or was closed from its menu, not play.
pub const MIN_SESSION: Duration = Duration::from_secs(20);

/// The time between `start` and `end` as a play session of `rom_id`, if it was long enough to be one.
pub fn play_session(rom_id: i64, start: SystemTime, end: SystemTime) -> Option<PlaySession> {
    let duration = end.duration_since(start).unwrap_or_default();
    (duration >= MIN_SESSION).then(|| PlaySession {
        rom_id: Some(rom_id),
        save_slot: Some(sync::session::AUTOSAVE.into()),
        start_time: sync::format_timestamp(start),
        end_time: sync::format_timestamp(end),
        duration_ms: duration.as_millis() as u64,
    })
}

/// Make sure RomM has the session, answering when it ended if it counted as play, and whether it is
/// still owed to RomM — which could not be reached, so the session waits in its marker for next time.
///
/// It goes with the sync session that follows the game where RomM holds those (`recorded`), on its
/// own otherwise. RomM is what keeps `last_played`, which orders *Continue playing* on every device,
/// and the playtime. Whether or not RomM took it, the answer is still the end time: the game was
/// played here, and Home should say so now rather than after the next library sync.
async fn record_session(ctx: &Context, play: Option<PlaySession>, recorded: bool, notes: &mut Vec<String>) -> (Option<String>, bool) {
    let Some(play) = play else { return (None, false) };
    let mut owed = false;
    if !recorded
        && let Some(conn) = &ctx.connection
        && let Err(e) = ctx.client.post_play_session(conn, &play).await
    {
        notes.push(format!("playtime not recorded on RomM yet: {e}"));
        owed = true;
    }
    (Some(play.end_time), owed)
}

/// Steps 1 and 2 for a game that has just finished downloading, so its first launch — here or from
/// anywhere else that opens these files — starts from the progress RomM already holds.
pub async fn after_download(ctx: Context, settings: Settings, installed: Installed) -> Vec<String> {
    match tokio::task::spawn_blocking(move || Plan::new(&settings, &installed)).await {
        Ok(plan) => ready(&ctx, &plan).await,
        Err(_) => Vec::new(),
    }
}

/// How a game's process ended.
#[derive(Debug, Clone)]
pub struct Exit {
    pub success: bool,
    pub code: Option<i32>,
    /// The last thing it printed, for a launch that failed — usually the reason.
    pub last_line: Option<String>,
}

/// Step 3: run `command` and wait for it. Blocking — the caller puts it on a thread of its own.
///
/// Both outputs are drained as they come, because a child whose pipe fills stops, and only the last
/// line of each is kept: an emulator can print every frame for hours. Stderr's wins, since that is
/// where a launcher says why it gave up.
pub fn run(command: &[String], pid: &AtomicU32) -> Result<Exit, String> {
    let (program, args) = command.split_first().ok_or("the launch command is empty")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    pid.store(child.id(), Ordering::Relaxed);
    let stdout = child.stdout.take().map(last_line);
    let stderr = child.stderr.take().map(last_line);
    let status = child.wait().map_err(|e| format!("lost track of {program}: {e}"))?;
    let joined = |reader: Option<std::thread::JoinHandle<Option<String>>>| reader.and_then(|r| r.join().ok().flatten());
    let (stderr, stdout) = (joined(stderr), joined(stdout));
    Ok(Exit { success: status.success(), code: status.code(), last_line: stderr.or(stdout) })
}

/// Read `pipe` to the end on a thread of its own, keeping its last non-blank line.
fn last_line(pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Option<String>> {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut last = None;
        for line in std::io::BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if !line.is_empty() {
                last = Some(line);
            }
        }
        last
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_splits_as_a_shell_would_and_joins_back_the_same() {
        let words = split_command(r#"retroarch -L "/my cores/{system}.so" --config='a b.cfg' it\'s {rom}"#).unwrap();
        assert_eq!(words, ["retroarch", "-L", "/my cores/{system}.so", "--config=a b.cfg", "it's", "{rom}"]);
        assert_eq!(split_command(&join_command(&words)).unwrap(), words);
        assert_eq!(split_command("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_command("a '' b").unwrap(), ["a", "", "b"], "an empty word is still a word");
        assert!(split_command("retroarch 'open").is_err());
        assert!(split_command("retroarch \\").is_err());
        assert_eq!(join_command(&["flatpak".into(), "run".into(), "x".into()]), "flatpak run x", "nothing quoted that need not be");
    }

    fn installed(files: &[&str]) -> Installed {
        Installed {
            host: String::new(),
            rom_id: 1,
            title: "Game".into(),
            system_id: "psx".into(),
            platform_id: None,
            dir: PathBuf::from("/g/psx"),
            files: files.iter().map(|f| f.to_string()).collect(),
            bytes: 0,
            ..Default::default()
        }
    }

    #[test]
    fn romm_slugs_become_the_systems_es_de_knows() {
        assert_eq!(es_system("ngc"), "gc");
        assert_eq!(es_system("PS"), "psx");
        assert_eq!(es_system("snes"), "snes");
        assert_eq!(es_system("some-new-thing"), "some-new-thing");
    }

    #[test]
    fn the_target_is_the_playlist_the_file_or_the_entry_point() {
        assert_eq!(target(&installed(&["a.cue", "a.bin", "b.cue", "b.bin", "Game.m3u"])), PathBuf::from("/g/psx/Game.m3u"));
        assert_eq!(target(&installed(&["Tetris.gb"])), PathBuf::from("/g/psx/Tetris.gb"));
        assert_eq!(target(&installed(&["G/track01.bin", "G/G.cue"])), PathBuf::from("/g/psx/G/G.cue"));
        assert_eq!(
            target(&installed(&["G/PS3_GAME/USRDIR/EBOOT.BIN", "G/PS3_GAME/PARAM.SFO"])),
            PathBuf::from("/g/psx/G"),
            "a tree with no entry point is its folder"
        );
        assert_eq!(
            target(&installed(&["Mana/Mana.sfc", "Mana/readme.txt", "Mana/Mana.ips"])),
            PathBuf::from("/g/psx/Mana/Mana.sfc"),
            "a cartridge rom among notes is the rom, not its folder"
        );
        assert_eq!(target(&installed(&["Mana.sfc", "cover.png"])), PathBuf::from("/g/psx/Mana.sfc"));
    }

    #[test]
    fn a_system_s_own_command_comes_first_and_a_flatpak_is_known_by_its_app() {
        use crate::sync::standalone::Emulator as Standalone;
        let mut settings =
            Settings { launch_command: Some(split_command("retroarch -L /c/snes9x_libretro.so").unwrap()), ..Settings::default() };
        settings.system_commands.insert("ps2".into(), split_command("flatpak run net.pcsx2.PCSX2 -batch {rom}").unwrap());
        let rom = Path::new("/g/ps2/Game.chd");
        let ps2 = Launcher::find(&settings, "ps2").unwrap();
        assert_eq!(ps2.command("ps2", rom), ["flatpak", "run", "net.pcsx2.PCSX2", "-batch", "/g/ps2/Game.chd"]);
        assert_eq!(ps2.emulator("ps2", rom), Emulator::Other("net.pcsx2.PCSX2".into()));
        assert_eq!(Standalone::for_launch("ps2", "net.pcsx2.PCSX2"), Some(Standalone::Pcsx2), "so its saves sync");
        let snes = Launcher::find(&settings, "snes").unwrap();
        assert_eq!(snes.emulator("snes", rom), Emulator::Core("snes9x".into()), "the rest start the shared way");

        // Picked from Flatpak's exports, the launcher's name is the app's.
        let exported = Launcher::Custom(preset(Path::new("/var/lib/flatpak/exports/bin/net.pcsx2.PCSX2")));
        assert_eq!(exported.emulator("ps2", rom), Emulator::Other("net.pcsx2.PCSX2".into()));
        // A core named by `{system}` is read filled in.
        let by_system = Launcher::Custom(split_command("retroarch -L /cores/{system}_libretro.so {rom}").unwrap());
        assert_eq!(by_system.emulator("mgba", rom), Emulator::Core("mgba".into()));
    }

    #[test]
    fn a_picked_emulator_gets_the_options_es_de_starts_it_with() {
        let appimage = "/home/deck/Applications/pcsx2-v2.2.0-linux-appimage-x64-Qt.AppImage";
        assert_eq!(preset(Path::new(appimage)), [appimage, "-batch", "{rom}"]);
        let dolphin = "/var/lib/flatpak/exports/bin/org.DolphinEmu.dolphin-emu";
        assert_eq!(preset(Path::new(dolphin)), [dolphin, "-b", "-e", "{rom}"]);
        assert_eq!(preset(Path::new("/usr/bin/rpcs3")), ["/usr/bin/rpcs3", "--no-gui", "{rom}"]);
        assert_eq!(preset(Path::new("/usr/bin/PPSSPPSDL")), ["/usr/bin/PPSSPPSDL", "{rom}"], "nothing to add");
        let eden = "/home/deck/Applications/Eden-Linux-v0.2.0-amd64.AppImage";
        assert_eq!(preset(Path::new(eden)), [eden, "-f", "-g", "{rom}"]);
        let ryubing = "/var/lib/flatpak/exports/bin/io.github.ryubing.Ryujinx";
        assert_eq!(preset(Path::new(ryubing)), [ryubing, "{rom}"]);
    }

    #[test]
    fn the_readme_s_example_commands_read_as_it_says() {
        let launcher = |line: &str| Launcher::Custom(split_command(line).unwrap());
        let rom = Path::new("/g/snes/Game.sfc");
        let native = launcher("retroarch -L /usr/lib/libretro/snes9x_libretro.so {rom}");
        assert_eq!(native.emulator("snes", rom), Emulator::Core("snes9x".into()));
        let flatpak = launcher(
            "flatpak run org.libretro.RetroArch -L /home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores/mgba_libretro.so {rom}",
        );
        assert_eq!(flatpak.emulator("gba", rom), Emulator::Core("mgba".into()));
        let windows = split_command(r"'C:\RetroArch-Win64\retroarch.exe' -L 'C:\RetroArch-Win64\cores\snes9x_libretro.dll' {rom}").unwrap();
        assert_eq!(windows, [r"C:\RetroArch-Win64\retroarch.exe", "-L", r"C:\RetroArch-Win64\cores\snes9x_libretro.dll", "{rom}"]);

        // An emulator per system, as picking its file fills it in — and whose saves sync.
        use crate::sync::standalone::Emulator as Standalone;
        for (system, file, line, standalone) in [
            (
                "ps2",
                "/home/deck/Applications/pcsx2-v2.2.0-linux-appimage-x64-Qt.AppImage",
                "/home/deck/Applications/pcsx2-v2.2.0-linux-appimage-x64-Qt.AppImage -batch {rom}",
                Standalone::Pcsx2,
            ),
            (
                "gc",
                "/var/lib/flatpak/exports/bin/org.DolphinEmu.dolphin-emu",
                "/var/lib/flatpak/exports/bin/org.DolphinEmu.dolphin-emu -b -e {rom}",
                Standalone::Dolphin,
            ),
            (
                "switch",
                "/home/deck/Applications/Eden-Linux-v0.2.0-amd64.AppImage",
                "/home/deck/Applications/Eden-Linux-v0.2.0-amd64.AppImage -f -g {rom}",
                Standalone::Eden,
            ),
            (
                "switch",
                "/var/lib/flatpak/exports/bin/io.github.ryubing.Ryujinx",
                "/var/lib/flatpak/exports/bin/io.github.ryubing.Ryujinx {rom}",
                Standalone::Ryujinx,
            ),
        ] {
            assert_eq!(join_command(&preset(Path::new(file))), line);
            let Emulator::Other(name) = launcher(line).emulator(system, rom) else { panic!("{line}") };
            assert_eq!(Standalone::for_launch(system, &name), Some(standalone), "{line}");
        }
    }

    #[test]
    fn retrodeck_is_told_the_system_and_a_custom_command_is_filled_in() {
        let rom = Path::new("/g/gc/Game.rvz");
        let rd = Launcher::RetroDeck(RetroDeck { es_de: PathBuf::new(), systems: Vec::new() });
        assert_eq!(rd.command("gc", rom), ["flatpak", "run", RETRODECK, "-s", "gc", "/g/gc/Game.rvz"]);

        let custom = Launcher::Custom(vec!["retroarch".into(), "-L".into(), "/c/{system}_libretro.so".into(), "{rom}".into()]);
        assert_eq!(custom.command("gc", rom), ["retroarch", "-L", "/c/gc_libretro.so", "/g/gc/Game.rvz"]);
        let bare = Launcher::Custom(vec!["dolphin-emu".into(), "-b".into()]);
        assert_eq!(bare.command("gc", rom), ["dolphin-emu", "-b", "/g/gc/Game.rvz"], "the game goes last");
        assert_eq!(bare.emulator("gc", rom), Emulator::Other("dolphin-emu".into()));
        let ra = Launcher::Custom(vec!["retroarch".into(), "-L".into(), "/c/mgba_libretro.so".into()]);
        assert_eq!(ra.emulator("gba", rom), Emulator::Core("mgba".into()));
    }

    const SYSTEMS: &str = r#"<?xml version="1.0"?>
<systemList>
  <system>
    <name>snes</name>
    <command label="Snes9x - Current">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so %ROM%</command>
    <command label="bsnes">%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/bsnes_libretro.so %ROM%</command>
  </system>
  <system>
    <name>gc</name>
    <command label="Dolphin (Standalone)">%EMULATOR_DOLPHIN% -b -e %ROM%</command>
  </system>
  <system>
    <name>ps3</name>
    <command label="RPCS3 Shortcut (Standalone)">%ENABLESHORTCUTS% %EMULATOR_OS-SHELL% %ROM%</command>
    <command label="RPCS3 Game Serial (Standalone)">%EMULATOR_RPCS3% --no-gui %RPCS3_GAMEID%:%INJECT%=%BASENAME%.ps3</command>
    <command label="RPCS3 Directory (Standalone)">%EMULATOR_RPCS3% --no-gui %ROM%</command>
    <command label="RPCS3 ISO (Standalone)">%EMULATOR_RPCS3% --no-gui %ROM%</command>
  </system>
</systemList>"#;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-launch-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("ES-DE/custom_systems")).unwrap();
            fs::write(root.join("bundled.xml"), SYSTEMS).unwrap();
            // RetroDECK's shipped custom table: every entry commented out.
            fs::write(
                root.join("ES-DE/custom_systems/es_systems.xml"),
                "<systemList><!-- <system><name>snes</name><command label=\"x\">nope</command></system> --></systemList>",
            )
            .unwrap();
            Self(root)
        }

        fn retrodeck(&self) -> RetroDeck {
            RetroDeck {
                es_de: self.0.join("ES-DE"),
                systems: vec![self.0.join("ES-DE/custom_systems/es_systems.xml"), self.0.join("bundled.xml")],
            }
        }

        fn gamelist(&self, system: &str, xml: &str) {
            let dir = self.0.join("ES-DE/gamelists").join(system);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("gamelist.xml"), xml).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resuming_hands_retroarch_the_newest_slot_through_retrodeck_or_a_command_of_one_s_own() {
        let tree = Tree::new("resume");
        let rd = Launcher::RetroDeck(tree.retrodeck());
        let rom = Path::new("/g/snes/Chrono.sfc");
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            rd.retroarch_command("snes", rom, &words(&["--entryslot", "0"])).unwrap(),
            [
                "flatpak",
                "run",
                RETRODECK,
                "-e",
                "%EMULATOR_RETROARCH% -L %CORE_RETROARCH%/snes9x_libretro.so %ROM% --entryslot 0",
                "-s",
                "snes",
                "/g/snes/Chrono.sfc"
            ],
            "the command RetroDECK would choose, as RetroDECK's -e takes one"
        );
        // RetroDECK runs that through the shell: a path goes quoted.
        let laid = rd.retroarch_command("snes", rom, &words(&["--appendconfig", "/home/me/My Store/ra.cfg"])).unwrap();
        assert!(laid[4].ends_with("%ROM% --appendconfig '/home/me/My Store/ra.cfg'"), "{laid:?}");
        assert_eq!(rd.retroarch_command("gc", Path::new("/g/gc/Game.rvz"), &words(&["--entryslot", "0"])), None, "Dolphin has no state slots to boot into");

        let ra = Launcher::Custom(vec!["retroarch".into(), "-L".into(), "/c/snes9x_libretro.so".into()]);
        assert_eq!(
            ra.retroarch_command("snes", rom, &words(&["--entryslot", "7"])).unwrap(),
            ["retroarch", "-L", "/c/snes9x_libretro.so", "/g/snes/Chrono.sfc", "--entryslot", "7"]
        );
        let other = Launcher::Custom(vec!["dolphin-emu".into(), "-b".into()]);
        assert_eq!(other.retroarch_command("gc", rom, &words(&["--entryslot", "0"])), None);
    }

    #[test]
    fn the_resume_setting_decides_whether_a_game_resumes_whatever_retroarch_s_own_says() {
        let (slot, auto) = (Some(Resume::Slot(3)), Some(Resume::Auto));
        let start = |slot: Option<u32>, auto_load: Option<bool>| Start { slot, auto_load };
        // Resuming: a numbered slot is named, which RetroArch loads instead of the auto state.
        assert_eq!(start_in(true, slot, true, true), start(Some(3), None));
        assert_eq!(start_in(true, slot, false, false), start(Some(3), None));
        // The auto state, by RetroArch's own Auto Load State — turned on for this launch if it is off.
        assert_eq!(start_in(true, auto, true, true), start(None, None));
        assert_eq!(start_in(true, auto, true, false), start(None, Some(true)));
        assert_eq!(start_in(true, None, false, false), start(None, None), "nothing to resume from");
        // Not resuming: RetroArch's Auto Load State is turned off where it would load something.
        assert_eq!(start_in(false, auto, true, true), start(None, Some(false)));
        assert_eq!(start_in(false, slot, true, true), start(None, Some(false)), "the auto state, older or not");
        assert_eq!(start_in(false, slot, false, true), start(None, None), "no auto state to load");
        assert_eq!(start_in(false, auto, true, false), start(None, None), "RetroArch would not load it anyway");
    }

    #[test]
    fn a_flatpak_is_run_with_the_game_s_folder_granted_for_the_launch() {
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let game = PathBuf::from("/run/media/sd/roms/switch/Game");
        let paths = [(game.clone(), Access::ReadWrite), (PathBuf::from("/home/me/.config/deckhand-rust/ra.cfg"), Access::Read)];

        let own = words(&["/opt/eden.AppImage", "-f", "-g", "/run/media/sd/roms/switch/Game/x.xci"]);
        assert_eq!(with_access(own.clone(), &paths), own, "not a Flatpak: as it is");

        let run = words(&["flatpak", "run", "--command=dolphin-emu", "org.DolphinEmu.dolphin-emu", "-b", "/g/x.rvz"]);
        assert_eq!(
            with_access(run, &paths[..1]),
            words(&["flatpak", "run", "--filesystem=/run/media/sd/roms/switch/Game:rw", "--command=dolphin-emu", "org.DolphinEmu.dolphin-emu", "-b", "/g/x.rvz"]),
            "granted before the app, the options it runs with kept"
        );

        // An exported launcher: the line it runs, its branch and architecture kept.
        let root = std::env::temp_dir().join(format!("deckhand-launch-flatpak-{}", std::process::id()));
        let bin = root.join("flatpak/exports/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("dev.eden_emu.eden"), "#!/bin/sh\nexec /usr/bin/flatpak run --branch=stable --arch=x86_64 dev.eden_emu.eden \"$@\"\n").unwrap();
        let launcher = bin.join("dev.eden_emu.eden").to_string_lossy().into_owned();
        let granted = with_access(words(&[&launcher, "-f", "-g", "/g/x.xci"]), &paths);
        assert_eq!(
            granted,
            words(&[
                "/usr/bin/flatpak",
                "run",
                "--filesystem=/run/media/sd/roms/switch/Game:rw",
                "--filesystem=/home/me/.config/deckhand-rust/ra.cfg:ro",
                "--branch=stable",
                "--arch=x86_64",
                "dev.eden_emu.eden",
                "-f",
                "-g",
                "/g/x.xci"
            ])
        );
        assert_eq!(program_name(&granted), "dev.eden_emu.eden", "still the same emulator");

        let gone = words(&[&bin.join("org.gone.App").to_string_lossy(), "/g/x"]);
        assert_eq!(with_access(gone.clone(), &paths), gone, "a launcher that cannot be read runs as it is");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_emulator_is_es_des_choice_game_then_system_then_default() {
        let tree = Tree::new("choice");
        let rd = tree.retrodeck();
        let rom = Path::new("/g/snes/Chrono Trigger (USA).sfc");
        assert_eq!(rd.emulator("snes", rom), Emulator::Core("snes9x".into()), "the first command is the default");
        assert_eq!(rd.emulator("gc", Path::new("/g/gc/x.rvz")), Emulator::Other("Dolphin (Standalone)".into()));

        // A system-wide choice, in the two-root shape ES-DE writes.
        tree.gamelist("snes", "<alternativeEmulator><label>bsnes</label></alternativeEmulator>\n<gameList></gameList>");
        assert_eq!(rd.emulator("snes", rom), Emulator::Core("bsnes".into()));

        // A per-game one wins over it, matched by file name wherever the game lives.
        tree.gamelist(
            "snes",
            "<alternativeEmulator><label>bsnes</label></alternativeEmulator>\n<gameList><game><path>./Chrono Trigger (USA).sfc</path>\
             <altemulator>Snes9x - Current</altemulator></game></gameList>",
        );
        assert_eq!(rd.emulator("snes", rom), Emulator::Core("snes9x".into()));
        assert_eq!(rd.emulator("snes", Path::new("/g/snes/Other.sfc")), Emulator::Core("bsnes".into()));
    }

    /// RetroDECK's PS3 table starts with the command for RPCS3's `.desktop` shortcuts, which runs the
    /// game as a script: what Deckhand downloads is handed the command for what it is instead.
    #[test]
    fn a_ps3_game_is_started_with_the_command_for_what_it_is() {
        let tree = Tree::new("ps3");
        let rd = Launcher::RetroDeck(tree.retrodeck());
        let rpcs3 = "%EMULATOR_RPCS3% --no-gui %ROM%";
        let label = |rom: &Path| match rd.emulator("ps3", rom) {
            Emulator::Other(label) => label,
            other => panic!("{other:?}"),
        };

        let iso = tree.0.join("roms/ps3/Game/Game.dec.iso");
        assert_eq!(label(&iso), "RPCS3 ISO (Standalone)");
        assert_eq!(rd.command("ps3", &iso), ["flatpak", "run", RETRODECK, "-e", rpcs3, "-s", "ps3", &iso.to_string_lossy()]);

        // A game's tree goes as its EBOOT.BIN: RetroDECK would look for Game/Game in the folder.
        let folder = tree.0.join("roms/ps3/Game");
        fs::create_dir_all(folder.join("PS3_GAME/USRDIR")).unwrap();
        fs::write(folder.join(EBOOT), "elf").unwrap();
        assert_eq!(label(&folder), "RPCS3 Directory (Standalone)");
        let eboot = folder.join(EBOOT);
        assert_eq!(rd.command("ps3", &folder), ["flatpak", "run", RETRODECK, "-e", rpcs3, "-s", "ps3", &eboot.to_string_lossy()]);

        // A shortcut is what the first command is for, and RetroDECK chooses it itself.
        let shortcut = tree.0.join("roms/ps3/Game.desktop");
        assert_eq!(label(&shortcut), "RPCS3 Shortcut (Standalone)");
        assert_eq!(rd.command("ps3", &shortcut), ["flatpak", "run", RETRODECK, "-s", "ps3", &shortcut.to_string_lossy()]);

        // A choice made in ES-DE stands, and RetroDECK reads it itself.
        tree.gamelist("ps3", "<alternativeEmulator><label>RPCS3 Shortcut (Standalone)</label></alternativeEmulator>");
        assert_eq!(label(&iso), "RPCS3 Shortcut (Standalone)");
        assert_eq!(rd.command("ps3", &iso), ["flatpak", "run", RETRODECK, "-s", "ps3", &iso.to_string_lossy()]);

        // A system whose first command is not a script is left to RetroDECK, as ever.
        let snes = Path::new("/g/snes/Game.sfc");
        assert_eq!(rd.command("snes", snes), ["flatpak", "run", RETRODECK, "-s", "snes", "/g/snes/Game.sfc"]);
    }

    #[test]
    fn with_no_table_to_read_the_label_decides() {
        let tree = Tree::new("label");
        let rd = RetroDeck { es_de: tree.0.join("ES-DE"), systems: Vec::new() };
        let rom = Path::new("/g/gc/x.rvz");
        assert_eq!(rd.emulator("gc", rom), Emulator::RetroArch);
        tree.gamelist("gc", "<alternativeEmulator><label>Dolphin (Standalone)</label></alternativeEmulator>");
        assert_eq!(rd.emulator("gc", rom), Emulator::Other("Dolphin (Standalone)".into()));
    }

    /// A downloaded game, a RetroArch to play it with, and a RomM holding its BIOS and a save from
    /// another device — then a launch that writes new progress.
    #[tokio::test]
    async fn a_download_gets_its_bios_and_save_and_a_launch_sends_the_new_save_back() {
        const CHRONO: &str = "Chrono Trigger (USA)";
        let root = std::env::temp_dir().join(format!("deckhand-launch-play-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ra")).unwrap();
        fs::create_dir_all(root.join("games/snes")).unwrap();
        fs::write(root.join(format!("games/snes/{CHRONO}.sfc")), "rom").unwrap();
        fs::write(
            root.join("ra/retroarch.cfg"),
            format!(
                "savefile_directory = \"{0}/saves\"\nsystem_directory = \"{0}/bios\"\n\
                 sort_savefiles_by_content_enable = \"true\"\nsort_savefiles_enable = \"false\"\n",
                root.display()
            ),
        )
        .unwrap();
        let save = root.join(format!("saves/snes/{CHRONO}.srm"));
        let other_device = serde_json::json!({
            "id": 11, "rom_id": 1, "file_name": format!("{CHRONO} [2026-07-01_10-00-00].srm"),
            "emulator": "snes9x", "updated_at": "2026-07-01T10:00:00Z",
            "download_path": "/api/raw/assets/saves/11.srm",
        });
        let pushed = serde_json::json!({
            "id": 12, "rom_id": 1, "file_name": format!("{CHRONO} [2026-07-02_10-00-00].srm"),
            "emulator": "snes9x", "updated_at": "2026-07-02T10:00:00Z",
        });
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("GET /api/raw/assets/saves/11.srm", 200, "from the other device".into()),
            ("GET /api/saves", 200, format!("[{other_device}]")),
            ("POST /api/saves", 201, pushed.to_string()),
            ("GET /api/states", 200, "[]".into()),
            ("GET /api/firmware?platform_id=7", 200, r#"[{"id":3,"file_name":"bios.bin","file_size_bytes":4}]"#.into()),
            ("GET /api/firmware/3/content/bios.bin", 200, "BIOS".into()),
        ])
        .await;

        let settings = Settings {
            retroarch_config: Some(root.join("ra/retroarch.cfg")),
            // A RetroArch command line in shape — `-L` names the core — that "plays" by writing a save.
            launch_command: Some(vec![
                "sh".into(),
                "-c".into(),
                format!(
                    "printf 'played here' > '{}'; ls '{}' > '{}'",
                    save.display(),
                    root.join("store/playing").display(),
                    root.join("seen").display()
                ),
                "-L".into(),
                "/cores/snes9x_libretro.so".into(),
            ]),
            ..Settings::default()
        };
        let ctx = Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None }),
            retroarch_config: settings.retroarch_config.clone(),
            store: root.join("store"),
        };
        let mut game = installed(&[&format!("{CHRONO}.sfc")]);
        game.system_id = "snes".into();
        game.platform_id = Some(7);
        game.dir = root.join("games/snes");

        let notes = after_download(ctx.clone(), settings.clone(), game.clone()).await;
        assert_eq!(notes, ["BIOS and saves synced"], "said, not named");
        assert_eq!(fs::read_to_string(root.join("bios/bios.bin")).unwrap(), "BIOS");
        assert_eq!(fs::read_to_string(&save).unwrap(), "from the other device");

        let mut stages = Vec::new();
        let session = play(ctx.clone(), settings.clone(), game.clone(), Stop::default(), |stage| stages.push(stage)).await.unwrap();
        assert!(session.exit.success, "{session:?}");
        assert_eq!(stages, [Stage::Running, Stage::Saving]);
        assert_eq!(session.notes, ["saves synced"], "nothing new to pull before, the new save after");
        let seen = fs::read_to_string(root.join("seen")).unwrap();
        assert!(seen.trim().ends_with(".start") && seen.lines().count() == 1, "its marker, while it ran: {seen:?}");
        assert_eq!(fs::read_dir(root.join("store/playing")).unwrap().count(), 0, "and gone once it closed");

        // Asked to resume, it does only once there is a state in slot 0 to boot into: RetroArch will
        // not start at all over a missing one.
        let resuming = Settings { resume_state: true, ..settings.clone() };
        let session = play(ctx.clone(), resuming.clone(), game.clone(), Stop::default(), |_| {}).await.unwrap();
        assert!(!session.notes.iter().any(|n| n.contains("resumed")), "{:?}", session.notes);
        let plan = Plan::new(&resuming, &game);
        let states = sync::retroarch::locate(resuming.retroarch_config.as_deref()).unwrap().game_states_dir(&plan.rom, Some("snes9x")).unwrap();
        fs::create_dir_all(&states).unwrap();
        fs::write(states.join(format!("{CHRONO}.state")), "state").unwrap();
        let session = play(ctx, resuming, game, Stop::default(), |_| {}).await.unwrap();
        assert!(session.notes.iter().any(|n| n.contains("resumed from the newest save state")), "{:?}", session.notes);
        assert!(requests.lock().unwrap().iter().any(|r| r.starts_with("POST /api/saves?rom_id=1")));
        let _ = fs::remove_dir_all(&root);
    }

    /// RetroArch's Auto Load State on, a game with an auto state, and Deckhand asked not to resume: the
    /// launch turns it off for itself, and puts RetroArch's own back after — which RetroArch, saving
    /// its settings as it quits, overwrote.
    #[tokio::test]
    async fn a_launch_not_resuming_keeps_retroarch_from_loading_the_auto_state_and_puts_its_setting_back() {
        const GAME: &str = "Chrono Trigger (USA)";
        let root = std::env::temp_dir().join(format!("deckhand-launch-auto-load-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ra")).unwrap();
        fs::create_dir_all(root.join("games/snes")).unwrap();
        fs::write(root.join(format!("games/snes/{GAME}.sfc")), "rom").unwrap();
        let config = root.join("ra/retroarch.cfg");
        fs::write(
            &config,
            format!(
                "savefile_directory = \"{0}/saves\"\nsavestate_directory = \"{0}/states\"\n\
                 savestate_auto_load = \"true\"\nconfig_save_on_exit = \"true\"\n",
                root.display()
            ),
        )
        .unwrap();
        fs::create_dir_all(root.join("states")).unwrap();
        fs::write(root.join(format!("states/{GAME}.state.auto")), "state").unwrap();

        // RetroArch in shape: what it was handed noted, and what was laid over its config saved into
        // it as it quits.
        let args = root.join("args");
        let script = format!(
            "printf '%s\\n' \"$@\" > '{args}'; for a; do last=$a; done; case \"$last\" in *.cfg) \
             grep -v '^savestate_auto_load' '{cfg}' > '{cfg}.new'; cat \"$last\" >> '{cfg}.new'; mv '{cfg}.new' '{cfg}';; esac",
            args = args.display(),
            cfg = config.display()
        );
        let settings = Settings {
            retroarch_config: Some(config.clone()),
            launch_command: Some(vec!["sh".into(), "-c".into(), script, "-L".into(), "/cores/snes9x_libretro.so".into()]),
            ..Settings::default()
        };
        let ctx = Context { client: RommClient::new().unwrap(), connection: None, retroarch_config: Some(config.clone()), store: root.join("store") };
        let mut game = installed(&[&format!("{GAME}.sfc")]);
        game.system_id = "snes".into();
        game.dir = root.join("games/snes");
        let auto_load = || sync::retroarch::setting(&config, sync::retroarch::AUTO_LOAD);

        let session = play(ctx.clone(), settings.clone(), game.clone(), Stop::default(), |_| {}).await.unwrap();
        assert!(session.exit.success, "{session:?}");
        assert!(!session.notes.iter().any(|n| n.contains("resumed")), "{:?}", session.notes);
        let handed = fs::read_to_string(&args).unwrap();
        assert!(handed.contains("--appendconfig"), "{handed}");
        assert_eq!(fs::read_to_string(auto_load_file(&ctx.store)).unwrap(), "savestate_auto_load = \"false\"\n");
        assert_eq!(auto_load().as_deref(), Some("true"), "RetroArch's own setting is back");
        assert_eq!(fs::read_dir(root.join("store/playing")).unwrap().count(), 0);

        // Asked to resume, RetroArch's own setting already does it: nothing is laid over.
        let resuming = Settings { resume_state: true, ..settings.clone() };
        let session = play(ctx.clone(), resuming, game, Stop::default(), |_| {}).await.unwrap();
        assert!(session.notes.iter().any(|n| n.contains("resumed from the newest save state")), "{:?}", session.notes);
        assert!(!fs::read_to_string(&args).unwrap().contains("--appendconfig"));

        // Deckhand ended with the game, which quit after: the next Deckhand puts it back, RomM or not.
        let laid = AutoLoad::lay(&ctx.store, &config, false).unwrap();
        let mut plan_game = Plan::new(&settings, &installed(&[&format!("{GAME}.sfc")])).game();
        plan_game.rom = root.join(format!("games/snes/{GAME}.sfc"));
        drop(crate::playing::hold(&ctx.store, 1, plan_game, Some(laid)).unwrap());
        sync::retroarch::set_setting(&config, sync::retroarch::AUTO_LOAD, "false").unwrap();
        assert_eq!(finish_left(&ctx).await, 0, "offline: the saves wait");
        assert_eq!(auto_load().as_deref(), Some("true"));
        let marker = fs::read_dir(root.join("store/playing")).unwrap().next().unwrap().unwrap().path();
        assert!(!fs::read_to_string(&marker).unwrap().contains("auto_load"), "and is not put back twice");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_session_long_enough_to_be_play_is_posted_and_a_short_one_is_not() {
        let (host, requests) = crate::stub_server::start_recording(vec![("POST /api/play-sessions", 200, "{}".into())]).await;
        let ctx = Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None }),
            retroarch_config: None,
            store: PathBuf::new(),
        };
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_782_900_000);
        let mut notes = Vec::new();
        let short = play_session(1, start, start + Duration::from_secs(5));
        assert_eq!(short, None, "a crash on boot is not play");
        assert_eq!(record_session(&ctx, short, false, &mut notes).await, (None, false));
        assert!(requests.lock().unwrap().is_empty());

        let long = play_session(1, start, start + Duration::from_secs(600));
        assert_eq!(long.as_ref().map(|p| (p.duration_ms, p.save_slot.as_deref())), Some((600_000, Some("autosave"))));
        // Taken with the sync session already: not posted again.
        assert_eq!(record_session(&ctx, long.clone(), true, &mut notes).await.0.as_deref(), Some("2026-07-01T10:10:00Z"));
        assert!(requests.lock().unwrap().is_empty());

        let (ended, owed) = record_session(&ctx, long.clone(), false, &mut notes).await;
        assert_eq!(ended.as_deref(), Some("2026-07-01T10:10:00Z"));
        assert!(!owed);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(requests.lock().unwrap().len(), 1);

        // RomM out of reach as the game closes: still owed, so its marker stays for the next time.
        let refusing = crate::stub_server::start(vec![("POST /api/play-sessions", 503, "{}".into())]).await;
        let ctx = Context { connection: ctx.connection.map(|c| Connection { host: refusing, ..c }), ..ctx };
        let (ended, owed) = record_session(&ctx, long, false, &mut notes).await;
        assert!(ended.is_some() && owed);
        assert!(notes.iter().any(|n| n.starts_with("playtime not recorded on RomM yet")), "{notes:?}");
    }

    #[tokio::test]
    async fn romm_hears_a_game_is_being_played_until_it_ends() {
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("POST /api/activity/heartbeat", 200, "{}".into()),
            ("DELETE /api/activity/heartbeat", 204, String::new()),
        ])
        .await;
        let device = |id: Option<&str>| Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host: host.clone(), token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: id.map(str::to_string) }),
            retroarch_config: None,
            store: PathBuf::new(),
        };
        let game = tokio::time::sleep(Duration::from_millis(300));
        while_playing(&device(Some("deck-1")), 42, None, game).await;
        let lines = requests.lock().unwrap().clone();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("POST /api/activity/heartbeat"));
        assert!(lines[1].starts_with("DELETE /api/activity/heartbeat?device_id=deck-1"));

        // Not yet a device RomM knows: nobody to say it of.
        requests.lock().unwrap().clear();
        assert_eq!(while_playing(&device(None), 42, None, async { 7 }).await, 7);
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_romm_without_activity_is_not_asked_again_while_the_game_runs() {
        let (host, requests) = crate::stub_server::start_recording(vec![]).await;
        let ctx = Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: Some("d".into()) }),
            retroarch_config: None,
            store: PathBuf::new(),
        };
        let finished = while_playing(&ctx, 42, None, async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            "exit"
        });
        assert_eq!(finished.await, "exit", "the game's own answer, whatever RomM said");
        let beats = requests.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count();
        assert_eq!(beats, 1);
    }

    #[test]
    fn a_process_is_waited_on_and_its_last_word_kept() {
        let ok = run(&["sh".into(), "-c".into(), "echo starting; echo done".into()], &AtomicU32::new(0)).unwrap();
        assert!(ok.success);
        let failed = run(&["sh".into(), "-c".into(), "echo out; echo 'no such game' >&2; exit 3".into()], &AtomicU32::new(0)).unwrap();
        assert_eq!((failed.success, failed.code, failed.last_line.as_deref()), (false, Some(3), Some("no such game")));
        assert!(run(&["/nonexistent/launcher".into()], &AtomicU32::new(0)).unwrap_err().contains("could not start"));
    }

    /// RomM rebuilt under the device, and Steam starts a game by the id it had: the game plays, and
    /// nothing of it — its save, that it is being played — goes to the game RomM keeps there now.
    #[tokio::test]
    async fn a_game_whose_id_a_rebuilt_romm_gave_to_another_plays_without_its_saves_moving() {
        const MANA: &str = "Secret of Mana (USA)";
        let root = std::env::temp_dir().join(format!("deckhand-launch-rebuilt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ra")).unwrap();
        fs::create_dir_all(root.join("games/snes")).unwrap();
        fs::write(root.join(format!("games/snes/{MANA}.sfc")), "rom").unwrap();
        fs::write(root.join("ra/retroarch.cfg"), format!("savefile_directory = \"{}/saves\"\n", root.display())).unwrap();
        let save = root.join(format!("saves/{MANA}.srm"));
        let zipang = serde_json::json!({ "id": 1, "fs_name_no_ext": "Zipang (Japan)", "created_at": "2026-10-12T19:00:00+00:00" });
        let (host, requests) = crate::stub_server::start_recording(vec![("GET /api/roms/1/simple", 200, zipang.to_string())]).await;

        let settings = Settings {
            retroarch_config: Some(root.join("ra/retroarch.cfg")),
            launch_command: Some(vec!["sh".into(), "-c".into(), format!("mkdir -p '{0}' && printf 'played' > '{1}'", root.join("saves").display(), save.display()), "-L".into(), "/cores/snes9x_libretro.so".into()]),
            ..Settings::default()
        };
        let ctx = Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: Some("deck".into()) }),
            retroarch_config: settings.retroarch_config.clone(),
            store: root.join("store"),
        };
        let mut game = installed(&[&format!("{MANA}.sfc")]);
        game.system_id = "snes".into();
        game.platform_id = Some(7);
        game.dir = root.join("games/snes");
        game.stamp = Some(Stamp { stem: MANA.into(), born: Some("2026-08-01T22:26:48+00:00".into()) });

        let mut stages = Vec::new();
        let session = play(ctx, settings, game, Stop::default(), |stage| stages.push(stage)).await.unwrap();

        assert!(session.exit.success && fs::read_to_string(&save).unwrap() == "played", "the game ran");
        assert_eq!(stages, [Stage::Running], "with no save going up, nothing waits on one after");
        assert_eq!(session.notes.len(), 1, "{:?}", session.notes);
        assert!(session.notes[0].contains("\"Zipang (Japan)\""), "and says why its save stayed: {:?}", session.notes);
        let asked = requests.lock().unwrap().clone();
        assert!(asked.iter().all(|r| r.starts_with("GET /api/roms/1/simple")), "RomM was asked, and told nothing: {asked:?}");
        let _ = fs::remove_dir_all(&root);
    }
    /// A game asked to stop while it runs — Steam's *Exit game* — hears it where it runs, writes its
    /// save as it closes, and the launch sees its closing through.
    #[tokio::test]
    async fn a_game_asked_to_stop_closes_the_way_its_own_quit_does() {
        let root = std::env::temp_dir().join(format!("deckhand-launch-stop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("games/snes")).unwrap();
        fs::write(root.join("games/snes/Some Game (USA).sfc"), "rom").unwrap();
        let save = root.join("save");
        let settings = Settings {
            launch_command: Some(vec![
                "sh".into(),
                "-c".into(),
                format!("trap 'printf saved > {}; exit 0' TERM; while :; do sleep 0.1; done", save.display()),
            ]),
            ..Settings::default()
        };
        let ctx = Context { client: RommClient::new().unwrap(), connection: None, retroarch_config: None, store: root.join("store") };
        let mut game = installed(&["Some Game (USA).sfc"]);
        game.system_id = "snes".into();
        game.dir = root.join("games/snes");

        let stop = Stop::default();
        let asking = {
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                stop.ask();
            })
        };
        let session = play(ctx.clone(), settings.clone(), game.clone(), stop, |_| {}).await.unwrap();
        asking.await.unwrap();
        assert!(session.exit.success, "{session:?}");
        assert_eq!(fs::read_to_string(&save).unwrap(), "saved");
        assert_eq!(fs::read_dir(root.join("store/playing")).unwrap().count(), 0, "its closing seen through");

        let stopped = Stop::default();
        stopped.ask();
        assert!(play(ctx, settings, game, stopped, |_| {}).await.unwrap_err().contains("before it started"));
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_game_that_does_not_close_in_time_is_left_to_its_marker() {
        let mut child = Command::new("sh").args(["-c", "trap '' TERM; while :; do sleep 0.1; done"]).spawn().unwrap();
        let pid = AtomicU32::new(child.id());
        let stop = Stop::default();
        stop.ask();
        let game = std::future::pending::<()>();
        assert_eq!(unless_stopped(&stop, &pid, Duration::from_millis(300), game).await, None);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// Deckhand ended with its game, which wrote a new save first: whichever Deckhand starts next
    /// sends the save and the time played, and lets the marker go — or keeps it while RomM cannot
    /// take them.
    #[tokio::test]
    async fn a_game_an_earlier_launch_never_saw_close_is_finished_by_the_next() {
        const GAME: &str = "Some Game (USA)";
        let root = std::env::temp_dir().join(format!("deckhand-launch-left-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ra")).unwrap();
        fs::create_dir_all(root.join("games/snes")).unwrap();
        fs::create_dir_all(root.join("saves")).unwrap();
        fs::write(root.join(format!("games/snes/{GAME}.sfc")), "rom").unwrap();
        fs::write(root.join("ra/retroarch.cfg"), format!("savefile_directory = \"{}/saves\"\n", root.display())).unwrap();
        fs::write(root.join(format!("saves/{GAME}.srm")), "played here").unwrap();
        let pushed = serde_json::json!({
            "id": 12, "rom_id": 1, "file_name": format!("{GAME} [2026-07-02_10-00-00].srm"),
            "emulator": "snes9x", "updated_at": "2026-07-02T10:00:00Z",
        });
        let game = crate::playing::Game {
            title: GAME.into(),
            rom: root.join(format!("games/snes/{GAME}.sfc")),
            system: "snes".into(),
            emulator: Emulator::Core("snes9x".into()),
            stamp: None,
        };
        let context = |host: String| Context {
            client: RommClient::new().unwrap(),
            connection: Some(Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None }),
            retroarch_config: Some(root.join("ra/retroarch.cfg")),
            store: root.join("store"),
        };
        // Ten minutes in, and Deckhand ended with the game: last touched as it went.
        let held = crate::playing::hold(&root.join("store"), 1, game, None).unwrap();
        let end = held.start() + Duration::from_secs(600);
        drop(held);
        let marker = fs::read_dir(root.join("store/playing")).unwrap().next().unwrap().unwrap().path();
        fs::OpenOptions::new().write(true).open(&marker).unwrap().set_modified(end).unwrap();

        let offline = crate::stub_server::start(vec![]).await;
        assert_eq!(finish_left(&context(offline)).await, 0);
        assert!(marker.is_file(), "kept while RomM cannot take it");

        let (host, requests) = crate::stub_server::start_recording(vec![
            ("GET /api/saves", 200, "[]".into()),
            ("POST /api/saves", 201, pushed.to_string()),
            ("GET /api/states", 200, "[]".into()),
            ("POST /api/play-sessions", 200, "{}".into()),
        ])
        .await;
        assert_eq!(finish_left(&context(host)).await, 1);
        assert!(!marker.exists());
        let asked = requests.lock().unwrap().clone();
        assert!(asked.iter().any(|r| r.starts_with("POST /api/saves?rom_id=1")), "the save it wrote went up: {asked:?}");
        assert!(asked.iter().any(|r| r.starts_with("POST /api/play-sessions")), "and the ten minutes: {asked:?}");
        let _ = fs::remove_dir_all(&root);
    }
}

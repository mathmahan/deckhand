//! `deckhand pre`, `post` and `sync` — save sync without the window, for whatever launches a game to
//! call around it. A port of the Kotlin app's `SyncCli`, for RetroArch battery saves.
//!
//! Wired into a game's Steam launch options:
//!
//! ```text
//! deckhand pre %command% && %command% && deckhand post %command%
//! ```
//!
//! `pre` and `post` do the same thing — bring that one game's save into step with RomM — because each
//! catches what the other cannot: `pre` brings down progress made on another device before it can be
//! played over, and `post` sends up what was just played before another device starts from the old
//! copy. A `post` that never runs, because the game exited with an error and `&&` stopped there, loses
//! nothing: the next `pre` sends the same change.
//!
//! Both always exit 0. The launch options chain them with `&&`, so a failure would stop the game from
//! starting, and a sync is never worth that.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Settings;
use crate::romm::RommClient;
use crate::romm::models::PlaySession;
use crate::sync::{self, Report};

/// How long a `pre` or `post` may take before it gives up. The launch waits on it with nothing on
/// screen, so a hung request must cost one sync, not the game.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Run `args` (the program name excluded) as a command, if they are one, and answer its exit code.
/// None means they are not, and the window should open.
pub fn run(args: &[String]) -> Option<i32> {
    let (command, rest) = args.split_first()?;
    match command.as_str() {
        "pre" | "post" => {
            let label = command.as_str();
            let stage = if label == "pre" { Stage::Pre } else { Stage::Post };
            let launch = Launch::parse(rest);
            match launch.rom.clone() {
                Some(rom) => {
                    let name = rom.file_name().map_or_else(|| rom.display().to_string(), |n| n.to_string_lossy().into());
                    report(label, &name, execute(launch.timeout, game(rom, launch.core, launch.system, stage)))
                }
                // Nothing in the command named a file: the whole folder, as `sync` would — which is
                // also what the Kotlin wiring's bare `post` always did.
                None => report(label, "every save", execute(launch.timeout, sweep())),
            };
            Some(0)
        }
        "sync" => {
            let ok = report("sync", "every save", execute(None, sweep()));
            Some(if ok { 0 } else { 1 })
        }
        "play" => Some(play(rest)),
        // Left running by ES-DE's game-start hook, holding the game's marker until `post` takes it.
        "keep" => {
            if let Some(rom) = Launch::parse(rest).rom {
                crate::playing::keep(&Settings::config_dir(), &rom);
            }
            Some(0)
        }
        "hooks" => {
            let (ok, lines) = match rest.first().map(String::as_str) {
                Some("install") => match crate::hooks::install() {
                    Ok(steps) => (true, steps),
                    Err(why) => (false, vec![why]),
                },
                Some("remove") => match crate::hooks::remove() {
                    Ok(steps) => (true, steps),
                    Err(why) => (false, vec![why]),
                },
                Some("status") | None => {
                    let (ok, line) = crate::hooks::status().describe();
                    (ok, vec![line])
                }
                Some(other) => (false, vec![format!("unknown: {other} — install, remove or status")]),
            };
            for line in lines {
                println!("{line}");
            }
            Some(if ok { 0 } else { 1 })
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Some(0)
        }
        _ => None,
    }
}

const USAGE: &str = "\
Deckhand save sync — every save on this device kept in step with RomM, through RomM's own
sync sessions where the server has them.

Usage:
  deckhand pre  [--rom <file>] [--core <id>] [--timeout <secs>] [<launch command>...]
  deckhand post [--rom <file>] [--core <id>] [--timeout <secs>] [<launch command>...]
  deckhand sync              every save on this device — RetroArch's, and every downloaded
                             game's — as Settings > Saves > Sync now
  deckhand play <rom id>     a downloaded game, played the way Deckhand's Play button plays it:
                             BIOS, the save down, the game, the save up, the play session.
                             What a game added to Steam from Deckhand runs
  deckhand keep --rom <file>  what ES-DE's game-start hook leaves running while the game plays:
                             holds its play-time marker until post takes it, so a session
                             ES-DE never sees the end of is still timed
  deckhand hooks [install|remove|status]
                             ES-DE's game-start and game-end hooks, so a game launched from
                             ES-DE syncs like one launched from Deckhand

pre and post sync the one game being launched, found in the launch command or given by --rom;
with neither they sync every save. They always exit 0, so they never stop a game from starting.

  --rom <file>      the game's content file, instead of finding it in the command
  --core <id>       the libretro core that runs it (snes9x, mgba), instead of RetroArch's -L.
                    Needed for a first launch on this device when RetroArch sorts saves by core
  --system <name>   the ES-DE system it runs as (snes, gc): what ES-DE's hooks pass, and how
                    the emulator — a core, or Dolphin, PCSX2, PPSSPP — is read from ES-DE
  --timeout <secs>  give up after this long (default 90)

Steam launch options:
  deckhand pre %command% && %command% && deckhand post %command%";

/// What a `pre` or `post` was asked to sync.
#[derive(Debug, PartialEq)]
struct Launch {
    rom: Option<PathBuf>,
    /// The core's id — `snes9x`, not `Snes9x` or `snes9x_libretro.so`.
    core: Option<String>,
    /// The ES-DE system, when the caller says — ES-DE's hooks do.
    system: Option<String>,
    timeout: Option<Duration>,
}

impl Launch {
    /// Flags in any order; every other argument is a word of the launch command.
    fn parse(args: &[String]) -> Self {
        let mut launch = Launch { rom: None, core: None, system: None, timeout: Some(TIMEOUT) };
        let mut words = Vec::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--rom" => launch.rom = args.next().map(PathBuf::from),
                "--core" => launch.core = args.next().map(|c| c.to_lowercase()),
                "--system" => launch.system = args.next().map(|s| s.to_lowercase()),
                "--timeout" => {
                    if let Some(secs) = args.next().and_then(|s| s.parse().ok()) {
                        launch.timeout = Some(Duration::from_secs(secs));
                    }
                }
                _ => words.push(arg.clone()),
            }
        }

        // The command arrives as its own words when `%command%` is passed bare, and as one string
        // when it is quoted — the Kotlin wiring's form — which is split here the way a shell would.
        // Both are tried, the words as given first.
        let split: Vec<String> = words.iter().flat_map(|w| split_command(w)).collect();
        let forms = [&words, &split];
        let core_path = forms.iter().find_map(|tokens| core_path(tokens));
        launch.core = launch.core.or_else(|| core_path.as_deref().and_then(core_id));
        launch.rom = launch.rom.or_else(|| forms.iter().find_map(|tokens| content_file(tokens, core_path.as_deref())));
        launch
    }
}

/// The path after RetroArch's `-L` / `--libretro`, in either spelling.
pub fn core_path(tokens: &[String]) -> Option<String> {
    tokens.iter().enumerate().find_map(|(i, token)| match token.as_str() {
        "-L" | "--libretro" => tokens.get(i + 1).cloned(),
        _ => token.strip_prefix("--libretro=").map(str::to_string),
    })
}

/// A core's id from its file: `…/cores/snes9x_libretro.so` is `snes9x`.
pub fn core_id(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let id = stem.strip_suffix("_libretro").unwrap_or(stem).to_lowercase();
    (!id.is_empty()).then_some(id)
}

/// The game's file: the last word that is a file on this machine, other than the core. Last,
/// because a launch command names its program and its core before the content it loads — and
/// Steam's own wrapper puts several more of its files in front of all of that.
fn content_file(tokens: &[String], core: Option<&str>) -> Option<PathBuf> {
    tokens
        .iter()
        .rev()
        .filter(|t| Some(t.as_str()) != core)
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// A command line split into words: whitespace separates, and `"…"` or `'…'` keep a word whole.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

/// What a run needs that only the app's own files hold: the pairing, the library to match saves
/// against, and RetroArch's whereabouts.
struct Context {
    client: RommClient,
    connection: crate::config::Connection,
    builds: Vec<sync::Build>,
    install: sync::retroarch::Install,
    store: PathBuf,
}

impl Context {
    /// Register this install with RomM, or check it still is, and keep a new id — so what this run
    /// pulls is acknowledged and its pushes are not refused. See [`sync::device`].
    async fn register(&mut self) {
        if let Some(id) = sync::device::ensure(&self.client, &mut self.connection).await {
            let mut settings = Settings::load();
            if let Some(connection) = settings.connection.as_mut().filter(|c| c.host == self.connection.host) {
                connection.device_id = Some(id);
                let _ = settings.save();
            }
        }
    }

    fn load() -> Result<Self, String> {
        let settings = Settings::load();
        let connection = settings.connection.clone().ok_or("not paired with a RomM server")?;
        // The library the window last loaded. Reading it from RomM instead would put a minute of
        // requests in front of every launch; a game added since is the cost, and opening Deckhand
        // once picks it up.
        let library = crate::cache::load(&connection.host)
            .ok_or("no library on this device yet — open Deckhand once while paired")?
            .library();
        Ok(Self {
            client: RommClient::new().map_err(|e| e.to_string())?,
            connection,
            builds: sync::builds(&library),
            install: sync::retroarch::locate(settings.retroarch_config.as_deref())?
                .with_content_dirs(crate::downloads::content_dirs(&Settings::config_dir())),
            store: Settings::config_dir(),
        })
    }
}

/// Which side of the game a `pre` or `post` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pre,
    Post,
}

async fn game(rom: PathBuf, core: Option<String>, system: Option<String>, stage: Stage) -> Result<Report, String> {
    use crate::launch::Emulator;
    let mut ctx = Context::load()?;
    let rom_id = sync::find_rom(&ctx.builds, &rom).ok();
    // `post` ends the play session `pre` started. Taken before anything that can hang, since taking it
    // is also what lets ES-DE's `keep` go — see `crate::playing`.
    let taken = match stage {
        Stage::Post => crate::playing::take(&ctx.store, &rom),
        Stage::Pre => None,
    };
    // The game as the library the window last read has it, which may be days old — held to RomM
    // before anything goes there under its id. A RomM rebuilt since keeps another game there, and
    // this one's save and playtime would land on it.
    if let Some(rom_id) = rom_id
        && let Some(stamp) = sync::stamp_of(&ctx.builds, rom_id)
    {
        sync::still_holds(&ctx.client, &ctx.connection, rom_id, &|now| stamp.same_rom(now)).await?;
    }
    ctx.register().await;
    // A session an earlier launch never got to send — before `pre` writes this one's marker over the
    // game's own.
    let left = crate::playing::settle_left(&ctx.client, &ctx.connection, &ctx.store).await;
    if left > 0 {
        eprintln!("deckhand: sent RomM the playtime of {left} earlier session(s)");
    }
    // It goes to RomM with the sync that follows the game.
    let plays: Vec<PlaySession> = taken.as_ref().and_then(|t| t.session(rom_id)).into_iter().collect();
    // Told the system, ES-DE's own configuration says what runs the game — a core RetroArch files
    // saves by, or a standalone emulator with saves of its own. A core named outright wins.
    let emulator = match (&core, &system, crate::launch::RetroDeck::detect()) {
        (None, Some(system), Some(rd)) => rd.emulator(system, &rom),
        _ => Emulator::RetroArch,
    };
    let (client, connection, install, builds, store) = (&ctx.client, &ctx.connection, &ctx.install, &ctx.builds, &ctx.store);
    let result = match emulator {
        Emulator::Core(id) => sync::sync_game(client, connection, install, builds, store, &rom, Some(&id), plays.clone()).await,
        Emulator::RetroArch => sync::sync_game(client, connection, install, builds, store, &rom, core.as_deref(), plays.clone()).await,
        Emulator::Other(label) => match sync::standalone::Emulator::for_launch(&system.unwrap_or_default(), &label) {
            None => Err(format!("it runs on {label}, whose saves this build doesn't sync")),
            Some(standalone) => match sync::find_rom(builds, &rom) {
                Ok(rom_id) => {
                    let title = rom.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let stamp = sync::stamp_of(builds, rom_id);
                    sync::standalone::sync(client, connection, store, standalone, &rom, &title, rom_id, stamp.as_ref(), plays.clone()).await
                }
                Err(why) => Err(why),
            },
        },
    };
    // A play session no sync session took — a RomM without them, a sync that failed — goes on its own,
    // and one RomM cannot take now waits in its marker for the next time.
    if let Some(play) = plays.first()
        && result.as_ref().is_ok_and(|r| r.plays_recorded == 0) | result.is_err()
        && let Err(e) = client.post_play_session(connection, play).await
    {
        eprintln!("deckhand post: playtime not recorded on RomM yet: {e}");
        if let Some(taken) = &taken {
            crate::playing::owe(store, &rom, taken, rom_id);
        }
    }
    if stage == Stage::Pre {
        crate::playing::start(store, &rom, rom_id);
    }
    result
}

async fn sweep() -> Result<Report, String> {
    let mut ctx = Context::load()?;
    ctx.register().await;
    let games = sync::games_here(Settings::load(), ctx.store.clone(), ctx.connection.host.clone()).await;
    sync::run(&ctx.client, &ctx.connection, &ctx.install, &ctx.builds, &ctx.store, &games, None).await
}

/// Run `work` to completion, or until `timeout`.
fn execute(timeout: Option<Duration>, work: impl Future<Output = Result<Report, String>>) -> Result<Report, String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        match timeout {
            Some(limit) => tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
                Err(format!("gave up after {}s; the next launch will try again", limit.as_secs()))
            }),
            None => work.await,
        }
    })
}

/// `deckhand play <rom id>` — what a game added to Steam runs: the game downloaded here, the way the
/// window's Play runs it. Answers the game's own exit code; a game that never started is 1.
fn play(args: &[String]) -> i32 {
    let Some(rom_id) = args.first().and_then(|a| a.parse::<i64>().ok()) else {
        eprintln!("deckhand play: which game? deckhand play <rom id>");
        return 2;
    };
    let settings = Settings::load();
    let host = settings.connection.as_ref().map(|c| c.host.clone()).unwrap_or_default();
    let downloads = crate::downloads::Downloads::load(Settings::config_dir(), &host);
    // By the id the shortcut was made with, which a rebuilt RomM may have moved the game from.
    let Some(installed) = downloads.installed_as(rom_id).cloned() else {
        eprintln!("deckhand play: #{rom_id} isn't downloaded on this device; open Deckhand to download it");
        return 1;
    };
    let client = match RommClient::new() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("deckhand play: {e}");
            return 1;
        }
    };
    let ctx = crate::launch::Context {
        client,
        connection: settings.connection.clone(),
        retroarch_config: settings.retroarch_config.clone(),
        store: Settings::config_dir(),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("deckhand play: {e}");
            return 1;
        }
    };
    let (title, game) = (installed.title.clone(), installed.rom_id);
    let told = |stage| match stage {
        crate::launch::Stage::Running => eprintln!("deckhand play: starting {title}"),
        crate::launch::Stage::Saving => eprintln!("deckhand play: {title} closed, syncing its saves"),
    };
    match runtime.block_on(crate::launch::play(ctx, settings, installed, told)) {
        Ok(session) => {
            for note in &session.notes {
                eprintln!("deckhand play:   {note}");
            }
            // What the space budget weighs: this game was used just now. Read afresh: the records
            // read as the game started may be hours old, and the window may have downloaded or
            // removed games since.
            if let Some(at) = &session.played_at {
                crate::downloads::Downloads::load(Settings::config_dir(), &host).mark_played(game, at);
            }
            match (session.exit.success, session.exit.code) {
                (true, _) => 0,
                (false, Some(code)) => code,
                (false, None) => 1,
            }
        }
        Err(why) => {
            eprintln!("deckhand play: {title} did not start: {why}");
            1
        }
    }
}

/// Say what a run did, on stderr — which is where Steam logs a launch command's output.
/// Answers whether it went cleanly.
fn report(label: &str, what: &str, result: Result<Report, String>) -> bool {
    match result {
        Err(why) => {
            eprintln!("deckhand {label}: {what}: {why}");
            false
        }
        Ok(report) => {
            eprintln!("deckhand {label}: {what}: {}", report.summary());
            for line in report.conflicts.iter().chain(&report.other_cores).chain(&report.notes) {
                eprintln!("deckhand {label}:   {line}");
            }
            for line in &report.failed {
                eprintln!("deckhand {label}:   failed: {line}");
            }
            report.failed.is_empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// A ROM and a core on disk, since only a file that exists counts as one.
    struct Files {
        root: PathBuf,
    }

    impl Files {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("deckhand-cli-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("roms/snes")).unwrap();
            fs::create_dir_all(root.join("cores")).unwrap();
            fs::write(root.join("roms/snes/Chrono Trigger (USA).sfc"), "rom").unwrap();
            fs::write(root.join("cores/snes9x_libretro.so"), "core").unwrap();
            fs::write(root.join("retroarch"), "program").unwrap();
            Self { root }
        }

        fn path(&self, relative: &str) -> String {
            self.root.join(relative).to_string_lossy().into_owned()
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn the_game_and_core_are_found_in_a_bare_command() {
        // `deckhand pre %command%`: the shell has already split the command into its words.
        let files = Files::new("bare");
        let rom = files.path("roms/snes/Chrono Trigger (USA).sfc");
        let launch =
            Launch::parse(&args(&[&files.path("retroarch"), "-L", &files.path("cores/snes9x_libretro.so"), &rom]));
        assert_eq!(launch.rom, Some(PathBuf::from(&rom)));
        assert_eq!(launch.core.as_deref(), Some("snes9x"));
    }

    #[test]
    fn the_game_and_core_are_found_in_a_quoted_command() {
        // `deckhand pre "%command%"`: one string, with the ROM's own quotes inside it.
        let files = Files::new("quoted");
        let rom = files.path("roms/snes/Chrono Trigger (USA).sfc");
        let command =
            format!("\"{}\" -L \"{}\" \"{rom}\"", files.path("retroarch"), files.path("cores/snes9x_libretro.so"));
        let launch = Launch::parse(&args(&[&command]));
        assert_eq!(launch.rom, Some(PathBuf::from(&rom)));
        assert_eq!(launch.core.as_deref(), Some("snes9x"));
    }

    #[test]
    fn flags_win_over_the_command() {
        let files = Files::new("flags");
        let other = files.path("roms/snes/Chrono Trigger (USA).sfc");
        let launch = Launch::parse(&args(&["--core", "bsnes", "--rom", "/elsewhere/Game.sfc", "--timeout", "5", &other]));
        assert_eq!(launch.rom, Some(PathBuf::from("/elsewhere/Game.sfc")));
        assert_eq!(launch.core.as_deref(), Some("bsnes"));
        assert_eq!(launch.timeout, Some(Duration::from_secs(5)));
    }

    #[test]
    fn a_command_naming_no_file_names_no_game() {
        let launch = Launch::parse(&args(&["flatpak", "run", "net.retrodeck.retrodeck"]));
        assert_eq!(launch.rom, None);
        assert_eq!(launch.core, None);
    }

    #[test]
    fn a_command_line_splits_like_a_shell_would() {
        assert_eq!(
            split_command(r#""/usr/bin/retroarch" -L '/cores/a b.so' "/roms/Game (USA).sfc""#),
            vec!["/usr/bin/retroarch", "-L", "/cores/a b.so", "/roms/Game (USA).sfc"]
        );
        assert_eq!(split_command("  a  \"\"  b "), vec!["a", "", "b"]);
    }

    #[test]
    fn a_core_id_is_its_file_name_without_the_libretro_tail() {
        assert_eq!(core_id("/app/lib/libretro/snes9x_libretro.so").as_deref(), Some("snes9x"));
        assert_eq!(core_id("cores/Genesis_Plus_GX_libretro.dll").as_deref(), Some("genesis_plus_gx"));
        assert_eq!(core_path(&args(&["retroarch", "--libretro=/cores/mgba_libretro.so"])).as_deref(), Some("/cores/mgba_libretro.so"));
    }

    #[test]
    fn only_the_save_commands_are_commands() {
        assert_eq!(run(&args(&["--fixtures=60"])), None, "the window's own flags open the window");
        assert_eq!(run(&[]), None);
        assert_eq!(run(&args(&["help"])), Some(0));
    }
}

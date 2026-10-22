//! ES-DE's `game-start` and `game-end` hooks, so a game launched from ES-DE itself syncs its saves
//! the way one launched from Deckhand does — a port of the Kotlin app's `EsDeHooks`.
//!
//! ES-DE runs every script in `<ES-DE>/scripts/<event>/` around a launch, waits for it, and hands it
//! the ROM path, the game's name and the system. That is what `deckhand pre`/`post` take, so the
//! hook is a short script, not a sync of its own.
//!
//! The hook does not run on the host. ES-DE is inside RetroDECK's Flatpak, and so is anything it
//! starts: the sandbox cannot ask the host to run a program (RetroDECK does not hold the Flatpak
//! portal's talk name), and cannot mount an AppImage. It can read and execute any host path and
//! reach the network, though — so the hook runs a **copy of this binary** kept at a host path, which
//! needs only what RetroDECK's runtime has (glibc, libudev). The Kotlin app had to extract a whole
//! JVM for the same; here it is one file.
//!
//! Three other things the install takes care of:
//!
//! - **The sandbox moves `XDG_CONFIG_HOME`** into RetroDECK's own tree, where this app has no
//!   pairing. The script sets it back to the host's, as the install saw it.
//! - **ES-DE ships with custom event scripts off.** The install turns them on — but only while ES-DE
//!   is not running, since ES-DE writes its whole settings file when it quits and would undo it.
//! - **The copy goes stale** when this app is updated. Every start of the app refreshes it if the
//!   hooks are installed, through a rename, so a hook mid-launch keeps the file it opened.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Settings;

/// The scripts' file name. The number puts it first among any of the user's own, which ES-DE runs in
/// name order, and makes it obviously this app's.
const SCRIPT: &str = "10-deckhand-sync.sh";

/// Each event, the command it runs, and how long that may hold ES-DE. `game-start` is the tighter:
/// ES-DE waits with nothing on screen, and a pull given up there is tried again next launch.
/// `game-end` is uploading what was just played, which is worth waiting longer for.
const EVENTS: [(&str, &str, u64); 2] = [("game-start", "pre", 60), ("game-end", "post", 120)];

/// Where the hooks' copy of this binary lives — under the cache: rebuildable, and never in RetroDECK's
/// own tree.
pub fn runtime() -> PathBuf {
    Settings::cache_dir().join("runtime").join("deckhand")
}

/// Where a hook's output goes, since nothing shows it during a launch.
pub fn log() -> PathBuf {
    Settings::cache_dir().join("esde-hook.log")
}

fn settings_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".var/app")
        .join(crate::launch::RETRODECK)
        .join("config/ES-DE/settings/es_settings.xml")
}

/// ES-DE's scripts folder — `<ES-DE>/scripts`, in RetroDECK's storage.
fn scripts_dir() -> Option<PathBuf> {
    crate::launch::RetroDeck::detect().map(|rd| rd.es_de().join("scripts"))
}

/// Where the integration stands. The default is nothing found — what is said when it could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub retrodeck: bool,
    pub installed: bool,
    pub runtime_current: bool,
    pub scripts_enabled: bool,
}

impl Status {
    /// Whether it works, and one line saying so — or saying the one thing to fix.
    pub fn describe(&self) -> (bool, String) {
        match self {
            Status { retrodeck: false, .. } => (false, "RetroDECK isn't installed".into()),
            Status { installed: false, .. } => (false, "off — games launched from ES-DE sync only on Sync now".into()),
            Status { runtime_current: false, .. } => (false, "installed, but its copy of Deckhand is out of date — reinstall".into()),
            Status { scripts_enabled: false, .. } => {
                (false, "installed, but ES-DE's custom event scripts are off — quit ES-DE and reinstall".into())
            }
            _ => (true, "on — games launched from ES-DE sync around the launch".into()),
        }
    }
}

pub fn status() -> Status {
    let Some(dir) = scripts_dir() else {
        return Status { retrodeck: false, installed: false, runtime_current: false, scripts_enabled: false };
    };
    Status {
        retrodeck: true,
        installed: EVENTS.iter().all(|(event, _, _)| dir.join(event).join(SCRIPT).is_file()),
        runtime_current: runtime_is_current(),
        scripts_enabled: scripts_enabled(&settings_file()),
    }
}

/// Install the hooks: the runtime copy, the two scripts, and ES-DE's setting. Answers what it did.
pub fn install() -> Result<Vec<String>, String> {
    let dir = scripts_dir().ok_or("RetroDECK isn't installed, so there is no ES-DE to hook into")?;
    let mut steps = Vec::new();
    copy_runtime().map_err(|e| format!("could not copy Deckhand to {}: {e}", runtime().display()))?;
    steps.push(format!("copied Deckhand to {}", runtime().display()));
    for (event, command, timeout) in EVENTS {
        let file = dir.join(event).join(SCRIPT);
        write_executable(&file, script(command, timeout).as_bytes()).map_err(|e| format!("could not write {}: {e}", file.display()))?;
        steps.push(format!("wrote the {event} hook"));
    }
    steps.push(match enable_scripts(&settings_file(), es_de_running) {
        Ok(true) => "turned on ES-DE's custom event scripts".into(),
        Ok(false) => "ES-DE's custom event scripts were already on".into(),
        Err(why) => why,
    });
    Ok(steps)
}

/// Remove the two scripts and the runtime copy. ES-DE's setting is left as it is: other scripts may
/// depend on it, and it does nothing with none installed.
pub fn remove() -> Result<Vec<String>, String> {
    let mut steps = Vec::new();
    if let Some(dir) = scripts_dir() {
        for (event, _, _) in EVENTS {
            let file = dir.join(event).join(SCRIPT);
            match fs::remove_file(&file) {
                Ok(()) => steps.push(format!("removed the {event} hook")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not remove {}: {e}", file.display())),
            }
        }
    }
    let _ = fs::remove_file(runtime());
    Ok(steps)
}

/// Bring an installed set of hooks up to this build — what every start of the app does: its copy of
/// Deckhand, and the scripts themselves where this build writes them differently. Never installs
/// anything that was not: without both scripts there is nothing to keep current.
pub fn refresh_if_installed() -> Option<String> {
    let status = status();
    let dir = scripts_dir().filter(|_| status.installed)?;
    let mut done = Vec::new();
    if !status.runtime_current {
        done.push(match copy_runtime() {
            Ok(()) => "refreshed the ES-DE hooks' copy of Deckhand".into(),
            Err(e) => format!("could not refresh the ES-DE hooks' copy of Deckhand: {e}"),
        });
    }
    for (event, command, timeout) in EVENTS {
        let (file, wanted) = (dir.join(event).join(SCRIPT), script(command, timeout));
        if fs::read_to_string(&file).is_ok_and(|text| text != wanted) {
            done.push(match write_executable(&file, wanted.as_bytes()) {
                Ok(()) => format!("rewrote the {event} hook"),
                Err(e) => format!("could not rewrite the {event} hook: {e}"),
            });
        }
    }
    (!done.is_empty()).then(|| done.join(" · "))
}

/// The hook script. Always exits 0: ES-DE starts the game after it either way, and a sync is never
/// worth holding the launch for — `--timeout` bounds it.
///
/// The game-start hook also leaves `deckhand keep` running behind it, holding the game's play-time
/// marker until the game-end hook takes it (see [`crate::playing`]). ES-DE waits for the script, not
/// for what it leaves in the background, and the sandbox ends `keep` along with ES-DE.
fn script(command: &str, timeout: u64) -> String {
    let config_home = Settings::config_dir().parent().map(Path::to_path_buf).unwrap_or_default();
    let cache_home = Settings::cache_dir().parent().map(Path::to_path_buf).unwrap_or_default();
    let keep = if command == "pre" { format!("{} keep --rom \"$1\" >> \"$LOG\" 2>&1 &\n", quoted(&runtime())) } else { String::new() };
    format!(
        "#!/bin/sh\n\
         # Generated by Deckhand: syncs this game's saves with RomM around a launch from ES-DE.\n\
         # Reinstall or remove it from Deckhand's Settings, or with `deckhand hooks`. Safe to delete.\n\
         #\n\
         # It runs inside RetroDECK's Flatpak, which points XDG_CONFIG_HOME at its own tree; without\n\
         # these Deckhand cannot find its pairing.\n\
         XDG_CONFIG_HOME={config}\n\
         XDG_CACHE_HOME={cache}\n\
         export XDG_CONFIG_HOME XDG_CACHE_HOME\n\
         LOG={log}\n\
         [ -f \"$LOG\" ] && [ \"$(wc -c < \"$LOG\")\" -gt 1048576 ] && : > \"$LOG\"\n\
         echo \"--- $(date -Is) {command} $3 $1\" >> \"$LOG\"\n\
         {binary} {command} --system \"$3\" --rom \"$1\" --timeout {timeout} >> \"$LOG\" 2>&1\n\
         {keep}\
         exit 0\n",
        config = quoted(&config_home),
        cache = quoted(&cache_home),
        log = quoted(&log()),
        binary = quoted(&runtime()),
    )
}

/// A path as one shell word that the shell takes literally — in single quotes, so a `$`, a backtick
/// or a quote in a folder name is part of the name rather than something to run.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Whether the copy is this build, byte for byte.
fn runtime_is_current() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    match (fs::metadata(&exe), fs::metadata(runtime())) {
        (Ok(a), Ok(b)) if a.len() == b.len() => fs::read(&exe).ok() == fs::read(runtime()).ok(),
        _ => false,
    }
}

fn copy_runtime() -> std::io::Result<()> {
    // Inside an AppImage this is the binary in the mounted image, readable for as long as it runs.
    let exe = std::env::current_exe()?;
    write_executable(&runtime(), &fs::read(exe)?)
}

/// Write through a staged sibling and rename it over: a hook mid-launch keeps reading the file it
/// opened, and the next launch gets the new one.
fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".{}.deckhand-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    fs::write(&staging, bytes)?;
    // An executable bit is a Unix idea, and so are the hooks: they only exist where RetroDECK does.
    // Elsewhere this still builds, and an install stops at "RetroDECK isn't installed" first.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&staging, path)
}

fn scripts_enabled(settings: &Path) -> bool {
    fs::read_to_string(settings).is_ok_and(|text| text.contains(r#"<bool name="CustomEventScripts" value="true" />"#))
}

/// Turn `CustomEventScripts` on, answering whether it changed anything. Refused while ES-DE runs.
fn enable_scripts(settings: &Path, running: impl Fn() -> bool) -> Result<bool, String> {
    let text = fs::read_to_string(settings).map_err(|_| "ES-DE has no settings file yet — start it once, then reinstall".to_string())?;
    if scripts_enabled(settings) {
        return Ok(false);
    }
    if running() {
        return Err("ES-DE is running, so its custom event scripts were left off — quit it and reinstall".into());
    }
    let off = r#"<bool name="CustomEventScripts" value="false" />"#;
    let on = r#"<bool name="CustomEventScripts" value="true" />"#;
    // ES-DE writes only the settings it knows; one without the key still takes it.
    let updated = if text.contains(off) { text.replacen(off, on, 1) } else { format!("{}\n{on}\n", text.trim_end()) };
    crate::sync::write_atomic(settings, updated.as_bytes()).map_err(|e| format!("could not update {}: {e}", settings.display()))?;
    Ok(true)
}

/// Whether ES-DE is up. Unknown reads as yes: every caller then leaves ES-DE's files alone, which
/// loses a setting rather than the user's.
fn es_de_running() -> bool {
    crate::processes::running("es-de").unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scripts_switch_is_turned_on_once_and_never_under_a_running_es_de() {
        let dir = std::env::temp_dir().join(format!("deckhand-hooks-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("es_settings.xml");
        fs::write(&file, "<?xml version=\"1.0\"?>\n<bool name=\"CustomEventScripts\" value=\"false\" />\n").unwrap();

        assert!(enable_scripts(&file, || true).unwrap_err().contains("running"));
        assert!(!scripts_enabled(&file));
        assert_eq!(enable_scripts(&file, || false), Ok(true));
        assert!(scripts_enabled(&file));
        assert_eq!(enable_scripts(&file, || false), Ok(false), "already on");

        fs::write(&file, "<?xml version=\"1.0\"?>\n<string name=\"Theme\" value=\"x\" />\n").unwrap();
        assert_eq!(enable_scripts(&file, || false), Ok(true), "a file without the key takes it");
        assert!(scripts_enabled(&file));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_script_points_the_sandbox_back_at_this_apps_files() {
        let script = script("pre", 60);
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains(&format!("XDG_CONFIG_HOME='{}'", Settings::config_dir().parent().unwrap().display())));
        assert!(script.contains(&format!("'{}' pre --system \"$3\" --rom \"$1\" --timeout 60", runtime().display())));
        assert_eq!(quoted(Path::new("/home/a'b/$x")), r"'/home/a'\''b/$x'", "a quote in a name stays in the name");
        assert!(script.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn the_start_hook_leaves_a_keeper_behind_and_the_end_hook_does_not() {
        let keeper = format!("'{}' keep --rom \"$1\" >> \"$LOG\" 2>&1 &\n", runtime().display());
        let start = script("pre", 60);
        assert!(start.contains(&keeper), "{start}");
        assert!(start.find(" pre ").unwrap() < start.find(" keep ").unwrap(), "once pre has written the marker");
        assert!(!script("post", 120).contains(" keep "));
    }

    #[test]
    fn status_says_the_one_thing_to_fix() {
        let ok = Status { retrodeck: true, installed: true, runtime_current: true, scripts_enabled: true };
        assert!(ok.describe().0);
        assert!(Status { scripts_enabled: false, ..ok }.describe().1.contains("custom event scripts"));
        assert!(Status { runtime_current: false, ..ok }.describe().1.contains("out of date"));
        assert!(Status { installed: false, ..ok }.describe().1.starts_with("off"));
    }
}

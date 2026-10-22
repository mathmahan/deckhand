//! Putting RetroDECK on a device that has none: from Flathub, then through its own first-run setup.
//!
//! Deckhand plays games through RetroDECK, so without it a device can play nothing — and it is one
//! command away. Deckhand runs outside any sandbox, so it calls `flatpak` itself, as it already
//! starts games with `flatpak run`.
//!
//! - **System-wide where the device allows it without a password.** SteamOS sets Flathub up as a
//!   system remote, and its polkit rule lets an active, local user in `wheel` install system
//!   Flatpaks without one — which is how Discover does it, in Game Mode as well. Whether this device
//!   does is asked of polkit first (`pkcheck`), rather than found out after the download.
//! - **For this user alone otherwise,** with Flathub added as a remote of the user's: a password
//!   prompt is nothing Game Mode can show.
//! - **Then RetroDECK's own setup.** Deckhand counts RetroDECK as there once its first-run wizard has
//!   said where RetroDECK's folder goes (`rd_home_path` in `retrodeck.json`), so it is started once
//!   and waited for.
//!
//! `flatpak` is read the way it writes to a pipe rather than a terminal: a line as each step starts
//! (`Installing 1/2…`) and a line for each step of its progress, ending in a percentage. It runs in
//! the C locale, so those words are the ones read here — in its UTF-8 form, since GLib writes any
//! character the locale cannot spell, the ellipsis included, as `?`. Updating RetroDECK stays with
//! Discover and `flatpak update`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::launch::RETRODECK;

/// What RetroDECK costs, from Flathub's listing when this was written — before its KDE runtime,
/// when that is not here yet.
pub const DOWNLOAD_BYTES: u64 = 2_600_000_000;
pub const INSTALLED_BYTES: u64 = 6_600_000_000;
/// Room to ask for: RetroDECK installed, and its runtime beside it.
pub const NEEDED_BYTES: u64 = 8_000_000_000;

/// Flathub's repository file, for a per-user remote.
const FLATHUB: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// Whether RetroDECK is on this device, as far as playing games through it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Installed and set up: Deckhand can start games through it.
    Ready,
    /// Installed, but its first-run setup has not said where its folder goes.
    NotSetUp,
    Missing,
}

/// Where RetroDECK stands on this device now.
pub fn presence() -> Presence {
    if crate::launch::RetroDeck::detect().is_some() {
        Presence::Ready
    } else if installed().is_some() {
        Presence::NotSetUp
    } else {
        Presence::Missing
    }
}

/// Which of Flatpak's installations a Flatpak goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every user's, under `/var/lib/flatpak`.
    System,
    /// This user's own, under `~/.local/share/flatpak`.
    User,
}

impl Scope {
    fn flag(self) -> &'static str {
        match self {
            Scope::System => "--system",
            Scope::User => "--user",
        }
    }

    /// Where the installation keeps its apps — and so where the room for one is.
    pub fn root(self) -> Option<PathBuf> {
        match self {
            Scope::System => Some(PathBuf::from("/var/lib/flatpak")),
            Scope::User => dirs::data_dir().map(|dir| dir.join("flatpak")),
        }
    }
}

/// The installation RetroDECK is in, if it is in one.
fn installed() -> Option<Scope> {
    [Scope::System, Scope::User]
        .into_iter()
        .find(|scope| scope.root().is_some_and(|root| root.join("app").join(RETRODECK).join("current/active").exists()))
}

/// Where RetroDECK would go on this device: system-wide when Flathub is a system remote and polkit
/// lets this user install there without a password, this user's own otherwise. None with no
/// `flatpak` at all.
pub fn scope() -> Option<Scope> {
    if !succeeds(Command::new("flatpak").arg("--version")) {
        return None;
    }
    let system = system_flathub() && (is_root() || allowed_without_password());
    Some(if system { Scope::System } else { Scope::User })
}

fn system_flathub() -> bool {
    Command::new("flatpak")
        .args(["remotes", "--system", "--columns=name"])
        .env("LC_ALL", "C.UTF-8")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == "flathub"))
}

/// Whether polkit lets this process install system Flatpaks with nobody asked: `pkcheck` without
/// `--allow-user-interaction` answers yes only where no password would be.
fn allowed_without_password() -> bool {
    let pid = std::process::id().to_string();
    ["org.freedesktop.Flatpak.app-install", "org.freedesktop.Flatpak.runtime-install"]
        .into_iter()
        .all(|action| succeeds(Command::new("pkcheck").args(["--action-id", action, "--process", &pid])))
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid cannot fail and touches nothing.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

fn succeeds(command: &mut Command) -> bool {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// How far the install has got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Which of Flatpak's steps it is on, from 1 — a runtime RetroDECK needs, then RetroDECK.
    pub step: usize,
    pub of: usize,
    /// How far into that step.
    pub percent: u32,
}

/// What `flatpak install` says, read a line at a time.
#[derive(Debug, Default)]
struct Reading {
    step: usize,
    of: usize,
}

impl Reading {
    /// The progress `line` says, if it says any: a step starting (`Installing…`, `Installing 2/3…`)
    /// or a step's progress, which is the same words and then a percentage.
    fn line(&mut self, line: &str) -> Option<Progress> {
        let rest = line.trim_start().strip_prefix("Installing")?;
        // The step's count, then its ellipsis — however the locale spelled that — so a line that only
        // starts with the word, such as an authenticator being installed, is not one.
        let counted_to = rest.find(|c: char| !(c.is_ascii_digit() || c == '/' || c == ' ')).unwrap_or(rest.len());
        let (counted, after) = rest.split_at(counted_to);
        let after = ["…", "...", "?"].iter().find_map(|dots| after.strip_prefix(dots))?;
        (self.step, self.of) = match counted.trim().split_once('/') {
            Some((step, of)) => (step.trim().parse().ok()?, of.trim().parse().ok()?),
            None => (1, 1),
        };
        let percent = after.split_once('%').and_then(|(before, _)| before.split_whitespace().last()?.parse().ok()).unwrap_or(0);
        Some(Progress { step: self.step, of: self.of, percent })
    }
}

/// A way to stop what [`install`] or [`set_up`] runs, from another thread: it is asked to stop, the
/// way a terminal's Ctrl+C would.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<(AtomicBool, AtomicU32)>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.0.store(true, Ordering::SeqCst);
        let pid = self.0.1.load(Ordering::SeqCst);
        #[cfg(unix)]
        if pid != 0 {
            // SAFETY: a signal to a process of ours, by the pid it was started under.
            unsafe { libc::kill(pid as i32, libc::SIGINT) };
        }
        #[cfg(not(unix))]
        let _ = pid;
    }

    pub fn asked(&self) -> bool {
        self.0.0.load(Ordering::SeqCst)
    }

    fn running(&self, pid: u32) {
        self.0.1.store(pid, Ordering::SeqCst);
        // Asked before it had a pid to ask.
        if self.asked() {
            self.cancel();
        }
    }

    fn done(&self) {
        self.0.1.store(0, Ordering::SeqCst);
    }
}

/// What an install says while it runs.
#[derive(Debug, Clone)]
pub enum Event {
    Progress(Progress),
    /// Installed; RetroDECK is open for its first-run setup, which the player goes through.
    SettingUp,
    /// It is over: where RetroDECK stands now, or why it went no further.
    Done(Result<Presence, Stop>),
}

/// Why an install went no further.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    Cancelled,
    Failed(String),
}

/// Install RetroDECK into `scope` from Flathub, telling `tell` how far it has got. Blocks until
/// `flatpak` is done — or until [`Cancel::cancel`].
pub fn install(scope: Scope, cancel: &Cancel, mut tell: impl FnMut(Progress)) -> Result<(), Stop> {
    let failed = |e: std::io::Error| Stop::Failed(format!("flatpak could not be started ({e})"));
    if scope == Scope::User {
        let added = Command::new("flatpak")
            .args(["remote-add", "--user", "--if-not-exists", "flathub", FLATHUB])
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .output()
            .map_err(failed)?;
        if !added.status.success() {
            return Err(Stop::Failed(last_words(&added.stderr).unwrap_or_else(|| "Flathub could not be added for this user".into())));
        }
    }
    let mut child = Command::new("flatpak")
        .args(["install", scope.flag(), "-y", "--noninteractive", "flathub", RETRODECK])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(failed)?;
    cancel.running(child.id());
    // Read on a thread of its own, so a pipe that fills up never stops `flatpak` writing the other.
    let errors = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut said = Vec::new();
            let _ = std::io::Read::read_to_end(&mut BufReader::new(stderr), &mut said);
            said
        })
    });
    let mut reading = Reading::default();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(progress) = reading.line(&line) {
                tell(progress);
            }
        }
    }
    let status = child.wait();
    cancel.done();
    let said = errors.and_then(|thread| thread.join().ok()).unwrap_or_default();
    if cancel.asked() {
        return Err(Stop::Cancelled);
    }
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(Stop::Failed(last_words(&said).unwrap_or_else(|| "flatpak stopped without saying why".into()))),
        Err(e) => Err(Stop::Failed(e.to_string())),
    }
}

/// The last thing `flatpak` wrote that says anything, without its `error:` label.
fn last_words(said: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(said);
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let line = line.strip_prefix("error:").or_else(|| line.strip_prefix("Error:")).unwrap_or(line).trim();
    Some(line.to_string())
}

/// Start RetroDECK once, for its first-run setup, and wait for it to close: where RetroDECK stands
/// after.
pub fn set_up() -> Result<Presence, Stop> {
    Command::new("flatpak")
        .args(["run", RETRODECK])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Stop::Failed(format!("RetroDECK could not be started ({e})")))?;
    Ok(presence())
}

/// What an offer to install RetroDECK needs to know, read off this device in one go — `flatpak` and
/// polkit are asked, so off the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub presence: Presence,
    /// Where it would go, or None with no `flatpak` to put it there.
    pub scope: Option<Scope>,
    /// Free bytes where it would go.
    pub free: Option<u64>,
}

pub fn look() -> Look {
    let presence = presence();
    let scope = if presence == Presence::Missing { scope() } else { None };
    let free = scope.and_then(Scope::root).and_then(|root| crate::downloads::storage::available(&root));
    Look { presence, scope, free }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_s_steps_and_progress_are_read_off_its_lines() {
        let mut reading = Reading::default();
        let said = [
            "Looking for matches…",
            "Required runtime for net.retrodeck.retrodeck/x86_64/stable (runtime/org.kde.Platform/x86_64/6.9) found in remote flathub",
            "Installing 1/2…",
            "Installing 1/2… ████▌                23%  12.1 MB/s  01:05",
            "Installing 1/2… ████████████████████ 100%  11.9 MB/s  00:00",
            "Installing 2/2…",
            "Installing 2/2… ██▏                  10%  9.4 MB/s  04:31",
            "Installation complete.",
        ];
        let read: Vec<Progress> = said.iter().filter_map(|line| reading.line(line)).collect();
        let at = |step, of, percent| Progress { step, of, percent };
        assert_eq!(read, [at(1, 2, 0), at(1, 2, 23), at(1, 2, 100), at(2, 2, 0), at(2, 2, 10)]);

        let mut alone = Reading::default();
        assert_eq!(alone.line("Installing… ██████████▍          52%  20.3 MB/s  00:44"), Some(at(1, 1, 52)), "one step only");
        assert_eq!(alone.line("Installing 1/2? ?????               40%  20.3 MB/s  00:44"), Some(at(1, 2, 40)), "spelled in plain ASCII");
        assert_eq!(alone.line("Installing required authenticator for remote flathub"), None);
    }

    #[test]
    fn what_flatpak_said_last_is_why_it_failed() {
        let said = b"Looking for matches...\nerror: No remote refs found for 'flathub'\n\n";
        assert_eq!(last_words(said).as_deref(), Some("No remote refs found for 'flathub'"));
        assert_eq!(last_words(b"  \n"), None);
    }
}

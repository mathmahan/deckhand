//! The processes a running game is made of, so a request to stop can reach the emulator itself.
//!
//! What Deckhand starts is rarely the emulator. RetroDECK is `flatpak run`, which becomes `bwrap`,
//! which starts a second `bwrap` as the sandbox's first process, which starts RetroDECK's launcher,
//! which starts the emulator. Neither `bwrap` passes a signal on: one sent to the outer one ends it
//! and leaves the whole sandbox running, orphaned, and the inner one is the sandbox's init, which
//! the kernel ends everything under when it goes. So a request to close goes to each process under
//! the game's own instead — the emulator first — and never to a `bwrap`. An emulator that hears
//! SIGTERM closes the way its own Quit does, writing its saves; the sandbox then winds down by
//! itself.
//!
//! The processes are looked for while the game runs ([`Tree::look`]), not only when the request
//! comes: the same Steam *Exit game* that asks Deckhand to stop may end the outer `bwrap` first,
//! and then nothing under it can be found from the process Deckhand started.

/// Whether a process named `name` is running, by its name as `/proc/<pid>/comm` has it — a program's
/// name cut to its first 15 bytes. None where that cannot be told: no `/proc` to read. What an unknown
/// answer means is the caller's: one about to write another program's settings takes it as running,
/// and leaves them alone.
pub fn running(name: &str) -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        let entries = std::fs::read_dir("/proc").ok()?;
        Some(entries.flatten().any(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|comm| comm.trim_end() == name)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        None
    }
}

/// The processes under one Deckhand started, as last seen — each by its id and when it started, so
/// a process id the system has since given to something else is never signalled.
#[derive(Debug, Default)]
pub struct Tree {
    procs: Vec<Proc>,
}

#[derive(Debug, Clone, PartialEq)]
struct Proc {
    pid: u32,
    /// Since boot, in clock ticks — `/proc/<pid>/stat`'s 22nd field.
    start: u64,
    /// How far under the process Deckhand started: 0 is that one.
    depth: usize,
    /// Never asked anything, only seen through — see the module's header.
    sandbox: bool,
}

/// What `/proc/<pid>/stat` says of a running process: its parent, when it started, and whether it
/// is a `bwrap`. None for one that has ended, a zombie included — its parent has just not collected
/// it yet.
#[cfg(target_os = "linux")]
fn stat(pid: u32) -> Option<(u32, u64, bool)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name is in parentheses and may hold anything, spaces and parentheses included.
    let (open, close) = (text.find('(')?, text.rfind(')')?);
    let name = &text[open + 1..close];
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // After the name: state (3rd field), ppid (4th), …, starttime (22nd).
    if *fields.first()? == "Z" {
        return None;
    }
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?, name == "bwrap"))
}

impl Tree {
    /// Add whatever is under `root` now, keeping every process seen before that is still running:
    /// one orphaned since by the outer `bwrap` going is still the game's.
    pub fn look(&mut self, root: u32) {
        #[cfg(target_os = "linux")]
        {
            self.procs.retain(|p| p.running());
            if root == 0 {
                return;
            }
            let Ok(entries) = std::fs::read_dir("/proc") else { return };
            let all: Vec<(u32, (u32, u64, bool))> = entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .filter_map(|pid| Some((pid, stat(pid)?)))
                .collect();
            let Some(&(_, (_, start, sandbox))) = all.iter().find(|(pid, _)| *pid == root) else { return };
            let mut found = vec![Proc { pid: root, start, depth: 0, sandbox }];
            let mut at = 0;
            while at < found.len() {
                let (parent, depth) = (found[at].pid, found[at].depth);
                for &(pid, (ppid, start, sandbox)) in &all {
                    if ppid == parent && !found.iter().any(|p| p.pid == pid) {
                        found.push(Proc { pid, start, depth: depth + 1, sandbox });
                    }
                }
                at += 1;
            }
            for proc in found {
                if !self.procs.contains(&proc) {
                    self.procs.retain(|p| p.pid != proc.pid);
                    self.procs.push(proc);
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = root;
    }

    /// Ask every process of the game that is still running to close, deepest first, so the emulator
    /// hears it before any launcher script around it.
    pub fn ask_to_close(&self) {
        #[cfg(target_os = "linux")]
        {
            let mut procs: Vec<&Proc> = self.procs.iter().filter(|p| !p.sandbox && p.running()).collect();
            procs.sort_by_key(|p| std::cmp::Reverse(p.depth));
            for proc in procs {
                if let Ok(pid) = i32::try_from(proc.pid) {
                    // SAFETY: kill(2) takes a process id and a signal number, and touches no memory.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
    }

    /// Whether every process of the game seen so far has ended.
    pub fn gone(&self) -> bool {
        !self.procs.iter().any(Proc::running)
    }
}

impl Proc {
    fn running(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            stat(self.pid).is_some_and(|(_, start, _)| start == self.start)
        }
        #[cfg(not(target_os = "linux"))]
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_process_is_found_by_its_name_and_a_name_nothing_runs_under_is_not() {
        let own = std::fs::read_to_string("/proc/self/comm").unwrap();
        assert_eq!(running(own.trim_end()), Some(true), "this test, under its own name");
        assert_eq!(running("no-such-program"), Some(false));
    }

    /// A launcher script around an "emulator" that writes its save when asked to close — the shape
    /// of RetroDECK's launcher and RetroArch, with no sandbox. The emulator is asked only once it says
    /// it has its trap: seen the moment it is forked, it would end on the request without saving, as
    /// an emulator asked to close before it has started does.
    #[test]
    fn a_game_asked_to_close_hears_it_where_it_runs_and_ends() {
        let dir = std::env::temp_dir().join(format!("deckhand-processes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (save, ready) = (dir.join("save"), dir.join("ready"));
        let emulator = format!("trap 'echo written > {}; exit 0' TERM; : > {}; while :; do sleep 0.1; done", save.display(), ready.display());
        let mut launcher = Command::new("sh")
            .args(["-c", &format!("sh -c \"{emulator}\"; echo after")])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let root = launcher.id();

        let mut tree = Tree::default();
        until("the emulator under the launcher is seen, ready to be asked", || {
            tree.look(root);
            ready.exists() && tree.procs.iter().any(|p| p.depth == 1)
        });
        assert!(!tree.gone());
        tree.ask_to_close();
        launcher.wait().unwrap();
        until("everything ends", || tree.gone());
        assert_eq!(std::fs::read_to_string(&save).unwrap().trim(), "written", "it closed the way its own Quit does");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_orphaned_by_its_parent_going_is_still_the_games() {
        // The parent leaves its child behind and ends, as the outer bwrap does.
        let mut parent = Command::new("sh").args(["-c", "sleep 30 & sleep 0.3"]).spawn().unwrap();
        let root = parent.id();
        let mut tree = Tree::default();
        until("the child is seen", || {
            tree.look(root);
            tree.procs.len() >= 2
        });
        parent.wait().unwrap();
        tree.look(root);
        assert!(!tree.gone(), "the orphan is still running, and still known");
        tree.ask_to_close();
        until("the orphan ends", || tree.gone());
    }

    #[test]
    fn a_sandbox_is_seen_through_but_never_signalled() {
        let mut tree = Tree::default();
        let mut sandbox = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = sandbox.id();
        tree.look(pid);
        tree.procs[0].sandbox = true;
        tree.ask_to_close();
        std::thread::sleep(Duration::from_millis(200));
        assert!(sandbox.try_wait().unwrap().is_none(), "still running");
        sandbox.kill().unwrap();
        sandbox.wait().unwrap();
        assert!(tree.gone());
    }
}

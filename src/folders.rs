//! Walking this device's folders with the pad — to a folder downloads go into, or to a file such as
//! a `retroarch.cfg` — one folder at a time, as a picker: the folder itself, the one above it, then
//! what is in it.
//!
//! Hidden folders are listed, after the others: a RetroArch config lives under `~/.config` or
//! `~/.var`, and a folder that cannot be reached is not one anyone can pick.

use std::path::{Path, PathBuf};

use crate::ui::picker::Row;

/// What the walk is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// A folder: each one offers itself.
    Folder,
    /// A file with this extension, lowercase and without its dot.
    File(&'static str),
    /// A program: a file that can be run — a binary, an AppImage, a Flatpak's launcher in
    /// `exports/bin` — or, on Windows, an `.exe`.
    Program,
}

/// What a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// This is the folder.
    Use(PathBuf),
    /// Go into this folder, or up to it.
    Open(PathBuf),
    /// This is the file.
    Pick(PathBuf),
}

/// The most a folder lists, so one holding a whole library of loose files stays a list to walk.
const MOST: usize = 400;

/// `dir`'s rows, and what each does.
pub fn list(dir: &Path, want: Want) -> (Vec<Row>, Vec<Step>) {
    let mut rows = Vec::new();
    let mut steps = Vec::new();
    let mut push = |label: String, detail: String, mark: Option<&str>, step: Step| {
        rows.push(Row { label, detail, aside: String::new(), mark: mark.map(str::to_string) });
        steps.push(step);
    };
    if want == Want::Folder {
        push("Use this folder".into(), dir.display().to_string(), None, Step::Use(dir.to_path_buf()));
    }
    if let Some(parent) = dir.parent() {
        push("Up".into(), parent.display().to_string(), None, Step::Open(parent.to_path_buf()));
    }

    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        // Through a symlink, as a file manager does: a mounted drive is often reached through one.
        if path.is_dir() {
            folders.push((name, path));
        } else if match want {
            Want::File(extension) => crate::downloads::layout::extension(&name) == extension,
            Want::Program => is_program(&path, &name),
            Want::Folder => false,
        } {
            files.push((name, path));
        }
    }
    // Visible before hidden, then by name as a person reads it.
    let order = |(name, _): &(String, PathBuf)| (name.starts_with('.'), name.to_lowercase());
    folders.sort_by_key(order);
    files.sort_by_key(order);
    for (name, path) in folders.into_iter().take(MOST) {
        let hidden = name.starts_with('.');
        push(format!("{name}/"), if hidden { "hidden folder" } else { "folder" }.into(), None, Step::Open(path));
    }
    let kind = if want == Want::Program { "program" } else { "file" };
    for (name, path) in files.into_iter().take(MOST) {
        push(name, kind.into(), None, Step::Pick(path));
    }
    (rows, steps)
}

/// Whether `path` is something to run. An AppImage counts before it has been made runnable: picking
/// one makes it so — see [`make_runnable`].
fn is_program(path: &Path, name: &str) -> bool {
    let extension = crate::downloads::layout::extension(name);
    #[cfg(windows)]
    {
        let _ = path;
        ["exe", "bat", "cmd"].contains(&extension.as_str())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        extension == "appimage" || std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
}

/// Let a picked program run, as `chmod +x` would: a downloaded AppImage arrives without it. Answers
/// why not, when it cannot be.
pub fn make_runnable(path: &Path) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(|e| e.to_string())?.permissions().mode();
        if mode & 0o111 == 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2))).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

/// Where a walk starts: `from` if it is a folder that exists, else the nearest one above it that
/// does, else home.
pub fn start(from: &Path) -> PathBuf {
    from.ancestors()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_lists_itself_its_parent_then_folders_and_wanted_files() {
        let dir = std::env::temp_dir().join(format!("deckhand-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["b", "A", ".config"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("retroarch.cfg"), "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();

        let (rows, steps) = list(&dir, Want::File("cfg"));
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Up", "A/", "b/", ".config/", "retroarch.cfg"], "hidden last, other files left out");
        assert_eq!(steps[4], Step::Pick(dir.join("retroarch.cfg")));

        let (rows, steps) = list(&dir, Want::Folder);
        assert_eq!(rows[0].label, "Use this folder");
        assert_eq!(steps[0], Step::Use(dir.clone()));
        assert_eq!(steps[1], Step::Open(dir.parent().unwrap().to_path_buf()));
        assert_eq!(rows.len(), 5, "no files when a folder is wanted");

        assert_eq!(start(&dir.join("not/made/yet")), dir, "the nearest folder that exists");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_walk_for_a_program_lists_what_runs_and_makes_an_appimage_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("deckhand-programs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        for (name, mode) in [("pcsx2-qt", 0o755), ("notes.txt", 0o644), ("Dolphin.AppImage", 0o644)] {
            std::fs::write(dir.join(name), "").unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let (rows, steps) = list(&dir, Want::Program);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Up", "bin/", "Dolphin.AppImage", "pcsx2-qt"], "not the notes");
        assert_eq!(steps[3], Step::Pick(dir.join("pcsx2-qt")));
        make_runnable(&dir.join("Dolphin.AppImage")).unwrap();
        assert_eq!(std::fs::metadata(dir.join("Dolphin.AppImage")).unwrap().permissions().mode() & 0o777, 0o755);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
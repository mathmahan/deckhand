//! Games already in the downloads folder that no record names — put there by an older Deckhand, by
//! the Kotlin app, by hand, or left behind when `installed.json` was lost — found and recorded, so
//! they are *on this device* like any game downloaded here.
//!
//! A game is looked for by its file's name in its platform's folder: the folder RomM names it by
//! (`ngc`) and the one ES-DE does (`gc`), since the Kotlin app files games ES-DE's way. A file or a
//! folder whose name, without its extension, is the game's is the game — which also finds an archive
//! that was unpacked (`Game (USA).cue` and `.bin` for `Game (USA).zip`) and a set's `.m3u`. Files no
//! record owns only: nothing is taken from another game. Never a save or a picture that happens to
//! share the name — RetroArch keeps saves beside the games when told to — since a recorded file is
//! one *Remove* deletes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::Installed;
use super::layout::{extension, safe_name};

/// A game as it is looked for.
#[derive(Debug, Clone)]
pub struct Wanted {
    pub rom_id: i64,
    pub title: String,
    /// RomM's platform slug.
    pub system_id: String,
    /// Its file's name without the extension — or its folder's, for a game kept as one.
    pub stem: String,
}

/// What is never the game itself, whatever it is named: saves, states, pictures, notes, patches.
pub fn not_a_game(name: &str) -> bool {
    let ext = extension(name);
    ["srm", "sav", "rtc", "eep", "fla", "mpk", "sra", "auto", "png", "jpg", "jpeg", "txt", "nfo", "part", "ips", "bps", "ups"]
        .contains(&ext.as_str())
        || ext.starts_with("state")
}

/// The games of `wanted` found in `root`, as records — every file under a matched folder, relative
/// to the platform folder, as a download records them. `taken` is every file a record already holds.
pub fn detect(root: &Path, wanted: &[Wanted], taken: &HashSet<PathBuf>) -> Vec<Installed> {
    let mut by_system: HashMap<&str, Vec<&Wanted>> = HashMap::new();
    for game in wanted {
        by_system.entry(game.system_id.as_str()).or_default().push(game);
    }
    let mut found = Vec::new();
    let mut used: HashSet<PathBuf> = HashSet::new();
    for (system, games) in by_system {
        let mut folders = vec![root.join(safe_name(system))];
        let es = root.join(crate::launch::es_system(system));
        if !folders.contains(&es) {
            folders.push(es);
        }
        for folder in folders {
            // The folder's entries by the name each is known by.
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            let mut by_stem: HashMap<String, Vec<(String, bool)>> = HashMap::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let dir = entry.path().is_dir();
                if !dir && not_a_game(&name) {
                    continue;
                }
                let stem = if dir { name.clone() } else { name.rsplit_once('.').map_or_else(|| name.clone(), |(s, _)| s.to_string()) };
                by_stem.entry(stem).or_default().push((name, dir));
            }
            for game in &games {
                if found.iter().any(|f: &Installed| f.rom_id == game.rom_id) {
                    continue;
                }
                let Some(entries) = by_stem.get(&game.stem) else { continue };
                let mut files = Vec::new();
                for (name, dir) in entries {
                    if *dir {
                        walk(&folder, Path::new(name), &mut files);
                    } else {
                        files.push(name.clone());
                    }
                }
                files.retain(|f| !taken.contains(&folder.join(f)) && !used.contains(&folder.join(f)));
                if files.is_empty() {
                    continue;
                }
                files.sort();
                let bytes = files.iter().filter_map(|f| std::fs::metadata(folder.join(f)).ok()).map(|m| m.len()).sum();
                used.extend(files.iter().map(|f| folder.join(f)));
                found.push(Installed {
                    rom_id: game.rom_id,
                    title: game.title.clone(),
                    system_id: game.system_id.clone(),
                    dir: folder.clone(),
                    files,
                    bytes,
                    ..Default::default()
                });
            }
        }
    }
    found.sort_by_key(|i| i.rom_id);
    found
}

/// Every file under `relative` in `folder`, as paths relative to `folder` — saves and pictures left
/// out, and no deeper than a game folder goes.
fn walk(folder: &Path, relative: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(folder.join(relative)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = relative.join(&name);
        if entry.path().is_dir() {
            if path.components().count() < 4 {
                walk(folder, &path, files);
            }
        } else if !not_a_game(&name) {
            files.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(rom_id: i64, system: &str, stem: &str) -> Wanted {
        Wanted { rom_id, title: stem.into(), system_id: system.into(), stem: stem.into() }
    }

    #[test]
    fn games_left_in_the_folder_are_found_by_name_in_either_spelling_of_their_platform() {
        let root = std::env::temp_dir().join(format!("deckhand-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["gb", "gc", "psx/Driver 2 (USA)"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("gb/Tetris (World).gb"), "rom").unwrap();
        std::fs::write(root.join("gb/Tetris (World).srm"), "save").unwrap();
        std::fs::write(root.join("gb/Kept (USA).gb"), "theirs").unwrap();
        std::fs::write(root.join("gc/Melee (USA).rvz"), "disc").unwrap();
        std::fs::write(root.join("psx/Driver 2 (USA)/Driver 2 (USA) (Disc 1).chd"), "d1").unwrap();
        std::fs::write(root.join("psx/Driver 2 (USA)/Driver 2 (USA) (Disc 2).chd"), "d2").unwrap();
        let games = [
            wanted(1, "gb", "Tetris (World)"),
            wanted(2, "ngc", "Melee (USA)"),
            wanted(3, "psx", "Driver 2 (USA)"),
            wanted(4, "gb", "Kept (USA)"),
            wanted(5, "gb", "Not Here (USA)"),
        ];
        let taken: HashSet<PathBuf> = [root.join("gb/Kept (USA).gb")].into_iter().collect();
        let found = detect(&root, &games, &taken);
        let summary: Vec<(i64, String, Vec<String>)> =
            found.iter().map(|i| (i.rom_id, i.dir.file_name().unwrap().to_string_lossy().into_owned(), i.files.clone())).collect();
        assert_eq!(
            summary,
            [
                (1, "gb".into(), vec!["Tetris (World).gb".into()]),
                (2, "gc".into(), vec!["Melee (USA).rvz".into()]),
                (3, "psx".into(), vec!["Driver 2 (USA)/Driver 2 (USA) (Disc 1).chd".into(), "Driver 2 (USA)/Driver 2 (USA) (Disc 2).chd".into()]),
            ],
            "the save beside it is not the game's, and a file another record holds is left to it"
        );
        assert_eq!(found[0].bytes, 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}

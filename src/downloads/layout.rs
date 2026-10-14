//! Where a rom's files sit on RomM's filesystem, and therefore where they land on this one — plus
//! the one artefact a finished download produces, the multi-disc `.m3u`.
//!
//! A port of `browse/data/RomLayout.kt` and the pure half of `browse/data/DownloadFinisher.kt`.
//! Separate from [`super`] because this is all decisions: it needs no repository, no network and no
//! disk, and it is the part that was wrong once and would be wrong silently.
//!
//! RomM reports **every** path from the library root. `fs_path` is the *platform's* directory
//! (`roms/psx`), the same for every rom on that platform and never blank; `fs_name` is the rom's own
//! entry inside it — a file name for a plain rom, a folder name for one with its own directory; and
//! a file's `file_path` is the directory holding it, again from the library root.
//!
//! Reading `file_path` as a subpath *within the rom's own folder*, which is what it looks like on a
//! single-file rom (where it happens to equal `fs_path`), rebuilds the whole library path underneath
//! the platform directory: a two-disc PS1 game lands in
//! `roms/psx/roms_psx/roms/psx/Driver 2 (USA)/`. The generated `.m3u` still resolves, since the same
//! wrong prefix is applied to it and to the files it names — so this is not a broken launch, it is a
//! library shown as a `roms_psx` folder with every game buried three levels inside it.

use crate::romm::models::{RomDetail, RomFile};

/// Entry points an emulator can be pointed at directly, and so the only things that belong in an
/// `.m3u`. A raw `.bin` track is deliberately absent: it belongs to its `.cue` and is not a disc on
/// its own, so listing it would make a single-disc game look like a two-disc one.
pub const PLAYABLE: [&str; 5] = ["cue", "chd", "iso", "pbp", "gdi"];

/// Characters that may not travel into a path, replaced one for one so the name keeps its shape.
///
/// A port of `SaveFiles.safeName`. Applied per *segment*, which is what stops a server-supplied
/// path escaping the directory it is written into: `..` sanitises to `_`, and a separator inside a
/// segment can no longer split it.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned == "." || cleaned == ".." { "_".to_string() } else { cleaned.to_string() }
}

/// The rom's own directory on the server (`roms/psx/Driver 2 (USA)`), or None when it sits directly
/// in the platform folder and its files are the platform folder's own.
///
/// Decided by what the files say rather than by the presence of a field: a rom has its own directory
/// exactly when a file is filed at or below `<fs_path>/<fs_name>`. Testing `fs_path` instead marks
/// every rom in the library as folder-structured, because it is always set.
pub fn rom_root(detail: &RomDetail) -> Option<String> {
    let platform = detail.fs_path.as_deref().unwrap_or_default().trim_matches('/');
    let name = detail.fs_name.as_deref().unwrap_or_default().trim_matches('/');
    if name.is_empty() {
        return None;
    }
    let root = if platform.is_empty() { name.to_string() } else { format!("{platform}/{name}") };
    let under = format!("{root}/");
    detail
        .files
        .iter()
        .any(|f| {
            let path = f.file_path.trim_matches('/');
            path == root || path.starts_with(&under)
        })
        .then_some(root)
}

/// The directory a rom's files are written into, relative to the platform's rom folder: the rom's
/// own name when it has its own directory on the server, else nothing — files go straight into the
/// platform folder, which is where a plain single- or multi-disc rom belongs.
pub fn local_subdirectory(detail: &RomDetail) -> Option<String> {
    rom_root(detail)?;
    let name = safe_name(detail.fs_name.as_deref().unwrap_or_default());
    (!name.is_empty()).then_some(name)
}

/// The path every file's [`relative_path`] is measured against — see [`rom_root`].
pub fn base_path(detail: &RomDetail) -> String {
    rom_root(detail).unwrap_or_else(|| {
        detail.fs_path.as_deref().unwrap_or_default().trim_matches('/').to_string()
    })
}

/// A file's path relative to `base`, with every segment sanitised so a server-supplied path can
/// never escape the directory it is written into. The unsanitised name still goes to the API call —
/// that is the name RomM knows.
///
/// A path that does not sit under `base` is kept whole rather than flattened onto its siblings: that
/// shape means this model of the server's layout is wrong, and losing the distinction between two
/// files is worse than an unexpected directory.
pub fn relative_path(file: &RomFile, base: &str) -> String {
    let raw = file.file_path.trim_matches('/');
    let within = if base.is_empty() || raw == base {
        ""
    } else if let Some(rest) = raw.strip_prefix(&format!("{base}/")) {
        rest
    } else {
        raw
    };
    let mut segments: Vec<String> =
        within.split('/').map(safe_name).filter(|s| !s.is_empty()).collect();
    // A name that sanitises away entirely would leave a path ending in a separator, which is a
    // directory, not a file. The id is the one thing about the file that is always there.
    let name = safe_name(&file.file_name);
    segments.push(if name.is_empty() { format!("file-{}", file.id) } else { name });
    segments.join("/")
}

/// The `.m3u` content for a multi-disc set, or None when the set is not multi-disc.
///
/// `files` are on-disk paths relative to the game's own download directory, which is where the
/// playlist is written — so the entries resolve beside it without a prefix. Sorted, so disc order is
/// stable rather than whatever order the server listed them in.
pub fn m3u_content(files: &[String]) -> Option<String> {
    let mut entries: Vec<&str> = files
        .iter()
        .filter(|name| PLAYABLE.contains(&extension(name).as_str()))
        .map(String::as_str)
        .collect();
    entries.sort_unstable();
    (entries.len() > 1).then(|| entries.join("\n") + "\n")
}

/// A path's lowercased extension, or "" when it has none.
pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: i64, path: &str, name: &str) -> RomFile {
        RomFile {
            id,
            file_name: name.into(),
            file_size_bytes: 1,
            file_path: path.into(),
            category: None,
        }
    }

    /// A plain rom: one file, filed in the platform directory itself.
    fn flat() -> RomDetail {
        RomDetail {
            id: 1,
            fs_name: Some("Chrono Trigger (USA).sfc".into()),
            fs_path: Some("roms/snes".into()),
            files: vec![file(10, "roms/snes", "Chrono Trigger (USA).sfc")],
            ..Default::default()
        }
    }

    /// A two-disc PS1 game: RomM gives it its own folder, and both discs are filed inside it.
    fn multi_disc() -> RomDetail {
        RomDetail {
            id: 2,
            fs_name: Some("Driver 2 (USA)".into()),
            fs_path: Some("roms/psx".into()),
            files: vec![
                file(20, "roms/psx/Driver 2 (USA)", "Driver 2 (USA) (Disc 1).cue"),
                file(21, "roms/psx/Driver 2 (USA)", "Driver 2 (USA) (Disc 1).bin"),
                file(22, "roms/psx/Driver 2 (USA)", "Driver 2 (USA) (Disc 2).cue"),
                file(23, "roms/psx/Driver 2 (USA)", "Driver 2 (USA) (Disc 2).bin"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_plain_rom_goes_straight_into_its_platform_folder() {
        let detail = flat();
        assert_eq!(rom_root(&detail), None, "its files are the platform folder's own");
        assert_eq!(local_subdirectory(&detail), None);
        assert_eq!(base_path(&detail), "roms/snes");
        assert_eq!(relative_path(&detail.files[0], "roms/snes"), "Chrono Trigger (USA).sfc");
    }

    #[test]
    fn a_rom_with_its_own_folder_keeps_it_and_nothing_above_it() {
        let detail = multi_disc();
        assert_eq!(rom_root(&detail).as_deref(), Some("roms/psx/Driver 2 (USA)"));
        assert_eq!(local_subdirectory(&detail).as_deref(), Some("Driver 2 (USA)"));
        let base = base_path(&detail);
        // The library path above the rom's own folder is gone: this is what rebuilt
        // `roms/psx/roms_psx/roms/psx/…` when `file_path` was read as a subpath.
        assert_eq!(relative_path(&detail.files[0], &base), "Driver 2 (USA) (Disc 1).cue");
    }

    #[test]
    fn a_nested_file_keeps_its_tree_under_the_rom_folder() {
        let detail = RomDetail {
            id: 3,
            fs_name: Some("Demons Souls".into()),
            fs_path: Some("roms/ps3".into()),
            files: vec![file(30, "roms/ps3/Demons Souls/PS3_GAME/USRDIR", "EBOOT.BIN")],
            ..Default::default()
        };
        let base = base_path(&detail);
        assert_eq!(relative_path(&detail.files[0], &base), "PS3_GAME/USRDIR/EBOOT.BIN");
    }

    #[test]
    fn a_server_supplied_path_cannot_escape_the_folder_it_is_written_into() {
        let base = "roms/psx/game";
        let hostile = file(40, "roms/psx/game/../../../../etc", "passwd");
        // Every `..` becomes `_`, so the path stays inside the game's folder and still names
        // something — rather than being dropped, which would silently download nothing.
        assert_eq!(relative_path(&hostile, base), "_/_/_/_/etc/passwd");
        let separators = file(41, "roms/psx/game", "../../evil.sfc");
        assert_eq!(relative_path(&separators, base), ".._.._evil.sfc");
    }

    #[test]
    fn a_file_filed_outside_the_rom_folder_is_kept_whole_rather_than_flattened() {
        // This shape means the model of the server's layout is wrong. An unexpected directory is a
        // better outcome than two files landing on the same name.
        let stray = file(50, "roms/elsewhere/extras", "readme.txt");
        assert_eq!(relative_path(&stray, "roms/psx/game"), "roms/elsewhere/extras/readme.txt");
    }

    #[test]
    fn a_file_whose_name_sanitises_away_still_lands_somewhere() {
        let nameless = file(60, "roms/snes", "   ");
        assert_eq!(relative_path(&nameless, "roms/snes"), "file-60");
    }

    #[test]
    fn only_a_multi_disc_set_gets_a_playlist_and_only_entry_points_are_in_it() {
        let discs: Vec<String> = multi_disc()
            .files
            .iter()
            .map(|f| relative_path(f, "roms/psx/Driver 2 (USA)"))
            .collect();
        assert_eq!(
            m3u_content(&discs).as_deref(),
            Some("Driver 2 (USA) (Disc 1).cue\nDriver 2 (USA) (Disc 2).cue\n"),
            "the raw .bin tracks belong to their .cue, not to the playlist"
        );
        assert_eq!(m3u_content(&["Chrono Trigger (USA).sfc".to_string()]), None);
        // One disc plus its own track is still one disc.
        assert_eq!(m3u_content(&["a.cue".to_string(), "a.bin".to_string()]), None);
    }

    #[test]
    fn names_that_cannot_travel_are_replaced_rather_than_dropped() {
        assert_eq!(safe_name("Final Fantasy VII: Disc 1.cue"), "Final Fantasy VII_ Disc 1.cue");
        assert_eq!(safe_name(".."), "_");
        assert_eq!(safe_name("a/b"), "a_b");
        assert_eq!(safe_name("  spaced  "), "spaced");
        assert_eq!(safe_name(""), "", "blank stays blank, so a caller can filter it");
    }
}

//! Unpacking a downloaded rom that came as a `.zip` or `.7z` — the step that turns "a download" into
//! "something an emulator opens".
//!
//! RomM serves a rom the way it was uploaded, and plenty are archives: a cartridge dump zipped to
//! save space, a disc's `.cue` and tracks in one `.7z`. RetroArch reads some of those, but none of
//! the standalone emulators RetroDECK runs disc systems on do, and a core that needs the real file
//! unpacks it again at every launch. So a game's archive is unpacked where it landed, and goes.
//!
//! - **Its folders come along.** An archive that is a game's folder (a DOS game, a PC title) keeps
//!   its shape; one holding a single file or a disc set at its root unpacks flat beside its siblings,
//!   as RetroDECK's rom folders are laid out.
//! - **Nothing of another game's is overwritten.** An entry that would land on a file that is not
//!   this download's sends the whole archive into a folder of its own instead.
//! - **Room first.** The unpacked size is read from the archive's own index and checked against the
//!   drive before anything is written; a short drive fails up front with both figures named.
//! - **All or nothing.** Each file goes through a `.part` and is renamed into place; a failure part
//!   way removes what was unpacked and keeps the archive, so a retry starts from what downloaded.
//! - **An archive this cannot read is kept as RomM sent it** — encrypted, damaged, or packed with a
//!   method not read here — which is what a download was before anything unpacked: an emulator that
//!   reads archives may still open it. Only this device failing (no room, a write refused) fails the
//!   download, because only that a retry can fix.
//! - **An entry naming a path outside** (`../`, an absolute path) is skipped — RomM serves what was
//!   uploaded, and an archive is someone else's input.
//!
//! Arcade sets stay zipped: there the zip *is* the rom set MAME and FinalBurn load, and unpacking it
//! breaks the game. So do DOS and PC games, which their emulators run from the archive.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::layout::{extension, safe_name};

/// Why an archive was not unpacked. Either way nothing of it is left behind but the archive.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The archive cannot be read here: encrypted, damaged, a method this does not read, or holding
    /// nothing to unpack.
    Unreadable(String),
    /// This device could not take it: not enough room, or a write refused.
    Device(String),
}

/// Whether a file is an archive this unpacks.
pub fn is_archive(name: &str) -> bool {
    matches!(extension(name).as_str(), "zip" | "7z")
}

/// Systems whose emulators load the archive itself, so it is left as it came: arcade rom sets, and
/// DOS and PC games. By RomM's platform slug.
pub fn keeps_archives(system: &str) -> bool {
    const KEEP: [&str; 19] = [
        "arcade", "mame", "fbneo", "fba", "neogeo", "neogeoaes", "neogeomvs", "cps1", "cps2", "cps3", "naomi", "naomi2",
        "atomiswave", "model2", "model3", "dos", "pc", "win", "windows",
    ];
    let system = system.to_lowercase();
    KEEP.contains(&system.as_str()) || system.starts_with("mame") || system.starts_with("arcade")
}

/// One entry of an archive: where it goes, relative to where the archive is unpacked, and its size.
struct Entry {
    /// Index in the archive (zip) or its name (7z), to find it again.
    index: usize,
    name: String,
    relative: Option<PathBuf>,
    size: u64,
    directory: bool,
}

/// An entry's path, made safe: each segment sanitised, `.` dropped, and None for anything that
/// climbs out (`..`) or is absolute.
fn safe_relative(name: &str) -> Option<PathBuf> {
    if name.starts_with('/') || name.starts_with('\\') || name.get(1..2) == Some(":") {
        return None;
    }
    let mut out = PathBuf::new();
    for segment in name.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other => out.push(safe_name(other)),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn entries(archive: &Path) -> Result<Vec<Entry>, Failure> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match extension(&name).as_str() {
        "zip" => {
            let file = fs::File::open(archive).map_err(|e| Failure::Device(format!("can't open {name}: {e}")))?;
            let mut zip =
                zip::ZipArchive::new(file).map_err(|e| Failure::Unreadable(format!("{name} is not a zip it can read: {e}")))?;
            (0..zip.len())
                .map(|index| {
                    let entry = zip.by_index_raw(index).map_err(|e| Failure::Unreadable(format!("{name}: {e}")))?;
                    Ok(Entry {
                        index,
                        name: entry.name().to_string(),
                        relative: safe_relative(entry.name()),
                        size: entry.size(),
                        directory: entry.is_dir(),
                    })
                })
                .collect()
        }
        "7z" => {
            let reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
                .map_err(|e| Failure::Unreadable(format!("{name} is not a 7z it can read: {e}")))?;
            Ok(reader
                .archive()
                .files
                .iter()
                .enumerate()
                .map(|(index, entry)| Entry {
                    index,
                    name: entry.name().to_string(),
                    relative: safe_relative(entry.name()),
                    size: entry.size(),
                    directory: entry.is_directory(),
                })
                .collect())
        }
        _ => Err(Failure::Unreadable(format!("{name} is not an archive this unpacks"))),
    }
}

/// Unpack `archive` beside itself, answering every file written, by absolute path. `ours` says
/// whether a file already there belongs to this download — what an update unpacks over. `progress`
/// is called with 0.0-1.0 of the unpacked bytes.
///
/// The archive itself is left for the caller to remove once the record of what replaced it is safe.
pub fn unpack(archive: &Path, ours: &dyn Fn(&Path) -> bool, progress: &mut dyn FnMut(f32)) -> Result<Vec<PathBuf>, Failure> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let beside = archive.parent().ok_or_else(|| Failure::Device("the archive has no folder".into()))?.to_path_buf();
    let entries = entries(archive)?;
    let files: Vec<&Entry> = entries.iter().filter(|e| !e.directory && e.relative.is_some()).collect();
    if files.is_empty() {
        return Err(Failure::Unreadable(format!("{name} holds nothing to unpack")));
    }

    // Beside the archive, unless that would overwrite something that is not this download's.
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
    let clashes = files.iter().any(|e| {
        let target = beside.join(e.relative.as_ref().expect("filtered"));
        target.exists() && !ours(&target)
    });
    let root = if clashes { beside.join(safe_name(stem)) } else { beside };

    let total: u64 = files.iter().map(|e| e.size).sum::<u64>().max(1);
    if let Some(free) = super::storage::available(&root)
        && free < total
    {
        return Err(Failure::Device(format!(
            "not enough space to unpack {name}: it needs {}, and the drive has {} free",
            super::human_bytes(total),
            super::human_bytes(free)
        )));
    }

    let mut written: Vec<PathBuf> = Vec::new();
    let result = write_all(archive, &files, &root, total, &mut written, progress);
    if let Err(failure) = result {
        for path in &written {
            let _ = fs::remove_file(path);
        }
        return Err(match failure {
            Failure::Unreadable(e) => Failure::Unreadable(format!("couldn't unpack {name}: {e}")),
            Failure::Device(e) => Failure::Device(format!("couldn't unpack {name}: {e}")),
        });
    }
    progress(1.0);
    Ok(written)
}

fn write_all(
    archive: &Path,
    files: &[&Entry],
    root: &Path,
    total: u64,
    written: &mut Vec<PathBuf>,
    progress: &mut dyn FnMut(f32),
) -> Result<(), Failure> {
    let mut done = 0u64;
    let mut out = |entry: &Entry, reader: &mut dyn Read, written: &mut Vec<PathBuf>, done: &mut u64| -> Result<(), Failure> {
        let target = root.join(entry.relative.as_ref().expect("filtered"));
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).map_err(|e| Failure::Device(format!("can't create {}: {e}", dir.display())))?;
        }
        let part = part_of(&target);
        let mut file = fs::File::create(&part).map_err(|e| Failure::Device(format!("can't write {}: {e}", part.display())))?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut got = 0u64;
        loop {
            // A read that fails is the archive's doing — damage, or a password it wants.
            let n = match reader.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    drop(file);
                    let _ = fs::remove_file(&part);
                    return Err(Failure::Unreadable(format!("{}: {e}", entry.name)));
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = file.write_all(&buffer[..n]) {
                drop(file);
                let _ = fs::remove_file(&part);
                return Err(Failure::Device(format!("can't write {}: {e}", part.display())));
            }
            // The room checked for was what the archive says it holds. An entry that unpacks to more
            // than its own index claims is damaged, or built to fill the drive: stopped at the claim.
            got += n as u64;
            if got > entry.size {
                drop(file);
                let _ = fs::remove_file(&part);
                return Err(Failure::Unreadable(format!("{} unpacks to more than the archive says it holds", entry.name)));
            }
            *done += n as u64;
            progress((*done as f32 / total as f32).min(1.0));
        }
        drop(file);
        fs::rename(&part, &target).map_err(|e| {
            let _ = fs::remove_file(&part);
            Failure::Device(format!("can't write {}: {e}", target.display()))
        })?;
        written.push(target);
        Ok(())
    };

    match extension(&archive.to_string_lossy()).as_str() {
        "zip" => {
            let file = fs::File::open(archive).map_err(|e| Failure::Device(e.to_string()))?;
            let mut zip = zip::ZipArchive::new(file).map_err(|e| Failure::Unreadable(e.to_string()))?;
            for entry in files {
                let mut reader = zip.by_index(entry.index).map_err(|e| {
                    Failure::Unreadable(match e {
                        zip::result::ZipError::UnsupportedArchive(why) => format!("{} uses something it can't read ({why})", entry.name),
                        other => format!("{}: {other}", entry.name),
                    })
                })?;
                out(entry, &mut reader, written, &mut done)?;
            }
            Ok(())
        }
        _ => {
            let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
                .map_err(|e| Failure::Unreadable(e.to_string()))?;
            let wanted: std::collections::HashMap<&str, &Entry> = files.iter().map(|e| (e.name.as_str(), *e)).collect();
            let mut failure: Option<Failure> = None;
            reader
                .for_each_entries(|entry, read| {
                    let Some(ours) = wanted.get(entry.name()) else {
                        // Not one of ours (a directory, an unsafe path): read past it.
                        std::io::copy(read, &mut std::io::sink())?;
                        return Ok(true);
                    };
                    match out(ours, read, written, &mut done) {
                        Ok(()) => Ok(true),
                        Err(why) => {
                            failure = Some(why);
                            Ok(false)
                        }
                    }
                })
                .map_err(|e| Failure::Unreadable(e.to_string()))?;
            failure.map_or(Ok(()), Err)
        }
    }
}

/// `<target>.part`, beside it.
fn part_of(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp folder of its own, gone when the test ends.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deckhand-unpack-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn zip_of(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            if name.ends_with('/') {
                zip.add_directory(*name, options).unwrap();
            } else {
                zip.start_file(*name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    fn names(root: &Path, written: &[PathBuf]) -> Vec<String> {
        let mut out: Vec<String> =
            written.iter().map(|p| p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        out.sort();
        out
    }

    #[test]
    fn a_zip_unpacks_beside_itself_keeping_its_folders() {
        let dir = Dir::new("zip");
        let archive = dir.0.join("Doom (1993).zip");
        zip_of(&archive, &[("DOOM/", b""), ("DOOM/DOOM.EXE", b"exe"), ("DOOM/DOOM.WAD", b"wad"), ("README.TXT", b"hi")]);
        let mut seen = 0.0;
        let written = unpack(&archive, &|_| false, &mut |p| seen = p).unwrap();
        assert_eq!(names(&dir.0, &written), ["DOOM/DOOM.EXE", "DOOM/DOOM.WAD", "README.TXT"]);
        assert_eq!(fs::read(dir.0.join("DOOM/DOOM.WAD")).unwrap(), b"wad");
        assert_eq!(seen, 1.0);
        assert!(archive.exists(), "the caller removes the archive once it has recorded what replaced it");
    }

    #[test]
    fn a_7z_unpacks_too() {
        let dir = Dir::new("7z");
        let src = dir.0.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("Game (USA).cue"), b"FILE \"Game (USA).bin\" BINARY").unwrap();
        fs::write(src.join("Game (USA).bin"), vec![7u8; 4096]).unwrap();
        let archive = dir.0.join("Game (USA).7z");
        sevenz_rust2::compress_to_path(&src, &archive).unwrap();
        fs::remove_dir_all(&src).unwrap();

        let written = unpack(&archive, &|_| false, &mut |_| {}).unwrap();
        assert_eq!(names(&dir.0, &written), ["Game (USA).bin", "Game (USA).cue"]);
        assert_eq!(fs::read(dir.0.join("Game (USA).bin")).unwrap(), vec![7u8; 4096]);
    }

    #[test]
    fn another_game_s_file_in_the_way_sends_the_archive_into_a_folder_of_its_own() {
        let dir = Dir::new("clash");
        fs::write(dir.0.join("README.TXT"), b"another game's").unwrap();
        let archive = dir.0.join("Game.zip");
        zip_of(&archive, &[("Game.rom", b"rom"), ("README.TXT", b"this game's")]);
        let written = unpack(&archive, &|_| false, &mut |_| {}).unwrap();
        assert_eq!(names(&dir.0, &written), ["Game/Game.rom", "Game/README.TXT"]);
        assert_eq!(fs::read(dir.0.join("README.TXT")).unwrap(), b"another game's", "left alone");
        // Over this download's own file — an update — it unpacks in place.
        let again = unpack(&archive, &|p| p == dir.0.join("README.TXT"), &mut |_| {}).unwrap();
        assert_eq!(names(&dir.0, &again), ["Game.rom", "README.TXT"]);
    }

    #[test]
    fn an_entry_climbing_out_is_skipped() {
        let dir = Dir::new("slip");
        let archive = dir.0.join("evil.zip");
        zip_of(&archive, &[("../escaped.txt", b"no"), ("/abs.txt", b"no"), ("ok/fine.txt", b"yes")]);
        let written = unpack(&archive, &|_| false, &mut |_| {}).unwrap();
        assert_eq!(names(&dir.0, &written), ["ok/fine.txt"]);
        assert!(!dir.0.parent().unwrap().join("escaped.txt").exists());
    }

    #[test]
    fn an_archive_it_cannot_read_is_said_so_and_nothing_of_it_is_left() {
        let dir = Dir::new("locked");
        // The first entry unpacks; the second wants a password.
        let archive = dir.0.join("Locked.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let plain = zip::write::SimpleFileOptions::default();
        zip.start_file("Locked.cue", plain).unwrap();
        zip.write_all(b"cue").unwrap();
        let locked = zip::unstable::write::FileOptionsExt::with_deprecated_encryption(plain, b"secret").unwrap();
        zip.start_file("Locked.bin", locked).unwrap();
        zip.write_all(b"bin").unwrap();
        zip.finish().unwrap();
        let failure = unpack(&archive, &|_| false, &mut |_| {}).unwrap_err();
        assert!(matches!(&failure, Failure::Unreadable(why) if why.contains("Locked.bin")), "{failure:?}");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "the entry unpacked before it is gone again; the archive stays");

        let damaged = dir.0.join("Damaged.7z");
        fs::write(&damaged, b"7z\xBC\xAF\x27\x1C and then nothing").unwrap();
        assert!(matches!(unpack(&damaged, &|_| false, &mut |_| {}), Err(Failure::Unreadable(_))));
    }

    #[test]
    fn arcade_sets_and_pc_games_stay_zipped() {
        assert!(keeps_archives("arcade") && keeps_archives("mame-2003") && keeps_archives("neogeoaes") && keeps_archives("dos"));
        assert!(!keeps_archives("psx") && !keeps_archives("snes") && !keeps_archives("neo-geo-cd"));
        assert!(is_archive("Game.ZIP") && is_archive("Game.7z") && !is_archive("Game.chd"));
    }

    #[test]
    fn an_entry_that_unpacks_past_what_the_archive_claims_is_stopped_there() {
        let dir = Dir::new("liar");
        let archive = dir.0.join("Game.zip");
        zip_of(&archive, &[("Game.rom", &[7u8; 4096])]);
        // The archive's index as a liar would write it: four bytes, for four kilobytes of content.
        let liar = Entry { index: 0, name: "Game.rom".into(), relative: Some(PathBuf::from("Game.rom")), size: 4, directory: false };
        let mut written = Vec::new();
        let result = write_all(&archive, &[&liar], &dir.0, 4, &mut written, &mut |_| {});
        assert!(matches!(&result, Err(Failure::Unreadable(why)) if why.contains("more than")), "{result:?}");
        assert!(written.is_empty() && !dir.0.join("Game.rom").exists() && !dir.0.join("Game.rom.part").exists());
    }
}
//! What this device has room for — a port of the Kotlin app's `StorageManager`, with a drive picker
//! that is about where Deckhand's downloads go.
//!
//! - **Free space** on whichever drive a folder is on, read before a download or an unpack starts,
//!   so a full card fails up front with both figures named rather than half-way through a file.
//! - **Where downloads go**: the Games folder in home, or the same folder on any drive mounted here —
//!   an SD card, an external disk — with what each has free.
//! - **A space budget** the player sets. Off until they choose one: nothing is removed unasked. With
//!   one set, a download that would go over it first makes room, removing the games used least
//!   recently — never a pinned one, never one being played or fetched. Removal is the reversible kind:
//!   the files go, the game stays in the library with its saves, and one press brings it back.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Space-budget steps for Settings, in GiB; 0 is off.
pub const BUDGET_STEPS: [u64; 9] = [0, 16, 32, 64, 128, 256, 512, 1024, 2048];
pub const BUDGET_LABELS: [&str; 9] = ["Off", "16 GB", "32 GB", "64 GB", "128 GB", "256 GB", "512 GB", "1 TB", "2 TB"];

const GIB: u64 = 1024 * 1024 * 1024;

/// A budget in GiB, in bytes — None when it is off.
pub fn budget_bytes(gib: Option<u64>) -> Option<u64> {
    gib.filter(|g| *g > 0).map(|g| g * GIB)
}

/// Free and total bytes of the filesystem holding `path`, or of its nearest ancestor that exists —
/// a folder about to be created is on the drive its parent is on.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    let mut at = Some(path);
    while let Some(dir) = at {
        if dir.exists() {
            return stat(dir);
        }
        at = dir.parent();
    }
    None
}

/// Free bytes where `path` would be written.
pub fn available(path: &Path) -> Option<u64> {
    space(path).map(|(free, _)| free)
}

#[cfg(unix)]
fn stat(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut info: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path, `info` is written by the call and read only after
    // it reports success.
    if unsafe { libc::statvfs(c.as_ptr(), &mut info) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    let block = info.f_frsize as u64;
    #[allow(clippy::unnecessary_cast)]
    Some((info.f_bavail as u64 * block, info.f_blocks as u64 * block))
}

#[cfg(windows)]
fn stat(dir: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated; the two out-pointers are valid for the call.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut())
    };
    (ok != 0).then_some((free, total))
}

#[cfg(not(any(unix, windows)))]
fn stat(_: &Path) -> Option<(u64, u64)> {
    None
}

/// A place downloads can go.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// "Home", "SD card · STEAMDECK", "Drive · data".
    pub name: String,
    /// Where games would be written — a `Games/Deckhand` folder on that drive.
    pub path: PathBuf,
    pub free: Option<u64>,
    pub total: Option<u64>,
    /// Where downloads go now.
    pub current: bool,
    /// The default, which choosing clears the setting back to.
    pub default: bool,
}

/// Every place downloads can go: the default first — RetroDECK's roms folder, or home's Games folder
/// — then each drive mounted here, then the folder in use now when it is none of those.
pub fn locations(current: &Path, default: &Path) -> Vec<Location> {
    let name = if crate::launch::retrodeck_roms().as_deref() == Some(default) { "RetroDECK" } else { "Home" };
    let mut out = vec![place(name.into(), default.to_path_buf(), current, true)];
    for (name, mount) in drives() {
        let path = mount.join("Games").join("Deckhand");
        if out.iter().all(|l| l.path != path) {
            out.push(place(name, path, current, false));
        }
    }
    if out.iter().all(|l| l.path != current) {
        out.push(place("The folder in use".into(), current.to_path_buf(), current, false));
    }
    out
}

fn place(name: String, path: PathBuf, current: &Path, default: bool) -> Location {
    let (free, total) = space(&path).map_or((None, None), |(f, t)| (Some(f), Some(t)));
    Location { current: path == current, name, path, free, total, default }
}

/// Drives mounted here that games can go on, by a name to show and where each is mounted.
#[cfg(target_os = "linux")]
fn drives() -> Vec<(String, PathBuf)> {
    let table = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    mounted(&table)
        .into_iter()
        .filter(|(_, mount)| writable(mount))
        .collect()
}

#[cfg(windows)]
fn drives() -> Vec<(String, PathBuf)> {
    ('C'..='Z')
        .map(|letter| (format!("Drive {letter}:"), PathBuf::from(format!("{letter}:\\"))))
        .filter(|(_, root)| root.exists())
        .collect()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn drives() -> Vec<(String, PathBuf)> {
    Vec::new()
}

/// The drives in a `/proc/mounts` table worth offering: real filesystems mounted where removable
/// and extra drives go — `/run/media` (SteamOS's SD card, any desktop's USB disk), `/media`, `/mnt`.
#[cfg(any(target_os = "linux", test))]
fn mounted(table: &str) -> Vec<(String, PathBuf)> {
    const FILESYSTEMS: [&str; 13] =
        ["ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "exfat", "vfat", "ntfs", "ntfs3", "fuseblk", "zfs", "bcachefs"];
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount), Some(kind)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let mount = unescape(mount);
        if !FILESYSTEMS.contains(&kind) || !["/run/media/", "/media/", "/mnt/"].iter().any(|p| mount.starts_with(p)) {
            continue;
        }
        let label = Path::new(&mount).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let card = device.starts_with("/dev/mmcblk");
        let name = if card { format!("SD card · {label}") } else { format!("Drive · {label}") };
        if out.iter().all(|(_, m)| m != Path::new(&mount)) {
            out.push((name, PathBuf::from(mount)));
        }
    }
    out
}

/// A mount point as `/proc/mounts` escapes it — `\040` for a space.
#[cfg(any(target_os = "linux", test))]
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len()
            && let Ok(code) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 4]).unwrap_or(""), 8)
        {
            out.push(code);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a mount point is somewhere this user can write.
#[cfg(target_os = "linux")]
fn writable(dir: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().to_string_lossy().as_bytes()) else { return false };
    // SAFETY: `c` is a valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// One installed game as making room weighs it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub rom_id: i64,
    pub title: String,
    pub bytes: u64,
    /// When it was last used here: played, or failing that downloaded.
    pub used: Option<SystemTime>,
    /// Pinned, being played, or being fetched: never removed to make room.
    pub kept: bool,
}

/// Which games go so `incoming` more bytes fit under `budget` beside `installed`: the least recently
/// used first, until the overshoot is covered. Never a kept one. Empty when it already fits; every
/// candidate, and still short, when even that is not enough — the download goes ahead regardless,
/// since it is what the player asked for.
pub fn make_room(installed: &[Candidate], budget: u64, incoming: u64) -> Vec<&Candidate> {
    let used: u64 = installed.iter().map(|c| c.bytes).sum();
    let over = (used + incoming).saturating_sub(budget);
    if over == 0 {
        return Vec::new();
    }
    let mut order: Vec<&Candidate> = installed.iter().filter(|c| !c.kept).collect();
    // Never used sorts first — but "used" includes when it was downloaded, so a game that has only
    // just arrived is recent, not first in line.
    order.sort_by_key(|c| c.used);
    let mut freed = 0u64;
    order
        .into_iter()
        .take_while(|c| {
            let take = freed < over;
            freed += c.bytes;
            take
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(days: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(days * 86_400))
    }

    fn game(rom_id: i64, gib: u64, used: Option<SystemTime>, kept: bool) -> Candidate {
        Candidate { rom_id, title: format!("Game {rom_id}"), bytes: gib * GIB, used, kept }
    }

    #[test]
    fn room_is_made_from_the_least_recently_used_and_never_from_a_kept_game() {
        let installed = [game(1, 10, at(5), false), game(2, 10, at(1), true), game(3, 10, at(3), false), game(4, 10, at(9), false)];
        let gone: Vec<i64> = make_room(&installed, 40 * GIB, 15 * GIB).iter().map(|c| c.rom_id).collect();
        assert_eq!(gone, [3, 1], "15 GB over: the oldest unpinned two, the pinned oldest of all kept");
        assert!(make_room(&installed, 60 * GIB, 15 * GIB).is_empty(), "fits already");
        let all: Vec<i64> = make_room(&installed, 5 * GIB, 50 * GIB).iter().map(|c| c.rom_id).collect();
        assert_eq!(all, [3, 1, 4], "everything not kept, and still short");
    }

    #[test]
    fn drives_are_read_from_the_mount_table() {
        let table = "\
/dev/nvme0n1p2 / btrfs rw 0 0
/dev/mmcblk0p1 /run/media/deck/STEAM\\040CARD ext4 rw 0 0
/dev/sda1 /mnt/data ext4 rw 0 0
tmpfs /run/media/deck/ram tmpfs rw 0 0
/dev/sdb1 /media/me/USB exfat rw 0 0
";
        let found = mounted(table);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["SD card · STEAM CARD", "Drive · data", "Drive · USB"]);
        assert_eq!(found[0].1, PathBuf::from("/run/media/deck/STEAM CARD"));
    }

    #[test]
    fn free_space_is_read_from_the_nearest_folder_that_exists() {
        let missing = std::env::temp_dir().join("deckhand-not-made-yet/Games/Deckhand");
        let (free, total) = space(&missing).expect("the temp dir's drive");
        assert!(total >= free && total > 0);
    }
}

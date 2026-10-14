//! Games in Steam: a downloaded game as a non-Steam shortcut in Steam's library, with RomM's art,
//! launched through Deckhand — `deckhand play <rom id>` — so it plays exactly as Play does: BIOS,
//! the save down, the game, the save up, the play session.
//!
//! A port of the Kotlin app's `steam/`, whose plan and on-Deck findings this follows:
//!
//! - **Steam owns `shortcuts.vdf` while it runs.** It holds shortcuts in memory and writes the file
//!   back on exit, so an edit made under it is lost. The file is only written with Steam closed: if
//!   it is running, Deckhand asks it to shut down (`steam -shutdown`), waits, writes, and starts it
//!   again. In Game Mode that would end the session Deckhand runs in, so there adding is refused —
//!   it is done from Desktop Mode, once, and the shortcuts are there in Game Mode from then on.
//! - **The file is binary VDF**, parsed and written back byte for byte — a merge must never damage
//!   the shortcuts the player already has. Anything in it this does not read leaves it untouched,
//!   and the old file is kept beside it before every write.
//! - **The app id** is `crc32(exe + name) | 0x80000000`, the scheme Steam ROM Manager uses and
//!   Steam honours: the same game always gets the same id, so adding again replaces rather than
//!   duplicates, and the art files are found by it.
//! - **The art** is RomM's, re-encoded as PNG under `config/grid/`: the cover as the portrait
//!   capsule (`<id>p.png`) and, cut square, the icon; the banner as the wide capsule; the
//!   transparent logo as the logo. The hero across the top of the game's page is made the way
//!   Home's spotlight is, from the first screenshot: sharp at the right, over a wash of its own
//!   colours that Steam's logo sits on. Not the banner, which has the wordmark drawn in already and
//!   would show it twice. Steam keeps nothing else for a shortcut — no description, developer or
//!   release date to fill in.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Binary VDF: a map is a run of `type, key\0, value` entries closed by `0x08`.
pub mod vdf {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Map(Vec<(String, Value)>),
        Str(String),
        Int(i32),
        Float(f32),
        U64(u64),
    }

    impl Value {
        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Map(entries) => entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
                _ => None,
            }
        }
    }

    /// A whole file: its top-level map.
    pub fn parse(bytes: &[u8]) -> Result<Vec<(String, Value)>, String> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let mut at = 0;
        let map = read_map(bytes, &mut at)?;
        if at != bytes.len() {
            return Err(format!("{} bytes after the end", bytes.len() - at));
        }
        Ok(map)
    }

    fn read_map(bytes: &[u8], at: &mut usize) -> Result<Vec<(String, Value)>, String> {
        let mut entries = Vec::new();
        loop {
            let kind = *bytes.get(*at).ok_or("the file ends inside a map")?;
            *at += 1;
            if kind == 0x08 {
                return Ok(entries);
            }
            let key = read_str(bytes, at)?;
            let value = match kind {
                0x00 => Value::Map(read_map(bytes, at)?),
                0x01 => Value::Str(read_str(bytes, at)?),
                0x02 => Value::Int(i32::from_le_bytes(take::<4>(bytes, at)?)),
                0x03 => Value::Float(f32::from_le_bytes(take::<4>(bytes, at)?)),
                0x07 => Value::U64(u64::from_le_bytes(take::<8>(bytes, at)?)),
                other => return Err(format!("a field of type {other:#04x}, which this does not read")),
            };
            entries.push((key, value));
        }
    }

    fn read_str(bytes: &[u8], at: &mut usize) -> Result<String, String> {
        let end = bytes[*at..].iter().position(|b| *b == 0).ok_or("a string with no end")?;
        let text = std::str::from_utf8(&bytes[*at..*at + end]).map_err(|_| "a string that is not UTF-8".to_string())?;
        *at += end + 1;
        Ok(text.to_string())
    }

    fn take<const N: usize>(bytes: &[u8], at: &mut usize) -> Result<[u8; N], String> {
        let slice = bytes.get(*at..*at + N).ok_or("the file ends inside a number")?;
        *at += N;
        Ok(slice.try_into().expect("N bytes"))
    }

    /// A whole file from its top-level map.
    pub fn write(map: &[(String, Value)]) -> Vec<u8> {
        let mut out = Vec::new();
        write_map(map, &mut out);
        out
    }

    fn write_map(map: &[(String, Value)], out: &mut Vec<u8>) {
        for (key, value) in map {
            let kind = match value {
                Value::Map(_) => 0x00,
                Value::Str(_) => 0x01,
                Value::Int(_) => 0x02,
                Value::Float(_) => 0x03,
                Value::U64(_) => 0x07,
            };
            out.push(kind);
            out.extend_from_slice(key.as_bytes());
            out.push(0);
            match value {
                Value::Map(inner) => write_map(inner, out),
                Value::Str(text) => {
                    out.extend_from_slice(text.as_bytes());
                    out.push(0);
                }
                Value::Int(n) => out.extend_from_slice(&n.to_le_bytes()),
                Value::Float(n) => out.extend_from_slice(&n.to_le_bytes()),
                Value::U64(n) => out.extend_from_slice(&n.to_le_bytes()),
            }
        }
        out.push(0x08);
    }
}

use vdf::Value;

/// One game as a Steam shortcut.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub name: String,
    /// The program, quoted the way Steam writes it — the id is hashed over it as written.
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
    pub tags: Vec<String>,
    /// The icon's file, once one is written; empty for Steam's own.
    pub icon: String,
}

impl Shortcut {
    /// `rom_id`'s game, run by the Deckhand at `deckhand`.
    pub fn for_game(deckhand: &Path, rom_id: i64, title: &str, system: &str) -> Self {
        let dir = deckhand.parent().unwrap_or(Path::new("/"));
        Shortcut {
            name: title.to_string(),
            exe: format!("\"{}\"", deckhand.display()),
            start_dir: format!("\"{}\"", dir.display()),
            launch_options: format!("play {rom_id}"),
            tags: vec!["Deckhand".into(), system.to_string()],
            icon: String::new(),
        }
    }

    /// Steam's id for it: `crc32(exe + name) | 0x80000000`.
    pub fn appid(&self) -> u32 {
        appid(&self.exe, &self.name)
    }

    fn entry(&self) -> Value {
        let s = |v: &str| Value::Str(v.to_string());
        Value::Map(vec![
            ("appid".into(), Value::Int(self.appid() as i32)),
            ("AppName".into(), s(&self.name)),
            ("Exe".into(), s(&self.exe)),
            ("StartDir".into(), s(&self.start_dir)),
            ("icon".into(), s(&self.icon)),
            ("ShortcutPath".into(), s("")),
            ("LaunchOptions".into(), s(&self.launch_options)),
            ("IsHidden".into(), Value::Int(0)),
            ("AllowDesktopConfig".into(), Value::Int(1)),
            ("AllowOverlay".into(), Value::Int(1)),
            ("OpenVR".into(), Value::Int(0)),
            ("Devkit".into(), Value::Int(0)),
            ("DevkitGameID".into(), s("")),
            ("DevkitOverrideAppID".into(), Value::Int(0)),
            ("LastPlayTime".into(), Value::Int(0)),
            ("FlatpakAppID".into(), s("")),
            ("tags".into(), Value::Map(self.tags.iter().enumerate().map(|(i, t)| (i.to_string(), s(t))).collect())),
        ])
    }
}

pub fn appid(exe: &str, name: &str) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(exe.as_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize() | 0x8000_0000
}

/// The shortcuts in a parsed file, adding the `shortcuts` map when there is none.
fn shortcuts(file: &mut Vec<(String, Value)>) -> &mut Vec<(String, Value)> {
    if !file.iter().any(|(k, _)| k.eq_ignore_ascii_case("shortcuts")) {
        file.push(("shortcuts".into(), Value::Map(Vec::new())));
    }
    match file.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("shortcuts")) {
        Some((_, Value::Map(entries))) => entries,
        _ => unreachable!("a shortcuts map was just made sure of"),
    }
}

fn id_of(entry: &Value) -> Option<u32> {
    match entry.get("appid") {
        Some(Value::Int(n)) => Some(*n as u32),
        _ => None,
    }
}

/// `file` with `games` in it — each replacing a shortcut of the same id, or added at the end — and
/// the entries numbered from 0 again, as Steam numbers them.
pub fn merged(mut file: Vec<(String, Value)>, games: &[Shortcut]) -> Vec<(String, Value)> {
    let entries = shortcuts(&mut file);
    for game in games {
        match entries.iter_mut().find(|(_, e)| id_of(e) == Some(game.appid())) {
            Some((_, existing)) => *existing = game.entry(),
            None => entries.push((String::new(), game.entry())),
        }
    }
    for (i, (key, _)) in entries.iter_mut().enumerate() {
        *key = i.to_string();
    }
    file
}

/// `file` without the shortcuts of `ids`.
pub fn without(mut file: Vec<(String, Value)>, ids: &[u32]) -> Vec<(String, Value)> {
    let entries = shortcuts(&mut file);
    entries.retain(|(_, e)| id_of(e).is_none_or(|id| !ids.contains(&id)));
    for (i, (key, _)) in entries.iter_mut().enumerate() {
        *key = i.to_string();
    }
    file
}

/// A Steam install, and the user whose library shortcuts go into.
#[derive(Debug, Clone, PartialEq)]
pub struct Steam {
    pub root: PathBuf,
    /// `userdata/<account id>` of the user who used Steam last.
    pub user: PathBuf,
    /// Steam from Flathub, which is started and stopped through `flatpak`.
    pub flatpak: bool,
}

impl Steam {
    /// Steam on this device: native or from Flathub, and of its users the one who used it last.
    pub fn find() -> Option<Self> {
        // Never the real one from a test: the run's own folder, where a test can make one.
        #[cfg(test)]
        let home = crate::config::test_root().join("home");
        #[cfg(not(test))]
        let home = dirs::home_dir()?;
        let roots = [
            (home.join(".local/share/Steam"), false),
            (home.join(".steam/steam"), false),
            (home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"), true),
        ];
        let mut seen = Vec::new();
        for (root, flatpak) in roots {
            let Ok(real) = root.canonicalize() else { continue };
            if seen.contains(&real) {
                continue;
            }
            seen.push(real.clone());
            if let Some(user) = last_user(&real.join("userdata")) {
                return Some(Steam { root: real, user, flatpak });
            }
        }
        None
    }

    pub fn shortcuts_file(&self) -> PathBuf {
        self.user.join("config/shortcuts.vdf")
    }

    pub fn grid(&self) -> PathBuf {
        self.user.join("config/grid")
    }

    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut command = if self.flatpak {
            let mut c = std::process::Command::new("flatpak");
            c.args(["run", "com.valvesoftware.Steam"]);
            c
        } else {
            std::process::Command::new("steam")
        };
        command.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command
    }

    /// Ask Steam to close, and wait until it has — it writes its shortcuts out as it goes, which is
    /// what is merged into next.
    pub fn shut_down(&self) -> Result<(), String> {
        self.command(&["-shutdown"]).spawn().map_err(|e| format!("could not ask Steam to close ({e})"))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        while running() {
            if Instant::now() > deadline {
                return Err("Steam did not close within a minute, so nothing was written".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        // The file is written as the last process goes; a moment for it to land.
        std::thread::sleep(Duration::from_secs(1));
        Ok(())
    }

    /// Start Steam again, on its own: it outlives Deckhand.
    pub fn start(&self) {
        let mut command = self.command(&[]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let _ = command.spawn();
    }
}

/// Of `userdata`'s accounts, the one whose config changed last.
fn last_user(userdata: &Path) -> Option<PathBuf> {
    std::fs::read_dir(userdata)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) && e.file_name() != "0")
        .map(|e| e.path())
        .filter(|p| p.join("config").is_dir())
        .max_by_key(|p| std::fs::metadata(p.join("config/localconfig.vdf")).or_else(|_| std::fs::metadata(p.join("config"))).and_then(|m| m.modified()).ok())
}

/// Whether Steam is running here.
pub fn running() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
            std::fs::read_to_string(e.path().join("comm")).is_ok_and(|comm| comm.trim() == "steam")
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Running inside Steam's Game Mode — gamescope's session — where closing Steam ends the session
/// this runs in, and the window is the whole screen.
pub fn game_mode() -> bool {
    std::env::var_os("SteamGamepadUI").is_some()
        || std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_lowercase().contains("gamescope"))
}

/// The art a shortcut shows, as RomM serves it.
#[derive(Debug, Clone, Default)]
pub struct Art {
    pub cover: Option<Vec<u8>>,
    pub banner: Option<Vec<u8>>,
    pub logo: Option<Vec<u8>>,
    /// The first screenshot, which the hero is made from — the cover, for a game with none.
    pub screenshot: Option<Vec<u8>>,
}

/// How many of the games added went in without a cover, a backdrop or a logo: RomM had none, or
/// what it sent could not be read.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gaps {
    pub covers: usize,
    pub backdrops: usize,
    pub logos: usize,
}

impl Gaps {
    /// The gaps as a sentence, for `games` games added — none when every one has all its art.
    pub fn sentence(&self, games: usize) -> Option<String> {
        let missing: Vec<(usize, &str)> =
            [(self.covers, "cover"), (self.backdrops, "backdrop"), (self.logos, "logo")].into_iter().filter(|(n, _)| *n > 0).collect();
        if missing.is_empty() {
            return None;
        }
        Some(if games == 1 {
            // "It went in without a cover or a logo."
            let names: Vec<String> = missing.iter().map(|(_, what)| format!("a {what}")).collect();
            match names.split_last() {
                Some((last, [])) => format!("It went in without {last}."),
                Some((last, first)) => format!("It went in without {} or {last}.", first.join(", ")),
                None => unreachable!("something is missing"),
            }
        } else {
            // "3 went in without a logo, 1 without a cover."
            let (n, what) = missing[0];
            let rest: String = missing[1..].iter().map(|(n, what)| format!(", {n} without a {what}")).collect();
            format!("{n} went in without a {what}{rest}.")
        })
    }
}

/// Steam's hero, the backdrop across the top of a game's page: the smaller of the sizes it asks for.
const HERO: (u32, u32) = (1920, 620);

/// The most of the hero's width its picture takes — a wider screenshot is cut to it, so the left
/// stays clear for the logo.
const HERO_PICTURE: f32 = 0.6;

/// How much of the picture, from its left edge, fades in from the wash.
const HERO_FEATHER: f32 = 0.4;

/// How far the hero's wash is darkened and its colour pushed: dark enough for any logo to read on.
const WASH_GRADE: (f32, f32) = (0.42, 1.25);

/// The icon Steam shows beside the name in its lists.
const ICON: u32 = 256;

/// The hero made of `picture`, as Home's spotlight draws it: the picture full height at the right,
/// fading in from its left edge, over a wash of its own colours — where Steam puts the logo, at the
/// bottom left.
fn hero(picture: &image::DynamicImage) -> Option<image::DynamicImage> {
    use image::imageops::{self, FilterType};
    if picture.width() == 0 || picture.height() == 0 {
        return None;
    }
    let (w, h) = HERO;
    // Shrunk to a smear of colour first, which is what makes the blur cheap, then stretched back.
    let small = crate::covers::cover_crop(picture, w as f32 / h as f32).resize_exact(96, 31, FilterType::Triangle);
    let mut wash = imageops::blur(&small.to_rgba8(), 2.0);
    crate::covers::grade(&mut wash, WASH_GRADE);
    for pixel in wash.pixels_mut() {
        pixel.0[3] = 255;
    }
    let mut hero = imageops::resize(&wash, w, h, FilterType::Triangle);

    let ratio = (picture.width() as f32 / picture.height() as f32).min(w as f32 * HERO_PICTURE / h as f32);
    let cut = crate::covers::cover_crop(picture, ratio);
    let width = ((h as f32) * ratio).round().max(1.0) as u32;
    // Pixel art stretched smoothly turns to mush: at double size or more it is stretched square,
    // as the spotlight's is.
    let filter = if h >= cut.height() * 2 { FilterType::Nearest } else { FilterType::CatmullRom };
    let mut shot = cut.resize_exact(width, h, filter).into_rgba8();
    let fade = width as f32 * HERO_FEATHER;
    for (x, _, pixel) in shot.enumerate_pixels_mut() {
        let t = ((x as f32 + 0.5) / fade).clamp(0.0, 1.0);
        pixel.0[3] = (f32::from(pixel.0[3]) * t * t * (3.0 - 2.0 * t)).round() as u8;
    }
    imageops::overlay(&mut hero, &shot, i64::from(w - width), 0);
    Some(image::DynamicImage::ImageRgba8(hero))
}

/// The icon: the middle of the cover, cut square.
fn icon(cover: &image::DynamicImage) -> image::DynamicImage {
    crate::covers::cover_crop(cover, 1.0).resize_exact(ICON, ICON, image::imageops::FilterType::CatmullRom)
}

/// Read, change and write back `steam`'s shortcuts file: the old one kept beside it, the new one
/// read back before it replaces anything.
fn rewrite(steam: &Steam, change: impl FnOnce(Vec<(String, Value)>) -> Vec<(String, Value)>) -> Result<(), String> {
    let file = steam.shortcuts_file();
    let old = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("could not read Steam's shortcuts ({e})")),
    };
    let parsed = vdf::parse(&old).map_err(|why| format!("Steam's shortcuts file has {why}, so it was left alone"))?;
    let changed = change(parsed);
    let bytes = vdf::write(&changed);
    if vdf::parse(&bytes).as_ref() != Ok(&changed) {
        return Err("the shortcuts did not read back as written, so nothing was changed".into());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if !old.is_empty() {
        std::fs::write(file.with_extension("vdf.deckhand-backup"), &old).map_err(|e| format!("could not keep a copy of Steam's shortcuts ({e})"))?;
    }
    let part = file.with_extension("vdf.deckhand-part");
    std::fs::write(&part, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &file).map_err(|e| e.to_string())
}

/// Put `games` into `steam`'s library, with their art, and say what art any of them went in
/// without. `stale` are shortcuts the same games had under an id they no longer have — a game's
/// title changed, or Deckhand moved — which go, so a game is never in twice. Steam must not be
/// running.
///
/// The art is written first: a shortcut names its icon's file, so that has to be there. Art left by
/// a shortcut that then fails to go in is never read, as Steam only looks for the ids it has.
pub fn add(steam: &Steam, games: &[(Shortcut, Art)], stale: &[u32]) -> Result<Gaps, String> {
    let grid = steam.grid();
    std::fs::create_dir_all(&grid).map_err(|e| e.to_string())?;
    let mut gaps = Gaps::default();
    let mut shortcuts = Vec::new();
    for (shortcut, art) in games {
        let id = shortcut.appid();
        let put = |name: &str, image: Option<&image::DynamicImage>| -> bool {
            image.and_then(as_png).is_some_and(|png| std::fs::write(grid.join(name), png).is_ok())
        };
        let cover = decoded(art.cover.as_deref());
        let banner = decoded(art.banner.as_deref());
        let backdrop = decoded(art.screenshot.as_deref())
            .or_else(|| cover.clone())
            .and_then(|picture| hero(&picture))
            .or_else(|| banner.clone());
        let [capsule_file, wide_file, hero_file, logo_file, icon_file] = art_files(id);
        gaps.covers += usize::from(!put(&capsule_file, cover.as_ref()));
        put(&wide_file, banner.as_ref());
        gaps.backdrops += usize::from(!put(&hero_file, backdrop.as_ref()));
        gaps.logos += usize::from(!put(&logo_file, decoded(art.logo.as_deref()).as_ref()));
        let mut shortcut = shortcut.clone();
        if put(&icon_file, cover.as_ref().map(icon).as_ref()) {
            shortcut.icon = grid.join(&icon_file).display().to_string();
        }
        shortcuts.push(shortcut);
    }
    let stale: Vec<u32> = stale.iter().copied().filter(|id| shortcuts.iter().all(|s| s.appid() != *id)).collect();
    rewrite(steam, |file| merged(without(file, &stale), &shortcuts))?;
    remove_art(steam, &stale);
    Ok(gaps)
}

/// Take the shortcuts of `ids` out of `steam`'s library, and their art. Steam must not be running.
pub fn remove(steam: &Steam, ids: &[u32]) -> Result<(), String> {
    rewrite(steam, |file| without(file, ids))?;
    remove_art(steam, ids);
    Ok(())
}

/// The files of a shortcut's art in Steam's grid folder: the portrait capsule, the wide one, the
/// hero, the logo and the icon.
fn art_files(id: u32) -> [String; 5] {
    [format!("{id}p.png"), format!("{id}.png"), format!("{id}_hero.png"), format!("{id}_logo.png"), format!("{id}_icon.png")]
}

fn remove_art(steam: &Steam, ids: &[u32]) {
    for id in ids {
        for name in art_files(*id) {
            let _ = std::fs::remove_file(steam.grid().join(name));
        }
    }
}

/// Any image RomM serves, decoded.
fn decoded(bytes: Option<&[u8]>) -> Option<image::DynamicImage> {
    image::load_from_memory(bytes?).ok()
}

/// An image as the PNG Steam's grid folder is read for.
fn as_png(image: &image::DynamicImage) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Which of this device's games are in Steam, by server and rom id — `steam-shortcuts.json` in the
/// config folder. The app id is kept so the shortcut can be taken out again even after the game's
/// title or Deckhand's path changed.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Bindings(std::collections::BTreeMap<String, u32>);

impl Bindings {
    fn file() -> PathBuf {
        crate::config::Settings::config_dir().join("steam-shortcuts.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write the bindings out — once for a change of any size, through a temp file, so adding a
    /// whole library to Steam is one write and a crash mid-write leaves the last one whole.
    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = crate::sync::write_atomic(&Self::file(), text.as_bytes());
        }
    }

    fn key(host: &str, rom_id: i64) -> String {
        format!("{host}#{rom_id}")
    }

    pub fn get(&self, host: &str, rom_id: i64) -> Option<u32> {
        self.0.get(&Self::key(host, rom_id)).copied()
    }

    /// Note a game as in Steam under `appid`. Kept in memory until [`Bindings::save`].
    pub fn bind(&mut self, host: &str, rom_id: i64, appid: u32) {
        self.0.insert(Self::key(host, rom_id), appid);
    }

    /// Note a game as no longer in Steam. Kept in memory until [`Bindings::save`].
    pub fn unbind(&mut self, host: &str, rom_id: i64) {
        self.0.remove(&Self::key(host, rom_id));
    }
}

/// Where the shortcut points: the AppImage this runs from — kept at one path across updates — or
/// this binary.
pub fn deckhand_path() -> Option<PathBuf> {
    crate::release::running_appimage().or_else(|| std::env::current_exe().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real shortcuts file's shape — one shortcut, as EmuDeck and the Kotlin app write them — built
    /// byte by byte, with each field type in it.
    fn real_file() -> Vec<u8> {
        let mut b = vec![0x00];
        b.extend(b"shortcuts\0");
        b.push(0x00);
        b.extend(b"0\0");
        b.push(0x02);
        b.extend(b"appid\0");
        b.extend(0x9090_f298u32.to_le_bytes());
        b.push(0x01);
        b.extend(b"AppName\0Grand Theft Auto: San Andreas\0");
        b.push(0x01);
        b.extend(b"Exe\0\"/home/deck/launch.sh\"\0");
        b.push(0x07);
        b.extend(b"LastPlayTimeLong\0");
        b.extend(7u64.to_le_bytes());
        b.push(0x00);
        b.extend(b"tags\0");
        b.push(0x01);
        b.extend(b"0\0favorite\0");
        b.push(0x08);
        b.push(0x08);
        b.push(0x08);
        b.push(0x08);
        b
    }

    #[test]
    fn a_shortcuts_file_reads_and_writes_back_byte_for_byte() {
        let bytes = real_file();
        let parsed = vdf::parse(&bytes).unwrap();
        assert_eq!(vdf::write(&parsed), bytes);
        let first = parsed[0].1.get("0").unwrap();
        assert_eq!(first.get("AppName"), Some(&Value::Str("Grand Theft Auto: San Andreas".into())));
        assert!(vdf::parse(&bytes[..bytes.len() - 1]).is_err(), "a cut-off file is refused, not half read");
        assert_eq!(vdf::parse(&[]).unwrap(), Vec::new(), "no file yet is no shortcuts yet");
    }

    #[test]
    fn the_app_id_is_the_one_steam_rom_manager_and_emudeck_use() {
        // From the Kotlin app's on-Deck findings: an EmuDeck entry for PCSX2.
        assert_eq!(appid("\"/home/deck/.local/share/applications/pcsx2-qt.sh\"", "pcsx2-qt") >> 31, 1);
        let one = Shortcut::for_game(Path::new("/home/deck/Deckhand.AppImage"), 7, "Chrono Trigger", "snes");
        assert_eq!(one.exe, "\"/home/deck/Deckhand.AppImage\"");
        assert_eq!(one.launch_options, "play 7");
        assert_eq!(one.appid(), appid(&one.exe, "Chrono Trigger"));
    }

    #[test]
    fn adding_keeps_every_other_shortcut_replaces_its_own_and_renumbers() {
        let file = vdf::parse(&real_file()).unwrap();
        let chrono = Shortcut::for_game(Path::new("/d/Deckhand.AppImage"), 7, "Chrono Trigger", "snes");
        let zelda = Shortcut::for_game(Path::new("/d/Deckhand.AppImage"), 8, "Zelda", "snes");
        let twice = merged(merged(file, &[chrono.clone(), zelda]), std::slice::from_ref(&chrono));
        let entries = match &twice[0].1 {
            Value::Map(e) => e.clone(),
            _ => panic!(),
        };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["0", "1", "2"], "added once each, the existing one kept");
        assert_eq!(entries[0].1.get("AppName"), Some(&Value::Str("Grand Theft Auto: San Andreas".into())));
        let gone = without(twice, &[chrono.appid()]);
        let left = match &gone[0].1 {
            Value::Map(e) => e.iter().map(|(k, v)| (k.clone(), v.get("AppName").cloned())).collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(left, [("0".to_string(), Some(Value::Str("Grand Theft Auto: San Andreas".into()))), ("1".to_string(), Some(Value::Str("Zelda".into())))]);
    }

    #[test]
    fn a_game_goes_in_with_its_art_and_comes_out_with_it_and_the_old_file_is_kept() {
        let root = std::env::temp_dir().join(format!("deckhand-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = Steam { root: root.clone(), user: root.join("userdata/42"), flatpak: false };
        std::fs::create_dir_all(steam.user.join("config")).unwrap();
        std::fs::write(steam.shortcuts_file(), real_file()).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(4, 6).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let game = Shortcut::for_game(Path::new("/d/Deckhand.AppImage"), 7, "Chrono Trigger", "snes");
        let art = Art { cover: Some(png.into_inner()), banner: None, logo: Some(b"not an image".to_vec()), screenshot: None };
        let gaps = add(&steam, &[(game.clone(), art)], &[]).unwrap();
        let id = game.appid();
        assert!(steam.grid().join(format!("{id}p.png")).is_file());
        assert!(!steam.grid().join(format!("{id}_logo.png")).exists(), "art that does not decode is left out");
        assert_eq!(gaps, Gaps { logos: 1, ..Gaps::default() }, "and said to be");
        let backdrop = image::open(steam.grid().join(format!("{id}_hero.png"))).unwrap();
        assert_eq!((backdrop.width(), backdrop.height()), HERO, "made from the cover, with no screenshot");
        let icon = steam.grid().join(format!("{id}_icon.png"));
        assert_eq!(image::open(&icon).unwrap().width(), ICON);
        let file = vdf::parse(&std::fs::read(steam.shortcuts_file()).unwrap()).unwrap();
        let entry = file[0].1.get("1").unwrap();
        assert_eq!(entry.get("icon"), Some(&Value::Str(icon.display().to_string())), "the shortcut names its icon");
        assert_eq!(std::fs::read(steam.shortcuts_file().with_extension("vdf.deckhand-backup")).unwrap(), real_file());
        let names = |steam: &Steam| -> Vec<Value> {
            let file = vdf::parse(&std::fs::read(steam.shortcuts_file()).unwrap()).unwrap();
            match &file[0].1 {
                Value::Map(e) => e.iter().filter_map(|(_, v)| v.get("AppName").cloned()).collect(),
                _ => panic!(),
            }
        };
        assert_eq!(names(&steam).len(), 2);
        remove(&steam, &[id]).unwrap();
        assert_eq!(names(&steam), [Value::Str("Grand Theft Auto: San Andreas".into())]);
        assert!(!steam.grid().join(format!("{id}p.png")).exists());
        assert!(!icon.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_game_added_under_a_new_name_replaces_its_old_shortcut() {
        let root = std::env::temp_dir().join(format!("deckhand-steam-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = Steam { root: root.clone(), user: root.join("userdata/42"), flatpak: false };
        std::fs::create_dir_all(steam.user.join("config")).unwrap();
        std::fs::write(steam.shortcuts_file(), real_file()).unwrap();
        let before = Shortcut::for_game(Path::new("/d/Deckhand.AppImage"), 7, "Chrono Trigger", "snes");
        let after = Shortcut::for_game(Path::new("/e/Deckhand.AppImage"), 7, "Chrono Trigger", "snes");
        add(&steam, &[(before.clone(), Art::default())], &[]).unwrap();
        std::fs::write(steam.grid().join(format!("{}_logo.png", before.appid())), b"old").unwrap();
        // Added again from where Deckhand is now: the same id, as well as the old, is fine to pass.
        add(&steam, &[(after.clone(), Art::default())], &[before.appid(), after.appid()]).unwrap();
        let file = vdf::parse(&std::fs::read(steam.shortcuts_file()).unwrap()).unwrap();
        let Value::Map(entries) = &file[0].1 else { panic!("no shortcuts map") };
        let ids: Vec<Option<u32>> = entries.iter().map(|(_, e)| id_of(e)).collect();
        assert_eq!(ids, [Some(0x9090_f298), Some(after.appid())], "the player's own kept, the old one gone");
        assert!(!steam.grid().join(format!("{}_logo.png", before.appid())).exists(), "with its art");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hero_is_the_picture_at_the_right_over_a_darker_wash_of_it() {
        // A SNES frame of one colour: the wash is the same colour, darker.
        let frame = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(256, 224, image::Rgb([200, 40, 40])));
        let made = hero(&frame).unwrap().into_rgba8();
        assert_eq!(made.dimensions(), HERO);
        let (w, h) = HERO;
        let right = made.get_pixel(w - 1, h / 2).0;
        let left = made.get_pixel(0, h / 2).0;
        assert_eq!(right, [200, 40, 40, 255], "the picture itself, at the right edge");
        assert!(left[0] < 120 && left[3] == 255, "the wash, darker and opaque: {left:?}");
        // A wide screenshot is cut, so the left of the hero stays clear for the logo.
        let wide = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1920, 600, image::Rgb([200, 40, 40])));
        let made = hero(&wide).unwrap().into_rgba8();
        assert!(made.get_pixel(w / 4, h / 2).0[0] < 120, "no picture in the left third");
    }

    /// Steam's hero and icon made of each picture named in `DECKHAND_STEAM_ART` (`:`-separated),
    /// drawn into `DECKHAND_SNAPSHOTS` to look at.
    #[test]
    #[ignore = "reads pictures named in DECKHAND_STEAM_ART"]
    fn draws_steam_art_for_pictures_named_in_deckhand_steam_art() {
        let out = PathBuf::from(std::env::var("DECKHAND_SNAPSHOTS").expect("DECKHAND_SNAPSHOTS"));
        for path in std::env::var("DECKHAND_STEAM_ART").expect("DECKHAND_STEAM_ART").split(':') {
            let picture = image::load_from_memory(&std::fs::read(path).unwrap()).unwrap();
            let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
            hero(&picture).unwrap().save(out.join(format!("{name}-hero.png"))).unwrap();
            icon(&picture).save(out.join(format!("{name}-icon.png"))).unwrap();
        }
    }

    #[test]
    fn missing_art_is_said_in_one_sentence() {
        assert_eq!(Gaps::default().sentence(1), None);
        assert_eq!(Gaps { logos: 1, ..Gaps::default() }.sentence(1).as_deref(), Some("It went in without a logo."));
        assert_eq!(Gaps { covers: 1, logos: 1, ..Gaps::default() }.sentence(1).as_deref(), Some("It went in without a cover or a logo."));
        assert_eq!(
            Gaps { covers: 1, backdrops: 0, logos: 3 }.sentence(40).as_deref(),
            Some("1 went in without a cover, 3 without a logo.")
        );
    }

    /// A real `shortcuts.vdf`, named by `DECKHAND_VDF`: read and written back byte for byte, and
    /// every shortcut's id recomputed from its program and name.
    #[test]
    #[ignore = "reads a file named in DECKHAND_VDF"]
    fn a_real_shortcuts_file_round_trips_and_its_ids_are_reproduced() {
        let bytes = std::fs::read(std::env::var("DECKHAND_VDF").expect("DECKHAND_VDF")).unwrap();
        let parsed = vdf::parse(&bytes).unwrap();
        assert_eq!(vdf::write(&parsed), bytes, "byte for byte");
        let Value::Map(entries) = &parsed[0].1 else { panic!("no shortcuts map") };
        for (_, entry) in entries {
            let (Some(Value::Str(exe)), Some(Value::Str(name)), Some(id)) = (entry.get("Exe"), entry.get("AppName"), id_of(entry)) else { continue };
            println!("{name}: stored {id:#x}, computed {:#x}", appid(exe, name));
            assert_eq!(appid(exe, name), id);
        }
    }

    #[test]
    fn bindings_are_written_once_and_read_back() {
        let mut bindings = Bindings::default();
        bindings.bind("https://romm.local", 7, 0x8000_0007);
        bindings.bind("https://romm.local", 8, 0x8000_0008);
        bindings.unbind("https://romm.local", 8);
        bindings.save();
        let back = Bindings::load();
        assert_eq!(back.get("https://romm.local", 7), Some(0x8000_0007));
        assert_eq!(back.get("https://romm.local", 8), None);
    }
}
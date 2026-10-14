//! argosy-sigil, RomM's own library for emulator saves, as this app calls it.
//!
//! Sigil knows, for each emulator, which files in a save folder are one game's: the `.srm` beside a
//! cartridge, the folders a PSP or PS3 game keeps under its disc id, one game's saves on a PS2 memory
//! card or a Dolphin GCI folder every game shares. It reads the game's own id out of its file, a CHD
//! included, so nothing is matched by title. It packs a game's saves into the one *unit* that travels
//! to RomM, hashed the way RomM hashes it, and puts a unit back without touching any other game's
//! saves — the same units RomM's Android app (Argosy) syncs, in the same `autosave` slot.
//!
//! This module is the safe face of the C API in [`ffi`]: an owned [`Identity`], and a [`Request`]
//! that resolves, collects and restores one game's saves. The C never touches the filesystem on its
//! own here — the root is listed, and files opened, written and removed, by this side, through
//! callbacks that refuse any path leaving the save root.
//!
//! Sigil *syncs* some systems — PlayStation, the Nintendo consoles with ids, Saturn, Dreamcast —
//! building and unpacking their units itself. The rest it only *locates*: it names a game's files,
//! and the client packs them by the unit's shape, one file as it is or a zip of them, as its docs
//! lay out. [`Request::collect`] and [`Request::restore`] do that here when sigil declines, so every
//! system goes through the same two calls.
//!
//! Every call is synchronous and does file I/O: call from a blocking context.

mod ffi;

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// What sigil said no with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: i32,
    pub message: String,
    /// The file, save or member at fault, when sigil names one.
    pub problem: Option<String>,
}

impl Error {
    fn new(code: c_int, problem: Option<String>) -> Self {
        Error { code, message: strerror(code), problem: problem.filter(|p| !p.is_empty()) }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.problem {
            Some(problem) => write!(f, "{} ({})", self.message, problem.lines().next().unwrap_or(problem)),
            None => f.write_str(&self.message),
        }
    }
}

fn strerror(code: c_int) -> String {
    // SAFETY: sigil returns a pointer to a static string for any code.
    unsafe { text_at(ffi::sigil_strerror(code)) }
}

/// The library's version, as it reports it.
#[cfg(test)]
pub fn version() -> String {
    // SAFETY: a static string.
    unsafe { text_at(ffi::sigil_version()) }
}

/// A game as sigil read it out of its file: its id, the name its saves go by, and the cart facts
/// that change what a save holds.
#[derive(Clone, Copy)]
pub struct Identity(ffi::sigil_result);

impl Identity {
    /// The game's id — `SLUS-20946`, `ULUS10538`. Empty for a cartridge sigil reads only facts from.
    pub fn title_id(&self) -> String {
        text(&self.0.title_id)
    }

    /// The name its saves go by on disk — PCSX2's `BASLUS-20946`, PPSSPP's `ULUS10538`.
    pub fn save_id(&self) -> String {
        text(&self.0.save_id)
    }

    /// The platform sigil took it for, by its slug.
    pub fn platform(&self) -> String {
        // SAFETY: a static string for any value.
        unsafe { text_at(ffi::sigil_platform_to_slug(self.0.platform)) }
    }

    /// A made-up identity, for tests that have no game file to read one from.
    #[cfg(test)]
    pub fn made_up(platform: &str, title_id: &str, save_id: &str, usage: i32) -> Identity {
        let slug = CString::new(platform).unwrap();
        // SAFETY: an all-zero sigil_result is a valid empty one; the fields are filled below.
        let mut raw: ffi::sigil_result = unsafe { std::mem::zeroed() };
        raw.struct_version = ffi::SIGIL_RESULT_V4;
        // SAFETY: a NUL-terminated string.
        raw.platform = unsafe { ffi::sigil_platform_from_slug(slug.as_ptr()) };
        fill(&mut raw.title_id, title_id);
        fill(&mut raw.raw_serial, title_id);
        fill(&mut raw.save_id, save_id);
        raw.source = ffi::SIGIL_SOURCE_BINARY;
        raw.usage = usage;
        Identity(raw)
    }
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("platform", &self.platform())
            .field("title_id", &self.title_id())
            .field("save_id", &self.save_id())
            .finish()
    }
}

/// The game in `path`, read for `platform` (a sigil slug — see [`platform_of`]) when that is known.
///
/// Remembered for as long as the file is unchanged, so a sync over every game on the device reads
/// each disc once a session, not once a sync.
pub fn identify(path: &Path, platform: Option<&str>) -> Result<Identity, Error> {
    type Key = (PathBuf, Option<String>, u64, Option<SystemTime>);
    static SEEN: OnceLock<Mutex<HashMap<Key, Result<Identity, Error>>>> = OnceLock::new();
    let meta = fs::metadata(path).map_err(|e| Error { code: -2, message: e.to_string(), problem: Some(path.display().to_string()) })?;
    let key: Key = (path.to_path_buf(), platform.map(str::to_string), meta.len(), meta.modified().ok());
    let seen = SEEN.get_or_init(Default::default);
    if let Some(known) = seen.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return known.clone();
    }
    let found = extract(path, platform);
    seen.lock().unwrap_or_else(|e| e.into_inner()).insert(key, found.clone());
    found
}

fn extract(path: &Path, platform: Option<&str>) -> Result<Identity, Error> {
    let file = c_path(path)?;
    let hint = match platform {
        Some(slug) => {
            let slug = CString::new(slug).unwrap_or_default();
            // SAFETY: a NUL-terminated string.
            unsafe { ffi::sigil_platform_from_slug(slug.as_ptr()) }
        }
        None => ffi::SIGIL_PLATFORM_AUTO,
    };
    // SAFETY: zeroed is a valid empty result, which sigil fills in.
    let mut out: ffi::sigil_result = unsafe { std::mem::zeroed() };
    out.struct_version = ffi::SIGIL_RESULT_V4;
    // SAFETY: valid path, no options, and a result to write into.
    let rc = unsafe { ffi::sigil_extract_from_path(file.as_ptr(), hint, std::ptr::null(), &mut out) };
    if rc != ffi::SIGIL_OK {
        return Err(Error::new(rc, Some(path.display().to_string())));
    }
    Ok(Identity(out))
}

/// The sigil platform slug for one of ES-DE's systems — the folder names this app files games
/// under. None for a system sigil has no platform of its own for, whose saves its default layout
/// covers from the content file's name alone.
pub fn platform_of(system: &str) -> Option<&'static str> {
    Some(match system.to_ascii_lowercase().as_str() {
        "psx" => "psx",
        "ps2" => "ps2",
        "ps3" => "ps3",
        "psp" => "psp",
        "psvita" => "psvita",
        "gb" => "gb",
        "gbc" => "gbc",
        "snes" | "sfc" | "snesna" => "snes",
        "n64" => "n64",
        "gc" => "gamecube",
        "wii" => "wii",
        "wiiu" => "wiiu",
        "n3ds" => "3ds",
        "switch" => "switch",
        "dreamcast" => "dreamcast",
        "saturn" | "saturnjp" => "saturn",
        "segacd" | "megacd" | "megacdjp" => "segacd",
        "fds" => "fds",
        "xbox" => "xbox",
        "xbox360" => "xbox360",
        _ => return None,
    })
}

/// How a unit holds a game's saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Nothing there.
    None,
    /// One file, as it is on disk.
    Single,
    /// Several files, zipped flat.
    Multi,
    /// The game's save folders, zipped.
    Folder,
}

impl Shape {
    fn from_c(shape: c_int) -> Shape {
        match shape {
            ffi::SIGIL_SAVE_SHAPE_SINGLE => Shape::Single,
            ffi::SIGIL_SAVE_SHAPE_MULTI => Shape::Multi,
            ffi::SIGIL_SAVE_SHAPE_FOLDER => Shape::Folder,
            _ => Shape::None,
        }
    }
}

/// One file of a game's saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Where it is, or would be.
    pub path: PathBuf,
    /// Its name in the unit's zip.
    pub entry: String,
}

/// Which files are a game's saves — see [`Request::resolve`].
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The content's stem for a layout of files, the game's save id for one of folders.
    pub key: String,
    /// The unit's name — empty when nothing of the game's is there to name it after.
    pub artifact: String,
    pub shape: Shape,
    /// The game's files that are there.
    pub members: Vec<Member>,
    /// The ones that are not there yet, where the emulator would write them.
    pub expected: Vec<Member>,
    /// Files every game shares — a memory card, a backup RAM volume — that the game's saves sit on.
    pub shared: Vec<PathBuf>,
}

/// A game's saves, packed — see [`Request::collect`].
#[derive(Debug, Clone)]
pub struct Collected {
    /// The name the unit travels under. Not always [`Resolved::artifact`]: an N64 game's `.srm`
    /// resolves as itself, and is collected as a zip of its save types.
    pub artifact: String,
    /// The unit, or None when the game has no saves here.
    pub data: Option<Vec<u8>>,
    /// RomM's `content_hash` of the unit.
    pub content_hash: String,
}

/// What putting a unit back did — see [`Request::restore`].
#[derive(Debug, Clone, Default)]
pub struct Restored {
    /// Every file written, and every one taken away.
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

/// One game's saves, as sigil is asked about them.
#[derive(Debug, Clone)]
pub struct Request {
    /// The emulator: a libretro core's id (`snes9x`, `swanstation`), or a standalone emulator's
    /// layout id (`pcsx2_standalone`).
    pub layout: String,
    /// The sigil platform slug, when the game has one.
    pub platform: Option<String>,
    /// The file the emulator loads, as it loads it — what a save named after the content takes its
    /// name from.
    pub content: PathBuf,
    pub identity: Option<Identity>,
    /// The core options or emulator settings the layout's files depend on — see [`layouts`].
    pub options: Vec<(String, String)>,
    /// The folder the layout's paths are relative to.
    pub root: PathBuf,
    /// The user profile whose saves these are, on an emulator that keeps several.
    pub profile: Option<String>,
    /// Every id the game's saves may carry — each disc of a set.
    pub game_ids: Vec<String>,
}

/// The C side of a [`Request`], alive for as long as the request struct points into it.
struct Prepared {
    layout: CString,
    platform: Option<CString>,
    content: CString,
    identity: Option<ffi::sigil_result>,
    // The strings the pointer arrays point into, kept alive with them.
    _option_text: Vec<(CString, CString)>,
    options: Vec<ffi::sigil_save_option>,
    _listing_text: Vec<CString>,
    listing: Vec<*const c_char>,
    root: CString,
    profile: Option<CString>,
    _game_id_text: Vec<CString>,
    game_ids: Vec<*const c_char>,
    open: Box<PathBuf>,
}

impl Prepared {
    fn new(request: &Request) -> Result<Prepared, Error> {
        let cstring = |s: &str| CString::new(s).map_err(|_| Error { code: -1, message: format!("{s:?} has a NUL in it"), problem: None });
        let option_text: Vec<(CString, CString)> =
            request.options.iter().map(|(k, v)| Ok((cstring(k)?, cstring(v)?))).collect::<Result<_, Error>>()?;
        let options = option_text.iter().map(|(k, v)| ffi::sigil_save_option { key: k.as_ptr(), value: v.as_ptr() }).collect();
        let listing_text: Vec<CString> = listing(&request.root, &request.layout).iter().filter_map(|p| CString::new(p.as_str()).ok()).collect();
        let listing = listing_text.iter().map(|p| p.as_ptr()).collect();
        let game_id_text: Vec<CString> = request.game_ids.iter().filter(|id| !id.is_empty()).filter_map(|id| CString::new(id.as_str()).ok()).collect();
        let game_ids = game_id_text.iter().map(|id| id.as_ptr()).collect();
        Ok(Prepared {
            layout: cstring(&request.layout)?,
            platform: request.platform.as_deref().map(cstring).transpose()?,
            content: c_path(&request.content)?,
            identity: request.identity.map(|i| i.0),
            _option_text: option_text,
            options,
            _listing_text: listing_text,
            listing,
            root: c_path(&request.root)?,
            profile: request.profile.as_deref().map(cstring).transpose()?,
            _game_id_text: game_id_text,
            game_ids,
            open: Box::new(request.root.clone()),
        })
    }

    fn save_request(&self) -> ffi::sigil_save_request {
        ffi::sigil_save_request {
            struct_version: ffi::SIGIL_SAVE_REQUEST_V1,
            layout: self.layout.as_ptr(),
            platform: self.platform.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            content_path: self.content.as_ptr(),
            result: self.identity.as_ref().map_or(std::ptr::null(), |r| r as *const ffi::sigil_result),
            features: 0,
            options: self.options.as_ptr(),
            option_count: self.options.len(),
            listing: self.listing.as_ptr(),
            listing_count: self.listing.len(),
            open: Some(open_member),
            open_ctx: (&*self.open as *const PathBuf).cast_mut().cast::<c_void>(),
            root_path: self.root.as_ptr(),
            profile: self.profile.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
        }
    }

    fn sync_request(&self, state: Option<&[u8]>, writer: Option<&mut Writer>) -> ffi::sigil_sync_request {
        let (write, remove, ctx): (ffi::sigil_save_write_fn, ffi::sigil_save_remove_fn, *mut c_void) = match writer {
            Some(writer) => (Some(write_member), Some(remove_member), (writer as *mut Writer).cast::<c_void>()),
            None => (None, None, std::ptr::null_mut()),
        };
        ffi::sigil_sync_request {
            struct_version: ffi::SIGIL_SYNC_REQUEST_V1,
            save: self.save_request(),
            game_ids: if self.game_ids.is_empty() { std::ptr::null() } else { self.game_ids.as_ptr() },
            game_id_count: self.game_ids.len(),
            // The game runs outside this app's hands — RetroDECK's, ES-DE's — so a shared volume is
            // never swapped for one holding only this game's saves.
            mode: ffi::SIGIL_SYNC_UNMANAGED,
            state: state.map_or(std::ptr::null(), <[u8]>::as_ptr),
            state_len: state.map_or(0, <[u8]>::len),
            // What to keep is this app's decision, made against RomM before anything is restored —
            // see `sync::session` — so sigil's own check is not a second veto.
            overwrite_local: 1,
            write,
            write_ctx: ctx,
            claimed: std::ptr::null(),
            claimed_count: 0,
            remove,
            companions: std::ptr::null(),
            companion_count: 0,
            repair: 0,
        }
    }
}

impl Request {
    /// Which files are the game's saves, and where missing ones would go. Reads names only.
    pub fn resolve(&self) -> Result<Resolved, Error> {
        let prepared = Prepared::new(self)?;
        let mut request = prepared.save_request();
        request.open = None;
        let mut out: *mut ffi::sigil_save_unit = std::ptr::null_mut();
        // SAFETY: the request points into `prepared`, which outlives the call.
        let rc = unsafe { ffi::sigil_save_resolve(&request, &mut out) };
        if rc != ffi::SIGIL_OK {
            // SAFETY: freeing what sigil allocated, or nothing.
            unsafe { ffi::sigil_save_unit_free(out) };
            return Err(Error::new(rc, None));
        }
        // SAFETY: on success `out` is a unit sigil allocated, read here and freed below.
        let unit = unsafe { &*out };
        let member = |m: &ffi::sigil_save_member| Member { path: self.root.join(text(&m.path)), entry: text(&m.entry) };
        // SAFETY: arrays of the counts sigil gave.
        let members = unsafe { slice(unit.members, unit.member_count) }.iter().map(member).collect();
        let expected = unsafe { slice(unit.expected, unit.expected_count) }.iter().map(member).collect();
        let shared = unsafe { slice(unit.unkeyed, unit.unkeyed_count) }.iter().map(|p| self.root.join(text(p))).collect();
        let resolved = Resolved { key: text(&unit.key), artifact: text(&unit.artifact), shape: Shape::from_c(unit.shape), members, expected, shared };
        // SAFETY: the unit sigil allocated, not used after this.
        unsafe { ffi::sigil_save_unit_free(out) };
        Ok(resolved)
    }

    /// The game's saves as the unit that travels to RomM. `state` is what the last call handed back.
    pub fn collect(&self, state: Option<&[u8]>) -> Result<Collected, Error> {
        let prepared = Prepared::new(self)?;
        let request = prepared.sync_request(state, None);
        let mut out: *mut ffi::sigil_sync_result = std::ptr::null_mut();
        // SAFETY: the request points into `prepared` and `state`, which outlive the call.
        let rc = unsafe { ffi::sigil_collect(&request, &mut out) };
        let result = Outcome::take(rc, out);
        if rc == ffi::SIGIL_ERR_UNSUPPORTED_FORMAT {
            return self.pack_located();
        }
        // No saves at all is an empty unit, not a failure: a game never played here.
        if rc == ffi::SIGIL_ERR_NOT_FOUND {
            return Ok(Collected { artifact: result.artifact, data: None, content_hash: String::new() });
        }
        if rc != ffi::SIGIL_OK {
            return Err(Error::new(rc, Some(result.problem)));
        }
        Ok(Collected { artifact: result.artifact, data: result.data, content_hash: result.content_hash })
    }

    /// Put `unit` — RomM's — in place of the game's saves here, writing only this game's files and
    /// only under the save root.
    pub fn restore(&self, unit: &[u8], state: Option<&[u8]>) -> Result<Restored, Error> {
        let prepared = Prepared::new(self)?;
        let mut writer = Writer { root: self.root.clone(), written: Vec::new(), removed: Vec::new(), error: None };
        let request = prepared.sync_request(state, Some(&mut writer));
        let mut out: *mut ffi::sigil_sync_result = std::ptr::null_mut();
        // SAFETY: the request points into `prepared`, `state` and `writer`, which outlive the call.
        let rc = unsafe { ffi::sigil_restore(&request, unit.as_ptr(), unit.len(), &mut out) };
        let result = Outcome::take(rc, out);
        if rc == ffi::SIGIL_ERR_UNSUPPORTED_FORMAT && writer.written.is_empty() && writer.removed.is_empty() {
            return self.unpack_located(unit);
        }
        if let Some(error) = writer.error {
            return Err(Error { code: rc, message: error, problem: None });
        }
        if rc != ffi::SIGIL_OK {
            return Err(Error::new(rc, Some(result.problem)));
        }
        Ok(Restored { written: writer.written, removed: writer.removed })
    }
}

impl Request {
    /// [`Request::collect`] for a system sigil only locates: the game's files packed by the unit's
    /// shape, hashed as RomM hashes the upload.
    fn pack_located(&self) -> Result<Collected, Error> {
        let resolved = self.resolve()?;
        if resolved.members.is_empty() {
            return Ok(Collected { artifact: resolved.artifact, data: None, content_hash: String::new() });
        }
        let io = |e: std::io::Error, path: &Path| Error { code: -2, message: format!("can't read {}: {e}", path.display()), problem: None };
        let data = match resolved.shape {
            Shape::Single => {
                let path = &resolved.members[0].path;
                fs::read(path).map_err(|e| io(e, path))?
            }
            _ => {
                let mut set = Vec::with_capacity(resolved.members.len());
                for member in &resolved.members {
                    set.push((member.entry.clone(), fs::read(&member.path).map_err(|e| io(e, &member.path))?));
                }
                zip_of(&set).map_err(|e| Error { code: -2, message: format!("could not zip it ({e})"), problem: None })?
            }
        };
        let content_hash = crate::sync::hash::of_upload(&data);
        Ok(Collected { artifact: resolved.artifact, data: Some(data), content_hash })
    }

    /// [`Request::restore`] for a system sigil only locates: each file of `unit` written where the
    /// member it is goes — one file as it is, a zip by its entries' names.
    fn unpack_located(&self, unit: &[u8]) -> Result<Restored, Error> {
        let resolved = self.resolve()?;
        let places: Vec<&Member> = resolved.members.iter().chain(&resolved.expected).collect();
        let fail = |message: String| Error { code: -2, message, problem: None };
        let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        match crate::sync::hash::zip_entries(unit) {
            // A zip: each entry to the member of that name, or for a folder unit, under the save
            // folders' parent, which every member's path ends in its entry beneath.
            Some(entries) => {
                for (entry, bytes) in entries {
                    let path = match places.iter().find(|m| m.entry.eq_ignore_ascii_case(&entry)) {
                        Some(member) => member.path.clone(),
                        None if resolved.shape == Shape::Folder => {
                            let base = places
                                .iter()
                                .find_map(|m| m.path.to_str()?.strip_suffix(m.entry.as_str()).map(PathBuf::from))
                                .ok_or_else(|| fail(format!("nowhere here takes {entry}")))?;
                            inside(&base, &entry).ok_or_else(|| fail(format!("{entry} would land outside the save folder")))?
                        }
                        None => return Err(fail(format!("nowhere here takes {entry}"))),
                    };
                    files.push((path, bytes));
                }
            }
            None => {
                let member = places.first().ok_or_else(|| fail("the emulator keeps no file for this save".into()))?;
                files.push((member.path.clone(), unit.to_vec()));
            }
        }
        let mut restored = Restored::default();
        for (path, bytes) in files {
            if !path.starts_with(&self.root) {
                return Err(fail(format!("{} is outside the save folder", path.display())));
            }
            crate::sync::write_atomic(&path, &bytes).map_err(|e| fail(format!("can't write {}: {e}", path.display())))?;
            restored.written.push(path);
        }
        Ok(restored)
    }
}

/// `set` zipped, each file at the name it is given.
fn zip_of(set: &[(String, Vec<u8>)]) -> zip::result::ZipResult<Vec<u8>> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in set {
        zip.start_file(name.as_str(), options)?;
        zip.write_all(bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// A sync result, copied out of sigil's memory and freed.
struct Outcome {
    artifact: String,
    data: Option<Vec<u8>>,
    content_hash: String,
    problem: String,
}

impl Outcome {
    fn take(_rc: c_int, out: *mut ffi::sigil_sync_result) -> Outcome {
        if out.is_null() {
            return Outcome { artifact: String::new(), data: None, content_hash: String::new(), problem: String::new() };
        }
        // SAFETY: a result sigil allocated; its arrays are of the lengths it gives.
        let outcome = unsafe {
            let r = &*out;
            Outcome {
                artifact: text(&r.artifact),
                data: (!r.data.is_null()).then(|| slice(r.data, r.len).to_vec()),
                content_hash: text(&r.content_hash),
                problem: text(&r.problem),
            }
        };
        // SAFETY: freed once, not used after.
        unsafe { ffi::sigil_sync_result_free(out) };
        outcome
    }
}

/// Where a restore writes, and what it did.
struct Writer {
    root: PathBuf,
    written: Vec<PathBuf>,
    removed: Vec<PathBuf>,
    error: Option<String>,
}

/// `relative` under `root`, when it stays there: no root, no drive, no `..`.
fn inside(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative.trim_end_matches('/'));
    relative.components().all(|c| matches!(c, Component::Normal(_))).then(|| root.join(relative))
}

unsafe extern "C" fn open_member(ctx: *mut c_void, relative: *const c_char) -> *mut ffi::sigil_io {
    // SAFETY: `ctx` is the request's root, alive for the call; `relative` a string sigil gives.
    let (root, relative) = unsafe { (&*ctx.cast::<PathBuf>(), text_at(relative)) };
    let Some(path) = inside(root, &relative) else { return std::ptr::null_mut() };
    match c_path(&path) {
        // SAFETY: a valid path; sigil owns and closes what it opens.
        Ok(path) => unsafe { ffi::sigil_io_open_file(path.as_ptr()) },
        Err(_) => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn write_member(ctx: *mut c_void, relative: *const c_char, data: *const u8, len: usize) -> c_int {
    // SAFETY: `ctx` is the restore's writer, alive and not otherwise borrowed during the call.
    let (writer, relative) = unsafe { (&mut *ctx.cast::<Writer>(), text_at(relative)) };
    let Some(path) = inside(&writer.root, &relative) else {
        writer.error = Some(format!("{relative} would land outside the save folder"));
        return -1;
    };
    // SAFETY: `len` bytes sigil holds for the call.
    let bytes = unsafe { slice(data, len) };
    match crate::sync::write_atomic(&path, bytes) {
        Ok(()) => {
            writer.written.push(path);
            0
        }
        Err(e) => {
            writer.error = Some(format!("can't write {}: {e}", path.display()));
            -1
        }
    }
}

unsafe extern "C" fn remove_member(ctx: *mut c_void, relative: *const c_char) -> c_int {
    // SAFETY: as `write_member`.
    let (writer, relative) = unsafe { (&mut *ctx.cast::<Writer>(), text_at(relative)) };
    let Some(path) = inside(&writer.root, &relative) else {
        writer.error = Some(format!("{relative} is outside the save folder"));
        return -1;
    };
    // A path ending in `/` is a folder sigil emptied first: a dropped PCSX2 save folder.
    let gone = if relative.ends_with('/') { fs::remove_dir(&path) } else { fs::remove_file(&path) };
    match gone {
        Ok(()) => {
            writer.removed.push(path);
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => {
            writer.error = Some(format!("can't remove {}: {e}", path.display()));
            -1
        }
    }
}

/// The files sigil needs to see under `root` for `layout`, relative and `/`-separated: those
/// straight in the root, and everything under the subfolders the layout writes into. Nothing else is
/// walked — an emulator's own folder holds caches and textures that are no save's.
///
/// This app's own working files are left out, and so is a file syncer's conflict copy: either would
/// read as one of the game's saves.
fn listing(root: &Path, layout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if ours(&name) && entry.path().is_file() {
            out.push(name);
        }
    }
    for sub in subdirs(layout) {
        walk(&root.join(&sub), &sub, &mut out, 0);
    }
    out.sort();
    out.dedup();
    out
}

fn walk(dir: &Path, relative: &str, out: &mut Vec<String>, depth: usize) {
    // Links are followed — RetroDECK links its emulators' save folders into its own tree — so a
    // loop is cut off rather than walked forever.
    if depth > 12 {
        return;
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !ours(&name) {
            continue;
        }
        let path = entry.path();
        let child = format!("{relative}/{name}");
        if path.is_dir() {
            walk(&path, &child, out, depth + 1);
        } else if path.is_file() {
            out.push(child);
        }
    }
}

/// Whether a file is anyone's save rather than this app's leftover or a file syncer's copy.
fn ours(name: &str) -> bool {
    !name.contains(".deckhand-") && !name.contains("sync-conflict")
}

/// The subfolders under a save root that `layout` writes into.
pub fn subdirs(layout: &str) -> Vec<String> {
    let Ok(layout) = CString::new(layout) else { return Vec::new() };
    let mut out = [std::ptr::null::<c_char>(); 16];
    // SAFETY: room for 16, and sigil writes at most that many static strings.
    let n = unsafe { ffi::sigil_save_layout_subdirs(layout.as_ptr(), out.as_mut_ptr(), out.len()) };
    // SAFETY: static strings.
    out[..n.min(out.len())].iter().map(|p| unsafe { text_at(*p) }).collect()
}

/// An option a layout's files depend on, and the value it counts as when it is not set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutOption {
    pub key: String,
    pub default: String,
}

/// One row of sigil's layout table.
#[derive(Debug, Clone)]
pub struct Layout {
    pub id: String,
    pub options: Vec<LayoutOption>,
}

/// Every layout row for `platform`, or every row at all.
pub fn layouts(platform: Option<&str>) -> Vec<Layout> {
    let slug = platform.and_then(|p| CString::new(p).ok());
    let mut out: *mut ffi::sigil_layout_info = std::ptr::null_mut();
    let mut count = 0usize;
    // SAFETY: a slug or NULL, and places for the array and its length.
    let rc = unsafe { ffi::sigil_layouts(slug.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()), &mut out, &mut count) };
    if rc != ffi::SIGIL_OK {
        return Vec::new();
    }
    // SAFETY: `count` rows sigil allocated, freed below.
    let rows = unsafe { slice(out, count) }
        .iter()
        .map(|row| Layout {
            id: text(&row.id),
            options: row.options[..row.option_count.min(ffi::SIGIL_LAYOUT_OPTIONS)]
                .iter()
                .map(|o| LayoutOption { key: text(&o.key), default: text(&o.default_value) })
                .collect(),
        })
        .collect();
    // SAFETY: freed once.
    unsafe { ffi::sigil_layouts_free(out) };
    rows
}

/// The layout row sigil uses for `layout` — its own, or the libretro default an id without one gets.
pub fn layout(layout: &str) -> Option<Layout> {
    static ALL: OnceLock<Vec<Layout>> = OnceLock::new();
    let all = ALL.get_or_init(|| layouts(None));
    all.iter().find(|l| l.id == layout).or_else(|| all.iter().find(|l| l.id == "libretro")).cloned()
}

// ---- Strings and arrays across the boundary ----------------------------------------------------

fn c_path(path: &Path) -> Result<CString, Error> {
    let text = path.to_str().ok_or_else(|| Error { code: -1, message: format!("{} is not UTF-8", path.display()), problem: None })?;
    CString::new(text).map_err(|_| Error { code: -1, message: format!("{} has a NUL in it", path.display()), problem: None })
}

/// A fixed buffer's text, up to its first NUL.
fn text(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A C string's text, or empty for NULL.
///
/// # Safety
/// `p` is NULL or a NUL-terminated string alive for the call.
unsafe fn text_at(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// `len` items at `p`, or none for NULL.
///
/// # Safety
/// `p` is NULL or points to `len` initialised items alive for the borrow.
unsafe fn slice<'a, T>(p: *const T, len: usize) -> &'a [T] {
    if p.is_null() || len == 0 {
        return &[];
    }
    // SAFETY: as the caller promises.
    unsafe { std::slice::from_raw_parts(p, len) }
}

#[cfg(test)]
fn fill(buf: &mut [c_char], value: &str) {
    for (slot, byte) in buf.iter_mut().zip(value.bytes().chain(std::iter::once(0))) {
        *slot = byte as c_char;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deckhand-sigil-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn md5(bytes: &[u8]) -> String {
        crate::sync::hash::file(bytes)
    }

    #[test]
    fn the_bindings_lay_every_struct_out_as_the_header_does() {
        use std::mem::{offset_of, size_of};
        // SAFETY: plain functions returning sizes.
        unsafe {
            assert_eq!(size_of::<ffi::sigil_result>(), ffi::deckhand_sigil_sizeof_result());
            assert_eq!(size_of::<ffi::sigil_save_member>(), ffi::deckhand_sigil_sizeof_save_member());
            assert_eq!(size_of::<ffi::sigil_save_request>(), ffi::deckhand_sigil_sizeof_save_request());
            assert_eq!(size_of::<ffi::sigil_save_unit>(), ffi::deckhand_sigil_sizeof_save_unit());
            assert_eq!(size_of::<ffi::sigil_layout_info>(), ffi::deckhand_sigil_sizeof_layout_info());
            assert_eq!(size_of::<ffi::sigil_sync_request>(), ffi::deckhand_sigil_sizeof_sync_request());
            assert_eq!(size_of::<ffi::sigil_sync_result>(), ffi::deckhand_sigil_sizeof_sync_result());
            assert_eq!(offset_of!(ffi::sigil_sync_result, unowned_changed), ffi::deckhand_sigil_offsetof_sync_result_unowned_changed());
            assert_eq!(offset_of!(ffi::sigil_sync_request, repair), ffi::deckhand_sigil_offsetof_sync_request_repair());
            assert_eq!(offset_of!(ffi::sigil_save_unit, alternate_count), ffi::deckhand_sigil_offsetof_save_unit_alternate_count());
            assert_eq!(offset_of!(ffi::sigil_result, n64_md5_n64), ffi::deckhand_sigil_offsetof_result_n64_md5_n64());
        }
        assert!(!version().is_empty());
    }

    #[test]
    fn a_cartridge_save_is_its_srm_as_it_is_and_goes_back_where_the_core_reads_it() {
        let root = temp("srm");
        let save = b"chrono trigger, chapter 3".to_vec();
        fs::write(root.join("Chrono Trigger (USA).srm"), &save).unwrap();
        fs::write(root.join("Secret of Mana (USA).srm"), b"another game").unwrap();
        let request = Request {
            layout: "snes9x".into(),
            platform: Some("snes".into()),
            content: PathBuf::from("/roms/snes/Chrono Trigger (USA).sfc"),
            identity: None,
            options: Vec::new(),
            root: root.clone(),
            profile: None,
            game_ids: Vec::new(),
        };
        let resolved = request.resolve().unwrap();
        assert_eq!(resolved.shape, Shape::Single);
        assert_eq!(resolved.members.iter().map(|m| m.path.clone()).collect::<Vec<_>>(), [root.join("Chrono Trigger (USA).srm")]);

        assert_eq!(resolved.artifact, "Chrono Trigger (USA).srm");
        let unit = request.collect(None).unwrap();
        assert_eq!(unit.data.as_deref(), Some(&save[..]), "a single file travels as it is");
        assert_eq!(unit.content_hash, md5(&save), "RomM's hash of a plain file");

        let elsewhere = temp("srm-elsewhere");
        let restored = Request { root: elsewhere.clone(), ..request.clone() }.restore(&save, None).unwrap();
        assert_eq!(fs::read(elsewhere.join("Chrono Trigger (USA).srm")).unwrap(), save);
        assert_eq!(restored.written, [elsewhere.join("Chrono Trigger (USA).srm")]);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn a_game_with_no_saves_here_collects_to_nothing_rather_than_failing() {
        let root = temp("none");
        let request = Request {
            layout: "snes9x".into(),
            platform: Some("snes".into()),
            content: PathBuf::from("/roms/snes/Earthbound (USA).sfc"),
            identity: None,
            options: Vec::new(),
            root: root.clone(),
            profile: None,
            game_ids: Vec::new(),
        };
        let unit = request.collect(None).unwrap();
        assert!(unit.data.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_psp_games_folders_are_its_own_and_no_other_games() {
        let root = temp("psp");
        let savedata = root.join("PSP/SAVEDATA");
        fs::create_dir_all(savedata.join("ULUS10538")).unwrap();
        fs::create_dir_all(savedata.join("ULUS10538DATA00")).unwrap();
        fs::create_dir_all(savedata.join("UCUS98711DATA01")).unwrap();
        fs::write(savedata.join("ULUS10538/SD.BIN"), b"dj max").unwrap();
        fs::write(savedata.join("ULUS10538DATA00/DATA.BIN"), b"more dj max").unwrap();
        fs::write(savedata.join("UCUS98711DATA01/DATA.BIN"), b"another game").unwrap();
        fs::write(savedata.join("ULUS10538/SD.BIN.deckhand-part"), b"half a write").unwrap();
        // FOLDER_PREFIX: a PSP game keeps every folder starting with its id.
        let identity = Identity::made_up("psp", "ULUS10538", "ULUS10538", 1);
        let request = Request {
            layout: "ppsspp_standalone".into(),
            platform: Some("psp".into()),
            content: PathBuf::from("/roms/psp/DJ Max - Portable 3.chd"),
            identity: Some(identity),
            options: Vec::new(),
            root: root.clone(),
            profile: None,
            game_ids: vec!["ULUS10538".into()],
        };
        let resolved = request.resolve().unwrap();
        assert_eq!(resolved.shape, Shape::Folder);
        assert_eq!(resolved.artifact, "ULUS10538.zip");
        let unit = request.collect(None).unwrap();
        let data = unit.data.unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data.clone())).unwrap();
        let mut names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(names, ["ULUS10538/SD.BIN", "ULUS10538DATA00/DATA.BIN"], "its folders, and not a half-written file or another game's");
        assert_eq!(unit.content_hash, crate::sync::hash::of_upload(&data), "RomM's hash of a zip");

        let fresh = temp("psp-fresh");
        fs::create_dir_all(fresh.join("PSP/SAVEDATA")).unwrap();
        Request { root: fresh.clone(), ..request }.restore(&data, None).unwrap();
        assert_eq!(fs::read(fresh.join("PSP/SAVEDATA/ULUS10538DATA00/DATA.BIN")).unwrap(), b"more dj max");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&fresh);
    }

    #[test]
    fn a_restore_never_writes_outside_its_root() {
        assert_eq!(inside(Path::new("/r"), "a/b.srm"), Some(PathBuf::from("/r/a/b.srm")));
        assert_eq!(inside(Path::new("/r"), "PSP/SAVEDATA/X/"), Some(PathBuf::from("/r/PSP/SAVEDATA/X")));
        assert_eq!(inside(Path::new("/r"), "../etc/passwd"), None);
        assert_eq!(inside(Path::new("/r"), "/etc/passwd"), None);
        assert_eq!(inside(Path::new("/r"), "a/../../b"), None);
    }

    #[test]
    fn the_layout_table_names_what_each_emulator_reads() {
        let duckstation = layout("duckstation").unwrap();
        assert!(duckstation.options.iter().any(|o| o.key == "Card1Type" && o.default == "PerGameTitle"));
        assert_eq!(layout("snes9x").unwrap().id, "libretro", "a core with no row of its own gets the default");
        assert_eq!(subdirs("ppsspp_standalone"), ["PSP/SAVEDATA"]);
        assert_eq!(platform_of("gc"), Some("gamecube"));
        assert_eq!(platform_of("genesis"), None);
    }
}

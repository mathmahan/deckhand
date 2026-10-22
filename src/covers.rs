//! The cover pipeline: fetch art from RomM, decode it at display size, and keep what the views
//! need to lay it out — a port of the Coil-backed `browse/covers/` package.
//!
//! Two things beyond "get the bytes" earn their place here, and both are layout inputs rather than
//! decoration:
//!
//! * **The measured ratio.** A shelf card starts at its *system's* box ratio and refines to the
//!   art's real one once loaded. Without that a game whose cover was scanned at a different shape
//!   than its system's box sits in a box that doesn't fit it.
//! * **The dominant colour.** The hero's wash for a game with no banner, so a band with no art is
//!   still that game's colour rather than a grey rectangle.
//!
//! Decoding happens off the UI thread, at the size the art is actually drawn at — decoding a
//! 1024 px cover to draw it 170 px tall costs ~36x the memory and looks no better.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use iced::widget::image::Handle;
use iced::Color;

use crate::config::Settings;

/// What a decoded piece of art tells the layout.
#[derive(Debug, Clone, Copy)]
pub struct CoverMeta {
    /// The art's real width/height, which supersedes the system's nominal box ratio.
    pub ratio: f32,
    /// The cover's dominant colour, for the hero's fallback wash.
    pub dominant: Color,
}

#[derive(Debug, Clone)]
pub enum Cover {
    /// A fetch is in flight. Held so a second view pass doesn't queue the same URL again.
    Loading,
    Ready(Handle, CoverMeta),
    /// The fetch or the decode failed, at this moment. Every consumer has a designed fallback, so a
    /// missing cover is a look, not an error. Asked again once [`RETRY`] has passed — not on every
    /// view pass, which would hammer a server that has already said no, but not never either: a
    /// Deck that lost its Wi-Fi for a moment kept every cover it was fetching blank all session.
    Failed(std::time::Instant),
}

/// How long a failed cover waits before it may be asked for again.
pub const RETRY: std::time::Duration = std::time::Duration::from_secs(120);

/// What kind of art a fetch is for, which decides how it is prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A cover or logo: decoded at its display height, otherwise untouched.
    Art,
    /// The detail page's 1024x512 banner, which is only usable *blurred* — see [`prepare`].
    Backdrop,
    /// [`Kind::Art`] drawn larger somewhere else than it is on a shelf — the spotlight's cover, for
    /// a game with no screenshot. Prepared the same way, and kept apart: the store holds one image
    /// per key, and the shelf's copy is a third of the height.
    Feature,
    /// A screenshot as the light Home's spotlight casts over the whole page: cut to the panel's
    /// shape, shrunk to a thumbnail, blurred and darkened. The GPU stretching it back up is the rest
    /// of the blur, for nothing.
    Ambient,
    /// A screenshot as the spotlight's picture: cropped to the box it fills, at exactly the pixels
    /// it is drawn at, graded down a little, and faded out at its left and bottom edges *in its own
    /// alpha* — so it melts into the page on every renderer, where a mask over it would need one the
    /// renderer has.
    Spotlight,
    /// A screenshot opened over the whole window: fitted into the box it is shown in, at the pixels
    /// it is drawn at. Made bigger as well as smaller, and squarely once at least doubled — see
    /// [`Kind::Spotlight`] for why the stretch is done here and not by the GPU.
    Full,
}

impl Kind {
    /// Prepared the way a cover is, and so worth keeping on disk prepared: a cover at one size is
    /// the same file whichever page asked for it.
    fn is_art(self) -> bool {
        matches!(self, Kind::Art | Kind::Feature)
    }
}

/// The key `url`'s art is held under once prepared as `kind`.
///
/// The URL itself for art drawn much as it was sent, and the URL tagged with what it became for the
/// rest: one screenshot is a thumbnail on the detail page, the spotlight's picture and the light
/// behind it, and those are three images the store must not mistake for one another.
pub fn key(url: &str, kind: Kind) -> String {
    match kind {
        Kind::Art | Kind::Backdrop => url.to_string(),
        Kind::Feature => format!("{url}#feature"),
        Kind::Ambient => format!("{url}#ambient"),
        Kind::Spotlight => format!("{url}#spotlight"),
        Kind::Full => format!("{url}#full"),
    }
}

/// How much decoded art the store keeps before it starts dropping the least recently drawn.
///
/// This is what the app's memory *is*: the library itself is 3 MB against a gigabyte of covers, so
/// this number and the size of a decoded cover are the two things that set it.
///
/// Sized from the most any one page can have on screen at once rather than picked round. A grid
/// window plus its prefetch margin is at most ~110 tiles, Home's shelves and hero another ~130,
/// and a detail page a dozen — call it 250 images at the ~200 KB a cover comes to once it is cut
/// to the pixels its box draws. That is ~50 MB of working set, so this leaves room to flip between
/// every page in the app and back without re-decoding anything, and stops well short of the point
/// where holding more would only be hoarding.
const BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// One entry in the store: the art, and what the eviction policy needs to know about it.
#[derive(Debug)]
struct Entry {
    cover: Cover,
    /// The clock reading when this was last *drawn*, not when it was fetched. Recency of use is
    /// what makes eviction safe: anything on screen was read this frame, so it is always the last
    /// thing that would go.
    ///
    /// Atomic because the read that touches it happens through `&self`, from the view pass.
    used: AtomicU64,
    /// Decoded size, so the budget can be totalled without walking into every handle.
    bytes: usize,
}

/// The cover store: decoded art by URL, plus the disk cache behind it.
///
/// Bounded by [`BUDGET_BYTES`] and evicted least-recently-drawn first. An evicted URL leaves the
/// map entirely rather than being tombstoned, so [`Covers::claim`] will fetch it again if it comes
/// back on screen — off the disk cache, which still has the bytes.
#[derive(Debug, Default)]
pub struct Covers {
    entries: HashMap<String, Entry>,
    /// Monotonic tick, bumped by every lookup — the clock [`Entry::used`] is stamped from.
    clock: AtomicU64,
    /// What the `Ready` entries currently add up to.
    bytes: usize,
    /// How many of them there are. Totalled as they come and go rather than counted on demand —
    /// the header asks for it on every frame, and walking the map for an answer that only changes
    /// when a cover lands is work the whole app pays for.
    ready: usize,
}

impl Covers {
    pub fn handle(&self, url: &str) -> Option<&Handle> {
        match &self.touch(url)?.cover {
            Cover::Ready(handle, _) => Some(handle),
            _ => None,
        }
    }

    /// `url` as art drawn large — [`Kind::Feature`] — or, until that has come, the copy a shelf or a
    /// grid holds, so the page has the cover from its first frame and a sharper one a moment later.
    pub fn feature(&self, url: &str) -> Option<(Handle, CoverMeta)> {
        [key(url, Kind::Feature), url.to_string()].iter().find_map(|key| match &self.touch(key)?.cover {
            Cover::Ready(handle, meta) => Some((handle.clone(), *meta)),
            _ => None,
        })
    }

    pub fn meta(&self, url: &str) -> Option<CoverMeta> {
        match &self.touch(url)?.cover {
            Cover::Ready(_, meta) => Some(*meta),
            _ => None,
        }
    }

    /// Whether `key` has come back, with art or without — what something waiting on it waits for.
    pub fn settled(&self, key: &str) -> bool {
        matches!(self.entries.get(key).map(|e| &e.cover), Some(Cover::Ready(..) | Cover::Failed(_)))
    }

    /// Look an entry up and mark it as used now. Every read goes through here, which is what keeps
    /// "least recently used" meaning "least recently *drawn*".
    fn touch(&self, url: &str) -> Option<&Entry> {
        let entry = self.entries.get(url)?;
        entry.used.store(self.clock.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        Some(entry)
    }

    /// Claim a URL for fetching. False means it is already loading, loaded, or failed too recently
    /// to ask again — the caller must not queue it.
    pub fn claim(&mut self, url: &str) -> bool {
        match self.entries.get(url).map(|e| &e.cover) {
            Some(Cover::Failed(at)) if at.elapsed() >= RETRY => {}
            Some(_) => return false,
            None => {}
        }
        self.put(url.to_string(), Cover::Loading);
        true
    }

    pub fn insert(&mut self, url: String, cover: Cover) {
        self.put(url, cover);
        // Down to nine tenths in one go once over, rather than to the line itself: every eviction
        // sorts what is held, and stopping at the line had one sort for every cover that arrived.
        if self.bytes > BUDGET_BYTES {
            self.evict_to(BUDGET_BYTES / 10 * 9);
        }
    }

    /// Store an entry, keeping the byte total in step with what it replaces.
    fn put(&mut self, url: String, cover: Cover) {
        let bytes = cover_bytes(&cover);
        let ready = matches!(cover, Cover::Ready(..));
        let used = AtomicU64::new(self.clock.fetch_add(1, Ordering::Relaxed));
        if let Some(previous) = self.entries.insert(url, Entry { cover, used, bytes }) {
            self.forget(&previous);
        }
        self.bytes += bytes;
        self.ready += usize::from(ready);
    }

    /// Take an entry back out of the running totals.
    fn forget(&mut self, entry: &Entry) {
        self.bytes -= entry.bytes;
        self.ready -= usize::from(matches!(entry.cover, Cover::Ready(..)));
    }

    /// Drop least-recently-drawn art until the store is back inside its budget.
    ///
    /// Only `Ready` entries are candidates: a `Loading` one is a fetch in flight whose result would
    /// re-insert itself anyway, and a `Failed` one costs a map entry rather than a megabyte.
    fn evict_to(&mut self, budget: usize) {
        if self.bytes <= budget {
            return;
        }
        let mut candidates: Vec<(u64, String)> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.cover, Cover::Ready(..)))
            .map(|(url, e)| (e.used.load(Ordering::Relaxed), url.clone()))
            .collect();
        candidates.sort_unstable_by_key(|(used, _)| *used);

        for (_, url) in candidates {
            if self.bytes <= budget {
                break;
            }
            if let Some(entry) = self.entries.remove(&url) {
                self.forget(&entry);
            }
        }
    }

    /// Drop the decoded art, keeping the disk cache behind it.
    ///
    /// For the one case where held art is *wrong* rather than merely old: a move to a denser panel,
    /// after which every cover is short of rows for the screen it is on.
    pub fn drop_art(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.ready = 0;
    }

    #[cfg(test)]
    pub fn ready_count(&self) -> usize {
        self.ready
    }

    /// What the decoded art currently held adds up to — what the budget is enforced against.
    #[cfg(test)]
    pub fn bytes_held(&self) -> usize {
        self.bytes
    }
}

/// What an entry costs to hold. Only decoded pixels are worth counting — the other two variants are
/// a map entry each.
fn cover_bytes(cover: &Cover) -> usize {
    match cover {
        Cover::Ready(Handle::Rgba { pixels, .. }, _) => pixels.len(),
        _ => 0,
    }
}

/// The name a URL's bytes are kept under in the cache's `covers/` — for the README's renders, which
/// read a real cache directory rather than this build's.
#[cfg(test)]
pub fn cache_name(url: &str) -> String {
    digest(url)
}

/// A stable filename for a cache key. FNV-1a over the string, so a URL with query params or a name
/// that is not a legal filename still round-trips.
fn digest(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("{hash:016x}")
}

/// The drawn height above which a RomM `small` cover would have to be upscaled.
///
/// RomM keeps every cover twice, `small` at two fifths of `big`. On the reference server most
/// pairs are 497x680 and 198x272, so 272 is the most a `small` can give before it is being
/// stretched — and the most any can: a taller box is never worth trying it for. Many are shorter.
/// A PSP case is 290x500 and 116x200, which is why a `small` that came is measured too — see
/// [`falls_short`].
const SMALL_COVER_H: f32 = 272.0;

/// How far past that height `small` is still the better answer.
///
/// Where the app draws covers falls into two clusters, once the UI scale and the panel's own
/// density are applied: the thumbnails — shelf cards at 287-316 device px, Browse tiles at 294 on a
/// 1:1 panel and 367 on a 1.25 one — and the feature images, the detail cover and the hero at 546
/// and 552. Nothing lands between 367 and 546, so the threshold goes in that gap rather than at a
/// number tuned to whichever machine it was measured on.
///
/// 1.4 puts it at 381: every thumbnail takes the small variant on any panel this runs on, and both
/// feature images keep the original. The worst upscale that allows is 1.35x, of photographic box
/// art at thumbnail size. Set against it, `big` is eight times the bytes over a link measured at
/// ~1.1 MB/s however many requests are in flight — a thirty-tile grid filling in two seconds
/// instead of twenty. Drop this to 1.0 to always take the original, at that cost.
const SMALL_COVER_TOLERANCE: f32 = 1.4;

/// The URL to actually download for art wanted at `target`.
///
/// A Browse tile draws at roughly 198x272 — which is `small.png` to the pixel — while `big.png` is
/// eight times the bytes for the same tile. Measured against the reference server: 744 KB a cover
/// against 86, over a link that holds ~1.1 MB/s however many requests are in flight. That is a
/// thirty-tile grid taking twenty seconds to fill instead of two, and it is the whole of the
/// difference.
///
/// Only RomM's own `/cover/big.png` is rewritten, and only when the target fits inside the small
/// variant. Anything else — a logo, a screenshot, a marquee, a URL from somewhere that is not this
/// server — comes back untouched, because nothing is known about what sizes it has.
///
/// This is *only* what goes over the wire. The canonical URL stays the key for the in-memory store
/// and for [`prepared_path`], so a library cached before this existed needs no migration, and the
/// views that look art up by `game.cover_url` need no knowledge of any of it.
pub fn source_url(url: &str, target: Target) -> String {
    if target.height_px > SMALL_COVER_H * SMALL_COVER_TOLERANCE {
        return url.to_string();
    }
    // Split rather than replace, so the `?ts=` RomM appends survives on the far side.
    match url.split_once("/cover/big.png") {
        Some((head, query)) => format!("{head}/cover/small.png{query}"),
        None => url.to_string(),
    }
}

/// Whether art in `bytes` would be drawn into `target` stretched past [`SMALL_COVER_TOLERANCE`] —
/// a `small` cover [`source_url`] picked by the reference server's sizes that turned out shorter,
/// for which `big` is the answer after all. Read off the header; nothing is decoded.
pub fn falls_short(bytes: &[u8], target: Target) -> bool {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format();
    let Ok((w, h)) = reader.map_err(image::ImageError::from).and_then(|r| r.into_dimensions()) else {
        return false;
    };
    h > 0 && (h as f32) * SMALL_COVER_TOLERANCE < target.rows_needed(w as f32 / h as f32)
}

/// Where a URL's original bytes are cached on disk.
pub fn cache_path(url: &str) -> PathBuf {
    Settings::cache_dir().join("covers").join(digest(url))
}

pub fn read_cached(url: &str) -> Option<Vec<u8>> {
    std::fs::read(cache_path(url)).ok().filter(|b| !b.is_empty())
}

/// Cache bytes for next launch. Best effort: a cold start that has to re-fetch is slower, not
/// broken, so a full or read-only cache directory must never surface as an error.
pub fn write_cached(url: &str, bytes: &[u8]) {
    write_best_effort(&cache_path(url), bytes);
}

fn write_best_effort(path: &std::path::Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, bytes);
}

/// The ladder [`Target`] heights are rounded up to before they become a cache key.
///
/// Without it the key would carry the exact tile height, which is derived from the window width —
/// so every drag of the window edge would miss the cache and write another near-identical copy of
/// every cover. Rounding up keeps the cache to a handful of sizes per image and never asks for
/// fewer pixels than the box wanted.
const SIZE_STEP: u32 = 64;

/// Where the *prepared* form of a URL is cached: decoded and reduced to the rows its box samples.
///
/// Separate from the original because the two answer different questions. The original is what the
/// server sent, and preparing it again costs a full-size decode plus a resize — measured at 13 ms a
/// cover, which is most of what opening a page costs. This is that work already done, and reading
/// it back is ~8x cheaper because the file is a fraction of the size.
///
/// The box ratio is part of the key, not just the height: a box that crops the art sideways draws
/// it from more rows than one that does not, so the same cover at the same height is a different
/// number of pixels in a landscape system's grid than in a portrait one's, and one file cannot
/// stand for both.
fn prepared_path(url: &str, rows: u32, box_ratio: Option<f32>) -> PathBuf {
    let box_key = match box_ratio {
        Some(r) => format!("{:.0}", r * 100.0),
        None => "n".to_string(),
    };
    Settings::cache_dir()
        .join("prepared")
        .join(format!("{}-{rows}x{box_key}-{PREPARED_VERSION}.png", digest(url)))
}

/// Bumped when what a prepared file is made from changes, so files made the old way are not found
/// again — they age out under [`PREPARED_BUDGET_BYTES`]. 2: a `small` cover too short for its tile
/// gives way to `big` ([`falls_short`]); before, a PSP tile kept its 200-row `small` for good.
const PREPARED_VERSION: u32 = 2;

/// How much prepared art is kept on disk before the oldest is dropped.
///
/// The prepared cache holds a file per size an image is drawn at, so unlike the originals it has no
/// natural ceiling — a library browsed at several window widths would keep growing one. This is the
/// ceiling. It is only ever a saving of work: everything in here can be rebuilt from the originals
/// beside it, and dropping the wrong file costs one decode.
const PREPARED_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// How much of the *originally downloaded* art to keep.
///
/// Smaller than the prepared budget on purpose. These are the bytes exactly as the server sent
/// them, at whatever size it holds them, and nothing draws from them directly — they exist only so
/// art can be re-prepared at a size [`prepared_path`] does not already have, after a UI-scale
/// change or a move to a denser panel. Losing one costs a download; losing a prepared file costs a
/// decode of something still on disk. So the cache that is actually read gets the room.
const ORIGINAL_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// Trim both on-disk caches back inside their budgets, oldest file first.
///
/// Called once at startup, off the UI thread. Both directories grow with every distinct piece of
/// art the app has ever drawn — covers, logos, composites, banners and screenshots, across every
/// system browsed — and on a handheld that is the storage the games are supposed to be using.
pub fn prune_caches() {
    let cache = Settings::cache_dir();
    prune_dir(&cache.join("prepared"), PREPARED_BUDGET_BYTES);
    prune_dir(&cache.join("covers"), ORIGINAL_BUDGET_BYTES);
}

/// Bytes on disk across both art caches: what the Settings page reports, and what clearing frees.
pub fn disk_usage() -> u64 {
    let cache = Settings::cache_dir();
    [cache.join("covers"), cache.join("prepared")]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| entry.ok()?.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Delete both art caches.
///
/// Nothing in them is load-bearing: the originals come back from the server and the prepared
/// copies are rebuilt from those. Art already on screen is held in memory and stays put.
pub fn clear_disk_caches() {
    let cache = Settings::cache_dir();
    for dir in ["covers", "prepared"] {
        let _ = std::fs::remove_dir_all(cache.join(dir));
    }
}

/// Delete oldest-first from `dir` until it fits `budget`.
///
/// By write time rather than read time: a read does not touch a file's timestamp, and touching it
/// on every cache hit would trade the disk writes this cache exists to avoid for a different set of
/// disk writes. That makes this closer to first-in-first-out than to least-recently-used, which is
/// the right trade here — the cost of being wrong is one re-download, not a stall.
///
/// Best effort throughout. A cache that cannot be read or trimmed is not worth failing a launch
/// over; the worst case is a directory that stays too big until the next run.
fn prune_dir(dir: &std::path::Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };

    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            // Directories have no business here, and counting one's length as bytes on disk would
            // be meaningless anyway.
            meta.is_file()
                .then(|| Some((meta.modified().ok()?, meta.len(), entry.path())))
                .flatten()
        })
        .collect();

    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total <= budget {
        return;
    }
    files.sort_unstable_by_key(|(modified, _, _)| *modified);
    for (_, len, path) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// The prepared art for `url` at `target`, if a previous run already made it.
///
/// Deliberately takes no source bytes. Art that has been prepared once needs nothing from the
/// original ever again, and the caller can only know whether to go and get those bytes by asking
/// this first — which is the whole point: the original is the big file, and after a prune it may
/// not be on disk at all, making "fetch it anyway" a download over the network for something that
/// is discarded unread.
///
/// `None` covers both a miss and a file that will not decode; either way the answer is the same,
/// which is to build it from the original.
pub fn prepared_art(url: &str, target: Target, kind: Kind) -> Option<(Handle, CoverMeta)> {
    // Only covers are written here — see [`prepare_cached`] — so for anything else there is
    // nothing to find.
    if !kind.is_art() {
        return None;
    }
    let path = prepared_path(url, prepared_rows(target), target.box_ratio);
    let cached = std::fs::read(&path).ok().filter(|b| !b.is_empty())?;
    let image = image::load_from_memory(&cached).ok()?;
    let ratio = aspect_of(&image)?;
    // A hit is already the finished image: resampling it again would only soften it.
    Some(finish(image, ratio))
}

/// The height a target is prepared at, rounded up the size ladder so dragging a window does not
/// write a file per pixel. Shared with [`prepared_art`] so a lookup and a build agree on the name.
fn prepared_rows(target: Target) -> u32 {
    (target.height_px.max(1.0).ceil() as u32).div_ceil(SIZE_STEP) * SIZE_STEP
}

/// Prepare `bytes` for `target`, keeping the result on disk for [`prepared_art`] to find.
///
/// Call [`prepared_art`] first. This is the build path, and it is the only one that needs the
/// original.
pub fn prepare_cached(
    url: &str,
    bytes: &[u8],
    target: Target,
    kind: Kind,
) -> Option<(Handle, CoverMeta)> {
    // Everything but a cover is exempt: there are one or two of each on screen at a time, they are
    // quick to make from the original, and a blur or a fade is not idempotent — re-preparing a
    // cached one would blur what is already blurred.
    if !kind.is_art() {
        return prepare(bytes, target, kind);
    }

    let rows = prepared_rows(target);
    // The rounded-up height is what gets prepared, so the key describes the file exactly.
    let target = Target { height_px: rows as f32, ..target };
    let path = prepared_path(url, rows, target.box_ratio);

    let decoded = image::load_from_memory(bytes).ok()?;
    let ratio = aspect_of(&decoded)?;
    let prepared = resample(decoded, ratio, target, kind);

    let mut encoded = std::io::Cursor::new(Vec::new());
    if prepared.write_to(&mut encoded, image::ImageFormat::Png).is_ok() {
        write_best_effort(&path, encoded.get_ref());
    }
    Some(finish(prepared, ratio))
}

/// The box a piece of art is drawn into, as the decoder needs to see it.
///
/// Height is in **device pixels**, not layout units — the caller multiplies by the panel's real
/// density, so a 1x screen stops paying for pixels it cannot show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    /// The drawn height of the box, in device pixels.
    pub height_px: f32,
    /// The box's own width/height, for art drawn with `ContentFit::Cover` — see
    /// [`Target::rows_needed`]. `None` for `Contain`, and for any box sized from the art's own
    /// ratio, where the two agree by construction and the height is the whole story.
    pub box_ratio: Option<f32>,
}

impl Target {
    /// Art drawn `height_px` tall with no cropping to allow for.
    pub fn tall(height_px: f32) -> Self {
        Self { height_px, box_ratio: None }
    }

    /// How many rows of source art the box actually samples, for art of `art_ratio`.
    ///
    /// Under `Cover` the art is scaled by whichever axis needs the *most* — so art narrower than
    /// its box is scaled up to the box's width and cropped top and bottom, and the rows the box
    /// shows are then only part of the picture. A grid tile is the case that matters: it is shaped
    /// by its system's box, not by the scan inside it, so a portrait cover in a landscape system is
    /// magnified about 2x more than its height alone implies. Matching height only there would draw
    /// it soft.
    fn rows_needed(&self, art_ratio: f32) -> f32 {
        match self.box_ratio {
            Some(box_ratio) if art_ratio > 0.0 => self.height_px * (box_ratio / art_ratio).max(1.0),
            _ => self.height_px,
        }
    }
}

/// Decode `bytes` and prepare them for display in `target`.
///
/// Runs on a blocking thread — a 745x745 PNG decode is milliseconds, but a shelf's worth at once
/// would still be a visible hitch on the UI thread.
pub fn prepare(bytes: &[u8], target: Target, kind: Kind) -> Option<(Handle, CoverMeta)> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let ratio = aspect_of(&decoded)?;
    Some(finish(resample(decoded, ratio, target, kind), ratio))
}

/// An image's width/height, or `None` for one with no area to have a shape.
fn aspect_of(image: &image::DynamicImage) -> Option<f32> {
    let (w, h) = (image.width(), image.height());
    (w > 0 && h > 0).then(|| w as f32 / h as f32)
}

/// The ambient light's size: the panel's own 16:10, at a size where a blur costs nothing.
const AMBIENT: (u32, u32) = (64, 40);

/// The ambient light's blur, at [`AMBIENT`]'s size — about 40 px across a 1280 px panel, before
/// the GPU's stretch adds its own.
const AMBIENT_BLUR: f32 = 2.0;

/// How far the ambient light is darkened, and how far its colour is pushed. Dark enough that the
/// text over it never has to fight it; saturated enough that what is left is still the game's
/// colour rather than a grey.
const AMBIENT_GRADE: (f32, f32) = (0.38, 1.3);

/// The spotlight's picture, held back a little so it sits behind the text rather than beside it.
const SPOTLIGHT_GRADE: (f32, f32) = (0.82, 1.05);

/// How much of the spotlight's picture, from its left edge, fades in from nothing — the text sits
/// over that part.
const FEATHER_LEFT: f32 = 0.55;

/// Where, from the top, the spotlight's picture starts fading out — the shelf sits under the rest.
const FEATHER_BOTTOM: f32 = 0.45;

/// Cut `decoded` down to the pixels `target` will draw. Split out from [`prepare`] so the cache
/// above can do this once and skip it on every later read.
fn resample(
    decoded: image::DynamicImage,
    ratio: f32,
    target: Target,
    kind: Kind,
) -> image::DynamicImage {
    let h = decoded.height();
    match kind {
        // Decode at exactly the number of source rows the box samples, and no more. This was a flat
        // 2x the drawn height, which is right only on a 2x panel: on the 1x screens most of these
        // run on it kept three times the pixels anything could draw, and decoded art is the whole
        // of this app's memory. Never *upscale* — art smaller than its slot is drawn as-is and the
        // GPU stretches it, which is cheaper and no worse.
        Kind::Art | Kind::Feature => {
            let target_px = target.rows_needed(ratio).round().max(1.0) as u32;
            if h > target_px {
                let width = ((target_px as f32) * ratio).round().max(1.0) as u32;
                decoded.resize_exact(width, target_px, image::imageops::FilterType::CatmullRom)
            } else {
                decoded
            }
        }
        // The banner is blurred past legibility on purpose. Left as-is it fights the band on three
        // counts: it carries the game's wordmark dead centre (which the title beside it already
        // shows), it has a hard-edged platform sidebar down its left quarter, and at 2:1 against a
        // ~4.3:1 band a crop keeps precisely the middle strip both of those live in. Blurred it
        // stops being a picture and becomes what a backdrop wants — this game's colours.
        //
        // Downscaling *first* is what makes the blur cheap: a 24 px sigma over 1024x512 is slow,
        // where the same visual result is a 3 px sigma over a 128 px-wide thumbnail that the GPU
        // then stretches back up. The stretch is free and adds blur of its own.
        Kind::Backdrop => {
            let small = decoded.resize_exact(160, 80, image::imageops::FilterType::Triangle);
            image::DynamicImage::ImageRgba8(image::imageops::blur(&small.to_rgba8(), 3.0))
        }
        // The same trick as the banner's, taken further: a whole screenshot reduced to a 64x40
        // smear of its colours. Cropped to the panel's shape first, so stretching it back over the
        // window stretches both ways alike.
        Kind::Ambient => {
            let (w, h) = AMBIENT;
            let small = cover_crop(&decoded, w as f32 / h as f32).resize_exact(w, h, image::imageops::FilterType::Triangle);
            let mut light = image::imageops::blur(&small.to_rgba8(), AMBIENT_BLUR);
            grade(&mut light, AMBIENT_GRADE);
            image::DynamicImage::ImageRgba8(light)
        }
        // Cropped to the box rather than fitted into it, so the picture fills it edge to edge, and
        // resampled to exactly the pixels the box draws, so the GPU draws it one to one.
        //
        // Up as well as down — the one kind of art here ever made bigger. A console's own frame,
        // 256x224 for a SNES, is a third of the box's height, and a picture is what the spotlight is
        // for. Square, by nearest neighbour, once it is to be at least doubled: pixel art stretched
        // smoothly turns to mush, where stretched square it is the picture the console drew. Done
        // here rather than by the GPU's own nearest filtering, which iced 0.14's wgpu renderer drew
        // nothing of on a page with other art on it.
        Kind::Spotlight => {
            let box_ratio = target.box_ratio.unwrap_or(ratio);
            let picture = cover_crop(&decoded, box_ratio);
            let rows = target.height_px.round().max(1.0) as u32;
            let width = ((rows as f32) * box_ratio).round().max(1.0) as u32;
            let filter = if rows >= picture.height() * 2 {
                image::imageops::FilterType::Nearest
            } else {
                image::imageops::FilterType::CatmullRom
            };
            let mut picture = picture.resize_exact(width, rows, filter).into_rgba8();
            grade(&mut picture, SPOTLIGHT_GRADE);
            feather(&mut picture);
            image::DynamicImage::ImageRgba8(picture)
        }
        // Fitted into the box, the whole picture showing: as tall as the box where it is the
        // narrower of the two, as wide where it is the wider.
        Kind::Full => {
            let box_ratio = target.box_ratio.unwrap_or(ratio);
            let rows = if ratio > box_ratio { target.height_px * box_ratio / ratio } else { target.height_px };
            let rows = rows.round().max(1.0) as u32;
            if rows == h {
                return decoded;
            }
            let width = ((rows as f32) * ratio).round().max(1.0) as u32;
            let filter = if rows >= h * 2 {
                image::imageops::FilterType::Nearest
            } else {
                image::imageops::FilterType::CatmullRom
            };
            decoded.resize_exact(width, rows, filter)
        }
    }
}

/// The middle of `image` at `ratio` — what `ContentFit::Cover` would show of it, as pixels of its
/// own rather than as a crop the renderer has to honour.
pub(crate) fn cover_crop(image: &image::DynamicImage, ratio: f32) -> image::DynamicImage {
    let (w, h) = (image.width(), image.height());
    let (cw, ch) = if w as f32 > h as f32 * ratio {
        (((h as f32) * ratio).round().clamp(1.0, w as f32) as u32, h)
    } else {
        (w, ((w as f32) / ratio).round().clamp(1.0, h as f32) as u32)
    };
    image.crop_imm((w - cw) / 2, (h - ch) / 2, cw, ch)
}

/// Darken and saturate, as a CSS `brightness()` and `saturate()` would: each channel pushed away
/// from the pixel's luma by `saturation`, then scaled by `brightness`.
pub(crate) fn grade(rgba: &mut image::RgbaImage, (brightness, saturation): (f32, f32)) {
    for pixel in rgba.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let channel = |c: f32| ((luma + (c - luma) * saturation) * brightness).round().clamp(0.0, 255.0) as u8;
        pixel.0 = [channel(r), channel(g), channel(b), a];
    }
}

/// Fade the spotlight's picture in across its left [`FEATHER_LEFT`] and out below
/// [`FEATHER_BOTTOM`], in its alpha. Smoothstep rather than straight ramps: a straight one shows
/// where it starts and stops as a faint line.
fn feather(rgba: &mut image::RgbaImage) {
    let smooth = |edge0: f32, edge1: f32, x: f32| {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let (w, h) = rgba.dimensions();
    let across: Vec<f32> = (0..w).map(|x| smooth(0.0, FEATHER_LEFT, (x as f32 + 0.5) / w as f32)).collect();
    let down: Vec<f32> = (0..h).map(|y| 1.0 - smooth(FEATHER_BOTTOM, 1.0, (y as f32 + 0.5) / h as f32)).collect();
    for (x, y, pixel) in rgba.enumerate_pixels_mut() {
        let a = f32::from(pixel.0[3]) * across[x as usize] * down[y as usize];
        pixel.0[3] = a.round() as u8;
    }
}

/// Turn a prepared image into what the views consume: a handle, and the measurements the layout
/// reads off it.
fn finish(prepared: image::DynamicImage, ratio: f32) -> (Handle, CoverMeta) {
    // Into, not to: art that decoded as RGBA already is handed over rather than copied.
    let rgba = prepared.into_rgba8();
    let meta = CoverMeta { ratio, dominant: dominant_colour(&rgba) };
    let (pw, ph) = (rgba.width(), rgba.height());
    (Handle::from_rgba(pw, ph, rgba.into_raw()), meta)
}

/// The art's dominant colour, for the hero's fallback wash.
///
/// A weighted average rather than a histogram or a k-means pass: the wash is drawn at 10-30% alpha
/// behind a scrim, so its *hue* is all that survives and a cleverer clustering would not be visible.
/// Two weights do the work — transparent pixels are skipped entirely (a logo is mostly empty, and
/// averaging its transparency in returns grey for every game), and saturated pixels count for more
/// than flat ones, so a cover that is 80% black border still yields the colour on the box.
fn dominant_colour(rgba: &image::RgbaImage) -> Color {
    let (mut r, mut g, mut b, mut total) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    // Sample rather than sum: on a 2x-decoded cover every 7th pixel is thousands of samples, which
    // is far past what the average needs.
    for px in rgba.pixels().step_by(7) {
        let [pr, pg, pb, pa] = px.0;
        if pa < 128 {
            continue;
        }
        let (fr, fg, fb) = (pr as f64, pg as f64, pb as f64);
        let max = fr.max(fg).max(fb);
        let min = fr.min(fg).min(fb);
        // Chroma, plus a floor so a genuinely monochrome cover still averages to something.
        let weight = (max - min) / 255.0 + 0.15;
        r += fr * weight;
        g += fg * weight;
        b += fb * weight;
        total += weight;
    }
    if total <= 0.0 {
        return crate::theme::SURFACE_VARIANT;
    }
    let (r, g, b) = ((r / total) as f32 / 255.0, (g / total) as f32 / 255.0, (b / total) as f32 / 255.0);
    // Lift a very dark average clear of the page's own ground, or the wash reads as nothing at all.
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    if luma < 0.18 {
        let lift = 0.18 / luma.max(0.02);
        return Color::from_rgb((r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0));
    }
    Color::from_rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_png(r: u8, g: u8, b: u8, w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([r, g, b, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn decoding_records_the_arts_real_ratio_not_its_slot() {
        let (_, meta) = prepare(&solid_png(200, 40, 40, 400, 200), Target::tall(170.0), Kind::Art).expect("decodes");
        assert!((meta.ratio - 2.0).abs() < 0.01, "ratio comes from the source, got {}", meta.ratio);
    }

    #[test]
    fn dominant_colour_finds_the_hue_and_lifts_a_dark_one() {
        let (_, meta) = prepare(&solid_png(200, 40, 40, 64, 64), Target::tall(170.0), Kind::Art).expect("decodes");
        assert!(meta.dominant.r > meta.dominant.g && meta.dominant.r > meta.dominant.b, "a red cover washes red");

        // A near-black cover is lifted clear of the page ground rather than washing it in nothing.
        let (_, dark) = prepare(&solid_png(8, 8, 10, 64, 64), Target::tall(170.0), Kind::Art).expect("decodes");
        let luma = 0.299 * dark.dominant.r + 0.587 * dark.dominant.g + 0.114 * dark.dominant.b;
        assert!(luma >= 0.17, "a dark cover is lifted, got luma {luma}");
    }

    #[test]
    fn a_transparent_logo_does_not_average_to_grey() {
        let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        for x in 20..44 {
            for y in 20..44 {
                img.put_pixel(x, y, image::Rgba([40, 200, 80, 255]));
            }
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let (_, meta) = prepare(bytes.get_ref(), Target::tall(170.0), Kind::Art).expect("decodes");
        assert!(meta.dominant.g > meta.dominant.r, "the opaque green wins, not the empty space");
    }

    #[test]
    fn garbage_bytes_fail_rather_than_panic() {
        assert!(prepare(b"not an image at all", Target::tall(170.0), Kind::Art).is_none());
    }

    fn art(kb: usize) -> Cover {
        let pixels = vec![0u8; kb * 1024];
        Cover::Ready(
            Handle::from_rgba(16, (kb * 1024 / 64) as u32, pixels),
            CoverMeta { ratio: 1.0, dominant: Color::BLACK },
        )
    }

    fn rgba_of(handle: &Handle) -> (u32, u32, &[u8]) {
        let Handle::Rgba { width, height, pixels, .. } = handle else { panic!("decoded to rgba") };
        (*width, *height, pixels.as_ref())
    }

    #[test]
    fn one_screenshot_is_held_once_for_each_thing_it_becomes() {
        let url = "https://romm.lan/assets/romm/resources/roms/1/211/screenshots/0.jpg";
        let keys = [Kind::Art, Kind::Feature, Kind::Ambient, Kind::Spotlight, Kind::Full].map(|kind| key(url, kind));
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "a thumbnail, a picture and a light are three images");
            }
        }
        assert_eq!(key(url, Kind::Art), url, "art drawn as sent keeps its own URL");
    }

    #[test]
    fn the_ambient_light_is_a_small_dark_smear_of_the_picture() {
        let (handle, _) = prepare(&solid_png(200, 120, 40, 960, 720), Target::tall(800.0), Kind::Ambient).expect("decodes");
        let (w, h, pixels) = rgba_of(&handle);
        assert_eq!((w, h), AMBIENT, "the same thumbnail whatever the screen, for the GPU to stretch");
        // Darkened well below the source, and still the source's hue.
        let [r, g, b, _] = [pixels[0], pixels[1], pixels[2], pixels[3]];
        assert!(r < 120 && r > g && g > b, "dark, and still orange: {r} {g} {b}");
    }

    #[test]
    fn the_spotlight_fills_its_box_and_fades_into_the_page_on_two_sides() {
        let target = Target { height_px: 300.0, box_ratio: Some(1.5) };
        let (handle, _) = prepare(&solid_png(90, 160, 220, 1280, 720), target, Kind::Spotlight).expect("decodes");
        let (w, h, pixels) = rgba_of(&handle);
        assert_eq!((w, h), (450, 300), "cropped to the box's shape and brought down to its rows");
        let alpha = |x: u32, y: u32| pixels[((y * w + x) * 4 + 3) as usize];
        assert!(alpha(0, 0) < 8, "nothing at the left edge, where the text is");
        assert_eq!(alpha(w - 1, 0), 255, "all of it at the top right");
        assert!(alpha(w - 1, h - 1) < 8, "nothing at the bottom, where the shelf is");
        assert!(alpha(w / 4, h / 4) > 0 && alpha(w / 4, h / 4) < 255, "part way in between");
    }

    #[test]
    fn a_consoles_own_small_screenshot_is_scaled_up_square() {
        // 256x224, the SNES's frame, in a box four times as tall: made the box's size, by nearest
        // neighbour, so its pixels stay hard-edged blocks.
        let mut frame = image::RgbaImage::new(256, 224);
        for (x, y, pixel) in frame.enumerate_pixels_mut() {
            *pixel = if (x + y) % 2 == 0 { image::Rgba([250, 250, 250, 255]) } else { image::Rgba([10, 10, 10, 255]) };
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(frame).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let target = Target { height_px: 560.0, box_ratio: Some(1.5) };
        let (handle, _) = prepare(png.get_ref(), target, Kind::Spotlight).expect("decodes");
        let (w, h, pixels) = rgba_of(&handle);
        assert_eq!((w, h), (840, 560), "the box's own pixels");
        // Top right, clear of both fades: every pixel is one of the frame's two, none a blend.
        let at = |x: u32, y: u32| pixels[((y * w + x) * 4) as usize];
        let bright = (at(w - 40, 10), at(w - 41, 10), at(w - 42, 10), at(w - 43, 10));
        for value in [bright.0, bright.1, bright.2, bright.3] {
            assert!(!(60..=180).contains(&value), "a hard edge, not a smear: {bright:?}");
        }
    }

    #[test]
    fn a_screenshot_opened_full_is_fitted_whole_into_its_box() {
        // Narrower than the box: as tall as it, made bigger.
        let target = Target { height_px: 672.0, box_ratio: Some(1.6) };
        let (handle, _) = prepare(&solid_png(90, 160, 220, 256, 224), target, Kind::Full).expect("decodes");
        let (w, h, _) = rgba_of(&handle);
        assert_eq!((w, h), (768, 672), "three times over, the whole frame");
        // Wider than the box: as wide as it, made smaller.
        let target = Target { height_px: 720.0, box_ratio: Some(1.0) };
        let (handle, _) = prepare(&solid_png(90, 160, 220, 1280, 720), target, Kind::Full).expect("decodes");
        let (w, h, _) = rgba_of(&handle);
        assert_eq!((w, h), (720, 405), "the box's width, nothing cut off");
    }

    fn store(entries: &[(&str, usize)]) -> Covers {
        let mut covers = Covers::default();
        for (url, kb) in entries {
            covers.insert((*url).to_string(), art(*kb));
        }
        covers
    }

    #[test]
    fn the_store_totals_what_it_is_holding() {
        let mut covers = store(&[("a", 100), ("b", 200)]);
        assert_eq!(covers.bytes_held(), 300 * 1024);
        assert_eq!(covers.ready_count(), 2);

        // Both totals are kept as entries come and go, so they have to survive every path that
        // changes the map — a claim over a loaded cover included.
        assert!(!covers.claim("a"), "already held");
        covers.insert("c".into(), Cover::Failed(std::time::Instant::now()));
        assert_eq!(covers.ready_count(), 2, "a failure is not art");
        covers.evict_to(0);
        assert_eq!((covers.ready_count(), covers.bytes_held()), (0, 0));
    }

    #[test]
    fn replacing_an_entry_does_not_double_count_it() {
        let mut covers = store(&[("a", 100)]);
        covers.insert("a".into(), art(50));
        assert_eq!(covers.bytes_held(), 50 * 1024, "the old size went with the old entry");
    }

    #[test]
    fn the_least_recently_drawn_cover_is_the_one_that_goes() {
        let mut covers = store(&[("a", 100), ("b", 100), ("c", 100)]);
        // Drawing a page reads the covers on it. "b" is not on this one.
        let _ = covers.handle("a");
        let _ = covers.handle("c");

        covers.evict_to(250 * 1024);
        assert!(covers.handle("b").is_none(), "the one nothing drew is the one dropped");
        assert!(covers.handle("a").is_some() && covers.handle("c").is_some());
        assert_eq!(covers.bytes_held(), 200 * 1024);
    }

    #[test]
    fn eviction_stops_as_soon_as_it_is_back_inside_the_budget() {
        let mut covers = store(&[("a", 100), ("b", 100), ("c", 100), ("d", 100)]);
        covers.evict_to(250 * 1024);
        assert_eq!(covers.ready_count(), 2, "two dropped, not the whole store flushed");
    }

    #[test]
    fn a_failed_cover_is_asked_for_again_once_it_has_waited() {
        let mut covers = Covers::default();
        assert!(covers.claim("u"));
        covers.insert("u".into(), Cover::Failed(std::time::Instant::now()));
        assert!(!covers.claim("u"), "not straight away");
        covers.insert("u".into(), Cover::Failed(std::time::Instant::now() - RETRY));
        assert!(covers.claim("u"), "once the wait is over");
        assert!(!covers.claim("u"), "and only once: it is loading now");
    }

    #[test]
    fn an_evicted_cover_can_be_fetched_again() {
        // Eviction removes the entry rather than tombstoning it, so scrolling back up to art that
        // was dropped re-claims it — off the disk cache, which still has the bytes.
        let mut covers = store(&[("a", 100), ("b", 100)]);
        let _ = covers.handle("b");
        covers.evict_to(100 * 1024);

        assert!(covers.claim("a"), "the dropped cover is fetchable again");
        assert!(!covers.claim("b"), "the one still held is not re-fetched");
    }

    #[test]
    fn a_fetch_in_flight_is_not_evicted_out_from_under_itself() {
        // A `Loading` entry is a claim, not memory: dropping it would let the same URL be claimed
        // twice and fetched twice, and the result would re-insert itself anyway.
        let mut covers = store(&[("a", 400)]);
        assert!(covers.claim("pending"));
        covers.evict_to(1024);
        assert!(!covers.claim("pending"), "the in-flight claim survived");
        assert_eq!(covers.bytes_held(), 0, "the art it was over budget for did not");
    }

    #[test]
    fn a_box_that_matches_its_art_keeps_only_the_rows_it_draws() {
        // The ordinary case, and the one that decides how much memory the app uses: art shaped like
        // the box it sits in is sampled row for row.
        let target = Target { height_px: 300.0, box_ratio: Some(0.71) };
        assert_eq!(target.rows_needed(0.71), 300.0);
        // Art *wider* than its box is cropped left and right, which costs no extra rows.
        assert_eq!(target.rows_needed(1.4), 300.0);
    }

    #[test]
    fn art_narrower_than_its_box_keeps_the_rows_the_crop_eats() {
        // A portrait scan in a landscape system's grid: `Cover` scales it to the tile's width and
        // crops top and bottom, so the tile shows a slice of a much larger picture. Sampling it at
        // the tile's height alone is what would draw it soft.
        let landscape = Target { height_px: 300.0, box_ratio: Some(1.37) };
        let needed = landscape.rows_needed(0.7);
        assert!((needed - 300.0 * (1.37 / 0.7)).abs() < 0.01, "got {needed}");
        assert!(needed > 570.0, "a 2x crop needs about twice the rows: {needed}");
    }

    #[test]
    fn art_with_no_box_to_crop_against_is_sampled_at_its_drawn_height() {
        let target = Target::tall(300.0);
        for art_ratio in [0.5, 1.0, 2.0] {
            assert_eq!(target.rows_needed(art_ratio), 300.0);
        }
    }

    #[test]
    fn decoding_keeps_the_rows_the_target_asked_for_and_never_invents_any() {
        // 800 rows of source into a 200 px box: down to 200, not the 400 a fixed 2x would have kept.
        let (handle, _) = prepare(&solid_png(10, 20, 30, 800, 800), Target::tall(200.0), Kind::Art)
            .expect("decodes");
        let Handle::Rgba { height, .. } = handle else { panic!("decoded to rgba") };
        assert_eq!(height, 200);

        // Art smaller than its box is left alone rather than upscaled — the GPU stretches it for
        // free, and storing the upscale would only cost memory.
        let (small, _) = prepare(&solid_png(10, 20, 30, 64, 64), Target::tall(500.0), Kind::Art)
            .expect("decodes");
        let Handle::Rgba { height, .. } = small else { panic!("decoded to rgba") };
        assert_eq!(height, 64);
    }

    #[test]
    fn the_prepared_cache_keys_on_everything_that_changes_the_file() {
        let a = prepared_path("https://a/x.png", 320, Some(1.37));
        // A different size, a different box shape, and a different image are all different files:
        // one prepared for a landscape grid is cropped differently to one for a portrait grid, and
        // reusing it across the two would draw one of them soft.
        assert_ne!(a, prepared_path("https://a/x.png", 384, Some(1.37)));
        assert_ne!(a, prepared_path("https://a/x.png", 320, Some(0.71)));
        assert_ne!(a, prepared_path("https://a/x.png", 320, None));
        assert_ne!(a, prepared_path("https://a/y.png", 320, Some(1.37)));
        assert_eq!(a, prepared_path("https://a/x.png", 320, Some(1.37)), "and stable");
    }

    /// The prepared cache, end to end, in a cache folder of this test's own.
    #[test]
    fn a_cache_hit_returns_the_same_art_without_preparing_it_again() {
        let dir = std::env::temp_dir().join(format!("deckhand-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::config::TEST_CACHE.with(|cache| *cache.borrow_mut() = Some(dir.join("deckhand-rust")));

        // Best effort throughout: a first run has no prepared directory to trim.
        prune_caches();

        let bytes = solid_png(180, 60, 40, 700, 700);
        let target = Target { height_px: 200.0, box_ratio: Some(1.0) };
        let url = "https://romm.example/roms/1/cover.png";

        let (first, meta) = prepare_cached(url, &bytes, target, Kind::Art).expect("prepares");
        let files: Vec<_> = std::fs::read_dir(dir.join("deckhand-rust/prepared"))
            .expect("the prepared directory exists")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "one prepared file, named for the target it was prepared for");

        // Second time round the original is not needed at all: `prepared_art` takes no source
        // bytes, which is what lets the caller skip reading - or re-downloading - the original.
        let (second, again) = prepared_art(url, target, Kind::Art).expect("reads the prepared file");
        let (Handle::Rgba { width: w1, height: h1, .. }, Handle::Rgba { width: w2, height: h2, .. }) =
            (&first, &second)
        else {
            panic!("both decode to rgba")
        };
        assert_eq!((w1, h1), (w2, h2), "same pixels, not a second resample of the first");
        assert!((meta.ratio - again.ratio).abs() < 0.01);

        // Rounded up to the size ladder, so dragging the window does not write a file per pixel.
        assert_eq!(*h1, 256, "200 px of box rounds up to the next {SIZE_STEP} px step");

        // A target the size ladder rounds elsewhere is a different file, and a miss.
        let taller = Target { height_px: 400.0, box_ratio: Some(1.0) };
        assert!(prepared_art(url, taller, Kind::Art).is_none(), "a different size is not this file");

        // Backdrops are never written to the prepared cache, so there is never anything to find -
        // asking must not hand back art prepared for the same URL as a cover.
        assert!(prepared_art(url, target, Kind::Backdrop).is_none(), "backdrops are not cached here");

        // A prune well inside the budget leaves the cache alone; there is nothing to reclaim.
        prune_caches();
        assert!(prepared_path(url, 256, Some(1.0)).exists(), "still cached after a prune");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_prune_drops_the_oldest_files_until_the_budget_fits() {
        let dir = std::env::temp_dir().join(format!("deckhand-prune-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test directory");

        // Five 1 KB files, written oldest first and stamped far enough apart that the sort is not
        // at the mercy of the filesystem's timestamp resolution.
        let mut paths = Vec::new();
        for i in 0..5u32 {
            let path = dir.join(format!("f{i}"));
            std::fs::write(&path, vec![0u8; 1024]).expect("write");
            let stamp = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000 + u64::from(i) * 60);
            // Opened for writing: setting a timestamp needs write access, and `File::open` on
            // Windows hands back a read-only handle that refuses it.
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open")
                .set_modified(stamp)
                .expect("stamp");
            paths.push(path);
        }

        // Room for three of the five.
        prune_dir(&dir, 3 * 1024);

        assert!(!paths[0].exists(), "the oldest goes first");
        assert!(!paths[1].exists(), "then the next oldest");
        for path in &paths[2..] {
            assert!(path.exists(), "{path:?} is inside the budget and stays");
        }

        // Already inside the budget: nothing more to reclaim, so nothing else goes.
        prune_dir(&dir, 3 * 1024);
        assert_eq!(std::fs::read_dir(&dir).expect("read").count(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_a_directory_that_is_not_there_is_not_an_error() {
        // A first run has neither cache directory, and a launch must not depend on one existing.
        prune_dir(&std::env::temp_dir().join("deckhand-does-not-exist-4f2a"), 1024);
    }

    #[test]
    fn a_tile_sized_target_downloads_the_small_cover() {
        let tile = Target { height_px: 272.0, box_ratio: Some(0.72) };
        assert_eq!(
            source_url("https://romm.lan/assets/romm/resources/roms/15/1504/cover/big.png", tile),
            "https://romm.lan/assets/romm/resources/roms/15/1504/cover/big.png"
                .replace("big.png", "small.png"),
        );
    }

    #[test]
    fn the_timestamp_romm_appends_survives_the_rewrite() {
        // Dropping the query would defeat RomM's own cache busting and serve stale art after a
        // rescrape.
        let tile = Target { height_px: 200.0, box_ratio: None };
        let out = source_url("https://romm.lan/x/cover/big.png?ts=2026-08-11 10:48:15", tile);
        assert_eq!(out, "https://romm.lan/x/cover/small.png?ts=2026-08-11 10:48:15");
    }

    #[test]
    fn every_thumbnail_size_the_app_draws_takes_the_small_cover() {
        // The sizes a grid or a shelf actually asks for, once the UI scale and the panel density
        // are in: 294 is a Browse tile on a 1:1 panel, 367 the same tile on a 1.25 one, and 287 to
        // 316 the shelf cards. All of them past the small variant's own 272, and all of them the
        // case this exists for - falling back to `big` on any leaves that page as slow as before.
        for height_px in [287.0, 294.0, 316.0, 367.0] {
            let target = Target { height_px, box_ratio: Some(0.72) };
            assert_eq!(
                source_url("https://romm.lan/x/cover/big.png", target),
                "https://romm.lan/x/cover/small.png",
                "{height_px} px is a thumbnail",
            );
        }
    }

    #[test]
    fn a_small_cover_shorter_than_the_reference_servers_gives_way_to_the_big_one() {
        let png = |w: u32, h: u32| {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(w, h).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            bytes.into_inner()
        };
        // RomM's pairs: two fifths of the big one, whatever that is.
        let (snes, psp) = (png(198, 272), png(116, 200));
        for height_px in [294.0, 316.0, 367.0] {
            let tile = Target { height_px, box_ratio: Some(0.58) };
            assert!(!falls_short(&snes, tile), "{height_px}: the reference server's small cover serves");
            assert!(falls_short(&psp, tile), "{height_px}: a PSP case's would be stretched past 1.4x");
        }
        assert!(!falls_short(&psp, Target::tall(260.0)), "within what a thumbnail is allowed");
        assert!(!falls_short(b"not an image", Target::tall(900.0)), "nothing known, nothing fetched again");
    }

    #[test]
    fn the_feature_images_keep_the_original() {
        // The detail cover and the hero, at the same two panel densities. These are the sizes the
        // threshold is meant to sit below.
        for height_px in [437.0, 546.0, 552.0] {
            let target = Target { height_px, box_ratio: None };
            let url = "https://romm.lan/x/cover/big.png";
            assert_eq!(source_url(url, target), url, "{height_px} px earns the original");
        }
    }

    #[test]
    fn a_target_taller_than_the_small_variant_keeps_the_original() {
        // The detail page and the hero draw well past 272 px, where a small cover would be
        // upscaled - there the bytes are worth it.
        let big = Target { height_px: 437.0, box_ratio: None };
        let url = "https://romm.lan/x/cover/big.png";
        assert_eq!(source_url(url, big), url);
    }

    #[test]
    fn art_that_is_not_a_romm_cover_is_left_alone() {
        // Logos, screenshots and marquees have their own paths and no known variants.
        let tile = Target { height_px: 96.0, box_ratio: None };
        for url in [
            "https://romm.lan/assets/romm/resources/roms/15/1504/logo.png",
            "https://elsewhere.example/art.jpg",
        ] {
            assert_eq!(source_url(url, tile), url, "{url} has no variant to pick");
        }
    }

    #[test]
    fn cache_paths_are_stable_and_distinct_per_url() {
        // The file *name*, not the whole path. The directory comes from `XDG_CACHE_HOME`, which the
        // prepared-cache test sets process-wide while these run alongside it — comparing full paths
        // makes this fail whenever the two interleave, which is a race in the test rather than
        // anything wrong with the hash it is checking.
        let name = |url| cache_path(url).file_name().map(std::ffi::OsStr::to_owned);
        assert_eq!(name("https://a/x.png"), name("https://a/x.png"));
        assert_ne!(name("https://a/x.png"), name("https://a/y.png"));
    }
}

//! The in-memory library the whole UI reads from — a port of `browse/model/BrowseModels.kt`.
//!
//! Systems in rail order, games grouped per system and pre-sorted by [`Game::sort_title`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::romm::models::{Collection, Rom};
use crate::systems::SystemConfig;

/// A system (platform) in the library. [`System::cover_ratio`] is the width/height of that system's
/// physical box art — a PSP box is 0.58 where a SNES one is 1.37, and a shelf of mixed systems only
/// lines up because each card's width follows its own ratio at a shared height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct System {
    pub id: String,
    pub name: String,
    pub cover_ratio: f32,
}

/// One game as the browse UI sees it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Game {
    pub rom_id: i64,
    pub title: String,
    /// Lowercased, leading article stripped — "The Lost City" sorts under L.
    pub sort_title: String,
    pub system_id: String,
    /// None = no art; the card renders the generated placeholder instead.
    pub cover_url: Option<String>,
    /// RomM's `last_played` (ISO), when known — drives the continue-playing shelf.
    pub last_played: Option<String>,
    /// When the rom entered the library — drives the recently-added shelf.
    pub created_at: Option<String>,
    /// IGDB aggregate remapped to 0-5 stars, or None when unrated.
    pub rating: Option<f32>,
    pub summary: Option<String>,
    /// The game's transparent logo art as a **host-relative URL path**, already resolved through
    /// `RomSsMetadata::resource_url_path` — consumers only need [`crate::app::App::asset_url`] to
    /// add the host.
    pub logo_path: Option<String>,
    /// ScreenScraper's 745x745 composite. A fixed 1:1 shape across every rom, so unlike a cover it
    /// needs no per-game ratio — which is exactly why the hero's art panel uses it.
    pub miximage_path: Option<String>,
    /// ScreenScraper's 1024x512 banner. Drives the hero's backdrop.
    pub marquee_path: Option<String>,

    // --- Detail-page facts. All from the list response, so opening a game costs no fetch. ---
    /// Size on the server, for the SIZE stat.
    pub size_bytes: Option<i64>,
    /// Screenshot URLs, already absolute against the host.
    pub screenshots: Vec<String>,
    pub genres: Vec<String>,
    pub companies: Vec<String>,
    /// A display range like "1-2"; RomM sends a string, not a number.
    pub players: Option<String>,
    pub year: Option<i32>,
    /// The regional builds RomM grouped under this tile, by rom id — this game's own id is *not*
    /// in here.
    ///
    /// Kept because play history lives on whichever rom was actually launched, so a game played
    /// through a sibling has to surface as recent on the tile that represents it. `from_rom`
    /// resolves that at fetch time; a game read back from the cache has no rom to consult, and
    /// [`crate::cache`] re-stamps it against a fresh played map using exactly this list.
    #[serde(default)]
    pub sibling_ids: Vec<i64>,
    /// Every build under this tile as (rom id, file name without extension), this game's own
    /// first. A battery save is named after the file the emulator loaded, so this is how save sync
    /// tells which build — and so which RomM rom — a save on disk belongs to.
    #[serde(default)]
    pub file_stems: Vec<(i64, String)>,
    /// RomM's fingerprint of this game's own rom — its size and content hash — which a download
    /// records, so a changed rom on RomM shows as an update. See [`crate::downloads`].
    #[serde(default)]
    pub fingerprint: Option<Fingerprint>,
    /// More than one file: a disc set, or a folder of files — so Download has discs to offer.
    #[serde(default)]
    pub multi_file: bool,
    /// How long the main story takes, in seconds, by HowLongToBeat's players — RomM's figure.
    #[serde(default)]
    pub beat_secs: Option<u64>,
    /// RomM cannot serve the file — gone from its disk since the scan, or never there — so there
    /// is nothing to download.
    #[serde(default)]
    pub unavailable: bool,
    /// RomM's name for this build, from the tags it read out of the file name: `USA · Rev 1`.
    #[serde(default)]
    pub build: Option<String>,
    /// Other titles the game is known by — an abbreviation, a Japanese release's name — which
    /// search matches as well as the title.
    #[serde(default)]
    pub alt_names: Vec<String>,
    /// How many RetroAchievements achievements it has. The list itself is fetched by the detail
    /// page for the one game it shows.
    #[serde(default)]
    pub achievements: u32,
    /// Its RetroAchievements id, which this user's progress is keyed by.
    #[serde(default)]
    pub ra_id: Option<i64>,
    /// Where this user stands with it in RomM, and whether it is on their backlog or marked as being
    /// played now — Home's Now playing, Backlog and Finished shelves.
    #[serde(default)]
    pub status: Option<PlayStatus>,
    #[serde(default)]
    pub backlogged: bool,
    #[serde(default)]
    pub now_playing: bool,
}

/// RomM's per-user status for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayStatus {
    /// Started, not finished.
    Incomplete,
    Finished,
    Completed100,
    /// Won't play it again.
    Retired,
    /// Will never play it.
    NeverPlaying,
}

impl PlayStatus {
    pub fn from_romm(status: &str) -> Option<Self> {
        Some(match status {
            "incomplete" => Self::Incomplete,
            "finished" => Self::Finished,
            "completed_100" => Self::Completed100,
            "retired" => Self::Retired,
            "never_playing" => Self::NeverPlaying,
            _ => return None,
        })
    }

    /// As RomM's own interface names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Incomplete => "Incomplete",
            Self::Finished => "Finished",
            Self::Completed100 => "Completed 100%",
            Self::Retired => "Retired",
            Self::NeverPlaying => "Never playing",
        }
    }

    /// Reached the end, or everything.
    pub fn finished(self) -> bool {
        matches!(self, Self::Finished | Self::Completed100)
    }
}

/// What identifies one version of a rom on RomM: its size, and its content hash when RomM has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
}

impl Fingerprint {
    /// RomM's figures for a rom, when it gave a size at all.
    pub fn of(bytes: Option<i64>, md5: Option<&str>) -> Option<Self> {
        let bytes = u64::try_from(bytes?).ok().filter(|b| *b > 0)?;
        Some(Fingerprint { bytes, md5: md5.map(str::to_lowercase).filter(|m| !m.trim().is_empty()) })
    }

    /// Whether `other` is the same version: the same size, and the same hash where both have one.
    pub fn same(&self, other: &Fingerprint) -> bool {
        self.bytes == other.bytes && match (&self.md5, &other.md5) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }
}

/// Which rom a rom id stood for when it was written down: RomM's file name for it, and — for a rom
/// the library reads it for — when it entered RomM.
///
/// A rom id is only RomM's row number. A RomM rebuilt from an empty database numbers its roms again
/// from 1 as it scans them, so every id kept here from before names whatever game was scanned into
/// that place — and a save that followed the number would go up as another game's. So everything
/// that keeps an id keeps this beside it, and is checked against what RomM holds under the id now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// RomM's file name without its extension — `fs_name_no_ext`, the same for one rom whichever
    /// device asks.
    pub stem: String,
    /// RomM's `created_at` for the rom. A tile's own rom has one; its sibling builds, which RomM
    /// lists without it, do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub born: Option<String>,
}

impl Stamp {
    /// Whether `now` is the rom this was taken of: the same row in RomM's database, or the same file.
    ///
    /// Either is enough. A file renamed in RomM keeps its row; a library scanned into a rebuilt RomM
    /// gets new rows for the same files, and a game downloaded from one is still that game.
    pub fn same_rom(&self, now: &Stamp) -> bool {
        matches!((&self.born, &now.born), (Some(a), Some(b)) if a == b) || self.stem.eq_ignore_ascii_case(&now.stem)
    }

    /// Whether `now` is the very row this was taken of — when both know when it was made, by that;
    /// otherwise by the file.
    ///
    /// What a save's history follows, which is stricter than [`Stamp::same_rom`]: the versions this
    /// device last agreed on are rows of the RomM that held them, and a rebuilt one numbers its saves
    /// from 1 again too.
    pub fn same_row(&self, now: &Stamp) -> bool {
        match (&self.born, &now.born) {
            (Some(a), Some(b)) => a == b,
            _ => self.stem.eq_ignore_ascii_case(&now.stem),
        }
    }
}

/// A page of RomM's roms as the library's games — leaving out what this user hid in RomM, which
/// stays hidden here.
pub fn games_from(roms: &[Rom], system_id: &str, host: &str, played: &HashMap<i64, String>) -> Vec<Game> {
    roms.iter().filter(|rom| !rom.hidden()).map(|rom| Game::from_rom(rom, system_id, host, played)).collect()
}

impl Game {
    /// Every build under this tile — its own rom first, then its siblings.
    pub fn builds(&self) -> impl Iterator<Item = i64> + '_ {
        std::iter::once(self.rom_id).chain(self.sibling_ids.iter().copied())
    }

    /// Build a game from a RomM rom. `played` is the ungrouped rom id → `last_played` map, so a
    /// game played through a non-representative sibling build still surfaces as recent.
    pub fn from_rom(rom: &Rom, system_id: &str, host: &str, played: &HashMap<i64, String>) -> Self {
        let title = rom.display_name();
        let ss = rom.ss_metadata.clone().unwrap_or_default();
        let meta = rom.metadatum.as_ref();
        // The newest play across the tile's own rom and its siblings.
        let last_played = std::iter::once(rom.id)
            .chain(rom.sibling_roms.iter().map(|s| s.id))
            .filter_map(|id| played.get(&id).cloned())
            .max()
            .or_else(|| rom.rom_user.as_ref().and_then(|u| u.last_played.clone()));

        let file_stems = std::iter::once((rom.id, rom.file_stem()))
            .chain(rom.sibling_roms.iter().map(|s| (s.id, s.fs_name_no_ext.clone())))
            .filter_map(|(id, stem)| Some((id, stem.filter(|s| !s.trim().is_empty())?)))
            .collect();

        Self {
            rom_id: rom.id,
            sibling_ids: rom.sibling_roms.iter().map(|s| s.id).collect(),
            file_stems,
            sort_title: sort_title(&title),
            title,
            system_id: system_id.to_string(),
            cover_url: rom.cover_url(host),
            last_played,
            created_at: rom.created_at.clone(),
            rating: meta.and_then(|m| m.rating_out_of_five()),
            summary: rom.summary.clone().filter(|s| !s.trim().is_empty()),
            // Resolved to the resource root here rather than at render: the bare path RomM sends
            // serves its SPA shell, not the image — see `RomSsMetadata`.
            logo_path: ss.logo_url_path(),
            miximage_path: ss.miximage_url_path(),
            marquee_path: ss.marquee_url_path(),
            size_bytes: rom.fs_size_bytes.filter(|b| *b > 0),
            screenshots: rom
                .merged_screenshots
                .iter()
                .filter(|s| !s.trim().is_empty())
                .map(|s| absolute(s, host))
                .collect(),
            genres: meta.map(|m| m.genres.clone()).unwrap_or_default(),
            companies: meta.map(|m| m.companies.clone()).unwrap_or_default(),
            players: meta.and_then(|m| m.player_count.clone()).filter(|p| !p.trim().is_empty()),
            year: meta.and_then(|m| m.release_year()),
            fingerprint: Fingerprint::of(rom.fs_size_bytes, rom.md5_hash.as_deref()),
            multi_file: rom.has_multiple_files,
            beat_secs: rom
                .hltb_metadata
                .as_ref()
                .and_then(|h| h.main_story)
                .filter(|s| *s > 0)
                .map(|s| s as u64),
            unavailable: rom.missing_from_fs || rom.has_file_on_disk == Some(false),
            build: rom.build_label(),
            alt_names: rom
                .alternative_names
                .iter()
                .filter(|n| !n.trim().is_empty() && **n != rom.display_name())
                .cloned()
                .collect(),
            achievements: rom.merged_ra_metadata.as_ref().map_or(0, |ra| ra.achievements.len() as u32),
            ra_id: rom.ra_id,
            status: rom.rom_user.as_ref().and_then(|u| u.status.as_deref()).and_then(PlayStatus::from_romm),
            backlogged: rom.rom_user.as_ref().is_some_and(|u| u.backlogged),
            now_playing: rom.rom_user.as_ref().is_some_and(|u| u.now_playing),
        }
    }

    /// How long the main story takes, as the detail page shows it: minutes under an hour, else
    /// hours to the nearest half — HowLongToBeat's own rounding. N/A when RomM has no figure.
    pub fn beat_label(&self) -> String {
        let Some(secs) = self.beat_secs else { return "N/A".into() };
        if secs < 3600 {
            return format!("{} min", (secs as f64 / 60.0).round().max(1.0) as u64);
        }
        let halves = (secs as f64 / 1800.0).round() as u64;
        match (halves / 2, halves % 2) {
            (hours, 0) => format!("{hours} h"),
            (hours, _) => format!("{hours}½ h"),
        }
    }

    /// The size as the detail page shows it. RomM reports bytes; a rom is read in MB or GB.
    pub fn size_label(&self) -> Option<String> {
        let bytes = self.size_bytes? as f64;
        const MB: f64 = 1024.0 * 1024.0;
        Some(if bytes >= 1024.0 * MB {
            format!("{:.1} GB", bytes / (1024.0 * MB))
        } else if bytes >= MB {
            format!("{:.0} MB", bytes / MB)
        } else {
            format!("{:.0} KB", bytes / 1024.0)
        })
    }

    /// Just the date part of an ISO timestamp — what the hero and the card labels show.
    /// RomM holds no file for this game in any build it shows: its own is missing, and it has no
    /// other to offer instead.
    pub fn missing(&self) -> bool {
        self.unavailable && self.sibling_ids.is_empty()
    }

}

/// An ISO date as a person would say it: "1 Oct", with the year as well when it is not `this_year`'s.
/// None for anything that does not start with a date.
pub fn short_date(iso: &str, this_year: i64) -> Option<String> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let year: i64 = iso.get(0..4)?.parse().ok()?;
    let month: usize = iso.get(5..7)?.parse().ok()?;
    let day: u32 = iso.get(8..10)?.parse().ok()?;
    let name = MONTHS.get(month.checked_sub(1)?)?;
    Some(if year == this_year { format!("{day} {name}") } else { format!("{day} {name} {year}") })
}

/// The year it is now, by the system clock — what [`short_date`] leaves out.
pub fn this_year() -> i64 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400) as i64;
    crate::sync::names::civil_from_days(days).0
}

/// Resolve a possibly host-relative RomM path against the connected server.
fn absolute(path: &str, host: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if path.starts_with('/') {
        format!("{}{path}", host.trim_end_matches('/'))
    } else {
        format!("{}/{path}", host.trim_end_matches('/'))
    }
}

/// Test-only access to [`sort_title`], so another module's fixtures build the same keys the real
/// loader does rather than hand-rolling a second, subtly different rule.
#[cfg(test)]
pub fn test_sort_title(title: &str) -> String {
    sort_title(title)
}

/// Lowercase and drop a leading article, so "The Legend of Zelda" sorts under L.
fn sort_title(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.to_string();
        }
    }
    lower
}

/// The library: systems in order, games grouped per system.
///
/// There is deliberately no "all games in one flat list" *grid* accessor — grids are always
/// single-system. [`Library::all_games`] exists for the cross-system shelves, which are lists.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub systems: Vec<System>,
    /// Every game, one system after another in `systems` order and each system's by title — then
    /// any whose system is not in `systems`, which [`Library::all_games`] leaves out.
    games: Vec<Game>,
    /// Each system's games, as a range of `games`.
    by_system: HashMap<String, std::ops::Range<usize>>,
    /// Where the games of the systems in `systems` end.
    listed: usize,
    pub total_games: usize,
    /// Each game's place in `games`, by its rom id. What makes [`Library::game`] a lookup rather
    /// than a walk over every game — the detail page asks on every frame.
    by_id: HashMap<i64, usize>,
    /// A build's tile, by the build's rom id — a sibling's game is the tile it is filed under.
    tile_of: HashMap<i64, i64>,
    /// Which library this is: every one built or changed gets a new number, so what was worked out
    /// from one — the search index, Browse's sorted view — can tell it is out of date. 0 is the
    /// empty one.
    generation: u64,
}

/// A new [`Library::generation`].
fn next_generation() -> u64 {
    static BUILT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Library {
    pub fn new(systems: Vec<System>, games: Vec<Game>) -> Self {
        let total_games = games.len();
        let mut groups: HashMap<String, Vec<Game>> = HashMap::new();
        for game in games {
            groups.entry(game.system_id.clone()).or_default().push(game);
        }
        for list in groups.values_mut() {
            list.sort_by(|a, b| a.sort_title.cmp(&b.sort_title));
        }
        let mut games = Vec::with_capacity(total_games);
        let mut by_system = HashMap::new();
        let mut place = |id: String, list: Vec<Game>, games: &mut Vec<Game>| {
            let start = games.len();
            games.extend(list);
            by_system.insert(id, start..games.len());
        };
        for system in &systems {
            if let Some(list) = groups.remove(&system.id) {
                place(system.id.clone(), list, &mut games);
            }
        }
        let listed = games.len();
        let mut rest: Vec<(String, Vec<Game>)> = groups.into_iter().collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, list) in rest {
            place(id, list, &mut games);
        }
        let by_id = games.iter().enumerate().map(|(at, g)| (g.rom_id, at)).collect();
        let mut tile_of = HashMap::new();
        for game in &games {
            for sibling in &game.sibling_ids {
                tile_of.entry(*sibling).or_insert(game.rom_id);
            }
        }
        Self { systems, games, by_system, listed, total_games, by_id, tile_of, generation: next_generation() }
    }

    /// Which library this is — see the field.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The game whose tile is `rom_id`.
    pub fn game(&self, rom_id: i64) -> Option<&Game> {
        self.games.get(*self.by_id.get(&rom_id)?)
    }

    /// The game `rom_id` belongs to: its own tile, or the tile it is a build of.
    pub fn game_of_build(&self, rom_id: i64) -> Option<&Game> {
        self.game(rom_id).or_else(|| self.game(*self.tile_of.get(&rom_id)?))
    }

    /// A game's place in this library — stable for as long as [`Library::generation`] is.
    pub fn index_of(&self, rom_id: i64) -> Option<usize> {
        self.by_id.get(&rom_id).copied()
    }

    /// The game at a place [`Library::index_of`] gave.
    pub fn at(&self, index: usize) -> &Game {
        &self.games[index]
    }

    pub fn is_empty(&self) -> bool {
        self.total_games == 0
    }

    pub fn games_for(&self, system_id: &str) -> &[Game] {
        self.by_system.get(system_id).map_or(&[], |range| &self.games[range.clone()])
    }

    /// What RomM holds under `rom_id` now, by its [`Stamp`] — a tile's own rom or one of its builds.
    pub fn stamp(&self, rom_id: i64) -> Option<Stamp> {
        let game = self.game_of_build(rom_id)?;
        let (_, stem) = game.file_stems.iter().find(|(id, _)| *id == rom_id)?;
        let born = if game.rom_id == rom_id { game.created_at.clone() } else { None };
        Some(Stamp { stem: stem.clone(), born })
    }

    /// The builds of `system_id` whose file name is one of `names`, by rom id — where a game kept
    /// under an id RomM has since given to another is looked for.
    pub fn builds_named(&self, system_id: &str, names: &[String]) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .games_for(system_id)
            .iter()
            .flat_map(|g| &g.file_stems)
            .filter(|(_, stem)| names.iter().any(|n| n.eq_ignore_ascii_case(stem)))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every game in system order — for cross-system *lists*; grids stay single-system.
    pub fn all_games(&self) -> &[Game] {
        &self.games[..self.listed]
    }

    pub fn system_name<'a>(&'a self, system_id: &'a str) -> &'a str {
        self.systems
            .iter()
            .find(|s| s.id == system_id)
            .map(|s| s.name.as_str())
            .unwrap_or(system_id)
    }

    pub fn cover_ratio(&self, system_id: &str) -> f32 {
        self.systems
            .iter()
            .find(|s| s.id == system_id)
            .map(|s| s.cover_ratio)
            .unwrap_or(SystemConfig::DEFAULT_RATIO)
    }

    /// The last `limit` played, most recent first — the continue-playing shelf.
    pub fn continue_playing(&self, limit: usize) -> Vec<&Game> {
        let mut played: Vec<&Game> = self
            .all_games().iter()
            .filter(|g| g.last_played.as_deref().is_some_and(|s| !s.trim().is_empty()))
            .collect();
        played.sort_by(|a, b| b.last_played.cmp(&a.last_played));
        played.truncate(limit);
        played
    }

    /// The newest arrivals — Home's second shelf.
    pub fn recently_added(&self, limit: usize) -> Vec<&Game> {
        let mut recent: Vec<&Game> = self
            .all_games().iter()
            .filter(|g| g.created_at.as_deref().is_some_and(|s| !s.trim().is_empty()))
            .collect();
        recent.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        recent.truncate(limit);
        recent
    }

    /// Record that `rom_id` was just played, at `when` (RFC 3339) — what RomM now says too, after a
    /// session is posted, without re-reading the library to hear it.
    pub fn mark_played(&mut self, rom_id: i64, when: String) {
        let tile = if self.by_id.contains_key(&rom_id) { Some(rom_id) } else { self.tile_of.get(&rom_id).copied() };
        if let Some(at) = tile.and_then(|id| self.index_of(id)) {
            self.games[at].last_played = Some(when);
            // The "played" order has changed under anything sorted by it.
            self.generation = next_generation();
        }
    }

    /// The game played last — what Home opens on, at the head of Continue playing. None on a fresh
    /// pairing, before anything has been played.
    #[cfg(test)]
    pub fn hero(&self) -> Option<&Game> {
        self.continue_playing(1).into_iter().next()
    }
}

/// One Home shelf: a titled horizontal row of covers.
#[derive(Debug, Clone)]
pub struct Shelf {
    /// Stable across refreshes — the per-shelf cursor is keyed by it, because rows are not stable:
    /// a shelf that empties out stops being drawn and every shelf below it shifts up.
    pub id: String,
    pub title: String,
    /// What the shelf is ordered by, or why it holds what it does — said beside its title.
    pub note: String,
    pub games: Vec<Game>,
}

/// The full Home shelf list: Continue playing, On this device, Recently added, then a shelf per
/// collection.
///
/// `installed` is the set of rom ids actually on disk. It is a parameter rather than something read
/// off the games because what is downloaded is live state that changes while Home is open — see
/// [`crate::downloads`] — where everything else on a shelf is a property of the library snapshot.
///
/// The game played last leads Continue playing rather than being taken out of it for a hero of its
/// own: Home's spotlight is whichever card the cursor is on, and the page opens on that one.
pub fn home_shelves(
    library: &Library,
    collections: &[Collection],
    installed: &std::collections::HashSet<i64>,
) -> Vec<Shelf> {
    let mut shelves = Vec::new();

    shelves.push(Shelf {
        id: "continue".into(),
        title: "Continue playing".into(),
        note: "Last played first".into(),
        games: library.continue_playing(10).into_iter().cloned().collect(),
    });

    // Second, above Recently added: on a handheld, "what can I actually play right now" is the
    // question asked more often than "what is new on the server". Empty until something is
    // downloaded, and an empty shelf is not drawn — so it appears the first time it has an answer.
    shelves.push(Shelf {
        id: "installed".into(),
        title: "On this device".into(),
        note: "Ready to play".into(),
        games: library
            .all_games().iter()
            .filter(|g| g.builds().any(|id| installed.contains(&id)))
            .cloned()
            .collect(),
    });

    // What this user marked in RomM, which is where the marking is done: playing now, and the
    // backlog. Each is empty for anyone who never marks anything, and an empty shelf is not drawn.
    let marked = |keep: &dyn Fn(&Game) -> bool| -> Vec<Game> {
        let mut games: Vec<Game> = library.all_games().iter().filter(|g| keep(g)).cloned().collect();
        games.sort_by(|a, b| a.sort_title.cmp(&b.sort_title));
        games
    };
    shelves.push(Shelf {
        id: "now-playing".into(),
        title: "Now playing".into(),
        note: "Marked in RomM".into(),
        games: marked(&|g| g.now_playing),
    });
    shelves.push(Shelf {
        id: "backlog".into(),
        title: "Backlog".into(),
        note: "Marked in RomM".into(),
        games: marked(&|g| g.backlogged),
    });

    shelves.push(Shelf {
        id: "recent".into(),
        title: "Recently added".into(),
        note: "Newest first".into(),
        games: library.recently_added(12).into_iter().cloned().collect(),
    });

    // Last of the library's own: finished games are the ones least likely to be what is played next.
    shelves.push(Shelf {
        id: "finished".into(),
        title: "Finished".into(),
        note: "Finished or completed in RomM".into(),
        games: marked(&|g| g.status.is_some_and(PlayStatus::finished)),
    });

    shelves.extend(collection_shelves(collections, library));
    // A shelf with no items isn't shown or focusable.
    shelves.retain(|s| !s.games.is_empty());
    shelves
}

/// One Home shelf per manual RomM collection, in server order.
///
/// The favourites collection is skipped (it has its own filter chip elsewhere), and so are the
/// virtual/smart ones — they are server-side queries that can run to thousands of games, which is a
/// browse view's job, not a shelf's. A collection whose games aren't in the library is dropped
/// rather than shown empty.
fn collection_shelves(collections: &[Collection], library: &Library) -> Vec<Shelf> {
    let by_id: HashMap<i64, &Game> = library.all_games().iter().map(|g| (g.rom_id, g)).collect();
    collections
        .iter()
        .filter(|c| !c.is_favorite && !c.is_virtual && !c.is_smart)
        .filter_map(|c| {
            let games: Vec<Game> = c.rom_ids.iter().filter_map(|id| by_id.get(id).map(|g| (*g).clone())).collect();
            (!games.is_empty()).then(|| Shelf {
                id: format!("collection-{}", c.id),
                title: c.display_name(),
                note: match games.len() {
                    1 => "Collection · 1 game".into(),
                    n => format!("Collection · {n} games"),
                },
                games,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_romm_says_of_a_game_beyond_its_metadata_is_carried() {
        let json = serde_json::json!({
            "id": 37, "name": "Chrono Trigger", "fs_name": "Chrono Trigger (USA) (Rev 1).sfc",
            "regions": ["USA"], "revision": "1", "tags": ["Beta"],
            "alternative_names": ["CT", "Chrono Trigger"],
            "missing_from_fs": false, "has_file_on_disk": false,
            "merged_ra_metadata": { "achievements": [{ "title": "One" }, { "title": "Two" }] },
            "rom_user": { "status": "completed_100", "backlogged": true, "now_playing": false, "hidden": true }
        });
        let rom: Rom = serde_json::from_value(json).unwrap();
        assert!(rom.hidden(), "hidden is the loader's to leave out");
        let game = Game::from_rom(&rom, "snes", "https://romm.local", &HashMap::new());
        assert_eq!(game.build.as_deref(), Some("USA · Rev 1 · Beta"));
        assert_eq!(game.alt_names, ["CT"], "the title itself is not another name");
        assert!(game.unavailable, "RomM holds no file for it");
        assert_eq!(game.achievements, 2);
        assert_eq!(game.status, Some(PlayStatus::Completed100));
        assert!(game.status.unwrap().finished());
        assert!(game.backlogged && !game.now_playing);
    }

    #[test]
    fn a_game_is_found_by_its_rom_id_and_by_any_of_its_builds() {
        let fixture = crate::fixtures::library(12);
        let mut games: Vec<Game> = fixture.all_games().to_vec();
        games[4].sibling_ids = vec![9001, 9002];
        let library = Library::new(fixture.systems, games.clone());
        for game in &games {
            assert_eq!(library.game(game.rom_id).map(|g| &g.title), Some(&game.title));
        }
        assert!(library.game(9001).is_none(), "a build is not a tile of its own");
        assert_eq!(library.game_of_build(9002).map(|g| g.rom_id), Some(games[4].rom_id), "but it is found as its game's");
        assert!(library.game_of_build(424242).is_none());
    }

    #[test]
    fn home_has_now_playing_and_backlog_shelves_and_finished_games_further_down() {
        let mut library = crate::fixtures::library(12);
        let mut games: Vec<Game> = library.all_games().to_vec();
        games[1].now_playing = true;
        games[2].backlogged = true;
        games[3].backlogged = true;
        games[4].status = Some(PlayStatus::Finished);
        games[5].status = Some(PlayStatus::Retired);
        library = Library::new(library.systems.clone(), games.clone());
        let shelves = home_shelves(&library, &[], &std::collections::HashSet::new());
        let ids: Vec<&str> = shelves.iter().map(|s| s.id.as_str()).collect();
        let at = |id: &str| ids.iter().position(|s| *s == id).unwrap_or_else(|| panic!("{id} in {ids:?}"));
        assert!(at("now-playing") < at("backlog") && at("backlog") < at("recent") && at("recent") < at("finished"));
        let of = |id: &str| shelves[at(id)].games.iter().map(|g| g.rom_id).collect::<Vec<_>>();
        assert_eq!(of("now-playing"), [games[1].rom_id]);
        assert_eq!(of("backlog").len(), 2);
        assert_eq!(of("finished"), [games[4].rom_id], "retired is not finished");
    }

    #[test]
    fn how_long_to_beat_reads_in_minutes_then_half_hours() {
        let game = |secs: Option<u64>| Game { beat_secs: secs, ..crate::fixtures::library(1).all_games()[0].clone() };
        assert_eq!(game(None).beat_label(), "N/A");
        assert_eq!(game(Some(45 * 60 + 10)).beat_label(), "45 min");
        assert_eq!(game(Some(36_688)).beat_label(), "10 h", "10.19 hours");
        assert_eq!(game(Some(37_900)).beat_label(), "10½ h", "10.53 hours");
        assert_eq!(game(Some(3_500)).beat_label(), "58 min");
        assert_eq!(game(Some(3_600 * 40)).beat_label(), "40 h");
    }
    use std::collections::HashSet;

    fn game(id: i64, title: &str, played: Option<&str>, created: Option<&str>) -> Game {
        Game {
            rom_id: id,
            title: title.into(),
            sort_title: sort_title(title),
            system_id: "snes".into(),
            last_played: played.map(str::to_string),
            created_at: created.map(str::to_string),
            ..Default::default()
        }
    }

    fn library(games: Vec<Game>) -> Library {
        Library::new(
            vec![System { id: "snes".into(), name: "Super Nintendo".into(), cover_ratio: 1.37 }],
            games,
        )
    }

    #[test]
    fn a_rom_is_the_same_by_its_row_in_romm_or_its_file_and_its_history_only_by_its_row() {
        let stamp = |stem: &str, born: Option<&str>| Stamp { stem: stem.into(), born: born.map(str::to_string) };
        let mana = stamp("Secret of Mana (USA)", Some("2026-08-01"));
        assert!(mana.same_rom(&stamp("Secret of Mana (USA) (Rev 1)", Some("2026-08-01"))), "renamed in RomM");
        assert!(mana.same_rom(&stamp("secret of mana (usa)", Some("2026-10-12"))), "scanned into a rebuilt RomM");
        assert!(!mana.same_rom(&stamp("Zipang (Japan)", Some("2026-10-12"))));
        assert!(mana.same_rom(&stamp("Secret of Mana (USA)", None)), "a sibling build, listed without a date");
        assert!(mana.same_row(&stamp("Secret of Mana (USA) (Rev 1)", Some("2026-08-01"))));
        assert!(!mana.same_row(&stamp("Secret of Mana (USA)", Some("2026-10-12"))), "a rebuilt RomM's row is a new one");
        assert!(mana.same_row(&stamp("Secret of Mana (USA)", None)));
    }

    #[test]
    fn a_build_s_stamp_and_where_a_file_name_is_now_come_from_the_library() {
        let mut zelda = game(10, "Zelda", None, Some("2026-08-01"));
        zelda.sibling_ids = vec![11];
        zelda.file_stems = vec![(10, "Zelda (USA)".into()), (11, "Zelda (Europe)".into())];
        let library = library(vec![zelda]);
        assert_eq!(library.stamp(10), Some(Stamp { stem: "Zelda (USA)".into(), born: Some("2026-08-01".into()) }));
        assert_eq!(library.stamp(11), Some(Stamp { stem: "Zelda (Europe)".into(), born: None }), "RomM lists a sibling without its date");
        assert_eq!(library.stamp(12), None);
        assert_eq!(library.builds_named("snes", &["zelda (europe)".into()]), [11]);
        assert!(library.builds_named("gba", &["Zelda (Europe)".into()]).is_empty(), "only on the game's own system");
    }

    #[test]
    fn leading_articles_do_not_bunch_games_under_t() {
        assert_eq!(sort_title("The Legend of Zelda"), "legend of zelda");
        assert_eq!(sort_title("A Link to the Past"), "link to the past");
        assert_eq!(sort_title("Chrono Trigger"), "chrono trigger");
    }

    #[test]
    fn shelves_order_by_recency_and_lead_with_the_game_played_last() {
        let lib = library(vec![
            game(1, "Old", Some("2024-01-01T00:00:00"), Some("2024-01-01")),
            game(2, "Newest", Some("2026-03-01T00:00:00"), Some("2026-03-01")),
            game(3, "Never played", None, Some("2026-02-01")),
        ]);
        let shelves = home_shelves(&lib, &[], &HashSet::new());
        let continuing = shelves.iter().find(|s| s.id == "continue").expect("continue shelf");
        assert_eq!(
            continuing.games.iter().map(|g| g.rom_id).collect::<Vec<_>>(),
            vec![2, 1],
            "the game played last is the first card, where Home opens"
        );

        let recent = shelves.iter().find(|s| s.id == "recent").expect("recent shelf");
        assert_eq!(recent.games.iter().map(|g| g.rom_id).collect::<Vec<_>>(), vec![2, 3, 1]);
    }

    #[test]
    fn a_library_with_nothing_played_has_no_continue_shelf() {
        let lib = library(vec![game(1, "Fresh", None, Some("2026-01-01"))]);
        assert!(lib.hero().is_none());
        let shelves = home_shelves(&lib, &[], &HashSet::new());
        assert!(shelves.iter().all(|s| s.id != "continue"), "an empty shelf is not drawn");
        assert_eq!(shelves.len(), 1);
    }

    #[test]
    fn collection_shelves_skip_favourites_and_games_not_in_the_library() {
        let lib = library(vec![game(1, "Have it", None, Some("2026-01-01"))]);
        let collections = vec![
            Collection { id: 1, name: Some("Favourites".into()), rom_count: 1, rom_ids: vec![1], is_favorite: true, is_virtual: false, is_smart: false },
            Collection { id: 2, name: Some("RPGs".into()), rom_count: 2, rom_ids: vec![1, 99], is_favorite: false, is_virtual: false, is_smart: false },
            Collection { id: 3, name: Some("Elsewhere".into()), rom_count: 1, rom_ids: vec![99], is_favorite: false, is_virtual: false, is_smart: false },
        ];
        let shelves = home_shelves(&lib, &collections, &HashSet::new());
        let names: Vec<&str> = shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(names, vec!["Recently added", "RPGs"]);
        // The rom that is in the collection but not in the library is dropped, not rendered blank.
        assert_eq!(shelves[1].games.len(), 1);
    }

    /// Zelda's USA build, with its Europe build grouped under it.
    fn zelda() -> Rom {
        Rom {
            id: 10,
            name: Some("Zelda".into()),
            fs_name: Some("Zelda (USA).sfc".into()), fs_name_no_ext: None, fs_extension: Some("sfc".into()),
            file_name: None, platform_id: None, platform_slug: None, summary: None,
            path_cover_small: None, path_cover_s: None, path_cover_large: None, path_cover_l: None,
            rom_user: None, metadatum: None, created_at: None, updated_at: None,
            sibling_roms: vec![crate::romm::models::SiblingRom {
                id: 11,
                fs_name_no_ext: Some("Zelda (Europe)".into()),
            }],
            ss_metadata: None,
            fs_size_bytes: None,
            merged_screenshots: Vec::new(),
            md5_hash: None,
            has_multiple_files: false,
            hltb_metadata: None,
            missing_from_fs: false,
            has_file_on_disk: None,
            regions: vec!["USA".into()],
            revision: Some("1".into()),
            tags: Vec::new(),
            alternative_names: vec!["LttP".into()],
            merged_ra_metadata: None,
            ra_id: None,
        }
    }

    #[test]
    fn a_sibling_build_s_play_counts_as_the_game_being_played() {
        let played = HashMap::from([(11, "2026-05-05T00:00:00".to_string())]);
        let g = Game::from_rom(&zelda(), "snes", "https://romm.local", &played);
        assert_eq!(g.last_played.as_deref(), Some("2026-05-05T00:00:00"));
    }

    #[test]
    fn every_build_s_file_stem_is_kept_the_tile_s_own_first() {
        // Without the extension — a save is named after the file, not the file name.
        let g = Game::from_rom(&zelda(), "snes", "https://romm.local", &HashMap::new());
        assert_eq!(g.file_stems, vec![(10, "Zelda (USA)".to_string()), (11, "Zelda (Europe)".to_string())]);
    }
}

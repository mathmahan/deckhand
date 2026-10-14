//! Serde models for the RomM HTTP API — a port of `net/Models.kt`, trimmed to what the Home view
//! and pairing actually read.
//!
//! Every field is `Option`/`#[serde(default)]` for the same reason the Kotlin models are: RomM's
//! field names vary across server versions, and a library fetch that fails to decode one rom
//! should lose that rom, not the page.

use serde::{Deserialize, Serialize};

use crate::library::Stamp;

/// `GET /api/users/me` — fetched right after pairing to prove the token works, and by the detail
/// page for which achievements this user has earned.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    /// The RetroAchievements account linked to this RomM user, if any.
    #[serde(default)]
    pub ra_username: Option<String>,
    /// That account's progress, per game, as RomM last fetched it from RetroAchievements.
    #[serde(default)]
    pub ra_progression: Option<RaProgression>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RaProgression {
    #[serde(default, deserialize_with = "null_as_empty")]
    pub results: Vec<RaGameProgress>,
}

/// One game's RetroAchievements progress for this user.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RaGameProgress {
    /// The game's RetroAchievements id — [`Rom::ra_id`].
    #[serde(default)]
    pub rom_ra_id: Option<i64>,
    /// How many of the game's achievements this user has — RetroAchievements' own count, which is
    /// exact where matching the list below by badge is not.
    #[serde(default)]
    pub num_awarded: Option<i64>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub earned_achievements: Vec<EarnedAchievement>,
}

/// An achievement this user has, named by its badge — RomM's key for it here.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct EarnedAchievement {
    /// The achievement's badge id: [`Achievement::badge_id`].
    pub id: String,
    /// When, as RetroAchievements gives it: `2026-08-06 19:19:21`.
    #[serde(default)]
    pub date: Option<String>,
    /// When in hardcore mode, if it was.
    #[serde(default)]
    pub date_hardcore: Option<String>,
}

/// RomM's answer to a device asking to be approved (`POST /api/auth/device/init`).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthStart {
    /// The secret this device polls with.
    pub device_code: String,
    /// The short code the approving page shows, and the QR carries.
    pub user_code: String,
    /// The web page that approves it, with the code filled in — relative to the server.
    #[serde(default)]
    pub verification_path_complete: Option<String>,
    #[serde(default)]
    pub verification_path: Option<String>,
    /// How long the code lives, and how often to ask, in seconds.
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub interval: Option<u64>,
}

/// The token, once the device has been approved (`POST /api/auth/device/token`).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthToken {
    pub access_token: String,
    /// The RomM device the token is bound to — what save sync sends as this install.
    #[serde(default)]
    pub device_id: Option<String>,
}

/// The pairing response (`ClientTokenCreateSchema`): the secret plus the identity RomM gave it,
/// which is what makes it revocable.
#[derive(Debug, Clone, Deserialize)]
pub struct PairedToken {
    /// RomM sends the secret as `raw_token`; [`crate::romm::client`] falls back to other spellings.
    #[serde(default)]
    pub raw_token: Option<String>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub fs_slug: Option<String>,
    #[serde(default)]
    pub rom_count: i64,
}

impl Platform {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.slug.clone())
            .or_else(|| self.fs_slug.clone())
            .unwrap_or_else(|| format!("Platform #{}", self.id))
    }
}

/// Per-user data RomM keeps for a game (`rom_user`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RomUser {
    #[serde(default)]
    pub last_played: Option<String>,
    #[serde(default)]
    pub now_playing: bool,
    /// Hidden by this user in RomM: left out of the library here too.
    #[serde(default)]
    pub hidden: bool,
    /// On this user's backlog — one of Home's shelves.
    #[serde(default)]
    pub backlogged: bool,
    /// Where this user stands with it: `incomplete`, `finished`, `completed_100`, `retired`,
    /// `never_playing`, or nothing set.
    #[serde(default)]
    pub status: Option<String>,
}

/// Other builds of the same game, collapsed under one tile by `group_by_meta_id`.
#[derive(Debug, Clone, Deserialize)]
pub struct SiblingRom {
    pub id: i64,
    /// The build's file name without its extension — what an emulator names its save after, and
    /// so how a save made with a regional build finds that build rather than the tile's.
    #[serde(default)]
    pub fs_name_no_ext: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RomMetadata {
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub companies: Vec<String>,
    /// How many can play, as a display range ("1-2"). RomM sends a string, not a number.
    #[serde(default)]
    pub player_count: Option<String>,
    /// RomM sends this in **milliseconds**, where IGDB's own field is seconds — verified against a
    /// live server (Legend of Mana is `931996800000`, i.e. 1999-07-15). Anything reading it must
    /// check the magnitude rather than trusting either: taken as seconds regardless, every game
    /// renders as year 31501.
    #[serde(default)]
    pub first_release_date: Option<i64>,
    #[serde(default)]
    pub average_rating: Option<f64>,
}

impl RomMetadata {
    /// Anything past this can only be a millisecond value: as seconds it would land ~29,000 years
    /// from now. RomM sends milliseconds here where IGDB's own field is seconds, so this reads the
    /// magnitude rather than trusting either — taking it as seconds regardless rendered every game
    /// as year 31501.
    const MILLIS_THRESHOLD: i64 = 100_000_000_000;

    pub fn release_year(&self) -> Option<i32> {
        let raw = self.first_release_date?;
        let seconds = if raw.abs() >= Self::MILLIS_THRESHOLD { raw / 1000 } else { raw };
        // 1970 + whole years. Good enough for a year label and avoids a date dependency.
        Some(1970 + (seconds as f64 / 31_556_952.0).floor() as i32)
    }

    /// IGDB's 0-100 aggregate remapped to 0-5 stars, or None when unrated.
    pub fn rating_out_of_five(&self) -> Option<f32> {
        self.average_rating
            .filter(|r| *r > 0.0)
            .map(|r| (r / 20.0) as f32)
    }
}

/// ScreenScraper artwork paths. See [`RomSsMetadata::resource_url_path`] for why the prefix matters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RomSsMetadata {
    /// Transparent title-art wordmark.
    #[serde(default)]
    pub logo_path: Option<String>,
    /// A 1024x512 banner: the platform in a sidebar down the left quarter, then the game's wordmark
    /// centred over a dark textured field. Not an arcade marquee despite the name.
    #[serde(default)]
    pub marquee_path: Option<String>,
    /// A 745x745 composite on transparency: box art, a gameplay screenshot and the physical media.
    /// Note it carries the wordmark too — anything drawing this beside `logo_path` shows the same
    /// lettering twice.
    #[serde(default)]
    pub miximage_v2_path: Option<String>,
}

impl RomSsMetadata {
    /// `ss_metadata`'s paths are stored **relative to RomM's resource root**
    /// (`roms/9/891/logo/logo.png`), unlike `path_cover_large`, which arrives already absolute.
    /// Requesting the bare path returns RomM's SPA shell — HTTP 200 with an HTML body — so a
    /// consumer that skipped this prefix would not error, it would just silently render its
    /// fallback forever.
    pub const RESOURCE_ROOT: &'static str = "/assets/romm/resources/";

    fn resource_url_path(path: &Option<String>) -> Option<String> {
        let p = path.as_deref().map(str::trim).filter(|p| !p.is_empty())?;
        if p.starts_with("http://") || p.starts_with("https://") || p.starts_with('/') {
            Some(p.to_string())
        } else {
            Some(format!("{}{p}", Self::RESOURCE_ROOT))
        }
    }

    pub fn logo_url_path(&self) -> Option<String> {
        Self::resource_url_path(&self.logo_path)
    }
    pub fn marquee_url_path(&self) -> Option<String> {
        Self::resource_url_path(&self.marquee_path)
    }
    pub fn miximage_url_path(&self) -> Option<String> {
        Self::resource_url_path(&self.miximage_v2_path)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rom {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fs_name: Option<String>,
    #[serde(default)]
    pub fs_name_no_ext: Option<String>,
    #[serde(default)]
    pub fs_extension: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub platform_id: Option<i64>,
    #[serde(default)]
    pub platform_slug: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    // RomM field names vary across versions; accept the long and short variants.
    #[serde(default)]
    pub path_cover_small: Option<String>,
    #[serde(default)]
    pub path_cover_s: Option<String>,
    #[serde(default)]
    pub path_cover_large: Option<String>,
    #[serde(default)]
    pub path_cover_l: Option<String>,
    #[serde(default)]
    pub rom_user: Option<RomUser>,
    /// The list endpoint carries this too, so the library knows each game's rating without a
    /// per-game detail fetch.
    #[serde(default)]
    pub metadatum: Option<RomMetadata>,
    /// When RomM added this rom — drives Home's "Recently added" shelf.
    #[serde(default)]
    pub created_at: Option<String>,
    /// When RomM last changed the rom — a rescrape or a rename moves it as much as an edit does.
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub sibling_roms: Vec<SiblingRom>,
    #[serde(default)]
    pub ss_metadata: Option<RomSsMetadata>,
    /// On-disk size of the rom on the server, for the detail page's SIZE stat.
    #[serde(default)]
    pub fs_size_bytes: Option<i64>,
    /// Screenshot paths, already absolute (`/assets/romm/resources/…`) unlike `ss_metadata`'s.
    /// Carried on the *list* response, so the detail page needs no per-game fetch.
    #[serde(default)]
    pub merged_screenshots: Vec<String>,
    /// RomM's hash over the rom's content. With [`Rom::fs_size_bytes`] it is what tells a game
    /// downloaded here from the one RomM holds now — see [`crate::downloads`]'s update check. The
    /// list carries it where it carries no per-file detail at all.
    #[serde(default)]
    pub md5_hash: Option<String>,
    /// More than one file: a multi-disc set, or a game kept as a folder of files.
    #[serde(default)]
    pub has_multiple_files: bool,
    /// HowLongToBeat's figures, when RomM has matched the game there.
    #[serde(default)]
    pub hltb_metadata: Option<HltbMetadata>,
    /// The game's RetroAchievements id, which keys this user's progress on it.
    #[serde(default)]
    pub ra_id: Option<i64>,
    /// The file has gone from RomM's own disk since it was scanned: there is nothing to download.
    #[serde(default)]
    pub missing_from_fs: bool,
    /// False for a game RomM knows of but holds no file for. Absent on an older RomM, which is read
    /// as having one.
    #[serde(default)]
    pub has_file_on_disk: Option<bool>,
    /// What RomM read out of the file name's tags: `["USA"]`, a revision of `"1"`, and whatever
    /// else was tagged (`Beta`, `Proto`).
    #[serde(default, deserialize_with = "null_as_empty")]
    pub regions: Vec<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub tags: Vec<String>,
    /// Other titles the game is known by — an abbreviation, the Japanese release's name.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub alternative_names: Vec<String>,
    /// RetroAchievements' list for the game. Counted here and not kept: the whole list runs to
    /// dozens of entries a game, and the detail page fetches it for the one game it shows.
    #[serde(default)]
    pub merged_ra_metadata: Option<RaCount>,
}

/// RetroAchievements' list for a game, counted without being kept.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RaCount {
    #[serde(default, deserialize_with = "null_as_empty")]
    pub achievements: Vec<serde::de::IgnoredAny>,
}

/// RetroAchievements' list for a game, whole — what the detail page shows.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RaMetadata {
    #[serde(default, deserialize_with = "null_as_empty")]
    pub achievements: Vec<Achievement>,
}

/// A list RomM may send as `null` rather than leave out, read as empty: one game's odd field must
/// not cost the whole page of games it arrived in.
fn null_as_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

/// One RetroAchievements achievement, as RomM carries it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Achievement {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub points: Option<i64>,
    /// How many RetroAchievements players have unlocked it — its rarity.
    #[serde(default)]
    pub num_awarded: Option<i64>,
    /// The badge's id, which is how RomM names an achievement this user has earned.
    #[serde(default)]
    pub badge_id: Option<String>,
    /// The badge, kept on RomM's own disk (`/assets/romm/resources/…`).
    #[serde(default)]
    pub badge_path: Option<String>,
    /// The same badge on RetroAchievements' site, for a RomM that has not stored it.
    #[serde(default)]
    pub badge_url: Option<String>,
    /// The greyed-out badge for one not yet earned, on RomM's disk and on RetroAchievements'.
    #[serde(default)]
    pub badge_path_lock: Option<String>,
    #[serde(default)]
    pub badge_url_lock: Option<String>,
    #[serde(default)]
    pub display_order: Option<i64>,
    /// `progression`, `win_condition`, `missable` — or nothing.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// HowLongToBeat's averages for a game, in seconds, as RomM keeps them. Each is absent when nobody
/// has logged that kind of playthrough.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HltbMetadata {
    /// The main story alone — HowLongToBeat's headline figure.
    #[serde(default)]
    pub main_story: Option<i64>,
    #[serde(default)]
    pub main_plus_extra: Option<i64>,
    #[serde(default)]
    pub completionist: Option<i64>,
}

impl Rom {
    /// Hidden by this user in RomM.
    pub fn hidden(&self) -> bool {
        self.rom_user.as_ref().is_some_and(|u| u.hidden)
    }

    /// RomM's name for this build, from the tags it read out of the file name: `USA · Rev 1`.
    /// None when it read none.
    pub fn build_label(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if !self.regions.is_empty() {
            parts.push(self.regions.join(", "));
        }
        if let Some(revision) = self.revision.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            parts.push(format!("Rev {revision}"));
        }
        parts.extend(self.tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()));
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The rom's file name without its extension, which is what RetroArch names a battery save
    /// after: `Chrono Trigger (USA).sfc` saves as `Chrono Trigger (USA).srm`.
    pub fn file_stem(&self) -> Option<String> {
        file_stem(self.fs_name_no_ext.as_deref(), self.fs_name.as_deref(), self.fs_extension.as_deref())
    }

    /// Which rom this is, as everything that keeps its id keeps it — see [`Stamp`].
    pub fn stamp(&self) -> Option<Stamp> {
        Some(Stamp { stem: self.file_stem()?, born: self.created_at.clone() })
    }

    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.fs_name.clone())
            .or_else(|| self.file_name.clone())
            .unwrap_or_else(|| format!("Game #{}", self.id))
    }

    /// Resolve a usable cover URL against `host` from RomM's *own* stored assets, largest first.
    ///
    /// The remote `url_cover` (IGDB/ScreenScraper) is deliberately not a fallback: RomM is the
    /// single source of truth for art, so a game RomM hasn't stored a cover for shows the designed
    /// placeholder rather than reaching out to a third-party CDN.
    pub fn cover_url(&self, host: &str) -> Option<String> {
        let candidate = [
            &self.path_cover_large,
            &self.path_cover_l,
            &self.path_cover_small,
            &self.path_cover_s,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .find(|c| !c.trim().is_empty())?;

        let host = host.trim_end_matches('/');
        Some(if candidate.starts_with("http://") || candidate.starts_with("https://") {
            candidate.to_string()
        } else if candidate.starts_with('/') {
            format!("{host}{candidate}")
        } else {
            format!("{host}/{candidate}")
        })
    }
}

/// One file of a rom (`GET /api/roms/{id}`'s `files`) — the unit a download actually fetches.
///
/// A plain rom has one; a multi-disc game has one per disc; a folder-structured rom (RomM calls it
/// Structure A — a PS3 title, say) has a whole tree of them.
#[derive(Debug, Clone, Deserialize)]
pub struct RomFile {
    pub id: i64,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size_bytes: i64,
    /// The directory holding this file, **from RomM's library root** — `roms/psx` for a rom that
    /// sits directly in the platform folder, `roms/psx/Driver 2 (USA)` for one with its own
    /// directory. It is *not* a subpath within the rom's own folder; see
    /// [`crate::downloads::layout`] for how the two are told apart, and what reading it the other
    /// way cost.
    #[serde(default)]
    pub file_path: String,
    /// RomM's `RomFileCategory` — "game", "update", "dlc", "patch", "manual", … — or None on a
    /// server too old to categorise files, which is treated the same as "game".
    #[serde(default)]
    pub category: Option<String>,
}

impl RomFile {
    /// Whether this file is the game itself rather than something shipped alongside it. An
    /// uncategorised file counts: only a server new enough to label them says anything here, and
    /// on an older one every file is the game.
    pub fn is_game(&self) -> bool {
        self.category.as_deref().is_none_or(|c| c == "game")
    }

    pub fn size(&self) -> u64 {
        self.file_size_bytes.max(0) as u64
    }
}

/// `GET /api/roms/{id}` — the one per-game fetch this build makes, and only to download it.
///
/// Browsing deliberately needs none of this: the list response carries everything the grid and the
/// detail page draw. What is only here is `files`, which is what a download has to be told.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RomDetail {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    /// The rom's own entry inside its platform directory: a file name for a plain rom, a folder
    /// name for one that has its own directory.
    #[serde(default)]
    pub fs_name: Option<String>,
    /// The **platform's** directory on RomM's filesystem, from the library root (`roms/psx`) — the
    /// same for every rom on that platform, and never blank.
    #[serde(default)]
    pub fs_path: Option<String>,
    #[serde(default)]
    pub platform_id: Option<i64>,
    #[serde(default)]
    pub platform_slug: Option<String>,
    #[serde(default)]
    pub files: Vec<RomFile>,
    /// See [`Rom::fs_size_bytes`] and [`Rom::md5_hash`]: what a download records as the version it
    /// took.
    #[serde(default)]
    pub fs_size_bytes: Option<i64>,
    #[serde(default)]
    pub md5_hash: Option<String>,
    /// The other builds of this game RomM knows — regions, revisions, hacks.
    #[serde(default)]
    pub sibling_roms: Vec<SiblingRom>,
    /// RetroAchievements' list, whole — the detail page's Achievements section.
    #[serde(default)]
    pub merged_ra_metadata: Option<RaMetadata>,
    /// See [`Rom::file_stem`] and [`Rom::created_at`]: what a download records as the rom it took.
    #[serde(default)]
    pub fs_name_no_ext: Option<String>,
    #[serde(default)]
    pub fs_extension: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl RomDetail {
    /// Which rom this is — see [`Stamp`].
    pub fn stamp(&self) -> Option<Stamp> {
        let stem = file_stem(self.fs_name_no_ext.as_deref(), self.fs_name.as_deref(), self.fs_extension.as_deref())?;
        Some(Stamp { stem, born: self.created_at.clone() })
    }
}

/// A rom's file name without its extension: RomM's own `fs_name_no_ext`, or `fs_name` with
/// `fs_extension` taken off for a server that does not send it. A rom kept as a folder has no
/// extension, and its name is already the stem.
fn file_stem(no_ext: Option<&str>, name: Option<&str>, extension: Option<&str>) -> Option<String> {
    if let Some(stem) = no_ext.filter(|s| !s.trim().is_empty()) {
        return Some(stem.to_string());
    }
    let name = name.filter(|s| !s.trim().is_empty())?;
    let stem = match extension.filter(|e| !e.is_empty()) {
        Some(ext) => name.strip_suffix(ext).and_then(|s| s.strip_suffix('.')).unwrap_or(name),
        None => name,
    };
    Some(stem.to_string())
}

/// A BIOS or firmware file RomM holds for a platform (`GET /api/firmware?platform_id=`).
///
/// `file_name` is already the name the emulators look for — `scph5501.bin`, `dc_boot.bin` — because
/// RomM validates uploads against the known-BIOS tables, so a file lands in the BIOS folder as it is.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Firmware {
    pub id: i64,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size_bytes: i64,
}

/// A RomM collection. Home appends one shelf per manual collection, in server order.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Collection {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub rom_count: i64,
    #[serde(default)]
    pub rom_ids: Vec<i64>,
    #[serde(default)]
    pub is_favorite: bool,
    #[serde(default)]
    pub is_virtual: bool,
    #[serde(default)]
    pub is_smart: bool,
}

impl Collection {
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("Collection #{}", self.id))
    }
}

/// One of the collections RomM builds itself from its metadata (`GET /api/collections/virtual`): a
/// genre, a franchise, a series, a company, a game mode. Its id is a string, its own namespace.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VirtualCollection {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// `genre`, `franchise`, `collection` (IGDB's series), `company`, `mode`, …
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub rom_ids: Vec<i64>,
}

/// One save in RomM — a row of `GET /api/saves`, and what an upload answers with.
///
/// `file_name` is what RomM stored, which for an upload into a history slot is the uploaded name
/// with RomM's own timestamp tag added — see [`crate::sync`] for why that tag is stripped before a
/// save is compared with anything on disk.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveAsset {
    pub id: i64,
    #[serde(default)]
    pub rom_id: Option<i64>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub emulator: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Where the bytes are, host-relative. Present on every RomM that has saves at all, unlike the
    /// newer `/api/saves/{id}/content`.
    #[serde(default)]
    pub download_path: Option<String>,
    /// A state's thumbnail, when one was uploaded with it.
    #[serde(default)]
    pub screenshot: Option<Screenshot>,
    /// The libretro core that wrote a state, which RomM 5.4 records for the states its own game
    /// streams capture and for no others.
    #[serde(default)]
    pub core: Option<String>,
    #[serde(default)]
    pub file_size_bytes: Option<i64>,
    /// A save's slot on RomM — `autosave` for the history this app and the Kotlin app write. States
    /// carry none; their slot is in their name.
    #[serde(default)]
    pub slot: Option<String>,
    /// What RomM hashed the upload to — see [`crate::sync::hash`]. RomM 5 and up.
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Which devices hold this save, and whether what they hold is still its current version.
    /// Listed when the request names a device (`?device_id=`), the asking device first.
    #[serde(default)]
    pub device_syncs: Vec<DeviceSync>,
}

/// One device's record of one save, as RomM keeps it.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceSync {
    pub device_id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub last_synced_at: Option<String>,
    /// The player told RomM to stop syncing this save on that device.
    #[serde(default)]
    pub is_untracked: bool,
    /// That device holds the save's current version.
    #[serde(default)]
    pub is_current: bool,
}

/// One save as a device holds it, as it is put to RomM's sync negotiation (`ClientSaveState`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClientSave {
    pub rom_id: i64,
    pub file_name: String,
    /// What pairs it with RomM's copy: the same slot on every device that syncs the game.
    pub slot: Option<String>,
    pub emulator: Option<String>,
    pub content_hash: Option<String>,
    pub updated_at: String,
    pub file_size_bytes: u64,
}

/// What RomM's negotiation answers: a session, and one operation per save either side has.
#[derive(Debug, Clone, Deserialize)]
pub struct Negotiation {
    pub session_id: i64,
    #[serde(default)]
    pub operations: Vec<SyncOperation>,
}

/// One thing RomM's negotiation says to do about one save (`SyncOperationSchema`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SyncOperation {
    /// `upload`, `download`, `conflict` or `no_op` — and `delete` on servers newer than 5.3.
    pub action: String,
    pub rom_id: i64,
    /// RomM's current save in the slot, when it has one.
    #[serde(default)]
    pub save_id: Option<i64>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default)]
    pub emulator: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub server_updated_at: Option<String>,
    #[serde(default)]
    pub server_content_hash: Option<String>,
}

/// One play session, as RomM ingests it — on its own (`/api/play-sessions`) or with the sync
/// session that follows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlaySession {
    pub rom_id: Option<i64>,
    pub save_slot: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub duration_ms: u64,
}

/// The thumbnail RomM keeps beside a save state.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Screenshot {
    #[serde(default)]
    pub download_path: Option<String>,
}

/// One page of `GET /api/roms`.
#[derive(Debug, Clone, Default)]
pub struct RomPage {
    pub items: Vec<Rom>,
    pub total: usize,
}

/// One install RomM's web app asked of this device (`/api/devices/{id}/installs`).
#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    pub id: String,
    pub rom_id: i64,
}

//! Thin wrapper around the RomM HTTP API — a port of `net/RommApi.kt` + `net/RommRepository.kt`,
//! trimmed to pairing, the reads the library needs, and the save endpoints sync uses.
//!
//! Auth flow (see <https://docs.romm.app/latest/developers/api-authentication/>):
//!  1. User creates a Client API Token in the RomM web UI and presses "Pair Device", which yields
//!     an 8-character code valid for 5 minutes.
//!  2. The device POSTs that code to `/api/client-tokens/exchange` and receives a long-lived token
//!     of the form `rmm_<64 hex chars>`.
//!  3. Every subsequent request sends `Authorization: Bearer rmm_...`.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;

use super::models::{
    ClientSave, Collection, Firmware, InstallRequest, Negotiation, PairedToken, Platform, PlaySession, Rom, RomDetail,
    RomFile, RomPage, SaveAsset, UserInfo, VirtualCollection,
};
use crate::config::Connection;
use crate::library::Stamp;

/// How many roms one `/api/roms` page asks for. RomM's own web UI pages at 72; a self-hosted server
/// serves ~4 s per 250-item page, so this trades a bigger response for far fewer round trips.
pub const PAGE: usize = 250;

/// How long RomM's web app shows a device as playing after its last heartbeat.
pub const HEARTBEAT_LASTS: Duration = Duration::from_secs(90);

/// Concurrent per-platform fetches. Enough to keep a self-hosted server busy without burying it —
/// the Kotlin loader uses the same gate.
pub const FETCH_PARALLELISM: usize = 4;

/// The parameters of one `GET /api/roms` page.
///
/// A struct rather than seven positional arguments because the two calls this app makes differ in
/// ways that are invisible at a call site otherwise — one is grouped and ordered by name, the other
/// ungrouped and ordered by play time, and swapping those silently returns a plausible wrong answer
/// rather than an error.
#[derive(Debug, Clone)]
pub struct RomQuery {
    pub platform_id: Option<i64>,
    pub offset: usize,
    pub limit: usize,
    pub order_by: &'static str,
    pub order_dir: &'static str,
    /// Collapse alternate builds of the same game (patched / revision / region siblings) into one
    /// entry; the chosen one then carries `sibling_roms`.
    pub group_by_meta: bool,
    /// Only the roms this user has played (`last_played=true`).
    pub played_only: bool,
}

impl RomQuery {
    /// One page of a platform's library, as the grid shows it: grouped, alphabetical.
    pub fn platform_page(platform_id: i64, offset: usize) -> Self {
        Self {
            platform_id: Some(platform_id),
            offset,
            limit: PAGE,
            order_by: "name",
            order_dir: "asc",
            group_by_meta: true,
            played_only: false,
        }
    }

    /// One page of the roms the user has ever played, newest first.
    ///
    /// Ungrouped on purpose: grouped, a game's tile is one representative rom and its siblings'
    /// play history is invisible, so a regional build you actually played never surfaces as recent.
    /// Filtered, not just ordered: ordered alone, RomM answers with the whole library, the
    /// unplayed roms after the played ones.
    pub fn played(offset: usize) -> Self {
        Self {
            platform_id: None,
            offset,
            limit: PAGE,
            order_by: "last_played",
            order_dir: "desc",
            group_by_meta: false,
            played_only: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status, or `-1` when the request never reached a server (DNS, TLS, refused).
    pub status: i32,
    pub message: String,
    /// What the server said about it, when it said something — FastAPI's `detail`. The status alone
    /// cannot tell a route this RomM does not have from a device it has forgotten: both are 404.
    pub detail: Option<String>,
}

impl ApiError {
    fn new(status: i32, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), detail: None }
    }

    /// A transport failure, which is what makes a host candidate worth retrying under another
    /// scheme — an HTTP *status* means we reached the right server and it said no.
    pub fn is_transport(&self) -> bool {
        self.status == -1
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

type Result<T> = std::result::Result<T, ApiError>;

/// RomM's `path` on `conn`'s server, as a URL a query or path segments can be added to.
fn endpoint(conn: &Connection, path: &str) -> Result<reqwest::Url> {
    reqwest::Url::parse(&format!("{}{path}", conn.host.trim_end_matches('/'))).map_err(|_| bad_address())
}

/// What a request says when the paired address cannot be made into a URL.
fn bad_address() -> ApiError {
    ApiError::new(-1, "The server address is not a valid URL.")
}

/// Map a status onto the message the user actually sees. Ported verbatim from `friendlyError`.
fn friendly_error(status: u16) -> String {
    match status {
        400 | 404 | 422 => "Invalid or expired pairing code. Generate a new one in RomM.".into(),
        401 | 403 => "Authentication failed. The token may have been revoked.".into(),
        500..=599 => format!("The RomM server returned an error ({status})."),
        other => format!("Request failed ({other})."),
    }
}

/// What a device approved by QR may do: read the library and its art, its BIOS and the user's
/// own record of each game, keep saves, states, devices and play sessions, and change the user's
/// own collections, which is where favourites live — everything this app does, and nothing that
/// edits the library itself.
///
/// RomM grants what was asked for when the device was approved and nothing added since, so a
/// scope added here reaches a device paired before only when it is paired again.
pub const DEVICE_SCOPES: [&str; 12] = [
    "me.read",
    "roms.read",
    "platforms.read",
    "collections.read",
    "collections.write",
    "firmware.read",
    "assets.read",
    "assets.write",
    "roms.user.read",
    "roms.user.write",
    "devices.read",
    "devices.write",
];

#[derive(Clone)]
pub struct RommClient {
    /// Verifies certificates — the normal path.
    secure: reqwest::Client,
    /// Skips verification, for the self-signed certificate a LAN RomM often carries. Only ever used
    /// when the stored connection recorded `insecure`, never chosen silently.
    insecure: reqwest::Client,
}

impl RommClient {
    pub fn new() -> reqwest::Result<Self> {
        let base = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                // Per-read rather than per-request. A whole-request ceiling is the wrong shape for
                // the one request here measured in gigabytes: a 60-second cap would abort every rom
                // larger than the link can carry in a minute. This fails a connection that has gone
                // quiet for a minute instead, which is what a stall actually looks like — and it
                // suits a 250-item library page for the same reason.
                .read_timeout(Duration::from_secs(60))
                .user_agent(concat!("Deckhand/", env!("CARGO_PKG_VERSION"), " (rust)"))
        };
        Ok(Self {
            secure: base().build()?,
            insecure: base().danger_accept_invalid_certs(true).build()?,
        })
    }

    fn http(&self, insecure: bool) -> &reqwest::Client {
        if insecure { &self.insecure } else { &self.secure }
    }

    /// Run a request and return its body, turning both transport failures and non-2xx statuses into
    /// [`ApiError`] so callers only ever handle one error type.
    async fn execute(&self, request: reqwest::RequestBuilder) -> Result<String> {
        let response = request.send().await.map_err(|e| {
            let kind = if e.is_timeout() {
                "timed out"
            } else if e.is_connect() {
                "could not connect"
            } else {
                "request failed"
            };
            ApiError::new(-1, format!("Could not reach the server ({kind})."))
        })?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let mut error = ApiError::new(status.as_u16() as i32, friendly_error(status.as_u16()));
            error.detail = serde_json::from_str::<Value>(&body).ok().and_then(|v| match &v["detail"] {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("message").and_then(Value::as_str).map(str::to_string),
                _ => None,
            });
            return Err(error);
        }
        Ok(body)
    }

    /// A request to RomM with this connection's token, asking for JSON — where every call made as
    /// the paired device starts. `url` is a whole URL; [`RommClient::get`] and [`RommClient::post`]
    /// take a path.
    fn api(&self, conn: &Connection, method: reqwest::Method, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        self.http(conn.insecure).request(method, url).bearer_auth(&conn.token).header("Accept", "application/json")
    }

    fn get(&self, conn: &Connection, path: &str) -> reqwest::RequestBuilder {
        self.api(conn, reqwest::Method::GET, format!("{}{path}", conn.host.trim_end_matches('/')))
    }

    fn post(&self, conn: &Connection, path: &str) -> reqwest::RequestBuilder {
        self.api(conn, reqwest::Method::POST, format!("{}{path}", conn.host.trim_end_matches('/')))
    }

    /// [`RommClient::execute`], and the answer read as `T`.
    async fn execute_json<T: serde::de::DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let body = self.execute(request).await?;
        serde_json::from_str(&body).map_err(|e| ApiError::new(200, format!("The server sent something unexpected: {e}")))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, conn: &Connection, path: &str) -> Result<T> {
        self.execute_json(self.get(conn, path)).await
    }

    /// Exchange an 8-character pairing code for a long-lived client API token.
    ///
    /// Tolerant token extraction, like the Kotlin original: RomM's `/exchange` returns
    /// `ClientTokenCreateSchema` whose secret lives in `raw_token`, but older and other shapes use
    /// `token`/`access_token`. As a last resort the body is scanned for an `rmm_...` literal.
    /// Ask RomM to approve this device (`POST /api/auth/device/init`): it answers with a code for a
    /// phone to approve, and one to poll with. Open — the device has no credentials yet.
    pub async fn device_auth_init(&self, host: &str, insecure: bool, name: &str, identifier: &str, platform: &str) -> Result<crate::romm::models::DeviceAuthStart> {
        let body = self
            .execute(
                self.http(insecure)
                    .post(format!("{}/api/auth/device/init", host.trim_end_matches('/')))
                    .json(&serde_json::json!({
                        "client_device_identifier": identifier,
                        "name": name,
                        "client": "deckhand",
                        "platform": platform,
                        "client_version": env!("CARGO_PKG_VERSION"),
                        "requested_scopes": DEVICE_SCOPES,
                    })),
            )
            .await?;
        serde_json::from_str(&body).map_err(|_| ApiError::new(200, "RomM answered the request to pair in a way this does not read."))
    }

    /// Ask whether the device has been approved yet (`POST /api/auth/device/token`). Until it is,
    /// RomM answers 400 with `authorization_pending` — or `slow_down`, `access_denied`,
    /// `expired_token` — in the error's detail.
    pub async fn device_auth_token(&self, host: &str, insecure: bool, device_code: &str) -> Result<crate::romm::models::DeviceAuthToken> {
        let body = self
            .execute(
                self.http(insecure)
                    .post(format!("{}/api/auth/device/token", host.trim_end_matches('/')))
                    .json(&serde_json::json!({ "device_code": device_code })),
            )
            .await?;
        serde_json::from_str(&body).map_err(|_| ApiError::new(200, "RomM approved the device but sent no token this reads."))
    }

    pub async fn exchange_pairing_code(&self, host: &str, code: &str, insecure: bool) -> Result<PairedToken> {
        let body = self
            .execute(
                self.http(insecure)
                    .post(format!("{}/api/client-tokens/exchange", host.trim_end_matches('/')))
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({ "code": code })),
            )
            .await?;

        let parsed: Option<Value> = serde_json::from_str(&body).ok();
        let raw = parsed
            .as_ref()
            .and_then(|v| {
                ["raw_token", "token", "access_token", "client_token", "key", "value"]
                    .into_iter()
                    .find_map(|k| v.get(k).and_then(Value::as_str))
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| scan_for_token(&body))
            .ok_or_else(|| ApiError::new(200, "Server accepted the code but returned no token."))?;

        Ok(PairedToken {
            raw_token: Some(raw),
            id: parsed.as_ref().and_then(|v| v.get("id")).and_then(Value::as_i64),
            name: parsed
                .as_ref()
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    /// `GET /api/users/me`. Called right after an exchange to prove the token works before it is
    /// written to disk — a token that pairs but cannot read is worse than no token, because the app
    /// would then open on an empty library rather than on the login screen.
    pub async fn fetch_current_user(&self, conn: &Connection) -> Result<UserInfo> {
        self.get_json(conn, "/api/users/me").await
    }

    pub async fn fetch_platforms(&self, conn: &Connection) -> Result<Vec<Platform>> {
        let value: Value = self
            .get_json(conn, "/api/platforms")
            .await
            .map_err(|e| ApiError::new(e.status, format!("Could not read your platforms: {e}")))?;
        Ok(decode_list(&value))
    }

    pub async fn fetch_collections(&self, conn: &Connection) -> Result<Vec<Collection>> {
        let value: Value = self.get_json(conn, "/api/collections").await?;
        Ok(decode_list(&value))
    }

    /// `POST /api/collections/{id}/roms` — put roms in a collection. RomM answers with the collection
    /// as it now is.
    pub async fn add_to_collection(&self, conn: &Connection, collection: i64, rom_ids: &[i64]) -> Result<Collection> {
        let request = self
            .post(conn, &format!("/api/collections/{collection}/roms"))
            .json(&serde_json::json!({ "rom_ids": rom_ids }));
        self.execute_json(request).await
    }

    /// `DELETE /api/collections/{id}/roms` — take roms out of a collection, answered the same way.
    pub async fn remove_from_collection(&self, conn: &Connection, collection: i64, rom_ids: &[i64]) -> Result<Collection> {
        let url = format!("{}/api/collections/{collection}/roms", conn.host.trim_end_matches('/'));
        let request = self.api(conn, reqwest::Method::DELETE, url).json(&serde_json::json!({ "rom_ids": rom_ids }));
        self.execute_json(request).await
    }

    /// `POST /api/collections?is_favorite=true` — the account's favourites collection, for an account
    /// that has none yet: what RomM's own web app makes the first time a heart is pressed there.
    pub async fn create_favourites(&self, conn: &Connection) -> Result<Collection> {
        let form = reqwest::multipart::Form::new().text("name", "Favourites").text("description", "");
        let request = self.post(conn, "/api/collections?is_favorite=true&is_public=false").multipart(form);
        self.execute_json(request).await
    }

    /// `GET /api/collections/smart` — the account's saved-filter collections. Their ids are a
    /// sequence of their own, not the manual collections'.
    pub async fn fetch_smart_collections(&self, conn: &Connection) -> Result<Vec<Collection>> {
        let value: Value = self.get_json(conn, "/api/collections/smart").await?;
        Ok(decode_list(&value))
    }

    /// `GET /api/collections/virtual?type=all` — every collection RomM generates from its metadata.
    /// No `limit`: the server caps the list at whatever is passed.
    pub async fn fetch_virtual_collections(&self, conn: &Connection) -> Result<Vec<VirtualCollection>> {
        let value: Value = self.get_json(conn, "/api/collections/virtual?type=all").await?;
        Ok(decode_list(&value))
    }

    /// One page of `GET /api/roms`.
    pub async fn fetch_roms(&self, conn: &Connection, query: &RomQuery) -> Result<RomPage> {
        let RomQuery { platform_id, offset, limit, order_by, order_dir, group_by_meta, played_only } = query;
        let mut path = format!(
            "/api/roms?limit={limit}&offset={offset}&order_by={order_by}&order_dir={order_dir}\
             &group_by_meta_id={group_by_meta}"
        );
        // RomM expects the (repeatable) `platform_ids` list param, not `platform_id`.
        if let Some(id) = platform_id {
            path.push_str(&format!("&platform_ids={id}"));
        }
        if *played_only {
            path.push_str("&last_played=true");
        }

        let value: Value = self.get_json(conn, &path).await?;
        let items: Vec<Rom> = decode_list(&value);
        let total = value
            .get("total")
            .and_then(Value::as_u64)
            .map(|t| t as usize)
            .unwrap_or(items.len());
        Ok(RomPage { items, total })
    }

    /// Every rom this user has played, newest first, across as many pages as that takes.
    ///
    /// A RomM too old for `last_played=true` answers with the whole library, the played roms first:
    /// the first unplayed one ends the reading there rather than paging through the rest.
    pub async fn fetch_played(&self, conn: &Connection) -> Result<Vec<Rom>> {
        let mut played = Vec::new();
        let mut offset = 0;
        loop {
            let page = self.fetch_roms(conn, &RomQuery::played(offset)).await?;
            let fetched = page.items.len();
            offset += fetched;
            let before = played.len();
            played.extend(page.items.into_iter().filter(|r| r.rom_user.as_ref().is_some_and(|u| u.last_played.is_some())));
            if played.len() - before < fetched || fetched < PAGE || offset >= page.total {
                return Ok(played);
            }
        }
    }

    /// The platforms, by id, with a rom RomM changed after `since` (`GET /api/roms?updated_after=`):
    /// a rescrape, a rename, a file swapped one-for-one — what leaves a platform's `rom_count`
    /// where it was.
    ///
    /// Newest first, so a RomM too old for the filter, which answers with the whole library, is
    /// read only as far as its first rom from before `since`.
    pub async fn changed_platforms(&self, conn: &Connection, since: &str) -> Result<HashSet<i64>> {
        // To the second, in UTC: RomM's `+00:00` and this app's `Z` compare as text from there.
        let after = |rom: &Rom| rom.updated_at.as_deref().is_some_and(|at| at.get(..19) > since.get(..19));
        let mut platforms = HashSet::new();
        let mut offset = 0;
        loop {
            let path = format!(
                "/api/roms?updated_after={}&limit={PAGE}&offset={offset}&order_by=updated_at&order_dir=desc\
                 &group_by_meta_id=false&with_char_index=false&with_filter_values=false&with_rom_id_index=false",
                urlencode(since)
            );
            let value: Value = self.get_json(conn, &path).await?;
            let items: Vec<Rom> = decode_list(&value);
            let total = value.get("total").and_then(Value::as_u64).map_or(0, |t| t as usize);
            offset += items.len();
            let changed: Vec<&Rom> = items.iter().take_while(|r| after(r)).collect();
            platforms.extend(changed.iter().filter_map(|r| r.platform_id));
            if changed.len() < items.len() || items.len() < PAGE || offset >= total {
                return Ok(platforms);
            }
        }
    }

    /// `GET /api/roms/{id}` — the one per-game fetch this build makes, and only to download it.
    ///
    /// Browsing never needs it: the list response already carries the summary, the metadata, the
    /// size and the screenshot paths. What is only here is `files`, which is the list a download
    /// has to be given — a multi-disc game's discs, a folder-structured rom's whole tree.
    pub async fn fetch_rom_detail(&self, conn: &Connection, rom_id: i64) -> Result<RomDetail> {
        self.get_json(conn, &format!("/api/roms/{rom_id}")).await
    }

    /// Which rom RomM holds under `rom_id` now (`GET /api/roms/{id}/simple`, RomM 5's light form of
    /// one rom) — what a sync asks before it moves a game's saves under an id it kept.
    pub async fn fetch_rom_stamp(&self, conn: &Connection, rom_id: i64) -> Result<Option<Stamp>> {
        match self.get_json::<Rom>(conn, &format!("/api/roms/{rom_id}/simple")).await {
            Ok(rom) => Ok(rom.stamp()),
            // A RomM from before the light form: the whole rom says the same.
            Err(e) if e.status == 404 => Ok(self.fetch_rom_detail(conn, rom_id).await?.stamp()),
            Err(e) => Err(e),
        }
    }

    /// Stream one of a rom's files to `dest`, reporting bytes written as they land.
    ///
    /// Written to `<dest>.part` and renamed on success, so an interrupted download never leaves a
    /// truncated file that looks like a finished one — the installed record is written from the
    /// rename, and a half rom under the real name would be indistinguishable from a whole one.
    ///
    /// `file_id` is not optional, and that is the point: without `file_ids` RomM zips **every** file
    /// of a multi-file rom and serves that, so asking for one disc by name alone would download the
    /// whole set under that disc's name. The name still goes in the path — it is what RomM knows
    /// this file as — while the id is what selects it.
    ///
    /// `progress` is called per chunk with the running total for *this* file; throttling is the
    /// caller's, since only it knows what the figure is being drawn into.
    pub async fn download_file(
        &self,
        conn: &Connection,
        rom_id: i64,
        file: &RomFile,
        dest: &Path,
        mut progress: impl FnMut(u64),
    ) -> Result<u64> {
        let mut url = endpoint(conn, &format!("/api/roms/{rom_id}/content"))?;
        url.path_segments_mut()
            .map_err(|_| bad_address())?
            .push(&file.file_name);
        url.query_pairs_mut().append_pair("file_ids", &file.id.to_string());
        self.fetch_to_file(conn, url, dest, Some(file.size()).filter(|s| *s > 0), &mut progress).await
    }

    /// `POST /api/play-sessions` — one session, so RomM bumps the rom's `last_played` and counts its
    /// playtime, attributed to this device when it is one. Times are RFC 3339.
    pub async fn post_play_session(&self, conn: &Connection, session: &PlaySession) -> Result<()> {
        let body = serde_json::json!({ "device_id": conn.device_id, "sessions": [session] });
        let request = self
            .post(conn, "/api/play-sessions")
            .json(&body);
        self.execute(request).await.map(|_| ())
    }

    /// `POST /api/activity/heartbeat` — this device is playing `rom_id` now. RomM's web app shows
    /// it as playing for [`HEARTBEAT_LASTS`] after each.
    pub async fn heartbeat(&self, conn: &Connection, device_id: &str, rom_id: i64) -> Result<()> {
        let request = self
            .post(conn, "/api/activity/heartbeat")
            .json(&serde_json::json!({ "rom_id": rom_id, "device_id": device_id }));
        self.execute(request).await.map(|_| ())
    }

    /// `DELETE /api/activity/heartbeat` — the game has closed, so RomM stops showing it at once
    /// rather than when the last heartbeat runs out.
    pub async fn clear_heartbeat(&self, conn: &Connection, device_id: &str) -> Result<()> {
        let url = format!("{}/api/activity/heartbeat?device_id={}", conn.host.trim_end_matches('/'), urlencode(device_id));
        self.execute(self.api(conn, reqwest::Method::DELETE, url)).await.map(|_| ())
    }

    /// `POST /api/devices` — register this install, answering the id RomM gave it. `allow_existing`
    /// hands back the device RomM already has under this name rather than minting a duplicate.
    pub async fn register_device(&self, conn: &Connection, name: &str, platform: &str, hostname: &str) -> Result<String> {
        let body = serde_json::json!({
            "name": name,
            "platform": platform,
            "client": "Deckhand",
            "client_version": env!("CARGO_PKG_VERSION"),
            "hostname": hostname,
            // Deckhand moves saves over the HTTP API, which is what RomM calls "api".
            "sync_mode": "api",
            "allow_existing": true,
        });
        let request = self
            .post(conn, "/api/devices")
            .json(&body);
        let response: Value = self.execute_json(request).await?;
        response["device_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::new(200, "RomM registered the device but sent no id."))
    }

    /// `GET /api/devices` — the ids of the devices RomM knows for this account.
    pub async fn device_ids(&self, conn: &Connection) -> Result<Vec<String>> {
        let value: Value = self.get_json(conn, "/api/devices").await?;
        Ok(value.as_array().into_iter().flatten().filter_map(|d| d["id"].as_str().map(str::to_string)).collect())
    }

    /// `PUT /api/devices/{id}` — say this device takes installs from RomM's web app, which then
    /// offers it under *Install on device*. Only what is named changes.
    pub async fn accept_installs(&self, conn: &Connection, device_id: &str) -> Result<()> {
        let url = format!("{}/api/devices/{}", conn.host.trim_end_matches('/'), urlencode(device_id));
        let request = self
            .api(conn, reqwest::Method::PUT, url)
            .json(&serde_json::json!({ "capabilities": { "remote_install": true } }));
        self.execute(request).await.map(|_| ())
    }

    /// `POST /api/devices/{id}/installs/claim` — take the installs RomM's web app asked of this
    /// device, answering every one it holds taken and has not answered, oldest first. Claiming is
    /// also what shows the device as online there, for 90 seconds.
    ///
    /// Only a token RomM tied to this device may claim: one approved by QR. A token paired by code
    /// is answered 403, and a RomM with installs turned off 404.
    pub async fn claim_installs(&self, conn: &Connection, device_id: &str) -> Result<Vec<InstallRequest>> {
        let request = self.post(conn, &format!("/api/devices/{}/installs/claim", urlencode(device_id)));
        let value: Value = self.execute_json(request).await?;
        Ok(decode_list(&value))
    }

    /// `PUT /api/devices/{id}/installs/{request}` — how a taken install ended: `done`,
    /// `already_installed` or `failed`, with why when it failed.
    pub async fn report_install(&self, conn: &Connection, device_id: &str, request: &str, outcome: &Outcome) -> Result<()> {
        let url = format!("{}/api/devices/{}/installs/{}", conn.host.trim_end_matches('/'), urlencode(device_id), urlencode(request));
        let body = match outcome {
            Outcome::Done => serde_json::json!({ "status": "done" }),
            Outcome::AlreadyInstalled => serde_json::json!({ "status": "already_installed" }),
            // RomM keeps 500 characters of a reason.
            Outcome::Failed(why) => serde_json::json!({ "status": "failed", "reason": why.chars().take(500).collect::<String>() }),
        };
        self.execute(self.api(conn, reqwest::Method::PUT, url).json(&body)).await.map(|_| ())
    }

    /// `POST /api/saves/{id}/downloaded` — tell RomM this device now holds that save. RomM refuses a
    /// push to a slot until its newest save has been acknowledged by the pushing device.
    ///
    /// `content_hash` is what the device holds now, which RomM after 5.3 keeps as the device's
    /// baseline for the save; 5.3 itself ignores it.
    pub async fn confirm_downloaded(&self, conn: &Connection, save_id: i64, device_id: &str, content_hash: Option<&str>) -> Result<()> {
        let request = self
            .post(conn, &format!("/api/saves/{save_id}/downloaded"))
            .json(&serde_json::json!({ "device_id": device_id, "content_hash": content_hash }));
        self.execute(request).await.map(|_| ())
    }

    /// `GET /api/firmware?platform_id=` — the BIOS and firmware RomM holds for one platform.
    pub async fn fetch_firmware(&self, conn: &Connection, platform_id: i64) -> Result<Vec<Firmware>> {
        let value: Value = self.get_json(conn, &format!("/api/firmware?platform_id={platform_id}")).await?;
        Ok(decode_list(&value))
    }

    /// Stream one firmware file to `dest`, the same `.part`-then-rename way a rom file comes down.
    pub async fn download_firmware(&self, conn: &Connection, firmware: &Firmware, dest: &Path) -> Result<u64> {
        let mut url = endpoint(conn, &format!("/api/firmware/{}/content", firmware.id))?;
        url.path_segments_mut()
            .map_err(|_| bad_address())?
            .push(&firmware.file_name);
        self.fetch_to_file(conn, url, dest, None, &mut |_| {}).await
    }

    /// GET `url` with the token and write the body to `dest` through `<dest>.part`.
    ///
    /// A connection that drops part way leaves the `.part` behind, and the next attempt asks only
    /// for the rest of it — a 4 GB disc that stalled at 90% does not start over. Only a `.part` that
    /// can be this file's is carried on from: written within the day, short of `size` when that is
    /// known, and answered with the rest from exactly where it ends. Anything else starts afresh.
    /// Every other way out — a disk error, a cancelled download — removes it.
    async fn fetch_to_file(
        &self,
        conn: &Connection,
        url: reqwest::Url,
        dest: &Path,
        size: Option<u64>,
        progress: &mut impl FnMut(u64),
    ) -> Result<u64> {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::new(-1, format!("Could not create {}: {e}", parent.display())))?;
        }
        let part = part_path(dest);
        let have = resumable(&part, size).await;
        let mut request = self.http(conn.insecure).get(url).bearer_auth(&conn.token);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = request.send().await.map_err(|_| ApiError::new(-1, "Could not reach the server."))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::new(status.as_u16() as i32, friendly_error(status.as_u16())));
        }
        // The rest, from where the `.part` ends — or, from a server that sent the whole file (or
        // the rest of something else), the whole file.
        let from = if status == reqwest::StatusCode::PARTIAL_CONTENT && range_start(&response) == Some(have) { have } else { 0 };

        // Removes the `.part` however this ends, unless told to keep it — which includes this
        // future being dropped, the way a cancelled download stops.
        let mut guard = PartGuard(Some(part.clone()));
        match stream_to_file(response, &part, from, progress).await {
            Ok(written) => {
                tokio::fs::rename(&part, dest)
                    .await
                    .map_err(|e| ApiError::new(-1, format!("Could not write {}: {e}", dest.display())))?;
                guard.0 = None;
                Ok(written)
            }
            Err(Stream::Interrupted(e)) => {
                guard.0 = None;
                Err(e)
            }
            Err(Stream::Disk(e)) => Err(e),
        }
    }

    /// Fetch one asset (a cover, a logo, the hero's banner) as raw bytes.
    ///
    /// The credential travels only to RomM itself. Off-host assets are still fetched, just
    /// anonymously: refusing them would break art for no gain, since content a tampered response
    /// points at could equally have been tampered with in place. It is only the credential that
    /// must not travel.
    pub async fn fetch_asset(&self, conn: &Connection, url: &str) -> Result<Vec<u8>> {
        let mut request = self.http(conn.insecure).get(url);
        if same_host(&conn.host, url) {
            request = request.bearer_auth(&conn.token);
        }
        let response = request
            .send()
            .await
            .map_err(|_| ApiError::new(-1, "Could not reach the server."))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::new(status.as_u16() as i32, friendly_error(status.as_u16())));
        }
        Ok(response.bytes().await.map_err(|_| ApiError::new(-1, "Asset download failed."))?.to_vec())
    }

    /// The account's saves (`GET /api/saves`): one game's, or every game's in one request.
    ///
    /// A sweep asks for every game's at once rather than one call per game: the records carry
    /// `rom_id`, so they are grouped locally, and a per-game `GET /api/roms/{id}` would fetch a
    /// document hundreds of times larger to read the same few lines out of it.
    pub async fn fetch_saves(&self, conn: &Connection, rom_id: Option<i64>) -> Result<Vec<SaveAsset>> {
        let path = match rom_id {
            Some(id) => format!("/api/saves?rom_id={id}"),
            None => "/api/saves".to_string(),
        };
        let value: Value = self.get_json(conn, &path).await?;
        Ok(decode_list(&value))
    }

    /// One save's bytes.
    ///
    /// `download_path` first, because every RomM that has saves serves it; the `/api/saves/{id}/content`
    /// endpoint is RomM 5's and stands in only when a record carries no path. The path is the
    /// server's choice, so it goes through [`RommClient::fetch_asset`] and its same-host rule like
    /// any other.
    pub async fn download_save(&self, conn: &Connection, save: &SaveAsset) -> Result<Vec<u8>> {
        let host = conn.host.trim_end_matches('/');
        let url = match save.download_path.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(path) if path.starts_with("http://") || path.starts_with("https://") => path.to_string(),
            Some(path) => format!("{host}/{}", path.trim_start_matches('/')),
            None => format!("{host}/api/saves/{}/content", save.id),
        };
        self.fetch_asset(conn, &url).await
    }

    /// Upload a battery save into the game's rolling history (`POST /api/saves`), answering with
    /// the record RomM created.
    ///
    /// The query matches what the Kotlin app sends, so both write one history per game: the
    /// `autosave` slot, kept to the newest ten by RomM itself. A 409 is retried once with
    /// `overwrite`; a second 409 answers `None`, which on RomM 5 means the slot is waiting on this
    /// device to confirm the newest save in it — a reason to try again later, not a failure.
    /// Every save and every state of the roms `rom_ids` — a game's own and its sibling builds — in two
    /// requests (`GET /api/saves?rom_ids=…`, `/api/states?rom_ids=…`). Filtered here as well, in case
    /// a server too old to know `rom_ids` answers with the whole account.
    ///
    /// Asked as this device when it is one, so each save says which devices hold it
    /// ([`SaveAsset::device_syncs`]) — and without, should RomM not take the device.
    pub async fn fetch_game_saves(&self, conn: &Connection, rom_ids: &[i64]) -> Result<(Vec<SaveAsset>, Vec<SaveAsset>)> {
        let query: String = rom_ids.iter().map(|id| format!("rom_ids={id}")).collect::<Vec<_>>().join("&");
        let ours = |list: Vec<SaveAsset>| -> Vec<SaveAsset> {
            list.into_iter().filter(|s| s.rom_id.is_some_and(|id| rom_ids.contains(&id))).collect()
        };
        let as_device = conn.device_id.as_deref().map(|d| format!("/api/saves?{query}&device_id={}", urlencode(d)));
        let saves: Value = match as_device {
            Some(path) => match self.get_json(conn, &path).await {
                Ok(saves) => saves,
                Err(e) if matches!(e.status, 403 | 404) => self.get_json(conn, &format!("/api/saves?{query}")).await?,
                Err(e) => return Err(e),
            },
            None => self.get_json(conn, &format!("/api/saves?{query}")).await?,
        };
        let states: Value = self.get_json(conn, &format!("/api/states?{query}")).await?;
        Ok((ours(decode_list(&saves)), ours(decode_list(&states))))
    }

    /// One game's save states (`GET /api/states?rom_id=`). The records are the same shape as saves.
    pub async fn fetch_states(&self, conn: &Connection, rom_id: i64) -> Result<Vec<SaveAsset>> {
        let value: Value = self.get_json(conn, &format!("/api/states?rom_id={rom_id}")).await?;
        Ok(decode_list(&value))
    }

    /// One state's bytes: its `download_path`, or RomM 5's `/api/states/{id}/content` without one.
    pub async fn download_state(&self, conn: &Connection, state: &SaveAsset) -> Result<Vec<u8>> {
        let host = conn.host.trim_end_matches('/');
        let url = match state.download_path.as_deref().filter(|p| !p.trim().is_empty()) {
            Some(path) if path.starts_with("http://") || path.starts_with("https://") => path.to_string(),
            Some(path) => format!("{host}/{}", path.trim_start_matches('/')),
            None => format!("{host}/api/states/{}/content", state.id),
        };
        self.fetch_asset(conn, &url).await
    }

    /// Upload a save state (`POST /api/states`) under `file_name`, answering with RomM's record.
    ///
    /// RomM keeps one state per name for a game: under a name it has already, the upload replaces
    /// that state where it is — the same id, new bytes and time, the emulator this upload names, and
    /// the picture when one is sent (one not sent leaves the old). So a name is the whole identity.
    pub async fn upload_state(
        &self,
        conn: &Connection,
        rom_id: i64,
        emulator: &str,
        file_name: &str,
        bytes: Vec<u8>,
        screenshot: Option<(Vec<u8>, &str)>,
    ) -> Result<SaveAsset> {
        let mut url = endpoint(conn, "/api/states")?;
        url.query_pairs_mut().append_pair("rom_id", &rom_id.to_string()).append_pair("emulator", emulator);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|_| ApiError::new(-1, "Could not build the upload."))?;
        let mut form = reqwest::multipart::Form::new().part("stateFile", part);
        // The picture taken with the state, by its extension, which RomM keeps as its screenshot.
        if let Some((picture, ext)) = screenshot {
            let mime = if ext == "jpg" { "image/jpeg" } else { "image/png" };
            let shot = reqwest::multipart::Part::bytes(picture)
                .file_name(format!("{file_name}.{ext}"))
                .mime_str(mime)
                .map_err(|_| ApiError::new(-1, "Could not build the upload."))?;
            form = form.part("screenshotFile", shot);
        }
        let request = self
            .api(conn, reqwest::Method::POST, url)
            .multipart(form);
        self.execute_json(request).await
    }

    pub async fn upload_save(
        &self,
        conn: &Connection,
        rom_id: i64,
        emulator: &str,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<Option<SaveAsset>> {
        let mut url = endpoint(conn, "/api/saves")?;
        url.query_pairs_mut()
            .append_pair("rom_id", &rom_id.to_string())
            .append_pair("emulator", emulator)
            .append_pair("slot", "autosave")
            .append_pair("autocleanup", "true")
            .append_pair("autocleanup_limit", "10");
        // Attributed to this device, which is what RomM's slot markers are kept per.
        if let Some(device) = &conn.device_id {
            url.query_pairs_mut().append_pair("device_id", device);
        }

        for overwrite in [false, true] {
            let mut url = url.clone();
            if overwrite {
                url.query_pairs_mut().append_pair("overwrite", "true");
            }
            // A multipart body is consumed by sending it, so each attempt builds its own.
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(file_name.to_string())
                .mime_str("application/octet-stream")
                .map_err(|_| ApiError::new(-1, "Could not build the upload."))?;
            let request = self
                .api(conn, reqwest::Method::POST, url)
                .multipart(reqwest::multipart::Form::new().part("saveFile", part));
            match self.execute(request).await {
                Ok(body) => {
                    return serde_json::from_str(&body).map(Some).map_err(|e| {
                        ApiError::new(200, format!("The server sent something unexpected: {e}"))
                    });
                }
                Err(error) if error.status == 409 => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    // ---- RomM 5's sync protocol -----------------------------------------------------------------

    /// `POST /api/sync/negotiate` — put what this device holds to RomM, and get back what to do about
    /// each save on either side, under a new sync session. `rom_ids` are the games on this device:
    /// RomM offers downloads for those (and for any game a save is sent for), not for the account's
    /// whole save library. At most 500, which [`crate::sync::session`] keeps to.
    pub async fn negotiate(&self, conn: &Connection, device_id: &str, saves: &[ClientSave], rom_ids: &[i64]) -> Result<Negotiation> {
        let body = serde_json::json!({ "device_id": device_id, "saves": saves, "rom_ids": rom_ids });
        let request = self
            .post(conn, "/api/sync/negotiate")
            .json(&body);
        self.execute_json(request).await
    }

    /// `POST /api/sync/sessions/{id}/complete` — close a sync session with what came of it, carrying
    /// the play sessions it ends. Answers how many of those RomM now has, new or already known.
    pub async fn complete_sync(
        &self,
        conn: &Connection,
        session_id: i64,
        completed: usize,
        failed: usize,
        plays: &[PlaySession],
    ) -> Result<usize> {
        let mut body = serde_json::json!({ "operations_completed": completed, "operations_failed": failed });
        if !plays.is_empty() {
            body["play_sessions"] = serde_json::json!(plays);
        }
        let request = self
            .post(conn, &format!("/api/sync/sessions/{session_id}/complete"))
            .json(&body);
        let value: Value = self.execute_json(request).await?;
        let results = value["play_session_ingest"]["results"].as_array().cloned().unwrap_or_default();
        Ok(results.iter().filter(|r| matches!(r["status"].as_str(), Some("created" | "duplicate"))).count())
    }

    /// Upload one save into its slot (`POST /api/saves`), as this device and within `push`'s session.
    ///
    /// Without `overwrite`, RomM refuses (409) a save for a slot whose newest version this device has
    /// not synced — someone else's progress landed since. That is answered as [`Pushed::Refused`], for
    /// the caller to settle as the conflict it is, never retried over the top.
    pub async fn push_save(&self, conn: &Connection, push: Push<'_>) -> Result<Pushed> {
        let mut url = endpoint(conn, "/api/saves")?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("rom_id", &push.rom_id.to_string())
                .append_pair("emulator", push.emulator)
                .append_pair("slot", push.slot)
                // The device's baseline on RomM after 5.3; ignored by 5.3.
                .append_pair("content_hash", push.content_hash);
            // A rolling history, kept to the newest ten by RomM itself — as RomM's own player and the
            // other clients keep theirs.
            if push.autocleanup {
                query.append_pair("autocleanup", "true").append_pair("autocleanup_limit", "10");
            }
            if let Some(device) = &conn.device_id {
                query.append_pair("device_id", device);
            }
            if let Some(session) = push.session_id {
                query.append_pair("session_id", &session.to_string());
            }
            if push.overwrite {
                query.append_pair("overwrite", "true");
            }
        }
        let part = reqwest::multipart::Part::bytes(push.bytes)
            .file_name(push.file_name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|_| ApiError::new(-1, "Could not build the upload."))?;
        let request = self
            .api(conn, reqwest::Method::POST, url)
            .multipart(reqwest::multipart::Form::new().part("saveFile", part));
        match self.execute(request).await {
            Ok(body) => serde_json::from_str(&body)
                .map(|saved| Pushed::Saved(Box::new(saved)))
                .map_err(|e| ApiError::new(200, format!("The server sent something unexpected: {e}"))),
            Err(error) if error.status == 409 => Ok(Pushed::Refused),
            Err(error) => Err(error),
        }
    }

    /// One save's bytes from RomM 5's content route, as this device's within `session_id`.
    ///
    /// Not optimistic: RomM is told this device holds the save only by
    /// [`RommClient::confirm_downloaded`], once the bytes are in place — a download that dies on the
    /// way to disk must not leave RomM believing it arrived.
    pub async fn pull_save(&self, conn: &Connection, save_id: i64, session_id: Option<i64>) -> Result<Vec<u8>> {
        let mut url = endpoint(conn, &format!("/api/saves/{save_id}/content"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("optimistic", "false");
            if let Some(device) = &conn.device_id {
                query.append_pair("device_id", device);
            }
            if let Some(session) = session_id {
                query.append_pair("session_id", &session.to_string());
            }
        }
        self.fetch_asset(conn, url.as_str()).await
    }

    /// Every save RomM holds for the roms `rom_ids` (`GET /api/saves?rom_ids=…`), at most 500 — what
    /// a sync settles by name the saves whose slots other saves share.
    pub async fn fetch_saves_of(&self, conn: &Connection, rom_ids: &[i64]) -> Result<Vec<SaveAsset>> {
        let mut url = endpoint(conn, "/api/saves")?;
        for id in rom_ids {
            url.query_pairs_mut().append_pair("rom_ids", &id.to_string());
        }
        let request = self.api(conn, reqwest::Method::GET, url);
        let value: Value = self.execute_json(request).await?;
        // Filtered here as well, in case a RomM too old for `rom_ids` answers with the whole account.
        Ok(decode_list::<SaveAsset>(&value).into_iter().filter(|s| s.rom_id.is_some_and(|id| rom_ids.contains(&id))).collect())
    }

    /// Every version RomM holds in one slot of one game (`GET /api/saves?rom_id=&slot=`) — what a
    /// refused push is settled against.
    pub async fn slot_saves(&self, conn: &Connection, rom_id: i64, slot: &str) -> Result<Vec<SaveAsset>> {
        let mut url = endpoint(conn, "/api/saves")?;
        url.query_pairs_mut().append_pair("rom_id", &rom_id.to_string()).append_pair("slot", slot);
        let request = self.api(conn, reqwest::Method::GET, url);
        let value: Value = self.execute_json(request).await?;
        Ok(decode_list::<SaveAsset>(&value).into_iter().filter(|s| s.slot.as_deref() == Some(slot)).collect())
    }
}

/// One upload into a save slot — see [`RommClient::push_save`].
pub struct Push<'a> {
    pub rom_id: i64,
    pub emulator: &'a str,
    pub slot: &'a str,
    pub file_name: &'a str,
    pub bytes: Vec<u8>,
    /// RomM's hash of `bytes` — see [`crate::sync::hash`].
    pub content_hash: &'a str,
    pub session_id: Option<i64>,
    /// Past RomM's check that this device holds the slot's newest version: what keeping this
    /// device's copy of a conflict is.
    pub overwrite: bool,
    /// Have RomM trim the slot to its newest ten versions. Off for a slot other saves share, since
    /// the trimming counts all of them.
    pub autocleanup: bool,
}

/// What came of a [`Push`].
#[derive(Debug)]
pub enum Pushed {
    Saved(Box<SaveAsset>),
    /// The slot has a version this device has not synced (409).
    Refused,
}

/// How an install RomM asked of this device ended — see [`RommClient::report_install`].
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done,
    AlreadyInstalled,
    Failed(String),
}

/// `s` as a URL query value.
fn urlencode(s: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", [("v", s)]).map(|u| u.query().unwrap_or_default().trim_start_matches("v=").to_string()).unwrap_or_default()
}

/// Where a download is written while it is in flight: the destination's name plus `.part`.
///
/// Appended rather than substituted — `Path::with_extension` would replace `.chd` instead of
/// following it, and two discs of one game would then share a single `Driver 2 (Disc 1).part`.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// How much of a download an earlier attempt left at `part` that can be carried on from — 0 for
/// none, or for a `.part` that cannot be trusted to be this file's start.
async fn resumable(part: &Path, size: Option<u64>) -> u64 {
    let Ok(meta) = tokio::fs::metadata(part).await else { return 0 };
    let recent = meta.modified().ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age < std::time::Duration::from_secs(24 * 3600));
    let short = size.is_none_or(|size| meta.len() < size);
    if recent && short { meta.len() } else { 0 }
}

/// Where a `206 Partial Content` answer starts, from its `Content-Range: bytes <start>-<end>/<size>`.
fn range_start(response: &reqwest::Response) -> Option<u64> {
    let range = response.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    range.strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

/// A `.part` removed when this is dropped while it still holds the path.
struct PartGuard(Option<PathBuf>);

impl Drop for PartGuard {
    fn drop(&mut self) {
        if let Some(part) = self.0.take() {
            let _ = std::fs::remove_file(part);
        }
    }
}

/// How a body failed to reach the disk: the connection, which a retry can carry on from, or the
/// disk itself, which leaves nothing worth keeping.
enum Stream {
    Interrupted(ApiError),
    Disk(ApiError),
}

/// Copy a response body to `path` — after the `from` bytes already there, when carrying on — calling
/// `progress` with the running total.
async fn stream_to_file(
    response: reqwest::Response,
    path: &Path,
    from: u64,
    progress: &mut impl FnMut(u64),
) -> std::result::Result<u64, Stream> {
    use tokio::io::AsyncWriteExt;
    let disk = |e: std::io::Error| Stream::Disk(ApiError::new(-1, format!("Could not write {}: {e}", path.display())));
    let file = if from > 0 {
        tokio::fs::OpenOptions::new().append(true).open(path).await
    } else {
        tokio::fs::File::create(path).await
    }
    .map_err(disk)?;
    // Written through tokio's file, which hands the writes to its blocking pool: a slow SD card
    // holds up that pool, not the runtime thread other requests are waiting on.
    let mut file = tokio::io::BufWriter::with_capacity(1 << 20, file);
    let mut stream = response.bytes_stream();
    let mut copied = from;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            // What arrived is kept for the next attempt to carry on from — buffered here, so out to
            // the file before saying so.
            file.flush().await.map_err(disk)?;
            return Err(Stream::Interrupted(ApiError::new(-1, "The download stopped early.")));
        };
        file.write_all(&chunk).await.map_err(disk)?;
        copied += chunk.len() as u64;
        progress(copied);
    }
    file.flush().await.map_err(disk)?;
    // On the disk before it is renamed into place, so a power cut cannot leave a short game file.
    file.get_ref().sync_all().await.map_err(disk)?;
    Ok(copied)
}

/// Last-resort token extraction: scan a non-JSON body for an `rmm_...` literal.
fn scan_for_token(body: &str) -> Option<String> {
    let start = body.find("rmm_")?;
    let token: String = body[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (token.len() > 4).then_some(token)
}

/// RomM returns a bare array from some endpoints and `{items: [...]}` from others; a rom that fails
/// to decode is dropped rather than failing the page.
fn decode_list<T: serde::de::DeserializeOwned>(value: &Value) -> Vec<T> {
    let array = match value {
        Value::Array(a) => a,
        Value::Object(_) => match value.get("items").or_else(|| value.get("results")) {
            Some(Value::Array(a)) => a,
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    array
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}

/// Whether an asset URL points at the connected server, so the bearer token may be attached.
fn same_host(host: &str, url: &str) -> bool {
    fn authority(s: &str) -> Option<String> {
        let rest = s.split_once("://").map(|(_, r)| r).unwrap_or(s);
        let auth = rest.split(['/', '?', '#']).next()?;
        (!auth.is_empty()).then(|| auth.to_ascii_lowercase())
    }
    match (authority(host), authority(url)) {
        (Some(a), Some(b)) => a == b,
        // A host-relative path can only be our own server's.
        _ => !url.starts_with("http"),
    }
}

/// The URLs to try for a typed-in server address, in order.
///
/// A bare address gets `https://` first. On a host that can only be on this machine or this network
/// an `http://` fallback follows — there, plaintext is a convenience rather than an exposure, and a
/// LAN RomM very often has no certificate at all.
pub fn host_candidates(raw: &str) -> Vec<String> {
    let value = raw.trim().trim_end_matches('/');
    if value.is_empty() {
        return Vec::new();
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return vec![value.to_string()];
    }
    let https = format!("https://{value}");
    if is_private_host(value) {
        vec![https, format!("http://{value}")]
    } else {
        vec![https]
    }
}

/// LAN-only name suffixes — mDNS and the router-assigned search domains that go with it.
const LAN_SUFFIXES: [&str; 5] = [".local", ".lan", ".home", ".internal", ".localdomain"];

/// Whether `value` names a host on this machine or this network.
///
/// Covers loopback, the RFC1918 ranges, link-local, the LAN-only suffixes and single-label names —
/// a bare `nas` or `steamdeck` can only be resolved by a local search domain.
pub fn is_private_host(value: &str) -> bool {
    let rest = value.split_once("://").map(|(_, r)| r).unwrap_or(value);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Strip userinfo and the port, and unwrap a bracketed IPv6 literal.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(end) = host_port.strip_prefix('[').and_then(|h| h.find(']').map(|i| &h[..i])) {
        end.to_ascii_lowercase()
    } else {
        host_port.split(':').next().unwrap_or("").to_ascii_lowercase()
    };

    if host.is_empty() {
        return false;
    }
    if host == "localhost" || host == "::1" || host.starts_with("127.") {
        return true;
    }
    if LAN_SUFFIXES.iter().any(|s| host.ends_with(s)) {
        return true;
    }
    if !host.contains('.') {
        return true; // single-label: only a local resolver can answer it
    }
    let octets: Vec<u32> = host.split('.').filter_map(|o| o.parse().ok()).collect();
    if octets.len() == 4 && octets.iter().all(|o| *o <= 255) {
        let (a, b) = (octets[0], octets[1]);
        return a == 10
            || (a == 192 && b == 168)
            || (a == 172 && (16..=31).contains(&b))
            || (a == 169 && b == 254);
    }
    false
}

/// RomM's web UI shows the code grouped as `ABCD-EFGH` and its copy button includes the dash, so
/// strip any separators before sending — the real code is 8 alphanumerics.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_alphanumeric).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file server for the download tests: `answer` is given each request's `Range` start (0 for
    /// none) and how many requests came before, and says the status line, extra headers, the
    /// `Content-Length` to claim and the bytes actually sent — fewer than claimed is a connection
    /// that drops. `hold` keeps the connection open after them, as a download still under way.
    async fn file_server(
        answer: impl Fn(u64, usize) -> (&'static str, String, usize, Vec<u8>, bool) + Send + Sync + 'static,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let answer = std::sync::Arc::new(answer);
        tokio::spawn(async move {
            let mut served = 0;
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = socket.read(&mut buffer).await else { break };
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                }
                let text = String::from_utf8_lossy(&request).to_lowercase();
                let from = text
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.trim_end_matches('-').trim_end_matches("-\r").split('-').next()?.trim().parse().ok())
                    .unwrap_or(0);
                let (status, headers, claimed, body, hold) = answer(from, served);
                served += 1;
                let head = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {claimed}\r\n\r\n");
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.flush().await;
                if hold {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            }
        });
        format!("http://{addr}")
    }

    fn connection(host: String) -> Connection {
        Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None }
    }

    #[tokio::test]
    async fn a_download_that_drops_is_carried_on_from_where_it_stopped() {
        const FILE: &[u8] = b"0123456789";
        let host = file_server(|from, served| match served {
            // The first attempt: the connection drops after six bytes.
            0 => ("200 OK", String::new(), FILE.len(), FILE[..6].to_vec(), false),
            // The next asks for the rest, and gets it.
            _ => {
                let start = from as usize;
                ("206 Partial Content", format!("Content-Range: bytes {start}-9/10\r\n"), FILE.len() - start, FILE[start..].to_vec(), false)
            }
        })
        .await;
        let dir = std::env::temp_dir().join(format!("deckhand-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("Game.iso");
        let url = reqwest::Url::parse(&format!("{host}/game")).unwrap();
        let client = RommClient::new().unwrap();

        assert!(client.fetch_to_file(&connection(host.clone()), url.clone(), &dest, Some(10), &mut |_| {}).await.is_err());
        assert_eq!(std::fs::read(part_path(&dest)).unwrap(), b"012345", "what arrived is kept for the next attempt");

        let mut seen = 0;
        client.fetch_to_file(&connection(host), url, &dest, Some(10), &mut |n| seen = n).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), FILE, "the rest joined on, not the file again");
        assert_eq!(seen, 10, "progress counts from where it carried on");
        assert!(!part_path(&dest).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_server_that_sends_the_whole_file_again_is_not_added_to_the_part() {
        const FILE: &[u8] = b"0123456789";
        let host = file_server(|_, _| ("200 OK", String::new(), FILE.len(), FILE.to_vec(), false)).await;
        let dir = std::env::temp_dir().join(format!("deckhand-no-range-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("Game.iso");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(part_path(&dest), b"0123").unwrap();
        let url = reqwest::Url::parse(&format!("{host}/game")).unwrap();
        RommClient::new().unwrap().fetch_to_file(&connection(host), url, &dest, Some(10), &mut |_| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), FILE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_download_stopped_part_way_leaves_nothing_behind() {
        let host = file_server(|_, _| ("200 OK", String::new(), 1_000_000, vec![7u8; 4096], true)).await;
        let dir = std::env::temp_dir().join(format!("deckhand-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("Game.iso");
        let part = part_path(&dest);
        let url = reqwest::Url::parse(&format!("{host}/game")).unwrap();
        let worker = tokio::spawn({
            let dest = dest.clone();
            async move { RommClient::new().unwrap().fetch_to_file(&connection(host), url, &dest, None, &mut |_| {}).await }
        });
        for _ in 0..200 {
            if part.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(part.exists(), "under way");
        // What Cancel does to the worker: drops it where it is.
        worker.abort();
        let _ = worker.await;
        assert!(!part.exists(), "the half-written file goes with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_games_saves_come_for_all_its_roms_in_two_requests_and_nobody_elses() {
        let one = |id: i64, rom: i64| serde_json::json!({ "id": id, "rom_id": rom, "file_name": format!("{id}.srm") });
        let (host, requests) = crate::stub_server::start_recording(vec![
            // A server that ignored `rom_ids` would answer with the whole account, as this one does.
            ("GET /api/saves?", 200, serde_json::json!([one(1, 4), one(2, 5), one(3, 99)]).to_string()),
            ("GET /api/states?", 200, serde_json::json!([one(7, 5)]).to_string()),
        ])
        .await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let (saves, states) = RommClient::new().unwrap().fetch_game_saves(&conn, &[4, 5]).await.unwrap();
        assert_eq!(saves.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 2], "rom 99's is not this game's");
        assert_eq!(states.len(), 1);
        let lines = requests.lock().unwrap().clone();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.contains("rom_ids=4&rom_ids=5")), "{lines:?}");
    }

    fn played_page(ids: std::ops::Range<i64>, played: bool, total: usize) -> String {
        let rom = |id: i64| serde_json::json!({ "id": id, "rom_user": { "last_played": played.then_some("2026-10-01T20:00:00") } });
        serde_json::json!({ "items": ids.map(rom).collect::<Vec<_>>(), "total": total }).to_string()
    }

    #[tokio::test]
    async fn played_roms_are_read_page_by_page_to_the_last() {
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("GET /api/roms?limit=250&offset=0&", 200, played_page(0..250, true, 260)),
            ("GET /api/roms?limit=250&offset=250&", 200, played_page(250..260, true, 260)),
        ])
        .await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let played = RommClient::new().unwrap().fetch_played(&conn).await.unwrap();
        assert_eq!(played.len(), 260);
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_platforms_romm_changed_since_a_time_are_its_newer_roms_ones() {
        let rom = |id: i64, platform: i64, at: &str| serde_json::json!({ "id": id, "platform_id": platform, "updated_at": at });
        // Newest first, as asked — and then an older one, as a RomM that ignored the filter would
        // send: the reading stops there rather than walking its whole library.
        let page = serde_json::json!({
            "items": [rom(1, 4, "2026-10-12T09:30:00+00:00"), rom(2, 7, "2026-10-12T09:10:00.5+00:00"), rom(3, 9, "2026-10-01T00:00:00+00:00")],
            "total": 3000
        });
        let (host, requests) = crate::stub_server::start_recording(vec![("GET /api/roms?", 200, page.to_string())]).await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let changed = RommClient::new().unwrap().changed_platforms(&conn, "2026-10-12T09:00:00Z").await.unwrap();
        assert_eq!(changed, HashSet::from([4, 7]));
        let lines = requests.lock().unwrap().clone();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("updated_after=2026-10-12T09%3A00%3A00Z") && lines[0].contains("order_by=updated_at&order_dir=desc"), "{lines:?}");
    }

    #[tokio::test]
    async fn installs_are_claimed_and_answered_as_this_device() {
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("POST /api/devices/deck-1/installs/claim", 200, r#"[{"id":"r1","user_id":1,"device_id":"deck-1","rom_id":42,"file_ids":[7],"status":"taken"}]"#.into()),
            ("PUT /api/devices/deck-1/installs/r1", 200, "{}".into()),
            ("PUT /api/devices/deck-1", 200, "{}".into()),
        ])
        .await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: Some("deck-1".into()) };
        let client = RommClient::new().unwrap();
        let taken = client.claim_installs(&conn, "deck-1").await.unwrap();
        assert_eq!(taken.iter().map(|r| (r.id.as_str(), r.rom_id)).collect::<Vec<_>>(), [("r1", 42)]);
        client.report_install(&conn, "deck-1", "r1", &Outcome::Failed("x".repeat(600))).await.unwrap();
        client.accept_installs(&conn, "deck-1").await.unwrap();
        let lines = requests.lock().unwrap().clone();
        assert!(lines[1].starts_with("PUT /api/devices/deck-1/installs/r1 "), "{lines:?}");
        assert!(lines[2].starts_with("PUT /api/devices/deck-1 "), "{lines:?}");
    }

    #[tokio::test]
    async fn a_server_that_ignores_the_played_filter_is_not_read_to_the_end_of_its_library() {
        let mut first: serde_json::Value = serde_json::from_str(&played_page(0..250, true, 3000)).unwrap();
        // The played roms first, then the rest of the library, as an ordering-only answer has it.
        for rom in first["items"].as_array_mut().unwrap().iter_mut().skip(77) {
            rom["rom_user"]["last_played"] = serde_json::Value::Null;
        }
        let (host, requests) = crate::stub_server::start_recording(vec![("GET /api/roms?", 200, first.to_string())]).await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let played = RommClient::new().unwrap().fetch_played(&conn).await.unwrap();
        assert_eq!(played.len(), 77);
        assert_eq!(requests.lock().unwrap().len(), 1, "the first unplayed rom ends the reading");
    }

    #[test]
    fn bare_lan_address_gets_an_http_fallback() {
        assert_eq!(
            host_candidates("192.168.1.10:8080"),
            vec!["https://192.168.1.10:8080", "http://192.168.1.10:8080"]
        );
        assert_eq!(host_candidates("romm.local"), vec!["https://romm.local", "http://romm.local"]);
        // A public name gets https only — an http fallback there would be a downgrade, not a
        // convenience.
        assert_eq!(host_candidates("romm.example.com"), vec!["https://romm.example.com"]);
        // An explicit scheme is respected as typed.
        assert_eq!(host_candidates("http://romm.example.com/"), vec!["http://romm.example.com"]);
    }

    #[test]
    fn private_host_covers_the_lan_ranges_and_single_labels() {
        for host in ["localhost", "127.0.0.1", "10.0.0.5", "192.168.0.2", "172.20.1.1", "steamdeck", "nas.local"] {
            assert!(is_private_host(host), "{host} should be private");
        }
        for host in ["romm.example.com", "8.8.8.8", "172.32.0.1"] {
            assert!(!is_private_host(host), "{host} should not be private");
        }
    }

    #[test]
    fn pairing_code_strips_the_grouping_dash() {
        assert_eq!(normalize_code("ABCD-EFGH"), "ABCDEFGH");
        assert_eq!(normalize_code(" abcd efgh "), "abcdefgh");
    }

    #[test]
    fn token_is_found_in_a_non_json_body() {
        assert_eq!(
            scan_for_token("oops <b>rmm_abc123</b>").as_deref(),
            Some("rmm_abc123")
        );
        assert_eq!(scan_for_token("nothing here"), None);
    }

    #[test]
    fn a_partial_download_sits_beside_its_destination_rather_than_replacing_its_extension() {
        // `with_extension` would turn both discs of a set into one `Driver 2 (Disc 1).part`, and
        // the second would overwrite the first mid-flight.
        assert_eq!(
            part_path(Path::new("/roms/psx/Driver 2 (Disc 1).chd")),
            Path::new("/roms/psx/Driver 2 (Disc 1).chd.part")
        );
        assert_eq!(part_path(Path::new("/roms/ps3/EBOOT")), Path::new("/roms/ps3/EBOOT.part"));
    }

    #[test]
    fn asset_credential_only_travels_to_the_connected_host() {
        assert!(same_host("https://romm.local", "https://romm.local/assets/x.png"));
        assert!(same_host("https://romm.local", "/assets/x.png"));
        assert!(!same_host("https://romm.local", "https://images.igdb.com/x.png"));
    }
}

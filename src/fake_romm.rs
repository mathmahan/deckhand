//! A stand-in RomM for the save-sync tests — one that remembers, on a real socket.
//!
//! [`crate::stub_server`] answers each request with a canned reply, which tests one request at a
//! time. A sync is a conversation instead: the device says what it holds, the server says what to do
//! about each save, files move, and the session is closed. What the server says depends on what came
//! before — which versions exist, which of them each device last had, what another device did in the
//! meantime — so testing it takes a server that keeps all of that.
//!
//! What this one keeps and decides is written from two sources only: RomM's published API description
//! (`/openapi.json`, 5.4.0), for the requests and the shape of every answer, and what a real RomM 5.4
//! was seen to answer in the cases the description leaves open — when a device's save counts as
//! changed, when a push is refused, what happens to an open session, what a state sent up under a
//! name the game has already does. [`contract`] at the bottom of this file replays those observations
//! against this server on every test run, and against a real RomM when one is named, so the two
//! cannot drift apart unnoticed.
//!
//! Its clock is its own: it moves a second with every write, and otherwise only when a test moves
//! it, so a test can place a device's file times exactly against the server's. Times are kept to the
//! second, as RomM's database keeps them — though a push is answered with its time to the
//! microsecond, as RomM answers it before the database has rounded it off.

#![cfg(test)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::sync::{format_timestamp_precise, hash, parse_timestamp};

/// Where the clock starts: a September 2026 afternoon.
const START: Duration = Duration::from_secs(1_790_000_000);

/// One stored version of a save.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: i64,
    pub rom_id: i64,
    pub file_name: String,
    /// None is a save kept apart from syncing — an archive copy, never paired with a device's save.
    pub slot: Option<String>,
    pub emulator: Option<String>,
    pub content_hash: String,
    pub updated_at: SystemTime,
    pub bytes: Vec<u8>,
    /// The device that sent it, if one did.
    pub origin: Option<String>,
}

/// One save state. A game has one per name: a state sent up under a name the game has already is
/// that state, replaced where it is.
#[derive(Debug, Clone)]
pub struct State {
    pub id: i64,
    pub rom_id: i64,
    pub file_name: String,
    /// The emulator, or core, the last upload under its name said made it.
    pub emulator: Option<String>,
    pub updated_at: SystemTime,
    pub bytes: Vec<u8>,
    /// Its screenshot: the screenshot's own id, and the picture.
    pub picture: Option<(i64, Vec<u8>)>,
}

/// What the server remembers of one device and one version.
#[derive(Debug, Clone, Default)]
struct Holding {
    /// The version's time when the device last had it.
    had: Option<SystemTime>,
    /// The version's content when the device last had it: a device still listing that, after the
    /// version was replaced, holds a version the server no longer has.
    had_hash: Option<String>,
    /// The device asked for the version to be left out of its syncs.
    untracked: bool,
}

#[derive(Debug, Clone)]
struct Session {
    device: String,
    open: bool,
    completed: i64,
    failed: i64,
}

/// A play session taken in, and the sync session it came with, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub rom_id: Option<i64>,
    pub start: String,
    pub session: Option<i64>,
}

/// Everything the server holds.
#[derive(Debug)]
struct Server {
    clock: SystemTime,
    versions: Vec<Version>,
    states: Vec<State>,
    holdings: HashMap<(String, i64), Holding>,
    /// Every device the server knows, and whether its sync is on.
    devices: HashMap<String, bool>,
    sessions: BTreeMap<i64, Session>,
    plays: Vec<Play>,
    last_id: i64,
    /// Every request line, in order.
    log: Vec<String>,
    /// Every action a negotiation answered, in order.
    answered: Vec<String>,
    /// What a test has arranged: a server from before sync sessions, another device's push landing
    /// just ahead of the next one, a server that stops answering negotiations.
    legacy: bool,
    racer: Option<(i64, String, Vec<u8>)>,
    negotiations_left: Option<usize>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            clock: SystemTime::UNIX_EPOCH + START,
            versions: Vec::new(),
            states: Vec::new(),
            holdings: HashMap::new(),
            devices: HashMap::new(),
            sessions: BTreeMap::new(),
            plays: Vec::new(),
            last_id: 0,
            log: Vec::new(),
            answered: Vec::new(),
            legacy: false,
            racer: None,
            negotiations_left: None,
        }
    }
}

/// A running stand-in RomM.
pub struct FakeRomm {
    pub host: String,
    server: Arc<Mutex<Server>>,
}

impl FakeRomm {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free local port");
        let host = format!("http://{}", listener.local_addr().expect("the port it got"));
        let server = Arc::new(Mutex::new(Server::default()));
        let shared = server.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(answer(socket, shared.clone()));
            }
        });
        FakeRomm { host, server }
    }

    fn lock(&self) -> MutexGuard<'_, Server> {
        self.server.lock().unwrap()
    }

    /// Make `id` a device the server knows, with its sync on.
    pub fn device(&self, id: &str) {
        self.lock().devices.insert(id.to_string(), true);
    }

    /// Turn `id`'s sync off, as its settings in RomM would.
    pub fn sync_off(&self, id: &str) {
        self.lock().devices.insert(id.to_string(), false);
    }

    /// Forget every device, as a rebuilt server would.
    pub fn forget_devices(&self) {
        self.lock().devices.clear();
    }

    pub fn now(&self) -> SystemTime {
        self.lock().clock
    }

    pub fn advance(&self, by: Duration) {
        self.lock().clock += by;
    }

    /// A version sent straight into `slot` by some other client — another device, or the web player
    /// — which then holds it. Answers its id.
    pub fn push_from(&self, device: Option<&str>, rom_id: i64, slot: &str, name: &str, bytes: &[u8]) -> i64 {
        let mut server = self.lock();
        let at = server.tick();
        let id = server.next_id();
        server.versions.push(Version {
            id,
            rom_id,
            file_name: stamped(name, at),
            slot: Some(slot.into()),
            emulator: Some("snes9x".into()),
            content_hash: hash::of_upload(bytes),
            updated_at: at,
            bytes: bytes.to_vec(),
            origin: device.map(str::to_string),
        });
        if let Some(device) = device {
            server.had(device, id);
        }
        id
    }

    /// A version replaced where it is — same id, new bytes, new time — as the web player saves.
    pub fn rewrite(&self, save_id: i64, bytes: &[u8]) {
        let mut server = self.lock();
        server.replace(save_id, bytes.to_vec()).expect("a version to replace");
    }

    /// The bytes stored for `save_id` changed under the hash recorded for them — a damaged disk.
    pub fn garble(&self, save_id: i64, bytes: &[u8]) {
        let mut server = self.lock();
        let version = server.versions.iter_mut().find(|v| v.id == save_id).expect("a version to damage");
        version.bytes = bytes.to_vec();
    }

    /// Every version of a game's saves deleted on the server.
    pub fn delete_all(&self, rom_id: i64) {
        self.lock().versions.retain(|v| v.rom_id != rom_id);
    }

    pub fn untrack(&self, device: &str, save_id: i64) {
        self.lock().holdings.entry((device.into(), save_id)).or_default().untracked = true;
    }

    /// The next push from anyone is preceded by another device's, of `bytes` into `slot`.
    pub fn race(&self, rom_id: i64, slot: &str, bytes: &[u8]) {
        self.lock().racer = Some((rom_id, slot.into(), bytes.to_vec()));
    }

    /// A RomM from before sync sessions: `/api/sync/` is not there.
    pub fn without_sync_sessions(&self) {
        self.lock().legacy = true;
    }

    /// Answer `n` more negotiations, then fail every one after.
    pub fn fail_negotiations_after(&self, n: usize) {
        self.lock().negotiations_left = Some(n);
    }

    /// A game's versions, oldest first.
    pub fn rows(&self, rom_id: i64) -> Vec<Version> {
        let mut versions: Vec<Version> = self.lock().versions.iter().filter(|v| v.rom_id == rom_id).cloned().collect();
        versions.sort_by_key(|v| v.updated_at);
        versions
    }

    /// The version a slot is at now.
    pub fn newest(&self, rom_id: i64, slot: &str) -> Option<Version> {
        self.rows(rom_id).into_iter().rfind(|v| v.slot.as_deref() == Some(slot))
    }

    /// Whether the server has `device` holding `save_id` as it is now.
    pub fn holds(&self, device: &str, save_id: i64) -> bool {
        let server = self.lock();
        let Some(version) = server.versions.iter().find(|v| v.id == save_id) else { return false };
        server.holding(device, save_id).had.is_some_and(|had| had >= version.updated_at)
    }

    /// A state sent up by some other client — another device, the web player — under `name`, which
    /// replaces the game's state of that name as an upload from this app's would. Answers its id.
    pub fn state_from(&self, rom_id: i64, name: &str, emulator: &str, bytes: &[u8], picture: Option<&[u8]>) -> i64 {
        let query = HashMap::from([("rom_id".to_string(), rom_id.to_string()), ("emulator".to_string(), emulator.to_string())]);
        let (_, answer) = self.lock().store_state(&query, name, bytes.to_vec(), picture.map(<[u8]>::to_vec));
        answer["id"].as_i64().expect("a stored state")
    }

    /// A game's states, oldest first.
    pub fn states(&self, rom_id: i64) -> Vec<State> {
        let mut states: Vec<State> = self.lock().states.iter().filter(|s| s.rom_id == rom_id).cloned().collect();
        states.sort_by_key(|s| (s.updated_at, s.id));
        states
    }

    /// A state deleted on the server, as from RomM's web page.
    pub fn delete_state(&self, id: i64) {
        self.lock().states.retain(|s| s.id != id);
    }

    /// The actions negotiations answered, in order.
    pub fn answered(&self) -> Vec<String> {
        self.lock().answered.clone()
    }

    /// How many requests began with `prefix` — `POST /api/saves?`, say.
    pub fn requests(&self, prefix: &str) -> usize {
        self.lock().log.iter().filter(|line| line.starts_with(prefix)).count()
    }

    /// Every request line, in order.
    pub fn log(&self) -> Vec<String> {
        self.lock().log.clone()
    }

    pub fn plays(&self) -> Vec<Play> {
        self.lock().plays.clone()
    }

    /// How many sync sessions are still open.
    pub fn open_sessions(&self) -> usize {
        self.lock().sessions.values().filter(|s| s.open).count()
    }
}

/// `name` with the server's time in it, as a slotted save is stored: `Game [2026-07-01_10-00-00].srm`.
fn stamped(name: &str, at: SystemTime) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    let time = format_timestamp_precise(at);
    let (date, clock) = (&time[..10], time[11..19].replace(':', "-"));
    format!("{stem} [{date}_{clock}]{extension}")
}

/// The server's way of writing a time: microseconds, and an offset rather than `Z`.
fn written(at: SystemTime) -> String {
    format_timestamp_precise(at).replace('Z', "+00:00")
}

fn not_found(what: &str) -> (u16, Value) {
    (404, json!({ "detail": format!("{what} not found") }))
}

impl Server {
    /// The clock moved on by a write.
    fn tick(&mut self) -> SystemTime {
        self.clock += Duration::from_secs(1);
        self.clock
    }

    fn next_id(&mut self) -> i64 {
        self.last_id += 1;
        self.last_id
    }

    fn holding(&self, device: &str, save_id: i64) -> Holding {
        self.holdings.get(&(device.to_string(), save_id)).cloned().unwrap_or_default()
    }

    /// `device` now has `save_id` as it is.
    fn had(&mut self, device: &str, save_id: i64) {
        let Some(version) = self.versions.iter().find(|v| v.id == save_id) else { return };
        let (at, hash) = (version.updated_at, version.content_hash.clone());
        let holding = self.holdings.entry((device.to_string(), save_id)).or_default();
        holding.had = Some(at);
        holding.had_hash = Some(hash);
    }

    fn version_json(version: &Version) -> Value {
        json!({
            "id": version.id,
            "rom_id": version.rom_id,
            "file_name": version.file_name,
            "slot": version.slot,
            "emulator": version.emulator,
            "content_hash": version.content_hash,
            "updated_at": written(version.updated_at),
            "file_size_bytes": version.bytes.len(),
            "download_path": format!("/api/raw/assets/saves/{}", version.id),
            "device_syncs": [],
        })
    }

    /// The version each slot is at, for the games in `scope` (every game when there is none). Saves
    /// with no slot are never in it: nothing is paired with them.
    fn slots(&self, scope: Option<&HashSet<i64>>) -> BTreeMap<(i64, String), Version> {
        let mut slots: BTreeMap<(i64, String), Version> = BTreeMap::new();
        for version in &self.versions {
            let Some(slot) = &version.slot else { continue };
            if scope.is_some_and(|games| !games.contains(&version.rom_id)) {
                continue;
            }
            let key = (version.rom_id, slot.clone());
            if slots.get(&key).is_none_or(|at| version.updated_at > at.updated_at) {
                slots.insert(key, version.clone());
            }
        }
        slots
    }

    /// `POST /api/sync/negotiate`: what the device should do about each of its saves, and about each
    /// slot it did not mention.
    fn negotiate(&mut self, body: &Value) -> (u16, Value) {
        if let Some(left) = self.negotiations_left.as_mut() {
            if *left == 0 {
                return (500, json!({ "detail": "the server failed" }));
            }
            *left -= 1;
        }
        let device = body["device_id"].as_str().unwrap_or_default().to_string();
        match self.devices.get(&device) {
            None => return not_found(&format!("Device {device}")),
            // Deckhand tells this refusal from others by the word "disabled", as RomM words it.
            Some(false) => return (400, json!({ "detail": "this device has sync disabled" })),
            Some(true) => {}
        }
        // Seen of RomM 5.4: a new negotiation leaves any session the device still has open alone.
        let session_id = self.next_id();
        self.sessions.insert(session_id, Session { device: device.clone(), open: true, completed: 0, failed: 0 });

        let listed = body["saves"].as_array().cloned().unwrap_or_default();
        let scope: Option<HashSet<i64>> = body["rom_ids"]
            .as_array()
            .map(|games| games.iter().chain(listed.iter().map(|save| &save["rom_id"])).filter_map(Value::as_i64).collect());
        let slots = self.slots(scope.as_ref());
        let mut operations = Vec::new();
        let mut mentioned = HashSet::new();
        for save in &listed {
            let rom_id = save["rom_id"].as_i64().unwrap_or_default();
            let slot = save["slot"].as_str();
            let Some(version) = slot.and_then(|slot| slots.get(&(rom_id, slot.to_string()))) else {
                operations.push(json!({
                    "action": "upload", "rom_id": rom_id, "save_id": null, "file_name": save["file_name"],
                    "slot": save["slot"], "emulator": save["emulator"], "reason": "on the device, not on the server",
                }));
                continue;
            };
            mentioned.insert(version.id);
            let device_time = save["updated_at"].as_str().and_then(parse_timestamp).unwrap_or(SystemTime::UNIX_EPOCH);
            let holding = self.holding(&device, version.id);
            let device_hash = save["content_hash"].as_str();
            // Seen of RomM 5.4: the same content on both sides is taken as the device having the
            // version, from then on.
            if !holding.untracked && device_hash == Some(version.content_hash.as_str()) {
                self.had(&device, version.id);
            }
            let (action, reason) = decide(device_hash, device_time, version, &holding);
            operations.push(operation(action, version, reason));
        }
        for version in slots.values().filter(|v| !mentioned.contains(&v.id)) {
            let holding = self.holding(&device, version.id);
            match holding.had {
                _ if holding.untracked => {}
                Some(had) if had >= version.updated_at => {}
                Some(_) => operations.push(operation("download", version, "updated on the server since this device last had it")),
                None => operations.push(operation("download", version, "on the server, not on the device")),
            }
        }
        self.answered.extend(operations.iter().map(|op| op["action"].as_str().unwrap_or_default().to_string()));
        let total = |action: &str| operations.iter().filter(|op| op["action"] == action).count();
        let answer = json!({
            "session_id": session_id,
            "total_upload": total("upload"),
            "total_download": total("download"),
            "total_conflict": total("conflict"),
            "total_no_op": total("no_op"),
            "total_delete": total("delete"),
            "operations": operations,
        });
        (200, answer)
    }

    /// `POST /api/saves`: a version stored, or refused.
    fn store(&mut self, query: &HashMap<String, String>, name: &str, bytes: Vec<u8>) -> (u16, Value) {
        if let Some((rom_id, slot, racing)) = self.racer.take() {
            let at = self.tick();
            let id = self.next_id();
            let content_hash = hash::of_upload(&racing);
            let file_name = stamped(name, at);
            self.versions.push(Version { id, rom_id, file_name, slot: Some(slot), emulator: None, content_hash, updated_at: at, bytes: racing, origin: Some("another device".into()) });
        }
        let rom_id: i64 = query.get("rom_id").and_then(|id| id.parse().ok()).unwrap_or_default();
        let slot = query.get("slot").cloned();
        let device = query.get("device_id").cloned();
        let overwrite = query.get("overwrite").is_some_and(|v| v == "true");
        if let Some(device) = &device
            && !self.devices.contains_key(device)
        {
            return not_found(&format!("Device {device}"));
        }
        let newest = slot.as_ref().and_then(|slot| self.slots(Some(&HashSet::from([rom_id]))).remove(&(rom_id, slot.clone())));
        // Seen of RomM 5.4: a device that has not had the slot's newest version may not push over it
        // unless it says to overwrite.
        if let (Some(device), Some(newest), false) = (&device, &newest, overwrite)
            && self.holding(device, newest.id).had.is_none_or(|had| had < newest.updated_at)
        {
            return (409, json!({ "detail": "this device has not had the version the slot is at" }));
        }
        let content_hash = hash::of_upload(&bytes);
        // Seen of RomM 5.4: the same content into the same slot again is the version already there.
        if !overwrite
            && let Some(slot) = &slot
            && let Some(same) = self.versions.iter().find(|v| v.rom_id == rom_id && v.slot.as_ref() == Some(slot) && v.content_hash == content_hash)
        {
            return (200, Self::version_json(same));
        }
        let at = self.tick();
        let id = self.next_id();
        let file_name = if slot.is_some() { stamped(name, at) } else { name.to_string() };
        let version = Version { id, rom_id, file_name, slot: slot.clone(), emulator: query.get("emulator").cloned(), content_hash, updated_at: at, bytes, origin: device.clone() };
        let mut answer = Self::version_json(&version);
        // Answered before the database rounds the time off to the second it keeps.
        answer["updated_at"] = json!(written(at + Duration::from_micros(516_854)));
        self.versions.push(version);
        if let Some(device) = &device {
            self.had(device, id);
        }
        self.count_in(query);
        if let (Some(slot), Some("true")) = (&slot, query.get("autocleanup").map(String::as_str)) {
            let keep = query.get("autocleanup_limit").and_then(|n| n.parse().ok()).unwrap_or(10usize).max(1);
            self.keep_newest(rom_id, slot, keep);
        }
        (200, answer)
    }

    /// The `keep` newest versions of a slot stay; the rest go.
    fn keep_newest(&mut self, rom_id: i64, slot: &str, keep: usize) {
        let mut times: Vec<SystemTime> = self.versions.iter().filter(|v| v.rom_id == rom_id && v.slot.as_deref() == Some(slot)).map(|v| v.updated_at).collect();
        times.sort_unstable_by(|a, b| b.cmp(a));
        if let Some(&oldest_kept) = times.get(keep - 1) {
            self.versions.retain(|v| !(v.rom_id == rom_id && v.slot.as_deref() == Some(slot) && v.updated_at < oldest_kept));
        }
    }

    /// `POST /api/states`: a state kept under its name. A name the game has already is that state,
    /// replaced where it is — same id, new bytes, a new time, and the emulator the upload names,
    /// whichever that is; a picture sent with it replaces the state's, and one not sent leaves it.
    fn store_state(&mut self, query: &HashMap<String, String>, name: &str, bytes: Vec<u8>, picture: Option<Vec<u8>>) -> (u16, Value) {
        let Some(rom_id) = query.get("rom_id").and_then(|r| r.parse::<i64>().ok()) else {
            return (422, json!({ "detail": "rom_id is needed" }));
        };
        let at = self.tick();
        let existing = self.states.iter().position(|s| s.rom_id == rom_id && s.file_name == name);
        let kept_picture = existing.and_then(|i| self.states[i].picture.as_ref().map(|(id, _)| *id));
        let picture = picture.map(|p| (kept_picture.unwrap_or_else(|| self.next_id()), p));
        let index = match existing {
            Some(index) => index,
            None => {
                let id = self.next_id();
                self.states.push(State { id, rom_id, file_name: name.to_string(), emulator: None, updated_at: at, bytes: Vec::new(), picture: None });
                self.states.len() - 1
            }
        };
        let state = &mut self.states[index];
        state.emulator = query.get("emulator").cloned();
        state.updated_at = at;
        state.bytes = bytes;
        if picture.is_some() {
            state.picture = picture;
        }
        let mut answer = Self::state_json(state);
        // A new one is answered before the database has rounded its time off, as a save push is; one
        // replaced, with the time the database keeps.
        if existing.is_none() {
            answer["updated_at"] = json!(written(at + Duration::from_micros(516_854)));
        }
        (200, answer)
    }

    fn state_json(state: &State) -> Value {
        json!({
            "id": state.id,
            "rom_id": state.rom_id,
            "file_name": state.file_name,
            "emulator": state.emulator,
            "updated_at": written(state.updated_at),
            "file_size_bytes": state.bytes.len(),
            "download_path": format!("/api/states/{}/content", state.id),
            "screenshot": state.picture.as_ref().map(|(id, _)| json!({
                "id": id,
                "file_name": format!("{}.png", state.file_name),
                "download_path": format!("/api/screenshots/{id}/content"),
            })),
        })
    }

    /// `PUT /api/saves/{id}`: new bytes for a version where it is, at a new time.
    fn replace(&mut self, save_id: i64, bytes: Vec<u8>) -> Option<Value> {
        let at = self.tick();
        let version = self.versions.iter_mut().find(|v| v.id == save_id)?;
        version.content_hash = hash::of_upload(&bytes);
        version.bytes = bytes;
        version.updated_at = at;
        Some(Self::version_json(version))
    }

    /// A transfer made as part of a sync session counts towards it.
    fn count_in(&mut self, query: &HashMap<String, String>) {
        if let Some(session) = query.get("session_id").and_then(|id| id.parse::<i64>().ok()).and_then(|id| self.sessions.get_mut(&id)) {
            session.completed += 1;
        }
    }

    /// `POST /api/sync/sessions/{id}/complete`, with the play sessions it brings.
    fn complete(&mut self, session_id: i64, body: &Value) -> (u16, Value) {
        let Some(session) = self.sessions.get_mut(&session_id) else { return not_found(&format!("Sync session {session_id}")) };
        if !session.open {
            return (400, json!({ "detail": "the session is already COMPLETED" }));
        }
        session.open = false;
        session.completed = body["operations_completed"].as_i64().unwrap_or_default();
        session.failed = body["operations_failed"].as_i64().unwrap_or_default();
        let entries = body["play_sessions"].as_array().cloned().unwrap_or_default();
        let results: Vec<Value> = entries.iter().enumerate().map(|(index, entry)| json!({ "index": index, "status": self.ingest(entry, Some(session_id)) })).collect();
        let session = self.session_json(session_id);
        (200, json!({ "session": session, "play_session_ingest": { "results": results } }))
    }

    /// One play session in, unless the same game's session from the same start is in already.
    fn ingest(&mut self, entry: &Value, session: Option<i64>) -> &'static str {
        let play = Play { rom_id: entry["rom_id"].as_i64(), start: entry["start_time"].as_str().unwrap_or_default().to_string(), session };
        if self.plays.iter().any(|p| p.rom_id == play.rom_id && p.start == play.start) {
            return "duplicate";
        }
        self.plays.push(play);
        "created"
    }

    fn session_json(&self, id: i64) -> Value {
        let session = &self.sessions[&id];
        json!({
            "id": id,
            "device_id": session.device,
            "status": if session.open { "IN_PROGRESS" } else { "COMPLETED" },
            "operations_completed": session.completed,
            "operations_failed": session.failed,
        })
    }

    /// One request, answered: a status, and JSON or a file's bytes.
    fn route(&mut self, method: &str, path: &str, query: &HashMap<String, String>, headers: &str, body: Vec<u8>) -> (u16, Value, Option<Vec<u8>>) {
        let json_body = || serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        if self.legacy && path.starts_with("/api/sync/") {
            return (404, json!({ "detail": "Not Found" }), None);
        }
        let id = |text: &str| text.parse::<i64>().unwrap_or_default();
        let (status, value) = match (method, parts.as_slice()) {
            ("POST", ["api", "sync", "negotiate"]) => self.negotiate(&json_body()),
            ("POST", ["api", "sync", "sessions", session, "complete"]) => self.complete(id(session), &json_body()),
            ("GET", ["api", "sync", "sessions", session]) => match self.sessions.contains_key(&id(session)) {
                true => (200, self.session_json(id(session))),
                false => not_found(&format!("Sync session {session}")),
            },
            ("POST", ["api", "saves"]) => match save_file(headers, &body) {
                Some((name, bytes)) => self.store(query, &name, bytes),
                None => (422, json!({ "detail": "a saveFile part is needed" })),
            },
            ("PUT", ["api", "saves", save]) => match save_file(headers, &body).and_then(|(_, bytes)| self.replace(id(save), bytes)) {
                Some(version) => (200, version),
                None => not_found(&format!("Save {save}")),
            },
            ("GET", ["api", "saves"]) => {
                let rom_id: Option<i64> = query.get("rom_id").and_then(|r| r.parse().ok());
                let slot = query.get("slot");
                let listed = self.versions.iter().filter(|v| rom_id.is_none_or(|r| v.rom_id == r) && slot.is_none_or(|s| v.slot.as_ref() == Some(s)));
                (200, Value::Array(listed.map(Self::version_json).collect()))
            }
            ("GET", ["api", "saves", save, "content"]) => {
                let Some(version) = self.versions.iter().find(|v| v.id == id(save)).cloned() else { return (404, json!({ "detail": "Not Found" }), None) };
                // Unless asked not to, the server takes a download as had, without waiting to be told.
                if let Some(device) = query.get("device_id")
                    && query.get("optimistic").is_none_or(|v| v != "false")
                {
                    self.had(device, version.id);
                }
                self.count_in(query);
                return (200, Value::Null, Some(version.bytes));
            }
            ("GET", ["api", "raw", "assets", "saves", save]) => match self.versions.iter().find(|v| v.id == id(save)) {
                Some(version) => return (200, Value::Null, Some(version.bytes.clone())),
                None => (404, json!({ "detail": "Not Found" })),
            },
            ("POST", ["api", "saves", save, "downloaded"]) => {
                let device = json_body()["device_id"].as_str().unwrap_or_default().to_string();
                match self.versions.iter().any(|v| v.id == id(save)) {
                    true => {
                        self.had(&device, id(save));
                        (200, json!({ "id": id(save) }))
                    }
                    false => not_found(&format!("Save {save}")),
                }
            }
            ("POST", ["api", "saves", save, "untrack"]) => {
                let device = json_body()["device_id"].as_str().unwrap_or_default().to_string();
                self.holdings.entry((device, id(save))).or_default().untracked = true;
                (200, json!({ "id": id(save) }))
            }
            ("POST", ["api", "saves", "delete"]) => {
                let ids: Vec<i64> = json_body()["saves"].as_array().map(|ids| ids.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                // Seen of RomM 5.4: one id it does not have refuses the whole request.
                match ids.iter().find(|id| !self.versions.iter().any(|v| v.id == **id)) {
                    Some(missing) => not_found(&format!("Save {missing}")),
                    None => {
                        self.versions.retain(|v| !ids.contains(&v.id));
                        (200, json!(ids))
                    }
                }
            }
            ("GET", ["api", "states"]) => {
                let rom_id: Option<i64> = query.get("rom_id").and_then(|r| r.parse().ok());
                (200, Value::Array(self.states.iter().filter(|s| rom_id.is_none_or(|r| s.rom_id == r)).map(Self::state_json).collect()))
            }
            ("POST", ["api", "states"]) => match file_part(headers, &body, "stateFile") {
                Some((name, bytes)) => {
                    let picture = file_part(headers, &body, "screenshotFile").map(|(_, picture)| picture);
                    self.store_state(query, &name, bytes, picture)
                }
                None => (422, json!({ "detail": "a stateFile part is needed" })),
            },
            ("GET", ["api", "states", state, "content"]) => match self.states.iter().find(|s| s.id == id(state)) {
                Some(state) => return (200, Value::Null, Some(state.bytes.clone())),
                None => (404, json!({ "detail": "Not Found" })),
            },
            ("GET", ["api", "screenshots", shot, "content"]) => {
                match self.states.iter().find_map(|s| s.picture.as_ref().filter(|(i, _)| *i == id(shot))) {
                    Some((_, picture)) => return (200, Value::Null, Some(picture.clone())),
                    None => (404, json!({ "detail": "Not Found" })),
                }
            }
            ("POST", ["api", "states", "delete"]) => {
                let ids: Vec<i64> = json_body()["states"].as_array().map(|ids| ids.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                self.states.retain(|s| !ids.contains(&s.id));
                (200, json!(ids))
            }
            ("GET", ["api", "devices"]) => (200, Value::Array(self.devices.keys().map(|id| json!({ "id": id })).collect())),
            ("POST", ["api", "devices"]) => {
                let id = format!("device-{}", self.next_id());
                self.devices.insert(id.clone(), true);
                (201, json!({ "device_id": id, "name": json_body()["name"], "created_at": written(self.clock) }))
            }
            ("PUT", ["api", "devices", device]) => match (self.devices.get_mut(*device), json_body()["sync_enabled"].as_bool()) {
                (Some(on), Some(sync)) => {
                    *on = sync;
                    (200, json!({ "id": device, "sync_enabled": sync }))
                }
                (Some(on), None) => (200, json!({ "id": device, "sync_enabled": *on })),
                (None, _) => not_found(&format!("Device {device}")),
            },
            ("DELETE", ["api", "devices", device]) => match self.devices.remove(*device) {
                Some(_) => return (204, Value::Null, Some(Vec::new())),
                None => not_found(&format!("Device {device}")),
            },
            ("POST", ["api", "play-sessions"]) => {
                let entries = json_body()["sessions"].as_array().cloned().unwrap_or_default();
                let results: Vec<Value> = entries.iter().enumerate().map(|(index, entry)| json!({ "index": index, "status": self.ingest(entry, None) })).collect();
                (201, json!({ "results": results }))
            }
            _ => (404, json!({ "detail": "Not Found" })),
        };
        (status, value, None)
    }
}

/// What a device's listed save and the version its slot is at call for, with why — the rules seen of
/// RomM 5.4.
///
/// Content the same on both sides needs nothing. A device still listing what it last had of a version
/// since replaced takes the replacement. Otherwise, when the device has had this version, each side
/// counts as changed if its time is past the version's time when the device had it: one side changed
/// moves that side's copy over, both changed is a conflict, neither is nothing. When the device never
/// had this version, the later time wins, and the same time is a conflict.
fn decide(device_hash: Option<&str>, device_time: SystemTime, version: &Version, holding: &Holding) -> (&'static str, &'static str) {
    if holding.untracked {
        return ("no_op", "untracked: this device asked to be left out of this save's syncs");
    }
    if device_hash == Some(version.content_hash.as_str()) {
        return ("no_op", "the same content on both sides");
    }
    if device_hash.is_some() && device_hash == holding.had_hash.as_deref() && holding.had.is_some_and(|had| version.updated_at > had) {
        return ("download", "the device holds a version the server has since replaced");
    }
    match holding.had {
        Some(had) => match (device_time > had, version.updated_at > had) {
            (true, true) => ("conflict", "changed on the device and on the server since they last agreed"),
            (true, false) => ("upload", "changed on the device since it last had this version"),
            (false, true) => ("download", "changed on the server since this device last had it"),
            (false, false) => ("no_op", "unchanged on both sides since they last agreed"),
        },
        None if device_time > version.updated_at => ("upload", "no shared history, and the device's copy is newer"),
        None if device_time < version.updated_at => ("download", "no shared history, and the server's copy is newer"),
        None => ("conflict", "no shared history, the same time, and different content"),
    }
}

fn operation(action: &str, version: &Version, reason: &str) -> Value {
    json!({
        "action": action, "rom_id": version.rom_id, "save_id": version.id, "file_name": version.file_name,
        "slot": version.slot, "emulator": version.emulator, "reason": reason,
        "server_updated_at": written(version.updated_at), "server_content_hash": version.content_hash,
    })
}

/// Serve one connection: read its request, answer it, close.
async fn answer(mut socket: TcpStream, server: Arc<Mutex<Server>>) {
    let Some((line, headers, body)) = request(&mut socket).await else { return };
    let (status, value, bytes) = {
        let mut server = server.lock().unwrap();
        server.log.push(line.clone());
        let mut words = line.split_whitespace();
        let (method, target) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        server.route(method, path, &query_pairs(query), &headers, body)
    };
    let (kind, payload) = match bytes {
        Some(bytes) => ("application/octet-stream", bytes),
        None => ("application/json", value.to_string().into_bytes()),
    };
    let head = format!("HTTP/1.1 {status} Answer\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&payload).await;
    let _ = socket.flush().await;
}

/// Read a whole request off `socket`: its first line, the header lines after it, and the body its
/// `Content-Length` says it has.
async fn request(socket: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut read = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        read.extend_from_slice(&chunk[..n]);
        if let Some(end) = position(&read, b"\r\n\r\n") {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&read[..head_end]).into_owned();
    let length: usize = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    while read.len() < head_end + 4 + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        read.extend_from_slice(&chunk[..n]);
    }
    let (line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    Some((line.to_string(), headers.to_string(), read[head_end + 4..].to_vec()))
}

/// The file sent as a multipart body's `saveFile` part: its name and bytes.
fn save_file(headers: &str, body: &[u8]) -> Option<(String, Vec<u8>)> {
    file_part(headers, body, "saveFile")
}

/// The file sent as a multipart body's part named `field`: its name and bytes.
fn file_part(headers: &str, body: &[u8], field: &str) -> Option<(String, Vec<u8>)> {
    let content_type = headers.lines().filter_map(|l| l.split_once(':')).find(|(name, _)| name.trim().eq_ignore_ascii_case("content-type"))?.1;
    let boundary = content_type.split(';').find_map(|p| p.trim().strip_prefix("boundary="))?.trim_matches('"');
    let separator = format!("--{boundary}");
    let mut rest = body;
    while let Some(at) = position(rest, separator.as_bytes()) {
        rest = &rest[at + separator.len()..];
        let Some(head_end) = position(rest, b"\r\n\r\n") else { break };
        let head = String::from_utf8_lossy(&rest[..head_end]);
        let content = &rest[head_end + 4..];
        let end = position(content, separator.as_bytes()).map_or(content.len(), |e| e.saturating_sub(2));
        if head.contains(&format!("name=\"{field}\"")) {
            let name = head.split("filename=\"").nth(1)?.split('"').next()?.to_string();
            return Some((name, content[..end].to_vec()));
        }
    }
    None
}

fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// `a=1&b=x%20y` as pairs, with `%XX` and `+` turned back into what they stand for.
fn query_pairs(query: &str) -> HashMap<String, String> {
    fn unescape(text: &str) -> String {
        let mut out = Vec::with_capacity(text.len());
        let mut bytes = text.bytes();
        while let Some(byte) = bytes.next() {
            match byte {
                b'+' => out.push(b' '),
                b'%' => {
                    let pair = [bytes.next(), bytes.next()];
                    let hex = pair.iter().flatten().map(|&b| b as char).collect::<String>();
                    match u8::from_str_radix(&hex, 16) {
                        Ok(decoded) if hex.len() == 2 => out.push(decoded),
                        _ => {
                            out.push(b'%');
                            out.extend(hex.bytes());
                        }
                    }
                }
                other => out.push(other),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    query.split('&').filter(|pair| !pair.is_empty()).map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (unescape(key), unescape(value))
    }).collect()
}

/// The observations this server is built on, replayed: one conversation of throwaway devices with a
/// save in a slot of its own, and a save state under a name of its own, whose every answer is checked
/// against what a real RomM 5.4 gave.
///
/// It runs against this server on every test run. Against a real RomM it runs only when asked —
///
/// ```text
/// DECKHAND_LIVE_ROMM=1 DECKHAND_LIVE_ROMM_ROM=<a game's id> cargo test -- --ignored a_real_romm
/// ```
///
/// — using the pairing in Deckhand's own settings, so the server's address is in no file here. It
/// registers its own devices and keeps to a slot and a state name no client uses, so nothing it does
/// meets a real save, and it removes its saves, its state and its devices when it is done. The sync sessions it opened stay in RomM's
/// history, as any client's do.
mod contract {
    use super::*;

    const SLOT: &str = "deckhand-contract-check";

    /// The state the conversation sends up: a name no game's states have.
    const STATE: &str = "deckhand-contract-check probe.state";

    /// A RomM to talk to, plain HTTP: what this checks is the server, not Deckhand's client.
    struct Api {
        http: reqwest::Client,
        host: String,
        token: String,
        rom: i64,
        /// A real server's times are its own; a pause keeps two writes from landing in one second.
        live: bool,
        devices: Mutex<Vec<String>>,
    }

    impl Api {
        async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
            let mut request = self.http.request(method, format!("{}{path}", self.host)).bearer_auth(&self.token);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.expect("the server to answer");
            let status = response.status().as_u16();
            (status, response.json().await.unwrap_or(Value::Null))
        }

        async fn device(&self, name: &str) -> String {
            let body = json!({ "name": name, "client": SLOT, "allow_existing": false, "allow_duplicate": true });
            let (_, answer) = self.call(reqwest::Method::POST, "/api/devices", Some(body)).await;
            let id = answer["device_id"].as_str().expect("a new device's id").to_string();
            self.devices.lock().unwrap().push(id.clone());
            id
        }

        async fn push(&self, device: &str, bytes: &[u8], more: &str) -> (u16, Value) {
            let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("probe.srm");
            let form = reqwest::multipart::Form::new().part("saveFile", part);
            let url = format!("{}/api/saves?rom_id={}&slot={SLOT}&emulator=probe&device_id={device}{more}", self.host, self.rom);
            let response = self.http.post(url).bearer_auth(&self.token).multipart(form).send().await.expect("the server to answer");
            (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
        }

        /// Send a state up under `name`, as `emulator`, with a picture or without.
        async fn state(&self, name: &str, bytes: &[u8], emulator: &str, picture: Option<&[u8]>) -> (u16, Value) {
            let mut form = reqwest::multipart::Form::new().part("stateFile", reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(name.to_string()));
            if let Some(picture) = picture {
                let part = reqwest::multipart::Part::bytes(picture.to_vec()).file_name(format!("{name}.png")).mime_str("image/png").expect("a type");
                form = form.part("screenshotFile", part);
            }
            let url = format!("{}/api/states?rom_id={}&emulator={emulator}", self.host, self.rom);
            let response = self.http.post(url).bearer_auth(&self.token).multipart(form).send().await.expect("the server to answer");
            (response.status().as_u16(), response.json().await.unwrap_or(Value::Null))
        }

        /// The bytes at a path the server gave — a state's, its picture's.
        async fn bytes(&self, path: &str) -> Vec<u8> {
            let url = format!("{}/{}", self.host, path.trim_start_matches('/'));
            let response = self.http.get(url).bearer_auth(&self.token).send().await.expect("the server to answer");
            response.bytes().await.map(|b| b.to_vec()).unwrap_or_default()
        }

        /// The game's states sent up under `name`.
        async fn states_named(&self, name: &str) -> Vec<Value> {
            let (_, states) = self.call(reqwest::Method::GET, &format!("/api/states?rom_id={}", self.rom), None).await;
            states.as_array().into_iter().flatten().filter(|s| s["file_name"] == name).cloned().collect()
        }

        async fn replace(&self, save_id: i64, bytes: &[u8]) -> u16 {
            let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("probe.srm");
            let form = reqwest::multipart::Form::new().part("saveFile", part);
            let url = format!("{}/api/saves/{save_id}", self.host);
            self.http.put(url).bearer_auth(&self.token).multipart(form).send().await.expect("the server to answer").status().as_u16()
        }

        /// Negotiate `listed` for `device`, close the session, and answer the actions about this slot
        /// and about saves with no slot, with the version each names.
        async fn negotiate(&self, device: &str, listed: Vec<Value>, names: &HashMap<i64, &str>) -> String {
            let body = json!({ "device_id": device, "rom_ids": [self.rom], "saves": listed });
            let (status, answer) = self.call(reqwest::Method::POST, "/api/sync/negotiate", Some(body)).await;
            if status != 200 {
                let disabled = answer["detail"].as_str().is_some_and(|d| d.contains("disabled"));
                return format!("{status}{}", if disabled { " (disabled)" } else { "" });
            }
            let session = answer["session_id"].as_i64().unwrap_or_default();
            self.call(reqwest::Method::POST, &format!("/api/sync/sessions/{session}/complete"), Some(json!({}))).await;
            let ops = answer["operations"].as_array().cloned().unwrap_or_default();
            let ours = ops.iter().filter(|op| op["rom_id"].as_i64() == Some(self.rom) && (op["slot"] == SLOT || op["slot"].is_null()));
            let said: Vec<String> = ours
                .map(|op| {
                    let action = op["action"].as_str().unwrap_or_default();
                    let version = op["save_id"].as_i64().and_then(|id| names.get(&id)).map_or(String::new(), |name| format!(" {name}"));
                    let untracked = if op["reason"].as_str().unwrap_or_default().to_lowercase().contains("untracked") { " (untracked)" } else { "" };
                    format!("{action}{version}{untracked}")
                })
                .collect();
            said.join(", ")
        }

        async fn pause(&self) {
            if self.live {
                tokio::time::sleep(Duration::from_millis(1200)).await;
            }
        }

        /// Remove what the conversation made, a save at a time: one the server no longer has would
        /// refuse a batch.
        async fn tidy(&self) {
            let (_, saves) = self.call(reqwest::Method::GET, &format!("/api/saves?rom_id={}&slot={SLOT}", self.rom), None).await;
            for id in saves.as_array().into_iter().flatten().filter_map(|s| s["id"].as_i64()) {
                self.call(reqwest::Method::POST, "/api/saves/delete", Some(json!({ "saves": [id] }))).await;
            }
            for id in self.states_named(STATE).await.iter().filter_map(|s| s["id"].as_i64()) {
                self.call(reqwest::Method::POST, "/api/states/delete", Some(json!({ "states": [id] }))).await;
            }
            let devices = std::mem::take(&mut *self.devices.lock().unwrap());
            for device in devices {
                self.call(reqwest::Method::DELETE, &format!("/api/devices/{device}"), None).await;
            }
        }
    }

    fn at(text: &str) -> SystemTime {
        parse_timestamp(text).expect("a time the server wrote")
    }

    /// A device's save as a negotiation lists it.
    fn listed(bytes: &[u8], time: SystemTime, rom: i64, slot: Option<&str>) -> Value {
        json!({
            "rom_id": rom, "file_name": "probe.srm", "slot": slot, "emulator": "probe",
            "content_hash": hash::of_upload(bytes), "updated_at": format_timestamp_precise(time),
            "file_size_bytes": bytes.len(),
        })
    }

    /// What a real RomM 5.4 answered at each step of [`conversation`].
    const SEEN: &[&str] = &[
        "A pushes v1: 200, its name stamped",
        "A, the same content: no_op v1",
        "A, changed since it had v1: upload v1",
        "A, changed but dated before it had v1: no_op v1",
        "B, holding nothing: download v1",
        "B fetches v1: 200",
        "B pushes v2: 200, a new version",
        "A, unchanged: download v2",
        "A, changed but before v2: download v2",
        "A pushes without having had v2: 409",
        "A, a save with no slot: upload, download v2",
        "C untracks v2: 200",
        "C, changed: no_op v2 (untracked)",
        "D, never had it, older: download v2",
        "D, never had it, newer: upload v2",
        "D, never had it, the same content: no_op v2",
        "D, having agreed on v2's content, a different save dated just before v2: no_op v2",
        "E, never had it, exactly v2's stored time: conflict v2",
        "F, never had it, v2's time as its push answered: upload v2",
        "B pushes v2's content again: 200, v2",
        "v2 replaced where it is: 200",
        "B, unchanged since it had v2: download v2",
        "B, changed after it had v2: conflict v2",
        "D's sync turned off: 200",
        "D, its sync off: 400 (disabled)",
        "an unknown device: 404",
        "a session completed: 200 COMPLETED",
        "completed again: 400",
        "a session left open by a newer negotiation completes: 200",
        "A overwrites without having had v2: 200, a new version",
        "A pushes in a session, keeping one: 200, 1 done, 1 left",
        "a state sent up: 200, a new state with its picture",
        "the same name from another core: 200, the same state, its content, picture, core and time replaced",
        "the same name without a picture: 200, the picture kept",
        "the game's states of that name: 1",
    ];

    /// The conversation, as said: each step and what the server answered.
    async fn conversation(api: &Api) -> Vec<String> {
        use reqwest::Method;
        let rom = api.rom;
        let mut said = Vec::new();
        let mut names: HashMap<i64, &str> = HashMap::new();
        let (a, b, c, d) = (api.device("probe-a").await, api.device("probe-b").await, api.device("probe-c").await, api.device("probe-d").await);
        let (e, f) = (api.device("probe-e").await, api.device("probe-f").await);
        let secs = Duration::from_secs;

        let (status, v1) = api.push(&a, b"v1", "").await;
        let stamped = v1["file_name"].as_str().is_some_and(|n| n.starts_with("probe [") && n.ends_with("].srm") && n.len() == "probe [2026-10-19_09-20-18].srm".len());
        said.push(format!("A pushes v1: {status}{}", if stamped { ", its name stamped" } else { "" }));
        let v1_id = v1["id"].as_i64().unwrap_or_default();
        names.insert(v1_id, "v1");
        let v1_at = at(v1["updated_at"].as_str().unwrap_or_default());
        let mine = |bytes: &[u8], time| vec![listed(bytes, time, rom, Some(SLOT))];
        said.push(format!("A, the same content: {}", api.negotiate(&a, mine(b"v1", v1_at), &names).await));
        said.push(format!("A, changed since it had v1: {}", api.negotiate(&a, mine(b"v1 changed", v1_at + secs(30)), &names).await));
        said.push(format!("A, changed but dated before it had v1: {}", api.negotiate(&a, mine(b"v1 changed", v1_at - secs(30)), &names).await));
        said.push(format!("B, holding nothing: {}", api.negotiate(&b, Vec::new(), &names).await));
        let (status, _) = api.call(Method::GET, &format!("/api/saves/{v1_id}/content?device_id={b}"), None).await;
        said.push(format!("B fetches v1: {status}"));

        api.pause().await;
        let (status, v2) = api.push(&b, b"v2 from B", "").await;
        let v2_id = v2["id"].as_i64().unwrap_or_default();
        said.push(format!("B pushes v2: {status}{}", if v2_id != v1_id { ", a new version" } else { "" }));
        names.insert(v2_id, "v2");
        let v2_text = v2["updated_at"].as_str().unwrap_or_default().to_string();
        let v2_at = at(&v2_text);
        said.push(format!("A, unchanged: {}", api.negotiate(&a, mine(b"v1", v1_at), &names).await));
        said.push(format!("A, changed but before v2: {}", api.negotiate(&a, mine(b"v1 by A", v1_at - Duration::from_millis(500)), &names).await));
        let (status, _) = api.push(&a, b"v3 from A", "").await;
        said.push(format!("A pushes without having had v2: {status}"));
        said.push(format!("A, a save with no slot: {}", api.negotiate(&a, vec![listed(b"loose", v2_at, rom, None)], &names).await));

        let (status, _) = api.call(Method::POST, &format!("/api/saves/{v2_id}/untrack"), Some(json!({ "device_id": c }))).await;
        said.push(format!("C untracks v2: {status}"));
        said.push(format!("C, changed: {}", api.negotiate(&c, mine(b"c", v2_at + secs(60)), &names).await));

        said.push(format!("D, never had it, older: {}", api.negotiate(&d, mine(b"d older", v2_at - secs(60)), &names).await));
        said.push(format!("D, never had it, newer: {}", api.negotiate(&d, mine(b"d newer", v2_at + secs(60)), &names).await));
        said.push(format!("D, never had it, the same content: {}", api.negotiate(&d, mine(b"v2 from B", v2_at - secs(600)), &names).await));
        let (_, versions) = api.call(Method::GET, &format!("/api/saves?rom_id={rom}&slot={SLOT}"), None).await;
        let stored = versions.as_array().into_iter().flatten().find(|v| v["id"].as_i64() == Some(v2_id)).and_then(|v| v["updated_at"].as_str()).unwrap_or_default().to_string();
        let just_before = at(&stored) - Duration::from_millis(500);
        said.push(format!("D, having agreed on v2's content, a different save dated just before v2: {}", api.negotiate(&d, mine(b"d differs", just_before), &names).await));
        let mut same_time = listed(b"e same time", v2_at, rom, Some(SLOT));
        same_time["updated_at"] = json!(stored);
        said.push(format!("E, never had it, exactly v2's stored time: {}", api.negotiate(&e, vec![same_time], &names).await));
        let mut reported = listed(b"f same time", v2_at, rom, Some(SLOT));
        reported["updated_at"] = json!(v2_text);
        said.push(format!("F, never had it, v2's time as its push answered: {}", api.negotiate(&f, vec![reported], &names).await));

        let (status, again) = api.push(&b, b"v2 from B", "").await;
        said.push(format!("B pushes v2's content again: {status}{}", if again["id"].as_i64() == Some(v2_id) { ", v2" } else { ", a new version" }));
        api.pause().await;
        said.push(format!("v2 replaced where it is: {}", api.replace(v2_id, b"v2 replaced in place").await));
        said.push(format!("B, unchanged since it had v2: {}", api.negotiate(&b, mine(b"v2 from B", v2_at), &names).await));
        said.push(format!("B, changed after it had v2: {}", api.negotiate(&b, mine(b"v2 by B", v2_at + Duration::from_millis(500)), &names).await));

        let (status, _) = api.call(Method::PUT, &format!("/api/devices/{d}"), Some(json!({ "sync_enabled": false }))).await;
        said.push(format!("D's sync turned off: {status}"));
        said.push(format!("D, its sync off: {}", api.negotiate(&d, Vec::new(), &names).await));
        said.push(format!("an unknown device: {}", api.negotiate("no-such-device-here", Vec::new(), &names).await));

        let open = |device: String| async move {
            let body = json!({ "device_id": device, "rom_ids": [rom], "saves": [] });
            api.call(Method::POST, "/api/sync/negotiate", Some(body)).await.1["session_id"].as_i64().unwrap_or_default()
        };
        let session = open(a.clone()).await;
        let close = |session: i64| async move { api.call(Method::POST, &format!("/api/sync/sessions/{session}/complete"), Some(json!({}))).await };
        let (status, closed) = close(session).await;
        said.push(format!("a session completed: {status} {}", closed["session"]["status"].as_str().unwrap_or_default()));
        said.push(format!("completed again: {}", close(session).await.0));
        let (older, newer) = (open(a.clone()).await, open(a.clone()).await);
        said.push(format!("a session left open by a newer negotiation completes: {}", close(older).await.0));
        close(newer).await;

        let (status, v3) = api.push(&a, b"v3 from A", "&overwrite=true").await;
        said.push(format!("A overwrites without having had v2: {status}{}", if v3["id"].as_i64().is_some_and(|id| id != v2_id) { ", a new version" } else { "" }));
        let session = open(a.clone()).await;
        api.pause().await;
        let (status, _) = api.push(&a, b"v4 from A", &format!("&session_id={session}&autocleanup=true&autocleanup_limit=1")).await;
        let (_, record) = api.call(Method::GET, &format!("/api/sync/sessions/{session}"), None).await;
        let (_, left) = api.call(Method::GET, &format!("/api/saves?rom_id={rom}&slot={SLOT}"), None).await;
        said.push(format!(
            "A pushes in a session, keeping one: {status}, {} done, {} left",
            record["operations_completed"].as_i64().unwrap_or_default(),
            left.as_array().map_or(0, Vec::len)
        ));
        close(session).await;

        // Save states: one per name, replaced where it is by a second upload under it.
        let (status, first) = api.state(STATE, b"state one", "probe-a", Some(b"\x89PNG one")).await;
        let pictured = match first["screenshot"]["download_path"].as_str() {
            Some(path) => api.bytes(path).await == b"\x89PNG one",
            None => false,
        };
        said.push(format!("a state sent up: {status}, a new state{}", if pictured { " with its picture" } else { "" }));
        api.pause().await;
        let (status, second) = api.state(STATE, b"state two", "probe-b", Some(b"\x89PNG two")).await;
        let mut replaced = Vec::new();
        if second["id"] == first["id"] {
            replaced.push("the same state");
        }
        if api.bytes(&format!("/api/states/{}/content", second["id"])).await == b"state two" {
            replaced.push("its content");
        }
        if let Some(path) = second["screenshot"]["download_path"].as_str()
            && api.bytes(path).await == b"\x89PNG two"
        {
            replaced.push("picture");
        }
        if second["emulator"] == "probe-b" {
            replaced.push("core");
        }
        let time = |state: &Value| state["updated_at"].as_str().and_then(parse_timestamp);
        if time(&second).zip(time(&first)).is_some_and(|(two, one)| two > one) {
            replaced.push("time");
        }
        let replaced = match replaced.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last} replaced", rest.join(", ")),
            Some((only, _)) => format!("{only} replaced"),
            None => "nothing replaced".into(),
        };
        said.push(format!("the same name from another core: {status}, {replaced}"));
        api.pause().await;
        let (status, third) = api.state(STATE, b"state three", "probe-b", None).await;
        let kept = match third["screenshot"]["download_path"].as_str() {
            Some(path) => api.bytes(path).await == b"\x89PNG two",
            None => false,
        };
        said.push(format!("the same name without a picture: {status}, the picture {}", if kept { "kept" } else { "gone" }));
        said.push(format!("the game's states of that name: {}", api.states_named(STATE).await.len()));
        said
    }

    /// Run the conversation, tidy up whatever happens, and compare.
    async fn check(api: Api) {
        let said = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(conversation(&api))).await;
        api.tidy().await;
        let said = said.unwrap_or_else(|_| vec!["the conversation broke off".into()]);
        for (step, (heard, seen)) in said.iter().zip(SEEN).enumerate() {
            assert_eq!(heard, seen, "step {}", step + 1);
        }
        assert_eq!(said.len(), SEEN.len(), "{said:#?}");
    }

    #[tokio::test]
    async fn this_server_answers_as_romm_was_seen_to() {
        let server = FakeRomm::start().await;
        let api = Api { http: reqwest::Client::new(), host: server.host.clone(), token: "t".into(), rom: 1, live: false, devices: Mutex::default() };
        check(api).await;
    }

    #[tokio::test]
    #[ignore = "talks to a real RomM — see the module's header"]
    async fn a_real_romm_still_answers_as_it_was_seen_to() {
        let (Ok(_), Ok(rom)) = (std::env::var("DECKHAND_LIVE_ROMM"), std::env::var("DECKHAND_LIVE_ROMM_ROM")) else {
            eprintln!("DECKHAND_LIVE_ROMM and DECKHAND_LIVE_ROMM_ROM are not set; nothing to check");
            return;
        };
        // The pairing as the app keeps it. A test build's own settings live in a scratch folder (see
        // `crate::config::test_root`), so the file is read where the app itself would read it.
        let file = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
            .expect("a config folder")
            .join("deckhand-rust/settings.json");
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(file).expect("Deckhand's settings")).expect("settings as JSON");
        let connection = &settings["connection"];
        let insecure = connection["insecure"].as_bool().unwrap_or(false);
        let http = reqwest::Client::builder().danger_accept_invalid_certs(insecure).build().unwrap();
        let api = Api {
            http,
            host: connection["host"].as_str().expect("Deckhand paired with a RomM").trim_end_matches('/').to_string(),
            token: connection["token"].as_str().expect("a pairing token").to_string(),
            rom: rom.parse().expect("DECKHAND_LIVE_ROMM_ROM to be a game's id"),
            live: true,
            devices: Mutex::default(),
        };
        check(api).await;
    }
}

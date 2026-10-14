//! A RomM that keeps state, for testing the save sync against: RomM 5.3.1's sync protocol ported from
//! its own source (`endpoints/sync.py`, `handler/sync/comparison.py`, `endpoints/saves.py`) onto a
//! real socket.
//!
//! [`crate::stub_server`] answers canned responses, which tests one request at a time. A sync is a
//! conversation — negotiate, transfer, acknowledge, complete — whose answers depend on what came
//! before, and on what *other* devices did in between; testing it means a server that remembers.
//! So this one holds rows, device records and sessions the way RomM does, decides the way RomM
//! decides, and refuses what RomM refuses: a push to a slot whose newest version the pushing device
//! has not synced (409), a negotiation from a device whose sync is off.
//!
//! Its clock is its own and only moves when a test or a write moves it, a second at a time, so a test
//! can say exactly how a device's file times sit against the server's.

#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::sync::{format_timestamp_precise, hash, parse_timestamp};

/// One version of one save.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: i64,
    pub rom_id: i64,
    pub file_name: String,
    pub slot: Option<String>,
    pub emulator: Option<String>,
    pub content_hash: String,
    pub updated_at: SystemTime,
    pub bytes: Vec<u8>,
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Synced {
    at: SystemTime,
    untracked: bool,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: i64,
    pub device_id: String,
    pub status: &'static str,
    pub completed: i64,
    pub failed: i64,
}

#[derive(Debug, Default)]
pub struct State {
    pub now: Option<SystemTime>,
    pub rows: Vec<Row>,
    syncs: HashMap<(String, i64), Synced>,
    pub sessions: Vec<SessionRecord>,
    /// Device id → sync enabled.
    pub devices: HashMap<String, bool>,
    /// Play sessions taken: (rom id, start time, sync session id).
    pub plays: Vec<(Option<i64>, String, Option<i64>)>,
    /// Every request line, in order.
    pub log: Vec<String>,
    /// Every operation a negotiation answered, in order: (action, reason).
    pub answered: Vec<(String, String)>,
    /// When false, `/api/sync/*` is a route this RomM does not have.
    pub no_sessions: bool,
    /// Pushed by "another device" just before the next push from anyone — a race.
    pub race: Option<(i64, String, Vec<u8>)>,
    /// How many negotiations are answered before the rest fail — a server that goes away part way
    /// through a library too big for one of them.
    pub negotiations_before_failing: Option<usize>,
    next_id: i64,
}

impl State {
    fn now(&self) -> SystemTime {
        self.now.unwrap_or(SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000))
    }

    /// Writes take a second each, as RomM's names for them (`[2026-07-01_10-00-00]`) need.
    fn tick(&mut self) -> SystemTime {
        let now = self.now() + Duration::from_secs(1);
        self.now = Some(now);
        now
    }

    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn sync(&mut self, device: &str, save_id: i64, at: SystemTime) {
        let untracked = false;
        self.syncs.insert((device.to_string(), save_id), Synced { at, untracked });
    }

    /// The newest version of every non-null slot in `scope`.
    fn current(&self, scope: Option<&HashSet<i64>>) -> HashMap<(i64, String), &Row> {
        let mut map: HashMap<(i64, String), &Row> = HashMap::new();
        for row in &self.rows {
            let Some(slot) = &row.slot else { continue };
            if scope.is_some_and(|s| !s.contains(&row.rom_id)) {
                continue;
            }
            let key = (row.rom_id, slot.clone());
            if map.get(&key).is_none_or(|c| row.updated_at > c.updated_at) {
                map.insert(key, row);
            }
        }
        map
    }
}

/// A running fake RomM.
pub struct FakeRomm {
    pub host: String,
    pub state: Arc<Mutex<State>>,
}

impl FakeRomm {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("the bound address");
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { break };
                let state = shared.clone();
                tokio::spawn(async move { serve(socket, state).await });
            }
        });
        FakeRomm { host: format!("http://{addr}"), state }
    }

    /// A device RomM knows, with its sync on.
    pub fn device(&self, id: &str) {
        self.state.lock().unwrap().devices.insert(id.to_string(), true);
    }

    pub fn now(&self) -> SystemTime {
        self.state.lock().unwrap().now()
    }

    pub fn advance(&self, by: Duration) {
        let mut state = self.state.lock().unwrap();
        let now = state.now() + by;
        state.now = Some(now);
    }

    /// A version pushed by another client, as RomM's own player would — straight into the slot.
    pub fn push_from(&self, device: Option<&str>, rom_id: i64, slot: &str, name: &str, bytes: &[u8]) -> i64 {
        let mut state = self.state.lock().unwrap();
        let at = state.tick();
        let id = state.id();
        state.rows.push(Row {
            id,
            rom_id,
            file_name: tagged(name, at),
            slot: Some(slot.into()),
            emulator: Some("snes9x".into()),
            content_hash: hash::of_upload(bytes),
            updated_at: at,
            bytes: bytes.to_vec(),
            origin: device.map(str::to_string),
        });
        if let Some(device) = device {
            state.sync(device, id, at);
        }
        id
    }

    /// RomM's own player saving over a version in place: same id, new bytes, new time.
    pub fn rewrite(&self, save_id: i64, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap();
        let at = state.tick();
        let row = state.rows.iter_mut().find(|r| r.id == save_id).expect("a row to rewrite");
        row.bytes = bytes.to_vec();
        row.content_hash = hash::of_upload(bytes);
        row.updated_at = at;
    }

    /// The player deleting every version of a game's saves on RomM.
    pub fn delete_all(&self, rom_id: i64) {
        self.state.lock().unwrap().rows.retain(|r| r.rom_id != rom_id);
    }

    pub fn untrack(&self, device: &str, save_id: i64) {
        let mut state = self.state.lock().unwrap();
        let at = state.now();
        state.syncs.entry((device.into(), save_id)).or_insert(Synced { at, untracked: true }).untracked = true;
    }

    pub fn rows(&self, rom_id: i64) -> Vec<Row> {
        let mut rows: Vec<Row> = self.state.lock().unwrap().rows.iter().filter(|r| r.rom_id == rom_id).cloned().collect();
        rows.sort_by_key(|r| r.updated_at);
        rows
    }

    /// The newest version in a slot.
    pub fn newest(&self, rom_id: i64, slot: &str) -> Option<Row> {
        self.rows(rom_id).into_iter().filter(|r| r.slot.as_deref() == Some(slot)).max_by_key(|r| r.updated_at)
    }

    /// Whether RomM records `device` as holding `save_id`'s current version.
    pub fn holds(&self, device: &str, save_id: i64) -> bool {
        let state = self.state.lock().unwrap();
        let row = state.rows.iter().find(|r| r.id == save_id);
        match (row, state.syncs.get(&(device.to_string(), save_id))) {
            (Some(row), Some(synced)) => synced.at >= row.updated_at,
            _ => false,
        }
    }

    /// The actions RomM's negotiations answered, in order.
    pub fn answered(&self) -> Vec<String> {
        self.state.lock().unwrap().answered.iter().map(|(action, _)| action.clone()).collect()
    }

    pub fn requests(&self, prefix: &str) -> usize {
        self.state.lock().unwrap().log.iter().filter(|l| l.starts_with(prefix)).count()
    }
}

/// RomM's datetime tag on a slotted upload: `Game [2026-07-01_10-00-00].srm`.
fn tagged(name: &str, at: SystemTime) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    let stamp = format_timestamp_precise(at);
    let stamp = format!("{}_{}", &stamp[..10], stamp[11..19].replace(':', "-"));
    format!("{stem} [{stamp}]{ext}")
}

fn row_json(row: &Row) -> Value {
    json!({
        "id": row.id,
        "rom_id": row.rom_id,
        "file_name": row.file_name,
        "slot": row.slot,
        "emulator": row.emulator,
        "content_hash": row.content_hash,
        "updated_at": iso(row.updated_at),
        "file_size_bytes": row.bytes.len(),
        "download_path": format!("/api/raw/assets/saves/{}", row.id),
        "device_syncs": [],
    })
}

/// RomM's own format: microseconds and an offset.
fn iso(t: SystemTime) -> String {
    format_timestamp_precise(t).replace('Z', "+00:00")
}

/// `handler/sync/comparison.py`'s `compare_save_state`, as 5.3.1 has it.
fn compare(
    client_hash: Option<&str>,
    client_at: SystemTime,
    server_hash: &str,
    server_at: SystemTime,
    synced: Option<SystemTime>,
) -> (&'static str, &'static str) {
    if client_hash == Some(server_hash) {
        return ("no_op", "Content is identical");
    }
    if let Some(synced) = synced {
        return match (client_at > synced, server_at > synced) {
            (true, true) => ("conflict", "Both sides changed since last sync"),
            (true, false) => ("upload", "Client save is newer than last sync"),
            (false, true) => ("download", "Server save is newer than last sync"),
            (false, false) => ("no_op", "No changes since last sync"),
        };
    }
    if client_at > server_at {
        ("upload", "Client save is newer (no sync history)")
    } else if server_at > client_at {
        ("download", "Server save is newer (no sync history)")
    } else {
        ("conflict", "Same timestamp but different content")
    }
}

fn negotiate(state: &mut State, body: &Value) -> (u16, Value) {
    if let Some(left) = &mut state.negotiations_before_failing {
        match left.checked_sub(1) {
            Some(fewer) => *left = fewer,
            None => return (500, json!({ "detail": "Internal Server Error" })),
        }
    }
    let device = body["device_id"].as_str().unwrap_or_default().to_string();
    match state.devices.get(&device) {
        None => return (404, json!({ "detail": format!("Device with ID {device} not found") })),
        Some(false) => return (400, json!({ "detail": "Sync is disabled for this device" })),
        Some(true) => {}
    }
    for session in state.sessions.iter_mut().filter(|s| s.device_id == device && s.status == "in_progress") {
        session.status = "cancelled";
    }
    let session_id = state.id();
    state.sessions.push(SessionRecord { id: session_id, device_id: device.clone(), status: "in_progress", completed: 0, failed: 0 });

    let saves = body["saves"].as_array().cloned().unwrap_or_default();
    let scope: Option<HashSet<i64>> = body["rom_ids"].as_array().map(|ids| {
        ids.iter().filter_map(Value::as_i64).chain(saves.iter().filter_map(|s| s["rom_id"].as_i64())).collect()
    });
    let current: HashMap<(i64, String), Row> =
        state.current(scope.as_ref()).into_iter().map(|(k, v)| (k, v.clone())).collect();
    let mut ops = Vec::new();
    let mut matched = HashSet::new();
    for save in &saves {
        let rom_id = save["rom_id"].as_i64().unwrap_or_default();
        let slot = save["slot"].as_str().map(str::to_string);
        let found = slot.as_ref().and_then(|slot| current.get(&(rom_id, slot.clone())));
        let Some(row) = found else {
            ops.push(json!({ "action": "upload", "rom_id": rom_id, "save_id": null, "file_name": save["file_name"], "slot": slot,
                             "emulator": save["emulator"], "reason": "Save exists on client but not on server" }));
            continue;
        };
        matched.insert(row.id);
        let synced = state.syncs.get(&(device.clone(), row.id)).copied();
        if synced.is_some_and(|s| s.untracked) {
            ops.push(op_json("no_op", row, "Save is untracked on this device"));
            continue;
        }
        let client_at = save["updated_at"].as_str().and_then(parse_timestamp).unwrap_or(SystemTime::UNIX_EPOCH);
        let (action, reason) = compare(save["content_hash"].as_str(), client_at, &row.content_hash, row.updated_at, synced.map(|s| s.at));
        ops.push(op_json(action, row, reason));
    }
    for row in current.values().filter(|r| !matched.contains(&r.id)) {
        match state.syncs.get(&(device.clone(), row.id)) {
            Some(synced) if synced.untracked => {}
            Some(synced) if row.updated_at <= synced.at => {}
            Some(_) => ops.push(op_json("download", row, "Server save updated since last sync, not present on client")),
            None => ops.push(op_json("download", row, "Save exists on server but not on client")),
        }
    }
    for op in &ops {
        state.answered.push((op["action"].as_str().unwrap_or_default().into(), op["reason"].as_str().unwrap_or_default().into()));
    }
    let count = |a: &str| ops.iter().filter(|o| o["action"] == a).count();
    let body = json!({
        "session_id": session_id,
        "total_upload": count("upload"),
        "total_download": count("download"),
        "total_conflict": count("conflict"),
        "total_no_op": count("no_op"),
        "operations": ops,
    });
    (200, body)
}

fn op_json(action: &str, row: &Row, reason: &str) -> Value {
    json!({
        "action": action, "rom_id": row.rom_id, "save_id": row.id, "file_name": row.file_name, "slot": row.slot,
        "emulator": row.emulator, "reason": reason, "server_updated_at": iso(row.updated_at),
        "server_content_hash": row.content_hash,
    })
}

/// `endpoints/saves.py`'s `add_save`, as 5.3.1 has it.
fn upload(state: &mut State, query: &HashMap<String, String>, name: &str, bytes: Vec<u8>) -> (u16, Value) {
    if let Some((rom_id, slot, race)) = state.race.take() {
        let at = state.tick();
        let id = state.id();
        let content_hash = hash::of_upload(&race);
        state.rows.push(Row { id, rom_id, file_name: tagged(name, at), slot: Some(slot), emulator: None, content_hash, updated_at: at, bytes: race, origin: Some("racer".into()) });
    }
    let rom_id: i64 = query.get("rom_id").and_then(|r| r.parse().ok()).unwrap_or_default();
    let slot = query.get("slot").cloned();
    let device = query.get("device_id").cloned();
    let overwrite = query.get("overwrite").is_some_and(|v| v == "true");
    if let Some(device) = &device
        && !state.devices.contains_key(device)
    {
        return (404, json!({ "detail": format!("Device with ID {device} not found") }));
    }
    if let (Some(device), Some(slot), false) = (&device, &slot, overwrite) {
        let latest = state.rows.iter().filter(|r| r.rom_id == rom_id && r.slot.as_ref() == Some(slot)).max_by_key(|r| r.updated_at);
        if let Some(latest) = latest {
            let synced = state.syncs.get(&(device.clone(), latest.id));
            if synced.is_none_or(|s| s.at < latest.updated_at) {
                return (409, json!({ "detail": "Slot has a newer save since your last sync" }));
            }
        }
    }
    let content_hash = hash::of_upload(&bytes);
    let keep = query.get("autocleanup").filter(|v| *v == "true").map(|_| {
        query.get("autocleanup_limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(10).max(1)
    });
    if let (Some(slot), false) = (&slot, overwrite)
        && let Some(existing) = state.rows.iter().find(|r| r.rom_id == rom_id && r.slot.as_ref() == Some(slot) && r.content_hash == content_hash)
    {
        // Already there: that version comes back, nothing is stored, nobody is recorded as holding it.
        let existing = existing.clone();
        prune(state, rom_id, slot, keep);
        return (200, row_json(&existing));
    }
    let at = state.tick();
    let id = state.id();
    let file_name = if slot.is_some() { tagged(name, at) } else { name.to_string() };
    let row = Row {
        id,
        rom_id,
        file_name,
        slot: slot.clone(),
        emulator: query.get("emulator").cloned(),
        content_hash,
        updated_at: at,
        bytes,
        origin: device.clone(),
    };
    state.rows.push(row.clone());
    if let Some(device) = &device {
        state.sync(device, id, at);
    }
    if let Some(session) = query.get("session_id").and_then(|s| s.parse::<i64>().ok())
        && let Some(record) = state.sessions.iter_mut().find(|s| s.id == session)
    {
        record.completed += 1;
    }
    if let Some(slot) = &slot {
        prune(state, rom_id, slot, keep);
    }
    (200, row_json(&row))
}

fn prune(state: &mut State, rom_id: i64, slot: &str, keep: Option<usize>) {
    let Some(keep) = keep else { return };
    let mut versions: Vec<(SystemTime, i64)> =
        state.rows.iter().filter(|r| r.rom_id == rom_id && r.slot.as_deref() == Some(slot)).map(|r| (r.updated_at, r.id)).collect();
    versions.sort_by(|a, b| b.cmp(a));
    let gone: HashSet<i64> = versions.into_iter().skip(keep).map(|(_, id)| id).collect();
    state.rows.retain(|r| !gone.contains(&r.id));
}

fn complete(state: &mut State, session_id: i64, body: &Value) -> (u16, Value) {
    let Some(index) = state.sessions.iter().position(|s| s.id == session_id) else {
        return (404, json!({ "detail": format!("Sync session with ID {session_id} not found") }));
    };
    if state.sessions[index].status != "in_progress" {
        return (400, json!({ "detail": format!("Session is already {}", state.sessions[index].status) }));
    }
    let session = &mut state.sessions[index];
    session.status = "completed";
    session.completed = body["operations_completed"].as_i64().unwrap_or_default();
    session.failed = body["operations_failed"].as_i64().unwrap_or_default();
    let mut results = Vec::new();
    for (i, play) in body["play_sessions"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let key = (play["rom_id"].as_i64(), play["start_time"].as_str().unwrap_or_default().to_string());
        let duplicate = state.plays.iter().any(|(rom, start, _)| (*rom, start.clone()) == key);
        if !duplicate {
            state.plays.push((key.0, key.1, Some(session_id)));
        }
        results.push(json!({ "index": i, "status": if duplicate { "duplicate" } else { "created" } }));
    }
    (200, json!({ "session": { "id": session_id }, "play_session_ingest": { "results": results } }))
}

async fn serve(mut socket: TcpStream, state: Arc<Mutex<State>>) {
    let Some((line, headers, body)) = read(&mut socket).await else { return };
    let (status, content, bytes) = {
        let mut state = state.lock().unwrap();
        state.log.push(line.clone());
        route(&mut state, &line, &headers, body)
    };
    let (kind, payload) = match bytes {
        Some(bytes) => ("application/octet-stream", bytes),
        None => ("application/json", content.to_string().into_bytes()),
    };
    let head = format!("HTTP/1.1 {status} OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&payload).await;
    let _ = socket.flush().await;
}

/// Status, JSON, or raw bytes for a download.
fn route(state: &mut State, line: &str, headers: &str, body: Vec<u8>) -> (u16, Value, Option<Vec<u8>>) {
    let mut words = line.split_whitespace();
    let (method, target) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query: HashMap<String, String> = url::form_urlencoded_parse(query);
    let json_body = || serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if path.starts_with("/api/sync/") && state.no_sessions {
        return (404, json!({ "detail": "Not Found" }), None);
    }
    match (method, segments.as_slice()) {
        ("POST", ["api", "sync", "negotiate"]) => {
            let (status, value) = negotiate(state, &json_body());
            (status, value, None)
        }
        ("POST", ["api", "sync", "sessions", id, "complete"]) => {
            let (status, value) = complete(state, id.parse().unwrap_or_default(), &json_body());
            (status, value, None)
        }
        ("POST", ["api", "saves"]) => match multipart(headers, &body) {
            Some((name, bytes)) => {
                let (status, value) = upload(state, &query, &name, bytes);
                (status, value, None)
            }
            None => (422, json!({ "detail": "no saveFile" }), None),
        },
        ("GET", ["api", "saves"]) => {
            let rom_id: Option<i64> = query.get("rom_id").and_then(|r| r.parse().ok());
            let slot = query.get("slot");
            let rows: Vec<Value> = state
                .rows
                .iter()
                .filter(|r| rom_id.is_none_or(|id| r.rom_id == id) && slot.is_none_or(|s| r.slot.as_ref() == Some(s)))
                .map(row_json)
                .collect();
            (200, Value::Array(rows), None)
        }
        ("GET", ["api", "saves", id, "content"]) => {
            let id: i64 = id.parse().unwrap_or_default();
            let Some(row) = state.rows.iter().find(|r| r.id == id).cloned() else {
                return (404, json!({ "detail": format!("Save with ID {id} not found") }), None);
            };
            let optimistic = query.get("optimistic").is_none_or(|v| v != "false");
            if let Some(device) = query.get("device_id")
                && optimistic
            {
                state.sync(device, id, row.updated_at);
            }
            if let Some(session) = query.get("session_id").and_then(|s| s.parse::<i64>().ok())
                && let Some(record) = state.sessions.iter_mut().find(|s| s.id == session)
            {
                record.completed += 1;
            }
            (200, Value::Null, Some(row.bytes))
        }
        ("GET", ["api", "raw", "assets", "saves", id]) => {
            let id: i64 = id.parse().unwrap_or_default();
            match state.rows.iter().find(|r| r.id == id) {
                Some(row) => (200, Value::Null, Some(row.bytes.clone())),
                None => (404, json!({ "detail": "Not Found" }), None),
            }
        }
        ("POST", ["api", "saves", id, "downloaded"]) => {
            let id: i64 = id.parse().unwrap_or_default();
            let device = json_body()["device_id"].as_str().unwrap_or_default().to_string();
            let Some(at) = state.rows.iter().find(|r| r.id == id).map(|r| r.updated_at) else {
                return (404, json!({ "detail": format!("Save with ID {id} not found") }), None);
            };
            state.sync(&device, id, at);
            (200, json!({ "id": id }), None)
        }
        ("GET", ["api", "states"]) => (200, json!([]), None),
        ("GET", ["api", "devices"]) => {
            let ids: Vec<Value> = state.devices.keys().map(|id| json!({ "id": id })).collect();
            (200, Value::Array(ids), None)
        }
        ("POST", ["api", "play-sessions"]) => {
            for play in json_body()["sessions"].as_array().cloned().unwrap_or_default() {
                state.plays.push((play["rom_id"].as_i64(), play["start_time"].as_str().unwrap_or_default().to_string(), None));
            }
            (201, json!({ "results": [] }), None)
        }
        _ => (404, json!({ "detail": "Not Found" }), None),
    }
}

/// The `saveFile` part of a multipart body: its file name and bytes.
fn multipart(headers: &str, body: &[u8]) -> Option<(String, Vec<u8>)> {
    let boundary = headers.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        (name.trim().eq_ignore_ascii_case("content-type")).then_some(value)?.split("boundary=").nth(1).map(|b| b.trim().trim_matches('"').to_string())
    })?;
    let delimiter = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(at) = find(rest, &delimiter) {
        rest = &rest[at + delimiter.len()..];
        parts.push(rest);
    }
    for part in parts {
        let head_end = find(part, b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&part[..head_end]);
        if !head.contains("name=\"saveFile\"") {
            continue;
        }
        let name = head.split("filename=\"").nth(1)?.split('"').next()?.to_string();
        let content = &part[head_end + 4..];
        let end = find(content, &delimiter).map_or(content.len(), |e| e.saturating_sub(2));
        return Some((name, content[..end].to_vec()));
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One whole request: its line, its headers, its body.
async fn read(socket: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut request = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buf[..n]);
        let Some(end) = find(&request, b"\r\n\r\n") else { continue };
        let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if request.len() >= end + 4 + length {
            break;
        }
    }
    let end = find(&request, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&request[..end]).into_owned();
    let line = head.lines().next().unwrap_or_default().to_string();
    let headers = head.lines().skip(1).collect::<Vec<_>>().join("\n");
    Some((line, headers, request[end + 4..].to_vec()))
}

mod url {
    use std::collections::HashMap;

    /// `a=1&b=x%20y` into a map, percent-decoded.
    pub fn form_urlencoded_parse(query: &str) -> HashMap<String, String> {
        query
            .split('&')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.split_once('=').or(Some((p, ""))))
            .map(|(k, v)| (decode(k), decode(v)))
            .collect()
    }

    fn decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                    match hex {
                        Some(b) => {
                            out.push(b);
                            i += 2;
                        }
                        None => out.push(b'%'),
                    }
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

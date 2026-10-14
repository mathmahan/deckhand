//! This install as a RomM 5 device.
//!
//! RomM 5 keeps, per device, which save each slot was last synced at, and refuses a push to the
//! autosave slot (409) until the pushing device has acknowledged the slot's newest save — so a
//! device that pulls without saying so stalls its own pushes indefinitely, which is how the Kotlin
//! app found out. Registering, sending the id with every upload, and acknowledging every pull is
//! what keeps a push from waiting on RomM.
//!
//! A port of the Kotlin `ensureDevice`: the stored id is checked against RomM's own list once per
//! process, and only an answer that leaves it out re-registers — a failed check is not evidence the
//! device is gone, and registering on every network blip would mint a duplicate each time.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Connection;
use crate::romm::RommClient;

static CHECKED: AtomicBool = AtomicBool::new(false);

/// Make sure `conn` carries a device id RomM knows, registering if it has none or RomM has forgotten
/// it. Answers `Some` when the id changed, so the caller can store it. A server without devices — an
/// older RomM — leaves the connection as it is.
pub async fn ensure(client: &RommClient, conn: &mut Connection) -> Option<String> {
    if let Some(stored) = &conn.device_id {
        if CHECKED.load(Ordering::Relaxed) {
            return None;
        }
        match client.device_ids(conn).await {
            Ok(known) if known.contains(stored) => {
                CHECKED.store(true, Ordering::Relaxed);
                return None;
            }
            Ok(_) => {} // forgotten: register again below
            Err(_) => return None,
        }
    }
    let id = client.register_device(conn, &name(), platform(), &hostname()).await.ok()?;
    CHECKED.store(true, Ordering::Relaxed);
    conn.device_id = Some(id.clone());
    Some(id)
}

/// Check the stored id against RomM's list again on the next [`ensure`] — RomM said it does not know
/// it, and the next sync should register rather than fail the same way.
pub fn forget() {
    CHECKED.store(false, Ordering::Relaxed);
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|h| h.trim().to_string())
        .unwrap_or_default()
}

/// "Steam Deck" on one, by its board vendor; "Windows" or "Linux" otherwise.
pub fn platform() -> &'static str {
    if cfg!(windows) {
        return "Windows";
    }
    let vendor = std::fs::read_to_string("/sys/devices/virtual/dmi/id/board_vendor").unwrap_or_default();
    if vendor.trim() == "Valve" { "Steam Deck" } else { "Linux" }
}

/// This install, to RomM and in its list of devices: `Deckhand (steamdeck)`.
pub fn name() -> String {
    match hostname() {
        h if h.is_empty() => "Deckhand".into(),
        h => format!("Deckhand ({h})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_device_is_registered_once_and_again_only_when_romm_forgets_it() {
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("GET /api/devices", 200, r#"[{"id":"other"}]"#.into()),
            ("POST /api/devices", 201, r#"{"device_id":"d-1","name":"Deckhand","created_at":"x"}"#.into()),
        ])
        .await;
        let client = RommClient::new().unwrap();
        let mut conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        CHECKED.store(false, Ordering::Relaxed);
        assert_eq!(ensure(&client, &mut conn).await.as_deref(), Some("d-1"), "none stored: registered");
        assert_eq!(conn.device_id.as_deref(), Some("d-1"));

        // Stored, and RomM's list leaves it out: registered again.
        CHECKED.store(false, Ordering::Relaxed);
        conn.device_id = Some("gone".into());
        assert_eq!(ensure(&client, &mut conn).await.as_deref(), Some("d-1"));
        let posts = requests.lock().unwrap().iter().filter(|l| l.starts_with("POST /api/devices")).count();
        assert_eq!(posts, 2);

        // Unreachable: the stored id stands.
        let mut offline = Connection { host: "http://127.0.0.1:1".into(), device_id: Some("kept".into()), ..conn };
        CHECKED.store(false, Ordering::Relaxed);
        assert_eq!(ensure(&client, &mut offline).await, None);
        assert_eq!(offline.device_id.as_deref(), Some("kept"));
    }
}

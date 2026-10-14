//! Pairing by QR code — RomM's device authorisation, so a Deck needs no keyboard to sign in.
//!
//! Deckhand asks RomM for a code (`/api/auth/device/init`) and shows it as a QR code of RomM's
//! approval page with the code filled in. Scanned with a phone, signed in there, approved — and the
//! next time Deckhand asks (`/api/auth/device/token`, every few seconds) RomM hands over a token
//! for exactly the scopes this app uses, bound to a device record of its own, which is the id save
//! sync sends. Nothing secret is on screen: the code only names a request someone signed in has to
//! approve, and it lives ten minutes.

use std::time::{Duration, Instant};

use iced::widget::image::Handle;

use crate::config::Connection;
use crate::romm::{host_candidates, RommClient};

/// A request to pair, waiting on a phone.
#[derive(Debug, Clone)]
pub struct QrPairing {
    pub host: String,
    pub insecure: bool,
    device_code: String,
    /// The short code RomM's page shows, for checking against — and for typing in, without a camera.
    pub user_code: String,
    /// RomM's page that approves it, with the code filled in: what the QR code holds.
    pub url: String,
    /// The QR code, drawn.
    pub qr: Option<Handle>,
    /// How long to wait between asks.
    pub interval: Duration,
    pub expires: Instant,
}

/// Where a request stands after one ask.
#[derive(Debug, Clone)]
pub enum Poll {
    /// Not approved yet: ask again after the interval.
    Waiting,
    /// Asked too fast: wait longer from now on.
    SlowDown,
    Approved(Box<Connection>),
    /// Turned down, expired, or the server out of reach — said on the login screen.
    Failed(String),
}

/// Ask RomM to approve this device, trying the address as typed and the schemes it might mean — the
/// same candidates pairing by code tries.
pub async fn start(client: &RommClient, typed: &str, insecure: bool) -> Result<QrPairing, String> {
    let candidates = host_candidates(typed);
    if candidates.is_empty() {
        return Err("Enter your RomM server address.".into());
    }
    let name = crate::sync::device::name();
    let identifier = format!("deckhand-{}", identifier_of(&crate::sync::device::hostname()));
    let mut last = None;
    for host in candidates {
        match client.device_auth_init(&host, insecure, &name, &identifier, crate::sync::device::platform()).await {
            Ok(start) => {
                let path = start
                    .verification_path_complete
                    .or_else(|| start.verification_path.map(|p| format!("{p}?user_code={}", start.user_code)))
                    .unwrap_or_else(|| format!("/pair/device?user_code={}", start.user_code));
                let url = format!("{}{}", host.trim_end_matches('/'), if path.starts_with('/') { path } else { format!("/{path}") });
                return Ok(QrPairing {
                    qr: qr_image(&url),
                    url,
                    host,
                    insecure,
                    device_code: start.device_code,
                    user_code: start.user_code,
                    interval: Duration::from_secs(start.interval.unwrap_or(5).max(1)),
                    expires: Instant::now() + Duration::from_secs(start.expires_in.unwrap_or(600)),
                });
            }
            Err(e) if e.is_transport() => last = Some(e),
            // Reached, and refused: a RomM from before device authorisation answers 404 here.
            Err(e) if e.status == 404 => {
                return Err("This RomM can't pair by QR code — it needs RomM 5.3 or later. Use a pairing code instead.".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(last.map(|e| e.to_string()).unwrap_or_else(|| "Could not reach the server.".into()))
}

/// Wait the interval, then ask whether the request has been approved.
pub async fn poll(client: &RommClient, pairing: &QrPairing) -> Poll {
    tokio::time::sleep(pairing.interval).await;
    if Instant::now() >= pairing.expires {
        return Poll::Failed("The QR code expired before it was approved. Show a new one.".into());
    }
    match client.device_auth_token(&pairing.host, pairing.insecure, &pairing.device_code).await {
        Ok(token) => {
            let connection = Connection {
                host: pairing.host.clone(),
                token: token.access_token,
                insecure: pairing.insecure,
                token_id: None,
                token_name: Some(crate::sync::device::name()),
                device_id: token.device_id,
            };
            // Proved before it is kept, as a pairing code's token is.
            match client.fetch_current_user(&connection).await {
                Ok(_) => Poll::Approved(Box::new(connection)),
                Err(e) => Poll::Failed(e.to_string()),
            }
        }
        Err(e) => match e.detail.as_deref() {
            Some("authorization_pending") => Poll::Waiting,
            Some("slow_down") => Poll::SlowDown,
            Some("access_denied") => Poll::Failed("Pairing was turned down in RomM.".into()),
            Some("expired_token") => Poll::Failed("The QR code expired before it was approved. Show a new one.".into()),
            // A dropped connection is not an answer: ask again.
            _ if e.is_transport() => Poll::Waiting,
            _ => Poll::Failed(e.to_string()),
        },
    }
}

/// A stable, opaque name for this machine to RomM: the same each time it pairs, so RomM can tell
/// a re-pairing from a new device, and not the hostname itself.
fn identifier_of(hostname: &str) -> String {
    use md5::{Digest, Md5};
    let digest = Md5::digest(format!("deckhand:{hostname}").as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// `text` as a QR code, black on white with the quiet zone a scanner needs, eight pixels a module.
pub fn qr_image(text: &str) -> Option<Handle> {
    const SCALE: usize = 8;
    const QUIET: usize = 4;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let side = (modules + QUIET * 2) * SCALE;
    let mut pixels = vec![255u8; side * side * 4];
    for y in 0..modules {
        for x in 0..modules {
            if colors[y * modules + x] == qrcode::Color::Dark {
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let (px, py) = ((x + QUIET) * SCALE + dx, (y + QUIET) * SCALE + dy);
                        let at = (py * side + px) * 4;
                        pixels[at..at + 3].copy_from_slice(&[0, 0, 0]);
                    }
                }
            }
        }
    }
    Some(Handle::from_rgba(side as u32, side as u32, pixels))
}

/// A request as `start` would make it, without a server — for the app's tests.
#[cfg(test)]
pub fn start_for_test(host: &str) -> QrPairing {
    let url = format!("{host}/pair/device?user_code=ABCD1234");
    QrPairing {
        host: host.into(),
        insecure: false,
        device_code: "secret".into(),
        user_code: "ABCD1234".into(),
        qr: qr_image(&url),
        url,
        interval: Duration::from_secs(5),
        expires: Instant::now() + Duration::from_secs(600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub_server;

    #[test]
    fn the_qr_code_is_drawn_and_the_identifier_is_stable_and_opaque() {
        assert!(qr_image("https://romm.local/pair/device?user_code=ABCD1234").is_some());
        let id = identifier_of("steamdeck");
        assert_eq!(id, identifier_of("steamdeck"));
        assert!(!id.contains("steamdeck") && id.len() == 24);
    }

    #[tokio::test]
    async fn a_request_is_shown_waited_on_and_turned_into_a_connection_once_approved() {
        let host = stub_server::start(vec![
            (
                "POST /api/auth/device/init",
                201,
                r#"{"device_code":"secret","user_code":"ABCD1234","verification_path":"/pair/device","verification_path_complete":"/pair/device?user_code=ABCD1234","expires_in":600,"interval":1}"#.into(),
            ),
            ("POST /api/auth/device/token", 200, r#"{"access_token":"rmm_approved","device_id":"dev-9","scopes":["roms.read"],"expires_at":null}"#.into()),
            ("GET /api/users/me", 200, r#"{"id":1,"username":"player"}"#.into()),
        ])
        .await;
        let client = RommClient::new().unwrap();
        let pairing = start(&client, &host, false).await.unwrap();
        assert_eq!(pairing.user_code, "ABCD1234");
        assert_eq!(pairing.url, format!("{host}/pair/device?user_code=ABCD1234"));
        assert!(pairing.qr.is_some());
        match poll(&client, &pairing).await {
            Poll::Approved(connection) => {
                assert_eq!(connection.token, "rmm_approved");
                assert_eq!(connection.device_id.as_deref(), Some("dev-9"), "save sync's device, from the pairing itself");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn waiting_slowing_down_and_refusal_are_told_apart() {
        let host = stub_server::start(vec![
            ("POST /api/auth/device/token", 400, r#"{"detail":"authorization_pending"}"#.into()),
        ])
        .await;
        let client = RommClient::new().unwrap();
        let pairing = QrPairing {
            host: host.clone(),
            insecure: false,
            device_code: "secret".into(),
            user_code: "X".into(),
            url: String::new(),
            qr: None,
            interval: Duration::from_millis(1),
            expires: Instant::now() + Duration::from_secs(60),
        };
        assert!(matches!(poll(&client, &pairing).await, Poll::Waiting));
        let denied = stub_server::start(vec![("POST /api/auth/device/token", 400, r#"{"detail":"access_denied"}"#.into())]).await;
        let refused = poll(&client, &QrPairing { host: denied, ..pairing.clone() }).await;
        assert!(matches!(refused, Poll::Failed(why) if why.contains("turned down")));
        let gone = QrPairing { expires: Instant::now(), ..pairing };
        assert!(matches!(poll(&client, &gone).await, Poll::Failed(why) if why.contains("expired")));
    }
}

//! A stub RomM server for tests — canned responses on a real socket.
//!
//! Pairing, the library load, a download and a save sync all talk to this, because all four are
//! mostly *translation*: RomM's field names vary across server versions, its list endpoints come
//! back in two different shapes, its `ss_metadata` paths need a prefix that nothing errors without,
//! and its file paths are given from the library root rather than from the rom's own folder. None of
//! that is exercised by testing the pure functions around it.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Routes as (method + path prefix) → (status, JSON body).
pub type Routes = Vec<(&'static str, u16, String)>;

/// Every request line the server has seen, in order — `POST /api/saves?rom_id=1&… HTTP/1.1`.
pub type Requests = Arc<Mutex<Vec<String>>>;

/// Start a server on an ephemeral port and return its base URL. It serves until the test ends.
pub async fn start(routes: Routes) -> String {
    start_recording(routes).await.0
}

/// [`start`], also handing back the request lines it receives, for a test that has to know what
/// was asked rather than only what came back.
pub async fn start_recording(routes: Routes) -> (String, Requests) {
    serve(routes.into_iter().map(|(pattern, status, body)| (pattern, status, body.into_bytes())).collect()).await
}

/// [`start`] with bodies that are bytes rather than text — an archive a download fetches.
pub async fn start_bytes(routes: Vec<(&'static str, u16, Vec<u8>)>) -> String {
    serve(routes).await.0
}

async fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> (String, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("the bound address");
    let routes = Arc::new(routes);
    let requests = Requests::default();

    let log = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let routes = routes.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let Some(line) = read_request(&mut socket).await else { return };
                log.lock().unwrap().push(line.clone());

                let (status, body) = routes
                    .iter()
                    .find(|(pattern, _, _)| {
                        let (method, path) = pattern.split_once(' ').expect("\"METHOD /path\"");
                        line.starts_with(method) && line.contains(path)
                    })
                    .map(|(_, status, body)| (*status, body.clone()))
                    .unwrap_or((404, b"{\"detail\":\"not found\"}".to_vec()));

                let head = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.flush().await;
            });
        }
    });

    (format!("http://{addr}"), requests)
}

/// Read one whole request — headers, then as much body as `Content-Length` promises — and return
/// its request line.
///
/// All of it, not just the first read: an upload's body can arrive after its headers, and answering
/// and closing before it is in resets the connection under a client still sending.
async fn read_request(socket: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut buf = vec![0u8; 8192];
    loop {
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buf[..n]);
        let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
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
    let text = String::from_utf8_lossy(&request);
    Some(text.lines().next().unwrap_or_default().to_string())
}

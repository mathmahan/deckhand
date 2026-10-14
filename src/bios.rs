//! BIOS and firmware: whatever RomM holds for a platform, put where the emulators look for it.
//!
//! A port of the Kotlin app's `BiosSync`. Some systems will not boot a game without one — a
//! PlayStation, a Saturn, a Dreamcast — and RomM is where a user already keeps them, so a game that
//! needs one should not need a second trip to fetch it.
//!
//! The folder is RetroArch's `system_directory` ([`crate::sync::retroarch::bios_dir`]), which on
//! RetroDECK is its shared `bios/` folder: every bundled core *and* standalone emulator reads it.
//! RomM's `file_name` is already the name an emulator expects, so a file lands under it unchanged.
//!
//! Run after a download and again before every launch. Both are cheap when there is nothing to do —
//! one request, and a size check per file — and the second is what gives a game downloaded before
//! its BIOS was uploaded to RomM the file anyway. Best-effort throughout: a missing BIOS is the
//! emulator's to report, and never worth stopping a launch for.

use std::fs;
use std::path::Path;

use crate::config::Connection;
use crate::downloads::layout::safe_name;
use crate::romm::RommClient;
use crate::romm::models::Firmware;

/// Whether `dest` should be (re)fetched for `firmware`: missing, or a different size from RomM's
/// copy — a partial file from before `.part` downloads, or a file RomM has since replaced. A
/// nameless record is never fetched; there is nothing to call the file.
pub fn needs_download(dest: &Path, firmware: &Firmware) -> bool {
    !firmware.file_name.trim().is_empty()
        && fs::metadata(dest).map_or(true, |m| !m.is_file() || m.len() != firmware.file_size_bytes.max(0) as u64)
}

/// Fetch every firmware file RomM has for `platform_id` that `dir` does not already hold.
///
/// Answers the names fetched, for a banner; an empty list is the common case and says nothing. An
/// error is only for a list RomM would not give — one file that fails is skipped, so a single bad
/// record does not cost the rest.
pub async fn sync(client: &RommClient, conn: &Connection, platform_id: i64, dir: &Path) -> Result<Vec<String>, String> {
    let firmware = client.fetch_firmware(conn, platform_id).await.map_err(|e| e.to_string())?;
    let mut fetched = Vec::new();
    for fw in &firmware {
        // Sanitised: the name comes from the server and must not be able to write outside `dir`.
        // The request still carries RomM's own spelling.
        let dest = dir.join(safe_name(&fw.file_name));
        if !needs_download(&dest, fw) {
            continue;
        }
        if client.download_firmware(conn, fw, &dest).await.is_ok() {
            fetched.push(fw.file_name.clone());
        }
    }
    Ok(fetched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firmware(name: &str, size: i64) -> Firmware {
        Firmware { id: 1, file_name: name.into(), file_size_bytes: size }
    }

    #[test]
    fn a_file_is_fetched_when_missing_or_the_wrong_size_and_not_otherwise() {
        let dir = std::env::temp_dir().join(format!("deckhand-bios-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("scph5501.bin");
        assert!(needs_download(&dest, &firmware("scph5501.bin", 4)), "missing");
        fs::write(&dest, "ab").unwrap();
        assert!(needs_download(&dest, &firmware("scph5501.bin", 4)), "a partial file");
        fs::write(&dest, "abcd").unwrap();
        assert!(!needs_download(&dest, &firmware("scph5501.bin", 4)), "already here");
        assert!(!needs_download(&dir.join("x"), &firmware("  ", 4)), "nothing to call it");
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_platforms_firmware_lands_in_the_bios_folder_once() {
        let (host, requests) = crate::stub_server::start_recording(vec![
            ("GET /api/firmware?platform_id=7", 200, r#"[{"id":3,"file_name":"scph5501.bin","file_size_bytes":4}]"#.into()),
            ("GET /api/firmware/3/content/scph5501.bin", 200, "BIOS".into()),
        ])
        .await;
        let conn = Connection { host, token: "t".into(), insecure: false, token_id: None, token_name: None, device_id: None };
        let dir = std::env::temp_dir().join(format!("deckhand-bios-sync-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let client = RommClient::new().unwrap();

        assert_eq!(sync(&client, &conn, 7, &dir).await.unwrap(), vec!["scph5501.bin"]);
        assert_eq!(fs::read_to_string(dir.join("scph5501.bin")).unwrap(), "BIOS");
        assert!(sync(&client, &conn, 7, &dir).await.unwrap().is_empty(), "the second run has nothing to do");
        let fetches = requests.lock().unwrap().iter().filter(|l| l.contains("/content/")).count();
        assert_eq!(fetches, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}

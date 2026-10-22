//! Persisted settings — the RomM connection and the UI toggles that outlive a run.
//!
//! A port of `store/SettingsStore.kt`, trimmed to what this build has settings *for*. The file is
//! `$XDG_CONFIG_HOME/deckhand-rust/settings.json` (falling back to `~/.config/…`), deliberately a
//! sibling of the Kotlin app's `deckhand/` rather than the same directory: the two write different
//! shapes, and a shared file would mean one app's write could strand the other.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A paired server: where it is, and the token that reads it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
    pub host: String,
    pub token: String,
    /// Set when the server's certificate could not be verified and the user accepted it anyway.
    /// Recorded per connection, so it never silently applies to a different host.
    #[serde(default)]
    pub insecure: bool,
    /// RomM's id for this device's client token, from the pairing response. Kept because the token
    /// list the API returns never includes the secret — without the id there is no way to tell our
    /// own token from another device's, and revoking the wrong one would lock out a machine that is
    /// not in your hands.
    #[serde(default)]
    pub token_id: Option<i64>,
    /// The token's name in RomM, so a revoke action can name which one goes.
    #[serde(default)]
    pub token_name: Option<String>,
    /// This install as RomM 5 knows it (`POST /api/devices`). RomM keeps per-device save markers and
    /// refuses a push to a slot this device has not acknowledged — see [`crate::sync::device`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub connection: Option<Connection>,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Columns added to (or taken from) every Browse grid — see [`crate::grid::density`].
    #[serde(default)]
    pub grid_density: i32,
    /// The `retroarch.cfg` save sync reads, for an install it does not find on its own — a
    /// portable RetroArch unpacked somewhere of the user's choosing. Picked in Settings ▸ Saves, from
    /// those found or by walking the folders. See [`crate::sync::retroarch::candidates`] for where
    /// it looks without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retroarch_config: Option<PathBuf>,
    /// Where downloaded roms are written, one folder per platform inside it. Picked in Settings ▸
    /// Storage, from the drives here or by walking the folders. See [`Settings::roms_dir`] for the
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roms_dir: Option<PathBuf>,
    /// What starts a downloaded game, as argv words with `{rom}` and `{system}` (the ES-DE system
    /// name) filled in. Unset means RetroDECK — see [`crate::launch`]. Typed in Settings ▸ Playing as
    /// a command line, for a machine with a RetroArch or ES-DE of its own instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_command: Option<Vec<String>>,
    /// A command for one system's games, by ES-DE system name (`ps2`, `gc`), ahead of
    /// `launch_command` and RetroDECK — a PCSX2 installed on its own, say. Set in Settings ▸ Playing ▸
    /// *Emulator for each system*.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub system_commands: std::collections::BTreeMap<String, Vec<String>>,
    /// The space downloaded games may take, in GiB, or None for no limit — see
    /// [`crate::downloads::storage`]. A download that would go over it first makes room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_budget_gb: Option<u64>,
    /// Saves are synced as they change, while the window is open, not only around a launch — see
    /// [`crate::watch`]. On unless turned off.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub watch_saves: bool,
    /// Play boots straight into the game's newest save state, where there is one on this device, and
    /// off starts it fresh — whatever RetroArch's own Auto Load State says; see [`crate::launch`].
    /// Off until chosen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume_state: bool,
    /// Turning RetroArch's state pictures on was offered after a sync sent a state up without one,
    /// and declined: not offered again. Settings ▸ Saves ▸ State pictures still turns them on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub state_pictures_declined: bool,
}

/// Written by hand so the token never reaches a log line or a panic message through `{:?}` — a
/// `Message` carries a `Connection`, and the settings travel in a few.
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("host", &self.host)
            .field("token", &"<hidden>")
            .field("insecure", &self.insecure)
            .field("token_id", &self.token_id)
            .field("token_name", &self.token_name)
            .field("device_id", &self.device_id)
            .finish()
    }
}

fn on() -> bool {
    true
}

fn is_on(value: &bool) -> bool {
    *value
}

fn default_ui_scale() -> f32 {
    crate::theme::ui_scale::DEFAULT
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connection: None,
            ui_scale: default_ui_scale(),
            grid_density: 0,
            retroarch_config: None,
            roms_dir: None,
            launch_command: None,
            system_commands: Default::default(),
            storage_budget_gb: None,
            resume_state: false,
            state_pictures_declined: false,
            watch_saves: true,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// A cache folder for the test running on this thread alone — see [`Settings::cache_dir`].
    pub static TEST_CACHE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Where a test run keeps what the app would keep in the user's folders: one temporary directory
/// per run. Every path below resolves under it in a test build, so no test — and no code path a
/// test happens to reach, such as pairing or signing out — can read or write the real settings,
/// caches or downloads. One once did, and overwrote a real pairing with a test's.
#[cfg(test)]
pub fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!("deckhand-tests-{}", std::process::id()))
}

impl Settings {
    /// `$XDG_CONFIG_HOME/deckhand-rust`, falling back to `~/.config/deckhand-rust`.
    pub fn config_dir() -> PathBuf {
        #[cfg(test)]
        return test_root().join("config");
        #[cfg(not(test))]
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("deckhand-rust")
    }

    /// `$XDG_CACHE_HOME/deckhand-rust` — the cover cache lives here, so clearing it never touches
    /// the pairing.
    pub fn cache_dir() -> PathBuf {
        // Never the user's from a test, whatever XDG_CACHE_HOME says — it is set on plenty of
        // desktops, and was what a test read. A test that wants a cache of its own names it for its
        // own thread with `TEST_CACHE`; every other test gets the run's.
        #[cfg(test)]
        return TEST_CACHE.with(|dir| dir.borrow().clone()).unwrap_or_else(|| test_root().join("cache"));
        #[cfg(not(test))]
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(dirs::cache_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("deckhand-rust")
    }

    pub fn file() -> PathBuf {
        Self::config_dir().join("settings.json")
    }

    /// Where downloads go: `roms_dir` when the file names one, else [`Self::default_roms_dir`].
    ///
    /// Not the cache directory and not the config one. A rom is the user's own content — it is why
    /// the drive has the space it does, and it has to survive anything that clears a cache.
    ///
    /// Per-platform folders inside it are named by RomM's own platform slug, except in RetroDECK's
    /// roms folder, where they take ES-DE's system names — ES-DE scans the folders it knows and
    /// ignores the rest.
    pub fn roms_dir(&self) -> PathBuf {
        self.roms_dir.clone().unwrap_or_else(Self::default_roms_dir)
    }

    /// Where downloads go when nothing says otherwise: RetroDECK's roms folder where RetroDECK is, so
    /// ES-DE lists the games too, else `~/Games/Deckhand`.
    pub fn default_roms_dir() -> PathBuf {
        #[cfg(test)]
        return test_root().join("Games").join("Deckhand");
        #[cfg(not(test))]
        crate::launch::retrodeck_roms()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Games").join("Deckhand"))
    }

    /// Read the stored settings. A missing or unreadable file yields the defaults rather than an
    /// error: the app must still open — on the login screen — when its config is gone or corrupt.
    pub fn load() -> Self {
        fs::read_to_string(Self::file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Persist, writing to a temp file and renaming, so a crash mid-write cannot leave a truncated
    /// settings file behind — the one that would strand a paired device on the login screen.
    pub fn save(&self) -> std::io::Result<()> {
        let dir = Self::config_dir();
        fs::create_dir_all(&dir)?;
        let tmp = dir.join("settings.json.tmp");
        // Readable by this user alone: it holds the RomM token.
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        // `mode` only applies to a file it creates; one left over from an older build is narrowed too.
        #[cfg(unix)]
        fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        std::io::Write::write_all(&mut file, serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(tmp, Self::file())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_shows_a_token() {
        let settings = Settings {
            connection: Some(Connection {
                host: "https://romm.local".into(),
                token: "rmm_secret".into(),
                insecure: false,
                token_id: None,
                token_name: None,
                device_id: None,
            }),
            ..Settings::default()
        };
        let shown = format!("{settings:?}");
        assert!(!shown.contains("secret") && shown.contains("romm.local"), "{shown}");
    }

    #[test]
    fn a_settings_file_with_the_old_update_token_still_reads() {
        let settings: Settings = serde_json::from_str(r#"{"release_token": "old", "resume_state": true}"#).unwrap();
        assert!(settings.resume_state);
        assert!(!serde_json::to_string(&settings).unwrap().contains("old"), "and is dropped when written");
    }
}

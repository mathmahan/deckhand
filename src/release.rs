//! Deckhand's own new versions: whether the repository has published one newer than this build,
//! and — run as an AppImage — fetching it and putting it in place of the file that is running.
//!
//! - **Asked where it is published.** `repository` in `Cargo.toml` says where. On GitHub the
//!   repository is public, and its releases are read through GitHub's API with no token. Anywhere
//!   else it is a private Forgejo one, whose releases read only with a token that can see it, typed
//!   or pasted in Settings ▸ About ▸ *Update token*, and best made able to read this one repository
//!   and nothing else. Without one nothing is asked there, and Settings says so. A token is only
//!   ever sent to the API's own host.
//! - **When**: at launch, every six hours while the window stays open, and from the Settings row.
//! - **Replaced in place.** The new AppImage takes the running one's path and name, so whatever
//!   starts Deckhand — a Steam shortcut, a desktop entry — starts the new one. It is checked before
//!   it replaces anything: its size against the release's, that it is an AppImage at all, and its
//!   SHA-256 against the one the release notes give. The swap is a single rename, so a failure
//!   anywhere before it leaves the installed version as it was; the running copy keeps the old
//!   file open until it exits.
//! - **Anything else** — a `cargo build`, a Windows binary — is told what is out, and updated by
//!   hand.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// How often the app asks again while it stays open — a Deck in Game Mode can stay open for days.
pub const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The repository this build's releases are published in: `repository` in `Cargo.toml`.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// `repository`'s scheme, host and `owner/name`.
fn parts(repository: &str) -> (&str, &str, &str) {
    let repository = repository.trim_end_matches('/').trim_end_matches(".git");
    let (scheme, rest) = repository.split_once("://").unwrap_or(("https", repository));
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    (scheme, host, path)
}

fn on_github(repository: &str) -> bool {
    let (_, host, _) = parts(repository);
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// Whether `repository`'s releases read only with a token: a Forgejo one's, which is private, do;
/// GitHub's, which is public, do not.
pub fn needs_token(repository: &str) -> bool {
    !on_github(repository)
}

/// Where `repository`'s newest release is read. `https://host/owner/name` is Forgejo's
/// `https://host/api/v1/repos/owner/name/releases/latest`; on GitHub, which keeps its API on a host
/// of its own, `https://api.github.com/repos/owner/name/releases/latest`.
pub fn latest_url(repository: &str) -> String {
    let (scheme, host, path) = parts(repository);
    if on_github(repository) {
        format!("https://api.github.com/repos/{path}/releases/latest")
    } else {
        format!("{scheme}://{host}/api/v1/repos/{path}/releases/latest")
    }
}

/// `repository`'s releases page, for a copy that cannot install a release itself.
pub fn releases_page(repository: &str) -> String {
    format!("{}/releases", repository.trim_end_matches('/').trim_end_matches(".git"))
}

/// A published release, as much of it as an update needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// `0.6.0` — the tag without its `v`.
    pub version: String,
    /// The notes' first line, without its markdown — what the release is about.
    pub headline: Option<String>,
    /// The AppImage it carries for this machine's architecture, if any.
    pub appimage: Option<Asset>,
    /// A draft or a pre-release, which is never offered.
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Its SHA-256 in lowercase hex, when the release notes give one.
    pub sha256: Option<String>,
}

impl Release {
    /// Whether it is newer than the build running.
    pub fn is_newer(&self) -> bool {
        !self.preview && newer(&self.version, env!("CARGO_PKG_VERSION"))
    }
}

/// Where the app's own update stands — what Settings shows, and what pressing its row does.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Status {
    /// No `release_token`, where the repository needs one, so nothing is asked.
    #[default]
    Off,
    Checking,
    /// This build is the newest.
    Current,
    /// A newer one is out.
    Out(Box<Release>),
    /// Being fetched, and how far.
    Installing(Box<Release>, f32),
    /// In place of the old file: it runs from the next start.
    Installed(Box<Release>),
    /// What went wrong, and the release a retry would install — None when it was the asking that
    /// failed.
    Failed(String, Option<Box<Release>>),
}

/// News from an install in flight.
#[derive(Debug, Clone)]
pub enum Event {
    Progress(f32),
    Done(Result<(), String>),
}

#[derive(Deserialize)]
struct Wire {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<WireAsset>,
}

#[derive(Deserialize)]
struct WireAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// `sha256:<hex>`, which GitHub gives for every asset it holds. Forgejo gives none.
    #[serde(default)]
    digest: Option<String>,
}

fn parse(json: &str) -> Result<Release, String> {
    let wire: Wire = serde_json::from_str(json).map_err(|e| format!("the release could not be read: {e}"))?;
    let notes = wire.body.unwrap_or_default();
    let appimage = wire
        .assets
        .into_iter()
        .find(|a| a.name.ends_with(".AppImage") && a.name.contains(std::env::consts::ARCH))
        .map(|a| Asset {
            // The notes' word first: they are what the release's author vouched for.
            sha256: sha256_in(&notes, &a.name).or_else(|| digest(a.digest.as_deref())),
            name: a.name,
            url: a.browser_download_url,
            size: a.size,
        });
    Ok(Release {
        version: wire.tag_name.trim_start_matches('v').to_string(),
        headline: headline(&notes),
        appimage,
        preview: wire.draft || wire.prerelease,
    })
}

/// Whether version `a` is newer than `b`: dotted numbers compared as numbers, so 0.10 comes after
/// 0.9, and 0.6 is 0.6.0. Anything that does not read that way — a pre-release suffix, a typo — is
/// never newer.
fn newer(a: &str, b: &str) -> bool {
    fn numbers(version: &str) -> Option<Vec<u64>> {
        let mut parts: Vec<u64> =
            version.trim().trim_start_matches('v').split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        while parts.len() > 1 && parts.last() == Some(&0) {
            parts.pop();
        }
        Some(parts)
    }
    matches!((numbers(a), numbers(b)), (Some(a), Some(b)) if a > b)
}

/// The notes' first line with words on it, without its markdown: `**New in 0.5.1: less memory**`
/// reads `New in 0.5.1: less memory`.
fn headline(notes: &str) -> Option<String> {
    let line = notes.lines().map(str::trim).find(|l| !l.is_empty())?;
    let plain = line.trim_start_matches('#').replace("**", "").replace('`', "");
    let plain = plain.trim();
    (!plain.is_empty()).then(|| plain.to_string())
}

/// The SHA-256 the release notes give for `asset`: a 64-digit hex word on the line naming the asset,
/// or failing that on the only line that has one.
fn sha256_in(notes: &str, asset: &str) -> Option<String> {
    let found: Vec<(&str, String)> = notes
        .lines()
        .filter_map(|line| {
            line.split(|c: char| !c.is_ascii_hexdigit())
                .find(|word| word.len() == 64)
                .map(|word| (line, word.to_ascii_lowercase()))
        })
        .collect();
    let named = found.iter().find(|(line, _)| line.contains(asset));
    named.or(if found.len() == 1 { found.first() } else { None }).map(|(_, hash)| hash.clone())
}

/// The SHA-256 in an asset's `sha256:<hex>` digest.
fn digest(digest: Option<&str>) -> Option<String> {
    let hex = digest?.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Deckhand/", env!("CARGO_PKG_VERSION"), " (rust)"))
        .build()
        .map_err(|e| e.to_string())
}

/// `request` with the token on it — when there is one and `url` is on the API's own host, and only
/// then.
fn authorised(request: reqwest::RequestBuilder, api: &str, url: &str, token: &str) -> reqwest::RequestBuilder {
    let origin = |u: &str| reqwest::Url::parse(u).ok().map(|u| u.origin());
    match (origin(api), origin(url)) {
        (Some(a), Some(b)) if a == b && !token.is_empty() => {
            request.header(reqwest::header::AUTHORIZATION, format!("token {token}"))
        }
        _ => request,
    }
}

fn unreachable(url: &str) -> String {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string));
    format!("could not reach {}", host.as_deref().unwrap_or(url))
}

/// Read the newest release. `api` is [`latest_url`], or a test server's; `token` may be empty.
pub async fn check(api: &str, token: &str) -> Result<Release, String> {
    let response = authorised(client()?.get(api), api, api, token).send().await.map_err(|_| unreachable(api))?;
    // GitHub turns away whoever has asked too often this hour, and says so in this header.
    let spent = response.headers().get("x-ratelimit-remaining").is_some_and(|left| left == "0");
    match response.status().as_u16() {
        200 => parse(&response.text().await.map_err(|_| "the release stopped arriving part way".to_string())?),
        403 | 429 if spent => Err("asked too often for now — it asks again later".into()),
        401 | 403 if !token.is_empty() => Err("the release token was turned down".into()),
        401 | 403 => Err("the repository turned the question down".into()),
        // Forgejo answers for a private repository it will not show with a 404, not a 403.
        404 => Err("no release there, or the token cannot see the repository".into()),
        code => Err(format!("the repository answered {code}")),
    }
}

/// The AppImage this process runs from, when it is one. The runtime names the file in `APPIMAGE`
/// and the folder it mounted in `APPDIR`, and the binary running has to be inside that folder — so
/// an `APPIMAGE` left in the environment by something else is never taken for this one.
pub fn running_appimage() -> Option<PathBuf> {
    let file = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let mounted = PathBuf::from(std::env::var_os("APPDIR")?);
    let exe = std::env::current_exe().ok()?;
    (exe.starts_with(&mounted) && file.is_file()).then_some(file)
}

/// Fetch `release`'s AppImage and put it where `target` — the AppImage running — is. `progress`
/// hears 0.0–1.0 of the download.
pub async fn install(
    api: &str,
    token: &str,
    release: &Release,
    target: &Path,
    mut progress: impl FnMut(f32),
) -> Result<(), String> {
    let asset = release.appimage.as_ref().ok_or("that release has no AppImage for this machine")?;
    let dir = target.parent().ok_or("the AppImage has no folder")?;
    let name = target.file_name().ok_or("the AppImage has no name")?.to_string_lossy();
    // Beside it, so the swap is a rename on the one drive; hidden, so a half-fetched file is never
    // taken for one to run.
    let part = dir.join(format!(".{name}.download"));
    let response =
        authorised(client()?.get(&asset.url), api, &asset.url, token).send().await.map_err(|_| unreachable(&asset.url))?;
    if !response.status().is_success() {
        return Err(format!("the download answered {}", response.status().as_u16()));
    }
    let result = fetch_into(response, &part, asset, &mut progress).await.and_then(|()| swap(&part, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

async fn fetch_into(
    response: reqwest::Response,
    part: &Path,
    asset: &Asset,
    progress: &mut impl FnMut(f32),
) -> Result<(), String> {
    let cannot_write = |e: std::io::Error| format!("can't write beside the AppImage: {e}");
    let mut file = std::fs::File::create(part).map_err(cannot_write)?;
    let mut hash = Sha256::new();
    let mut head: Vec<u8> = Vec::with_capacity(16);
    let (mut done, mut shown) = (0u64, u64::MAX);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "the download stopped part way".to_string())?;
        head.extend(chunk.iter().take(16 - head.len()));
        hash.update(&chunk);
        file.write_all(&chunk).map_err(cannot_write)?;
        done += chunk.len() as u64;
        // Once a percent, not once a chunk.
        let percent = done * 100 / asset.size.max(1);
        if percent != shown {
            shown = percent;
            progress((done as f32 / asset.size.max(1) as f32).min(1.0));
        }
    }
    file.sync_all().map_err(cannot_write)?;
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    verify(done, &head, &digest, asset)
}

/// That what arrived is the release's AppImage, whole.
fn verify(size: u64, head: &[u8], sha256: &str, asset: &Asset) -> Result<(), String> {
    if size != asset.size {
        return Err(format!("the download is {size} bytes, where the release has {}", asset.size));
    }
    // An ELF executable with the AppImage type 2 mark at byte 8.
    if !(head.starts_with(b"\x7fELF") && head.get(8..11) == Some(b"AI\x02".as_slice())) {
        return Err("what downloaded is not an AppImage".into());
    }
    // Without a checksum to hold it to, a file of the right size with the right header could still
    // be anything: an update without one is not installed.
    match asset.sha256.as_deref() {
        None => Err("the release notes give no SHA-256 for its AppImage, so it was not installed".into()),
        Some(expected) if expected != sha256 => Err("the download does not match the SHA-256 in the release notes".into()),
        Some(_) => Ok(()),
    }
}

/// Runnable like the file it replaces, then renamed over it.
fn swap(part: &Path, target: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target).map_or(0o755, |m| m.permissions().mode() & 0o7777) | 0o100;
        std::fs::set_permissions(part, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("can't make the new AppImage runnable: {e}"))?;
    }
    std::fs::rename(part, target).map_err(|e| format!("can't put the new AppImage in place: {e}"))
}

/// Start `appimage` afresh, with the arguments this run had. The caller then exits, which lets go
/// of the old file.
pub fn relaunch(appimage: &Path) -> std::io::Result<()> {
    std::process::Command::new(appimage).args(std::env::args_os().skip(1)).spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub_server;

    const NOTES: &str = "**New in 0.9.0: something**\n\nBody text.\n\nsha256 `D941905E616084021EACFBC530839E5CCF95ED6D34444509DFB367DF54FC985F`\n";

    fn json(tag: &str, notes: &str, assets: &[(&str, usize, &str)]) -> String {
        serde_json::json!({
            "tag_name": tag,
            "name": format!("Deckhand {tag}"),
            "body": notes,
            "draft": false,
            "prerelease": false,
            "assets": assets.iter().map(|(name, size, url)| serde_json::json!({
                "name": name, "size": size, "browser_download_url": url
            })).collect::<Vec<_>>(),
        })
        .to_string()
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.6.0", "0.5.1"));
        assert!(newer("v0.10.0", "0.9.9"), "0.10 is after 0.9");
        assert!(newer("1", "0.99.0"));
        assert!(!newer("0.5.1", "0.5.1"));
        assert!(!newer("0.6", "0.6.0"), "the same version written shorter");
        assert!(!newer("0.4.9", "0.5.0"));
        assert!(!newer("0.7.0-rc1", "0.6.0"), "a pre-release is never offered");
        assert!(!newer("latest", "0.6.0"));
    }

    #[test]
    fn the_api_is_found_from_the_repository() {
        assert_eq!(
            latest_url("https://repo.example/mattias/Deckhand"),
            "https://repo.example/api/v1/repos/mattias/Deckhand/releases/latest"
        );
        assert_eq!(latest_url("https://repo.example/a/b.git/"), "https://repo.example/api/v1/repos/a/b/releases/latest");
        assert_eq!(latest_url("https://github.com/mathmahan/deckhand"), "https://api.github.com/repos/mathmahan/deckhand/releases/latest");
        assert_eq!(latest_url("https://GitHub.com/a/b.git"), "https://api.github.com/repos/a/b/releases/latest");
        assert!(latest_url(REPOSITORY).ends_with("/releases/latest"));
        assert_eq!(releases_page("https://github.com/a/b.git"), "https://github.com/a/b/releases");
    }

    #[test]
    fn only_a_forgejo_repository_needs_a_token() {
        assert!(needs_token("https://repo.example/a/b"));
        assert!(!needs_token("https://github.com/a/b"));
        assert!(!needs_token("https://www.github.com/a/b"));
        assert!(needs_token("https://github.com.example/a/b"), "a host that only starts like GitHub's");
    }

    #[test]
    fn a_checksum_comes_from_the_notes_else_from_githubs_digest() {
        let arch = std::env::consts::ARCH;
        let image = format!("Deckhand-0.9.0-{arch}.AppImage");
        let with_digest = |notes: &str, digest: &str| {
            let json = serde_json::json!({
                "tag_name": "v0.9.0", "body": notes,
                "assets": [{ "name": image, "size": 1, "browser_download_url": "u", "digest": digest }],
            });
            parse(&json.to_string()).unwrap().appimage.unwrap().sha256
        };
        let c = "C".repeat(64);
        assert_eq!(with_digest("", &format!("sha256:{c}")), Some("c".repeat(64)));
        assert_eq!(with_digest(NOTES, &format!("sha256:{c}")).unwrap(), "d941905e616084021eacfbc530839e5ccf95ed6d34444509dfb367df54fc985f");
        assert_eq!(with_digest("", "sha512:abc"), None, "only a SHA-256 is any use");
        assert_eq!(with_digest("", "sha256:12"), None);
    }

    #[test]
    fn a_release_reads_its_version_headline_appimage_and_checksum() {
        let arch = std::env::consts::ARCH;
        // A version ahead of any build, so the release reads as newer whatever this one is.
        let image = format!("Deckhand-99.1.0-{arch}.AppImage");
        let release = parse(&json("v99.1.0", NOTES, &[("notes.txt", 3, "u1"), (&image, 1234, "https://repo.example/dl")])).unwrap();
        assert_eq!(release.version, "99.1.0");
        assert_eq!(release.headline.as_deref(), Some("New in 0.9.0: something"));
        let asset = release.appimage.clone().expect("this machine's AppImage");
        assert_eq!((asset.name.as_str(), asset.size, asset.url.as_str()), (image.as_str(), 1234, "https://repo.example/dl"));
        assert_eq!(asset.sha256.as_deref(), Some("d941905e616084021eacfbc530839e5ccf95ed6d34444509dfb367df54fc985f"));
        assert!(release.is_newer());

        let other_arch = parse(&json("v99.1.0", "", &[("Deckhand-99.1.0-riscv128.AppImage", 1, "u")])).unwrap();
        assert_eq!(other_arch.appimage, None, "another machine's AppImage is not this one's");
        assert_eq!(other_arch.headline, None);
    }

    #[test]
    fn with_several_checksums_the_one_naming_the_asset_is_taken() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let notes = format!("{a}  Deckhand-x86_64.AppImage\n{b}  Deckhand-aarch64.AppImage\n");
        assert_eq!(sha256_in(&notes, "Deckhand-aarch64.AppImage"), Some(b));
        assert_eq!(sha256_in(&notes, "Deckhand-riscv.AppImage"), None, "two, and neither names it");
    }

    #[test]
    fn a_draft_or_pre_release_is_never_newer() {
        let mut release = parse(&json("v99.0.0", "", &[])).unwrap();
        assert!(release.is_newer());
        release.preview = true;
        assert!(!release.is_newer());
    }

    #[test]
    fn the_token_goes_to_the_repository_host_only() {
        let http = client().unwrap();
        let api = "https://repo.example/api/v1/repos/a/b/releases/latest";
        let token = |url: &str| {
            let request = authorised(http.get(url), api, url, "secret").build().unwrap();
            request.headers().get("authorization").map(|v| v.to_str().unwrap().to_string())
        };
        assert_eq!(token("https://repo.example/a/b/releases/download/v1/x.AppImage").as_deref(), Some("token secret"));
        assert_eq!(token("https://cdn.example/x.AppImage"), None, "another host");
        assert_eq!(token("http://repo.example/x.AppImage"), None, "the same host in the clear");
        let request = authorised(http.get(api), api, api, "").build().unwrap();
        assert!(request.headers().get("authorization").is_none(), "no token, no header");
    }

    /// A temp folder of its own, gone when the test ends.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deckhand-release-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn appimage_bytes(fill: u8) -> Vec<u8> {
        let mut bytes = b"\x7fELF\x02\x01\x01\x00AI\x02\x00\x00\x00\x00\x00".to_vec();
        bytes.extend(std::iter::repeat_n(fill, 4096));
        bytes
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[tokio::test]
    async fn the_newest_release_is_asked_for_and_a_refusal_is_named() {
        let host = stub_server::start(vec![("GET /api/v1/repos/a/b/releases/latest", 200, json("v9.0.0", NOTES, &[]))]).await;
        let found = check(&format!("{host}/api/v1/repos/a/b/releases/latest"), "t").await.unwrap();
        assert_eq!(found.version, "9.0.0");
        let missing = check(&format!("{host}/api/v1/repos/a/private/releases/latest"), "t").await.unwrap_err();
        assert!(missing.contains("token cannot see"), "{missing}");
        let down = check("http://127.0.0.1:9/api/v1/repos/a/b/releases/latest", "t").await.unwrap_err();
        assert_eq!(down, "could not reach 127.0.0.1");
    }

    #[tokio::test]
    async fn an_install_replaces_the_appimage_in_place_and_keeps_it_runnable() {
        let new = appimage_bytes(9);
        let host = stub_server::start_bytes(vec![("GET /dl/Deckhand.AppImage", 200, new.clone())]).await;
        let dir = Dir::new("install");
        let target = dir.0.join("Deckhand.AppImage");
        std::fs::write(&target, appimage_bytes(1)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
        let release = Release {
            version: "9.0.0".into(),
            headline: None,
            appimage: Some(Asset {
                name: "Deckhand.AppImage".into(),
                url: format!("{host}/dl/Deckhand.AppImage"),
                size: new.len() as u64,
                sha256: Some(sha256_hex(&new)),
            }),
            preview: false,
        };
        let mut seen = Vec::new();
        install(&format!("{host}/api/v1/repos/a/b/releases/latest"), "t", &release, &target, |p| seen.push(p)).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), new);
        assert_eq!(seen.last(), Some(&1.0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o750, "as runnable as before");
        }
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1, "nothing left beside it");
    }

    #[tokio::test]
    async fn a_download_that_is_not_the_release_s_leaves_the_appimage_alone() {
        let good = appimage_bytes(9);
        let host = stub_server::start_bytes(vec![
            ("GET /dl/good", 200, good.clone()),
            ("GET /dl/page", 200, b"<html>sign in</html>".to_vec()),
        ])
        .await;
        let dir = Dir::new("refused");
        let target = dir.0.join("Deckhand.AppImage");
        let old = appimage_bytes(1);
        std::fs::write(&target, &old).unwrap();
        let api = format!("{host}/api/v1/repos/a/b/releases/latest");
        let release = |path: &str, size: usize, sha256: Option<String>| Release {
            version: "9.0.0".into(),
            headline: None,
            appimage: Some(Asset { name: "Deckhand.AppImage".into(), url: format!("{host}{path}"), size: size as u64, sha256 }),
            preview: false,
        };

        let tampered = install(&api, "t", &release("/dl/good", good.len(), Some("0".repeat(64))), &target, |_| {}).await;
        assert!(tampered.unwrap_err().contains("SHA-256"));
        let short = install(&api, "t", &release("/dl/good", good.len() + 1, None), &target, |_| {}).await;
        assert!(short.unwrap_err().contains("bytes"));
        let page = install(&api, "t", &release("/dl/page", 20, None), &target, |_| {}).await;
        assert_eq!(page.unwrap_err(), "what downloaded is not an AppImage");
        let gone = install(&api, "t", &release("/dl/missing", 1, None), &target, |_| {}).await;
        assert_eq!(gone.unwrap_err(), "the download answered 404");

        assert_eq!(std::fs::read(&target).unwrap(), old, "the installed version is untouched");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1, "and no half-fetched file is left");
    }
}

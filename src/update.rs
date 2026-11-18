//! In-app updates (design: `docs/update-design.md`).
//!
//! Each release publishes `rico-update.json` (the *manifest*: version,
//! installer name, size, SHA-256, signing date) and `rico-update.json.sig`,
//! an Ed25519 signature of the manifest's exact bytes made with the owner's
//! key, which never leaves the owner's PC. Rico:
//!
//! 1. downloads both from GitHub's "latest release" (a few hundred bytes);
//! 2. accepts the manifest only if one of [`PUBLIC_KEYS`] signed it;
//! 3. refuses downgrades in several ways (see [`evaluate`]);
//! 4. downloads the installer only from GitHub's hosts, checking every
//!    redirect, at most the signed size, and keeps it only if its SHA-256 is
//!    the signed one;
//! 5. starts it (Windows only) after the user pressed "Restart now", after
//!    checking it again while it is held open without write sharing.
//!
//! Without public keys (before the owner made the signing key) nothing is
//! checked: [`configured`] is `false` and the interface says so.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The owner's public keys: the everyday one (system keychain on the owner's
/// PC) and the spare one (kept offline), made with `rico-sign keygen` on
/// 2026-11-18. With no keys, updates stay off.
pub const PUBLIC_KEYS: &[[u8; 32]] = &[
    // everyday: edjtzP51CvAUX2fGeAy/Y3ttqNqr6XM3A8GGihdfpCI=
    [
        0x79, 0xd8, 0xed, 0xcc, 0xfe, 0x75, 0x0a, 0xf0, 0x14, 0x5f, 0x67, 0xc6, 0x78, 0x0c, 0xbf, 0x63, 0x7b,
        0x6d, 0xa8, 0xda, 0xab, 0xe9, 0x73, 0x37, 0x03, 0xc1, 0x86, 0x8a, 0x17, 0x5f, 0xa4, 0x22,
    ],
    // spare: w5QgT6vARh4d6yn2fOAPWN5RitWOBIdU2qO16DrLOBM=
    [
        0xc3, 0x94, 0x20, 0x4f, 0xab, 0xc0, 0x46, 0x1e, 0x1d, 0xeb, 0x29, 0xf6, 0x7c, 0xe0, 0x0f, 0x58, 0xde,
        0x51, 0x8a, 0xd5, 0x8e, 0x04, 0x87, 0x54, 0xda, 0xa3, 0xb5, 0xe8, 0x3a, 0xcb, 0x38, 0x13,
    ],
];

/// Where the manifest of the newest stable release is.
pub const MANIFEST_URL: &str = "https://github.com/Trive253x/rico/releases/latest/download/rico-update.json";
pub const SIGNATURE_URL: &str =
    "https://github.com/Trive253x/rico/releases/latest/download/rico-update.json.sig";

/// Hosts the updater may contact, also along redirects (GitHub serves the
/// release files through a redirect to its file host).
const HOSTS: [&str; 4] = [
    "github.com",
    // Only for the candidate channel: the list of releases.
    "api.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 3;
/// Never more than this for a manifest or its signature.
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
/// Never more than this for an installer, whatever the manifest says.
pub const MAX_INSTALLER_BYTES: u64 = 64 * 1024 * 1024;
/// A manifest signed longer ago than this is refused ("freeze" attack).
pub const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);
/// The first check waits this long after the start (the start stays fast).
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
/// Then one check a day.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Updates can be checked: the build has the owner's public keys and runs
/// on Windows (the installer is a Windows program).
pub fn configured() -> bool {
    !PUBLIC_KEYS.is_empty() && cfg!(windows)
}

/// The version of this program, from `Cargo.toml` at build time (not from a
/// file someone could change).
pub fn current_version() -> semver::Version {
    version_with(env!("CARGO_PKG_VERSION"), option_env!("RICO_PRERELEASE"))
}

/// `0.18.0` plus the candidate suffix the release scripts give when they
/// build a candidate (`RICO_PRERELEASE=rc.2` gives `0.18.0-rc.2`), so a
/// candidate knows it is older than its final release. Only `rc.N` is
/// taken; anything else is ignored.
fn version_with(base: &str, pre: Option<&str>) -> semver::Version {
    let full = match pre {
        Some(p)
            if p.strip_prefix("rc.")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            format!("{base}-{p}")
        }
        _ => base.to_string(),
    };
    semver::Version::parse(&full).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

/// The signed description of a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Manifest {
    pub version: String,
    /// `stable` or `candidate`.
    pub channel: String,
    /// The installer's file name in the release.
    pub file: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// RFC 3339.
    pub signed_at: String,
    #[serde(default)]
    pub notes_url: Option<String>,
    /// Versions older than this have a known security flaw.
    #[serde(default)]
    pub insecure_below: Option<String>,
}

/// A manifest whose signature was checked, with its values already parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub manifest: Manifest,
    pub version: semver::Version,
    /// Seconds since 1970.
    pub signed_at: i64,
}

impl Verified {
    /// Where the installer is: built from signed values only.
    pub fn installer_url(&self) -> String {
        format!(
            "https://github.com/Trive253x/rico/releases/download/v{}/{}",
            self.manifest.version, self.manifest.file
        )
    }

    /// The release notes, only if they are on the project's GitHub.
    pub fn notes_url(&self) -> Option<&str> {
        self.manifest
            .notes_url
            .as_deref()
            .filter(|u| u.starts_with("https://github.com/Trive253x/rico/releases/"))
    }
}

/// Why a manifest or an installer is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No key of ours signed these bytes.
    BadSignature,
    /// Not a manifest we can use (missing or wrong fields).
    Malformed(String),
    /// Signed more than [`MAX_AGE`] ago, or before the newest manifest seen.
    Stale,
    /// Older than a version this installation already saw signed.
    BelowHighestSeen,
    /// A channel the user did not choose.
    OtherChannel,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => f.write_str("bad signature"),
            Self::Malformed(why) => write!(f, "malformed manifest: {why}"),
            Self::Stale => f.write_str("manifest too old"),
            Self::BelowHighestSeen => f.write_str("older than a version already seen"),
            Self::OtherChannel => f.write_str("other channel"),
        }
    }
}

/// Checks the signature of `manifest` (its exact bytes) with `keys`, then
/// its fields. `signature` is the `.sig` file: the 64-byte signature in
/// base64 (spaces and new lines around it are ignored).
pub fn verify(manifest: &[u8], signature: &[u8], keys: &[[u8; 32]]) -> Result<Verified, Refusal> {
    if manifest.len() > MAX_MANIFEST_BYTES || signature.len() > 1024 {
        return Err(Refusal::Malformed("too big".into()));
    }
    let sig_text = std::str::from_utf8(signature).map_err(|_| Refusal::BadSignature)?;
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_text.trim())
        .map_err(|_| Refusal::BadSignature)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| Refusal::BadSignature)?;
    let signed = keys.iter().any(|k| {
        VerifyingKey::from_bytes(k)
            // Strict: no malleable signatures, no weak keys.
            .is_ok_and(|key| key.verify_strict(manifest, &sig).is_ok())
    });
    if !signed {
        return Err(Refusal::BadSignature);
    }
    let m: Manifest =
        serde_json::from_slice(manifest).map_err(|e| Refusal::Malformed(e.classify_name().into()))?;
    let version = semver::Version::parse(&m.version).map_err(|_| Refusal::Malformed("version".into()))?;
    if !matches!(m.channel.as_str(), "stable" | "candidate") {
        return Err(Refusal::Malformed("channel".into()));
    }
    if m.file != format!("rico-setup-{}.exe", m.version) {
        return Err(Refusal::Malformed("file".into()));
    }
    if m.size == 0 || m.size > MAX_INSTALLER_BYTES {
        return Err(Refusal::Malformed("size".into()));
    }
    if m.sha256.len() != 64 || !m.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(Refusal::Malformed("sha256".into()));
    }
    let signed_at = chrono::DateTime::parse_from_rfc3339(&m.signed_at)
        .map_err(|_| Refusal::Malformed("signed_at".into()))?
        .timestamp();
    if let Some(v) = &m.insecure_below {
        semver::Version::parse(v).map_err(|_| Refusal::Malformed("insecure_below".into()))?;
    }
    Ok(Verified {
        manifest: m,
        version,
        signed_at,
    })
}

trait ClassifyName {
    fn classify_name(&self) -> &'static str;
}

impl ClassifyName for serde_json::Error {
    // The error kind only: serde's message can quote the payload.
    fn classify_name(&self) -> &'static str {
        match self.classify() {
            serde_json::error::Category::Io => "io",
            serde_json::error::Category::Syntax => "syntax",
            serde_json::error::Category::Data => "data",
            serde_json::error::Category::Eof => "eof",
        }
    }
}

/// What this installation remembers of the manifests it saw, so that no one
/// can make it go back (kept in `update-state.json` in the settings folder).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct State {
    /// The highest version seen in a valid signed stable manifest.
    #[serde(default)]
    pub highest_version: Option<String>,
    /// The newest `signed_at` of a stable manifest (seconds since 1970).
    #[serde(default)]
    pub newest_signed_at: i64,
    /// The same for candidates, kept apart: a candidate seen once must not
    /// block the stable releases below it for who goes back to stable.
    #[serde(default)]
    pub highest_candidate: Option<String>,
    #[serde(default)]
    pub newest_candidate_signed_at: i64,
}

impl State {
    fn highest(&self, candidate: bool) -> Option<semver::Version> {
        let v = if candidate {
            &self.highest_candidate
        } else {
            &self.highest_version
        };
        v.as_deref().and_then(|v| semver::Version::parse(v).ok())
    }

    fn newest(&self, candidate: bool) -> i64 {
        if candidate {
            self.newest_candidate_signed_at
        } else {
            self.newest_signed_at
        }
    }

    /// Remembers a manifest that passed [`evaluate`]: never goes down.
    pub fn remember(&mut self, v: &Verified) {
        let candidate = v.manifest.channel == "candidate";
        if self.highest(candidate).is_none_or(|h| v.version > h) {
            let text = Some(v.version.to_string());
            if candidate {
                self.highest_candidate = text;
            } else {
                self.highest_version = text;
            }
        }
        if candidate {
            self.newest_candidate_signed_at = self.newest_candidate_signed_at.max(v.signed_at);
        } else {
            self.newest_signed_at = self.newest_signed_at.max(v.signed_at);
        }
    }
}

/// What to do with a verified manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not newer than this program: nothing to do.
    UpToDate,
    /// A newer version to offer.
    Newer,
}

/// The rules against downgrades and old manifests, on a manifest whose
/// signature was checked:
///
/// - a candidate only if the user chose candidates;
/// - signed in the last [`MAX_AGE`], and not before the newest manifest this
///   installation saw (an old "latest" cannot be served forever);
/// - not older than the highest version this installation saw signed
///   (nobody can offer an older, flawed version in place of a newer one);
/// - offered only if strictly newer than this program (semantic versions,
///   candidates included: `0.17.0-rc.1` < `0.17.0` < `0.17.1`).
pub fn evaluate(
    v: &Verified,
    current: &semver::Version,
    state: &State,
    now: i64,
    candidates: bool,
) -> Result<Verdict, Refusal> {
    if v.manifest.channel == "candidate" && !candidates {
        return Err(Refusal::OtherChannel);
    }
    let max_age = MAX_AGE.as_secs() as i64;
    // A little room for clocks running ahead.
    if now.saturating_sub(v.signed_at) > max_age || v.signed_at > now + 24 * 3600 {
        return Err(Refusal::Stale);
    }
    let candidate = v.manifest.channel == "candidate";
    if v.signed_at < state.newest(candidate) {
        return Err(Refusal::Stale);
    }
    // A stable release below a stable one seen, or a candidate below a
    // candidate seen; and never anything below a stable release seen.
    if state.highest(candidate).is_some_and(|h| v.version < h)
        || state.highest(false).is_some_and(|h| v.version < h)
    {
        return Err(Refusal::BelowHighestSeen);
    }
    if v.version > *current {
        Ok(Verdict::Newer)
    } else {
        Ok(Verdict::UpToDate)
    }
}

/// The running version has a known security flaw, says the signed manifest.
pub fn is_insecure(v: &Verified, current: &semver::Version) -> bool {
    v.manifest
        .insecure_below
        .as_deref()
        .and_then(|s| semver::Version::parse(s).ok())
        .is_some_and(|limit| *current < limit)
}

fn state_path() -> Option<PathBuf> {
    crate::config::settings_dir().map(|d| d.join("update-state.json"))
}

/// The saved state, or an empty one.
pub fn load_state() -> State {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &State) -> Result<()> {
    let path = state_path().context("no settings folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

/// The folder where installers are downloaded (in Rico's own data folder,
/// not in a shared temporary folder).
pub fn download_dir() -> Option<PathBuf> {
    crate::config::log_dir().map(|d| d.join("updates"))
}

/// Removes the installers an update left in `dir`; returns how many.
///
/// Called once Rico has started as the updated version: only regular files
/// named `rico-setup-*.exe` or `rico-setup-*.exe.part` are touched.
pub fn remove_installers(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            e.file_name().to_str().is_some_and(|n| {
                n.starts_with("rico-setup-") && (n.ends_with(".exe") || n.ends_with(".exe.part"))
            })
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(crate::discord::USER_AGENT)
        .https_only(true)
        // Redirects are followed by hand, each one checked.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// `url` is HTTPS on one of the updater's hosts, without credentials or port.
fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.host_str().is_some_and(|h| HOSTS.contains(&h))
}

/// GET with redirects followed by hand: at most [`MAX_REDIRECTS`], each one
/// to an allowed host. No credentials are ever sent.
async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    let mut url = reqwest::Url::parse(url)?;
    for _ in 0..=MAX_REDIRECTS {
        if !allowed(&url) {
            bail!("host not allowed for updates");
        }
        let resp = client.get(url.clone()).send().await?;
        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .context("redirect without a location")?;
            url = url.join(location)?;
            continue;
        }
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status().as_u16());
        }
        return Ok(resp);
    }
    bail!("too many redirects")
}

/// Reads at most `max` bytes of the answer.
async fn read_limited(mut resp: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if resp.content_length().is_some_and(|l| l > max as u64) {
        bail!("too big");
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len() + chunk.len() > max {
            bail!("too big");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Downloads and verifies the newest manifest (stable channel).
pub async fn fetch_manifest() -> Result<Verified> {
    if PUBLIC_KEYS.is_empty() {
        bail!("updates are not set up in this build");
    }
    let client = client()?;
    let manifest = read_limited(get(&client, MANIFEST_URL).await?, MAX_MANIFEST_BYTES).await?;
    let signature = read_limited(get(&client, SIGNATURE_URL).await?, 1024).await?;
    verify(&manifest, &signature, PUBLIC_KEYS).map_err(|r| anyhow::anyhow!("{r}"))
}

/// The list of releases, for the candidate channel (only names and flags
/// are read; everything else is ignored, and the manifest found through it
/// is verified like any other).
const RELEASES_URL: &str = "https://api.github.com/repos/Trive253x/rico/releases?per_page=10";
const MAX_RELEASES_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct ReleaseInfo {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// A tag of ours: `v` and a version, nothing that could change a path.
fn valid_tag(tag: &str) -> bool {
    tag.strip_prefix('v').is_some_and(|v| {
        semver::Version::parse(v).is_ok()
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    })
}

/// The manifest of the newest candidate (pre-release), if there is one.
async fn fetch_candidate(client: &reqwest::Client) -> Result<Option<Verified>> {
    let list = read_limited(get(client, RELEASES_URL).await?, MAX_RELEASES_BYTES).await?;
    let releases: Vec<ReleaseInfo> = serde_json::from_slice(&list).context("release list")?;
    let Some(tag) = releases
        .iter()
        .find(|r| r.prerelease && !r.draft && valid_tag(&r.tag_name))
        .map(|r| r.tag_name.clone())
    else {
        return Ok(None);
    };
    let base = format!("https://github.com/Trive253x/rico/releases/download/{tag}");
    let manifest = read_limited(
        get(client, &format!("{base}/rico-update.json")).await?,
        MAX_MANIFEST_BYTES,
    )
    .await?;
    let signature = read_limited(get(client, &format!("{base}/rico-update.json.sig")).await?, 1024).await?;
    verify(&manifest, &signature, PUBLIC_KEYS)
        .map(Some)
        .map_err(|r| anyhow::anyhow!("{r}"))
}

/// The newest stable manifest and, with `candidates`, the newest candidate:
/// the higher of the two that pass the signature check.
pub async fn fetch_best(candidates: bool) -> Result<Verified> {
    let stable = fetch_manifest().await;
    if !candidates {
        return stable;
    }
    let candidate = match client() {
        Ok(client) => fetch_candidate(&client).await,
        Err(e) => Err(e),
    };
    best_of(stable, candidate)
}

/// The higher of the stable and the candidate manifest. When neither can be
/// used, the error names both channels, so a failing candidate check is not
/// hidden behind the stable one.
fn best_of(stable: Result<Verified>, candidate: Result<Option<Verified>>) -> Result<Verified> {
    match (stable, candidate) {
        (Ok(s), Ok(Some(c))) => Ok(if c.version > s.version { c } else { s }),
        (Ok(s), _) => Ok(s),
        (Err(_), Ok(Some(c))) => Ok(c),
        (Err(s), Ok(None)) => Err(anyhow::anyhow!("stable: {s:#}; candidate: none published")),
        (Err(s), Err(c)) => Err(anyhow::anyhow!("stable: {s:#}; candidate: {c:#}")),
    }
}

/// Progress of a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Bytes {
        done: u64,
        total: u64,
    },
    /// Downloaded and checked: the installer's path.
    Ready(PathBuf),
    Failed(String),
}

/// Downloads the installer of `v` into `dir`, reporting progress. The file
/// is written as `.part` and renamed only if its size and SHA-256 are the
/// signed ones; on any error nothing is left behind.
pub async fn download(v: Verified, dir: PathBuf, mut progress: impl FnMut(Progress)) {
    let result = download_inner(&v, &dir, &mut progress).await;
    match result {
        Ok(path) => progress(Progress::Ready(path)),
        Err(e) => {
            let _ = std::fs::remove_file(part_path(&dir, &v.manifest.file));
            progress(Progress::Failed(format!("{e:#}")));
        }
    }
}

fn part_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(format!("{file}.part"))
}

async fn download_inner(v: &Verified, dir: &Path, progress: &mut impl FnMut(Progress)) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;
    let total = v.manifest.size;
    tokio::fs::create_dir_all(dir).await?;
    clean_dir(dir, &v.manifest.file).await;
    let mut resp = get(&client()?, &v.installer_url()).await?;
    if resp.content_length().is_some_and(|l| l != total) {
        bail!("size differs from the signed one");
    }
    let part = part_path(dir, &v.manifest.file);
    let mut file = tokio::fs::File::create(&part).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last_reported = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        done += chunk.len() as u64;
        if done > total {
            bail!("bigger than the signed size");
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        // About every 1% (and at the end): no flood of messages.
        if done - last_reported >= total / 100 || done == total {
            last_reported = done;
            progress(Progress::Bytes { done, total });
        }
    }
    file.flush().await?;
    drop(file);
    if done != total {
        bail!("size differs from the signed one");
    }
    if hex(&hasher.finalize()) != v.manifest.sha256 {
        bail!("SHA-256 differs from the signed one");
    }
    let path = dir.join(&v.manifest.file);
    tokio::fs::rename(&part, &path).await?;
    Ok(path)
}

/// Removes old downloads: only files whose names Rico itself gives.
async fn clean_dir(dir: &Path, keep: &str) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(e)) = entries.next_entry().await {
        let name = e.file_name().to_string_lossy().to_string();
        let ours = name.starts_with("rico-setup-") && (name.ends_with(".exe") || name.ends_with(".exe.part"));
        if ours && name != keep {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// SHA-256 of what `reader` gives, in lowercase hex.
#[cfg(any(windows, test))]
fn sha256_of(mut reader: impl std::io::Read) -> Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

/// Arguments for the installer: no questions, progress window shown,
/// Rico closed if still open, and update mode (which restarts Rico and
/// refuses to go back to an older version).
pub const INSTALLER_ARGS: [&str; 5] = [
    "/SILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
    "/UPDATE=1",
];

/// Starts the downloaded installer, after checking it again while it is
/// held open without write sharing (nobody can change it between the check
/// and the start). Windows only. The caller then closes Rico.
pub fn start_installer(path: &Path, v: &Verified) -> Result<()> {
    if path.file_name().and_then(|n| n.to_str()) != Some(v.manifest.file.as_str()) {
        bail!("unexpected installer name");
    }
    start_installer_os(path, v)
}

#[cfg(windows)]
fn start_installer_os(path: &Path, v: &Verified) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only: others may read (Windows must, to start it),
    // nobody may write or delete it while we hold it.
    const FILE_SHARE_READ: u32 = 0x1;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)?;
    if file.metadata()?.len() != v.manifest.size || sha256_of(&file)? != v.manifest.sha256 {
        bail!("the installer changed after the download");
    }
    std::process::Command::new(path).args(INSTALLER_ARGS).spawn()?;
    // Held until the installer process exists.
    drop(file);
    Ok(())
}

#[cfg(not(windows))]
fn start_installer_os(_path: &Path, _v: &Verified) -> Result<()> {
    bail!("updates are installed only on Windows")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A key for the tests only (never a release key).
    pub(crate) fn test_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub(crate) fn manifest_json(version: &str, channel: &str, signed_at: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "version": version,
            "channel": channel,
            "file": format!("rico-setup-{version}.exe"),
            "size": 1234,
            "sha256": "ab".repeat(32),
            "signed_at": signed_at,
            "notes_url": format!("https://github.com/Trive253x/rico/releases/tag/v{version}"),
        }))
        .unwrap()
    }

    pub(crate) fn sign(key: &SigningKey, bytes: &[u8]) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .encode(key.sign(bytes).to_bytes())
            .into_bytes()
    }

    fn now() -> i64 {
        chrono::DateTime::parse_from_rfc3339("2026-11-18T12:00:00Z")
            .unwrap()
            .timestamp()
    }

    #[test]
    fn old_installers_are_removed_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("rico-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rico-setup-dir.exe")).unwrap();
        for name in [
            "rico-setup-0.18.0-rc.2.exe",
            "rico-setup-0.18.0.exe.part",
            "notes.txt",
            "other.exe",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert_eq!(remove_installers(&dir), 2);
        assert!(!dir.join("rico-setup-0.18.0-rc.2.exe").exists());
        assert!(!dir.join("rico-setup-0.18.0.exe.part").exists());
        assert!(dir.join("notes.txt").exists());
        assert!(dir.join("other.exe").exists());
        assert!(dir.join("rico-setup-dir.exe").is_dir());
        assert_eq!(remove_installers(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_channels_failing_are_both_reported() {
        let e = best_of(Err(anyhow::anyhow!("HTTP 404")), Err(anyhow::anyhow!("HTTP 403"))).unwrap_err();
        let text = format!("{e:#}");
        assert!(text.contains("stable: HTTP 404"), "{text}");
        assert!(text.contains("candidate: HTTP 403"), "{text}");
        let e = best_of(Err(anyhow::anyhow!("HTTP 404")), Ok(None)).unwrap_err();
        assert!(format!("{e:#}").contains("candidate: none"));
        // A working channel still wins over a failing one, and the higher version wins.
        let c = verified("0.18.0-rc.2", "candidate", "2026-11-18T10:00:00Z");
        let got = best_of(Err(anyhow::anyhow!("HTTP 404")), Ok(Some(c))).unwrap();
        assert_eq!(got.manifest.version, "0.18.0-rc.2");
        let s = verified("0.18.0", "stable", "2026-11-18T10:00:00Z");
        let c = verified("0.18.0-rc.2", "candidate", "2026-11-18T10:00:00Z");
        let got = best_of(Ok(s), Ok(Some(c))).unwrap();
        assert_eq!(got.manifest.version, "0.18.0");
        let s = verified("0.17.0", "stable", "2026-11-18T10:00:00Z");
        let got = best_of(Ok(s), Err(anyhow::anyhow!("HTTP 403"))).unwrap();
        assert_eq!(got.manifest.version, "0.17.0");
    }

    fn verified(version: &str, channel: &str, signed_at: &str) -> Verified {
        let key = test_key(7);
        let m = manifest_json(version, channel, signed_at);
        verify(&m, &sign(&key, &m), &[key.verifying_key().to_bytes()]).unwrap()
    }

    #[test]
    fn only_our_signature_is_accepted() {
        let ours = test_key(7);
        let other = test_key(9);
        let keys = [ours.verifying_key().to_bytes()];
        let m = manifest_json("0.18.0", "stable", "2026-11-18T10:00:00Z");
        assert!(verify(&m, &sign(&ours, &m), &keys).is_ok());
        // The spare key works too.
        let both = [other.verifying_key().to_bytes(), ours.verifying_key().to_bytes()];
        assert!(verify(&m, &sign(&ours, &m), &both).is_ok());
        // Someone else's key, a changed byte, a broken or empty signature.
        assert_eq!(verify(&m, &sign(&other, &m), &keys), Err(Refusal::BadSignature));
        let mut changed = m.clone();
        let pos = changed.iter().position(|b| *b == b'8').unwrap();
        changed[pos] = b'9';
        assert_eq!(
            verify(&changed, &sign(&ours, &m), &keys),
            Err(Refusal::BadSignature)
        );
        assert_eq!(verify(&m, b"not base64!", &keys), Err(Refusal::BadSignature));
        assert_eq!(verify(&m, b"", &keys), Err(Refusal::BadSignature));
        // No keys: nothing is ever accepted.
        assert_eq!(verify(&m, &sign(&ours, &m), &[]), Err(Refusal::BadSignature));
    }

    #[test]
    fn signed_but_unusable_manifests() {
        let key = test_key(7);
        let keys = [key.verifying_key().to_bytes()];
        let check = |v: serde_json::Value| {
            let m = serde_json::to_vec(&v).unwrap();
            verify(&m, &sign(&key, &m), &keys)
        };
        let good = serde_json::json!({
            "version": "0.18.0", "channel": "stable", "file": "rico-setup-0.18.0.exe",
            "size": 10, "sha256": "ab".repeat(32), "signed_at": "2026-11-18T10:00:00Z"
        });
        assert!(check(good.clone()).is_ok());
        for (field, value) in [
            ("version", serde_json::json!("not-a-version")),
            ("channel", serde_json::json!("nightly")),
            ("file", serde_json::json!("../evil.exe")),
            ("file", serde_json::json!("rico-setup-0.17.0.exe")),
            ("size", serde_json::json!(0)),
            ("size", serde_json::json!(MAX_INSTALLER_BYTES + 1)),
            ("sha256", serde_json::json!("AB".repeat(32))),
            ("sha256", serde_json::json!("ab")),
            ("signed_at", serde_json::json!("yesterday")),
        ] {
            let mut v = good.clone();
            v[field] = value;
            assert!(matches!(check(v), Err(Refusal::Malformed(_))), "{field}");
        }
        let mut v = good;
        v.as_object_mut().unwrap().remove("sha256");
        assert!(matches!(check(v), Err(Refusal::Malformed(_))));
        // Too big to even look at.
        let huge = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(matches!(
            verify(&huge, &sign(&key, &huge), &keys),
            Err(Refusal::Malformed(_))
        ));
    }

    #[test]
    fn no_downgrades() {
        let current = semver::Version::parse("0.17.0").unwrap();
        let fresh = "2026-11-18T10:00:00Z";
        let state = State::default();
        // Newer: offered; equal or older: nothing to do.
        assert_eq!(
            evaluate(
                &verified("0.18.0", "stable", fresh),
                &current,
                &state,
                now(),
                false
            ),
            Ok(Verdict::Newer)
        );
        assert_eq!(
            evaluate(
                &verified("0.17.1", "stable", fresh),
                &current,
                &state,
                now(),
                false
            ),
            Ok(Verdict::Newer)
        );
        for old in ["0.17.0", "0.16.0", "0.17.0-rc.1"] {
            let channel = if old.contains("rc") { "candidate" } else { "stable" };
            assert_eq!(
                evaluate(&verified(old, channel, fresh), &current, &state, now(), true),
                Ok(Verdict::UpToDate),
                "{old}"
            );
        }
        // Candidates order before their release.
        let rc = semver::Version::parse("0.18.0-rc.1").unwrap();
        assert_eq!(
            evaluate(&verified("0.18.0", "stable", fresh), &rc, &state, now(), false),
            Ok(Verdict::Newer)
        );
        // A version below the highest one seen is refused, even if newer
        // than the program.
        let mut seen = State::default();
        seen.remember(&verified("0.19.0", "stable", fresh));
        assert_eq!(
            evaluate(
                &verified("0.18.0", "stable", fresh),
                &current,
                &seen,
                now(),
                false
            ),
            Err(Refusal::BelowHighestSeen)
        );
        assert_eq!(
            evaluate(
                &verified("0.19.0", "stable", fresh),
                &current,
                &seen,
                now(),
                false
            ),
            Ok(Verdict::Newer)
        );
    }

    #[test]
    fn old_manifests_and_channels() {
        let current = semver::Version::parse("0.17.0").unwrap();
        let state = State::default();
        // Older than 90 days, or from the future.
        assert_eq!(
            evaluate(
                &verified("0.18.0", "stable", "2026-01-01T00:00:00Z"),
                &current,
                &state,
                now(),
                false
            ),
            Err(Refusal::Stale)
        );
        assert_eq!(
            evaluate(
                &verified("0.18.0", "stable", "2027-01-01T00:00:00Z"),
                &current,
                &state,
                now(),
                false
            ),
            Err(Refusal::Stale)
        );
        // Signed before the newest manifest seen.
        let mut seen = State::default();
        seen.remember(&verified("0.18.0", "stable", "2026-11-18T10:00:00Z"));
        assert_eq!(
            evaluate(
                &verified("0.18.0", "stable", "2026-11-10T10:00:00Z"),
                &current,
                &seen,
                now(),
                false
            ),
            Err(Refusal::Stale)
        );
        // Candidates only for who chose them.
        let rc = verified("0.18.0-rc.1", "candidate", "2026-11-18T10:00:00Z");
        assert_eq!(
            evaluate(&rc, &current, &state, now(), false),
            Err(Refusal::OtherChannel)
        );
        assert_eq!(evaluate(&rc, &current, &state, now(), true), Ok(Verdict::Newer));
    }

    #[test]
    fn the_state_never_goes_down() {
        let mut s = State::default();
        s.remember(&verified("0.19.0", "stable", "2026-11-18T10:00:00Z"));
        s.remember(&verified("0.18.0", "stable", "2026-11-10T10:00:00Z"));
        assert_eq!(s.highest_version.as_deref(), Some("0.19.0"));
        assert_eq!(
            s.newest_signed_at,
            chrono::DateTime::parse_from_rfc3339("2026-11-18T10:00:00Z")
                .unwrap()
                .timestamp()
        );
        // Saved and read back as it was.
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<State>(&text).unwrap(), s);
        // A broken file is an empty state, never an error that stops Rico.
        assert_eq!(serde_json::from_str::<State>("{}").unwrap(), State::default());
    }

    #[test]
    fn known_flawed_versions() {
        let key = test_key(7);
        let mut v: serde_json::Value =
            serde_json::from_slice(&manifest_json("0.18.0", "stable", "2026-11-18T10:00:00Z")).unwrap();
        v["insecure_below"] = serde_json::json!("0.17.2");
        let m = serde_json::to_vec(&v).unwrap();
        let checked = verify(&m, &sign(&key, &m), &[key.verifying_key().to_bytes()]).unwrap();
        assert!(is_insecure(&checked, &semver::Version::parse("0.17.0").unwrap()));
        assert!(!is_insecure(&checked, &semver::Version::parse("0.17.2").unwrap()));
        v["insecure_below"] = serde_json::json!("soon");
        let m = serde_json::to_vec(&v).unwrap();
        assert!(matches!(
            verify(&m, &sign(&key, &m), &[key.verifying_key().to_bytes()]),
            Err(Refusal::Malformed(_))
        ));
    }

    #[test]
    fn urls_come_from_signed_values_and_hosts_are_checked() {
        let v = verified("0.18.0", "stable", "2026-11-18T10:00:00Z");
        assert_eq!(
            v.installer_url(),
            "https://github.com/Trive253x/rico/releases/download/v0.18.0/rico-setup-0.18.0.exe"
        );
        assert!(v.notes_url().is_some());
        for (url, ok) in [
            (MANIFEST_URL, true),
            ("https://release-assets.githubusercontent.com/x", true),
            ("https://objects.githubusercontent.com/x", true),
            ("http://github.com/x", false),
            ("https://evil.example/x", false),
            ("https://github.com.evil.example/x", false),
            ("https://user:pass@github.com/x", false),
            ("https://github.com:8443/x", false),
            ("https://127.0.0.1/x", false),
            ("https://api.github.com/repos/Trive253x/rico/releases", true),
        ] {
            assert_eq!(allowed(&reqwest::Url::parse(url).unwrap()), ok, "{url}");
        }
    }

    #[test]
    fn only_the_signed_installer_name_is_started() {
        let v = verified("0.18.0", "stable", "2026-11-18T10:00:00Z");
        assert!(start_installer(Path::new("/tmp/other.exe"), &v).is_err());
    }

    #[test]
    fn candidate_builds_know_their_suffix() {
        assert_eq!(version_with("0.18.0", None).to_string(), "0.18.0");
        assert_eq!(version_with("0.18.0", Some("rc.2")).to_string(), "0.18.0-rc.2");
        // Only rc.N is taken.
        for bad in ["beta", "rc.", "rc.x", "rc.1+evil", "-rc.1"] {
            assert_eq!(version_with("0.18.0", Some(bad)).to_string(), "0.18.0", "{bad}");
        }
        // A candidate build gets its final release and the next candidate.
        let rc1 = version_with("0.18.0", Some("rc.1"));
        let state = State::default();
        let fresh = "2026-11-18T10:00:00Z";
        assert_eq!(
            evaluate(&verified("0.18.0", "stable", fresh), &rc1, &state, now(), false),
            Ok(Verdict::Newer)
        );
        assert_eq!(
            evaluate(
                &verified("0.18.0-rc.2", "candidate", fresh),
                &rc1,
                &state,
                now(),
                true
            ),
            Ok(Verdict::Newer)
        );
    }

    #[test]
    fn candidates_do_not_block_stable_releases() {
        let current = semver::Version::parse("0.18.0").unwrap();
        let fresh = "2026-11-18T10:00:00Z";
        let mut seen = State::default();
        // A candidate of 0.20.0 seen while candidates were on...
        seen.remember(&verified("0.20.0-rc.1", "candidate", fresh));
        // ...does not stop the stable 0.19.0 after going back to stable.
        assert_eq!(
            evaluate(
                &verified("0.19.0", "stable", fresh),
                &current,
                &seen,
                now(),
                false
            ),
            Ok(Verdict::Newer)
        );
        // But an older candidate than the one seen is refused, and no
        // candidate below a stable release seen.
        assert_eq!(
            evaluate(
                &verified("0.19.0-rc.1", "candidate", fresh),
                &current,
                &seen,
                now(),
                true
            ),
            Err(Refusal::BelowHighestSeen)
        );
        seen.remember(&verified("0.19.0", "stable", fresh));
        assert_eq!(
            evaluate(
                &verified("0.19.0-rc.2", "candidate", fresh),
                &current,
                &seen,
                now(),
                true
            ),
            Err(Refusal::BelowHighestSeen)
        );
        assert!(valid_tag("v0.18.0-rc.1") && valid_tag("v0.18.0"));
        for bad in ["0.18.0", "v0.18", "v0.18.0/../x", "v0.18.0?x", "v0.18.0-rc.1%2f"] {
            assert!(!valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_of(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(!configured() || cfg!(windows));
    }
}

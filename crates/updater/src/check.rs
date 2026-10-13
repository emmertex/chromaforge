//! The update check itself: read the release feed, compare it with this build, pick the file.
//!
//! Nothing is downloaded here ([`crate::download`]) and nothing is installed ([`crate::install`]);
//! a check is one small HTTPS request that returns an [`Update`] describing what is newer, or
//! `None` when this build is current.
//!
//! Where the feed comes from, in order:
//! 1. the body the caller already has ([`check`]'s `feed`, which the engine fetches off the UI
//!    thread and caches);
//! 2. [`Options::manifest_url`] (the `<PREFIX>_UPDATE_URL` override, or a build that pins one);
//! 3. the static `latest.json` published on the newest release;
//! 4. GitHub's Releases API for the brand's repository ([`lcc_brand::REPOSITORY`]).
//!
//! The API needs no token for a public repository, but it is rate-limited per IP; a 403 there is
//! reported as such, and the static manifest is tried first because it is a plain file on the
//! release and keeps working when the API is rate-limited.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lcc_net::{Client, ClientConfig};

use crate::error::UpdateError;
use crate::release::{self, Release};
use crate::target::{Asset, Target};
use crate::version::Version;

/// Feed bodies are small; anything larger is not our release list.
const MAX_FEED_BYTES: u64 = 4 * 1024 * 1024;
/// Downloads can be a disk image or an installer: generous, but not unbounded.
pub const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// How long a finished check answers a repeat question without asking again.
pub const REUSE_AFTER: Duration = Duration::from_secs(20);

/// What a check was told to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Repository to ask, `owner/name`.
    pub repo: String,
    /// The build to compare against (usually [`Version::current`]).
    pub current: Version,
    /// The machine's platform; `None` when we have no update for it.
    pub target: Option<Target>,
    /// Include prereleases (`-rc.1`) in what counts as newer.
    pub prerelease: bool,
    /// A feed URL that replaces the GitHub API (the `<PREFIX>_UPDATE_URL` override, or the static
    /// manifest a build was pointed at).
    pub manifest_url: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            repo: lcc_brand::REPOSITORY.to_string(),
            current: Version::current(),
            target: Target::current(),
            prerelease: false,
            manifest_url: override_url(),
        }
    }
}

/// `<PREFIX>_UPDATE_URL`: point the updater at another feed (a local server, a mirror, a fork).
/// Read through the brand, so the legacy prefix still works.
pub fn override_url() -> Option<String> {
    lcc_brand::env("UPDATE_URL").map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A newer release, and the file this machine would install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// What is running now.
    pub current: Version,
    /// The newer release.
    pub version: Version,
    /// The release title (or its tag).
    pub title: String,
    pub tag: String,
    /// The asset to download.
    pub asset: Asset,
    /// SHA-256 to check the download against, when the release publishes one.
    pub sha256: Option<String>,
    /// `YYYY-MM-DD`, when the feed says.
    pub published: Option<String>,
    /// The release notes (Markdown).
    pub notes: String,
    /// One line on what installing it will do on this machine.
    pub action: String,
}

impl Update {
    /// The version as a dialog shows it (`1.2.3`).
    pub fn version_text(&self) -> String {
        self.version.to_text()
    }
}

/// Check for an update: `Ok(None)` when this build is the newest the feed knows of.
///
/// `feed` is a release feed body the caller already has (the engine fetches and caches it, so a
/// check costs no request); `None` fetches it from [`Options::manifest_url`], the release's
/// `latest.json`, or the API.
pub fn check(opts: &Options, feed: Option<&str>, cancel: Option<Arc<AtomicBool>>) -> Result<Option<Update>, UpdateError> {
    let Some(target) = opts.target else {
        return Err(UpdateError::Unsupported(format!(
            "there is no update for this build ({} {}); the releases page has the files for the platforms we ship",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };
    let body = match feed {
        Some(text) => text.to_string(),
        None => fetch_feed(opts, cancel.clone())?,
    };
    let release = release::newest(&body, opts.prerelease)?;
    from_release(opts, target, release, cancel)
}

/// [`check`] for a release already parsed (a test, or a caller that cached the parse).
pub fn from_release(opts: &Options, target: Target, release: Release, cancel: Option<Arc<AtomicBool>>) -> Result<Option<Update>, UpdateError> {
    if release.version <= opts.current {
        return Ok(None);
    }
    let Some(asset) = release.best_asset(&target).cloned() else {
        return Err(UpdateError::NoAssetFor { version: release.version.to_text(), target: format!("{}-{}", target.os.slug(), target.arch) });
    };
    // the release's own checksum list, when it is not already in the feed entry
    let sha256 = match &asset.sha256 {
        Some(sha) => Some(sha.clone()),
        None => match release.checksum_asset() {
            Some(sums) => fetch_checksum(&sums.url, &asset.name, cancel)?,
            None => None,
        },
    };
    Ok(Some(Update {
        current: opts.current.clone(),
        version: release.version.clone(),
        title: release.title.clone(),
        tag: release.tag.clone(),
        asset,
        sha256,
        published: release.published.clone(),
        notes: release.notes.clone(),
        action: target.describe(),
    }))
}

/// The feed body: the override URL, else the static manifest on the newest release, else the API.
fn fetch_feed(opts: &Options, cancel: Option<Arc<AtomicBool>>) -> Result<String, UpdateError> {
    if let Some(url) = &opts.manifest_url {
        return get_text(url, cancel);
    }
    match get_text(&manifest_url(&opts.repo), cancel.clone()) {
        Ok(body) => Ok(body),
        // no manifest on that release (an older one, or none published yet): ask the API
        Err(UpdateError::Http { code: 404, .. }) => get_text(&api_url(&opts.repo), cancel),
        Err(e) => Err(e),
    }
}

/// `owner/name` from either `owner/name` or a GitHub URL (`https://github.com/owner/name[.git]`),
/// which is how `brand.toml` spells the repository.
fn owner_name(repo: &str) -> &str {
    let r = repo.trim();
    let r = ["https://github.com/", "http://github.com/", "github.com/"].iter().find_map(|p| r.strip_prefix(p)).unwrap_or(r);
    let r = r.trim_matches('/');
    r.strip_suffix(".git").unwrap_or(r)
}

/// `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`.
pub fn manifest_url(repo: &str) -> String {
    format!("https://github.com/{}/releases/latest/download/latest.json", owner_name(repo))
}

/// `https://api.github.com/repos/<owner>/<repo>/releases?per_page=30`.
pub fn api_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{}/releases?per_page=30", owner_name(repo))
}

fn client() -> Result<Client, UpdateError> {
    Client::new(ClientConfig {
        max_body: MAX_FEED_BYTES,
        connect_timeout: Duration::from_secs(8),
        stall_timeout: Duration::from_secs(20),
        ..ClientConfig::default()
    })
    .map_err(|e| UpdateError::Net(e.to_string()))
}

/// GET a small text body.
fn get_text(url: &str, cancel: Option<Arc<AtomicBool>>) -> Result<String, UpdateError> {
    let client = client()?;
    let mut req = client.get(url).header("Accept", "application/vnd.github+json, application/json");
    if let Some(flag) = cancel {
        req = req.cancel(flag);
    }
    let resp = req.send()?;
    let status = resp.status;
    let body = resp.text()?;
    if status == 403 || status == 429 {
        // never the body: it can be a whole HTML page
        let what = if body.contains("rate limit") { "GitHub's rate limit was reached; try again later" } else { "the feed refused the request" };
        return Err(UpdateError::Http { code: status, what: what.to_string() });
    }
    if !(200..300).contains(&status) {
        return Err(UpdateError::Http { code: status, what: "the release feed was not found".into() });
    }
    Ok(body)
}

/// The release's checksum list: the SHA-256 for `file`, when it lists it.
fn fetch_checksum(url: &str, file: &str, cancel: Option<Arc<AtomicBool>>) -> Result<Option<String>, UpdateError> {
    // a checksum that cannot be fetched is not a failure: the download is still TLS-verified and
    // size-checked, and saying "no update" because of a missing sums file would be worse
    match get_text(url, cancel) {
        Ok(body) => Ok(crate::checksum::lookup(&body, file)),
        Err(e) => {
            log::debug!("update: checksum list: {e}");
            Ok(None)
        }
    }
}

/// The result of one check, for the UI and `update.status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// A newer release is available.
    Found(Box<Update>),
    /// This build is the newest the feed knows of.
    Current(String),
    /// The check failed.
    Failed(String),
}

/// A memo of the last check, so a dialog opened twice does not ask GitHub twice, and only one
/// check runs at a time.
#[derive(Default)]
pub struct Cache {
    last: Mutex<Option<(std::time::Instant, Options, Checked)>>,
    running: AtomicBool,
}

impl Cache {
    /// The last result, when it is recent and was asked with the same options.
    pub fn get(&self, opts: &Options) -> Option<Checked> {
        let guard = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.as_ref() {
            Some((when, had, result)) if had == opts && when.elapsed() < REUSE_AFTER => Some(result.clone()),
            _ => None,
        }
    }

    /// Record a result.
    pub fn put(&self, opts: &Options, result: Checked) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some((std::time::Instant::now(), opts.clone(), result));
    }

    /// Claim the single check slot (`false`: another one is already running).
    pub fn begin(&self) -> bool {
        !self.running.swap(true, Ordering::SeqCst)
    }

    /// Release the single check slot.
    pub fn end(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Os;

    fn opts() -> Options {
        Options {
            // the real repository, so the URL helpers are checked against what we publish to
            repo: lcc_brand::REPOSITORY.to_string(),
            current: Version::parse("1.0.0").expect("a version"),
            target: Some(Target { os: Os::Windows, arch: "x64" }),
            prerelease: false,
            manifest_url: None,
        }
    }

    /// The feed URLs point at the repository the app is built from, whether brand.toml spells it
    /// as a URL (as it does) or as `owner/name` (the bug: a URL gave `github.com/https://…`, a 404).
    #[test]
    fn feed_urls_name_the_repository_once() {
        for repo in [lcc_brand::REPOSITORY, "emmertex/x", "https://github.com/emmertex/x/", "https://github.com/emmertex/x.git"] {
            for url in [manifest_url(repo), api_url(repo)] {
                assert_eq!(url.matches("github.com").count(), 1, "{url}");
                assert!(!url.contains("//github.com/https") && !url.contains("repos/https"), "{url}");
            }
        }
        let own = lcc_brand::REPOSITORY.trim_end_matches('/');
        assert_eq!(manifest_url(lcc_brand::REPOSITORY), format!("{own}/releases/latest/download/latest.json"));
    }

    fn feed(version: &str) -> String {
        serde_json::json!([{
            "tag_name": format!("v{version}"),
            "name": format!("App v{version}"),
            "body": "notes",
            "published_at": "2026-10-12T00:00:00Z",
            "assets": [
                {"name": format!("app-{version}-windows-x64.msi"), "browser_download_url": "https://x/a.msi", "size": 100},
            ],
        }])
        .to_string()
    }

    fn run(opts: &Options, body: &str) -> Result<Option<Update>, UpdateError> {
        check(opts, Some(body), None)
    }

    #[test]
    fn finds_a_newer_release_and_picks_the_file() {
        let u = run(&opts(), &feed("1.2.0")).expect("a check").expect("an update");
        assert_eq!((u.current.to_text().as_str(), u.version_text().as_str()), ("1.0.0", "1.2.0"));
        assert_eq!(u.asset.name, "app-1.2.0-windows-x64.msi");
        assert_eq!(u.asset.size, 100);
        assert_eq!(u.action, "run the 64-bit installer and then start the app again");
        assert_eq!(u.title, "App v1.2.0");
        assert_eq!(u.published.as_deref(), Some("2026-10-12"));
        assert_eq!(u.notes, "notes");
        assert_eq!(u.tag, "v1.2.0");
        assert_eq!(u.sha256, None, "this release publishes no sums file");
    }

    #[test]
    fn the_same_version_or_an_older_one_is_not_an_update() {
        assert!(run(&opts(), &feed("1.0.0")).expect("a check").is_none());
        assert!(run(&opts(), &feed("0.9.9")).expect("a check").is_none());
        // a development build ahead of the newest release stays quiet
        let ahead = Options { current: Version::parse("1.3.0").expect("v"), ..opts() };
        assert!(run(&ahead, &feed("1.2.0")).expect("a check").is_none());
    }

    #[test]
    fn a_release_without_our_file_is_reported() {
        let mac = Options { target: Some(Target { os: Os::Macos, arch: "universal" }), ..opts() };
        let err = run(&mac, &feed("1.2.0")).expect_err("no dmg");
        assert!(matches!(err, UpdateError::NoAssetFor { .. }), "{err}");
        assert!(err.to_string().contains("macos-universal"), "{err}");
    }

    #[test]
    fn a_platform_we_do_not_build_for_says_so() {
        let none = Options { target: None, ..opts() };
        let err = run(&none, &feed("1.2.0")).expect_err("no platform");
        assert!(matches!(err, UpdateError::Unsupported(_)), "{err}");
    }

    #[test]
    fn urls_are_what_github_serves() {
        assert_eq!(manifest_url("a/b"), "https://github.com/a/b/releases/latest/download/latest.json");
        assert_eq!(manifest_url("/a/b/"), "https://github.com/a/b/releases/latest/download/latest.json");
        assert_eq!(api_url("a/b"), "https://api.github.com/repos/a/b/releases?per_page=30");
    }

    #[test]
    fn the_cache_answers_again_and_claims_one_check() {
        let c = Cache::default();
        let o = opts();
        assert!(c.get(&o).is_none());
        c.put(&o, Checked::Current("1.0.0".into()));
        assert_eq!(c.get(&o), Some(Checked::Current("1.0.0".into())));
        let other = Options { prerelease: true, ..opts() };
        assert!(c.get(&other).is_none(), "different options: a different question");
        assert!(c.begin(), "the first check takes the slot");
        assert!(!c.begin(), "a second one waits");
        c.end();
        assert!(c.begin());
    }

    #[test]
    fn defaults_come_from_the_brand() {
        let o = Options::default();
        assert_eq!(o.repo, lcc_brand::REPOSITORY);
        assert_eq!(o.current, Version::current());
        assert_eq!(o.target, Target::current());
        assert!(!o.prerelease);
    }

    #[test]
    fn a_checksums_asset_pins_the_download() {
        let body = serde_json::json!([{
            "tag_name": "v1.2.0",
            "assets": [
                {"name": "app-1.2.0-windows-x64.msi", "browser_download_url": "https://x/a.msi", "size": 100},
                {"name": "SHA256SUMS.txt", "browser_download_url": "http://127.0.0.1:1/sums", "size": 40},
            ],
        }])
        .to_string();
        // the sums file is unreachable here: the update still comes back, without a hash
        let u = run(&opts(), &body).expect("a check").expect("an update");
        assert_eq!(u.sha256, None);
        assert_eq!(u.asset.kind, crate::target::Kind::Msi);
    }
}

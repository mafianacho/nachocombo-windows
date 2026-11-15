//! In-app updates from the project's GitHub Releases, the decisions only (ported from the macOS `Updates.swift`):
//! version order, what to download from a release, where downloads may come from, checking a download against the
//! release's `SHA256SUMS.txt` and its listed size, the release-note summary and when the next automatic check is due.
//! The files and the swap are in `update_store`; the network and the dialogs are the app's.
use super::sha256;
use serde::Deserialize;
use std::cmp::Ordering;

/// The repository the updater reads. Public: `GET /repos/{this}/releases/latest` needs no sign-in.
pub const REPO: &str = "mafianacho/nachocombo-windows";
pub const LATEST_URL: &str = "https://api.github.com/repos/mafianacho/nachocombo-windows/releases/latest";
pub const CHECKSUMS: &str = "SHA256SUMS.txt";
pub const EXE: &str = "NachoCombo.exe";
/// Nothing in a release is anywhere near this big; a larger download or unpacked exe is refused.
pub const MAX_DOWNLOAD: usize = 64 << 20;

/// The zip the release workflow builds: `NachoCombo-windows-<version>-x64.zip` (the exe, README, licenses).
pub fn zip_name(v: &Version) -> String { format!("NachoCombo-windows-{v}-x64.zip") }

/// MAJOR.MINOR.PATCH with an optional pre-release part ("0.4.0", "v0.4.0", "1.0.0-beta.1"). Build metadata after "+"
/// is ignored. A pre-release sorts before the same version without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version { pub major: u64, pub minor: u64, pub patch: u64, pub pre: Option<String> }

impl Version {
    /// None when the text is not a version (a leading "v" is allowed, "0.4" is not: releases always have three parts).
    pub fn parse(text: &str) -> Option<Version> {
        let mut raw = text.trim();
        if let Some(r) = raw.strip_prefix(['v', 'V']) { raw = r; }
        if let Some(plus) = raw.find('+') { raw = &raw[..plus]; }
        let (core, pre) = match raw.find('-') {
            Some(d) => { let p = &raw[d + 1..]; if p.is_empty() { return None; } (&raw[..d], Some(p.to_string())) }
            None => (raw, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 { return None; }
        let mut n = [0u64; 3];
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) { return None; }
            n[i] = p.parse().ok()?;
        }
        Some(Version { major: n[0], minor: n[1], patch: n[2], pre })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre { write!(f, "-{p}")?; }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| match (&self.pre, &o.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => compare_pre(a, b),
        })
    }
}
impl PartialOrd for Version { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }

/// SemVer 2.0 §11: dot-separated identifiers, numbers compare numerically and sort before words.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.split('.').collect(), b.split('.').collect());
    for (p, q) in x.iter().zip(&y) {
        if p == q { continue; }
        return match (p.parse::<u64>(), q.parse::<u64>()) {
            (Ok(m), Ok(n)) => m.cmp(&n),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => p.cmp(q),
        };
    }
    x.len().cmp(&y.len())
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    pub size: u64,
}

/// The part of GitHub's `GET /repos/{owner}/{repo}/releases/latest` answer the updater uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub page: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn parse(data: &[u8]) -> Option<Release> {
        let v: serde_json::Value = serde_json::from_slice(data).ok()?;
        let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
        // One odd asset must not hide the others.
        let assets = v.get("assets").and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| serde_json::from_value::<Asset>(x.clone()).ok()).collect()).unwrap_or_default();
        Some(Release {
            tag: text("tag_name")?,
            name: text("name"),
            body: text("body"),
            page: text("html_url"),
            draft: v.get("draft").and_then(|x| x.as_bool()).unwrap_or(false),
            prerelease: v.get("prerelease").and_then(|x| x.as_bool()).unwrap_or(false),
            assets,
        })
    }
    pub fn version(&self) -> Option<Version> { Version::parse(&self.tag) }
}

/// `SHA256SUMS.txt` as written by `sha256sum`: "<64 hex>  <file>" (text mode) or "<64 hex> *<file>" (binary mode).
/// Blank lines are skipped; any other line that is not a checksum line makes the whole file invalid, so a damaged
/// file is never half-trusted. File name → lowercase hex digest.
pub fn parse_sums(text: &str) -> Option<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = vec![];
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() { continue; }
        let b = line.as_bytes();
        if b.len() <= 66 || !b[..64].iter().all(|c| c.is_ascii_hexdigit()) || b[64] != b' ' || !(b[65] == b' ' || b[65] == b'*') { return None; }
        let name = &line[66..];
        if name.contains('/') || name.contains('\\') { return None; }
        if out.iter().any(|(n, _)| n == name) { return None; }
        out.push((name.to_string(), line[..64].to_ascii_lowercase()));
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// This copy is the newest (or newer: a development build).
    UpToDate,
    /// A newer release with both the zip and its checksums.
    Available { version: Version, zip: Asset, sums: Asset },
    /// Not something to offer (draft, pre-release, odd tag, missing files, a version this PC went back from):
    /// ignored without bothering the user.
    Ignored(String),
}

/// What a check found. `skipped`: versions this PC rolled back from, never offered again.
pub fn decide(current: &str, release: &Release, skipped: &[String]) -> Decision {
    if release.draft { return Decision::Ignored("draft".into()); }
    if release.prerelease { return Decision::Ignored("pre-release".into()); }
    let Some(latest) = release.version().filter(|v| v.pre.is_none()) else { return Decision::Ignored(format!("tag {} is not a version", release.tag)) };
    let Some(mine) = Version::parse(current) else { return Decision::Ignored(format!("this copy's version {current} is not a version")) };
    if mine >= latest { return Decision::UpToDate; }
    if skipped.iter().any(|s| Version::parse(s).as_ref() == Some(&latest)) { return Decision::Ignored(format!("{latest} was rolled back on this PC")); }
    let want = zip_name(&latest);
    let Some(zip) = release.assets.iter().find(|a| a.name == want).cloned() else { return Decision::Ignored(format!("no {want} in the release")) };
    let Some(sums) = release.assets.iter().find(|a| a.name == CHECKSUMS).cloned() else { return Decision::Ignored(format!("no {CHECKSUMS} in the release")) };
    Decision::Available { version: latest, zip, sums }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VerifyProblem {
    BadChecksumsFile,
    NotListed(String),
    /// The download's length is not the size the release lists (cut short, or something else was served).
    WrongSize { expected: u64, actual: u64 },
    Mismatch { expected: String, actual: String },
}

impl std::fmt::Display for VerifyProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyProblem::BadChecksumsFile => write!(f, "{CHECKSUMS} of the release could not be read."),
            VerifyProblem::NotListed(n) => write!(f, "{n} is not listed in {CHECKSUMS}."),
            VerifyProblem::WrongSize { expected, actual } => write!(f, "The download is {actual} bytes, the release lists {expected}."),
            VerifyProblem::Mismatch { .. } => write!(f, "The download does not match {CHECKSUMS}."),
        }
    }
}

/// The downloaded zip must have the size the release lists and be in its checksums with the same SHA-256, or nothing
/// is installed.
pub fn verify(data: &[u8], zip: &Asset, sums: &str) -> Result<(), VerifyProblem> {
    if data.len() as u64 != zip.size { return Err(VerifyProblem::WrongSize { expected: zip.size, actual: data.len() as u64 }); }
    let sums = parse_sums(sums).ok_or(VerifyProblem::BadChecksumsFile)?;
    let expected = sums.into_iter().find(|(n, _)| *n == zip.name).map(|(_, d)| d).ok_or_else(|| VerifyProblem::NotListed(zip.name.clone()))?;
    let actual = sha256::hex(data);
    if actual != expected { return Err(VerifyProblem::Mismatch { expected, actual }); }
    Ok(())
}

/// Where a feed and its downloads may come from. The real feed is GitHub over HTTPS, and its files must be this
/// repository's release downloads; a test feed (`--update-feed` / the round trip) may only be on this PC, and so must
/// its files be. Anything else is refused before a byte is fetched.
pub fn feed_allowed(url: &str) -> bool { url == LATEST_URL || is_loopback(url) }

pub fn download_allowed(feed: &str, url: &str) -> bool {
    if is_loopback(feed) { return is_loopback(url); }
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/")) && !url.contains("/../") && !url.contains('\\')
}

fn is_loopback(url: &str) -> bool {
    parse_url(url).is_some_and(|u| !u.secure && (u.host == "127.0.0.1" || u.host == "localhost"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct UrlParts { pub secure: bool, pub host: String, pub port: u16, pub path: String }

/// http(s)://host[:port]/path, the only shape the updater fetches.
pub fn parse_url(url: &str) -> Option<UrlParts> {
    let (secure, rest) = match url.strip_prefix("https://") { Some(r) => (true, r), None => (false, url.strip_prefix("http://")?) };
    let (authority, path) = match rest.find('/') { Some(i) => (&rest[..i], &rest[i..]), None => (rest, "/") };
    if authority.contains('@') || authority.is_empty() { return None; }
    let (host, port) = match authority.rfind(':') {
        Some(i) => (&authority[..i], authority[i + 1..].parse::<u16>().ok()?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') { return None; }
    Some(UrlParts { secure, host: host.to_ascii_lowercase(), port, path: path.to_string() })
}

/// The first few lines of the release notes as plain text (headings and list marks dropped), for the dialog.
pub fn summary(body: Option<&str>, max_lines: usize, max_chars: usize) -> String {
    let Some(body) = body else { return String::new() };
    let mut lines = vec![];
    for raw in body.lines() {
        let mut line = raw.trim().trim_start_matches('#').trim().to_string();
        for mark in ["- ", "* ", "+ "] { if let Some(r) = line.strip_prefix(mark) { line = format!("• {r}"); } }
        let line = line.replace("**", "").replace('`', "").trim().to_string();
        if line.is_empty() { continue; }
        lines.push(line);
        if lines.len() == max_lines { break; }
    }
    let text = lines.join("\n");
    if text.chars().count() > max_chars { text.chars().take(max_chars - 1).collect::<String>() + "…" } else { text }
}

/// Hours between automatic checks the user can pick (General ▸ Updates).
pub const INTERVAL_CHOICES: [u32; 3] = [6, 24, 168];

/// Due when never checked, when `interval` has passed, or when the clock was set back (it must not stop checks).
pub fn check_due(last: Option<u64>, now: u64, interval_hours: u32) -> bool {
    match last { None => true, Some(l) => now < l || now - l >= interval_hours as u64 * 3600 }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &str) -> Version { Version::parse(s).unwrap() }
    fn asset(name: &str, size: u64) -> Asset { Asset { name: name.into(), url: format!("https://github.com/{REPO}/releases/download/v0.2.0/{name}"), size } }
    fn release(tag: &str, assets: Vec<Asset>) -> Release {
        Release { tag: tag.into(), name: None, body: None, page: None, draft: false, prerelease: false, assets }
    }
    fn full(tag: &str) -> Release {
        let ver = v(tag);
        release(tag, vec![asset(&zip_name(&ver), 10), asset(CHECKSUMS, 100), asset("NachoCombo-0.2.0-macos.zip", 5)])
    }

    #[test] fn versions_parse_like_the_mac_app() {
        assert_eq!(v("v0.4.0"), Version { major: 0, minor: 4, patch: 0, pre: None });
        assert_eq!(v(" 1.2.3+build.5 "), v("1.2.3"));
        assert_eq!(v("1.0.0-beta.1").pre.as_deref(), Some("beta.1"));
        for bad in ["0.4", "1.2.3.4", "1..3", "a.b.c", "1.2.3-", "", "v", "1.2.-3", "1.2.x"] { assert_eq!(Version::parse(bad), None, "{bad}"); }
        assert_eq!(v("v1.2.3-rc.1").to_string(), "1.2.3-rc.1");
    }

    #[test] fn version_order_follows_semver() {
        let order = ["0.1.0", "0.1.1", "0.2.0-alpha", "0.2.0-alpha.1", "0.2.0-alpha.beta", "0.2.0-beta.2", "0.2.0-beta.11", "0.2.0-rc.1", "0.2.0", "0.10.0", "1.0.0"];
        for w in order.windows(2) { assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]); }
        assert_eq!(v("0.2.0").cmp(&v("v0.2.0+x")), Ordering::Equal);
        assert!(v("0.10.0") > v("0.9.9"), "numbers, not text");
    }

    #[test] fn release_json_tolerates_odd_assets() {
        let json = br#"{"tag_name":"v0.2.0","name":"NachoCombo for Windows 0.2.0","body":"notes","html_url":"https://github.com/x","draft":false,"prerelease":false,
            "assets":[{"name":"NachoCombo-windows-0.2.0-x64.zip","size":12,"browser_download_url":"https://github.com/a"},{"name":"broken"},
                      {"name":"SHA256SUMS.txt","size":99,"browser_download_url":"https://github.com/b"}]}"#;
        let r = Release::parse(json).unwrap();
        assert_eq!(r.assets.len(), 2);
        assert_eq!(r.version(), Some(v("0.2.0")));
        assert_eq!(Release::parse(b"{\"name\":\"no tag\"}"), None);
        assert_eq!(Release::parse(b"<html>rate limited</html>"), None);
    }

    #[test] fn decide_offers_only_a_newer_full_release() {
        let d = decide("0.1.0", &full("v0.2.0"), &[]);
        let Decision::Available { version, zip, sums } = d else { panic!("{d:?}") };
        assert_eq!((version, zip.name.as_str(), sums.name.as_str()), (v("0.2.0"), "NachoCombo-windows-0.2.0-x64.zip", CHECKSUMS));
        assert_eq!(decide("0.2.0", &full("v0.2.0"), &[]), Decision::UpToDate);
        assert_eq!(decide("0.3.0", &full("v0.2.0"), &[]), Decision::UpToDate, "a development build is newer");
        assert_eq!(decide("0.2.0-rc.1", &full("v0.2.0"), &[]).clone(), decide("0.1.0", &full("v0.2.0"), &[]), "a pre-release copy updates to the release");
    }

    #[test] fn decide_ignores_what_must_not_be_offered() {
        let mut r = full("v0.2.0"); r.draft = true;
        assert_eq!(decide("0.1.0", &r, &[]), Decision::Ignored("draft".into()));
        let mut r = full("v0.2.0"); r.prerelease = true;
        assert_eq!(decide("0.1.0", &r, &[]), Decision::Ignored("pre-release".into()));
        assert!(matches!(decide("0.1.0", &release("nightly", vec![]), &[]), Decision::Ignored(_)));
        assert!(matches!(decide("0.1.0", &full("v0.2.0-beta.1"), &[]), Decision::Ignored(_)), "a pre-release tag is not offered");
        assert!(matches!(decide("dev", &full("v0.2.0"), &[]), Decision::Ignored(_)));
        // The macOS zip alone is not a Windows update; nor is a zip without its checksums.
        let mac_only = release("v0.2.0", vec![asset("NachoCombo-0.2.0-macos.zip", 5), asset(CHECKSUMS, 9)]);
        assert_eq!(decide("0.1.0", &mac_only, &[]), Decision::Ignored("no NachoCombo-windows-0.2.0-x64.zip in the release".into()));
        let no_sums = release("v0.2.0", vec![asset("NachoCombo-windows-0.2.0-x64.zip", 5)]);
        assert_eq!(decide("0.1.0", &no_sums, &[]), Decision::Ignored("no SHA256SUMS.txt in the release".into()));
        // A version this PC went back from.
        assert_eq!(decide("0.1.0", &full("v0.2.0"), &["0.2.0".into()]), Decision::Ignored("0.2.0 was rolled back on this PC".into()));
        assert!(matches!(decide("0.1.0", &full("v0.2.1"), &["0.2.0".into()]), Decision::Available { .. }), "a newer one still comes");
    }

    #[test] fn sums_file_is_all_or_nothing() {
        let d = "a".repeat(64);
        let ok = format!("{d}  NachoCombo-windows-0.2.0-x64.zip\r\n\n{} *other.zip\n", "B".repeat(64));
        assert_eq!(parse_sums(&ok).unwrap(), vec![("NachoCombo-windows-0.2.0-x64.zip".into(), d.clone()), ("other.zip".into(), "b".repeat(64))]);
        for bad in [format!("{d} x.zip"), format!("{}  x.zip", "g".repeat(64)), format!("{d}  dir/x.zip"), format!("{d}  x.zip\n{d}  x.zip"), "garbage".into(), format!("{d}  ")] {
            assert_eq!(parse_sums(&bad), None, "{bad}");
        }
    }

    #[test] fn verify_checks_size_then_hash() {
        let data = b"zip bytes";
        let zip = asset("NachoCombo-windows-0.2.0-x64.zip", data.len() as u64);
        let sums = format!("{}  {}\n", sha256::hex(data), zip.name);
        assert_eq!(verify(data, &zip, &sums), Ok(()));
        assert_eq!(verify(b"zip byte", &zip, &sums), Err(VerifyProblem::WrongSize { expected: 9, actual: 8 }), "cut short");
        assert!(matches!(verify(b"zip BYTES", &zip, &sums), Err(VerifyProblem::Mismatch { .. })), "same size, other bytes");
        assert_eq!(verify(data, &zip, "not a sums file"), Err(VerifyProblem::BadChecksumsFile));
        assert_eq!(verify(data, &zip, &format!("{}  other.zip\n", sha256::hex(data))), Err(VerifyProblem::NotListed(zip.name.clone())));
        let wrong_size = Asset { size: 10, ..zip.clone() };
        assert!(matches!(verify(data, &wrong_size, &sums), Err(VerifyProblem::WrongSize { .. })));
    }

    #[test] fn downloads_only_from_this_repository_or_this_pc() {
        assert!(feed_allowed(LATEST_URL));
        assert!(feed_allowed("http://127.0.0.1:8793/repos/x/releases/latest"));
        assert!(!feed_allowed("https://api.github.com/repos/someone/else/releases/latest"));
        assert!(!feed_allowed("http://192.168.0.5/latest"));
        assert!(!feed_allowed("https://127.0.0.1/latest"), "a test feed is plain http on this PC only");
        let gh = format!("https://github.com/{REPO}/releases/download/v0.2.0/NachoCombo-windows-0.2.0-x64.zip");
        assert!(download_allowed(LATEST_URL, &gh));
        assert!(!download_allowed(LATEST_URL, "https://github.com/someone/else/releases/download/v1/x.zip"));
        assert!(!download_allowed(LATEST_URL, "http://github.com/mafianacho/nachocombo-windows/releases/download/v1/x.zip"));
        assert!(!download_allowed(LATEST_URL, "http://127.0.0.1:8793/files/x.zip"), "the real feed cannot point at this PC");
        assert!(!download_allowed(LATEST_URL, &format!("https://github.com/{REPO}/releases/download/../../../x")));
        let local = "http://127.0.0.1:8793/repos/x/releases/latest";
        assert!(download_allowed(local, "http://127.0.0.1:8793/files/x.zip"));
        assert!(!download_allowed(local, &gh), "a test feed cannot send the app to the internet");
    }

    #[test] fn urls_split_for_winhttp() {
        assert_eq!(parse_url("https://api.github.com/repos/a/b"), Some(UrlParts { secure: true, host: "api.github.com".into(), port: 443, path: "/repos/a/b".into() }));
        assert_eq!(parse_url("http://127.0.0.1:8793"), Some(UrlParts { secure: false, host: "127.0.0.1".into(), port: 8793, path: "/".into() }));
        for bad in ["ftp://x/y", "https://", "https://user@host/x", "http://host:99999/", "https://ho st/x", "file:///c:/x"] { assert_eq!(parse_url(bad), None, "{bad}"); }
    }

    #[test] fn summary_is_short_plain_text() {
        let body = "## What's new\r\n\r\n- **Bento** widgets\n* `App Switch` fixes\n+ third\n- fourth\n- fifth";
        assert_eq!(summary(Some(body), 4, 320), "What's new\n• Bento widgets\n• App Switch fixes\n• third");
        assert_eq!(summary(None, 4, 320), "");
        assert_eq!(summary(Some(&"x".repeat(400)), 4, 10).chars().count(), 10);
    }

    #[test] fn checks_are_due_by_interval_or_a_clock_set_back() {
        assert!(check_due(None, 1000, 6));
        assert!(!check_due(Some(1000), 1000 + 6 * 3600 - 1, 6));
        assert!(check_due(Some(1000), 1000 + 6 * 3600, 6));
        assert!(!check_due(Some(1000), 1000 + 23 * 3600, 24));
        assert!(check_due(Some(10_000), 5_000, 24), "clock set back");
    }
}

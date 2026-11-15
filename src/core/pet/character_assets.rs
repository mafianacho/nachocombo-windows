//! Character pictures from nacho-server (character-assets contract v1, public, no token):
//!   `GET /api/v1/character-assets` → `{catalog_version, flavors[{key,display_name,version,states{state:{url,sha256,
//!   size_bytes,content_type}}}]}`, ETag `"<catalog_version>"`, `If-None-Match` → 304;
//!   `GET /api/v1/character-assets/files/{sha256}.{png|webp}` → the bytes (content addressed, immutable).
//! Fallback v1 (per flavor+state): server picture (cached, sha256 checked) → that flavor's bundled picture → the
//! bundled `original` picture of the same state. Network errors, bad files and unknown flavors fall through silently.
//! The bundled pictures are never removed. Port of `PetCharacterAssets.swift`; the cache directory is given by the
//! caller (on Windows under `%LOCALAPPDATA%`).
use super::crypto;
use super::http::{PetHTTPClient, PetHTTPRequest};
use super::settings::ServerEndpoint;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CharacterAssetFile {
    pub url: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub content_type: String,
}

impl CharacterAssetFile {
    pub const MAX_BYTES: i64 = 1 << 20;
    pub const FILES_PREFIX: &str = "/api/v1/character-assets/files/";

    pub fn new(url: &str, sha256: &str, size_bytes: i64, content_type: &str) -> Self {
        Self { url: url.into(), sha256: sha256.into(), size_bytes, content_type: content_type.into() }
    }

    /// Only a well formed, content-addressed file of the contract is fetched (no other path, no traversal).
    pub fn is_well_formed(&self) -> bool {
        let hex = self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let typed = self.content_type == "image/png" || self.content_type == "image/webp";
        hex && typed && self.size_bytes > 0 && self.size_bytes <= Self::MAX_BYTES
            && self.url == format!("{}{}.{}", Self::FILES_PREFIX, self.sha256, self.file_extension())
    }

    pub fn file_extension(&self) -> &'static str { if self.content_type == "image/webp" { "webp" } else { "png" } }

    /// The cache file name: `<sha256>.<png|webp>`.
    fn file_name(&self) -> String { format!("{}.{}", self.sha256, self.file_extension()) }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CharacterAssetFlavor {
    pub key: String,
    pub display_name: String,
    pub version: i64,
    pub states: BTreeMap<String, CharacterAssetFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CharacterAssetCatalog {
    pub catalog_version: String,
    pub flavors: Vec<CharacterAssetFlavor>,
}

impl CharacterAssetCatalog {
    /// `None` for anything that is not a whole catalog (a missing field anywhere = not used, the old one stays).
    pub fn decode(data: &[u8]) -> Option<Self> { serde_json::from_slice(data).ok() }

    pub fn file(&self, flavor: &str, state: &str) -> Option<&CharacterAssetFile> {
        self.flavors.iter().find(|f| f.key == flavor)?.states.get(state).filter(|f| f.is_well_formed())
    }
}

/// Where one (flavor, state) picture comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetArtSource {
    Server(CharacterAssetFile),
    Bundle { flavor: String },
}

pub struct PetArtResolver;

impl PetArtResolver {
    pub const ORIGINAL_FLAVOR: &str = "original";

    /// `is_cached(sha256)`: the file is on the device and verified. `bundled_flavors`: flavors that ship a bundled
    /// picture for this state.
    pub fn resolve(
        flavor: &str, state: &str, catalog: Option<&CharacterAssetCatalog>, is_cached: impl Fn(&str) -> bool,
        bundled_flavors: &HashSet<String>,
    ) -> PetArtSource {
        if let Some(file) = catalog.and_then(|c| c.file(flavor, state)).filter(|f| is_cached(&f.sha256)) {
            return PetArtSource::Server(file.clone());
        }
        let flavor = if bundled_flavors.contains(flavor) { flavor } else { Self::ORIGINAL_FLAVOR };
        PetArtSource::Bundle { flavor: flavor.into() }
    }
}

/// The two requests. Swift builds the URL with `PetServerAPI.url` (another module); without a query that is just
/// `ServerEndpoint::rest`, used here directly.
pub struct PetCharacterAssetsAPI;

impl PetCharacterAssetsAPI {
    pub fn list_request(e: &ServerEndpoint, etag_version: Option<&str>) -> PetHTTPRequest {
        let mut r = PetHTTPRequest::get(e.rest("/api/v1/character-assets"), None);
        if let Some(v) = etag_version.filter(|v| !v.is_empty()) { r.headers.insert("If-None-Match".into(), format!("\"{v}\"")); }
        r
    }

    pub fn file_request(e: &ServerEndpoint, file: &CharacterAssetFile) -> Option<PetHTTPRequest> {
        if !file.is_well_formed() { return None; }
        let mut r = PetHTTPRequest::get(e.rest(&file.url), None);
        r.headers.insert("Accept".into(), file.content_type.clone());
        Some(r)
    }
}

/// Verified pictures on disk, named by sha256. A file that does not hash to its name is never kept or returned.
pub struct PetCharacterAssetCache {
    pub directory: PathBuf,
    /// sha256 of files already verified this run: `has`/`read` hit the disk and hash only the first time.
    verified: Mutex<HashSet<String>>,
}

impl PetCharacterAssetCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self { Self { directory: directory.into(), verified: Mutex::new(HashSet::new()) } }

    fn is_verified(&self, sha: &str) -> bool { self.verified.lock().unwrap().contains(sha) }
    fn mark_verified(&self, sha: &str) { self.verified.lock().unwrap().insert(sha.to_string()); }
    fn unmark(&self, sha: &str) { self.verified.lock().unwrap().remove(sha); }

    fn path(&self, file: &CharacterAssetFile) -> PathBuf { self.directory.join(file.file_name()) }

    fn magic_matches(data: &[u8], content_type: &str) -> bool {
        if content_type == "image/png" { return data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]); }
        data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP"
    }

    /// The bytes are what the catalog promised (size, type bytes, sha256).
    fn valid(data: &[u8], file: &CharacterAssetFile) -> bool {
        file.is_well_formed() && data.len() as i64 == file.size_bytes && Self::magic_matches(data, &file.content_type)
            && crypto::hex(&crypto::sha256(data)) == file.sha256
    }

    /// Write to a temporary name, then rename: a crash never leaves a half file under the real name.
    fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path).inspect_err(|_| { let _ = std::fs::remove_file(&tmp); })
    }

    pub fn store(&self, data: &[u8], file: &CharacterAssetFile) -> bool {
        if !Self::valid(data, file) { return false; }
        let ok = std::fs::create_dir_all(&self.directory).and_then(|_| Self::write_atomic(&self.path(file), data)).is_ok();
        if ok { self.mark_verified(&file.sha256); }
        ok
    }

    pub fn read(&self, file: &CharacterAssetFile) -> Option<Vec<u8>> {
        if !file.is_well_formed() { return None; }
        let data = std::fs::read(self.path(file)).ok()?;
        if self.is_verified(&file.sha256) { return Some(data); }
        if !Self::valid(&data, file) { return None; }
        self.mark_verified(&file.sha256);
        Some(data)
    }

    /// Cheap after the first check (memory only), so it may be asked on every frame.
    pub fn has(&self, file: &CharacterAssetFile) -> bool {
        (file.is_well_formed() && self.is_verified(&file.sha256)) || self.read(file).is_some()
    }

    fn catalog_path(&self) -> PathBuf { self.directory.join("catalog.json") }

    fn save_catalog(&self, body: &[u8]) {
        let _ = std::fs::create_dir_all(&self.directory).and_then(|_| Self::write_atomic(&self.catalog_path(), body));
    }

    fn load_catalog(&self) -> Option<CharacterAssetCatalog> { CharacterAssetCatalog::decode(&std::fs::read(self.catalog_path()).ok()?) }

    /// Removes pictures the catalog no longer lists (only inside this cache directory).
    fn prune(&self, keeping: &CharacterAssetCatalog) {
        let keep: HashSet<String> =
            keeping.flavors.iter().flat_map(|f| f.states.values()).filter(|f| f.is_well_formed()).map(|f| f.file_name()).collect();
        let Ok(entries) = std::fs::read_dir(&self.directory) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "catalog.json" || keep.contains(&name) { continue; }
            let _ = std::fs::remove_file(entry.path());
            self.unmark(&name.chars().take(64).collect::<String>());
        }
    }
}

#[derive(Default)]
struct SyncState {
    current: Option<CharacterAssetCatalog>,
    loaded: bool,
    running: bool,
}

pub type EndpointSource = Box<dyn Fn() -> Option<ServerEndpoint> + Send + Sync>;
pub type FlavorsSource = Box<dyn Fn() -> Vec<String> + Send + Sync>;

/// Fetches the catalog (If-None-Match) and the pictures of the wanted flavors into the cache. Every failure is silent:
/// the previous catalog and cache stay, and the resolver falls back to the bundled pictures.
pub struct PetCharacterAssetSync {
    http: Arc<dyn PetHTTPClient>,
    pub cache: PetCharacterAssetCache,
    endpoint: EndpointSource,
    wanted_flavors: FlavorsSource,
    state: Mutex<SyncState>,
}

impl PetCharacterAssetSync {
    /// Fetches the `original` flavor only.
    pub fn new(http: Arc<dyn PetHTTPClient>, cache: PetCharacterAssetCache, endpoint: EndpointSource) -> Arc<Self> {
        Self::with_flavors(http, cache, endpoint, Box::new(|| vec![PetArtResolver::ORIGINAL_FLAVOR.to_string()]))
    }

    pub fn with_flavors(http: Arc<dyn PetHTTPClient>, cache: PetCharacterAssetCache, endpoint: EndpointSource, wanted_flavors: FlavorsSource) -> Arc<Self> {
        Arc::new(Self { http, cache, endpoint, wanted_flavors, state: Mutex::new(SyncState::default()) })
    }

    /// The catalog in use: the last one fetched, or the one saved on disk by an earlier run.
    pub fn catalog(&self) -> Option<CharacterAssetCatalog> {
        let mut s = self.state.lock().unwrap();
        if !s.loaded {
            s.current = self.cache.load_catalog();
            s.loaded = true;
        }
        s.current.clone()
    }

    fn set_catalog(&self, c: CharacterAssetCatalog) {
        let mut s = self.state.lock().unwrap();
        s.current = Some(c);
        s.loaded = true;
    }

    pub fn source(&self, flavor: &str, state: &str, bundled_flavors: &HashSet<String>) -> PetArtSource {
        let cat = self.catalog();
        let file = cat.as_ref().and_then(|c| c.file(flavor, state));
        let is_cached = |sha: &str| file.is_some_and(|f| f.sha256 == sha && self.cache.has(f));
        PetArtResolver::resolve(flavor, state, cat.as_ref(), is_cached, bundled_flavors)
    }

    /// `completion(true)` when the cache gained a picture or the catalog changed. A refresh while one is running is
    /// ignored (its completion is never called, as in Swift). Never called with a lock held.
    pub fn refresh(self: &Arc<Self>, completion: Box<dyn FnOnce(bool) + Send>) {
        {
            let mut s = self.state.lock().unwrap();
            if s.running { return; }
            s.running = true;
        }
        let weak = Arc::downgrade(self);
        let finish = move |changed: bool| {
            if let Some(me) = weak.upgrade() { me.state.lock().unwrap().running = false; }
            completion(changed);
        };
        let Some(e) = (self.endpoint)() else { return finish(false) };
        let known = self.catalog();
        let weak: Weak<Self> = Arc::downgrade(self);
        let req = PetCharacterAssetsAPI::list_request(&e, known.as_ref().map(|k| k.catalog_version.as_str()));
        self.http.send(req, Box::new(move |result| {
            let Some(me) = weak.upgrade() else { return };
            let mut changed = false;
            if let Ok(resp) = &result {
                if resp.status == 200 {
                    if let Some(fresh) = CharacterAssetCatalog::decode(&resp.body) {
                        me.cache.save_catalog(&resp.body);
                        changed = known.as_ref() != Some(&fresh);
                        me.set_catalog(fresh);
                    }
                }
            }
            // 304, errors and bad bodies keep the catalog we have; missing pictures are still tried.
            let Some(cat) = me.catalog() else { return finish(changed) };
            let wanted: HashSet<String> = (me.wanted_flavors)().into_iter().collect();
            let mut todo: Vec<CharacterAssetFile> = Vec::new();
            let mut seen = HashSet::new();
            for f in cat.flavors.iter().filter(|f| wanted.contains(&f.key)) {
                for file in f.states.values() {
                    if file.is_well_formed() && !seen.contains(&file.sha256) && !me.cache.has(file) {
                        seen.insert(file.sha256.clone());
                        todo.push(file.clone());
                    }
                }
            }
            let me2 = Arc::clone(&me);
            me.download(todo, e, false, Box::new(move |gained| {
                if changed { me2.cache.prune(&cat); }
                finish(changed || gained);
            }));
        }));
    }

    fn download(self: &Arc<Self>, mut files: Vec<CharacterAssetFile>, e: ServerEndpoint, gained: bool, done: Box<dyn FnOnce(bool) + Send>) {
        if files.is_empty() { return done(gained); }
        let file = files.remove(0);
        let Some(req) = PetCharacterAssetsAPI::file_request(&e, &file) else { return self.download(files, e, gained, done) };
        let weak = Arc::downgrade(self);
        self.http.send(req, Box::new(move |result| {
            let Some(me) = weak.upgrade() else { return };
            let ok = matches!(&result, Ok(resp) if resp.status == 200 && me.cache.store(&resp.body, &file));
            me.download(files, e, gained || ok, done);
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::http::{HTTPCompletion, PetHTTPResponse, PetNetError};
    use crate::core::pet::media::PetIDs;
    use std::collections::HashMap;

    const BASE: &str = "https://nacho.example";

    /// Answers by URL path (Swift `CharAssetFakeHTTP`), synchronously.
    #[derive(Default)]
    struct RouteHTTP {
        routes: Mutex<HashMap<String, PetHTTPResponse>>,
        failing: Mutex<HashSet<String>>,
        requests: Mutex<Vec<PetHTTPRequest>>,
    }

    impl RouteHTTP {
        fn new() -> Arc<Self> { Arc::new(Self::default()) }
        fn route(&self, path: &str, status: u16, body: &[u8]) { self.routes.lock().unwrap().insert(path.into(), PetHTTPResponse::new(status, body.to_vec())); }
        fn requests(&self) -> Vec<PetHTTPRequest> { self.requests.lock().unwrap().clone() }
    }

    impl PetHTTPClient for RouteHTTP {
        fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) {
            self.requests.lock().unwrap().push(request.clone());
            let key = request.url.strip_prefix(BASE).unwrap_or(&request.url).to_string();
            if self.failing.lock().unwrap().contains(&key) { return completion(Err(PetNetError("offline".into()))); }
            let resp = self.routes.lock().unwrap().get(&key).cloned().unwrap_or_else(|| PetHTTPResponse::new(404, vec![]));
            completion(Ok(resp));
        }
    }

    /// A fresh temporary cache directory, removed when dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self { Self(std::env::temp_dir().join(format!("char-assets-{}", PetIDs::new_uuid()))) }
    }
    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn endpoint() -> ServerEndpoint { ServerEndpoint { base: BASE.into() } }

    fn png(n: u8) -> Vec<u8> { vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, n, n, n] }

    fn file(data: &[u8]) -> CharacterAssetFile {
        let sha = crypto::hex(&crypto::sha256(data));
        CharacterAssetFile::new(&format!("/api/v1/character-assets/files/{sha}.png"), &sha, data.len() as i64, "image/png")
    }

    fn catalog_json(version: &str, flavor: &str, states: &[(&str, &CharacterAssetFile)]) -> Vec<u8> {
        let s: Vec<String> = states
            .iter()
            .map(|(k, f)| format!(r#""{k}":{{"url":"{}","sha256":"{}","size_bytes":{},"content_type":"{}"}}"#, f.url, f.sha256, f.size_bytes, f.content_type))
            .collect();
        format!(r#"{{"catalog_version":"{version}","flavors":[{{"key":"{flavor}","display_name":"x","version":1,"states":{{{}}}}}]}}"#, s.join(",")).into_bytes()
    }

    fn sync(http: &Arc<RouteHTTP>, dir: &Path) -> Arc<PetCharacterAssetSync> {
        let http: Arc<dyn PetHTTPClient> = http.clone();
        PetCharacterAssetSync::new(http, PetCharacterAssetCache::new(dir), Box::new(|| Some(endpoint())))
    }

    fn refresh(s: &Arc<PetCharacterAssetSync>) -> bool {
        let out = Arc::new(Mutex::new(None));
        let o = out.clone();
        s.refresh(Box::new(move |c| *o.lock().unwrap() = Some(c))); // the fake answers synchronously
        let v = *out.lock().unwrap();
        v.expect("completion called")
    }

    fn set(names: &[&str]) -> HashSet<String> { names.iter().map(|s| s.to_string()).collect() }
    fn bundle(f: &str) -> PetArtSource { PetArtSource::Bundle { flavor: f.into() } }

    // resolver order

    #[test]
    fn resolver_order() {
        let f = file(&png(1));
        let cat = CharacterAssetCatalog::decode(&catalog_json("v1", "original", &[("idle", &f)]));
        let bundled = set(&["original"]);
        let r = |state: &str, cat: Option<&CharacterAssetCatalog>, cached: bool| PetArtResolver::resolve("original", state, cat, |_| cached, &bundled);
        // server picture, once cached
        assert_eq!(r("idle", cat.as_ref(), true), PetArtSource::Server(f.clone()));
        // not cached yet → bundle of the flavor
        assert_eq!(r("idle", cat.as_ref(), false), bundle("original"));
        // state missing from the catalog → bundle
        assert_eq!(r("happy", cat.as_ref(), true), bundle("original"));
        // no catalog at all (server down on first run) → bundle
        assert_eq!(r("idle", None, true), bundle("original"));
    }

    #[test]
    fn new_flavor_falls_back_to_original() {
        assert_eq!(PetArtResolver::resolve("cheese", "idle", None, |_| false, &set(&["original"])), bundle("original"));
        assert_eq!(PetArtResolver::resolve("cheese", "idle", None, |_| false, &set(&["original", "cheese"])), bundle("cheese"));
    }

    // requests

    #[test]
    fn requests() {
        let f = file(&png(2));
        let e = endpoint();
        assert_eq!(PetCharacterAssetsAPI::list_request(&e, None).header("If-None-Match"), None);
        let r = PetCharacterAssetsAPI::list_request(&e, Some("abc"));
        assert_eq!(r.header("If-None-Match"), Some("\"abc\""));
        assert_eq!(r.header("Authorization"), None);
        assert_eq!(r.url, "https://nacho.example/api/v1/character-assets");
        let fr = PetCharacterAssetsAPI::file_request(&e, &f).expect("file request");
        assert_eq!(fr.url, format!("{BASE}{}", f.url));
        assert_eq!(fr.header("Accept"), Some("image/png"));
        let mut bad = f.clone();
        bad.url = "/api/v1/admin/secrets".into();
        assert_eq!(PetCharacterAssetsAPI::file_request(&e, &bad), None);
    }

    // sync

    #[test]
    fn refresh_downloads_and_caches() {
        let dir = TempDir::new();
        let data = png(3);
        let f = file(&data);
        let http = RouteHTTP::new();
        http.route("/api/v1/character-assets", 200, &catalog_json("v1", "original", &[("idle", &f)]));
        http.route(&f.url, 200, &data);
        let s = sync(&http, &dir.0);
        assert!(refresh(&s));
        assert_eq!(s.cache.read(&f), Some(data.clone()));
        assert_eq!(s.source("original", "idle", &set(&["original"])), PetArtSource::Server(f.clone()));
        // second start: catalog comes from disk, 304 keeps it, no second download
        let http2 = RouteHTTP::new();
        http2.route("/api/v1/character-assets", 304, b"");
        let s2 = sync(&http2, &dir.0);
        assert!(!refresh(&s2));
        assert_eq!(http2.requests().first().and_then(|r| r.header("If-None-Match").map(str::to_string)).as_deref(), Some("\"v1\""));
        assert_eq!(http2.requests().len(), 1);
        assert_eq!(s2.source("original", "idle", &set(&["original"])), PetArtSource::Server(f));
    }

    #[test]
    fn server_error_keeps_previous_and_bundle() {
        let dir = TempDir::new();
        let data = png(4);
        let f = file(&data);
        let http = RouteHTTP::new();
        http.route("/api/v1/character-assets", 200, &catalog_json("v1", "original", &[("idle", &f)]));
        http.route(&f.url, 200, &data);
        let s = sync(&http, &dir.0);
        let _ = refresh(&s);
        http.route("/api/v1/character-assets", 500, b"");
        assert!(!refresh(&s));
        assert_eq!(s.source("original", "idle", &set(&["original"])), PetArtSource::Server(f));
        // network down on a fresh device: bundle
        let down = RouteHTTP::new();
        down.failing.lock().unwrap().insert("/api/v1/character-assets".into());
        let fresh = TempDir::new();
        let s3 = sync(&down, &fresh.0);
        assert!(!refresh(&s3));
        assert_eq!(s3.source("original", "idle", &set(&["original"])), bundle("original"));
    }

    #[test]
    fn hash_mismatch_is_not_cached() {
        let dir = TempDir::new();
        let good = png(5);
        let f = file(&good);
        let http = RouteHTTP::new();
        http.route("/api/v1/character-assets", 200, &catalog_json("v1", "original", &[("idle", &f)]));
        http.route(&f.url, 200, &png(6)); // same size, other bytes
        let s = sync(&http, &dir.0);
        let _ = refresh(&s);
        assert_eq!(s.cache.read(&f), None);
        assert_eq!(s.source("original", "idle", &set(&["original"])), bundle("original"));
    }

    #[test]
    fn tampered_cache_file_is_ignored() {
        let dir = TempDir::new();
        let data = png(7);
        let f = file(&data);
        let cache = PetCharacterAssetCache::new(&dir.0);
        assert!(cache.store(&data, &f));
        std::fs::write(dir.0.join(format!("{}.png", f.sha256)), png(8)).unwrap();
        assert_eq!(PetCharacterAssetCache::new(&dir.0).read(&f), None); // a new run re-verifies from disk
        assert!(!cache.store(b"not an image", &f));
    }

    #[test]
    fn new_catalog_prunes_old_files() {
        let dir = TempDir::new();
        let (a, b) = (png(9), png(10));
        let (fa, fb) = (file(&a), file(&b));
        let http = RouteHTTP::new();
        http.route("/api/v1/character-assets", 200, &catalog_json("v1", "original", &[("idle", &fa)]));
        http.route(&fa.url, 200, &a);
        http.route(&fb.url, 200, &b);
        let s = sync(&http, &dir.0);
        let _ = refresh(&s);
        http.route("/api/v1/character-assets", 200, &catalog_json("v2", "original", &[("idle", &fb)]));
        assert!(refresh(&s));
        assert_eq!(s.cache.read(&fa), None);
        assert_eq!(s.cache.read(&fb), Some(b));
    }

    #[test]
    fn repeated_lookup_does_not_touch_the_disk_again() {
        let dir = TempDir::new();
        let data = png(11);
        let f = file(&data);
        let http = RouteHTTP::new();
        http.route("/api/v1/character-assets", 200, &catalog_json("v1", "original", &[("idle", &f)]));
        http.route(&f.url, 200, &data);
        let s = sync(&http, &dir.0);
        let _ = refresh(&s);
        assert_eq!(s.source("original", "idle", &set(&["original"])), PetArtSource::Server(f.clone()));
        // Gone from disk: `has` still answers from memory (verified once), until the next refresh/prune.
        std::fs::remove_file(dir.0.join(format!("{}.png", f.sha256))).unwrap();
        for _ in 0..5 { assert_eq!(s.source("original", "idle", &set(&["original"])), PetArtSource::Server(f.clone())); }
        // A fresh cache object (new run) verifies from disk again and finds nothing.
        assert!(!PetCharacterAssetCache::new(&dir.0).has(&f));
    }

    // Not in Swift: a webp file and the well-formed rules.
    #[test]
    fn webp_and_well_formed_rules() {
        let mut webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        webp.push(1);
        let sha = crypto::hex(&crypto::sha256(&webp));
        let f = CharacterAssetFile::new(&format!("/api/v1/character-assets/files/{sha}.webp"), &sha, webp.len() as i64, "image/webp");
        assert!(f.is_well_formed());
        let dir = TempDir::new();
        let cache = PetCharacterAssetCache::new(&dir.0);
        assert!(cache.store(&webp, &f));
        assert!(!cache.store(&png(1), &file(&webp)), "png bytes promised, webp bytes given");
        let mut big = f.clone();
        big.size_bytes = CharacterAssetFile::MAX_BYTES + 1;
        assert!(!big.is_well_formed());
        let mut upper = f.clone();
        upper.sha256 = upper.sha256.to_uppercase();
        assert!(!upper.is_well_formed());
        let mut gif = f;
        gif.content_type = "image/gif".into();
        assert!(!gif.is_well_formed());
    }
}

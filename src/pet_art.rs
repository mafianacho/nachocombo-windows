//! The character's pictures: the server's when they are cached and verified, otherwise the ones bundled in the exe
//! (`assets/nacho/`, the same files as the Mac app). Decoded once, shared by the pet and the chat.
use crate::core::pet::character_assets::{PetArtResolver, PetArtSource, PetCharacterAssetCache, PetCharacterAssetSync};
use crate::core::pet::settings::ServerEndpoint;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub type Pixels = (Vec<u8>, u32, u32);

const BUNDLED: [(&str, &[u8]); 10] = [
    ("body", include_bytes!("../assets/nacho/body.webp")),
    ("calling-eyeless", include_bytes!("../assets/nacho/calling-eyeless.webp")),
    ("calling", include_bytes!("../assets/nacho/calling.png")),
    ("eyes-closed", include_bytes!("../assets/nacho/eyes-closed.png")),
    ("happy", include_bytes!("../assets/nacho/happy.png")),
    ("idle", include_bytes!("../assets/nacho/idle.png")),
    ("thinking", include_bytes!("../assets/nacho/thinking.png")),
    ("tired", include_bytes!("../assets/nacho/tired.png")),
    ("waiting", include_bytes!("../assets/nacho/waiting.png")),
    ("working", include_bytes!("../assets/nacho/working.png")),
];

thread_local! {
    static DECODED: RefCell<HashMap<String, Option<Rc>>> = RefCell::new(HashMap::new());
    static SYNC: RefCell<Option<Arc<PetCharacterAssetSync>>> = const { RefCell::new(None) };
}

type Rc = std::rc::Rc<Pixels>;

/// A bundled picture by name. The listening blink is a WebP that an older Windows may not decode: the plain listening
/// picture stands in for it then.
pub fn bundled(name: &str) -> Option<Rc> {
    let key = format!("bundle:{name}");
    if let Some(v) = DECODED.with(|d| d.borrow().get(&key).cloned()) { return v; }
    let bytes = BUNDLED.iter().find(|(n, _)| *n == name).map(|(_, b)| *b);
    let mut got = bytes.and_then(|b| crate::gfx::decode_bytes(b).ok()).map(std::rc::Rc::new);
    if got.is_none() && name == "calling-eyeless" { got = bundled("calling"); }
    DECODED.with(|d| d.borrow_mut().insert(key, got.clone()));
    got
}

/// Starts the server pictures: the catalog and the files go into a cache folder; every failure keeps the bundled ones.
pub fn start_server_art(dir: std::path::PathBuf, http: Arc<dyn crate::core::pet::http::PetHTTPClient>, endpoint: Box<dyn Fn() -> Option<ServerEndpoint> + Send + Sync>, changed: Box<dyn FnOnce(bool) + Send>) {
    let sync = SYNC.with(|s| {
        let mut s = s.borrow_mut();
        s.get_or_insert_with(|| PetCharacterAssetSync::new(http, PetCharacterAssetCache::new(dir), endpoint)).clone()
    });
    sync.refresh(changed);
}

pub fn stop_server_art() {
    SYNC.with(|s| *s.borrow_mut() = None);
    DECODED.with(|d| d.borrow_mut().retain(|k, _| k.starts_with("bundle:")));
}

/// Forget decoded server pictures (the catalog changed).
pub fn art_changed() { DECODED.with(|d| d.borrow_mut().retain(|k, _| k.starts_with("bundle:"))); }

/// The picture for an asset name (`idle`, `thinking` …): the server's when verified, else the bundled one.
pub fn image(name: &str) -> Option<Rc> {
    let source = SYNC.with(|s| s.borrow().as_ref().map(|sync| sync.source(PetArtResolver::ORIGINAL_FLAVOR, name, &HashSet::from([PetArtResolver::ORIGINAL_FLAVOR.to_string()]))));
    if let Some(PetArtSource::Server(file)) = source {
        let key = format!("server:{}", file.sha256);
        if let Some(v) = DECODED.with(|d| d.borrow().get(&key).cloned()) { if v.is_some() { return v; } }
        else {
            let got = SYNC.with(|s| s.borrow().as_ref().and_then(|sync| sync.cache.read(&file))).and_then(|b| crate::gfx::decode_bytes(&b).ok()).map(std::rc::Rc::new);
            DECODED.with(|d| d.borrow_mut().insert(key, got.clone()));
            if got.is_some() { return got; }
        }
    }
    bundled(name)
}

/// The key the canvases cache a picture under: a changed server picture has another hash, so it is uploaded again.
pub fn key_of(name: &str) -> String {
    SYNC.with(|s| s.borrow().as_ref().and_then(|sync| match sync.source(PetArtResolver::ORIGINAL_FLAVOR, name, &HashSet::from([PetArtResolver::ORIGINAL_FLAVOR.to_string()])) {
        PetArtSource::Server(f) => Some(format!("art:{name}:{}", f.sha256)),
        _ => None,
    })).unwrap_or_else(|| format!("art:{name}"))
}

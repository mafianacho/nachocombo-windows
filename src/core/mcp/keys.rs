//! API keys of the MCP server (port of `ComputerUseKeys.swift`). A key is 32 random bytes shown once as `nccu_` +
//! base64url; only its SHA-256 is stored (`computer-use-keys.json` in the app's folder, next to `settings.json`). The
//! key is 256 bits of randomness, so a plain hash is enough (no salt, no stretching). A key is checked against every
//! stored hash without an early exit. An unreadable key file makes every key invalid (a copy is kept as `.corrupt`).
//!
//! Windows: the folder is the user's own `%APPDATA%\NachoCombo`, which only this user (and administrators) can read; no
//! POSIX modes are set. A hash is not a secret, so the file needs no Credential Manager.
use super::limits::Clock;
use crate::core::pet::crypto;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type RandomBytes = Arc<dyn Fn(usize) -> Vec<u8> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    InvalidLabel,
    TooManyKeys,
    NotFound,
    PersistFailed,
}

impl KeyError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidLabel => "Give the key a name of 1 to 40 characters.",
            Self::TooManyKeys => "There are already 8 keys. Delete one first.",
            Self::NotFound => "That key is gone.",
            Self::PersistFailed => "The key file could not be written.",
        }
    }
}

/// What is stored about a key. The key itself is never stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRecord {
    pub id: String,
    pub label: String,
    pub hash: String,
    /// ISO 8601 (UTC).
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
}

/// The only moment the plain key exists outside the client: show it once, then drop it.
#[derive(Clone, Debug, PartialEq)]
pub struct IssuedKey {
    pub record: KeyRecord,
    pub token: String,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    keys: Vec<KeyRecord>,
}

struct State {
    keys: Vec<KeyRecord>,
    last_persisted: f64,
}

pub struct KeyStore {
    path: Option<PathBuf>,
    random: RandomBytes,
    now: Clock,
    state: Mutex<State>,
    pub last_load_problem: Option<String>,
}

impl KeyStore {
    pub const FILE_NAME: &'static str = "computer-use-keys.json";
    pub const TOKEN_PREFIX: &'static str = "nccu_";
    pub const MAX_KEYS: usize = 8;
    pub const MAX_LABEL: usize = 40;
    /// `lastUsedAt` is written at most this often, so a busy client does not rewrite the file on every request.
    const PERSIST_INTERVAL: f64 = 60.0;

    /// `path` None keeps the keys in memory only (tests).
    pub fn new(path: Option<PathBuf>, random: RandomBytes, now: Clock) -> Self {
        let mut store = Self { path, random, now, state: Mutex::new(State { keys: vec![], last_persisted: f64::MIN }), last_load_problem: None };
        store.load();
        store
    }

    pub fn path_in(dir: &Path) -> PathBuf { dir.join(Self::FILE_NAME) }

    pub fn records(&self) -> Vec<KeyRecord> { self.state.lock().unwrap().keys.clone() }

    pub fn create(&self, label: &str) -> Result<IssuedKey, KeyError> {
        let clean = label.trim();
        if clean.is_empty() || clean.chars().count() > Self::MAX_LABEL { return Err(KeyError::InvalidLabel); }
        let mut s = self.state.lock().unwrap();
        if s.keys.len() >= Self::MAX_KEYS { return Err(KeyError::TooManyKeys); }
        self.issue(&mut s, clean)
    }

    /// Takes effect at once.
    pub fn revoke(&self, id: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        let Some(i) = s.keys.iter().position(|k| k.id == id) else { return false };
        let removed = s.keys.remove(i);
        if self.persist(&s.keys).is_err() { s.keys.insert(i, removed); return false; }
        true
    }

    /// Replaces a key: the old key stops working at once, the new one keeps the label.
    pub fn reissue(&self, id: &str) -> Result<IssuedKey, KeyError> {
        let mut s = self.state.lock().unwrap();
        let Some(i) = s.keys.iter().position(|k| k.id == id) else { return Err(KeyError::NotFound) };
        let old = s.keys.remove(i);
        match self.issue(&mut s, &old.label) {
            Ok(k) => Ok(k),
            Err(e) => { s.keys.insert(i, old); Err(e) }
        }
    }

    /// Checks every stored hash (no early exit) so timing does not reveal which key matched.
    pub fn authenticate(&self, token: &str) -> Option<KeyRecord> {
        let candidate = crypto::hex(&crypto::sha256(token.as_bytes()));
        let mut s = self.state.lock().unwrap();
        let mut matched = None;
        for (i, k) in s.keys.iter().enumerate() {
            if constant_time_eq(candidate.as_bytes(), k.hash.as_bytes()) { matched = Some(i); }
        }
        let hit = matched?;
        let t = (self.now)();
        s.keys[hit].last_used_at = Some(crate::core::pet::desktop::iso8601(t));
        if t - s.last_persisted >= Self::PERSIST_INTERVAL && self.persist(&s.keys).is_ok() { s.last_persisted = t; }
        Some(s.keys[hit].clone())
    }

    fn issue(&self, s: &mut State, label: &str) -> Result<IssuedKey, KeyError> {
        let secret = (self.random)(32);
        let id_bytes = (self.random)(8);
        if secret.len() != 32 || id_bytes.len() != 8 { return Err(KeyError::PersistFailed); }
        let token = format!("{}{}", Self::TOKEN_PREFIX, crypto::base64url(&secret));
        let record = KeyRecord {
            id: crypto::hex(&id_bytes),
            label: label.to_string(),
            hash: crypto::hex(&crypto::sha256(token.as_bytes())),
            created_at: crate::core::pet::desktop::iso8601((self.now)()),
            last_used_at: None,
        };
        s.keys.push(record.clone());
        if self.persist(&s.keys).is_err() { s.keys.pop(); return Err(KeyError::PersistFailed); }
        Ok(IssuedKey { record, token })
    }

    fn load(&mut self) {
        let Some(path) = self.path.clone() else { return };
        let Ok(data) = std::fs::read(&path) else { return };
        match serde_json::from_slice::<FileFormat>(&data) {
            Ok(f) => self.state.get_mut().unwrap().keys = f.keys,
            Err(e) => {
                // Fail closed: an unreadable key file means no key is valid. Keep a copy for inspection.
                self.last_load_problem = Some(format!("Could not read the key file, so all keys are treated as invalid: {e}"));
                let backup = path.with_extension("json.corrupt");
                let _ = std::fs::remove_file(&backup);
                let _ = std::fs::rename(&path, &backup);
            }
        }
    }

    fn persist(&self, keys: &[KeyRecord]) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let data = serde_json::to_vec_pretty(&FileFormat { version: 1, keys: keys.to_vec() })?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }
}

/// Compares without returning at the first differing byte. A length difference is not secret (all hashes are 64).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::core::mcp::limits::tests::FakeClock;
    use std::sync::atomic::{AtomicU8, Ordering};

    /// Counting bytes: every call gives different bytes.
    pub fn counting() -> RandomBytes {
        let n = Arc::new(AtomicU8::new(1));
        Arc::new(move |len| { let b = n.fetch_add(1, Ordering::SeqCst); vec![b; len] })
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nachocombo-mcp-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn store(path: Option<PathBuf>) -> KeyStore { KeyStore::new(path, counting(), FakeClock::new(1_800_000_000.0).clock()) }

    #[test]
    fn a_key_is_shown_once_and_only_its_hash_is_kept() {
        let dir = temp_dir("hash");
        let path = KeyStore::path_in(&dir);
        let s = store(Some(path.clone()));
        let k = s.create("  Claude Desktop ").unwrap();
        assert!(k.token.starts_with("nccu_"));
        assert_eq!(k.token.len(), 5 + 43, "32 bytes in base64url without padding");
        assert_eq!(k.record.label, "Claude Desktop");
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains(&k.token), "the plain key never reaches the file");
        assert!(file.contains(&crypto::hex(&crypto::sha256(k.token.as_bytes()))));
        assert_eq!(s.authenticate(&k.token).map(|r| r.id), Some(k.record.id.clone()));
        assert_eq!(s.authenticate("nccu_wrong"), None);
        assert_eq!(s.authenticate(""), None);
        // A new store reads the same file.
        let again = store(Some(path));
        assert_eq!(again.records().len(), 1);
        assert!(again.authenticate(&k.token).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn labels_and_the_eight_key_limit() {
        let s = store(None);
        assert_eq!(s.create("   ").unwrap_err(), KeyError::InvalidLabel);
        assert_eq!(s.create(&"가".repeat(41)).unwrap_err(), KeyError::InvalidLabel);
        assert!(s.create(&"가".repeat(40)).is_ok());
        for i in 1..8 { s.create(&format!("k{i}")).unwrap(); }
        assert_eq!(s.create("nine").unwrap_err(), KeyError::TooManyKeys);
    }

    #[test]
    fn revoke_and_reissue_take_effect_at_once() {
        let s = store(None);
        let a = s.create("a").unwrap();
        let b = s.create("b").unwrap();
        assert!(s.revoke(&a.record.id));
        assert!(!s.revoke(&a.record.id));
        assert!(s.authenticate(&a.token).is_none());
        let b2 = s.reissue(&b.record.id).unwrap();
        assert_eq!(b2.record.label, "b");
        assert_ne!(b2.token, b.token);
        assert!(s.authenticate(&b.token).is_none());
        assert!(s.authenticate(&b2.token).is_some());
        assert_eq!(s.reissue("nope").unwrap_err(), KeyError::NotFound);
    }

    #[test]
    fn a_broken_key_file_makes_every_key_invalid_and_is_kept_aside() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = KeyStore::path_in(&dir);
        std::fs::write(&path, b"{not json").unwrap();
        let s = store(Some(path.clone()));
        assert!(s.records().is_empty());
        assert!(s.last_load_problem.is_some());
        assert!(dir.join("computer-use-keys.json.corrupt").exists());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}

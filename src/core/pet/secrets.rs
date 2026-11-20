//! Where the pet keeps its secrets (login tokens of each account scope, connector bindings). Port of
//! `PetSecrets.swift`. The shell implements [`PetSecretStore`] (Windows: Credential Manager, a 0600 file in the dev CLI);
//! the core only ever talks to it through these three calls.
//!
//! Rule that came from a macOS bug: a read that is refused or unavailable is NOT "signed out". The item may well be
//! there (on macOS the user pressed 거부 on the Keychain prompt; on Windows the credential store may be locked or
//! failing), so nothing is overwritten or deleted because of it and the next access asks again.
//!
//! Windows note for the shell: Credential Manager stores at most 2560 bytes per credential blob
//! (`CRED_MAX_CREDENTIAL_BLOB_SIZE`). [`PetSecretBundle`] keeps every pet secret in ONE item, which is larger than that
//! (several token sets, each with a JWT), so the backing store must accept values larger than 2560 bytes: the shell
//! splits a value over several credentials (`<key>`, `<key>#1`, …) and joins them on read. The core does not split.
use super::crypto;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

/// What a secret store answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetSecretLookup {
    Found(Vec<u8>),
    Missing,
    /// The store did not hand the value out (Windows: a `GetLastError` code; macOS: an OSStatus). Not "missing".
    Unavailable(i32),
}

/// A write the store refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PetSecretUnavailable {
    pub status: i32,
}

impl std::fmt::Display for PetSecretUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "secret store unavailable ({})", self.status) }
}

/// The token store the shell implements (Windows: Credential Manager). Calls may come from any thread.
pub trait PetSecretStore: Send + Sync {
    fn lookup(&self, key: &str) -> PetSecretLookup;
    fn write(&self, key: &str, value: &[u8]) -> Result<(), PetSecretUnavailable>;
    fn delete(&self, key: &str);

    /// The value, or None when it is missing *or* could not be read. Use `lookup` where the two must differ.
    fn read(&self, key: &str) -> Option<Vec<u8>> {
        match self.lookup(key) { PetSecretLookup::Found(d) => Some(d), _ => None }
    }
}

/// Secrets in memory (tests, and a shell that has no store).
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self { Self::default() }
    pub fn keys(&self) -> Vec<String> {
        let mut k: Vec<String> = self.values.lock().unwrap().keys().cloned().collect();
        k.sort();
        k
    }
}

impl PetSecretStore for MemorySecretStore {
    fn lookup(&self, key: &str) -> PetSecretLookup {
        self.values.lock().unwrap().get(key).map_or(PetSecretLookup::Missing, |d| PetSecretLookup::Found(d.clone()))
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), PetSecretUnavailable> {
        self.values.lock().unwrap().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) { self.values.lock().unwrap().remove(key); }
}

/// The bundle item: values as base64 (JSON has no bytes) and the keys of earlier-version items already moved in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Contents {
    #[serde(default)]
    items: BTreeMap<String, String>,
    /// Keys whose item of an earlier version was moved in already (or found missing): not looked up again.
    #[serde(default)]
    moved: BTreeSet<String>,
}

impl Contents {
    fn get(&self, key: &str) -> Option<Vec<u8>> { self.items.get(key).and_then(|v| crypto::base64_decode(v)) }
    fn set(&mut self, key: &str, value: &[u8]) { self.items.insert(key.into(), crypto::base64(value)); }
    /// Unreadable contents hold nothing usable: start empty (the next write replaces them).
    fn decode(data: &[u8]) -> Contents {
        let c: Contents = serde_json::from_slice(data).unwrap_or_default();
        if c.items.values().all(|v| crypto::base64_decode(v).is_some()) { c } else { Contents::default() }
    }
    fn encode(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }
}

#[derive(Default)]
struct BundleState {
    contents: Option<Contents>,
    /// Earlier-version items found missing in this launch (no need to ask twice).
    checked: HashSet<String>,
}

/// Every pet secret in one item of `backing`, read once per launch and kept in memory. Why one item: on macOS each
/// Keychain item had its own access prompt per build and a launch read up to three items; one item means at most one
/// prompt (and on Windows one Credential Manager read per launch).
///
/// Items of earlier versions (one per key) are moved in on first use: read once, copied into the bundle and then never
/// read again. They are not deleted then; they go when their key is deleted (sign out, dead login), so an old item can
/// never bring a signed-out login back.
///
/// A refused read (`Unavailable`) is not cached as "empty": nothing is written over the bundle until it was read, and
/// the next access asks the backing store again.
pub struct PetSecretBundle {
    backing: std::sync::Arc<dyn PetSecretStore>,
    state: Mutex<BundleState>,
}

impl PetSecretBundle {
    pub const ACCOUNT: &'static str = "pet-secrets.v1";

    pub fn new(backing: std::sync::Arc<dyn PetSecretStore>) -> Self { Self { backing, state: Mutex::new(BundleState::default()) } }

    /// Ok when the bundle is in memory; otherwise why it could not be read.
    fn load(&self, s: &mut BundleState) -> Result<(), i32> {
        if s.contents.is_some() { return Ok(()); }
        match self.backing.lookup(Self::ACCOUNT) {
            PetSecretLookup::Found(d) => s.contents = Some(Contents::decode(&d)),
            PetSecretLookup::Missing => s.contents = Some(Contents::default()),
            PetSecretLookup::Unavailable(status) => return Err(status),
        }
        Ok(())
    }

    fn save(&self, s: &mut BundleState, next: Contents) -> Result<(), PetSecretUnavailable> {
        self.backing.write(Self::ACCOUNT, &next.encode())?;
        s.contents = Some(next);
        Ok(())
    }
}

impl PetSecretStore for PetSecretBundle {
    fn lookup(&self, key: &str) -> PetSecretLookup {
        let mut s = self.state.lock().unwrap();
        if let Err(status) = self.load(&mut s) { return PetSecretLookup::Unavailable(status); }
        let Some(mut c) = s.contents.clone() else { return PetSecretLookup::Missing };
        if let Some(d) = c.get(key) { return PetSecretLookup::Found(d); }
        if c.moved.contains(key) || s.checked.contains(key) || key == Self::ACCOUNT { return PetSecretLookup::Missing; }
        match self.backing.lookup(key) {
            // an item of an earlier version
            PetSecretLookup::Missing => {
                s.checked.insert(key.into());
                PetSecretLookup::Missing
            }
            PetSecretLookup::Unavailable(status) => PetSecretLookup::Unavailable(status),
            PetSecretLookup::Found(d) => {
                c.set(key, &d);
                c.moved.insert(key.into());
                if self.save(&mut s, c.clone()).is_err() {
                    s.contents = Some(c); // kept for this launch; saved with the next write, the old item stays meanwhile
                }
                PetSecretLookup::Found(d)
            }
        }
    }

    fn write(&self, key: &str, value: &[u8]) -> Result<(), PetSecretUnavailable> {
        let mut s = self.state.lock().unwrap();
        self.load(&mut s).map_err(|status| PetSecretUnavailable { status })?;
        let mut c = s.contents.clone().unwrap_or_default();
        c.set(key, value);
        c.moved.insert(key.into()); // an older item of this key is out of date now
        self.save(&mut s, c)
    }

    fn delete(&self, key: &str) {
        let mut s = self.state.lock().unwrap();
        self.backing.delete(key); // the item of an earlier version, if any
        s.checked.insert(key.into());
        if self.load(&mut s).is_err() {
            // Sign out (or a dead login) while the bundle cannot be read: the secret must not come back at the next
            // launch, and it cannot be taken out without reading the rest. Drop the bundle; signing in restores it.
            self.backing.delete(Self::ACCOUNT);
            s.contents = None;
            return;
        }
        let Some(mut c) = s.contents.clone() else { return };
        if !c.items.contains_key(key) && c.moved.contains(key) { return; }
        c.items.remove(key);
        c.moved.insert(key.into());
        if self.save(&mut s, c.clone()).is_err() { s.contents = Some(c); }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Stand-in for a credential store that can refuse reads and writes (the macOS tests' scripted Keychain).
    //! `prompts` counts reads of an existing item (on macOS each could show an "allow access" prompt).
    use super::*;

    #[derive(Default)]
    pub struct ScriptedStore {
        pub items: Mutex<HashMap<String, Vec<u8>>>,
        /// Every read is refused with this status.
        pub refuse_all: Mutex<Option<i32>>,
        /// Only these items are refused.
        pub refused: Mutex<HashSet<String>>,
        pub fail_writes: std::sync::atomic::AtomicBool,
        pub prompts: std::sync::atomic::AtomicUsize,
        pub deleted: Mutex<Vec<String>>,
    }

    impl ScriptedStore {
        pub fn new() -> std::sync::Arc<Self> { std::sync::Arc::new(Self::default()) }
        pub fn get(&self, k: &str) -> Option<Vec<u8>> { self.items.lock().unwrap().get(k).cloned() }
        pub fn put(&self, k: &str, v: &[u8]) { self.items.lock().unwrap().insert(k.into(), v.to_vec()); }
        pub fn accounts(&self) -> BTreeSet<String> { self.items.lock().unwrap().keys().cloned().collect() }
        pub fn refuse(&self, s: Option<i32>) { *self.refuse_all.lock().unwrap() = s; }
        pub fn prompts(&self) -> usize { self.prompts.load(std::sync::atomic::Ordering::SeqCst) }
        pub fn reset_prompts(&self) { self.prompts.store(0, std::sync::atomic::Ordering::SeqCst); }
        pub fn deleted(&self) -> Vec<String> { self.deleted.lock().unwrap().clone() }
        fn prompt(&self) { self.prompts.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    }

    impl PetSecretStore for ScriptedStore {
        fn lookup(&self, key: &str) -> PetSecretLookup {
            if let Some(s) = *self.refuse_all.lock().unwrap() { self.prompt(); return PetSecretLookup::Unavailable(s); }
            if self.refused.lock().unwrap().contains(key) { self.prompt(); return PetSecretLookup::Unavailable(-128); }
            match self.get(key) {
                None => PetSecretLookup::Missing,
                Some(d) => { self.prompt(); PetSecretLookup::Found(d) }
            }
        }
        fn write(&self, key: &str, value: &[u8]) -> Result<(), PetSecretUnavailable> {
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) || self.refuse_all.lock().unwrap().is_some() {
                return Err(PetSecretUnavailable { status: -25293 });
            }
            self.put(key, value);
            Ok(())
        }
        fn delete(&self, key: &str) {
            self.deleted.lock().unwrap().push(key.into());
            self.items.lock().unwrap().remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::ScriptedStore;
    use super::*;
    use std::sync::Arc;
    use PetSecretLookup::*;

    const TOKENS_BLOB: &[u8] = br#"{"accessToken":"at-old","refreshToken":"rt-old","expiresAt":0,"refreshExpiresAt":null}"#;

    fn bundle(kc: &Arc<ScriptedStore>) -> PetSecretBundle { PetSecretBundle::new(kc.clone()) }

    #[test]
    fn one_keychain_read_serves_every_pet_secret_of_a_launch() {
        let kc = ScriptedStore::new();
        let first = bundle(&kc);
        first.write("tokens|s", b"t").unwrap();
        first.write("connector-agent|s", b"a").unwrap();
        first.write("connector|s|a", b"c").unwrap();
        assert_eq!(kc.accounts(), BTreeSet::from([PetSecretBundle::ACCOUNT.to_string()]), "one item holds them all");

        kc.reset_prompts();
        let launch = bundle(&kc); // next launch
        assert_eq!(launch.lookup("tokens|s"), Found(b"t".to_vec()));
        assert_eq!(launch.lookup("connector-agent|s"), Found(b"a".to_vec()));
        assert_eq!(launch.lookup("connector|s|a"), Found(b"c".to_vec()));
        assert_eq!(launch.lookup("tokens|other"), Missing);
        assert_eq!(kc.prompts(), 1, "one store read per launch");
    }

    #[test]
    fn items_of_earlier_versions_move_into_the_bundle_without_loss() {
        let kc = ScriptedStore::new();
        kc.put("tokens|s", TOKENS_BLOB);
        kc.put("connector-agent|s", b"a");
        kc.put("connector|s|a", b"c");
        let first = bundle(&kc);
        assert_eq!(first.lookup("tokens|s"), Found(TOKENS_BLOB.to_vec()));
        assert_eq!(first.lookup("connector-agent|s"), Found(b"a".to_vec()));
        assert_eq!(first.lookup("connector|s|a"), Found(b"c".to_vec()));
        assert!(kc.get(PetSecretBundle::ACCOUNT).is_some(), "moved into the bundle");
        assert_eq!(kc.get("tokens|s").as_deref(), Some(TOKENS_BLOB), "the old item is kept");
        assert!(kc.deleted().is_empty());

        kc.reset_prompts();
        let launch = bundle(&kc);
        assert_eq!(launch.lookup("tokens|s"), Found(TOKENS_BLOB.to_vec()));
        assert_eq!(launch.lookup("connector|s|a"), Found(b"c".to_vec()));
        assert_eq!(kc.prompts(), 1, "the old items are not read again");
    }

    #[test]
    fn a_refused_read_never_overwrites_the_bundle_and_is_asked_again() {
        let kc = ScriptedStore::new();
        bundle(&kc).write("tokens|s", b"t").unwrap();
        let stored = kc.get(PetSecretBundle::ACCOUNT);
        kc.refuse(Some(-128));
        let launch = bundle(&kc);
        assert_eq!(launch.lookup("tokens|s"), Unavailable(-128));
        assert!(launch.write("tokens|s", b"new").is_err());
        assert_eq!(kc.get(PetSecretBundle::ACCOUNT), stored, "not overwritten with a bundle that lost the rest");
        assert!(kc.deleted().is_empty());
        kc.refuse(None);
        assert_eq!(launch.lookup("tokens|s"), Found(b"t".to_vec()), "the next access asks again");
    }

    #[test]
    fn a_failed_write_keeps_what_was_stored() {
        let kc = ScriptedStore::new();
        let b = bundle(&kc);
        b.write("tokens|s", b"t").unwrap();
        kc.fail_writes.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(b.write("tokens|s", b"new").is_err());
        assert_eq!(b.lookup("tokens|s"), Found(b"t".to_vec()));
        kc.fail_writes.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(bundle(&kc).lookup("tokens|s"), Found(b"t".to_vec()));
    }

    #[test]
    fn delete_removes_the_value_and_the_old_item() {
        let kc = ScriptedStore::new();
        kc.put("tokens|s", TOKENS_BLOB);
        let b = bundle(&kc);
        b.write("connector|s|a", b"c").unwrap();
        b.delete("tokens|s");
        assert_eq!(b.lookup("tokens|s"), Missing);
        assert_eq!(kc.get("tokens|s"), None, "an old item would bring a signed-out login back");
        assert_eq!(bundle(&kc).lookup("tokens|s"), Missing);
        assert_eq!(bundle(&kc).lookup("connector|s|a"), Found(b"c".to_vec()), "the rest stays");
    }

    #[test]
    fn memory_store_and_read_helper() {
        let m = MemorySecretStore::new();
        assert_eq!(m.lookup("k"), Missing);
        m.write("k", b"v").unwrap();
        assert_eq!(m.read("k"), Some(b"v".to_vec()));
        assert_eq!(m.keys(), vec!["k"]);
        m.delete("k");
        assert_eq!(m.read("k"), None);
        let kc = ScriptedStore::new();
        kc.put("k", b"v");
        kc.refuse(Some(5));
        assert_eq!(kc.read("k"), None, "read folds unavailable into None; lookup keeps them apart");
    }
}

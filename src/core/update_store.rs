//! The update files and the swap, ported from the macOS `AutoUpdate.swift` with one rule changed: nothing is ever
//! installed by itself. A download is verified and kept as the one staged copy only after the user said "Update",
//! and the swap happens right after, while the user waits for the restart.
//!
//! Files (`%LOCALAPPDATA%\NachoCombo\Updates`), at most two exe copies at rest:
//!   staged\NachoCombo.exe + staged\info.json      verified new version about to be swapped in (0 or 1)
//!   previous\NachoCombo.exe + previous\info.json  the version before the running one, for going back (0 or 1)
//!   download\                                     work folder of a download in progress; removed after every attempt
//!   ledger.json                                   skipped versions, the trial of a just-installed version, last check
//!
//! The swap of a running exe: Windows lets a running exe be renamed but not overwritten or deleted. So the running
//! `NachoCombo.exe` is renamed beside itself (`.NachoCombo-update-old.exe`), the new exe takes its name, the app starts
//! it and quits. The next launch (the old process gone) moves the old exe into `previous\` if it is older than the
//! one running, or deletes it. Every failure before the new exe is in place puts the old name back.
//! Plain `std::fs` only: the whole state machine is tested on Linux; the app supplies the network and the exe check.
use super::unzip;
use super::update::{self, Asset, Version};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Blocking GET (never on the UI thread). None on any failure or a status other than 200.
pub trait Fetch { fn fetch(&self, url: &str) -> Option<Vec<u8>>; }

/// What the exe says about itself (Windows: the ProductVersion of its version resource).
pub trait ExeInfo { fn version_of(&self, exe: &Path) -> Option<String>; }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Trial {
    pub version: String,
    pub previous: String,
    /// Launches of `version` that started but did not reach "healthy" (counted at the start of each launch).
    pub started_launches: u32,
}

/// What survives between launches. Every field defaults on its own, so a damaged file loses only that field.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Ledger {
    /// Versions this PC went back from: never offered again.
    pub skipped: Vec<String>,
    pub trial: Option<Trial>,
    /// Set by a swap; the first launch of that version says so once.
    pub just_updated_to: Option<String>,
    /// Set by a rollback, for the same once-only notice.
    pub rolled_back_from: Option<String>,
    /// Seconds since 1970 of the last check that reached GitHub (or failed trying).
    pub last_check: Option<u64>,
    /// The newest version the tray already announced, so a check every 6 hours does not repeat the balloon.
    pub announced: Option<String>,
}

impl Ledger {
    pub fn is_skipped(&self, v: &Version) -> bool { self.skipped.iter().any(|s| Version::parse(s).as_ref() == Some(v)) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem(pub String);
impl std::fmt::Display for Problem { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) } }
fn problem<T>(s: impl Into<String>) -> Result<T, Problem> { Err(Problem(s.into())) }

#[derive(Clone, Debug, PartialEq)]
pub struct Copy { pub version: Version, pub exe: PathBuf }

/// What the start of a launch has to do before anything else.
#[derive(Clone, Debug, PartialEq)]
pub enum LaunchAction {
    Normal,
    /// This version did not get through its first launches: go back to the previous copy, then restart.
    RollBack { from: String, to: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice { Updated { to: String }, RolledBack { from: String, to: String } }

pub struct UpdateStore { pub root: PathBuf }

impl UpdateStore {
    /// Launches a new version may start without reaching "healthy" before the next launch rolls it back.
    pub const MAX_UNHEALTHY_LAUNCHES: u32 = 2;

    pub fn new(root: impl Into<PathBuf>) -> Self { UpdateStore { root: root.into() } }

    fn staged_dir(&self) -> PathBuf { self.root.join("staged") }
    fn previous_dir(&self) -> PathBuf { self.root.join("previous") }
    fn download_dir(&self) -> PathBuf { self.root.join("download") }
    fn ledger_path(&self) -> PathBuf { self.root.join("ledger.json") }

    /// Beside the exe, same volume, so renaming the running exe there always works.
    pub fn aside_path(target: &Path) -> PathBuf {
        let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "NachoCombo".into());
        target.with_file_name(format!(".{stem}-update-old.exe"))
    }

    // ---------- ledger ----------

    pub fn ledger(&self) -> Ledger {
        let v: serde_json::Value = std::fs::read(self.ledger_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        fn field<T: serde::de::DeserializeOwned + Default>(v: &serde_json::Value, k: &str) -> T {
            v.get(k).and_then(|x| serde_json::from_value(x.clone()).ok()).unwrap_or_default()
        }
        Ledger { skipped: field(&v, "skipped"), trial: field(&v, "trial"), just_updated_to: field(&v, "justUpdatedTo"),
            rolled_back_from: field(&v, "rolledBackFrom"), last_check: field(&v, "lastCheck"), announced: field(&v, "announced") }
    }

    pub fn save_ledger(&self, l: &Ledger) {
        let _ = std::fs::create_dir_all(&self.root);
        let tmp = self.root.join("ledger.json.tmp");
        if let Ok(b) = serde_json::to_vec_pretty(l) { if std::fs::write(&tmp, b).is_ok() { let _ = std::fs::rename(&tmp, self.ledger_path()); } }
    }

    // ---------- copies ----------

    fn copy_in(&self, dir: &Path) -> Option<Copy> {
        let info: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("info.json")).ok()?).ok()?;
        let version = Version::parse(info.get("version")?.as_str()?)?;
        let exe = dir.join(update::EXE);
        exe.is_file().then_some(Copy { version, exe })
    }
    pub fn staged(&self) -> Option<Copy> { self.copy_in(&self.staged_dir()) }
    pub fn previous(&self) -> Option<Copy> { self.copy_in(&self.previous_dir()) }
    /// Exe copies the updater keeps (not counting the installed one). Never more than 2 at rest. (Tests, round trip.)
    #[allow(dead_code)]
    pub fn kept(&self) -> usize { self.staged().is_some() as usize + self.previous().is_some() as usize }

    fn write_info(dir: &Path, v: &Version) -> std::io::Result<()> {
        std::fs::write(dir.join("info.json"), serde_json::json!({ "version": v.to_string() }).to_string())
    }

    /// Removes what must not stay: an interrupted download, a staged copy that is not newer than `current` (or was
    /// skipped), a previous copy that is not older, half-written folders and anything else in the folder.
    pub fn tidy(&self, current: &Version) {
        remove(&self.download_dir());
        let ledger = self.ledger();
        match self.staged() { Some(s) if *current < s.version && !ledger.is_skipped(&s.version) => {} _ => remove(&self.staged_dir()) }
        match self.previous() { Some(p) if p.version < *current => {} _ => remove(&self.previous_dir()) }
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if !matches!(n.as_str(), "staged" | "previous" | "ledger.json") { remove(&e.path()); }
            }
        }
    }

    /// Why this copy cannot replace itself, or None. A folder only an administrator may write to (Program Files) is
    /// the usual reason; the app then points to the release page instead.
    pub fn replace_problem(target: &Path) -> Option<String> {
        let Some(dir) = target.parent() else { return Some("This copy is not in a folder.".into()) };
        if !target.is_file() { return Some(format!("{} was not found.", target.display())); }
        let probe = dir.join(".nachocombo-write-test");
        match std::fs::write(&probe, b"x") {
            Ok(()) => { let _ = std::fs::remove_file(&probe); None }
            Err(_) => Some(format!("NachoCombo cannot write to {}. Move NachoCombo.exe to a folder of your own (for example your user folder), or download new versions from the release page.", dir.display())),
        }
    }

    // ---------- download and stage ----------

    /// Downloads the release's checksums and zip, checks size and SHA-256, takes `NachoCombo.exe` out, checks that it
    /// is a program that says it is `version`, and keeps it as the one staged copy. Every failure leaves the staged
    /// and previous copies as they were and removes the download folder.
    pub fn stage(&self, version: &Version, zip: &Asset, sums: &Asset, feed: &str, fetch: &dyn Fetch, info: &dyn ExeInfo) -> Result<Copy, Problem> {
        let result = self.stage_inner(version, zip, sums, feed, fetch, info);
        remove(&self.download_dir());
        result
    }

    fn stage_inner(&self, version: &Version, zip: &Asset, sums: &Asset, feed: &str, fetch: &dyn Fetch, info: &dyn ExeInfo) -> Result<Copy, Problem> {
        if let Some(s) = self.staged() { if s.version == *version && check_exe(&s.exe, version, info).is_ok() { return Ok(s); } }
        for a in [zip, sums] {
            if !update::download_allowed(feed, &a.url) { return problem(format!("{} does not come from the release page, so it was not downloaded.", a.name)); }
        }
        remove(&self.download_dir());
        if std::fs::create_dir_all(self.download_dir()).is_err() { return problem("Could not prepare a folder for the download."); }
        let Some(sums_text) = fetch.fetch(&sums.url).and_then(|b| String::from_utf8(b).ok()) else { return problem(format!("Could not download {}.", update::CHECKSUMS)) };
        let Some(data) = fetch.fetch(&zip.url) else { return problem(format!("Could not download {}.", zip.name)) };
        if data.len() > update::MAX_DOWNLOAD { return problem(format!("{} is larger than any release.", zip.name)); }
        if let Err(p) = update::verify(&data, zip, &sums_text) { return problem(p.to_string()); }
        let exe = match unzip::extract(&data, update::EXE, update::MAX_DOWNLOAD) { Ok(b) => b, Err(e) => return problem(format!("Could not unpack {}: {e}.", zip.name)) };
        let work = self.download_dir().join(update::EXE);
        if std::fs::write(&work, &exe).is_err() { return problem("Could not save the download."); }
        check_exe(&work, version, info)?;
        // The new copy replaces an older staged one: still at most one waiting.
        remove(&self.staged_dir());
        let staged = self.staged_dir();
        let kept = std::fs::create_dir_all(&staged).is_ok() && std::fs::rename(&work, staged.join(update::EXE)).is_ok() && Self::write_info(&staged, version).is_ok();
        if !kept { remove(&staged); return problem("Could not keep the verified update."); }
        self.staged().ok_or_else(|| Problem("Could not keep the verified update.".into()))
    }

    // ---------- swap ----------

    /// Puts the staged copy in place of `target` (the running exe). The running exe stays beside it until the next
    /// launch files it as the previous copy. On any failure `target` is what it was before.
    pub fn apply_staged(&self, target: &Path, current: &Version, info: &dyn ExeInfo) -> Result<Version, Problem> {
        let Some(s) = self.staged() else { return problem("No update is waiting.") };
        let mut ledger = self.ledger();
        if !(*current < s.version) || ledger.is_skipped(&s.version) { remove(&self.staged_dir()); return problem("The waiting update is not newer than this copy."); }
        // Checked again right before the swap: the folder could have been touched since the download.
        if let Err(p) = check_exe(&s.exe, &s.version, info) { remove(&self.staged_dir()); return Err(p); }
        swap(target, &s.exe)?;
        remove(&self.staged_dir());
        ledger.trial = Some(Trial { version: s.version.to_string(), previous: current.to_string(), started_launches: 0 });
        ledger.just_updated_to = Some(s.version.to_string());
        ledger.rolled_back_from = None;
        self.save_ledger(&ledger);
        Ok(s.version)
    }

    /// Goes back to the previous copy: it replaces `target`; the running (newer) version is deleted at the next launch
    /// and never offered again. On any failure `target` stays as it is.
    pub fn roll_back(&self, target: &Path, current: &Version, info: &dyn ExeInfo) -> Result<Version, Problem> {
        let Some(p) = self.previous().filter(|p| p.version < *current) else { return problem("There is no previous version to go back to.") };
        if check_exe(&p.exe, &p.version, info).is_err() {
            remove(&self.previous_dir());
            return problem("The kept previous copy is damaged. Download an older version from the release page.");
        }
        swap(target, &p.exe)?;
        remove(&self.previous_dir());
        let mut ledger = self.ledger();
        if !ledger.is_skipped(current) { ledger.skipped.push(current.to_string()); }
        ledger.trial = None;
        ledger.just_updated_to = None;
        ledger.rolled_back_from = Some(current.to_string());
        self.save_ledger(&ledger);
        // The user chose the older version: nothing waiting is swapped in behind their back.
        remove(&self.staged_dir());
        Ok(p.version)
    }

    /// The exe swapped in could not be started: the running one gets its name back and the ledger is what it was
    /// (`before`), so this copy keeps running as if nothing happened. The exe that did not start goes back to where
    /// it came from: the staged copy after an update, the previous copy after a rollback.
    pub fn undo_swap(&self, target: &Path, before: &Ledger, version: &Version, was_rollback: bool) -> Result<(), Problem> {
        let aside = Self::aside_path(target);
        if !aside.is_file() { return problem("The old copy is no longer beside the new one."); }
        let staged = if was_rollback { self.previous_dir() } else { self.staged_dir() };
        remove(&staged);
        let kept = std::fs::create_dir_all(&staged).is_ok() && move_file(target, &staged.join(update::EXE)) && Self::write_info(&staged, version).is_ok();
        if !kept { remove(&staged); let _ = std::fs::remove_file(target); }
        if std::fs::rename(&aside, target).is_err() { return problem(format!("Could not put {} back. It is beside it as {}.", target.display(), aside.display())); }
        self.save_ledger(before);
        Ok(())
    }

    // ---------- launches ----------

    /// First thing at every launch (after the process that started this one has quit). Tidies the folder, files the
    /// exe a swap left beside this one, and says whether to roll back before starting.
    pub fn launch_began(&self, current: &Version, target: &Path, info: &dyn ExeInfo) -> LaunchAction {
        self.tidy(current);
        self.adopt_aside(target, current, info);
        let mut ledger = self.ledger();
        if let Some(trial) = ledger.trial.clone() {
            if Version::parse(&trial.version).as_ref() == Some(current) {
                if trial.started_launches >= Self::MAX_UNHEALTHY_LAUNCHES {
                    if let Some(p) = self.previous() { return LaunchAction::RollBack { from: current.to_string(), to: p.version.to_string() }; }
                    ledger.trial = None; // nothing to go back to: stop counting
                } else if let Some(t) = ledger.trial.as_mut() {
                    t.started_launches += 1;
                }
            } else {
                // Replaced by hand since: the trial is over.
                ledger.trial = None;
            }
            self.save_ledger(&ledger);
        }
        LaunchAction::Normal
    }

    /// The old exe beside this one: kept as the previous copy when it is older, deleted when it is not (a rollback
    /// leaves the newer version there). Left alone if it cannot be read yet; the next launch tries again.
    fn adopt_aside(&self, target: &Path, current: &Version, info: &dyn ExeInfo) {
        let aside = Self::aside_path(target);
        if !aside.exists() { return; }
        let version = info.version_of(&aside).and_then(|v| Version::parse(&v));
        match version {
            Some(v) if v < *current => {
                let dir = self.previous_dir();
                remove(&dir);
                let kept = std::fs::create_dir_all(&dir).is_ok() && move_file(&aside, &dir.join(update::EXE)) && Self::write_info(&dir, &v).is_ok();
                if !kept { remove(&dir); }
            }
            _ => { let _ = std::fs::remove_file(&aside); }
        }
    }

    /// The running version started fine (a while after launch, or quit normally): it is no longer on trial.
    pub fn launch_healthy(&self, current: &Version) {
        let mut ledger = self.ledger();
        if ledger.trial.as_ref().is_some_and(|t| Version::parse(&t.version).as_ref() == Some(current)) {
            ledger.trial = None;
            self.save_ledger(&ledger);
        }
    }

    /// The first-launch notice after an update or a rollback: returned once, then forgotten.
    pub fn take_notice(&self, current: &Version) -> Option<Notice> {
        let mut ledger = self.ledger();
        let (to, from) = (ledger.just_updated_to.take(), ledger.rolled_back_from.take());
        if to.is_none() && from.is_none() { return None; }
        self.save_ledger(&ledger);
        if let Some(to) = to { if Version::parse(&to).as_ref() == Some(current) { return Some(Notice::Updated { to }); } }
        from.map(|from| Notice::RolledBack { from, to: current.to_string() })
    }
}

/// A program (it starts with "MZ") whose own version resource says `version`.
pub fn check_exe(exe: &Path, version: &Version, info: &dyn ExeInfo) -> Result<(), Problem> {
    let head = std::fs::read(exe).ok().map(|b| b.get(..2) == Some(b"MZ"));
    if head != Some(true) { return problem("The download holds no Windows program."); }
    match info.version_of(exe).and_then(|v| Version::parse(&v)) {
        Some(v) if v == *version => Ok(()),
        Some(v) => problem(format!("The download is version {v}, not {version}.")),
        None => problem("The download's version could not be read."),
    }
}

/// Renames `target` aside and moves `incoming` into its place; puts `target` back if that fails.
fn swap(target: &Path, incoming: &Path) -> Result<(), Problem> {
    let aside = UpdateStore::aside_path(target);
    if aside.exists() && std::fs::remove_file(&aside).is_err() { return problem(format!("An older copy beside {} is still in use. Restart the PC and try again.", target.display())); }
    if std::fs::rename(target, &aside).is_err() { return problem(format!("NachoCombo cannot replace itself in {}.", target.parent().map(|p| p.display().to_string()).unwrap_or_default())); }
    if !move_file(incoming, target) {
        let _ = std::fs::remove_file(target);
        let _ = std::fs::rename(&aside, target);
        return problem("Could not put the new copy in place. This copy was kept.");
    }
    Ok(())
}

/// Rename, or copy and delete when the two places are on different drives.
fn move_file(from: &Path, to: &Path) -> bool {
    if std::fs::rename(from, to).is_ok() { return true; }
    if std::fs::copy(from, to).is_ok() { let _ = std::fs::remove_file(from); return true; }
    let _ = std::fs::remove_file(to);
    false
}

fn remove(p: &Path) {
    if p.is_dir() { let _ = std::fs::remove_dir_all(p); } else if p.exists() { let _ = std::fs::remove_file(p); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::sha256;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const FEED: &str = "http://127.0.0.1:1/latest";

    /// A temporary folder per test, removed at the end.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("nachocombo-update-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    /// Fake exes say their version in their bytes: "MZ fake NachoCombo 0.2.0".
    struct Fake;
    impl ExeInfo for Fake {
        fn version_of(&self, exe: &Path) -> Option<String> {
            let b = std::fs::read(exe).ok()?;
            String::from_utf8_lossy(&b).strip_prefix("MZ fake NachoCombo ").map(|v| v.trim().to_string())
        }
    }
    fn fake_exe(v: &str) -> Vec<u8> { format!("MZ fake NachoCombo {v}").into_bytes() }

    /// Serves fake release files by URL and records what was asked for.
    #[derive(Default)]
    struct Server { files: HashMap<String, Vec<u8>>, asked: RefCell<Vec<String>> }
    impl Fetch for Server {
        fn fetch(&self, url: &str) -> Option<Vec<u8>> { self.asked.borrow_mut().push(url.into()); self.files.get(url).cloned() }
    }

    /// A release of `v` whose zip holds `exe`; `sum` overrides the checksum line.
    fn publish(v: &str, exe: &[u8], sum: Option<&str>) -> (Version, Asset, Asset, Server) {
        let ver = Version::parse(v).unwrap();
        let zip_bytes = unzip::build(&[("NachoCombo.exe", exe, true), ("README.md", b"readme", true)]);
        let zip = Asset { name: update::zip_name(&ver), url: format!("http://127.0.0.1:1/files/{}", update::zip_name(&ver)), size: zip_bytes.len() as u64 };
        let sums = Asset { name: update::CHECKSUMS.into(), url: "http://127.0.0.1:1/files/SHA256SUMS.txt".into(), size: 1 };
        let line = format!("{}  {}\n", sum.map(String::from).unwrap_or_else(|| sha256::hex(&zip_bytes)), zip.name);
        let mut s = Server::default();
        s.files.insert(zip.url.clone(), zip_bytes);
        s.files.insert(sums.url.clone(), line.into_bytes());
        (ver, zip, sums, s)
    }

    fn installed(t: &Tmp, v: &str) -> PathBuf {
        let dir = t.0.join("Programs");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("NachoCombo.exe");
        std::fs::write(&exe, fake_exe(v)).unwrap();
        exe
    }
    fn version_at(p: &Path) -> String { Fake.version_of(p).unwrap_or_else(|| "-".into()) }
    fn ver(s: &str) -> Version { Version::parse(s).unwrap() }

    #[test] fn stage_verifies_and_keeps_one_copy() {
        let t = Tmp::new("stage");
        let store = UpdateStore::new(t.0.join("Updates"));
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        let copy = store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        assert_eq!(copy.version, v);
        assert_eq!(version_at(&copy.exe), "0.2.0");
        assert!(!store.root.join("download").exists(), "work folder removed");
        assert_eq!(store.kept(), 1);
        // Asking again for the same version uses the staged copy without downloading.
        let before = server.asked.borrow().len();
        store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        assert_eq!(server.asked.borrow().len(), before);
    }

    #[test] fn stage_refuses_a_bad_download_and_keeps_nothing() {
        let t = Tmp::new("refuse");
        let store = UpdateStore::new(t.0.join("Updates"));
        // Checksum mismatch.
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), Some(&"0".repeat(64)));
        assert_eq!(store.stage(&v, &zip, &sums, FEED, &server, &Fake), Err(Problem("The download does not match SHA256SUMS.txt.".into())));
        // Size other than the release lists (a proxy cut it short).
        let (v, mut zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        zip.size += 1;
        assert!(store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap_err().0.contains("bytes, the release lists"));
        // The zip holds a different version than its name says.
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.1.9"), None);
        assert_eq!(store.stage(&v, &zip, &sums, FEED, &server, &Fake), Err(Problem("The download is version 0.1.9, not 0.2.0.".into())));
        // Not a program at all.
        let (v, zip, sums, server) = publish("0.2.0", b"<html>sign in</html>", None);
        assert_eq!(store.stage(&v, &zip, &sums, FEED, &server, &Fake), Err(Problem("The download holds no Windows program.".into())));
        // Unreachable.
        let (v, zip, sums, _) = publish("0.2.0", &fake_exe("0.2.0"), None);
        assert!(store.stage(&v, &zip, &sums, FEED, &Server::default(), &Fake).is_err());
        // From somewhere else than the feed allows: nothing is even fetched.
        let (v, mut zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        zip.url = "https://example.com/NachoCombo.zip".into();
        assert!(store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap_err().0.contains("does not come from the release page"));
        assert!(server.asked.borrow().is_empty());
        assert_eq!(store.kept(), 0);
        assert!(!store.root.join("download").exists());
    }

    #[test] fn apply_swaps_and_the_next_launch_keeps_the_old_one_as_previous() {
        let t = Tmp::new("apply");
        let target = installed(&t, "0.1.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        assert_eq!(store.apply_staged(&target, &ver("0.1.0"), &Fake), Ok(ver("0.2.0")));
        assert_eq!(version_at(&target), "0.2.0");
        assert_eq!(version_at(&UpdateStore::aside_path(&target)), "0.1.0", "the running exe was only renamed");
        assert!(store.staged().is_none());
        // The new exe starts: the old one goes to previous\, the notice comes once.
        assert_eq!(store.launch_began(&ver("0.2.0"), &target, &Fake), LaunchAction::Normal);
        assert!(!UpdateStore::aside_path(&target).exists());
        assert_eq!(store.previous().map(|p| p.version), Some(ver("0.1.0")));
        assert_eq!(store.take_notice(&ver("0.2.0")), Some(Notice::Updated { to: "0.2.0".into() }));
        assert_eq!(store.take_notice(&ver("0.2.0")), None);
        store.launch_healthy(&ver("0.2.0"));
        assert_eq!(store.ledger().trial, None);
        let entries: Vec<String> = std::fs::read_dir(target.parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(entries, vec!["NachoCombo.exe"], "nothing left beside the exe");
    }

    #[test] fn at_most_two_copies_and_older_ones_are_deleted() {
        let t = Tmp::new("two");
        let target = installed(&t, "0.1.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        for (from, to) in [("0.1.0", "0.2.0"), ("0.2.0", "0.3.0")] {
            let (v, zip, sums, server) = publish(to, &fake_exe(to), None);
            store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
            store.apply_staged(&target, &ver(from), &Fake).unwrap();
            store.launch_began(&ver(to), &target, &Fake);
            store.launch_healthy(&ver(to));
        }
        let (v, zip, sums, server) = publish("0.4.0", &fake_exe("0.4.0"), None);
        store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        assert_eq!(store.kept(), 2);
        assert_eq!(store.previous().map(|p| p.version), Some(ver("0.2.0")), "0.1.0 is gone");
    }

    #[test] fn a_failed_swap_keeps_the_running_exe() {
        let t = Tmp::new("failswap");
        let target = installed(&t, "0.1.0");
        // The new exe vanished between the check and the move (antivirus, a cleaner): the old name comes back.
        assert!(swap(&target, &t.0.join("gone.exe")).is_err());
        assert_eq!(version_at(&target), "0.1.0");
        assert!(!UpdateStore::aside_path(&target).exists());
        // A staged copy that no longer checks out is refused before anything moves.
        let store = UpdateStore::new(t.0.join("Updates"));
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        let copy = store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        std::fs::write(&copy.exe, fake_exe("0.1.5")).unwrap();
        assert!(store.apply_staged(&target, &ver("0.1.0"), &Fake).is_err());
        assert_eq!(version_at(&target), "0.1.0");
        assert!(store.staged().is_none() && store.ledger().trial.is_none());
    }

    #[test] fn a_new_exe_that_does_not_start_is_undone() {
        let t = Tmp::new("undo");
        let target = installed(&t, "0.1.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        let before = store.ledger();
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        store.apply_staged(&target, &ver("0.1.0"), &Fake).unwrap();
        store.undo_swap(&target, &before, &v, false).unwrap();
        assert_eq!(version_at(&target), "0.1.0");
        assert!(!UpdateStore::aside_path(&target).exists());
        assert_eq!(store.ledger(), before);
        assert_eq!(store.staged().map(|s| s.version), Some(v), "the verified copy stays for the next try");
        // Same after a rollback whose older exe would not start: it stays the previous copy.
        store.apply_staged(&target, &ver("0.1.0"), &Fake).unwrap();
        store.launch_began(&ver("0.2.0"), &target, &Fake);
        let before = store.ledger();
        let back = store.roll_back(&target, &ver("0.2.0"), &Fake).unwrap();
        store.undo_swap(&target, &before, &back, true).unwrap();
        assert_eq!(version_at(&target), "0.2.0");
        assert_eq!(store.previous().map(|p| p.version), Some(ver("0.1.0")));
        assert_eq!(store.ledger(), before, "0.2.0 is not marked skipped");
    }

    #[test] fn two_unhealthy_launches_roll_back_and_skip_that_version() {
        let t = Tmp::new("rollback");
        let target = installed(&t, "0.1.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        let (v, zip, sums, server) = publish("0.2.0", &fake_exe("0.2.0"), None);
        store.stage(&v, &zip, &sums, FEED, &server, &Fake).unwrap();
        store.apply_staged(&target, &ver("0.1.0"), &Fake).unwrap();
        let cur = ver("0.2.0");
        assert_eq!(store.launch_began(&cur, &target, &Fake), LaunchAction::Normal); // dies before healthy
        assert_eq!(store.ledger().trial.unwrap().started_launches, 1);
        assert_eq!(store.launch_began(&cur, &target, &Fake), LaunchAction::Normal); // dies again
        assert_eq!(store.launch_began(&cur, &target, &Fake), LaunchAction::RollBack { from: "0.2.0".into(), to: "0.1.0".into() });
        assert_eq!(store.roll_back(&target, &cur, &Fake), Ok(ver("0.1.0")));
        assert_eq!(version_at(&target), "0.1.0");
        // The old version starts: the newer exe beside it is deleted, not kept.
        assert_eq!(store.launch_began(&ver("0.1.0"), &target, &Fake), LaunchAction::Normal);
        assert!(!UpdateStore::aside_path(&target).exists());
        assert_eq!(store.kept(), 0);
        let l = store.ledger();
        assert_eq!(l.skipped, vec!["0.2.0"]);
        assert_eq!(store.take_notice(&ver("0.1.0")), Some(Notice::RolledBack { from: "0.2.0".into(), to: "0.1.0".into() }));
        // And it is not offered again.
        let r = update::Release { tag: "v0.2.0".into(), name: None, body: None, page: None, draft: false, prerelease: false, assets: vec![zip.clone(), sums.clone()] };
        assert!(matches!(update::decide("0.1.0", &r, &l.skipped), update::Decision::Ignored(_)));
    }

    #[test] fn a_damaged_previous_copy_is_not_rolled_back_to() {
        let t = Tmp::new("damaged");
        let target = installed(&t, "0.2.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        let dir = store.root.join("previous");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("NachoCombo.exe"), b"MZ fake NachoCombo 0.0.9").unwrap();
        std::fs::write(dir.join("info.json"), br#"{"version":"0.1.0"}"#).unwrap();
        assert!(store.roll_back(&target, &ver("0.2.0"), &Fake).unwrap_err().0.contains("damaged"));
        assert_eq!(version_at(&target), "0.2.0");
        assert!(store.previous().is_none());
    }

    #[test] fn tidy_removes_leftovers_and_stale_copies() {
        let t = Tmp::new("tidy");
        let target = installed(&t, "0.3.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        for (d, v) in [("staged", "0.2.0"), ("previous", "0.4.0")] {
            let dir = store.root.join(d);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("NachoCombo.exe"), fake_exe(v)).unwrap();
            std::fs::write(dir.join("info.json"), format!(r#"{{"version":"{v}"}}"#)).unwrap();
        }
        std::fs::create_dir_all(store.root.join("download")).unwrap();
        std::fs::write(store.root.join("junk.tmp"), b"x").unwrap();
        std::fs::write(UpdateStore::aside_path(&target), fake_exe("0.3.0")).unwrap(); // same version: not a previous one
        store.launch_began(&ver("0.3.0"), &target, &Fake);
        assert_eq!(store.kept(), 0, "staged is not newer, previous is not older");
        assert!(!store.root.join("download").exists() && !store.root.join("junk.tmp").exists());
        assert!(!UpdateStore::aside_path(&target).exists());
    }

    #[test] fn replaced_by_hand_ends_the_trial_and_nothing_to_go_back_to_stops_counting() {
        let t = Tmp::new("trial");
        let target = installed(&t, "0.5.0");
        let store = UpdateStore::new(t.0.join("Updates"));
        store.save_ledger(&Ledger { trial: Some(Trial { version: "0.2.0".into(), previous: "0.1.0".into(), started_launches: 1 }), ..Default::default() });
        store.launch_began(&ver("0.5.0"), &target, &Fake);
        assert_eq!(store.ledger().trial, None);
        store.save_ledger(&Ledger { trial: Some(Trial { version: "0.5.0".into(), previous: "0.4.0".into(), started_launches: 2 }), ..Default::default() });
        assert_eq!(store.launch_began(&ver("0.5.0"), &target, &Fake), LaunchAction::Normal, "no previous copy: keep running");
        assert_eq!(store.ledger().trial, None);
    }

    #[test] fn ledger_survives_damage_and_unknown_fields() {
        let t = Tmp::new("ledger");
        let store = UpdateStore::new(t.0.join("Updates"));
        std::fs::create_dir_all(&store.root).unwrap();
        std::fs::write(store.root.join("ledger.json"), b"{not json").unwrap();
        assert_eq!(store.ledger(), Ledger::default());
        std::fs::write(store.root.join("ledger.json"), br#"{"skipped":["0.2.0"],"trial":42,"future":true,"lastCheck":7}"#).unwrap();
        // One damaged field must not lose the others.
        let l = store.ledger();
        assert_eq!((l.skipped, l.trial, l.last_check), (vec!["0.2.0".to_string()], None, Some(7)));
        store.save_ledger(&Ledger { announced: Some("0.2.0".into()), ..Default::default() });
        assert_eq!(store.ledger().announced.as_deref(), Some("0.2.0"));
    }

    #[test] fn a_folder_that_cannot_be_written_is_reported() {
        let t = Tmp::new("ro");
        assert!(UpdateStore::replace_problem(&t.0.join("missing.exe")).is_some());
        let target = installed(&t, "0.1.0");
        assert_eq!(UpdateStore::replace_problem(&target), None);
    }
}

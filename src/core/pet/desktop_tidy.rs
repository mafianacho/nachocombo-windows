//! Desktop tidy-up and window arrangement proposals. Port of the macOS `DesktopTidy.swift` (the engine) and the
//! platform-free part of `DesktopToolRunner` in `Gadgets/Tools/DesktopTools.swift` (arguments, answers, arrangement
//! proposals). Nacho looks, proposes (a plan), and runs only what the user approved.
//!
//! The engine looks at the files directly on the desktop, plans where each would go (`정리됨/<category>/`), moves
//! them once the user approved the plan, and keeps a journal for one-step undo. Paths are injected: the Win32 shell
//! finds the Desktop with the known-folder API (a Desktop backed up to OneDrive included) and puts the journal in the
//! app's folder. Tests use a temporary folder as the desktop.
//!
//! Rules this file keeps (tests below):
//!  - Only plain files directly on the desktop are touched: no folders, symlinks or junctions, shortcuts (`.lnk`
//!    `.url` `.appref-ms`, the Windows "apps on the desktop"), hidden or system files, `desktop.ini`, `Thumbs.db`,
//!    downloads in progress, nothing changed in the last 2 minutes, never the `정리됨` folder itself.
//!  - Files are only ever MOVED, never copied and deleted. `std::fs::rename` replaces an existing destination on
//!    Windows and Unix, so every move checks the destination is free right before it ([`move_without_replacing`]);
//!    a move that fails (file in use, another volume) leaves the file where it is and reports it.
//!  - A name clash gets a number (`name 2.ext`). Undo never overwrites either (`conflict`).
//!  - The journal holds the paths (so undo works); answers show only names and `Desktop\정리됨\<category>\`.
//!
//! The arrangement half has no window host here: the shell reads the app names of the open windows, passes them to
//! [`PetDesktopTools::suggest`], and on `arrange_apply` takes the approved proposal with
//! [`PetDesktopTools::take_arrange_plan`] and runs it through its window tools.
// i18n-exempt:begin (folder names on disk and answers read by the model / the server's approval card, not drawn)
use super::clock;
use super::commands::{PetCommandOutcome, PetCommandProblem};
use super::desktop::iso8601;
use super::json::JsonObject;
use super::media::PetIDs;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

/// Unix seconds now (the shell passes `clock::now`; tests pin it).
pub type DesktopClock = Box<dyn Fn() -> f64 + Send + Sync>;
/// Short ids for plans and tidies (8 lowercase hex characters by default).
pub type DesktopIdSource = Box<dyn Fn() -> String + Send + Sync>;
/// Moves one file without ever replacing the destination: `Err(AlreadyExists)` when the destination is taken.
/// Default [`move_without_replacing`]; the shell may pass `MoveFileExW` without the replace-existing flag (atomic
/// no-replace), tests pass failing movers.
pub type DesktopMover = Box<dyn Fn(&Path, &Path) -> io::Result<()> + Send + Sync>;

/// Where a desktop file goes. Raw values are the macOS ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DesktopCategory {
    Screenshot,
    Image,
    Document,
    Archive,
    Installer,
    Other,
}

impl DesktopCategory {
    pub const ALL: [DesktopCategory; 6] = [Self::Screenshot, Self::Image, Self::Document, Self::Archive, Self::Installer, Self::Other];

    const IMAGE_EXTENSIONS: [&'static str; 9] = ["png", "jpg", "jpeg", "heic", "gif", "webp", "tiff", "bmp", "svg"];
    const DOCUMENT_EXTENSIONS: [&'static str; 16] =
        ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "key", "pages", "numbers", "txt", "md", "rtf", "csv", "hwp", "hwpx"];
    const ARCHIVE_EXTENSIONS: [&'static str; 8] = ["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];
    /// macOS disk images and packages, and the Windows installers.
    const INSTALLER_EXTENSIONS: [&'static str; 9] = ["dmg", "pkg", "mpkg", "exe", "msi", "msix", "msixbundle", "appx", "appxbundle"];
    /// macOS screenshots, Snipping Tool (`스크린샷 2026-11-14 153045.png`, `Screenshot …`), Windows 10 Snip & Sketch
    /// (`화면 캡처 …`) and Win+Print Screen (`스크린샷(1).png`, `Screenshot (1).png`).
    const SCREENSHOT_PREFIXES: [&'static str; 5] = ["스크린샷 ", "Screenshot ", "Screen Shot ", "화면 캡처 ", "스크린샷("];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Image => "image",
            Self::Document => "document",
            Self::Archive => "archive",
            Self::Installer => "installer",
            Self::Other => "other",
        }
    }

    /// Folder under `정리됨`.
    pub fn folder_name(self) -> &'static str {
        match self {
            Self::Screenshot => "스크린샷",
            Self::Image => "이미지",
            Self::Document => "문서",
            Self::Archive => "압축",
            Self::Installer => "설치 파일",
            Self::Other => "기타",
        }
    }

    pub fn from_raw(raw: &str) -> Option<DesktopCategory> { Self::ALL.into_iter().find(|c| c.raw_value() == raw) }

    /// First match wins: screenshot, image, document, archive, installer, other. The name is compared in its
    /// precomposed form (a name copied from a Mac may carry decomposed Hangul).
    pub fn classify(name: &str) -> DesktopCategory {
        let (_, ext) = split_extension(name);
        let ext = ext.to_lowercase();
        let ext = ext.as_str();
        if Self::IMAGE_EXTENSIONS.contains(&ext) {
            let composed = compose_hangul(name);
            if Self::SCREENSHOT_PREFIXES.iter().any(|p| composed.starts_with(p)) { return Self::Screenshot; }
            return Self::Image;
        }
        if Self::DOCUMENT_EXTENSIONS.contains(&ext) { return Self::Document; }
        if Self::ARCHIVE_EXTENSIONS.contains(&ext) { return Self::Archive; }
        if Self::INSTALLER_EXTENSIONS.contains(&ext) { return Self::Installer; }
        Self::Other
    }
}

/// One file a plan would move.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopTidyCandidate {
    pub name: String,
    pub category: DesktopCategory,
    pub size: u64,
    /// Modification time in Unix seconds when the plan was made.
    pub modified: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopTidyPlan {
    pub id: String,
    pub created_at: f64,
    pub expires_at: f64,
    pub candidates: Vec<DesktopTidyCandidate>,
    /// Files on the desktop the rules leave alone (folders, links, shortcuts and hidden files are not counted).
    pub skipped_count: usize,
}

impl DesktopTidyPlan {
    pub fn total(&self) -> usize { self.candidates.len() }

    pub fn counts(&self) -> BTreeMap<DesktopCategory, usize> {
        let mut out = BTreeMap::new();
        for c in &self.candidates { *out.entry(c.category).or_insert(0) += 1; }
        out
    }
}

/// One move of a tidy, as full paths (journal only; answers never carry them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopTidyMoveRecord {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopTidySkip {
    pub name: String,
    /// `missing`, `changed`, `recent`, `not_a_file`, `move_failed`.
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopTidyApplied {
    pub tidy_id: Option<String>,
    pub moves: Vec<DesktopTidyMoveRecord>,
    pub skipped: Vec<DesktopTidySkip>,
    pub counts: BTreeMap<DesktopCategory, usize>,
    /// False when the journal could not be written: the files were moved but undo cannot restore them.
    pub journal_saved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopTidyUndoSkip {
    pub name: String,
    /// `conflict` (another file sits at the original place), `not_found` (the moved file is gone), `invalid_record`,
    /// `move_failed`.
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopTidyUndone {
    pub tidy_id: String,
    pub restored: usize,
    pub not_restored: Vec<DesktopTidyUndoSkip>,
    /// True when nothing is left to restore for this tidy (all restored, or the rest is gone).
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopTidyHistoryRow {
    pub tidy_id: String,
    pub at: f64,
    pub count: usize,
    pub undone: bool,
    /// Files still waiting to be restored after a partial undo.
    pub pending_restore: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopTidyError {
    DesktopUnreadable,
    PlanNotFound,
    PlanStale,
    TidyNotFound,
    AlreadyUndone,
    NothingToUndo,
}

impl DesktopTidyError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::DesktopUnreadable => "desktop_unreadable",
            Self::PlanNotFound => "plan_not_found",
            Self::PlanStale => "plan_stale",
            Self::TidyNotFound => "tidy_not_found",
            Self::AlreadyUndone => "already_undone",
            Self::NothingToUndo => "nothing_to_undo",
        }
    }
}

// MARK: journal file

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    entries: Vec<JournalEntry>,
}

/// Fields in key order, so the file reads like the macOS one (`sortedKeys`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct JournalEntry {
    at: String,
    moves: Vec<DesktopTidyMoveRecord>,
    tidy_id: String,
    /// Files in the tidy when it ran (`moves` shrinks after a partial undo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    undone_at: Option<String>,
}

// MARK: engine

struct Stat {
    is_regular: bool,
    hidden: bool,
    size: u64,
    modified: Option<f64>,
}

pub struct DesktopTidyEngine {
    desktop: PathBuf,
    journal: PathBuf,
    now: DesktopClock,
    ids: DesktopIdSource,
    mover: DesktopMover,
    plans: Mutex<HashMap<String, DesktopTidyPlan>>,
    /// Held while the journal is read and written.
    journal_lock: Mutex<()>,
}

impl DesktopTidyEngine {
    pub const FOLDER_NAME: &'static str = "정리됨";
    /// Files changed more recently than this are left alone (still being written or just saved).
    pub const RECENT_SECONDS: f64 = 120.0;
    pub const PLAN_LIFETIME: f64 = 600.0;
    pub const JOURNAL_DAYS: i64 = 30;
    /// File names a plan lists; the rest is only counted.
    pub const MAX_LISTED_NAMES: usize = 50;
    pub const MAX_STORED_PLANS: usize = 8;
    /// Downloads in progress (browsers, download managers) and files still being written.
    pub const DOWNLOAD_EXTENSIONS: [&'static str; 5] = ["download", "crdownload", "part", "partial", "tmp"];
    /// Windows shortcuts: the "apps on the desktop", never moved.
    pub const SHORTCUT_EXTENSIONS: [&'static str; 3] = ["lnk", "url", "appref-ms"];
    /// Explorer's own files on the desktop (compared without case).
    pub const SHELL_FILES: [&'static str; 2] = ["desktop.ini", "thumbs.db"];
    /// `Desktop\정리됨\<category>\`: the destination as answers show it (no user path).
    pub const DESTINATION: &'static str = "Desktop\\정리됨\\<category>\\";

    /// `desktop`: the user's Desktop folder; `journal`: the journal file (its folder is made on the first save).
    pub fn new(desktop: PathBuf, journal: PathBuf, now: DesktopClock) -> Self {
        DesktopTidyEngine {
            desktop,
            journal,
            now,
            ids: Box::new(default_id),
            mover: Box::new(move_without_replacing),
            plans: Mutex::new(HashMap::new()),
            journal_lock: Mutex::new(()),
        }
    }

    pub fn with_ids(mut self, ids: DesktopIdSource) -> Self { self.ids = ids; self }
    pub fn with_mover(mut self, mover: DesktopMover) -> Self { self.mover = mover; self }

    pub fn desktop(&self) -> &Path { &self.desktop }
    pub fn journal_path(&self) -> &Path { &self.journal }
    pub fn now(&self) -> f64 { (self.now)() }
    fn new_id(&self) -> String { (self.ids)() }

    // MARK: rules

    fn inspect(path: &Path) -> Option<Stat> {
        let m = fs::symlink_metadata(path).ok()?;
        let ft = m.file_type();
        let modified = m.modified().ok().map(|t| match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64(),
            Err(e) => -e.duration().as_secs_f64(),
        });
        Some(Stat { is_regular: ft.is_file() && !ft.is_symlink(), hidden: hidden_or_system_file(&m), size: m.len(), modified })
    }

    /// Names the tool never touches whatever they are: dot files, Explorer's files, shortcuts, the tidy folder.
    fn left_alone(name: &str) -> bool {
        let lower = name.to_lowercase();
        let (_, ext) = split_extension(&lower);
        name.starts_with('.')
            || Self::SHELL_FILES.contains(&lower.as_str())
            || Self::SHORTCUT_EXTENSIONS.contains(&ext)
            || compose_hangul(name) == Self::FOLDER_NAME
    }

    /// Why a desktop entry is left alone, None when it may be moved.
    fn skip_reason(name: &str, stat: Option<&Stat>, moment: f64) -> Option<&'static str> {
        let Some(st) = stat else { return Some("missing") };
        if Self::left_alone(name) || !st.is_regular || st.hidden { return Some("not_a_file"); }
        let lower = name.to_lowercase();
        if Self::DOWNLOAD_EXTENSIONS.contains(&split_extension(&lower).1) { return Some("recent"); }
        match st.modified {
            Some(m) if moment - m >= Self::RECENT_SECONDS => None,
            _ => Some("recent"),
        }
    }

    fn tidy_root(&self) -> PathBuf { self.desktop.join(Self::FOLDER_NAME) }

    // MARK: plan

    /// Looks at the desktop and keeps a plan for 10 minutes. Moves nothing.
    pub fn plan(&self) -> Result<DesktopTidyPlan, DesktopTidyError> {
        let entries = fs::read_dir(&self.desktop).map_err(|_| DesktopTidyError::DesktopUnreadable)?;
        let mut names: Vec<String> = vec![];
        for e in entries {
            let Ok(e) = e else { continue };
            // A name that is not valid Unicode cannot be written to the journal faithfully: left alone.
            if let Ok(n) = e.file_name().into_string() { names.push(n); }
        }
        names.sort();
        let moment = self.now();
        let mut candidates = vec![];
        let mut skipped = 0;
        for name in names {
            let st = Self::inspect(&self.desktop.join(&name));
            if let Some(why) = Self::skip_reason(&name, st.as_ref(), moment) {
                // Folders, links, shortcuts and hidden files are not "skipped files": they are not the tool's business.
                if why != "not_a_file" && why != "missing" { skipped += 1; }
                continue;
            }
            let Some(s) = st else { continue };
            let Some(modified) = s.modified else { continue };
            candidates.push(DesktopTidyCandidate { category: DesktopCategory::classify(&name), name, size: s.size, modified });
        }
        let plan = DesktopTidyPlan {
            id: self.new_id(),
            created_at: moment,
            expires_at: moment + Self::PLAN_LIFETIME,
            candidates,
            skipped_count: skipped,
        };
        let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        plans.retain(|_, p| p.expires_at > moment);
        if plans.len() >= Self::MAX_STORED_PLANS {
            let oldest = plans.iter().min_by(|a, b| a.1.created_at.total_cmp(&b.1.created_at)).map(|(k, _)| k.clone());
            if let Some(k) = oldest { plans.remove(&k); }
        }
        if !plan.candidates.is_empty() { plans.insert(plan.id.clone(), plan.clone()); }
        Ok(plan)
    }

    // MARK: apply

    /// Runs a plan once. Every file is checked again right before it moves; a file changed since the plan is skipped.
    pub fn apply(&self, plan_id: &str) -> Result<DesktopTidyApplied, DesktopTidyError> {
        let moment = self.now();
        let found = self.plans.lock().unwrap_or_else(|e| e.into_inner()).remove(plan_id);
        let plan = found.ok_or(DesktopTidyError::PlanNotFound)?;
        if plan.expires_at <= moment { return Err(DesktopTidyError::PlanStale); }

        let mut moves = vec![];
        let mut skipped = vec![];
        let mut counts = BTreeMap::new();
        for item in &plan.candidates {
            let from = self.desktop.join(&item.name);
            let st = Self::inspect(&from);
            if let Some(why) = Self::skip_reason(&item.name, st.as_ref(), moment) {
                skipped.push(DesktopTidySkip { name: item.name.clone(), reason: why });
                continue;
            }
            let unchanged = st.as_ref().is_some_and(|s| s.size == item.size && s.modified == Some(item.modified));
            if !unchanged {
                skipped.push(DesktopTidySkip { name: item.name.clone(), reason: "changed" });
                continue;
            }
            let moved = self.category_folder(item.category).and_then(|folder| self.move_into(&from, &folder, &item.name));
            match moved {
                Ok(target) => {
                    moves.push(DesktopTidyMoveRecord { from: path_text(&from), to: path_text(&target) });
                    *counts.entry(item.category).or_insert(0) += 1;
                }
                Err(_) => skipped.push(DesktopTidySkip { name: item.name.clone(), reason: "move_failed" }),
            }
        }
        if moves.is_empty() {
            return Ok(DesktopTidyApplied { tidy_id: None, moves, skipped, counts: BTreeMap::new(), journal_saved: true });
        }
        let tidy_id = self.new_id();
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut journal = self.load_journal(moment);
        journal.entries.push(JournalEntry { tidy_id: tidy_id.clone(), at: iso8601(moment), moves: moves.clone(), total: Some(moves.len()), undone_at: None });
        let saved = self.save_journal(&journal);
        Ok(DesktopTidyApplied { tidy_id: Some(tidy_id), moves, skipped, counts, journal_saved: saved })
    }

    /// `정리됨/<category>`, made when missing. Both must be real folders: a link or junction there could lead the
    /// files off the desktop, a file there is not ours to replace.
    fn category_folder(&self, category: DesktopCategory) -> io::Result<PathBuf> {
        let root = self.tidy_root();
        ensure_real_dir(&root)?;
        let folder = root.join(category.folder_name());
        ensure_real_dir(&folder)?;
        Ok(folder)
    }

    /// Moves `from` to the first free name in `folder`. When another file takes that name between the check and the
    /// move, the next free name is tried; nothing is ever replaced.
    fn move_into(&self, from: &Path, folder: &Path, name: &str) -> io::Result<PathBuf> {
        for _ in 0..3 {
            let target = self.unique_target(name, folder);
            match (self.mover)(from, &target) {
                Ok(()) => return Ok(target),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
    }

    /// `name`, then `name 2.ext`, `name 3.ext` … The first free name in the folder (nothing is overwritten).
    pub fn unique_target(&self, name: &str, folder: &Path) -> PathBuf {
        let first = folder.join(name);
        if !taken(&first) { return first; }
        let (base, ext) = split_extension(name);
        let named = |tail: &str| if ext.is_empty() { format!("{base} {tail}") } else { format!("{base} {tail}.{ext}") };
        for n in 2..10_000 {
            let candidate = folder.join(named(&n.to_string()));
            if !taken(&candidate) { return candidate; }
        }
        folder.join(named(&self.new_id()))
    }

    // MARK: undo

    /// Puts the files of one tidy (default: the latest one not undone) back where they were. A file whose old place is
    /// taken stays where it is and is reported; a moved file that is gone is reported too.
    pub fn undo(&self, tidy_id: Option<&str>) -> Result<DesktopTidyUndone, DesktopTidyError> {
        let moment = self.now();
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut journal = self.load_journal(moment);
        let index = match tidy_id {
            Some(wanted) => {
                let i = journal.entries.iter().position(|e| e.tidy_id == wanted).ok_or(DesktopTidyError::TidyNotFound)?;
                if journal.entries[i].undone_at.is_some() { return Err(DesktopTidyError::AlreadyUndone); }
                i
            }
            None => journal.entries.iter().rposition(|e| e.undone_at.is_none()).ok_or(DesktopTidyError::NothingToUndo)?,
        };
        let entry = journal.entries[index].clone();
        let tidy_root = self.tidy_root();
        let mut restored = 0;
        let mut failed = vec![];
        let mut remaining = vec![];
        for record in &entry.moves {
            let from = PathBuf::from(&record.from);
            let to = PathBuf::from(&record.to);
            let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // A journal is a file the user could edit: only ever move from inside 정리됨 to directly on the desktop.
            let inside = plain_path(&to) && to.starts_with(&tidy_root) && to != tidy_root;
            let onto_desktop = plain_path(&from) && from.file_name().is_some() && from.parent() == Some(self.desktop.as_path());
            if !inside || !onto_desktop {
                failed.push(DesktopTidyUndoSkip { name, reason: "invalid_record" });
                continue;
            }
            if !taken(&to) {
                failed.push(DesktopTidyUndoSkip { name, reason: "not_found" });
                continue;
            }
            if taken(&from) {
                failed.push(DesktopTidyUndoSkip { name, reason: "conflict" });
                remaining.push(record.clone());
                continue;
            }
            match (self.mover)(&to, &from) {
                Ok(()) => restored += 1,
                Err(e) => {
                    let reason = if e.kind() == io::ErrorKind::AlreadyExists { "conflict" } else { "move_failed" };
                    failed.push(DesktopTidyUndoSkip { name, reason });
                    remaining.push(record.clone());
                }
            }
        }
        let complete = remaining.is_empty();
        journal.entries[index].moves = remaining;
        if complete { journal.entries[index].undone_at = Some(iso8601(moment)); }
        let _ = self.save_journal(&journal);
        Ok(DesktopTidyUndone { tidy_id: entry.tidy_id, restored, not_restored: failed, complete })
    }

    // MARK: history and journal

    /// Tidies of the last 30 days, newest first.
    pub fn history(&self) -> Vec<DesktopTidyHistoryRow> {
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let journal = self.load_journal(self.now());
        journal.entries.iter().rev().filter_map(|e| {
            let at = clock::parse_rfc3339(&e.at)?;
            Some(DesktopTidyHistoryRow {
                tidy_id: e.tidy_id.clone(),
                at,
                count: e.total.unwrap_or(e.moves.len()),
                undone: e.undone_at.is_some(),
                pending_restore: if e.undone_at.is_none() { e.moves.len() } else { 0 },
            })
        }).collect()
    }

    /// Drops journal entries older than 30 days (the record only, never a file). Called at app start (by
    /// [`PetDesktopTools::new`]) and with every journal read.
    pub fn prune_journal(&self) {
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let before = self.load_journal_raw();
        let kept = Self::prune(&before, self.now());
        if kept.entries.len() != before.entries.len() { let _ = self.save_journal(&kept); }
    }

    fn load_journal_raw(&self) -> Journal {
        let Ok(data) = fs::read(&self.journal) else { return Journal::default() };
        if let Ok(journal) = serde_json::from_slice::<Journal>(&data) { return journal; }
        // Unreadable: keep it next to the journal under another name instead of overwriting it.
        let dir = self.journal.parent().map(Path::to_path_buf).unwrap_or_default();
        let stamp = self.now().floor() as i64;
        let mut aside = dir.join(format!("journal-unreadable-{stamp}.json"));
        let mut n = 2;
        while taken(&aside) && n < 100 {
            aside = dir.join(format!("journal-unreadable-{stamp}-{n}.json"));
            n += 1;
        }
        let _ = (self.mover)(&self.journal, &aside);
        Journal::default()
    }

    fn prune(journal: &Journal, moment: f64) -> Journal {
        let limit = moment - (Self::JOURNAL_DAYS * 86_400) as f64;
        Journal { entries: journal.entries.iter().filter(|e| clock::parse_rfc3339(&e.at).is_some_and(|at| at >= limit)).cloned().collect() }
    }

    fn load_journal(&self, moment: f64) -> Journal { Self::prune(&self.load_journal_raw(), moment) }

    /// Writes a temporary file next to the journal, then puts it in the journal's place (the journal is this app's
    /// own record, the one file this tool replaces).
    fn save_journal(&self, journal: &Journal) -> bool {
        let Some(dir) = self.journal.parent() else { return false };
        if fs::create_dir_all(dir).is_err() { return false; }
        let Ok(text) = serde_json::to_string_pretty(journal) else { return false };
        let name = self.journal.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "journal.json".into());
        let tmp = dir.join(format!("{name}.saving"));
        fs::write(&tmp, text).is_ok() && fs::rename(&tmp, &self.journal).is_ok()
    }
}

/// Moves `from` to `to` unless something (a file, a folder, a link) is at `to`: `std::fs::rename` itself would
/// replace it. Never copies: across volumes, or when the file is in use, the move fails and the file stays.
pub fn move_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination exists")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::rename(from, to)
}

/// Anything at `path` (a dangling link too), or a path we cannot look at.
fn taken(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(e) => e.kind() != io::ErrorKind::NotFound,
    }
}

fn ensure_real_dir(path: &Path) -> io::Result<()> {
    let real = |m: &fs::Metadata| m.file_type().is_dir() && !m.file_type().is_symlink();
    match fs::symlink_metadata(path) {
        Ok(m) if real(&m) => Ok(()),
        Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "not a folder")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            if fs::symlink_metadata(path).is_ok_and(|m| real(&m)) { Ok(()) } else { Err(io::Error::other("not a folder")) }
        }
        Err(e) => Err(e),
    }
}

/// No `.` or `..` in a journal path.
fn plain_path(p: &Path) -> bool { p.components().all(|c| !matches!(c, Component::ParentDir | Component::CurDir)) }

fn path_text(p: &Path) -> String { p.to_string_lossy().into_owned() }

/// FILE_ATTRIBUTE_HIDDEN (0x2) or FILE_ATTRIBUTE_SYSTEM (0x4).
pub fn hidden_or_system(attributes: u32) -> bool { attributes & 0x6 != 0 }

#[cfg(windows)]
fn hidden_or_system_file(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    hidden_or_system(m.file_attributes())
}

/// Elsewhere only the dot rule hides a file.
#[cfg(not(windows))]
fn hidden_or_system_file(_m: &fs::Metadata) -> bool { false }

/// `("name", "ext")` for `name.ext`; `("name", "")` without an extension, for `.name` and for `name.`.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// Canonical composition of conjoining Hangul jamo into syllables (the Hangul part of NFC).
pub fn compose_hangul(s: &str) -> String {
    const S_BASE: u32 = 0xAC00;
    const L_BASE: u32 = 0x1100;
    const V_BASE: u32 = 0x1161;
    const T_BASE: u32 = 0x11A7;
    const L_COUNT: u32 = 19;
    const V_COUNT: u32 = 21;
    const T_COUNT: u32 = 28;
    const S_COUNT: u32 = L_COUNT * V_COUNT * T_COUNT;
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if let Some(last) = out.last_mut() {
            let l = *last as u32;
            if (L_BASE..L_BASE + L_COUNT).contains(&l) && (V_BASE..V_BASE + V_COUNT).contains(&cp) {
                if let Some(syl) = char::from_u32(S_BASE + ((l - L_BASE) * V_COUNT + (cp - V_BASE)) * T_COUNT) { *last = syl; continue; }
            }
            if (S_BASE..S_BASE + S_COUNT).contains(&l) && (l - S_BASE).is_multiple_of(T_COUNT) && (T_BASE + 1..T_BASE + T_COUNT).contains(&cp) {
                if let Some(syl) = char::from_u32(l + cp - T_BASE) { *last = syl; continue; }
            }
        }
        out.push(c);
    }
    out.into_iter().collect()
}

/// 8 lowercase hex characters.
fn default_id() -> String { PetIDs::new_uuid().replace('-', "").chars().take(8).collect() }

fn prefix(s: &str, n: usize) -> String { s.chars().take(n).collect() }

// MARK: tools

/// The checked arguments of a desktop tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetDesktopArgs {
    pub plan_id: Option<String>,
    pub tidy_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArrangePlan {
    apps: Vec<String>,
    arrangement: &'static str,
    expires_at: f64,
}

/// The desktop gadget's runner without the window host: owns the engine and the arrangement proposals.
pub struct PetDesktopTools {
    engine: DesktopTidyEngine,
    arrange_plans: Mutex<HashMap<String, ArrangePlan>>,
}

impl PetDesktopTools {
    pub const ACTIONS: [&'static str; 6] = ["tidy_plan", "tidy_apply", "tidy_undo", "tidy_history", "arrange_suggest", "arrange_apply"];
    pub const MAX_ARRANGE_APPS: usize = 9;
    pub const ARRANGE_LIFETIME: f64 = DesktopTidyEngine::PLAN_LIFETIME;

    /// Prunes the journal (30 days) as the macOS runner does at app start.
    pub fn new(engine: DesktopTidyEngine) -> Self {
        engine.prune_journal();
        PetDesktopTools { engine, arrange_plans: Mutex::new(HashMap::new()) }
    }

    pub fn engine(&self) -> &DesktopTidyEngine { &self.engine }

    // MARK: arguments

    /// Closed field lists: `plan_id` for the two apply actions (required), `tidy_id` for undo, nothing else.
    pub fn check(action: &str, params: Option<&Value>) -> Result<PetDesktopArgs, PetCommandProblem> {
        if !Self::ACTIONS.contains(&action) { return Err(PetCommandProblem::UnknownCapability(prefix(action, 64))); }
        let empty = JsonObject::new();
        let fields = match params {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(o)) => o,
            Some(_) => return Err(PetCommandProblem::BadParams("params must be an object".into())),
        };
        let allowed: &[&str] = match action {
            "tidy_apply" | "arrange_apply" => &["plan_id"],
            "tidy_undo" => &["tidy_id"],
            _ => &[],
        };
        let mut extra: Vec<String> = fields.keys().filter(|k| !allowed.contains(&k.as_str())).map(|k| prefix(k, 64)).collect();
        if !extra.is_empty() {
            extra.sort();
            return Err(PetCommandProblem::BadParams(format!("unknown field {}", extra.join(", "))));
        }
        let text = |key: &str| -> Result<Option<String>, PetCommandProblem> {
            let Some(value) = fields.get(key) else { return Ok(None) };
            let Some(raw) = value.as_str() else { return Err(PetCommandProblem::BadParams(format!("{key} must be a string"))) };
            let s = raw.trim();
            if !(1..=32).contains(&s.chars().count()) { return Err(PetCommandProblem::BadParams(format!("{key} must be 1-32 characters"))); }
            Ok(Some(s.to_string()))
        };
        let out = PetDesktopArgs { plan_id: text("plan_id")?, tidy_id: text("tidy_id")? };
        if matches!(action, "tidy_apply" | "arrange_apply") && out.plan_id.is_none() {
            return Err(PetCommandProblem::BadParams("plan_id is required".into()));
        }
        Ok(out)
    }

    // MARK: tidy

    /// `tidy_plan`, `tidy_apply`, `tidy_undo`, `tidy_history` (file work: call it off the UI thread).
    pub fn run_tidy(&self, action: &str, args: &PetDesktopArgs) -> PetCommandOutcome {
        match action {
            "tidy_plan" => Self::plan_outcome(&self.engine.plan()),
            "tidy_apply" => Self::apply_outcome(&self.engine.apply(args.plan_id.as_deref().unwrap_or(""))),
            "tidy_undo" => Self::undo_outcome(&self.engine.undo(args.tidy_id.as_deref())),
            "tidy_history" => Self::history_outcome(&self.engine.history()),
            _ => PetCommandOutcome::new(false, format!("unknown desktop action {}", prefix(action, 64))),
        }
    }

    pub fn failure(error: DesktopTidyError) -> PetCommandOutcome {
        let (message, hint, ko) = match error {
            DesktopTidyError::DesktopUnreadable => ("Nacho Combo cannot read the Desktop folder.",
                "tell the user the Desktop folder could not be read: check that it exists and, if the Desktop is backed up to OneDrive, that OneDrive is signed in",
                "바탕화면 폴더를 읽지 못했어요. 바탕화면 폴더가 있는지, 바탕화면을 OneDrive에 백업 중이라면 OneDrive에 로그인되어 있는지 확인해 주세요."),
            DesktopTidyError::PlanNotFound => ("No such plan (already used, or the app restarted).",
                "call tidy_plan again and show the plan to the user", "그 계획이 없어요. 계획을 다시 만들게요."),
            DesktopTidyError::PlanStale => ("The plan is older than 10 minutes.",
                "call tidy_plan again, show the new plan and ask the user again", "계획이 10분이 지나 만료됐어요. 다시 만들어 보여 드릴게요."),
            DesktopTidyError::TidyNotFound => ("No such tidy in the last 30 days.", "call tidy_history for the list", "그 정리 기록이 없어요."),
            DesktopTidyError::AlreadyUndone => ("That tidy was already undone.", "do not retry", "이미 되돌린 정리예요."),
            DesktopTidyError::NothingToUndo => ("There is no tidy to undo in the last 30 days.", "do not retry", "되돌릴 정리가 없어요."),
        };
        Self::reason(error.reason(), message, hint, ko)
    }

    fn reason(code: &str, message: &str, hint: &str, ko: &str) -> PetCommandOutcome {
        outcome(false, message, json!({"reason": code, "hint": hint, "guide_ko": ko}))
    }

    fn categories(counts: &BTreeMap<DesktopCategory, usize>) -> Value {
        Value::Array(DesktopCategory::ALL.iter().filter_map(|c| {
            let n = *counts.get(c)?;
            (n > 0).then(|| json!({"category": c.raw_value(), "folder": c.folder_name(), "count": n}))
        }).collect())
    }

    pub fn plan_outcome(result: &Result<DesktopTidyPlan, DesktopTidyError>) -> PetCommandOutcome {
        let plan = match result {
            Err(e) => return Self::failure(*e),
            Ok(p) => p,
        };
        let max = DesktopTidyEngine::MAX_LISTED_NAMES;
        let files: Vec<Value> = plan.candidates.iter().take(max).map(|c| json!({"name": prefix(&c.name, 200), "category": c.category.raw_value()})).collect();
        let mut details = json!({
            "total": plan.total(), "skipped": plan.skipped_count, "destination": DesktopTidyEngine::DESTINATION,
            "categories": Self::categories(&plan.counts()), "files": files, "more": plan.total().saturating_sub(max),
        });
        if plan.total() == 0 { return outcome(true, "There is nothing to tidy on the desktop.", details); }
        details["plan_id"] = json!(plan.id);
        details["expires_in_seconds"] = json!(DesktopTidyEngine::PLAN_LIFETIME as i64);
        outcome(true, &format!("{} desktop files could be tidied. Nothing was moved.", plan.total()), details)
    }

    pub fn apply_outcome(result: &Result<DesktopTidyApplied, DesktopTidyError>) -> PetCommandOutcome {
        let done = match result {
            Err(e) => return Self::failure(*e),
            Ok(d) => d,
        };
        let skipped: Vec<Value> = done.skipped.iter().take(DesktopTidyEngine::MAX_LISTED_NAMES)
            .map(|s| json!({"name": prefix(&s.name, 200), "reason": s.reason})).collect();
        let mut details = json!({
            "moved": done.moves.len(), "skipped_count": done.skipped.len(), "skipped": skipped,
            "categories": Self::categories(&done.counts),
        });
        let Some(id) = &done.tidy_id else {
            return outcome(true, "Nothing was moved: every file changed or is in use since the plan.", details);
        };
        details["tidy_id"] = json!(id);
        details["undo_with"] = json!("desktop_tidy_undo");
        if !done.journal_saved {
            details["journal_saved"] = json!(false);
            return outcome(true, &format!("Moved {} files, but the undo record could not be saved: tell the user undo is not available.", done.moves.len()), details);
        }
        outcome(true, &format!("Moved {} files into {}. Undo with desktop_tidy_undo.", done.moves.len(), DesktopTidyEngine::FOLDER_NAME), details)
    }

    pub fn undo_label_ko(reason: &str) -> &'static str {
        match reason {
            "conflict" => "되돌리지 못함 (원래 자리에 다른 파일이 있어요)",
            "not_found" => "찾지 못함 (옮긴 파일이 없어요)",
            _ => "되돌리지 못함",
        }
    }

    pub fn undo_outcome(result: &Result<DesktopTidyUndone, DesktopTidyError>) -> PetCommandOutcome {
        let undone = match result {
            Err(e) => return Self::failure(*e),
            Ok(u) => u,
        };
        let rows: Vec<Value> = undone.not_restored.iter().take(DesktopTidyEngine::MAX_LISTED_NAMES)
            .map(|s| json!({"name": prefix(&s.name, 200), "reason": s.reason, "label_ko": Self::undo_label_ko(s.reason)})).collect();
        let tail = if undone.not_restored.is_empty() { String::new() } else { format!(" {} files were not restored (see not_restored).", undone.not_restored.len()) };
        outcome(true, &format!("Restored {} files to the desktop.{tail}", undone.restored),
            json!({"tidy_id": undone.tidy_id, "restored": undone.restored, "complete": undone.complete, "not_restored": rows}))
    }

    pub fn history_outcome(rows: &[DesktopTidyHistoryRow]) -> PetCommandOutcome {
        let entries: Vec<Value> = rows.iter().map(|r| json!({
            "tidy_id": r.tidy_id, "at": iso8601(r.at), "count": r.count, "undone": r.undone, "pending_restore": r.pending_restore,
        })).collect();
        let message = if rows.is_empty() { "No desktop tidy in the last 30 days.".to_string() } else { format!("{} desktop tidies in the last 30 days.", rows.len()) };
        outcome(true, &message, json!({"entries": entries}))
    }

    // MARK: arrange

    /// Two or three apps side by side, four in quadrants, more in a grid (window arrangement raw values).
    pub fn arrangement_for_count(count: usize) -> &'static str {
        match count {
            0..=3 => "columns",
            4 => "quadrants",
            _ => "grid",
        }
    }

    /// `arrange_suggest` from the app names of the open windows (never window titles). None: the shell could not
    /// read the windows. Moves nothing; keeps the proposal for 10 minutes.
    pub fn suggest(&self, app_names: Option<Vec<String>>) -> PetCommandOutcome {
        let Some(names) = app_names else {
            return Self::reason("windows_unreadable", "Nacho Combo could not read the open windows.",
                "try once more; if it fails again tell the user the open windows could not be read",
                "열린 창 목록을 읽지 못했어요. 잠시 뒤 다시 해 볼게요.");
        };
        let mut apps: Vec<String> = vec![];
        let mut seen = std::collections::HashSet::new();
        for n in names {
            if apps.len() >= Self::MAX_ARRANGE_APPS { break; }
            if !n.is_empty() && seen.insert(n.clone()) { apps.push(prefix(&n, 200)); }
        }
        if apps.len() < 2 {
            return outcome(true, "Fewer than two apps have open windows, so there is nothing to arrange.", json!({"apps": apps}));
        }
        let moment = self.engine.now();
        let id = self.engine.new_id();
        let arrangement = Self::arrangement_for_count(apps.len());
        let plan = ArrangePlan { apps: apps.clone(), arrangement, expires_at: moment + Self::ARRANGE_LIFETIME };
        {
            let mut plans = self.arrange_plans.lock().unwrap_or_else(|e| e.into_inner());
            plans.retain(|_, p| p.expires_at > moment);
            if plans.len() >= DesktopTidyEngine::MAX_STORED_PLANS {
                let oldest = plans.iter().min_by(|a, b| a.1.expires_at.total_cmp(&b.1.expires_at)).map(|(k, _)| k.clone());
                if let Some(k) = oldest { plans.remove(&k); }
            }
            plans.insert(id.clone(), plan);
        }
        outcome(true, &format!("Proposed {arrangement} for {} apps. Nothing was moved.", apps.len()), json!({
            "plan_id": id, "expires_in_seconds": Self::ARRANGE_LIFETIME as i64, "arrangement": arrangement, "apps": apps,
            "window_arguments": {"apps": apps, "arrangement": arrangement},
        }))
    }

    /// `arrange_apply`: the approved proposal, once. The shell runs `(apps, arrangement)` through its window tools and
    /// answers with [`PetDesktopTools::arrange_applied`] of their result.
    pub fn take_arrange_plan(&self, plan_id: &str) -> Result<(Vec<String>, String), PetCommandOutcome> {
        let moment = self.engine.now();
        let found = self.arrange_plans.lock().unwrap_or_else(|e| e.into_inner()).remove(plan_id);
        let Some(plan) = found else { return Err(Self::failure(DesktopTidyError::PlanNotFound)) };
        if plan.expires_at <= moment { return Err(Self::failure(DesktopTidyError::PlanStale)); }
        Ok((plan.apps, plan.arrangement.to_string()))
    }

    /// The window tools' answer for an arrangement, with `undo_with: window_undo` added.
    pub fn arrange_applied(window_outcome: PetCommandOutcome) -> PetCommandOutcome {
        let mut details = window_outcome.details.unwrap_or_default();
        details.insert("undo_with".into(), json!("window_undo"));
        PetCommandOutcome { details: Some(details), ..window_outcome }
    }
}

fn outcome(ok: bool, message: &str, details: Value) -> PetCommandOutcome {
    PetCommandOutcome { ok, message: message.to_string(), gadgets: None, details: details.as_object().cloned() }
}
// i18n-exempt:end

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const START: f64 = 1_800_000_000.0;
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A temporary desktop and journal, removed when the test ends (also on failure).
    struct Desk {
        root: PathBuf,
        desktop: PathBuf,
        journal: PathBuf,
        clock: Arc<Mutex<f64>>,
    }

    impl Drop for Desk {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
    }

    impl Desk {
        fn new() -> Desk {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir().join(format!("nacho-desktop-tidy-{}-{stamp}-{n}", std::process::id()));
            let desktop = root.join("Desktop");
            fs::create_dir_all(&desktop).unwrap();
            Desk { journal: root.join("support").join("desktop-tidy").join("journal.json"), root, desktop, clock: Arc::new(Mutex::new(START)) }
        }
        fn now(&self) -> f64 { *self.clock.lock().unwrap() }
        fn advance(&self, secs: f64) { *self.clock.lock().unwrap() += secs; }
        fn clock(&self) -> DesktopClock {
            let c = self.clock.clone();
            Box::new(move || *c.lock().unwrap())
        }
        fn engine(&self) -> DesktopTidyEngine { DesktopTidyEngine::new(self.desktop.clone(), self.journal.clone(), self.clock()) }
        fn tools(&self) -> PetDesktopTools { PetDesktopTools::new(self.engine()) }
        fn make_in(&self, dir: &Path, name: &str, text: &str, age: f64) -> PathBuf {
            let p = dir.join(name);
            fs::write(&p, text).unwrap();
            set_age(&p, self.now() - age);
            p
        }
        fn make(&self, name: &str) -> PathBuf { self.make_in(&self.desktop, name, "x", 3600.0) }
        fn make_text(&self, name: &str, text: &str) -> PathBuf { self.make_in(&self.desktop, name, text, 3600.0) }
        fn tidied(&self, c: DesktopCategory, name: &str) -> PathBuf { self.desktop.join("정리됨").join(c.folder_name()).join(name) }
        fn on_desktop(&self, name: &str) -> PathBuf { self.desktop.join(name) }
        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.desktop).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            v.sort();
            v
        }
        /// Every regular file under the test root: name → content.
        fn contents(&self) -> BTreeMap<String, String> {
            fn walk(dir: &Path, out: &mut BTreeMap<String, String>) {
                for e in fs::read_dir(dir).unwrap() {
                    let p = e.unwrap().path();
                    let m = fs::symlink_metadata(&p).unwrap();
                    if m.is_dir() { walk(&p, out); } else if m.is_file() {
                        out.insert(p.file_name().unwrap().to_string_lossy().into_owned(), fs::read_to_string(&p).unwrap());
                    }
                }
            }
            let mut out = BTreeMap::new();
            walk(&self.desktop, &mut out);
            out
        }
    }

    fn set_age(p: &Path, at: f64) {
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs_f64(at)).unwrap();
    }

    fn exists(p: &Path) -> bool { fs::symlink_metadata(p).is_ok() }
    fn read(p: &Path) -> String { fs::read_to_string(p).unwrap() }
    fn plan(e: &DesktopTidyEngine) -> DesktopTidyPlan { e.plan().expect("plan") }
    fn applied(e: &DesktopTidyEngine, id: &str) -> DesktopTidyApplied { e.apply(id).expect("apply") }
    fn plan_names(p: &DesktopTidyPlan) -> Vec<&str> { p.candidates.iter().map(|c| c.name.as_str()).collect() }
    fn d(o: &PetCommandOutcome) -> &JsonObject { o.details.as_ref().expect("details") }
    fn reason(o: &PetCommandOutcome) -> String { d(o).get("reason").and_then(Value::as_str).unwrap_or("").to_string() }
    fn args(v: Value) -> PetDesktopArgs { PetDesktopTools::check("tidy_apply", Some(&v)).unwrap() }
    fn plan_args(o: &PetCommandOutcome) -> PetDesktopArgs { PetDesktopArgs { plan_id: d(o)["plan_id"].as_str().map(String::from), tidy_id: None } }

    /// Canonical decomposition of Hangul syllables (what a Mac may hand over).
    fn decompose_hangul(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            let cp = c as u32;
            if (0xAC00..0xAC00 + 11172).contains(&cp) {
                let i = cp - 0xAC00;
                out.push(char::from_u32(0x1100 + i / 588).unwrap());
                out.push(char::from_u32(0x1161 + (i % 588) / 28).unwrap());
                if !i.is_multiple_of(28) { out.push(char::from_u32(0x11A7 + i % 28).unwrap()); }
            } else { out.push(c); }
        }
        out
    }

    // MARK: classification

    #[test]
    fn classification_first_match_wins_and_screenshots_come_first() {
        use DesktopCategory::*;
        let table = [
            ("스크린샷 2026-11-14 오후 3.04.05.png", Screenshot), ("Screenshot 2026-11-14 at 15.04.png", Screenshot),
            ("Screen Shot 2020-01-01.JPG", Screenshot), ("Screenshot.png", Image), ("스크린샷.png", Image),
            ("Screenshot 1.pdf", Document), ("사진.HEIC", Image), ("a.svg", Image), ("보고서.hwpx", Document), ("a.numbers", Document),
            ("a.tar", Archive), ("a.tgz", Archive), ("Nacho.dmg", Installer), ("x.mpkg", Installer), ("movie.mov", Other), ("README", Other),
        ];
        for (name, want) in table { assert_eq!(DesktopCategory::classify(name), want, "{name}"); }
        let decomposed = decompose_hangul("스크린샷 1.png");
        assert_ne!(decomposed, "스크린샷 1.png");
        assert_eq!(DesktopCategory::classify(&decomposed), Screenshot, "decomposed Hangul");
        assert_eq!(DesktopCategory::ALL.map(|c| c.folder_name()), ["스크린샷", "이미지", "문서", "압축", "설치 파일", "기타"]);
        assert_eq!(DesktopCategory::ALL.map(|c| c.raw_value()), ["screenshot", "image", "document", "archive", "installer", "other"]);
        for c in DesktopCategory::ALL { assert_eq!(DesktopCategory::from_raw(c.raw_value()), Some(c)); }
    }

    #[test]
    fn windows_screenshot_names_and_installers() {
        use DesktopCategory::*;
        let table = [
            ("스크린샷 2026-11-14 153045.png", Screenshot), ("Screenshot 2026-11-14 153045.png", Screenshot),
            ("화면 캡처 2021-01-01 123456.jpg", Screenshot), ("스크린샷(1).png", Screenshot), ("Screenshot (3).png", Screenshot),
            ("화면 캡처 2021.pdf", Document), ("화면.png", Image),
            ("Setup.exe", Installer), ("app.MSI", Installer), ("a.msix", Installer), ("a.msixbundle", Installer),
            ("a.appx", Installer), ("a.appxbundle", Installer), ("a.pkg", Installer), ("a.dmg", Installer),
        ];
        for (name, want) in table { assert_eq!(DesktopCategory::classify(name), want, "{name}"); }
    }

    #[test]
    fn hangul_composition_is_the_identity_on_composed_text() {
        for s in ["정리됨", "스크린샷 1.png", "abc", "ㄱㅏ", ""] { assert_eq!(compose_hangul(s), s); }
        assert_eq!(compose_hangul(&decompose_hangul("정리됨 설치 파일")), "정리됨 설치 파일");
    }

    // MARK: plan

    #[test]
    fn plan_takes_only_plain_old_files_and_moves_nothing() {
        let t = Desk::new();
        t.make("보고서.pdf");
        t.make("사진.png");
        t.make("메모");
        t.make(".DS_Store");
        t.make(".hidden.txt");
        t.make("받는 중.zip.crdownload");
        t.make("a.download");
        t.make("b.part");
        t.make_in(&t.desktop, "방금.txt", "x", 30.0);
        t.make_in(&t.desktop, "경계.txt", "x", 119.0);
        t.make_in(&t.desktop, "정확히.txt", "x", 120.0);
        fs::create_dir_all(t.desktop.join("폴더")).unwrap();
        fs::create_dir_all(t.desktop.join("정리됨/문서")).unwrap();
        let before = t.names();

        let p = plan(&t.engine());

        let mut want = vec!["메모", "보고서.pdf", "사진.png", "정확히.txt"];
        want.sort();
        assert_eq!(plan_names(&p), want, "old regular files only");
        assert_eq!(p.counts()[&DesktopCategory::Document], 2);
        assert_eq!(p.counts()[&DesktopCategory::Image], 1);
        assert_eq!(p.counts()[&DesktopCategory::Other], 1);
        assert_eq!(p.skipped_count, 5, "crdownload, .download, .part, the 30 s file and the 119 s file");
        assert_eq!(t.names(), before, "planning moves nothing");
        assert_eq!(p.expires_at - p.created_at, 600.0);
    }

    #[test]
    fn plan_skips_windows_shortcuts_shell_files_downloads_and_the_tidy_folder() {
        let t = Desk::new();
        for n in ["Chrome.lnk", "사이트.url", "App.appref-ms", "LINK.LNK", "desktop.ini", "Desktop.ini", "Thumbs.db"] { t.make(n); }
        for n in ["a.partial", "b.tmp", "c.TMP"] { t.make(n); }
        t.make("keep.pdf");
        fs::create_dir_all(t.desktop.join("정리됨")).unwrap();
        let before = t.contents();

        let p = plan(&t.engine());

        assert_eq!(plan_names(&p), ["keep.pdf"]);
        assert_eq!(p.skipped_count, 3, "downloads count as skipped; shortcuts and Explorer's files are not the tool's business");
        assert_eq!(t.contents(), before);
    }

    #[test]
    fn a_file_named_like_the_tidy_folder_is_left_alone() {
        let t = Desk::new();
        t.make("정리됨");
        t.make(&decompose_hangul("정리됨"));
        let p = plan(&t.engine());
        assert_eq!(p.total(), 0);
        assert_eq!(p.skipped_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_moved() {
        let t = Desk::new();
        let target = t.make("보고서.pdf");
        std::os::unix::fs::symlink(&target, t.desktop.join("바로가기.pdf")).unwrap();
        std::os::unix::fs::symlink(&t.root, t.desktop.join("junction")).unwrap();
        let p = plan(&t.engine());
        assert_eq!(plan_names(&p), ["보고서.pdf"]);
        assert_eq!(p.skipped_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_unicode_is_left_alone() {
        use std::os::unix::ffi::OsStrExt;
        let t = Desk::new();
        let p = t.desktop.join(std::ffi::OsStr::from_bytes(b"bad\xff.pdf"));
        fs::write(&p, "x").unwrap();
        set_age(&p, t.now() - 3600.0);
        t.make("ok.pdf");
        assert_eq!(plan_names(&plan(&t.engine())), ["ok.pdf"]);
    }

    #[test]
    fn hidden_and_system_attributes() {
        assert!(hidden_or_system(0x2));
        assert!(hidden_or_system(0x4));
        assert!(hidden_or_system(0x2 | 0x20));
        assert!(!hidden_or_system(0x20), "archive");
        assert!(!hidden_or_system(0x80), "normal");
        assert!(!hidden_or_system(0x1), "read-only is a plain file");
    }

    #[cfg(windows)]
    #[test]
    fn hidden_files_are_left_alone_on_windows() {
        let t = Desk::new();
        let h = t.make("hidden.pdf");
        assert!(std::process::Command::new("attrib").arg("+h").arg(&h).status().unwrap().success());
        t.make("shown.pdf");
        assert_eq!(plan_names(&plan(&t.engine())), ["shown.pdf"]);
    }

    #[test]
    fn plan_of_an_empty_desktop_has_no_plan_to_apply() {
        let t = Desk::new();
        let e = t.engine();
        let p = plan(&e);
        assert_eq!(p.total(), 0);
        assert_eq!(e.apply(&p.id), Err(DesktopTidyError::PlanNotFound));
    }

    #[test]
    fn unreadable_desktop() {
        let t = Desk::new();
        let e = DesktopTidyEngine::new(t.root.join("missing"), t.journal.clone(), t.clock());
        assert_eq!(e.plan(), Err(DesktopTidyError::DesktopUnreadable));
    }

    #[test]
    fn only_eight_plans_are_kept() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        let first = plan(&e);
        let mut ids = vec![];
        for _ in 0..8 { t.advance(1.0); ids.push(plan(&e).id); }
        assert_eq!(e.apply(&first.id), Err(DesktopTidyError::PlanNotFound), "the oldest plan is dropped");
        assert_eq!(applied(&e, ids.last().unwrap()).moves.len(), 1);
    }

    // MARK: apply

    #[test]
    fn apply_moves_into_category_folders_and_keeps_the_content() {
        let t = Desk::new();
        t.make_text("스크린샷 1.png", "shot");
        t.make_text("보고서.pdf", "report");
        t.make_text("Nacho.dmg", "dmg");
        t.make_text("Setup.exe", "exe");
        let e = t.engine();
        let p = plan(&e);
        let done = applied(&e, &p.id);

        assert_eq!(done.moves.len(), 4);
        assert_eq!(done.counts[&DesktopCategory::Screenshot], 1);
        assert_eq!(done.counts[&DesktopCategory::Installer], 2);
        assert!(done.skipped.is_empty());
        assert!(done.journal_saved);
        assert_eq!(read(&t.tidied(DesktopCategory::Screenshot, "스크린샷 1.png")), "shot");
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "보고서.pdf")), "report");
        assert_eq!(read(&t.tidied(DesktopCategory::Installer, "Nacho.dmg")), "dmg");
        assert_eq!(read(&t.tidied(DesktopCategory::Installer, "Setup.exe")), "exe");
        assert!(!exists(&t.on_desktop("보고서.pdf")));
        assert_eq!(e.apply(&p.id), Err(DesktopTidyError::PlanNotFound), "a plan runs once");
    }

    #[test]
    fn apply_moves_only_the_files_of_the_plan() {
        let t = Desk::new();
        t.make("planned.pdf");
        let e = t.engine();
        let p = plan(&e);
        t.make("new after the plan.pdf");
        let done = applied(&e, &p.id);
        assert_eq!(done.moves.len(), 1);
        assert!(exists(&t.tidied(DesktopCategory::Document, "planned.pdf")));
        assert!(exists(&t.on_desktop("new after the plan.pdf")), "not in the plan, not moved");
        assert!(!exists(&t.tidied(DesktopCategory::Document, "new after the plan.pdf")));
    }

    #[test]
    fn name_clash_gets_a_number_and_nothing_is_overwritten() {
        let t = Desk::new();
        let e = t.engine();
        for (name, text) in [("a.pdf", "first"), ("a.pdf", "second"), ("a.pdf", "third"), ("noext", "n1"), ("noext", "n2"), ("이름.txt", "k1"), ("이름.txt", "k2")] {
            t.make_text(name, text);
            let p = plan(&e);
            applied(&e, &p.id);
        }
        use DesktopCategory::*;
        assert_eq!(read(&t.tidied(Document, "a.pdf")), "first");
        assert_eq!(read(&t.tidied(Document, "a 2.pdf")), "second");
        assert_eq!(read(&t.tidied(Document, "a 3.pdf")), "third");
        assert_eq!(read(&t.tidied(Other, "noext")), "n1");
        assert_eq!(read(&t.tidied(Other, "noext 2")), "n2");
        assert_eq!(read(&t.tidied(Document, "이름.txt")), "k1");
        assert_eq!(read(&t.tidied(Document, "이름 2.txt")), "k2");
    }

    #[test]
    fn a_plan_is_stale_after_ten_minutes() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        let p = plan(&e);
        t.advance(601.0);
        assert_eq!(e.apply(&p.id), Err(DesktopTidyError::PlanStale));
        assert!(exists(&t.on_desktop("a.pdf")), "nothing moved");
        assert_eq!(e.apply("deadbeef"), Err(DesktopTidyError::PlanNotFound));
        let again = plan(&e);
        t.advance(599.0);
        assert_eq!(applied(&e, &again.id).moves.len(), 1, "just inside the lifetime still runs");
    }

    #[test]
    fn files_changed_after_the_plan_are_skipped_and_reported() {
        let t = Desk::new();
        t.make("same.pdf");
        t.make_text("edited.pdf", "old");
        t.make_text("grown.txt", "old");
        t.make("touched.txt");
        t.make("gone.txt");
        t.make("swapped.txt");
        let e = t.engine();
        let p = plan(&e);
        // after the plan
        let edited = t.make_text("edited.pdf", "new"); // same size
        set_age(&edited, t.now() - 1800.0); // a different time
        t.make_text("grown.txt", "old and more");
        set_age(&t.on_desktop("touched.txt"), t.now() - 10.0);
        fs::remove_file(t.on_desktop("gone.txt")).unwrap();
        fs::remove_file(t.on_desktop("swapped.txt")).unwrap();
        fs::create_dir(t.on_desktop("swapped.txt")).unwrap();

        let done = applied(&e, &p.id);

        let moved: Vec<String> = done.moves.iter().map(|m| Path::new(&m.from).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(moved, ["same.pdf"]);
        let reasons: BTreeMap<&str, &str> = done.skipped.iter().map(|s| (s.name.as_str(), s.reason)).collect();
        assert_eq!(reasons["edited.pdf"], "changed");
        assert_eq!(reasons["grown.txt"], "changed");
        assert_eq!(reasons["touched.txt"], "recent");
        assert_eq!(reasons["gone.txt"], "missing");
        assert_eq!(reasons["swapped.txt"], "not_a_file");
        assert_eq!(read(&t.on_desktop("edited.pdf")), "new", "skipped files stay");
    }

    #[test]
    fn nothing_moved_writes_no_journal_entry() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        let p = plan(&e);
        fs::remove_file(t.on_desktop("a.pdf")).unwrap();
        let done = applied(&e, &p.id);
        assert_eq!(done.tidy_id, None);
        assert!(e.history().is_empty());
        assert!(!exists(&t.journal));
    }

    #[test]
    fn a_failed_move_leaves_the_file_and_copies_nothing() {
        let t = Desk::new();
        t.make_text("in use.pdf", "busy");
        t.make_text("free.pdf", "free");
        let e = t.engine().with_mover(Box::new(|from: &Path, to: &Path| {
            if from.ends_with("in use.pdf") { Err(io::Error::other("sharing violation")) } else { move_without_replacing(from, to) }
        }));
        let p = plan(&e);
        let done = applied(&e, &p.id);
        assert_eq!(done.moves.len(), 1);
        assert_eq!(done.skipped, [DesktopTidySkip { name: "in use.pdf".into(), reason: "move_failed" }]);
        assert_eq!(read(&t.on_desktop("in use.pdf")), "busy");
        assert!(!exists(&t.tidied(DesktopCategory::Document, "in use.pdf")), "no copy");
        assert_eq!(t.contents().len(), 2);
    }

    #[test]
    fn a_file_that_takes_the_name_at_the_last_moment_is_not_overwritten() {
        let t = Desk::new();
        t.make_text("a.pdf", "mine");
        let first = Arc::new(Mutex::new(true));
        let e = t.engine().with_mover(Box::new(move |from: &Path, to: &Path| {
            let mut f = first.lock().unwrap();
            if *f {
                *f = false;
                fs::write(to, "racer").unwrap();
            }
            move_without_replacing(from, to)
        }));
        let p = plan(&e);
        let done = applied(&e, &p.id);
        assert_eq!(done.moves.len(), 1);
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "a.pdf")), "racer", "not replaced");
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "a 2.pdf")), "mine");
    }

    #[test]
    fn move_without_replacing_refuses_a_taken_destination() {
        let t = Desk::new();
        let a = t.make_text("a.txt", "A");
        let b = t.make_text("b.txt", "B");
        let err = move_without_replacing(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(read(&a), "A");
        assert_eq!(read(&b), "B");
        fs::create_dir(t.on_desktop("dir")).unwrap();
        assert_eq!(move_without_replacing(&a, &t.on_desktop("dir")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        move_without_replacing(&a, &t.on_desktop("c.txt")).unwrap();
        assert_eq!(read(&t.on_desktop("c.txt")), "A");
    }

    #[test]
    fn a_file_where_the_tidy_folder_goes_is_not_replaced() {
        let t = Desk::new();
        let e = t.engine();
        t.make("a.pdf");
        // 정리됨 is left out of plans, but it may appear between plan and apply.
        let p = plan(&e);
        t.make_text("정리됨", "a file the user made");
        let done = applied(&e, &p.id);
        assert_eq!(done.moves.len(), 0);
        assert_eq!(done.skipped[0].reason, "move_failed");
        assert_eq!(read(&t.on_desktop("정리됨")), "a file the user made");
        assert!(exists(&t.on_desktop("a.pdf")));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_tidy_folder_is_not_followed() {
        let t = Desk::new();
        let elsewhere = t.root.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, t.desktop.join("정리됨")).unwrap();
        t.make("a.pdf");
        let e = t.engine();
        let p = plan(&e);
        let done = applied(&e, &p.id);
        assert_eq!(done.moves.len(), 0);
        assert!(exists(&t.on_desktop("a.pdf")));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    // MARK: journal

    #[test]
    fn journal_holds_tidy_id_time_and_moves() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        let p = plan(&e);
        let done = applied(&e, &p.id);
        let json: Value = serde_json::from_slice(&fs::read(&t.journal).unwrap()).unwrap();
        let entry = &json["entries"][0];
        assert_eq!(entry["tidy_id"].as_str(), done.tidy_id.as_deref());
        assert_eq!(entry["at"].as_str(), Some(iso8601(START).as_str()));
        assert_eq!(entry["total"], json!(1));
        assert!(entry.get("undone_at").is_none(), "absent until undone");
        assert_eq!(entry["moves"][0]["from"].as_str(), Some(path_text(&t.on_desktop("a.pdf")).as_str()));
        assert_eq!(entry["moves"][0]["to"].as_str(), Some(path_text(&t.tidied(DesktopCategory::Document, "a.pdf")).as_str()));
        let rows = e.history();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[0].at, START);
        assert!(!rows[0].undone);
        e.undo(None).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(&t.journal).unwrap()).unwrap();
        assert_eq!(json["entries"][0]["undone_at"].as_str(), Some(iso8601(START).as_str()));
    }

    #[test]
    fn entries_older_than_thirty_days_are_dropped_but_files_are_not_touched() {
        let t = Desk::new();
        t.make_text("a.pdf", "keep me");
        let e = t.engine();
        applied(&e, &plan(&e).id);
        t.advance(29.0 * 86_400.0);
        assert_eq!(e.history().len(), 1);
        t.make("b.pdf");
        applied(&e, &plan(&e).id);
        assert_eq!(e.history().len(), 2);
        t.advance(2.0 * 86_400.0);
        e.prune_journal();
        assert_eq!(e.history().len(), 1);
        let on_disk: Value = serde_json::from_slice(&fs::read(&t.journal).unwrap()).unwrap();
        assert_eq!(on_disk["entries"].as_array().unwrap().len(), 1, "the record is gone from the file");
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "a.pdf")), "keep me", "the file is not touched");
    }

    #[test]
    fn an_unreadable_journal_is_kept_aside_not_overwritten() {
        let t = Desk::new();
        fs::create_dir_all(t.journal.parent().unwrap()).unwrap();
        fs::write(&t.journal, "{ not json").unwrap();
        t.make("a.pdf");
        let e = t.engine();
        applied(&e, &plan(&e).id);
        let names: Vec<String> = fs::read_dir(t.journal.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        let aside = names.iter().find(|n| n.starts_with("journal-unreadable-")).expect("kept aside");
        assert_eq!(read(&t.journal.parent().unwrap().join(aside)), "{ not json");
        assert_eq!(e.history().len(), 1);
    }

    // MARK: undo

    #[test]
    fn undo_puts_everything_back_and_marks_the_tidy() {
        let t = Desk::new();
        t.make_text("a.pdf", "A");
        t.make_text("b.png", "B");
        let e = t.engine();
        let done = applied(&e, &plan(&e).id);
        let result = e.undo(None).unwrap();
        assert_eq!(Some(&result.tidy_id), done.tidy_id.as_ref());
        assert_eq!(result.restored, 2);
        assert!(result.complete);
        assert!(result.not_restored.is_empty());
        assert_eq!(read(&t.on_desktop("a.pdf")), "A");
        assert_eq!(read(&t.on_desktop("b.png")), "B");
        assert!(exists(&t.desktop.join("정리됨/문서")), "the empty category folder stays");
        assert!(e.history()[0].undone);
        assert_eq!(e.undo(None), Err(DesktopTidyError::NothingToUndo));
        assert_eq!(e.undo(done.tidy_id.as_deref()), Err(DesktopTidyError::AlreadyUndone));
        assert_eq!(e.undo(Some("nope")), Err(DesktopTidyError::TidyNotFound));
    }

    #[test]
    fn undo_takes_the_latest_not_undone_tidy_or_the_named_one() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        let first = applied(&e, &plan(&e).id);
        t.make("b.pdf");
        let second = applied(&e, &plan(&e).id);
        assert_eq!(e.undo(first.tidy_id.as_deref()).unwrap().restored, 1);
        assert!(exists(&t.on_desktop("a.pdf")));
        assert!(!exists(&t.on_desktop("b.pdf")));
        assert_eq!(Some(e.undo(None).unwrap().tidy_id), second.tidy_id, "the latest one not undone yet");
    }

    #[test]
    fn undo_never_overwrites_and_reports_what_it_could_not_do() {
        let t = Desk::new();
        t.make_text("taken.pdf", "moved");
        t.make_text("lost.pdf", "lost");
        t.make_text("fine.pdf", "fine");
        let e = t.engine();
        let done = applied(&e, &plan(&e).id);
        t.make_text("taken.pdf", "NEW FILE ON DESKTOP");
        fs::remove_file(t.tidied(DesktopCategory::Document, "lost.pdf")).unwrap();

        let result = e.undo(None).unwrap();

        assert_eq!(result.restored, 1);
        assert!(!result.complete, "the conflict is still to be restored");
        let reasons: BTreeMap<&str, &str> = result.not_restored.iter().map(|s| (s.name.as_str(), s.reason)).collect();
        assert_eq!(reasons, BTreeMap::from([("taken.pdf", "conflict"), ("lost.pdf", "not_found")]));
        assert_eq!(read(&t.on_desktop("taken.pdf")), "NEW FILE ON DESKTOP", "not overwritten");
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "taken.pdf")), "moved", "stays in 정리됨");
        assert_eq!(read(&t.on_desktop("fine.pdf")), "fine");
        let row = e.history()[0].clone();
        assert!(!row.undone);
        assert_eq!(row.pending_restore, 1);
        assert_eq!(row.count, 3, "the count of the tidy itself");
        // The user clears the conflict; undo again finishes the rest only.
        fs::rename(t.on_desktop("taken.pdf"), t.on_desktop("taken-new.pdf")).unwrap();
        let again = e.undo(done.tidy_id.as_deref()).unwrap();
        assert_eq!(again.restored, 1);
        assert!(again.complete);
        assert_eq!(read(&t.on_desktop("taken.pdf")), "moved");
        assert_eq!(read(&t.on_desktop("taken-new.pdf")), "NEW FILE ON DESKTOP");
    }

    #[test]
    fn undo_reports_a_conflict_that_appears_at_the_last_moment() {
        let t = Desk::new();
        t.make_text("a.pdf", "mine");
        let armed = Arc::new(Mutex::new(false));
        let trigger = armed.clone();
        let e = t.engine().with_mover(Box::new(move |from: &Path, to: &Path| {
            if *trigger.lock().unwrap() { fs::write(to, "racer").unwrap(); }
            move_without_replacing(from, to)
        }));
        applied(&e, &plan(&e).id);
        *armed.lock().unwrap() = true;
        let result = e.undo(None).unwrap();
        assert_eq!(result.not_restored[0].reason, "conflict");
        assert_eq!(read(&t.on_desktop("a.pdf")), "racer");
        assert_eq!(read(&t.tidied(DesktopCategory::Document, "a.pdf")), "mine");
    }

    #[test]
    fn a_tampered_journal_cannot_move_files_outside_the_tidy_folders() {
        let t = Desk::new();
        let outside = t.make_in(&t.root, "secret.txt", "outside", 3600.0);
        fs::create_dir_all(t.journal.parent().unwrap()).unwrap();
        let tidy_doc = t.tidied(DesktopCategory::Other, "x");
        let sneaky = t.desktop.join("정리됨").join("..").join("..").join("secret.txt");
        let record = json!({"entries": [{"tidy_id": "evil0001", "at": iso8601(START), "moves": [
            {"from": path_text(&t.on_desktop("stolen.txt")), "to": path_text(&outside)},
            {"from": path_text(&t.root.join("elsewhere.txt")), "to": path_text(&tidy_doc)},
            {"from": path_text(&t.on_desktop("stolen2.txt")), "to": path_text(&sneaky)},
        ]}]});
        fs::write(&t.journal, serde_json::to_vec(&record).unwrap()).unwrap();
        let result = t.engine().undo(None).unwrap();
        assert_eq!(result.restored, 0);
        assert_eq!(result.not_restored.iter().map(|s| s.reason).collect::<Vec<_>>(), ["invalid_record"; 3]);
        assert!(exists(&outside));
        assert!(!exists(&t.on_desktop("stolen.txt")));
        assert!(!exists(&t.on_desktop("stolen2.txt")));
    }

    // MARK: nothing is deleted

    #[test]
    fn no_file_is_ever_deleted_or_changed_across_tidy_and_undo() {
        let t = Desk::new();
        let mut expected = BTreeMap::new();
        for (i, name) in ["a.pdf", "b.png", "c.zip", "d.dmg", "e", "스크린샷 1.png", "f.exe", "g.lnk", "desktop.ini"].iter().enumerate() {
            t.make_text(name, &format!("content {i}"));
            expected.insert(name.to_string(), format!("content {i}"));
        }
        let e = t.engine();
        assert_eq!(t.contents(), expected);
        applied(&e, &plan(&e).id);
        assert_eq!(t.contents(), expected, "after tidy");
        assert!(exists(&t.on_desktop("g.lnk")) && exists(&t.on_desktop("desktop.ini")), "shortcuts stay");
        e.undo(None).unwrap();
        assert_eq!(t.contents(), expected, "after undo");
        applied(&e, &plan(&e).id);
        assert_eq!(t.contents(), expected, "after a second tidy");
        // A clash: the second file gets a number, both survive.
        t.make_text("a.pdf", "again");
        applied(&e, &plan(&e).id);
        assert_eq!(t.contents().len(), expected.len() + 1);
    }

    #[test]
    fn the_engine_source_never_removes_or_copies_a_file() {
        let src = include_str!("desktop_tidy.rs");
        let body = src.split(concat!("#[cfg", "(test)]")).next().unwrap();
        for forbidden in ["remove_file", "remove_dir", "fs::copy", "set_len", "truncate", "File::create", "MOVEFILE_REPLACE_EXISTING", "COPY_ALLOWED"] {
            assert!(!body.contains(forbidden), "desktop_tidy.rs uses {forbidden}");
        }
        assert!(body.contains("fs::rename(from, to)"));
    }

    // MARK: tools: arguments

    #[test]
    fn bad_arguments_are_refused() {
        let check = |a: &str, v: Value| PetDesktopTools::check(a, Some(&v));
        let bad = |a: &str, v: Value| matches!(check(a, v), Err(PetCommandProblem::BadParams(_)));
        assert_eq!(PetDesktopTools::check("tidy_apply", None), Err(PetCommandProblem::BadParams("plan_id is required".into())));
        assert!(PetDesktopTools::check("tidy_apply", None).unwrap_err().message().contains("plan_id is required"));
        assert!(bad("tidy_apply", json!({"plan_id": 5})));
        assert!(bad("tidy_apply", json!({"plan_id": ""})));
        assert!(bad("tidy_apply", json!({"plan_id": "   "})));
        assert!(bad("tidy_apply", json!({"plan_id": "a".repeat(33)})));
        assert!(bad("tidy_apply", json!({"plan_id": "a", "path": "/"})));
        assert!(bad("tidy_plan", json!({"path": "/"})), "no folder argument: only the desktop");
        assert!(bad("tidy_undo", json!({"plan_id": "a"})));
        assert!(bad("tidy_history", json!({"days": 5})));
        assert!(bad("arrange_apply", json!({})));
        assert!(bad("arrange_suggest", json!({"apps": ["a"]})));
        assert!(bad("tidy_plan", json!([1])));
        assert_eq!(check("tidy_apply", json!({"plan_id": "z", "b": 1, "a": 2})), Err(PetCommandProblem::BadParams("unknown field a, b".into())));
        assert!(matches!(check("tidy_delete", json!({})), Err(PetCommandProblem::UnknownCapability(_))));
        // good ones
        assert_eq!(check("tidy_apply", json!({"plan_id": " a1b2c3d4 "})).unwrap().plan_id.as_deref(), Some("a1b2c3d4"));
        assert_eq!(check("tidy_apply", json!({"plan_id": "가".repeat(32)})).unwrap().plan_id.map(|s| s.chars().count()), Some(32));
        assert_eq!(check("tidy_undo", json!({"tidy_id": "abc"})).unwrap(), PetDesktopArgs { plan_id: None, tidy_id: Some("abc".into()) });
        assert_eq!(PetDesktopTools::check("tidy_undo", None).unwrap(), PetDesktopArgs::default());
        assert_eq!(PetDesktopTools::check("tidy_plan", Some(&Value::Null)).unwrap(), PetDesktopArgs::default());
        for a in PetDesktopTools::ACTIONS {
            if a != "tidy_apply" && a != "arrange_apply" { assert!(check(a, json!({})).is_ok(), "{a}"); }
        }
    }

    // MARK: tools: tidy

    #[test]
    fn plan_then_apply_then_history_then_undo() {
        let t = Desk::new();
        let tools = t.tools();
        t.make("스크린샷 1.png");
        t.make("보고서.pdf");
        t.make("a.zip");
        let plan = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        assert!(plan.ok);
        assert_eq!(d(&plan)["total"], json!(3));
        assert_eq!(d(&plan)["expires_in_seconds"], json!(600));
        assert_eq!(d(&plan)["destination"], json!("Desktop\\정리됨\\<category>\\"));
        let cats: Vec<String> = d(&plan)["categories"].as_array().unwrap().iter()
            .map(|c| format!("{}:{}", c["category"].as_str().unwrap(), c["count"])).collect();
        assert_eq!(cats, ["screenshot:1", "document:1", "archive:1"]);
        assert_eq!(d(&plan)["categories"][1]["folder"], json!("문서"));
        assert_eq!(d(&plan)["files"].as_array().unwrap().len(), 3);
        assert_eq!(d(&plan)["files"][0], json!({"name": "a.zip", "category": "archive"}));
        assert_eq!(d(&plan)["more"], json!(0));
        assert_eq!(plan.message, "3 desktop files could be tidied. Nothing was moved.");
        assert!(exists(&t.on_desktop("보고서.pdf")), "planning moves nothing");

        let apply = tools.run_tidy("tidy_apply", &plan_args(&plan));
        assert!(apply.ok, "{}", apply.message);
        assert_eq!(d(&apply)["moved"], json!(3));
        assert_eq!(d(&apply)["skipped_count"], json!(0));
        assert_eq!(d(&apply)["undo_with"], json!("desktop_tidy_undo"));
        assert_eq!(apply.message, "Moved 3 files into 정리됨. Undo with desktop_tidy_undo.");
        let tidy_id = d(&apply)["tidy_id"].as_str().unwrap().to_string();
        assert!(!exists(&t.on_desktop("보고서.pdf")));

        let history = tools.run_tidy("tidy_history", &PetDesktopArgs::default());
        let entries = d(&history)["entries"].as_array().unwrap().clone();
        assert_eq!(entries[0]["tidy_id"], json!(tidy_id));
        assert_eq!(entries[0]["count"], json!(3));
        assert_eq!(entries[0]["undone"], json!(false));
        assert_eq!(entries[0]["pending_restore"], json!(3));
        assert_eq!(entries[0]["at"], json!(iso8601(START)));
        assert!(entries[0].get("moves").is_none(), "history has no file names");
        assert_eq!(history.message, "1 desktop tidies in the last 30 days.");

        let undo = tools.run_tidy("tidy_undo", &PetDesktopArgs::default());
        assert!(undo.ok);
        assert_eq!(d(&undo)["restored"], json!(3));
        assert_eq!(d(&undo)["complete"], json!(true));
        assert_eq!(undo.message, "Restored 3 files to the desktop.");
        assert!(exists(&t.on_desktop("보고서.pdf")));
        assert_eq!(d(&tools.run_tidy("tidy_history", &PetDesktopArgs::default()))["entries"][0]["undone"], json!(true));
    }

    #[test]
    fn answers_never_carry_the_user_path() {
        let t = Desk::new();
        let tools = t.tools();
        t.make("a.pdf");
        t.make("b.txt");
        let plan = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        let apply = tools.run_tidy("tidy_apply", &plan_args(&plan));
        t.make("a.pdf");
        let undo = tools.run_tidy("tidy_undo", &PetDesktopArgs::default());
        let history = tools.run_tidy("tidy_history", &PetDesktopArgs::default());
        let root = path_text(&t.root);
        for o in [plan, apply, undo, history] {
            let text = serde_json::to_string(&o.result_json()).unwrap();
            assert!(!text.contains(&root), "{text}");
            assert!(!text.contains("Desktop/") && !text.contains("~/"), "{text}");
        }
    }

    #[test]
    fn plan_lists_fifty_names_and_counts_the_rest() {
        let t = Desk::new();
        for i in 0..57 { t.make(&format!("f{i:02}.txt")); }
        let plan = t.tools().run_tidy("tidy_plan", &PetDesktopArgs::default());
        assert_eq!(d(&plan)["total"], json!(57));
        assert_eq!(d(&plan)["files"].as_array().unwrap().len(), 50);
        assert_eq!(d(&plan)["more"], json!(7));
    }

    #[test]
    fn an_empty_desktop_gives_no_plan_id() {
        let t = Desk::new();
        let plan = t.tools().run_tidy("tidy_plan", &PetDesktopArgs::default());
        assert!(plan.ok);
        assert_eq!(d(&plan)["total"], json!(0));
        assert!(d(&plan).get("plan_id").is_none());
        assert!(d(&plan).get("expires_in_seconds").is_none());
        assert_eq!(plan.message, "There is nothing to tidy on the desktop.");
    }

    #[test]
    fn stale_and_unknown_plans_are_refused_with_a_reason() {
        let t = Desk::new();
        let tools = t.tools();
        t.make("a.pdf");
        let plan = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        t.advance(601.0);
        let stale = tools.run_tidy("tidy_apply", &plan_args(&plan));
        assert!(!stale.ok);
        assert_eq!(reason(&stale), "plan_stale");
        assert!(d(&stale)["hint"].is_string());
        assert!(d(&stale)["guide_ko"].is_string());
        assert!(exists(&t.on_desktop("a.pdf")));
        assert_eq!(reason(&tools.run_tidy("tidy_apply", &args(json!({"plan_id": "zzzz"})))), "plan_not_found");
        assert_eq!(reason(&tools.run_tidy("tidy_undo", &PetDesktopArgs::default())), "nothing_to_undo");
        let named = PetDesktopArgs { tidy_id: Some("nope".into()), ..Default::default() };
        assert_eq!(reason(&tools.run_tidy("tidy_undo", &named)), "tidy_not_found");
        assert!(!tools.run_tidy("tidy_delete", &PetDesktopArgs::default()).ok);
    }

    #[test]
    fn every_error_has_a_reason_hint_and_korean_guide() {
        use DesktopTidyError::*;
        for e in [DesktopUnreadable, PlanNotFound, PlanStale, TidyNotFound, AlreadyUndone, NothingToUndo] {
            let o = PetDesktopTools::failure(e);
            assert!(!o.ok);
            assert_eq!(reason(&o), e.reason());
            assert!(!d(&o)["hint"].as_str().unwrap().is_empty());
            assert!(d(&o)["guide_ko"].as_str().unwrap().chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)));
            assert!(!o.message.contains("System Settings"));
        }
    }

    #[test]
    fn undo_reports_what_it_could_not_restore_in_korean() {
        let t = Desk::new();
        let tools = t.tools();
        t.make_text("a.pdf", "moved");
        let plan = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        assert!(tools.run_tidy("tidy_apply", &plan_args(&plan)).ok);
        t.make_text("a.pdf", "new");
        let undo = tools.run_tidy("tidy_undo", &PetDesktopArgs::default());
        assert!(undo.ok);
        assert_eq!(d(&undo)["restored"], json!(0));
        assert_eq!(d(&undo)["complete"], json!(false));
        let row = &d(&undo)["not_restored"][0];
        assert_eq!(row["reason"], json!("conflict"));
        assert_eq!(row["name"], json!("a.pdf"));
        assert!(row["label_ko"].as_str().unwrap().starts_with("되돌리지 못함"));
        assert_eq!(undo.message, "Restored 0 files to the desktop. 1 files were not restored (see not_restored).");
        assert_eq!(read(&t.on_desktop("a.pdf")), "new");
        assert!(PetDesktopTools::undo_label_ko("not_found").starts_with("찾지 못함"));
    }

    #[test]
    fn an_unreadable_desktop_tells_what_to_check() {
        let t = Desk::new();
        let tools = t.tools();
        fs::remove_dir(&t.desktop).unwrap();
        let out = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        assert!(!out.ok);
        assert_eq!(reason(&out), "desktop_unreadable");
        let guide = d(&out)["guide_ko"].as_str().unwrap();
        assert!(guide.contains("OneDrive") && guide.contains("바탕화면 폴더"), "{guide}");
        assert!(!guide.contains("시스템 설정"), "no macOS settings path");
    }

    #[test]
    fn apply_answers_skips_and_a_lost_journal() {
        let t = Desk::new();
        t.make("a.pdf");
        t.make("b.pdf");
        let tools = t.tools();
        let plan = tools.run_tidy("tidy_plan", &PetDesktopArgs::default());
        fs::remove_file(t.on_desktop("b.pdf")).unwrap();
        // The journal's folder is a file: the move works, the record cannot be written.
        fs::create_dir_all(t.journal.parent().unwrap().parent().unwrap()).unwrap();
        fs::write(t.journal.parent().unwrap(), "blocker").unwrap();
        let apply = tools.run_tidy("tidy_apply", &plan_args(&plan));
        assert!(apply.ok);
        assert_eq!(d(&apply)["moved"], json!(1));
        assert_eq!(d(&apply)["skipped_count"], json!(1));
        assert_eq!(d(&apply)["skipped"], json!([{"name": "b.pdf", "reason": "missing"}]));
        assert_eq!(d(&apply)["journal_saved"], json!(false));
        assert!(apply.message.contains("undo is not available"));

        let nothing = PetDesktopTools::apply_outcome(&Ok(DesktopTidyApplied {
            tidy_id: None, moves: vec![], skipped: vec![DesktopTidySkip { name: "x".into(), reason: "changed" }], counts: BTreeMap::new(), journal_saved: true,
        }));
        assert!(nothing.ok);
        assert!(d(&nothing).get("tidy_id").is_none());
        assert_eq!(nothing.message, "Nothing was moved: every file changed or is in use since the plan.");
    }

    // MARK: tools: arrange

    #[test]
    fn suggest_then_apply_uses_the_window_tool_arguments() {
        let t = Desk::new();
        let tools = t.tools();
        let apps = vec!["Safari".to_string(), "Notes".into(), "Terminal".into(), "Mail".into()];
        let suggest = tools.suggest(Some(apps.clone()));
        assert!(suggest.ok);
        assert_eq!(d(&suggest)["arrangement"], json!("quadrants"));
        assert_eq!(d(&suggest)["apps"], json!(apps));
        assert_eq!(d(&suggest)["expires_in_seconds"], json!(600));
        assert_eq!(d(&suggest)["window_arguments"], json!({"apps": apps, "arrangement": "quadrants"}));
        assert_eq!(suggest.message, "Proposed quadrants for 4 apps. Nothing was moved.");
        let id = d(&suggest)["plan_id"].as_str().unwrap().to_string();
        assert!(PetDesktopTools::check("arrange_apply", Some(&json!({"plan_id": id}))).is_ok());

        assert_eq!(tools.take_arrange_plan(&id), Ok((apps.clone(), "quadrants".to_string())));
        let again = tools.take_arrange_plan(&id).unwrap_err();
        assert_eq!(reason(&again), "plan_not_found", "runs once");

        let mut windows = JsonObject::new();
        windows.insert("windows".into(), json!([{"window": "Safari #1"}]));
        let answered = PetDesktopTools::arrange_applied(PetCommandOutcome { ok: true, message: "Arranged".into(), gadgets: None, details: Some(windows) });
        assert!(answered.ok);
        assert_eq!(answered.message, "Arranged");
        assert_eq!(d(&answered)["undo_with"], json!("window_undo"));
        assert_eq!(d(&answered)["windows"][0]["window"], json!("Safari #1"));
        assert_eq!(d(&PetDesktopTools::arrange_applied(PetCommandOutcome::new(false, "x")))["undo_with"], json!("window_undo"));
    }

    #[test]
    fn arrangement_follows_the_app_count() {
        assert_eq!(PetDesktopTools::arrangement_for_count(2), "columns");
        assert_eq!(PetDesktopTools::arrangement_for_count(3), "columns");
        assert_eq!(PetDesktopTools::arrangement_for_count(4), "quadrants");
        assert_eq!(PetDesktopTools::arrangement_for_count(5), "grid");
        let t = Desk::new();
        let mut names: Vec<String> = (1..=15).map(|i| format!("App {i}")).collect();
        names.push("App 1".into());
        names.push(String::new());
        let suggest = t.tools().suggest(Some(names));
        assert_eq!(d(&suggest)["apps"].as_array().unwrap().len(), PetDesktopTools::MAX_ARRANGE_APPS);
        assert_eq!(d(&suggest)["arrangement"], json!("grid"));
    }

    #[test]
    fn suggest_drops_empty_and_repeated_names_and_shortens_long_ones() {
        let t = Desk::new();
        let long = "x".repeat(300);
        let suggest = t.tools().suggest(Some(vec!["".into(), "A".into(), "A".into(), long.clone(), "B".into()]));
        let apps = d(&suggest)["apps"].as_array().unwrap().clone();
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0], json!("A"));
        assert_eq!(apps[1].as_str().unwrap().chars().count(), 200);
        assert_eq!(d(&suggest)["arrangement"], json!("columns"));
    }

    #[test]
    fn suggest_needs_two_apps_and_readable_windows() {
        let t = Desk::new();
        let tools = t.tools();
        let one = tools.suggest(Some(vec!["Safari".into(), "Safari".into()]));
        assert!(one.ok);
        assert!(d(&one).get("plan_id").is_none());
        assert_eq!(d(&one)["apps"], json!(["Safari"]));
        let unreadable = tools.suggest(None);
        assert!(!unreadable.ok);
        assert_eq!(reason(&unreadable), "windows_unreadable");
        assert!(d(&unreadable)["guide_ko"].is_string());
        assert!(d(&unreadable)["hint"].is_string());
    }

    #[test]
    fn an_arrangement_plan_expires() {
        let t = Desk::new();
        let tools = t.tools();
        let suggest = tools.suggest(Some(vec!["A".into(), "B".into()]));
        t.advance(601.0);
        let late = tools.take_arrange_plan(d(&suggest)["plan_id"].as_str().unwrap()).unwrap_err();
        assert_eq!(reason(&late), "plan_stale");
        assert_eq!(reason(&tools.take_arrange_plan("zzzz").unwrap_err()), "plan_not_found");
    }

    #[test]
    fn only_eight_arrangement_plans_are_kept() {
        let t = Desk::new();
        let tools = t.tools();
        let mut ids = vec![];
        for _ in 0..9 {
            ids.push(d(&tools.suggest(Some(vec!["A".into(), "B".into()])))["plan_id"].as_str().unwrap().to_string());
            t.advance(1.0);
        }
        assert_eq!(reason(&tools.take_arrange_plan(&ids[0]).unwrap_err()), "plan_not_found", "the oldest proposal is dropped");
        for id in &ids[1..] { assert!(tools.take_arrange_plan(id).is_ok()); }
    }

    #[test]
    fn the_suggestion_never_carries_window_titles() {
        let t = Desk::new();
        let o = t.tools().suggest(Some(vec!["Safari".into(), "Notes".into()]));
        assert!(!serde_json::to_string(&o.result_json()).unwrap().contains("title"));
    }

    #[test]
    fn ids_are_short_hex_and_injectable() {
        let id = default_id();
        assert_eq!(id.len(), 8);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        let t = Desk::new();
        t.make("a.pdf");
        let n = Arc::new(AtomicUsize::new(0));
        let e = t.engine().with_ids(Box::new(move || format!("id{:06}", n.fetch_add(1, Ordering::Relaxed))));
        let p = plan(&e);
        assert_eq!(p.id, "id000000");
        assert_eq!(applied(&e, "id000000").tidy_id.as_deref(), Some("id000001"));
    }

    #[test]
    fn the_runner_prunes_the_journal_when_it_starts() {
        let t = Desk::new();
        t.make("a.pdf");
        let e = t.engine();
        applied(&e, &plan(&e).id);
        t.advance(31.0 * 86_400.0);
        let _tools = t.tools();
        let on_disk: Value = serde_json::from_slice(&fs::read(&t.journal).unwrap()).unwrap();
        assert_eq!(on_disk["entries"], json!([]));
    }
}

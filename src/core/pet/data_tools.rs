//! System Monitor and Disk Map tools: the current numbers, the largest items of the last scan, and starting a scan.
//! Port of the monitor and disk part of the macOS `DataGadgetRunner.swift`, with the same result fields.
//!
//! Windows differences:
//! - These answers carry what is on this PC (busiest program names, folder and file paths), so Windows declares all
//!   three tools with `"approval": "required"` unless the user set the gadget to "always allow" (macOS asks for none).
//! - macOS memory pressure and swap do not exist as such: `memory_pressure_percent` is the commit charge (committed
//!   memory of the commit limit) and `swap_used_bytes` is null. There is no load average on Windows (`load_average`
//!   is empty); the processor queue length and the GPU use are sent as extra fields.
//! - Disk Map on Windows reads one whole fixed drive, not a folder inside the home folder. `disk_scan` takes `~`
//!   (the drive of the user's profile), a drive (`D:`) or a folder path, and scans the drive that holds it.
//! - There is no tool that deletes or moves anything (the Recycle Bin button stays in the Disk Map window).
use super::commands::{PetCommandOutcome, PetCommandProblem, PetCommands};
use super::desktop::iso8601;
use super::json::JsonObject;
use serde_json::{json, Value};

pub const MAX_DISK_ITEMS: i64 = 50;
pub const DEFAULT_DISK_ITEMS: i64 = 10;

fn bad(why: impl Into<String>) -> PetCommandProblem { PetCommandProblem::BadParams(why.into()) }

fn present(v: Option<&Value>) -> bool { v.is_some_and(|v| !v.is_null()) }

fn object(params: Option<&Value>, allowed: &[&str]) -> Result<JsonObject, PetCommandProblem> {
    if present(params) && !params.is_some_and(Value::is_object) { return Err(bad("parameters must be an object")); }
    let p = params.and_then(Value::as_object).cloned().unwrap_or_default();
    let extra: Vec<String> = p.keys().filter(|k| !allowed.contains(&k.as_str())).map(|k| k.chars().take(40).collect()).collect();
    if !extra.is_empty() { return Err(bad(format!("unknown field {}", extra.join(", ")))); }
    Ok(p)
}

fn with(ok: bool, message: impl Into<String>, details: Value) -> PetCommandOutcome {
    PetCommandOutcome { ok, message: message.into(), gadgets: None, details: details.as_object().cloned() }
}

// MARK: System Monitor

/// Busiest program, already sampled by the monitor's panels.
#[derive(Clone, Debug, PartialEq)]
pub struct PetProcessUsage { pub name: String, pub cpu_percent: f64, pub memory_bytes: u64 }

/// The current numbers; None where Windows does not report them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetMonitorStats {
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub memory_pressure_percent: Option<f64>,
    pub swap_used_bytes: Option<u64>,
    pub network_down_bytes_per_second: Option<f64>,
    pub network_up_bytes_per_second: Option<f64>,
    /// The system drive.
    pub disk_used_percent: Option<f64>,
    pub battery_percent: Option<f64>,
    pub battery_charging: Option<bool>,
    pub on_power_adapter: Option<bool>,
    pub cpu_temperature_celsius: Option<f64>,
    pub gpu_temperature_celsius: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub processor_queue: Option<f64>,
    pub top_processes: Vec<PetProcessUsage>,
}

fn tenth(v: Option<f64>) -> Value { v.filter(|v| v.is_finite()).map_or(Value::Null, |v| json!((v * 10.0).round() / 10.0)) }

pub fn parse_monitor_stats(params: Option<&Value>) -> Result<(), PetCommandProblem> { object(params, &[]).map(|_| ()) }

/// `None`: the monitor has not sampled yet (just switched on).
pub fn monitor_outcome(stats: Option<&PetMonitorStats>) -> PetCommandOutcome {
    let Some(s) = stats else {
        return with(false, "System Monitor has no reading yet; try again in a few seconds.", json!({"reason": "no_reading"}));
    };
    let mut d = json!({
        "cpu_percent": tenth(s.cpu_percent), "memory_percent": tenth(s.memory_percent),
        "memory_used_bytes": s.memory_used_bytes, "memory_total_bytes": s.memory_total_bytes,
        "memory_pressure_percent": tenth(s.memory_pressure_percent), "swap_used_bytes": s.swap_used_bytes,
        "network_down_bytes_per_second": tenth(s.network_down_bytes_per_second), "network_up_bytes_per_second": tenth(s.network_up_bytes_per_second),
        "disk_used_percent": tenth(s.disk_used_percent), "battery_percent": tenth(s.battery_percent),
        "battery_charging": s.battery_charging, "on_power_adapter": s.on_power_adapter,
        "cpu_temperature_celsius": tenth(s.cpu_temperature_celsius), "gpu_temperature_celsius": tenth(s.gpu_temperature_celsius),
        "gpu_percent": tenth(s.gpu_percent), "processor_queue": tenth(s.processor_queue), "load_average": [],
    });
    if !s.top_processes.is_empty() {
        let mut top = s.top_processes.clone();
        top.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        d["top_apps"] = json!(top.iter().take(5).map(|p| json!({"name": p.name.chars().take(80).collect::<String>(),
            "cpu_percent": (p.cpu_percent * 10.0).round() / 10.0, "memory_bytes": p.memory_bytes})).collect::<Vec<_>>());
    }
    let mut parts = vec![];
    if let Some(c) = s.cpu_percent { parts.push(format!("CPU {}%", c.round() as i64)); }
    if let Some(m) = s.memory_percent { parts.push(format!("memory {}%", m.round() as i64)); }
    if let Some(t) = s.cpu_temperature_celsius { parts.push(format!("CPU {} °C", t.round() as i64)); }
    if let Some(b) = s.battery_percent { parts.push(format!("battery {}%{}", b.round() as i64, if s.battery_charging == Some(true) { " (charging)" } else { "" })); }
    let message = if parts.is_empty() { "System Monitor is running but has no numbers yet.".to_string() } else { format!("Right now: {}.", parts.join(", ")) };
    with(true, message, d)
}

// MARK: Disk Map

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetDiskTopKind { Both, Folders, Files }

impl PetDiskTopKind {
    pub const ALL: [&'static str; 3] = ["both", "folders", "files"];
}

pub fn parse_disk_top(params: Option<&Value>) -> Result<(i64, PetDiskTopKind), PetCommandProblem> {
    let p = object(params, &["count", "kind"])?;
    let mut count = DEFAULT_DISK_ITEMS;
    if present(p.get("count")) {
        count = PetCommands::integer(p.get("count")).filter(|c| (1..=MAX_DISK_ITEMS).contains(c))
            .ok_or_else(|| bad(format!("count must be a whole number from 1 to {MAX_DISK_ITEMS}")))?;
    }
    let kind = match p.get("kind").filter(|v| !v.is_null()).map(Value::as_str) {
        None | Some(Some("both")) => PetDiskTopKind::Both,
        Some(Some("folders")) => PetDiskTopKind::Folders,
        Some(Some("files")) => PetDiskTopKind::Files,
        _ => return Err(bad("kind must be one of both, folders, files")),
    };
    Ok((count, kind))
}

/// The raw `path` of `disk_scan` (checked by [`scan_drive`] in the shell, which knows the profile and the drives).
pub fn parse_disk_scan(params: Option<&Value>) -> Result<Option<String>, PetCommandProblem> {
    let p = object(params, &["path"])?;
    if !present(p.get("path")) { return Ok(None); }
    let Some(s) = p.get("path").and_then(Value::as_str) else { return Err(bad("path must be a string")) };
    if s.chars().count() > 1024 || s.contains('\0') { return Err(bad("path is too long")); }
    let t = s.trim();
    Ok((!t.is_empty()).then(|| t.to_string()))
}

/// The drive a scan reads (`C:\`): `~` or nothing is the drive of `profile`; `D:`, `D:\` or a folder on a drive
/// (it must exist: `is_dir`) is that drive. `fixed`: the fixed drives (`C:\`, `D:\` …), the only ones Disk Map reads.
pub fn scan_drive(raw: Option<&str>, profile: &str, fixed: &[String], is_dir: impl Fn(&str) -> bool) -> Result<String, PetCommandOutcome> {
    let mut p = raw.unwrap_or("~").trim().to_string();
    if p == "~" || p.is_empty() { p = profile.to_string(); } else if let Some(rest) = p.strip_prefix("~\\").or_else(|| p.strip_prefix("~/")) {
        p = format!("{}\\{}", profile.trim_end_matches('\\'), rest);
    }
    let p = p.replace('/', "\\");
    let b = p.as_bytes();
    let drive_letter = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\');
    if !drive_letter {
        return Err(with(false, "Disk Map reads whole drives: give ~ (the drive of the user's folder), a drive such as D: or a folder on a drive.",
                        json!({"reason": "not_a_drive"})));
    }
    let drive = format!("{}:\\", (b[0] as char).to_ascii_uppercase());
    if !fixed.iter().any(|f| f.eq_ignore_ascii_case(&drive)) {
        let list: Vec<String> = fixed.to_vec();
        return Err(with(false, format!("{drive} is not a fixed drive of this PC. Disk Map reads: {}.", list.join(", ")),
                        json!({"reason": "not_a_fixed_drive", "drives": list})));
    }
    if p.len() > 3 && !is_dir(&p) {
        return Err(with(false, "There is no folder at that path.", json!({"reason": "not_a_folder"})));
    }
    Ok(drive)
}

pub fn scan_started(drive: &str, already: bool) -> PetCommandOutcome {
    let lead = if already { format!("Disk Map is already scanning {drive}.") } else { format!("Disk Map started scanning {drive}.") };
    with(true, format!("{lead} A whole drive can take a few minutes; call disk_top_items afterwards. Nothing is deleted or moved."),
         json!({"path": drive, "scan": if already { "running" } else { "started" }}))
}

/// One folder or big file of the last scan. `depth`: 1 = directly inside the scanned drive.
#[derive(Clone, Debug, PartialEq)]
pub struct PetDiskEntry { pub name: String, pub path: String, pub folder: bool, pub bytes: u64, pub depth: u32, pub modified: Option<f64> }

pub enum PetDiskState {
    /// No scan in memory (never scanned, or forgotten ten minutes after the window closed).
    Empty,
    Scanning { drive: String },
    Ready { drive: String, bytes: u64, scanned_at: f64, entries: Vec<PetDiskEntry> },
}

/// The largest items (macOS `diskTop`): folders down to two levels below the drive, files anywhere.
pub fn disk_top(entries: &[PetDiskEntry], count: i64, kind: PetDiskTopKind) -> Vec<&PetDiskEntry> {
    let n = count.clamp(1, MAX_DISK_ITEMS) as usize;
    fn top(mut v: Vec<&PetDiskEntry>, n: usize) -> Vec<&PetDiskEntry> { v.sort_by_key(|e| std::cmp::Reverse(e.bytes)); v.truncate(n); v }
    let folders = top(entries.iter().filter(|e| e.folder && e.depth <= 2).collect(), n);
    let files = top(entries.iter().filter(|e| !e.folder).collect(), n);
    match kind {
        PetDiskTopKind::Folders => folders,
        PetDiskTopKind::Files => files,
        PetDiskTopKind::Both => top(folders.into_iter().chain(files).collect(), n),
    }
}

pub fn disk_top_outcome(state: &PetDiskState, count: i64, kind: PetDiskTopKind, now: f64) -> PetCommandOutcome {
    match state {
        PetDiskState::Empty => with(false, "Disk Map has no scan in memory. Start one with disk_scan (the user's drive by default), then ask again in a few minutes.",
                                    json!({"reason": "no_scan"})),
        PetDiskState::Scanning { drive } => with(false, format!("Disk Map is still scanning {drive}. Ask again in a minute."),
                                                 json!({"reason": "scanning", "path": drive})),
        PetDiskState::Ready { drive, bytes, scanned_at, entries } => {
            let shown = disk_top(entries, count, kind);
            let (drive, bytes, scanned_at) = (drive, *bytes, *scanned_at);
            let items: Vec<Value> = shown.iter().enumerate().map(|(n, e)| {
                let mut row = json!({"rank": n + 1, "name": e.name.chars().take(200).collect::<String>(), "path": e.path.chars().take(1024).collect::<String>(),
                                     "kind": if e.folder { "folder" } else { "file" }, "bytes": e.bytes});
                if let Some(m) = e.modified.filter(|m| *m > 0.0) { row["modified_at"] = json!(iso8601(m)); }
                row
            }).collect();
            let minutes = ((now - scanned_at) / 60.0).max(0.0) as i64;
            with(true, format!("Top {} by size from the scan of {drive}, {minutes} minute{} ago. Read only: nothing was deleted or moved.",
                               items.len(), if minutes == 1 { "" } else { "s" }),
                 json!({"items": items, "scanned_at": iso8601(scanned_at), "scanned_path": drive, "scanned_bytes": bytes}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_rounds_and_keeps_unknowns_null() {
        let s = PetMonitorStats { cpu_percent: Some(42.37), memory_percent: Some(61.04), memory_pressure_percent: None, battery_percent: Some(80.0),
            battery_charging: Some(true), top_processes: vec![PetProcessUsage { name: "a.exe".into(), cpu_percent: 3.0, memory_bytes: 10 },
                                                              PetProcessUsage { name: "b.exe".into(), cpu_percent: 9.04, memory_bytes: 20 }], ..Default::default() };
        let o = monitor_outcome(Some(&s));
        assert!(o.ok);
        let d = o.details.unwrap();
        assert_eq!(d["cpu_percent"], 42.4);
        assert!(d["memory_pressure_percent"].is_null());
        assert!(d["cpu_temperature_celsius"].is_null());
        assert_eq!(d["top_apps"][0]["name"], "b.exe", "busiest first");
        assert_eq!(d["load_average"], json!([]));
        assert!(o.message.contains("CPU 42%") && o.message.contains("(charging)"), "{}", o.message);
        let none = monitor_outcome(None);
        assert!(!none.ok);
        assert_eq!(none.details.unwrap()["reason"], "no_reading");
        assert!(parse_monitor_stats(Some(&json!({"all": true}))).is_err());
    }

    #[test]
    fn disk_top_reads_count_and_kind() {
        assert_eq!(parse_disk_top(None), Ok((10, PetDiskTopKind::Both)));
        assert_eq!(parse_disk_top(Some(&json!({"count": 5, "kind": "files"}))), Ok((5, PetDiskTopKind::Files)));
        for b in [json!({"count": 0}), json!({"count": 51}), json!({"count": 2.5}), json!({"kind": "all"}), json!({"path": "C:\\"})] {
            assert!(parse_disk_top(Some(&b)).is_err(), "{b}");
        }
    }

    fn e(name: &str, folder: bool, bytes: u64, depth: u32) -> PetDiskEntry {
        PetDiskEntry { name: name.into(), path: format!("C:\\{name}"), folder, bytes, depth, modified: Some(1_700_000_000.0) }
    }

    #[test]
    fn disk_top_lists_folders_two_levels_deep_and_files_anywhere() {
        let entries = vec![e("Users", true, 900, 1), e("deep", true, 800, 3), e("Windows", true, 500, 1), e("big.iso", false, 700, 4), e("small.bin", false, 10, 1)];
        let names = |v: Vec<&PetDiskEntry>| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(disk_top(&entries, 10, PetDiskTopKind::Folders)), ["Users", "Windows"], "depth 3 folder is inside a listed one");
        assert_eq!(names(disk_top(&entries, 10, PetDiskTopKind::Files)), ["big.iso", "small.bin"]);
        assert_eq!(names(disk_top(&entries, 2, PetDiskTopKind::Both)), ["Users", "big.iso"]);
        let o = disk_top_outcome(&PetDiskState::Ready { drive: "C:\\".into(), bytes: 2000, scanned_at: 1_700_000_000.0, entries: entries.clone() }, 3,
                                 PetDiskTopKind::Both, 1_700_000_000.0 + 125.0);
        let d = o.details.unwrap();
        assert_eq!(d["items"][0]["rank"], 1);
        assert_eq!(d["items"][1]["kind"], "file");
        assert_eq!(d["scanned_path"], "C:\\");
        assert_eq!(d["items"][0]["modified_at"], "2023-11-14T22:13:20Z");
        assert!(o.message.contains("2 minutes ago") && o.message.contains("nothing was deleted"));
        assert_eq!(disk_top_outcome(&PetDiskState::Empty, 10, PetDiskTopKind::Both, 0.0).details.unwrap()["reason"], "no_scan");
        assert_eq!(disk_top_outcome(&PetDiskState::Scanning { drive: "D:\\".into() }, 10, PetDiskTopKind::Both, 0.0).details.unwrap()["reason"], "scanning");
    }

    #[test]
    fn scan_picks_the_drive_of_the_path() {
        let fixed = vec!["C:\\".to_string(), "D:\\".to_string()];
        let dir = |p: &str| ["C:\\Users\\me", "C:\\Users\\me\\Downloads", "D:\\Games"].iter().any(|d| p.eq_ignore_ascii_case(d));
        let profile = "C:\\Users\\me";
        assert_eq!(scan_drive(None, profile, &fixed, dir), Ok("C:\\".into()));
        assert_eq!(scan_drive(Some("~"), profile, &fixed, dir), Ok("C:\\".into()));
        assert_eq!(scan_drive(Some("~/Downloads"), profile, &fixed, dir), Ok("C:\\".into()));
        assert_eq!(scan_drive(Some("d:"), profile, &fixed, dir), Ok("D:\\".into()));
        assert_eq!(scan_drive(Some("D:/Games"), profile, &fixed, dir), Ok("D:\\".into()));
        let r = |raw: &str| scan_drive(Some(raw), profile, &fixed, dir).unwrap_err().details.unwrap()["reason"].as_str().unwrap().to_string();
        assert_eq!(r("E:\\"), "not_a_fixed_drive");
        assert_eq!(r("\\\\server\\share"), "not_a_drive");
        assert_eq!(r("Downloads"), "not_a_drive");
        assert_eq!(r("D:\\Nope"), "not_a_folder");
        assert_eq!(parse_disk_scan(Some(&json!({"path": "  "}))), Ok(None));
        assert!(parse_disk_scan(Some(&json!({"path": 3}))).is_err());
        assert_eq!(scan_started("C:\\", false).details.unwrap()["scan"], "started");
    }
}

//! The settings tools of App Switch, Mouse Tune, Calendar Clock, Tray Organizer (the macOS Menu Bar Organizer's tools)
//! and the Focus Dim scope. Port of the macOS `SettingsGadgetsModel.swift` / `MenuBarModel.swift`: closed argument
//! lists and the result shapes. The shell reads and writes the app's own options; nothing here touches Windows.
//!
//! Windows differences (docs/parity-macos.md has the table):
//! - Mouse Tune: Windows has no smooth scrolling, side-button or middle-button options; it has the pointer speed and
//!   "enhance pointer precision" instead. `mouse_set` takes `reverse_scroll`, `speed` (the same choices as macOS),
//!   `pointer_speed` and `precision`; the three macOS-only fields are refused with a reason.
//! - Calendar Clock: no next-event line on Windows (the clock reads no calendar), so no `show_next_event`.
//! - Tray Organizer: Windows 11 keeps a "show on the taskbar" switch per tray icon. There is no always-hidden section,
//!   no divider and no icon position, so `menubar_show_hidden` is not declared and `menubar_list_items` gives the
//!   program names and whether each icon stays shown.
use super::commands::{PetCommandOutcome, PetCommandProblem, PetCommands};
use super::json::JsonObject;
use crate::core::clock::ClockHours;
use crate::core::mouse::{Precision, POINTER_CHOICES, SPEED_CHOICES};
use serde_json::{json, Value};

fn bad(why: impl Into<String>) -> PetCommandProblem { PetCommandProblem::BadParams(why.into()) }

fn present(v: Option<&Value>) -> bool { v.is_some_and(|v| !v.is_null()) }

/// Unknown keys refused; `extra_reason` explains keys that exist on macOS but not here.
fn closed(p: &JsonObject, allowed: &[&str], mac_only: &[(&str, &str)]) -> Result<(), PetCommandProblem> {
    for (k, why) in mac_only {
        if p.contains_key(*k) { return Err(bad(format!("{k} is not an option on Windows: {why}"))); }
    }
    let extra: Vec<String> = p.keys().filter(|k| !allowed.contains(&k.as_str())).map(|k| k.chars().take(40).collect()).collect();
    if extra.is_empty() { Ok(()) } else { Err(bad(format!("unknown field {}", extra.join(", ")))) }
}

fn object(params: Option<&Value>) -> Result<JsonObject, PetCommandProblem> {
    if present(params) && !params.is_some_and(Value::is_object) { return Err(bad("parameters must be an object")); }
    Ok(params.and_then(Value::as_object).cloned().unwrap_or_default())
}

fn boolean(p: &JsonObject, key: &str) -> Result<Option<bool>, PetCommandProblem> {
    if !present(p.get(key)) { return Ok(None); }
    PetCommands::bool(p.get(key)).map(Some).ok_or_else(|| bad(format!("{key} must be true or false")))
}

fn outcome(message: impl Into<String>, details: Value) -> PetCommandOutcome {
    PetCommandOutcome { ok: true, message: message.into(), gadgets: None, details: details.as_object().cloned() }
}

/// Reads no parameters at all (status tools).
pub fn none(params: Option<&Value>) -> Result<(), PetCommandProblem> { closed(&object(params)?, &[], &[]) }

// MARK: App Switch

/// `hide_on_repeat` (required).
pub fn parse_appswitch_set(params: Option<&Value>) -> Result<bool, PetCommandProblem> {
    let p = object(params)?;
    closed(&p, &["hide_on_repeat"], &[])?;
    boolean(&p, "hide_on_repeat")?.ok_or_else(|| bad("hide_on_repeat must be true or false"))
}

/// `shortcut_apps`: names of the programs that have a shortcut (never the shortcuts themselves).
pub fn appswitch_outcome(hide_on_repeat: bool, shortcut_apps: &[String], lead: &str) -> PetCommandOutcome {
    let apps: Vec<String> = shortcut_apps.iter().take(100).map(|n| n.chars().take(100).collect()).collect();
    outcome(format!("{lead} Hide on repeat is {}; {} app{} have a shortcut.", if hide_on_repeat { "on" } else { "off" }, apps.len(),
                    if apps.len() == 1 { "" } else { "s" }), json!({"hide_on_repeat": hide_on_repeat, "shortcut_apps": apps}))
}

// MARK: Mouse Tune

/// Raw values of "enhance pointer precision": leave it, force on, force off.
pub const PRECISION_CHOICES: [&str; 3] = ["keep", "on", "off"];

pub fn precision_raw(p: Precision) -> &'static str {
    match p { Precision::Keep => "keep", Precision::On => "on", Precision::Off => "off" }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetMouseChange {
    pub reverse_scroll: Option<bool>,
    /// One of `SPEED_CHOICES`.
    pub speed: Option<f64>,
    /// One of `POINTER_CHOICES` (0 = leave the Windows setting as it is).
    pub pointer_speed: Option<i32>,
    pub precision: Option<Precision>,
}

impl PetMouseChange {
    pub fn is_empty(&self) -> bool { self.reverse_scroll.is_none() && self.speed.is_none() && self.pointer_speed.is_none() && self.precision.is_none() }
}

/// The macOS fields Windows does not have.
const MOUSE_MAC_ONLY: [(&str, &str); 3] = [
    ("smooth_scroll", "Windows scrolls a wheel mouse in lines; Mouse Tune has no smooth scrolling here"),
    ("side_buttons", "Windows already uses the side buttons as back and forward"),
    ("middle_button", "Mission Control does not exist on Windows"),
];

pub fn parse_mouse_set(params: Option<&Value>) -> Result<PetMouseChange, PetCommandProblem> {
    let p = object(params)?;
    closed(&p, &["reverse_scroll", "speed", "pointer_speed", "precision"], &MOUSE_MAC_ONLY)?;
    let mut c = PetMouseChange { reverse_scroll: boolean(&p, "reverse_scroll")?, ..Default::default() };
    if present(p.get("speed")) {
        let v = p.get("speed").and_then(Value::as_f64).filter(|v| SPEED_CHOICES.contains(v));
        c.speed = Some(v.ok_or_else(|| bad("speed must be one of 0.5, 1, 1.5, 2, 3"))?);
    }
    if present(p.get("pointer_speed")) {
        let v = PetCommands::integer(p.get("pointer_speed")).filter(|v| POINTER_CHOICES.contains(&(*v as i32)));
        let list: Vec<String> = POINTER_CHOICES.iter().map(|v| v.to_string()).collect();
        c.pointer_speed = Some(v.ok_or_else(|| bad(format!("pointer_speed must be one of {} (0 leaves the Windows setting)", list.join(", "))))? as i32);
    }
    if present(p.get("precision")) {
        c.precision = Some(match p.get("precision").and_then(Value::as_str) {
            Some("keep") => Precision::Keep,
            Some("on") => Precision::On,
            Some("off") => Precision::Off,
            _ => return Err(bad("precision must be one of keep, on, off")),
        });
    }
    if c.is_empty() { return Err(bad("give at least one of pointer_speed, precision, reverse_scroll, speed")); }
    Ok(c)
}

#[derive(Clone, Debug, PartialEq)]
pub struct PetMouseState {
    pub reverse_scroll: bool,
    pub speed: f64,
    pub pointer_speed: i32,
    pub precision: Precision,
}

pub fn mouse_outcome(s: &PetMouseState, lead: &str) -> PetCommandOutcome {
    outcome(format!("{lead} Wheel {}, scroll speed x{}, pointer speed {}, pointer precision {}.",
                    if s.reverse_scroll { "reversed" } else { "normal" }, s.speed,
                    if s.pointer_speed == 0 { "unchanged".to_string() } else { s.pointer_speed.to_string() }, precision_raw(s.precision)),
            json!({"reverse_scroll": s.reverse_scroll, "speed": s.speed, "pointer_speed": s.pointer_speed, "precision": precision_raw(s.precision),
                   "speed_choices": SPEED_CHOICES, "pointer_speed_choices": POINTER_CHOICES, "precision_choices": PRECISION_CHOICES}))
}

// MARK: Calendar Clock

pub const HOURS_CHOICES: [&str; 3] = ["system", "twelve", "twentyFour"];
pub const MAX_WORLD_CLOCKS: usize = crate::core::clock::MAX_WORLD_CLOCKS;

pub fn hours_raw(h: ClockHours) -> &'static str {
    match h { ClockHours::Twelve => "twelve", ClockHours::TwentyFour => "twentyFour", _ => "system" }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PetClockChange {
    pub show_date: Option<bool>,
    pub show_weekday: Option<bool>,
    pub show_seconds: Option<bool>,
    pub hours: Option<ClockHours>,
    /// Replaces the list (empty = none).
    pub world_clocks: Option<Vec<String>>,
}

/// `known(id)`: the time zone ids the clock can show (`core::clock::zone_offset` knows them).
pub fn parse_calendar_set(params: Option<&Value>, known: impl Fn(&str) -> bool) -> Result<PetClockChange, PetCommandProblem> {
    let p = object(params)?;
    closed(&p, &["show_date", "show_weekday", "show_seconds", "hours", "world_clocks"],
           &[("show_next_event", "the Windows clock reads no calendar, so it has no next-event line")])?;
    let mut c = PetClockChange { show_date: boolean(&p, "show_date")?, show_weekday: boolean(&p, "show_weekday")?,
                                 show_seconds: boolean(&p, "show_seconds")?, ..Default::default() };
    if present(p.get("hours")) {
        c.hours = Some(match p.get("hours").and_then(Value::as_str) {
            Some("system") => ClockHours::System,
            Some("twelve") => ClockHours::Twelve,
            Some("twentyFour") => ClockHours::TwentyFour,
            _ => return Err(bad("hours must be one of system, twelve, twentyFour")),
        });
    }
    if let Some(v) = p.get("world_clocks").filter(|v| !v.is_null()) {
        let Some(items) = v.as_array() else { return Err(bad("world_clocks must be a list of time zone ids")) };
        if items.len() > MAX_WORLD_CLOCKS { return Err(bad(format!("world_clocks takes at most {MAX_WORLD_CLOCKS} zones"))); }
        let mut out: Vec<String> = vec![];
        for item in items {
            let id = item.as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 64 && known(s));
            let Some(id) = id else { return Err(bad("world_clocks needs valid time zone ids such as Asia/Seoul")) };
            if !out.iter().any(|o| o == id) { out.push(id.to_string()); }
        }
        c.world_clocks = Some(out);
    }
    if c == PetClockChange::default() { return Err(bad("give at least one of hours, show_date, show_seconds, show_weekday, world_clocks")); }
    Ok(c)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetClockState {
    pub show_date: bool,
    pub show_weekday: bool,
    pub show_seconds: bool,
    pub hours: ClockHours,
    pub world_clocks: Vec<String>,
}

pub fn calendar_outcome(s: &PetClockState, lead: &str) -> PetCommandOutcome {
    let zones = if s.world_clocks.is_empty() { "no world clocks".to_string() } else { format!("world clocks {}", s.world_clocks.join(", ")) };
    outcome(format!("{lead} Date {}, weekday {}, seconds {}, hours {}, {zones}.", on(s.show_date), on(s.show_weekday), on(s.show_seconds), hours_raw(s.hours)),
            json!({"show_date": s.show_date, "show_weekday": s.show_weekday, "show_seconds": s.show_seconds, "hours": hours_raw(s.hours),
                   "world_clocks": s.world_clocks, "hours_choices": HOURS_CHOICES}))
}

fn on(b: bool) -> &'static str { if b { "on" } else { "off" } }

// MARK: Tray Organizer (macOS Menu Bar Organizer tools)

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PetTrayState {
    pub collapsed: bool,
    pub auto_collapse_seconds: i32,
    /// Tray icons Windows knows for this user.
    pub icons: usize,
    /// Icons the user keeps shown while collapsed.
    pub kept: usize,
}

/// One tray icon: the program's name only (never its tooltip, which can carry content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetTrayItem {
    pub app: String,
    pub kept: bool,
}

pub fn parse_tray_set_collapsed(params: Option<&Value>) -> Result<bool, PetCommandProblem> {
    let p = object(params)?;
    closed(&p, &["collapsed"], &[])?;
    boolean(&p, "collapsed")?.ok_or_else(|| bad("collapsed must be true or false"))
}

fn tray_details(s: &PetTrayState) -> Value {
    json!({"collapsed": s.collapsed, "auto_collapse_seconds": s.auto_collapse_seconds, "icons": s.icons, "kept_shown": s.kept,
           "always_hidden_section": false})
}

pub fn tray_status(s: &PetTrayState) -> PetCommandOutcome {
    let auto = if s.auto_collapse_seconds > 0 { format!(", folds again {} seconds after unfolding", s.auto_collapse_seconds) } else { String::new() };
    outcome(format!("The tray icons are {} ({} of {} kept shown{auto}).", if s.collapsed { "folded away" } else { "all shown" }, s.kept, s.icons),
            tray_details(s))
}

/// `changed`: whether the call folded or unfolded anything.
pub fn tray_changed(s: &PetTrayState, changed: bool) -> PetCommandOutcome {
    let mut o = tray_status(s);
    o.message = format!("{} {}", if changed { if s.collapsed { "Folded the tray icons away." } else { "Showing every tray icon." } } else { "Nothing changed." }, o.message);
    if let Some(d) = o.details.as_mut() { d.insert("changed".into(), json!(changed)); }
    o
}

/// Program names and whether each stays shown (`visible`) or folds away (`hidden`) while collapsed.
pub fn tray_items(items: &[PetTrayItem], collapsed: bool) -> PetCommandOutcome {
    let rows: Vec<Value> = items.iter().take(100).map(|i| {
        let zone = if i.kept || !collapsed { "visible" } else { "hidden" };
        json!({"app": i.app.chars().take(100).collect::<String>(), "kept": i.kept, "zone": zone})
    }).collect();
    let hidden = rows.iter().filter(|r| r["zone"] == "hidden").count();
    outcome(format!("{} tray icon{}, {hidden} folded away now. Names only.", rows.len(), if rows.len() == 1 { "" } else { "s" }),
            json!({"items": rows, "count": items.len(), "zones": ["visible", "hidden"]}))
}

// MARK: Focus Dim scope (read only, as on macOS)

pub fn dim_scope_raw(front_app: bool) -> &'static str { if front_app { "frontApp" } else { "frontWindow" } }

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: Value) -> Option<Value> { Some(v) }

    #[test]
    fn appswitch_takes_hide_on_repeat_only() {
        assert_eq!(parse_appswitch_set(p(json!({"hide_on_repeat": false})).as_ref()), Ok(false));
        assert!(parse_appswitch_set(p(json!({})).as_ref()).is_err());
        assert!(parse_appswitch_set(p(json!({"hide_on_repeat": 1})).as_ref()).is_err());
        assert!(parse_appswitch_set(p(json!({"hide_on_repeat": true, "shortcut": "ctrl+1"})).as_ref()).is_err(), "shortcuts are not editable by tools");
        let o = appswitch_outcome(true, &["메모장".into(), "Chrome".into()], "App Switch settings.");
        let d = o.details.unwrap();
        assert_eq!(d["hide_on_repeat"], true);
        assert_eq!(d["shortcut_apps"], json!(["메모장", "Chrome"]));
    }

    #[test]
    fn mouse_set_reads_windows_options_and_explains_the_macos_ones() {
        let c = parse_mouse_set(p(json!({"reverse_scroll": false, "speed": 1.5, "pointer_speed": 10, "precision": "off"})).as_ref()).unwrap();
        assert_eq!(c, PetMouseChange { reverse_scroll: Some(false), speed: Some(1.5), pointer_speed: Some(10), precision: Some(Precision::Off) });
        assert_eq!(parse_mouse_set(p(json!({"speed": 2})).as_ref()).unwrap().speed, Some(2.0), "an integer speed is a number");
        for bad_args in [json!({}), json!({"speed": 1.2}), json!({"pointer_speed": 5}), json!({"pointer_speed": 4.5}), json!({"precision": "max"}),
                         json!({"reverse_scroll": "yes"}), json!({"dpi": 800})] {
            assert!(parse_mouse_set(Some(&bad_args)).is_err(), "{bad_args}");
        }
        for (key, value) in [("smooth_scroll", json!(true)), ("side_buttons", json!(true)), ("middle_button", json!("missionControl"))] {
            let Err(PetCommandProblem::BadParams(why)) = parse_mouse_set(Some(&json!({key: value}))) else { panic!("{key} accepted") };
            assert!(why.contains("not an option on Windows"), "{why}");
        }
        let o = mouse_outcome(&PetMouseState { reverse_scroll: true, speed: 1.0, pointer_speed: 0, precision: Precision::Keep }, "Mouse Tune settings.");
        let d = o.details.unwrap();
        assert_eq!(d["precision"], "keep");
        assert_eq!(d["speed_choices"], json!([0.5, 1.0, 1.5, 2.0, 3.0]));
        assert!(o.message.contains("pointer speed unchanged"));
    }

    #[test]
    fn calendar_set_checks_zones_and_has_no_next_event() {
        let known = |id: &str| ["Asia/Seoul", "America/New_York", "Europe/London"].contains(&id);
        let c = parse_calendar_set(p(json!({"hours": "twentyFour", "world_clocks": ["Asia/Seoul", "Asia/Seoul", "Europe/London"]})).as_ref(), known).unwrap();
        assert_eq!(c.hours, Some(ClockHours::TwentyFour));
        assert_eq!(c.world_clocks, Some(vec!["Asia/Seoul".to_string(), "Europe/London".to_string()]), "duplicates dropped");
        assert_eq!(parse_calendar_set(p(json!({"world_clocks": []})).as_ref(), known).unwrap().world_clocks, Some(vec![]), "empty clears");
        assert!(parse_calendar_set(p(json!({"world_clocks": ["Mars/Base"]})).as_ref(), known).is_err());
        assert!(parse_calendar_set(p(json!({"world_clocks": vec!["Asia/Seoul"; 7]})).as_ref(), known).is_err());
        assert!(parse_calendar_set(p(json!({"hours": "24"})).as_ref(), known).is_err());
        assert!(parse_calendar_set(p(json!({})).as_ref(), known).is_err());
        let Err(PetCommandProblem::BadParams(why)) = parse_calendar_set(p(json!({"show_next_event": true})).as_ref(), known) else { panic!() };
        assert!(why.contains("not an option on Windows"));
        let s = PetClockState { show_date: true, show_weekday: false, show_seconds: true, hours: ClockHours::System, world_clocks: vec!["Asia/Seoul".into()] };
        let d = calendar_outcome(&s, "Calendar Clock settings.").details.unwrap();
        assert_eq!(d["hours"], "system");
        assert_eq!(d["hours_choices"], json!(HOURS_CHOICES));
        assert!(d.get("show_next_event").is_none());
    }

    #[test]
    fn tray_answers_carry_names_and_zones_only() {
        let s = PetTrayState { collapsed: true, auto_collapse_seconds: 10, icons: 3, kept: 1 };
        let d = tray_status(&s).details.unwrap();
        assert_eq!(d["collapsed"], true);
        assert_eq!(d["always_hidden_section"], false);
        let items = [PetTrayItem { app: "OneDrive".into(), kept: true }, PetTrayItem { app: "Steam".into(), kept: false }];
        let o = tray_items(&items, true);
        let rows = o.details.as_ref().unwrap()["items"].as_array().unwrap().clone();
        assert_eq!(rows[0]["zone"], "visible");
        assert_eq!(rows[1]["zone"], "hidden");
        assert_eq!(tray_items(&items, false).details.unwrap()["items"][1]["zone"], "visible", "nothing is hidden while expanded");
        assert_eq!(tray_changed(&s, false).details.unwrap()["changed"], false);
        assert_eq!(parse_tray_set_collapsed(p(json!({"collapsed": false})).as_ref()), Ok(false));
        assert!(parse_tray_set_collapsed(p(json!({})).as_ref()).is_err());
        assert!(none(p(json!({"x": 1})).as_ref()).is_err());
        assert!(none(None).is_ok());
    }
}

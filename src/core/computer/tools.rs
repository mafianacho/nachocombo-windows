//! The Computer Use tool groups and tools (macOS `ToolGroup`, `Gadgets/Tools/ComputerTools.swift`). Names, groups,
//! risks and input schemas are the macOS ones (fixed against `fixtures/mac-computer-tools.json`); the descriptions say
//! what Windows does. On MCP a tool is `computer_<name>` (the old Computer Use name `<name>` is accepted too); on
//! Nacho's path it is `combo_computer_<name>`.
//!
//! Not on Windows:
//! - **AppleScript** (`applescript_run`): Windows has no AppleScript. The group is listed as unavailable, can never be
//!   switched on, and the tool answers `not available on Windows`. PowerShell is the shell group.
//! - **The accessibility tree tools** (`ui_tree`, `find_element`, `click_element`): later, on UI Automation
//!   ([`ComputerTools::LATER`]).
use crate::core::pet::gadget_tools::PetToolRisk;
use serde_json::{json, Map, Value};

/// Every tool belongs to one group; every group has its own switch, off by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolGroup { Screen, AppsWindows, Input, Clipboard, OpenItems, SystemInfo, AppleScript, Shell, FileAccess, ProcessControl }

impl ToolGroup {
    pub const ALL: [ToolGroup; 10] = [Self::Screen, Self::AppsWindows, Self::Input, Self::Clipboard, Self::OpenItems, Self::SystemInfo,
        Self::AppleScript, Self::Shell, Self::FileAccess, Self::ProcessControl];

    /// The key in `settings.json` (`computerUse.enabledGroups`), as on macOS.
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Screen => "screen", Self::AppsWindows => "appsWindows", Self::Input => "input", Self::Clipboard => "clipboard",
            Self::OpenItems => "openItems", Self::SystemInfo => "systemInfo", Self::AppleScript => "appleScript", Self::Shell => "shell",
            Self::FileAccess => "fileAccess", Self::ProcessControl => "processControl",
        }
    }

    pub fn from_raw(s: &str) -> Option<Self> { Self::ALL.into_iter().find(|g| g.raw_value() == s) }

    /// Separate switches that ask before they go on.
    pub fn is_high_risk(self) -> bool { matches!(self, Self::AppleScript | Self::Shell | Self::FileAccess | Self::ProcessControl) }

    /// AppleScript does not exist on Windows.
    pub fn is_available(self) -> bool { self != Self::AppleScript }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Screen => "Screen", Self::AppsWindows => "Apps & Windows", Self::Input => "Input", Self::Clipboard => "Clipboard",
            Self::OpenItems => "Open", Self::SystemInfo => "System Info", Self::AppleScript => "Run AppleScript",
            Self::Shell => "Run PowerShell Commands", Self::FileAccess => "Read & Write Files", Self::ProcessControl => "End Processes",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Screen => "Screenshots (display, window, region) and the display list",
            Self::AppsWindows => "List, open, switch to and close apps; list, move, resize and minimize windows",
            Self::Input => "Mouse move, click, drag and scroll; key presses; typing text",
            Self::Clipboard => "Read and write clipboard text",
            Self::OpenItems => "Open web addresses (http, https, mailto) and files",
            Self::SystemInfo => "Windows version, PC model, uptime, memory, battery, displays and the process list",
            Self::AppleScript => "Not available on Windows",
            Self::Shell => "Run PowerShell commands (time and output limits)",
            Self::FileAccess => "Read text files and write new ones or append (keys, credentials and this app's folder excluded; never overwrites)",
            Self::ProcessControl => "Close or end running processes",
        }
    }

    /// The consequence line next to a high-risk switch (and in the confirmation).
    pub fn risk_note(self) -> Option<&'static str> {
        Some(match self {
            Self::Shell => "Whoever holds a key can run commands on this PC with your user rights (this can get around the file limits).",
            Self::FileAccess => "Whoever holds a key can read any file you can open and add files (keys, credentials and the startup folder excluded).",
            Self::ProcessControl => "Whoever holds a key can end running programs, also without saving.",
            _ => return None,
        })
    }
}

/// One Computer Use tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComputerTool {
    /// The macOS Computer Use name (`screen_capture`): what the backend runs; MCP accepts it as an alias.
    pub name: &'static str,
    pub group: ToolGroup,
    pub risk: PetToolRisk,
    /// Also declared to Nacho's connector (`combo_computer_<name>`).
    pub nacho: bool,
    pub english: &'static str,
}

impl ComputerTool {
    pub fn mcp_name(&self) -> String { format!("computer_{}", self.name) }
    pub fn connector_name(&self) -> String { format!("combo_computer_{}", self.name) }
    pub fn is_available(&self) -> bool { self.group.is_available() }
    pub fn high_risk(&self) -> bool { self.group.is_high_risk() }
    pub fn input_schema(&self) -> Value { ComputerTools::schema(self.name) }
    /// `tools/list` entry.
    pub fn json(&self) -> Value { json!({"name": self.mcp_name(), "description": self.english, "inputSchema": self.input_schema()}) }
}

const fn tool(name: &'static str, group: ToolGroup, risk: PetToolRisk, nacho: bool, english: &'static str) -> ComputerTool {
    ComputerTool { name, group, risk, nacho, english }
}

pub struct ComputerTools;

use PetToolRisk::*;
use ToolGroup::*;

impl ComputerTools {
    pub const DEFAULT_MAX_WIDTH: i64 = 1568;

    /// In the macOS registry order.
    pub const ALL: [ComputerTool; 26] = [
        tool("screen_list_displays", Screen, Read, true, "Returns the connected displays (id, position and size in pixels, scale, whether it is the primary display)."),
        tool("screen_capture", Screen, ContentOut, false, "Returns a screenshot as PNG. target: display (whole display, displayId optional), window (needs windowId from window_list), region (needs x, y, width, height in screen pixels). Secure screens (sign-in, UAC prompts) cannot be captured."),
        tool("app_list", AppsWindows, Read, false, "Running apps that have a window (name, exe name as bundleId, pid, whether frontmost)."),
        tool("app_open", AppsWindows, Change, false, "Starts an app. Give its Start menu name as name, or its exe name (for example notepad.exe) as bundleId."),
        tool("app_activate", AppsWindows, Change, false, "Brings an app to the front (restores it when minimized). Give either pid or bundleId (the exe name)."),
        tool("app_quit", AppsWindows, Destructive, false, "Asks an app to close its windows (the app asks about unsaved work; never forced). Give pid or bundleId (the exe name). File Explorer and Nacho Combo are never closed."),
        tool("window_list", AppsWindows, Read, false, "Visible windows (windowId, pid, app name, title, position and size in screen pixels, index within the app)."),
        tool("window_set_frame", AppsWindows, Change, false, "Moves and resizes a window. Only the given values among x, y, width, height (screen pixels) change."),
        tool("window_minimize", AppsWindows, Change, false, "Minimizes a window or restores it."),
        tool("mouse_move", Input, InputInject, true, "Moves the mouse pointer to screen coordinates (pixels; the primary display's top-left is 0,0). Windows of programs running as administrator and secure screens do not take input."),
        tool("mouse_click", Input, InputInject, true, "Clicks at the given screen coordinates (left, right or middle button, 1 to 3 clicks). Nacho Combo's own windows are refused."),
        tool("mouse_drag", Input, InputInject, true, "Drags with the left button from one point to another (screen pixels)."),
        tool("mouse_scroll", Input, InputInject, true, "Scrolls by about deltaX/deltaY pixels (Windows scrolls in wheel notches of about 40 pixels; positive deltaY = up). If x and y are given, the pointer moves there first."),
        tool("key_press", Input, InputInject, true, "One key press plus modifier keys, e.g. key=\"c\", modifiers=[\"ctrl\"] copies on Windows. Keys: a-z, 0-9, return, tab, space, delete (backspace), forwarddelete, escape, arrows (left/right/up/down), home, end, pageup, pagedown, insert, f1-f12. Modifiers: ctrl, shift, alt (option), win (cmd). fn does not exist."),
        tool("type_text", Input, InputInject, true, "Types text into the focused field (up to 4000 characters). Click the field first."),
        tool("clipboard_read", Clipboard, ContentOut, false, "Reads the text on the clipboard."),
        tool("clipboard_write", Clipboard, Change, false, "Writes text to the clipboard."),
        tool("open_url", OpenItems, Change, false, "Opens a web address in the default app (http, https and mailto only)."),
        tool("open_file", OpenItems, Change, false, "Opens a file in its default app (absolute path such as C:\\Users\\me\\notes.txt; programs, scripts, installers and shortcuts are refused)."),
        tool("system_info", SystemInfo, Read, false, "Summary of the Windows version, PC model, uptime, memory, CPU cores, battery, volume and displays."),
        tool("process_list", SystemInfo, Read, false, "Running processes (pid, exe name). Command lines are not included."),
        tool("applescript_run", AppleScript, Destructive, false, "Not available on Windows (there is no AppleScript). Use the shell group's PowerShell instead."),
        tool("shell_run", Shell, Destructive, false, "[High risk] Runs a PowerShell command (powershell.exe -NoProfile, no window) and returns its output. Timeout 1-120 s (30 by default; the whole process tree ends then), output up to 64 KB."),
        tool("file_read", FileAccess, ContentOut, false, "[High risk] Reads a text file (absolute path, up to 262144 bytes). Keys, credentials, browser profiles and Nacho Combo's own folder are refused."),
        tool("file_write", FileAccess, Destructive, false, "[High risk] Writes a new text file, or appends to one with append=true (up to 1 MB). Never overwrites an existing file. Only under the user's folder and the temp folder; never the startup folder or PowerShell profiles."),
        tool("process_kill", ProcessControl, Destructive, false, "[High risk] Ends a process. TERM (default), INT and HUP ask its windows to close; KILL ends it at once (unsaved work is lost). Never system processes or Nacho Combo."),
    ];

    /// macOS tools Windows does not have yet, with the reason.
    pub const LATER: [(&'static str, &'static str); 3] = [
        ("ui_tree", "needs a UI Automation reader on Windows (macOS reads the accessibility tree)"),
        ("find_element", "needs a UI Automation reader on Windows (macOS reads the accessibility tree)"),
        ("click_element", "needs element ids from ui_tree / find_element"),
    ];

    /// macOS Nacho tools that wait for the chat handoff (a picture on the next message, like `combo_capture_ask`).
    pub const NACHO_WITH_CHAT_HANDOFF: [&'static str; 1] = ["screen_capture"];

    pub fn find(name: &str) -> Option<&'static ComputerTool> {
        let legacy = name.strip_prefix("computer_").unwrap_or(name);
        Self::ALL.iter().find(|t| t.name == legacy)
    }

    /// By the connector name (`combo_computer_mouse_click`).
    pub fn find_connector(name: &str) -> Option<&'static ComputerTool> { Self::ALL.iter().find(|t| t.connector_name() == name) }

    pub fn in_group(g: ToolGroup) -> Vec<&'static ComputerTool> { Self::ALL.iter().filter(|t| t.group == g).collect() }

    /// The tools Nacho's connector gets on Windows (no chat handoff yet: no screenshot).
    pub fn nacho() -> Vec<&'static ComputerTool> { Self::ALL.iter().filter(|t| t.nacho).collect() }

    // MARK: schemas (macOS shapes)

    fn obj(properties: Value, required: &[&str]) -> Value {
        let mut o = json!({"type": "object", "properties": properties, "additionalProperties": false});
        if !required.is_empty() { o["required"] = json!(required); }
        o
    }
    fn s(d: &str) -> Value { json!({"type": "string", "description": d}) }
    fn i(d: &str) -> Value { json!({"type": "integer", "description": d}) }
    fn n(d: &str) -> Value { json!({"type": "number", "description": d}) }
    fn b(d: &str) -> Value { json!({"type": "boolean", "description": d}) }
    fn choice(values: &[&str], d: &str) -> Value { json!({"type": "string", "enum": values, "description": d}) }

    pub fn schema(name: &str) -> Value {
        let window = |extra: Value, required: &[&str]| {
            let mut p = json!({"pid": Self::i("Process ID of the app that owns the window (from app_list/window_list)"),
                               "windowIndex": Self::i("Window number within that app (starts at 0, the index from window_list)")});
            for (k, v) in extra.as_object().unwrap() { p[k] = v.clone(); }
            Self::obj(p, required)
        };
        match name {
            "screen_capture" => Self::obj(json!({
                "target": Self::choice(&["display", "window", "region"], "What to capture"),
                "displayId": Self::i("Display id from screen_list_displays (the primary display if omitted)"),
                "windowId": Self::i("Window id (windowId from window_list)"),
                "x": Self::n("Region left (screen pixels)"), "y": Self::n("Region top (screen pixels)"),
                "width": Self::n("Region width"), "height": Self::n("Region height"),
                "maxWidth": Self::i("Wider images are scaled down keeping the aspect ratio (default 1568, 64-4096)"),
            }), &["target"]),
            "app_open" => Self::obj(json!({"bundleId": Self::s("Exe name on Windows, e.g. notepad.exe"), "name": Self::s("App name as in the Start menu")}), &[]),
            "app_activate" | "app_quit" => Self::obj(json!({"pid": Self::i("Process ID"), "bundleId": Self::s("Exe name on Windows, e.g. notepad.exe")}), &[]),
            "window_set_frame" => window(json!({"x": Self::n("New x"), "y": Self::n("New y"), "width": Self::n("New width"), "height": Self::n("New height")}), &["pid", "windowIndex"]),
            "window_minimize" => window(json!({"minimized": Self::b("true = minimize, false = restore (default true)")}), &["pid", "windowIndex"]),
            "mouse_move" => Self::obj(json!({"x": Self::n("x"), "y": Self::n("y")}), &["x", "y"]),
            "mouse_click" => Self::obj(json!({"x": Self::n("x"), "y": Self::n("y"), "button": Self::choice(&["left", "right", "middle"], "Default left"),
                                              "clickCount": Self::i("1 to 3, default 1")}), &["x", "y"]),
            "mouse_drag" => Self::obj(json!({"fromX": Self::n("Start x"), "fromY": Self::n("Start y"), "toX": Self::n("End x"), "toY": Self::n("End y")}), &["fromX", "fromY", "toX", "toY"]),
            "mouse_scroll" => Self::obj(json!({"deltaX": Self::i("Horizontal (positive = right)"), "deltaY": Self::i("Vertical (positive = up)"), "x": Self::n("x"), "y": Self::n("y")}), &[]),
            "key_press" => Self::obj(json!({"key": Self::s("Key name"),
                "modifiers": {"type": "array", "items": {"type": "string"}, "description": "ctrl, shift, alt (option), win (cmd)"}}), &["key"]),
            "type_text" => Self::obj(json!({"text": Self::s("Text to type")}), &["text"]),
            "clipboard_write" => Self::obj(json!({"text": Self::s("Text to copy")}), &["text"]),
            "open_url" => Self::obj(json!({"url": Self::s("URL")}), &["url"]),
            "open_file" | "file_read" => Self::obj(json!({"path": Self::s("Absolute path")}), &["path"]),
            "applescript_run" => Self::obj(json!({"script": Self::s("AppleScript source (not available on Windows)")}), &["script"]),
            "shell_run" => Self::obj(json!({"command": Self::s("PowerShell command"), "cwd": Self::s("Working folder (absolute path, optional)"),
                                           "timeoutSeconds": Self::i("1 to 120, default 30")}), &["command"]),
            "file_write" => Self::obj(json!({"path": Self::s("Absolute path"), "content": Self::s("Content"), "append": Self::b("Append to an existing file")}), &["path", "content"]),
            "process_kill" => Self::obj(json!({"pid": Self::i("Process ID"), "signal": Self::choice(&["TERM", "KILL", "INT", "HUP"], "Default TERM")}), &["pid"]),
            _ => Self::obj(json!({}), &[]),
        }
    }

    // MARK: closed argument check (the same strictness on both exits)

    /// Unknown keys, wrong types, values outside an enum or bounds and missing required keys are refused before
    /// anything runs. Integers are whole JSON numbers (5 or 5.0, never "5" or true).
    pub fn check_arguments(name: &str, arguments: Option<&Value>) -> Result<Map<String, Value>, String> {
        let schema = Self::schema(name);
        let args = match arguments {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(o)) => o.clone(),
            Some(_) => return Err("arguments must be an object".into()),
        };
        let props = schema["properties"].as_object().cloned().unwrap_or_default();
        let mut unknown: Vec<&str> = args.keys().map(String::as_str).filter(|k| !props.contains_key(*k)).collect();
        unknown.sort_unstable();
        if !unknown.is_empty() { return Err(format!("unknown argument{} {}", if unknown.len() == 1 { "" } else { "s" }, unknown.join(", "))); }
        for r in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if args.get(r).is_none_or(Value::is_null) { return Err(format!("Missing required argument '{r}'")); }
        }
        for (k, v) in &args {
            if v.is_null() { continue; }
            let p = &props[k];
            let ok = match p["type"].as_str() {
                Some("string") => v.is_string(),
                Some("boolean") => v.is_boolean(),
                Some("number") => v.as_f64().is_some_and(f64::is_finite),
                Some("integer") => v.as_f64().is_some_and(|d| d.is_finite() && d.fract() == 0.0 && d.abs() < 1e15),
                Some("array") => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
                _ => true,
            };
            if !ok {
                let what = match p["type"].as_str() { Some("integer") => "an integer", Some("number") => "a finite number", Some("boolean") => "true or false", Some("array") => "an array of strings", _ => "a string" };
                return Err(format!("'{k}' must be {what}"));
            }
            if let Some(e) = p["enum"].as_array() {
                if !e.contains(v) { return Err(format!("'{k}' must be one of {}", e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))); }
            }
            if let (Some(min), Some(d)) = (p["minimum"].as_f64(), v.as_f64()) { if d < min { return Err(format!("'{k}' must be at least {min}")); } }
            if let (Some(max), Some(d)) = (p["maximum"].as_f64(), v.as_f64()) { if d > max { return Err(format!("'{k}' must be at most {max}")); } }
        }
        Ok(args)
    }

    /// Korean for Nacho's connector (the tools Nacho gets).
    pub fn korean(name: &str) -> Option<&'static str> {
        // i18n-exempt:begin (sent to the server as tool text, not drawn)
        Some(match name {
            "screen_list_displays" => "연결된 디스플레이 목록(아이디·위치와 크기(픽셀)·배율·주 디스플레이인지)을 돌려줘요. 클릭할 좌표를 정하기 전에 화면 배치를 볼 때 써요.",
            "screen_capture" => "화면을 PNG로 찍어 다음 메시지에 첨부해요. 디스플레이 전체, 창 하나, 지정한 영역 중에서 골라요.",
            "mouse_move" => "마우스 포인터를 화면 좌표(픽셀, 주 디스플레이 왼쪽 위가 0,0)로 옮겨요. 클릭하지는 않아요. 사용자가 매번 허용해요.",
            "mouse_click" => "지정한 화면 좌표를 클릭해요(왼쪽·오른쪽·가운데 버튼, 1~3번). 나초콤보 창은 누르지 않아요. 사용자가 매번 허용해요.",
            "mouse_drag" => "마우스 왼쪽 버튼으로 화면의 한 점에서 다른 점까지 끌어다 놓아요(픽셀 좌표). 사용자가 매번 허용해요.",
            "mouse_scroll" => "스크롤해요(휠 한 칸이 약 40픽셀, deltaY 양수가 위). 좌표를 주면 먼저 그 자리로 포인터를 옮겨요. 사용자가 매번 허용해요.",
            "key_press" => "키 하나를 수정 키와 함께 눌러요. 예: 키 c와 ctrl은 복사. 수정 키는 ctrl·shift·alt·win이에요. 사용자가 매번 허용해요.",
            "type_text" => "지금 초점이 있는 입력 칸에 글을 입력해요(최대 4000자). 먼저 그 칸을 클릭해 초점을 옮겨요. 사용자가 매번 허용해요.",
            _ => return None,
        })
        // i18n-exempt:end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn mac() -> Value { serde_json::from_str(include_str!("fixtures/mac-computer-tools.json")).unwrap() }

    /// Types, enums, bounds, `required` and `additionalProperties` stay; wording is per platform.
    fn shape(v: &Value) -> Value {
        match v {
            Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| k.as_str() != "description").map(|(k, v)| (k.clone(), shape(v))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(shape).collect()),
            v => v.clone(),
        }
    }

    #[test]
    fn the_catalog_matches_the_macos_registry() {
        let mac = mac();
        let theirs: Vec<&Value> = mac["tools"].as_array().unwrap().iter().collect();
        let by: BTreeMap<&str, &Value> = theirs.iter().map(|t| (t["action"].as_str().unwrap(), *t)).collect();
        for t in &ComputerTools::ALL {
            let m = by.get(t.name).unwrap_or_else(|| panic!("{} is not a macOS Computer Use tool", t.name));
            assert_eq!(m["mcp_name"], json!(t.mcp_name()), "{}", t.name);
            assert_eq!(m["legacy_mcp_name"], json!(t.name), "{}", t.name);
            assert_eq!(m["connector_name"], json!(t.connector_name()), "{}", t.name);
            assert_eq!(m["group"], json!(t.group.raw_value()), "{}", t.name);
            assert_eq!(m["high_risk_group"], json!(t.high_risk()), "{}", t.name);
            assert_eq!(m["risk"], json!(t.risk.raw_value()), "{}", t.name);
            assert_eq!(shape(&m["inputSchema"]), shape(&t.input_schema()), "{}: input schema differs from macOS", t.name);
            // Nacho's exit: a subset of the macOS one (the screenshot waits for the chat handoff).
            let mac_nacho = m["exits"].as_array().unwrap().contains(&json!("connector"));
            assert!(!t.nacho || mac_nacho, "{} is not a Nacho tool on macOS", t.name);
            if mac_nacho && !t.nacho { assert!(ComputerTools::NACHO_WITH_CHAT_HANDOFF.contains(&t.name), "{}: why is it not Nacho's?", t.name); }
        }
        // Order: the macOS registry order.
        let ours: Vec<&str> = ComputerTools::ALL.iter().map(|t| t.name).collect();
        let positions: Vec<usize> = ours.iter().map(|n| theirs.iter().position(|t| t["action"] == json!(n)).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
        // Every macOS tool is here or later with a reason.
        for m in &theirs {
            let n = m["action"].as_str().unwrap();
            assert!(ComputerTools::find(n).is_some() || ComputerTools::LATER.iter().any(|(l, _)| *l == n), "{n}: neither ported nor later");
        }
    }

    #[test]
    fn groups_match_macos_and_apple_script_is_unavailable() {
        let mac = mac();
        let theirs: Vec<(String, bool)> = mac["groups"].as_array().unwrap().iter().map(|g| (g["id"].as_str().unwrap().to_string(), g["high_risk"].as_bool().unwrap())).collect();
        let ours: Vec<(String, bool)> = ToolGroup::ALL.iter().map(|g| (g.raw_value().to_string(), g.is_high_risk())).collect();
        assert_eq!(ours, theirs);
        assert!(!ToolGroup::AppleScript.is_available());
        assert!(ToolGroup::ALL.iter().filter(|g| **g != ToolGroup::AppleScript).all(|g| g.is_available()));
        for g in ToolGroup::ALL { assert_eq!(ToolGroup::from_raw(g.raw_value()), Some(g)); }
        for g in [ToolGroup::Shell, ToolGroup::FileAccess, ToolGroup::ProcessControl] { assert!(g.risk_note().is_some(), "{g:?}"); }
    }

    #[test]
    fn names_and_descriptions_keep_the_mcp_rules() {
        let mut seen = BTreeSet::new();
        for t in &ComputerTools::ALL {
            let n = t.mcp_name();
            assert!(n.len() <= 64 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "{n}");
            assert!(seen.insert(n.clone()), "duplicate {n}");
            assert!(t.english.is_ascii() && !t.english.is_empty() && t.english.chars().count() <= 400, "{n}");
            assert!(!t.english.contains("Mac") && !t.english.contains("zsh") && !t.english.contains("Dock"), "{n}: macOS wording");
            assert_eq!(t.input_schema()["additionalProperties"], false, "{n}");
            assert_eq!(ComputerTools::find(&n).map(|x| x.name), Some(t.name));
            assert_eq!(ComputerTools::find(t.name).map(|x| x.name), Some(t.name), "old name accepted");
            assert_eq!(ComputerTools::find_connector(&t.connector_name()).map(|x| x.name), Some(t.name));
        }
        // The Nacho tools have Korean, and only the display list is read-only.
        for t in ComputerTools::nacho() {
            assert!(ComputerTools::korean(t.name).is_some(), "{}", t.name);
            assert!(t.risk == PetToolRisk::InputInject || t.name == "screen_list_displays", "{}", t.name);
        }
        assert_eq!(ComputerTools::nacho().len(), 7);
    }

    #[test]
    fn the_closed_argument_check_refuses_before_anything_runs() {
        let c = |n: &str, v: Value| ComputerTools::check_arguments(n, Some(&v));
        assert!(c("mouse_click", json!({"x": 10, "y": 20.5, "button": "left", "clickCount": 2})).is_ok());
        assert_eq!(c("mouse_click", json!({"x": 10})), Err("Missing required argument 'y'".into()));
        assert_eq!(c("mouse_click", json!({"x": 1, "y": 1, "extra": 1})), Err("unknown argument extra".into()));
        assert_eq!(c("mouse_click", json!({"x": "1", "y": 1})), Err("'x' must be a finite number".into()));
        assert_eq!(c("mouse_click", json!({"x": 1, "y": 1, "button": "side"})), Err("'button' must be one of left, right, middle".into()));
        assert_eq!(c("mouse_click", json!({"x": 1, "y": 1, "clickCount": 1.5})), Err("'clickCount' must be an integer".into()));
        assert_eq!(c("mouse_click", json!({"x": 1, "y": 1, "clickCount": true})), Err("'clickCount' must be an integer".into()));
        assert_eq!(c("key_press", json!({"key": "c", "modifiers": ["ctrl", 1]})), Err("'modifiers' must be an array of strings".into()));
        assert_eq!(c("screen_list_displays", json!([])), Err("arguments must be an object".into()));
        assert!(ComputerTools::check_arguments("screen_list_displays", None).unwrap().is_empty());
        assert_eq!(c("process_kill", json!({"pid": 10, "signal": "STOP"})), Err("'signal' must be one of TERM, KILL, INT, HUP".into()));
    }
}

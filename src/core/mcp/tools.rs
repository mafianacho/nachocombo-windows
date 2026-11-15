//! The MCP exit of the gadget tool registry (macOS `GadgetTool.mcpName` / `englishDescription` / `exits`). The
//! registry (`PetGadgetTools::declared`, W5a) is the one list of what Windows can do and already knows each tool's MCP
//! name (`window_place`, `combo_<action>` for Nacho Combo itself, the macOS rename table for the 16 first capabilities)
//! and whether MCP offers it (`on_mcp`: not for tools that need the chat — an approval card on every run or the next
//! message's attachments). This file adds the English description MCP clients read.
//!
//! A tool reaches MCP only with an English description here. A registry tool offered on MCP without one is left out
//! of `tools/list` and fails `every_declared_tool_has_an_mcp_entry` until it gets one.
use crate::core::pet::commands::PetCommands;
use crate::core::pet::gadget_tools::{PetGadgetTools, PetToolRisk};
use crate::core::settings::ModuleId;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    /// `tools/list` name.
    pub name: String,
    /// The connector name the executor runs (`PetCommands::parse_with`).
    pub connector: String,
    /// macOS gadget id: the MCP switch and "always allow" are stored under it.
    pub gadget: String,
    pub risk: PetToolRisk,
    /// The module that must run for the tool to work (None: works with the gadget off, like the first contract).
    /// While it is off the tool is left out of `tools/list` and answers `gadget_off`.
    pub requires: Option<ModuleId>,
    pub description: &'static str,
    pub input_schema: Value,
}

impl McpTool {
    /// `tools/list` entry.
    pub fn json(&self) -> Value { serde_json::json!({"name": self.name, "description": self.description, "inputSchema": self.input_schema}) }
}

pub struct McpTools;

impl McpTools {
    /// The gadget ids that can have an MCP switch, in the macOS order (`GadgetID.allCases` without `computer`, whose
    /// tools sit under the Computer Use groups).
    pub const GADGETS: [&'static str; 15] = [
        "combo", "window", "menubar", "monitor", "disk", "clipboard", "shelf", "capture", "dim", "awake", "device", "mouse", "appswitch", "calendar", "desktop",
    ];

    /// English for MCP clients (macOS wording, Windows behaviour: PC, File Explorer, minimize instead of hide …).
    pub fn english(connector: &str) -> Option<&'static str> {
        Some(match connector {
            PetCommands::ARRANGE_WINDOW => "Moves the front window (or the front window of the named running app) to a Window Snap position: halves, maximize, quarters, thirds, next/previous display.",
            PetCommands::LAYOUT_WINDOWS => "Places windows of several apps at once in named positions (halves, quarters, thirds, maximize) on numbered displays (numbered from the left).",
            PetCommands::LIST_WINDOWS => "Lists the displays (number, size) and every app's open windows (frame, display number, minimized). Window titles are not sent.",
            PetCommands::LIST_APPS => "Lists the running apps (programs with a window: name, exe name, frontmost, all windows minimized); installed=true adds the names of the Start menu shortcuts.",
            PetCommands::OPEN_APP => "Opens an installed app by its Start menu name or exe name and brings it to the front (switches to it when it already runs).",
            PetCommands::FOCUS_APP => "Switches to a running app and shows its window in front (restores it when minimized). Use appswitch_open for an app that is not running.",
            PetCommands::HIDE_APP => "Minimizes every window of a running app (Windows has no hide; the app keeps running). File Explorer and Nacho Combo are never minimized.",
            PetCommands::QUIT_APP => "Asks a running app to close its windows (the app asks about unsaved work; never forced). The user approves every call in Nacho Combo. File Explorer and Nacho Combo are never closed.",
            PetCommands::SET_SYSTEM => "Changes the volume (0-100) and the brightness of DDC/CI monitors (0-100). Give only what changes; unsupported items (laptop panel brightness, Do Not Disturb) fail with a reason.",
            PetCommands::SET_GADGET => "Switches one Nacho Combo gadget on or off (never Nacho Pet or Computer Use).",
            PetCommands::LIST_GADGETS => "Lists the Nacho Combo gadgets and whether each one is on, off or failed.",
            PetCommands::SET_DIM_STRENGTH => "Sets how dark Focus Dim makes everything but the front window (10-90%). Stored only while the gadget is off.",
            PetCommands::SAVE_WORKSPACE => "Saves the current PC state (running apps, window positions, gadget switches) as a named workspace; the same name is overwritten. Minimized windows are not saved.",
            PetCommands::APPLY_WORKSPACE => "Restores a saved workspace: opens missing apps, moves windows back and switches gadgets as saved. Never closes other apps.",
            PetCommands::LIST_WORKSPACES => "Lists the saved workspaces (name, app and window counts, time saved).",
            PetGadgetTools::DEVICE_STATUS => "Reads the current sound output (name, volume, muted), the input name and whether the microphones are muted.",
            PetGadgetTools::DEVICE_SET_VOLUME => "Sets the current sound output volume (0-100) and/or mute. Never switches devices.",
            PetGadgetTools::DEVICE_SET_MIC_MUTED => "Mutes (true) or unmutes (false) every microphone of this PC.",
            PetGadgetTools::DIM_STATUS => "Reads whether Focus Dim is dimming or paused, and its strength.",
            PetGadgetTools::DIM_PAUSE => "Pauses Focus Dim so the screen shows at normal brightness (the gadget stays on). dim_resume turns it back.",
            PetGadgetTools::DIM_RESUME => "Resumes a paused Focus Dim: everything but the front window is dimmed again.",
            PetGadgetTools::SHELF_LIST => "Lists the File Shelf: index, id, name, kind (file, image, text, link), size in bytes or characters. No content.",
            PetGadgetTools::CLIPBOARD_LIST => "Lists the clipboard history: index, id, kind (text, link, image, files), length and unit, pinned. No previews or content.",
            PetGadgetTools::CLIPBOARD_GET_ITEM => "Reads the content of clipboard history item N (text, link or file paths, cut at 4000 characters). Pictures are not readable as text. The user approves every call in Nacho Combo unless the gadget is always allowed.",
            PetGadgetTools::CLIPBOARD_COPY_AGAIN => "Copies clipboard history item N again: it is on the clipboard now and first in the list.",
            PetGadgetTools::CAPTURE_LIST_PINNED => "Lists the open pinned captures (index, pixel width and height). No pictures.",
            PetGadgetTools::AWAKE_STATUS => "Reads whether Keep Awake holds the PC awake now, the time left and why (manual, chosen app, battery guard).",
            PetGadgetTools::AWAKE_START => "Keeps the PC awake now: minutes 1-1440 for that long, 0 or omitted until stopped. The battery guard still applies.",
            PetGadgetTools::AWAKE_STOP => "Releases the manual hold so the PC may sleep again. App rules still apply.",
            PetGadgetTools::WINDOW_DISPLAYS => "Lists the displays (number from the left, name, size, usable area). Use it before placing windows on a specific display.",
            PetGadgetTools::WINDOW_PLACE => "Free placement, the primitive behind every layout: one rectangle (x, y, w, h) per window inside a display's usable area, top-left origin, 0-1 ratios (default) or pixels. Up to 200 items per call, overlaps and z order allowed; out-of-range values are clamped.",
            PetGadgetTools::WINDOW_TILE => "Tiles windows into a grid: any rows x cols, fill order, gap, one or several displays (span: per_display or across). all_windows:true tiles every open window. Up to 200 windows per call; cells smaller than an app allows are reported with the applied frame.",
            PetGadgetTools::WINDOW_SPLIT => "Convenience: splits a display into 2, 3 or 4 equal parts and places the windows in order: horizontal (side by side), vertical (stacked) or quadrant (2x2, needs parts 4). For other shapes use window_tile or window_place.",
            PetGadgetTools::WINDOW_RATIO => "Convenience: places 2-4 windows in the given proportions, e.g. ratios [70,30] gives 70% and 30% of the width. horizontal (default) or vertical.",
            PetGadgetTools::WINDOW_MOVE_DISPLAY => "Moves windows to another display keeping their relative position and size. Display numbers come from window_displays.",
            PetGadgetTools::WINDOW_ARRANGE_APPS => "Places the front window of each listed running app in a preset arrangement: grid, columns, rows, quadrants (up to 4). Up to 200 apps per call.",
            PetGadgetTools::WINDOW_UNDO => "Restores every window moved by the last placement(s) of these window tools to its position from before, in one call. steps 1-5, default 1.",
            PetGadgetTools::APPSWITCH_SETTINGS => "Reads the App Switch settings: hide on repeat, and the names of apps that have a shortcut.",
            PetGadgetTools::APPSWITCH_SET_SETTINGS => "Changes App Switch settings: hide_on_repeat minimizes the front app when its shortcut is pressed again. Shortcuts themselves are not changed.",
            PetGadgetTools::MENUBAR_STATUS => "Reads the Tray Organizer state: whether the notification area icons are folded, the auto-fold time and how many icons are kept in view.",
            PetGadgetTools::MENUBAR_LIST_ITEMS => "Lists the notification area icons: program name and whether each is shown or folded away. No tooltips or pictures.",
            PetGadgetTools::MENUBAR_TIDY => "Tidies the notification area now: folds away every icon that is not kept in view.",
            PetGadgetTools::MENUBAR_SET_COLLAPSED => "Folds (collapsed=true) or unfolds (false) the notification area icons. Unfolded icons fold again after the auto-fold time.",
            PetGadgetTools::MONITOR_STATS => "Current System Monitor numbers: CPU %, memory %, network down/up, disk use %, battery, temperatures when the PC reports them, busiest programs. Program names are content, so the user approves every call unless the gadget is always allowed.",
            PetGadgetTools::DISK_TOP_ITEMS => "The largest folders and files of Disk Map's last scan (rank, name, path, kind, bytes, modified time; at most 50) and when it was scanned. Paths are content, so the user approves every call unless the gadget is always allowed.",
            PetGadgetTools::DISK_SCAN => "Opens Disk Map and starts scanning (the drive with the user's folder by default). Takes minutes; read the result with disk_top_items. Deletes nothing. The user approves every call unless the gadget is always allowed.",
            PetGadgetTools::MOUSE_STATUS => "Reads the Mouse Tune settings: reverse wheel, scroll speed, pointer speed and pointer precision.",
            PetGadgetTools::MOUSE_SET => "Changes Mouse Tune settings (only the given ones): reverse wheel, scroll speed 0.5/1/1.5/2/3, pointer speed from a closed list, precision keep/on/off.",
            PetGadgetTools::CALENDAR_STATUS => "Reads what the Calendar Clock shows: date, weekday, seconds, hour format and world clocks. Never event contents.",
            PetGadgetTools::CALENDAR_SET => "Changes the Calendar Clock display (only the given items): date, weekday, seconds, hours (system, twelve, twentyFour) and up to 6 world clock time zones.",
            PetGadgetTools::DESKTOP_TIDY_PLAN => "Looks at the plain files directly on the desktop and plans where each would go (a tidy folder per category): plan_id, counts per category, up to 50 file names. Moves nothing; applying a plan is only possible for Nacho in the chat, with the user's approval.",
            PetGadgetTools::DESKTOP_TIDY_HISTORY => "Desktop tidies of the last 30 days: tidy_id, time, number of files, whether undone. No file names.",
            PetGadgetTools::DESKTOP_ARRANGE_SUGGEST => "Proposes a window arrangement for the apps with open windows (app names only, never window titles): plan_id and the window_arrange_apps arguments. Moves nothing.",
            _ => return None,
        })
    }

    /// Every tool Windows can offer on MCP, in the connector hello order (switches not applied).
    pub fn all() -> Vec<McpTool> {
        PetGadgetTools::declared(true)
            .into_iter()
            .filter(|t| t.on_mcp())
            .filter_map(|t| {
                let (_, input_schema) = PetCommands::spec(t.name)?;
                Some(McpTool {
                    name: t.mcp_name(),
                    connector: t.name.to_string(),
                    gadget: t.gadget.to_string(),
                    risk: t.risk,
                    requires: t.requires,
                    description: Self::english(t.name)?,
                    input_schema,
                })
            })
            .collect()
    }

    /// The gadgets that have at least one MCP tool on Windows, in the macOS order (rows of Settings ▸ MCP).
    pub fn gadgets(list: &[McpTool]) -> Vec<&'static str> {
        Self::GADGETS.iter().copied().filter(|g| list.iter().any(|t| t.gadget == *g)).collect()
    }

    pub fn find<'a>(name: &str, list: &'a [McpTool]) -> Option<&'a McpTool> { list.iter().find(|t| t.name == name) }

    /// Windows asks in Nacho Combo before such a call runs (approval card), as the connector does on Nacho's side:
    /// destructive always, content_out / input_inject unless the gadget is always allowed. macOS lets MCP clients
    /// run them under the switches alone; Windows keeps the user in the loop.
    pub fn needs_approval(tool: &McpTool, always_allowed: &BTreeSet<String>) -> bool {
        PetGadgetTools::approval_required(&tool.connector, always_allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn mac() -> Value { serde_json::from_str(include_str!("fixtures/mac-mcp-tools.json")).unwrap() }

    fn mac_tools() -> Vec<Value> { mac()["tools"].as_array().unwrap().clone() }

    /// Types, enums, bounds, `required` and `additionalProperties` stay; wording is per platform.
    fn shape(v: &Value) -> Value {
        match v {
            Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| k.as_str() != "description").map(|(k, v)| (k.clone(), shape(v))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(shape).collect()),
            v => v.clone(),
        }
    }

    #[test]
    fn names_follow_the_macos_registry() {
        let mac = mac_tools();
        for t in McpTools::all() {
            assert!(mac.iter().any(|m| m["mcp_name"] == json!(t.name) && m["connector_name"] == json!(t.connector)), "{} / {}", t.name, t.connector);
        }
    }

    #[test]
    fn the_windows_mcp_list_matches_the_macos_registry() {
        let mac: BTreeMap<String, Value> = mac_tools().into_iter().map(|t| (t["mcp_name"].as_str().unwrap().to_string(), t)).collect();
        let ours = McpTools::all();
        assert!(!ours.is_empty());
        for t in &ours {
            let m = mac.get(&t.name).unwrap_or_else(|| panic!("{} is not a macOS MCP tool", t.name));
            assert!(m["exits"].as_array().unwrap().contains(&json!("mcp")), "{} is connector only on macOS", t.name);
            assert_eq!(m["connector_name"], json!(t.connector), "{}", t.name);
            assert_eq!(m["gadget"], json!(t.gadget), "{}", t.name);
            assert_eq!(m["risk"], json!(t.risk.raw_value()), "{}", t.name);
            // Mouse Tune and Calendar Clock have Windows options of their own; the connector test
            // (`gadget_tools::input_schemas_match_the_macos_declaration`) fixes those differences.
            if [PetGadgetTools::MOUSE_SET, PetGadgetTools::CALENDAR_SET].contains(&t.connector.as_str()) { continue; }
            let (mut theirs, mut ours) = (shape(&m["inputSchema"]), shape(&t.input_schema));
            if t.connector == PetCommands::SET_GADGET {
                // Each platform's own gadget list (as in the connector test): macOS `menuBar` is Tray Organizer here,
                // Widget Board exists on Windows only.
                let set = |v: &mut Value| -> BTreeSet<String> {
                    v["properties"]["gadget"]["enum"].take().as_array().unwrap().iter().map(|x| x.as_str().unwrap().replace("menuBar", "trayOrganizer")).collect()
                };
                let mut o = set(&mut ours);
                o.remove("widgetBoard");
                assert_eq!(o, set(&mut theirs), "set_gadget gadgets");
            }
            assert_eq!(theirs, ours, "{}: input schema differs from macOS", t.name);
        }
    }

    #[test]
    fn every_declared_tool_has_an_mcp_entry() {
        let offered: BTreeSet<String> = McpTools::all().into_iter().map(|t| t.connector).collect();
        for t in PetGadgetTools::declared(true).into_iter().filter(|t| t.on_mcp()) {
            assert!(offered.contains(t.name), "{} is offered on MCP by the registry but has no English description in mcp/tools.rs", t.name);
        }
    }

    #[test]
    fn tools_that_need_the_chat_stay_off_mcp() {
        let names: Vec<String> = McpTools::all().into_iter().map(|t| t.name).collect();
        let mac_connector_only: BTreeSet<String> = mac_tools().into_iter()
            .filter(|t| !t["exits"].as_array().unwrap().contains(&json!("mcp")))
            .map(|t| t["mcp_name"].as_str().unwrap().to_string()).collect();
        for n in &mac_connector_only { assert!(!names.contains(n), "{n} is connector only on macOS"); }
        for n in ["shelf_give", "capture_ask", "desktop_tidy_apply", "desktop_tidy_undo", "desktop_arrange_apply", "combo_gadget_guide"] {
            assert!(mac_connector_only.contains(n), "{n}");
        }
    }

    #[test]
    fn names_and_descriptions_keep_the_mcp_rules() {
        let mut seen = BTreeSet::new();
        for t in McpTools::all() {
            assert!(t.name.len() <= 64 && t.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'), "{}", t.name);
            assert!(seen.insert(t.name.clone()), "duplicate {}", t.name);
            assert!(!t.description.is_empty() && t.description.chars().count() <= 400, "{}", t.name);
            assert!(t.description.is_ascii(), "{}: English for MCP clients", t.name);
            assert!(!t.description.contains("Mac"), "{}: says Mac", t.name);
            assert_eq!(t.input_schema["type"], "object");
            assert_eq!(t.input_schema["additionalProperties"], false);
        }
        // No deleting tool on Windows, on either exit.
        assert!(McpTools::all().iter().all(|t| t.connector != PetCommands::CLEAR_SHELF));
    }

    #[test]
    fn approval_follows_the_connector_rules() {
        let all = McpTools::all();
        let get = McpTools::find("clipboard_get_item", &all).unwrap();
        let quit = McpTools::find("appswitch_quit", &all).unwrap();
        let list = McpTools::find("clipboard_list", &all).unwrap();
        let none = BTreeSet::new();
        let trusted: BTreeSet<String> = ["clipboard".to_string(), "appswitch".to_string()].into();
        assert!(McpTools::needs_approval(get, &none));
        assert!(!McpTools::needs_approval(get, &trusted), "always allow lifts content_out");
        assert!(McpTools::needs_approval(quit, &trusted), "destructive asks every time");
        assert!(!McpTools::needs_approval(list, &none));
    }

    #[test]
    fn settings_rows_are_the_gadgets_with_tools() {
        let g = McpTools::gadgets(&McpTools::all());
        assert_eq!(g, vec!["combo", "window", "menubar", "monitor", "disk", "clipboard", "shelf", "capture", "dim", "awake", "device", "mouse", "appswitch", "calendar", "desktop"]);
    }
}

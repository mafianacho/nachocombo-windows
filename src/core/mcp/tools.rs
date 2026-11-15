//! The MCP exit of the gadget tool registry (macOS `GadgetTool.mcpName` / `englishDescription` / `exits`). The
//! connector declaration (`PetCommands::windows_declaration`, `PetGadgetTools`) stays the one list of what Windows can
//! do; this file only names each tool for MCP clients and describes it in English:
//!
//! - MCP name: `<gadget>_<action>` (`combo_<action>` for Nacho Combo itself). The 16 first capabilities keep their old
//!   connector names, so they map through [`McpTools::LEGACY`] (the macOS rename table).
//! - Exits: tools that hand something to the next chat message or that need the user's approval on every run
//!   (`shelf_give`, `capture_ask`, desktop tidy apply/undo, arrange apply) and the connector-only guide are not offered
//!   to MCP clients, as on macOS ([`McpTools::CONNECTOR_ONLY`]).
//! - A tool reaches MCP only with an English description here. A connector tool added to the registry without one is
//!   left out of `tools/list` and fails `every_declared_tool_has_an_mcp_entry` until it gets one.
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
    /// The macOS rename table: first-contract connector name → MCP name.
    pub const LEGACY: [(&'static str, &'static str); 16] = [
        (PetCommands::ARRANGE_WINDOW, "window_arrange"),
        (PetCommands::LAYOUT_WINDOWS, "window_layout"),
        (PetCommands::LIST_WINDOWS, "window_list_windows"),
        (PetCommands::LIST_APPS, "appswitch_list"),
        (PetCommands::OPEN_APP, "appswitch_open"),
        (PetCommands::FOCUS_APP, "appswitch_focus"),
        (PetCommands::HIDE_APP, "appswitch_hide"),
        (PetCommands::QUIT_APP, "appswitch_quit"),
        (PetCommands::SET_SYSTEM, "device_set"),
        (PetCommands::SET_GADGET, "combo_set_gadget"),
        (PetCommands::LIST_GADGETS, "combo_list_gadgets"),
        (PetCommands::SET_DIM_STRENGTH, "dim_set_strength"),
        (PetCommands::CLEAR_SHELF, "shelf_clear"),
        (PetCommands::SAVE_WORKSPACE, "combo_workspace_save"),
        (PetCommands::APPLY_WORKSPACE, "combo_workspace_apply"),
        (PetCommands::LIST_WORKSPACES, "combo_workspace_list"),
    ];

    /// Connector only on macOS (`exits: [.connector]`).
    pub const CONNECTOR_ONLY: [&'static str; 6] = [
        "combo_shelf_give",
        "combo_capture_ask",
        "combo_gadget_guide",
        "combo_desktop_tidy_apply",
        "combo_desktop_tidy_undo",
        "combo_desktop_arrange_apply",
    ];

    /// The gadget ids that can have an MCP switch, in the macOS order (`GadgetID.allCases` without `computer`, whose
    /// tools sit under the Computer Use groups).
    pub const GADGETS: [&'static str; 15] = [
        "combo", "window", "menubar", "monitor", "disk", "clipboard", "shelf", "capture", "dim", "awake", "device", "mouse", "appswitch", "calendar", "desktop",
    ];

    /// The MCP name of a connector name.
    pub fn mcp_name(connector: &str) -> String {
        if let Some((_, m)) = Self::LEGACY.iter().find(|(c, _)| *c == connector) { return m.to_string(); }
        match connector.strip_prefix("combo_") {
            Some(rest) if Self::GADGETS.iter().any(|g| *g != "combo" && rest.starts_with(&format!("{g}_"))) => rest.to_string(),
            _ => connector.to_string(),
        }
    }

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
            _ => return None,
        })
    }

    /// Every tool Windows can offer on MCP, in the connector hello order (switches not applied).
    pub fn all() -> Vec<McpTool> {
        let none = BTreeSet::new();
        PetCommands::windows_declaration(&none, &none, true)
            .into_iter()
            .filter_map(|c| {
                let connector = c["name"].as_str()?.to_string();
                if Self::CONNECTOR_ONLY.contains(&connector.as_str()) { return None; }
                let tool = PetGadgetTools::find(&connector)?;
                Some(McpTool {
                    name: Self::mcp_name(&connector),
                    gadget: tool.gadget.to_string(),
                    risk: tool.risk,
                    requires: tool.requires,
                    description: Self::english(&connector)?,
                    input_schema: c["input_schema"].clone(),
                    connector,
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
    fn names_follow_the_macos_rename_table() {
        let mac = mac_tools();
        for t in &mac {
            let connector = t["connector_name"].as_str().unwrap();
            assert_eq!(McpTools::mcp_name(connector), t["mcp_name"].as_str().unwrap(), "{connector}");
        }
        assert_eq!(McpTools::mcp_name("combo_window_place"), "window_place");
        assert_eq!(McpTools::mcp_name("combo_list_gadgets"), "combo_list_gadgets");
        assert_eq!(McpTools::mcp_name("combo_gadget_guide"), "combo_gadget_guide");
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
        let none = BTreeSet::new();
        let offered: BTreeSet<String> = McpTools::all().into_iter().map(|t| t.connector).collect();
        for c in PetCommands::windows_declaration(&none, &none, true) {
            let name = c["name"].as_str().unwrap();
            if McpTools::CONNECTOR_ONLY.contains(&name) { continue; }
            assert!(offered.contains(name), "{name} is declared to Nacho but has no MCP name/English description in mcp/tools.rs");
        }
    }

    #[test]
    fn connector_only_tools_stay_off_mcp() {
        let names: Vec<String> = McpTools::all().into_iter().map(|t| t.connector).collect();
        for c in McpTools::CONNECTOR_ONLY { assert!(!names.iter().any(|n| n == c), "{c}"); }
        let mac_connector_only: BTreeSet<String> = mac_tools().into_iter()
            .filter(|t| !t["exits"].as_array().unwrap().contains(&json!("mcp")))
            .map(|t| t["connector_name"].as_str().unwrap().to_string()).collect();
        assert_eq!(mac_connector_only, McpTools::CONNECTOR_ONLY.iter().map(|s| s.to_string()).collect());
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
        assert_eq!(g, vec!["combo", "window", "clipboard", "shelf", "capture", "dim", "awake", "device", "appswitch"]);
    }
}

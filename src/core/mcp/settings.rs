//! Settings of the NachoCombo MCP server (the part of macOS `ComputerUseSettings` that applies on Windows), stored in
//! `settings.json` under `computerUse` like macOS. Everything is off by default: the server switch closes the port,
//! and every gadget's MCP switch is off. A missing, broken or wrongly typed value reads as its default without
//! touching the rest of the settings file.
//!
//! Computer Use (experimental, W5d) adds its own switch (`computerEnabled`, Windows only: macOS keeps the gadget switch
//! in its module list) and the tool group switches (`enabledGroups`, the macOS key). Both are off by default; AppleScript
//! can never be on (Windows has none). Unlike macOS, switching Computer Use on switches no group on by itself.
//!
//! Not on Windows: `listenMode`/LAN (the server is 127.0.0.1 only), `bindAddress` (always 127.0.0.1) and
//! `allowedHosts` (tunnels are out of scope).
use crate::core::computer::tools::ToolGroup;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    /// Settings ▸ MCP ▸ the server switch: whether the port is open for MCP clients on this PC.
    pub server_enabled: bool,
    pub port: u16,
    /// MCP switch per gadget id (`window`, `clipboard` …): its tools are listed and callable only while on.
    pub enabled_gadgets: BTreeMap<String, bool>,
    /// Computer Use (experimental): Nacho's display and input tools, and the tool groups below on MCP.
    pub computer_enabled: bool,
    /// Computer Use tool group switches (`ToolGroup::raw_value` → on), all off by default.
    pub enabled_groups: BTreeMap<String, bool>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self { server_enabled: false, port: Self::DEFAULT_PORT, enabled_gadgets: BTreeMap::new(), computer_enabled: false, enabled_groups: BTreeMap::new() }
    }
}

impl McpSettings {
    pub const DEFAULT_PORT: u16 = 47653;

    pub fn is_valid_port(port: i64) -> bool { (1024..=65535).contains(&port) }

    /// `computer` has no switch of its own (its tools come with Computer Use, W5d).
    pub fn is_gadget_enabled(&self, gadget: &str) -> bool { gadget != "computer" && self.enabled_gadgets.get(gadget).copied().unwrap_or(false) }

    pub fn set_gadget_enabled(&mut self, gadget: &str, on: bool) {
        if gadget == "computer" { return; }
        self.enabled_gadgets.insert(gadget.to_string(), on);
    }

    /// A group's switch (AppleScript is never on: Windows has none).
    pub fn is_group_enabled(&self, group: ToolGroup) -> bool { group.is_available() && self.enabled_groups.get(group.raw_value()).copied().unwrap_or(false) }

    pub fn set_group_enabled(&mut self, group: ToolGroup, on: bool) {
        if !group.is_available() { return; }
        self.enabled_groups.insert(group.raw_value().to_string(), on);
    }

    /// The groups whose tools MCP clients get right now: Computer Use is on and the group is on.
    pub fn served_groups(&self) -> Vec<ToolGroup> {
        if !self.computer_enabled { return vec![]; }
        ToolGroup::ALL.into_iter().filter(|g| self.is_group_enabled(*g)).collect()
    }

    /// Reads each field on its own: a wrong type keeps that field's default.
    pub fn from_value(v: &Value) -> Self {
        let mut s = Self::default();
        let Some(o) = v.as_object() else { return s };
        if let Some(b) = o.get("serverEnabled").and_then(Value::as_bool) { s.server_enabled = b; }
        if let Some(p) = o.get("port").and_then(Value::as_i64).filter(|p| Self::is_valid_port(*p)) { s.port = p as u16; }
        if let Some(g) = o.get("enabledGadgets").and_then(Value::as_object) {
            s.enabled_gadgets = g.iter().filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b))).collect();
        }
        if let Some(b) = o.get("computerEnabled").and_then(Value::as_bool) { s.computer_enabled = b; }
        // Unknown group names and wrong types are dropped; AppleScript never loads as on.
        if let Some(g) = o.get("enabledGroups").and_then(Value::as_object) {
            s.enabled_groups = g.iter()
                .filter_map(|(k, v)| Some((ToolGroup::from_raw(k)?, v.as_bool()?)))
                .filter(|(g, _)| g.is_available())
                .map(|(g, b)| (g.raw_value().to_string(), b)).collect();
        }
        s
    }
}

impl<'de> Deserialize<'de> for McpSettings {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(Self::from_value(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn everything_is_off_by_default() {
        let s = McpSettings::default();
        assert!(!s.server_enabled);
        assert_eq!(s.port, 47653);
        assert!(!s.is_gadget_enabled("window"));
        assert!(!s.computer_enabled);
        assert!(ToolGroup::ALL.iter().all(|g| !s.is_group_enabled(*g)), "every Computer Use group off");
        assert!(s.served_groups().is_empty());
    }

    #[test]
    fn computer_use_groups_need_the_computer_use_switch_and_apple_script_never_loads() {
        let s = McpSettings::from_value(&json!({"enabledGroups": {"screen": true, "input": "yes", "appleScript": true, "teleport": true, "shell": true}}));
        assert!(s.is_group_enabled(ToolGroup::Screen) && s.is_group_enabled(ToolGroup::Shell));
        assert!(!s.is_group_enabled(ToolGroup::Input), "wrong type");
        assert!(!s.is_group_enabled(ToolGroup::AppleScript));
        assert_eq!(s.enabled_groups.len(), 2, "unknown and unavailable groups are dropped: {:?}", s.enabled_groups);
        assert!(s.served_groups().is_empty(), "Computer Use is off");
        let mut s = McpSettings::from_value(&json!({"computerEnabled": true, "enabledGroups": {"screen": true, "shell": false}}));
        assert_eq!(s.served_groups(), vec![ToolGroup::Screen]);
        s.set_group_enabled(ToolGroup::AppleScript, true);
        assert!(!s.is_group_enabled(ToolGroup::AppleScript));
        s.set_group_enabled(ToolGroup::FileAccess, true);
        assert_eq!(s.served_groups(), vec![ToolGroup::Screen, ToolGroup::FileAccess]);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["enabledGroups"], json!({"screen": true, "shell": false, "fileAccess": true}));
        assert_eq!(McpSettings::from_value(&v), s);
        assert_eq!(McpSettings::from_value(&json!({"computerEnabled": 1})).computer_enabled, false);
    }

    #[test]
    fn broken_values_fall_back_one_by_one() {
        let s = McpSettings::from_value(&json!({"serverEnabled": "yes", "port": 80, "enabledGadgets": {"window": true, "clipboard": 1}}));
        assert!(!s.server_enabled);
        assert_eq!(s.port, 47653);
        assert!(s.is_gadget_enabled("window"));
        assert!(!s.is_gadget_enabled("clipboard"));
        let s = McpSettings::from_value(&json!({"serverEnabled": true, "port": 50000, "enabledGadgets": [1]}));
        assert!(s.server_enabled);
        assert_eq!(s.port, 50000);
        assert!(s.enabled_gadgets.is_empty());
        assert_eq!(McpSettings::from_value(&json!("junk")), McpSettings::default());
    }

    #[test]
    fn computer_use_has_no_gadget_switch_here() {
        let mut s = McpSettings::default();
        s.set_gadget_enabled("computer", true);
        assert!(!s.is_gadget_enabled("computer"));
        let s = McpSettings::from_value(&json!({"enabledGadgets": {"computer": true}}));
        assert!(!s.is_gadget_enabled("computer"));
    }

    #[test]
    fn round_trips_in_the_macos_key_names() {
        let mut s = McpSettings { server_enabled: true, ..Default::default() };
        s.set_gadget_enabled("dim", true);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, json!({"serverEnabled": true, "port": 47653, "enabledGadgets": {"dim": true}, "computerEnabled": false, "enabledGroups": {}}));
        assert_eq!(serde_json::from_value::<McpSettings>(v).unwrap(), s);
    }
}

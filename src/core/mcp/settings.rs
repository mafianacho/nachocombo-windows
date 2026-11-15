//! Settings of the NachoCombo MCP server (the part of macOS `ComputerUseSettings` that applies on Windows), stored in
//! `settings.json` under `computerUse` like macOS. Everything is off by default: the server switch closes the port,
//! and every gadget's MCP switch is off. A missing, broken or wrongly typed value reads as its default without
//! touching the rest of the settings file.
//!
//! Not on Windows: `listenMode`/LAN (the server is 127.0.0.1 only), `bindAddress` (always 127.0.0.1),
//! `allowedHosts` (tunnels are out of scope) and the Computer Use tool groups (W5d adds them next to these fields).
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
}

impl Default for McpSettings {
    fn default() -> Self { Self { server_enabled: false, port: Self::DEFAULT_PORT, enabled_gadgets: BTreeMap::new() } }
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

    /// Reads each field on its own: a wrong type keeps that field's default.
    pub fn from_value(v: &Value) -> Self {
        let mut s = Self::default();
        let Some(o) = v.as_object() else { return s };
        if let Some(b) = o.get("serverEnabled").and_then(Value::as_bool) { s.server_enabled = b; }
        if let Some(p) = o.get("port").and_then(Value::as_i64).filter(|p| Self::is_valid_port(*p)) { s.port = p as u16; }
        if let Some(g) = o.get("enabledGadgets").and_then(Value::as_object) {
            s.enabled_gadgets = g.iter().filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b))).collect();
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
        assert_eq!(v, json!({"serverEnabled": true, "port": 47653, "enabledGadgets": {"dim": true}}));
        assert_eq!(serde_json::from_value::<McpSettings>(v).unwrap(), s);
    }
}

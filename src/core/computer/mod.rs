//! Computer Use for Windows (port of the macOS Computer Use MCP server, `docs/COMPUTER-USE-MCP.md`), **experimental**.
//! MCP clients on this PC can look at the screen and drive apps, windows, input and the clipboard through the
//! NachoCombo MCP server (`core::mcp`, 127.0.0.1 only), and Nacho in the chat gets the display list and the input tools.
//! Everything is off by default: the Computer Use switch, every tool group, and the high-risk groups ask before they
//! go on. Every MCP call waits for the user's approval card; Nacho's input calls too.
//!
//! - [`tools`]: the tool groups and the catalog (names, groups, risks, input schemas: the macOS ones; Windows wording).
//! - [`policy`]: argument rules that do not touch Windows (keys, coordinates, URLs, paths, shell limits, processes).
//! - [`ComputerBackend`]: what the Windows shell (`src/computer.rs`) implements; [`EmergencyStop`]: the latch behind
//!   `Ctrl+Alt+Win+.` and the page's button.
pub mod policy;
pub mod tools;

use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// One part of a tool answer (MCP `content`).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Text(String),
    Image { base64: String, mime: String },
}

impl ToolContent {
    pub fn json(&self) -> Value {
        match self {
            Self::Text(t) => json!({"type": "text", "text": t}),
            Self::Image { base64, mime } => json!({"type": "image", "data": base64, "mimeType": mime}),
        }
    }
}

/// A tool answer (macOS `ToolResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(t: impl Into<String>) -> Self { Self { content: vec![ToolContent::Text(t.into())], is_error: false } }
    pub fn error(t: impl Into<String>) -> Self { Self { content: vec![ToolContent::Text(t.into())], is_error: true } }
    /// A JSON object as the text part (what the Windows backend answers for lists and states).
    pub fn object(v: Value) -> Self { Self::text(serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())) }
    pub fn json(&self) -> Value { json!({"content": self.content.iter().map(ToolContent::json).collect::<Vec<_>>(), "isError": self.is_error}) }
    /// The text parts joined (for logs and Nacho's answer).
    pub fn text_parts(&self) -> String {
        self.content.iter().filter_map(|c| match c { ToolContent::Text(t) => Some(t.as_str()), _ => None }).collect::<Vec<_>>().join("\n")
    }
}

/// The platform side. It is only called after the service checked the key, the emergency stop, the group switch, the
/// approval and the closed argument check ([`tools::ComputerTools::check_arguments`]); it still applies [`policy`]
/// before it touches the system.
pub trait ComputerBackend: Send + Sync {
    /// `tool` is the macOS Computer Use name (`screen_capture`).
    fn execute(&self, tool: &str, arguments: &Map<String, Value>) -> ToolResult;
    /// The emergency stop fired: end a running shell command now.
    fn cancel(&self) {}
}

/// The emergency stop (macOS `ComputerUseModule.isEmergencyStopped`): latches at once from any thread, refuses every
/// call (also calls that already passed the approval and wait for their turn), and is cleared only when Computer Use is
/// switched on again. `generation` changes on every stop, so a call that started before can tell it was stopped.
#[derive(Default)]
pub struct EmergencyStop {
    active: AtomicBool,
    generation: AtomicU64,
}

impl EmergencyStop {
    pub fn fire(&self) {
        self.active.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    pub fn clear(&self) { self.active.store(false, Ordering::SeqCst); }
    pub fn is_active(&self) -> bool { self.active.load(Ordering::SeqCst) }
    pub fn generation(&self) -> u64 { self.generation.load(Ordering::SeqCst) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_emergency_stop_latches_until_cleared() {
        let s = EmergencyStop::default();
        assert!(!s.is_active());
        let g = s.generation();
        s.fire();
        assert!(s.is_active());
        assert_ne!(s.generation(), g, "a call that started before the stop sees a new generation");
        s.fire();
        assert!(s.is_active());
        s.clear();
        assert!(!s.is_active());
    }

    #[test]
    fn results_are_mcp_content() {
        let r = ToolResult { content: vec![ToolContent::Text("a".into()), ToolContent::Image { base64: "AA==".into(), mime: "image/png".into() }], is_error: false };
        assert_eq!(r.json(), json!({"content": [{"type": "text", "text": "a"}, {"type": "image", "data": "AA==", "mimeType": "image/png"}], "isError": false}));
        assert_eq!(ToolResult::error("no").json()["isError"], true);
        assert_eq!(ToolResult::object(json!({"ok": true})).text_parts(), "{\"ok\":true}");
    }
}

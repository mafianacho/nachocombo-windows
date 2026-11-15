//! The gadget tool registry: every connector capability this build knows (the 16 first capabilities and the gadget
//! tools), its macOS gadget id, risk, approval rule, the module that must be on, and whether the gadget MCP offers it.
//! Port of the macOS registry (`GadgetToolContract.swift`, `GadgetToolRegistry.swift`, `Gadgets/Tools/*Tools.swift`).
//! Names, input schemas and risks are the macOS ones; descriptions use Windows wording (PC instead of 맥). The
//! connector declaration ([`PetCommands::windows_declaration`]), the connection tab and the gadget MCP all read this
//! list, so a tool added here shows up everywhere.
//!
//! Public API (used by the connector, the connection tab and the gadget MCP):
//! - [`PetGadgetTools::declared`]: the tools Windows declares, in hello order ([`PetGadgetTool`] rows).
//! - [`PetGadgetTools::by_gadget`] / [`PetGadgetTools::gadgets`]: the same rows grouped per macOS gadget id.
//! - [`PetGadgetTool::risk`], [`PetGadgetTool::approval_required`], [`PetGadgetTools::approval_required`],
//!   [`PetGadgetTools::trustable`]: what asks the user first.
//! - [`PetGadgetTool::mcp_name`] / [`PetGadgetTool::on_mcp`]: the MCP name (`window_place`) and whether MCP offers it.
//! - [`PetGadgetTools::spec`] (Korean description + input schema) and [`PetGadgetTools::parse`] (closed checks).
//!
//! Risk decides the approval on the connector path: `read` and `change` run at once, `content_out` (the PC's content
//! leaves it) asks every time unless the user set the gadget to "always allow" (`trust.rs`), `destructive` always asks.
//! Two rules are stricter than the risk ([`PetApproval`]): desktop tidy moves files, so its apply and undo ask every
//! time; System Monitor and Disk Map answers carry program names and file paths, so on Windows they ask unless the
//! gadget is always allowed.
//!
//! Not declared by Windows: actions that delete or throw something away (`combo_clipboard_clear`,
//! `combo_shelf_forget`, `combo_capture_close_all`; `combo_clear_shelf` stays in the first-contract list but is not
//! declared), `combo_menubar_show_hidden` (Windows has no always-hidden tray section) and Computer Use (later).
use super::commands::{PetCommand, PetCommandOutcome, PetCommandProblem, PetCommands};
use super::crypto;
use super::data_tools;
use super::desktop_tidy::PetDesktopTools;
use super::gadget_guide::PetGadgetGuides;
use super::json::JsonObject;
use super::settings_tools;
use super::window_tools::PetWindowTools;
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// How much a tool can do (macOS `GadgetToolRisk`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetToolRisk {
    /// Looks at state or lists (numbers, kinds, lengths, names).
    Read,
    /// Changes the PC in a way the user can see and undo.
    Change,
    /// Sends content off the PC (clipboard text, a capture image, a file itself).
    ContentOut,
    /// Types, clicks or presses keys for the user (none on Windows yet; Computer Use is W5).
    InputInject,
    /// Cannot be undone (quit an app).
    Destructive,
}

impl PetToolRisk {
    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Change => "change",
            Self::ContentOut => "content_out",
            Self::InputInject => "input_inject",
            Self::Destructive => "destructive",
        }
    }
    /// The kinds "always allow" (per gadget) may lift.
    pub fn is_trustable(self) -> bool { matches!(self, Self::ContentOut | Self::InputInject) }
}

/// When the connector declares a tool with `"approval": "required"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetApproval {
    /// By risk (macOS `connectorApproval`).
    ByRisk,
    /// Every time, whatever the risk and "always allow" say (macOS `alwaysRequiresApproval`).
    Always,
    /// Windows: every time unless the gadget is always allowed, whatever the risk (the answer carries the PC's content).
    UnlessAlwaysAllowed,
}

/// One tool: its connector name, the macOS gadget id it belongs to (`clipboard`, `shelf` …, the ids "always allow"
/// is stored under), risk, and the module that must be on for it to run (None: runs with the gadget off, as the
/// migrated first-contract capabilities do on macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PetGadgetTool {
    /// The declared connector name.
    pub name: &'static str,
    pub gadget: &'static str,
    pub risk: PetToolRisk,
    pub requires: Option<ModuleId>,
    /// The macOS registry's `combo_<gadget>_<action>` (`combo_<action>` for Nacho Combo itself); the same as `name`
    /// except for the 16 first capabilities, which keep the names nacho-server already knows.
    pub canonical: &'static str,
    /// Offered by the gadget MCP too (macOS `exits` has `.mcp`). Off for what needs the chat (an approval card, the
    /// next message's attachments).
    pub mcp: bool,
    pub approval: PetApproval,
}

const fn tool(name: &'static str, gadget: &'static str, risk: PetToolRisk, requires: Option<ModuleId>) -> PetGadgetTool {
    PetGadgetTool { name, gadget, risk, requires, canonical: name, mcp: true, approval: PetApproval::ByRisk }
}

const fn legacy(name: &'static str, canonical: &'static str, gadget: &'static str, risk: PetToolRisk) -> PetGadgetTool {
    PetGadgetTool { name, gadget, risk, requires: None, canonical, mcp: true, approval: PetApproval::ByRisk }
}

impl PetGadgetTool {
    const fn chat_only(mut self) -> Self { self.mcp = false; self }
    const fn approval(mut self, a: PetApproval) -> Self { self.approval = a; self }

    /// `place` of `combo_window_place`, `set_gadget` of `combo_set_gadget`.
    pub fn action(&self) -> &'static str {
        let c = self.canonical;
        let prefix_len = if self.gadget == "combo" { "combo_".len() } else { "combo_".len() + self.gadget.len() + 1 };
        c.get(prefix_len..).unwrap_or(c)
    }

    /// The MCP name: `<gadget>_<action>` (`window_place`), `combo_<action>` for Nacho Combo itself.
    pub fn mcp_name(&self) -> String { format!("{}_{}", self.gadget, self.action()) }

    pub fn on_mcp(&self) -> bool { self.mcp }

    /// Whether the connector declares it with `"approval": "required"` (`always_allowed`: its gadget's switch).
    pub fn approval_required(&self, always_allowed: bool) -> bool {
        match self.approval {
            PetApproval::Always => true,
            PetApproval::UnlessAlwaysAllowed => !always_allowed,
            PetApproval::ByRisk => match self.risk {
                PetToolRisk::Read | PetToolRisk::Change => false,
                PetToolRisk::ContentOut | PetToolRisk::InputInject => !always_allowed,
                PetToolRisk::Destructive => true,
            },
        }
    }

    /// "Always allow" can lift its approval.
    pub fn is_trustable(&self) -> bool {
        match self.approval { PetApproval::Always => false, PetApproval::UnlessAlwaysAllowed => true, PetApproval::ByRisk => self.risk.is_trustable() }
    }
}

use PetToolRisk::*;

pub struct PetGadgetTools;

impl PetGadgetTools {
    pub const GADGET_GUIDE: &'static str = "combo_gadget_guide";
    pub const WINDOW_DISPLAYS: &'static str = "combo_window_displays";
    pub const WINDOW_PLACE: &'static str = "combo_window_place";
    pub const WINDOW_TILE: &'static str = "combo_window_tile";
    pub const WINDOW_SPLIT: &'static str = "combo_window_split";
    pub const WINDOW_RATIO: &'static str = "combo_window_ratio";
    pub const WINDOW_MOVE_DISPLAY: &'static str = "combo_window_move_display";
    pub const WINDOW_ARRANGE_APPS: &'static str = "combo_window_arrange_apps";
    pub const WINDOW_UNDO: &'static str = "combo_window_undo";
    pub const APPSWITCH_SETTINGS: &'static str = "combo_appswitch_settings";
    pub const APPSWITCH_SET_SETTINGS: &'static str = "combo_appswitch_set_settings";
    pub const DEVICE_STATUS: &'static str = "combo_device_status";
    pub const DEVICE_SET_VOLUME: &'static str = "combo_device_set_volume";
    pub const DEVICE_SET_MIC_MUTED: &'static str = "combo_device_set_mic_muted";
    pub const DIM_STATUS: &'static str = "combo_dim_status";
    pub const DIM_PAUSE: &'static str = "combo_dim_pause";
    pub const DIM_RESUME: &'static str = "combo_dim_resume";
    pub const SHELF_LIST: &'static str = "combo_shelf_list";
    pub const SHELF_GIVE: &'static str = "combo_shelf_give";
    pub const MENUBAR_STATUS: &'static str = "combo_menubar_status";
    pub const MENUBAR_LIST_ITEMS: &'static str = "combo_menubar_list_items";
    pub const MENUBAR_TIDY: &'static str = "combo_menubar_tidy";
    pub const MENUBAR_SET_COLLAPSED: &'static str = "combo_menubar_set_collapsed";
    pub const MONITOR_STATS: &'static str = "combo_monitor_stats";
    pub const DISK_TOP_ITEMS: &'static str = "combo_disk_top_items";
    pub const DISK_SCAN: &'static str = "combo_disk_scan";
    pub const CLIPBOARD_LIST: &'static str = "combo_clipboard_list";
    pub const CLIPBOARD_GET_ITEM: &'static str = "combo_clipboard_get_item";
    pub const CLIPBOARD_COPY_AGAIN: &'static str = "combo_clipboard_copy_again";
    pub const CAPTURE_ASK: &'static str = "combo_capture_ask";
    pub const CAPTURE_LIST_PINNED: &'static str = "combo_capture_list_pinned";
    pub const AWAKE_STATUS: &'static str = "combo_awake_status";
    pub const AWAKE_START: &'static str = "combo_awake_start";
    pub const AWAKE_STOP: &'static str = "combo_awake_stop";
    pub const MOUSE_STATUS: &'static str = "combo_mouse_status";
    pub const MOUSE_SET: &'static str = "combo_mouse_set";
    pub const CALENDAR_STATUS: &'static str = "combo_calendar_status";
    pub const CALENDAR_SET: &'static str = "combo_calendar_set";
    pub const DESKTOP_TIDY_PLAN: &'static str = "combo_desktop_tidy_plan";
    pub const DESKTOP_TIDY_APPLY: &'static str = "combo_desktop_tidy_apply";
    pub const DESKTOP_TIDY_UNDO: &'static str = "combo_desktop_tidy_undo";
    pub const DESKTOP_TIDY_HISTORY: &'static str = "combo_desktop_tidy_history";
    pub const DESKTOP_ARRANGE_SUGGEST: &'static str = "combo_desktop_arrange_suggest";
    pub const DESKTOP_ARRANGE_APPLY: &'static str = "combo_desktop_arrange_apply";

    /// Every gadget tool this build knows, in the macOS hello order (after the 16 first capabilities).
    pub const TOOLS: [PetGadgetTool; 44] = [
        tool(Self::GADGET_GUIDE, "combo", Read, None).chat_only(),
        tool(Self::WINDOW_DISPLAYS, "window", Read, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_PLACE, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_TILE, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_SPLIT, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_RATIO, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_MOVE_DISPLAY, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_ARRANGE_APPS, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::WINDOW_UNDO, "window", Change, Some(ModuleId::WindowSnap)),
        tool(Self::APPSWITCH_SETTINGS, "appswitch", Read, Some(ModuleId::AppSwitch)),
        tool(Self::APPSWITCH_SET_SETTINGS, "appswitch", Change, Some(ModuleId::AppSwitch)),
        tool(Self::DEVICE_STATUS, "device", Read, Some(ModuleId::DeviceControl)),
        tool(Self::DEVICE_SET_VOLUME, "device", Change, Some(ModuleId::DeviceControl)),
        tool(Self::DEVICE_SET_MIC_MUTED, "device", Change, Some(ModuleId::DeviceControl)),
        tool(Self::DIM_STATUS, "dim", Read, Some(ModuleId::FocusDim)),
        tool(Self::DIM_PAUSE, "dim", Change, Some(ModuleId::FocusDim)),
        tool(Self::DIM_RESUME, "dim", Change, Some(ModuleId::FocusDim)),
        tool(Self::SHELF_LIST, "shelf", Read, Some(ModuleId::FileShelf)),
        tool(Self::SHELF_GIVE, "shelf", ContentOut, Some(ModuleId::FileShelf)).chat_only(),
        tool(Self::MENUBAR_STATUS, "menubar", Read, Some(ModuleId::TrayOrganizer)),
        tool(Self::MENUBAR_LIST_ITEMS, "menubar", Read, Some(ModuleId::TrayOrganizer)),
        tool(Self::MENUBAR_TIDY, "menubar", Change, Some(ModuleId::TrayOrganizer)),
        tool(Self::MENUBAR_SET_COLLAPSED, "menubar", Change, Some(ModuleId::TrayOrganizer)),
        tool(Self::MONITOR_STATS, "monitor", Read, Some(ModuleId::SystemMonitor)).approval(PetApproval::UnlessAlwaysAllowed),
        tool(Self::DISK_TOP_ITEMS, "disk", Read, Some(ModuleId::DiskMap)).approval(PetApproval::UnlessAlwaysAllowed),
        tool(Self::DISK_SCAN, "disk", Change, Some(ModuleId::DiskMap)).approval(PetApproval::UnlessAlwaysAllowed),
        tool(Self::CLIPBOARD_LIST, "clipboard", Read, Some(ModuleId::Clipboard)),
        tool(Self::CLIPBOARD_GET_ITEM, "clipboard", ContentOut, Some(ModuleId::Clipboard)),
        tool(Self::CLIPBOARD_COPY_AGAIN, "clipboard", Change, Some(ModuleId::Clipboard)),
        tool(Self::CAPTURE_ASK, "capture", ContentOut, Some(ModuleId::CaptureFloat)).chat_only(),
        tool(Self::CAPTURE_LIST_PINNED, "capture", Read, Some(ModuleId::CaptureFloat)),
        tool(Self::AWAKE_STATUS, "awake", Read, Some(ModuleId::KeepAwake)),
        tool(Self::AWAKE_START, "awake", Change, Some(ModuleId::KeepAwake)),
        tool(Self::AWAKE_STOP, "awake", Change, Some(ModuleId::KeepAwake)),
        tool(Self::MOUSE_STATUS, "mouse", Read, Some(ModuleId::MouseTune)),
        tool(Self::MOUSE_SET, "mouse", Change, Some(ModuleId::MouseTune)),
        tool(Self::CALENDAR_STATUS, "calendar", Read, Some(ModuleId::CalendarClock)),
        tool(Self::CALENDAR_SET, "calendar", Change, Some(ModuleId::CalendarClock)),
        // Desktop tidy has no module of its own (always available, as on macOS); every move is approved.
        tool(Self::DESKTOP_TIDY_PLAN, "desktop", Read, None),
        tool(Self::DESKTOP_TIDY_APPLY, "desktop", Change, None).chat_only().approval(PetApproval::Always),
        // macOS runs undo without a card; Windows asks every time (files move again).
        tool(Self::DESKTOP_TIDY_UNDO, "desktop", Change, None).chat_only().approval(PetApproval::Always),
        tool(Self::DESKTOP_TIDY_HISTORY, "desktop", Read, None),
        tool(Self::DESKTOP_ARRANGE_SUGGEST, "desktop", Read, None),
        // Runs through Window Snap's arrange_apps, so the window gadget must be on.
        tool(Self::DESKTOP_ARRANGE_APPLY, "desktop", Change, Some(ModuleId::WindowSnap)).chat_only().approval(PetApproval::Always),
    ];

    /// The 16 first capabilities: gadget and risk as the macOS registry gives them (`legacyConnectorName`).
    pub const LEGACY: [PetGadgetTool; 16] = [
        legacy(PetCommands::ARRANGE_WINDOW, "combo_window_arrange", "window", Change),
        legacy(PetCommands::LAYOUT_WINDOWS, "combo_window_layout", "window", Change),
        legacy(PetCommands::LIST_WINDOWS, "combo_window_list_windows", "window", Read),
        legacy(PetCommands::LIST_APPS, "combo_appswitch_list", "appswitch", Read),
        legacy(PetCommands::OPEN_APP, "combo_appswitch_open", "appswitch", Change),
        legacy(PetCommands::FOCUS_APP, "combo_appswitch_focus", "appswitch", Change),
        legacy(PetCommands::HIDE_APP, "combo_appswitch_hide", "appswitch", Change),
        legacy(PetCommands::QUIT_APP, "combo_appswitch_quit", "appswitch", Destructive),
        legacy(PetCommands::SET_SYSTEM, "combo_device_set", "device", Change),
        legacy(PetCommands::SET_GADGET, PetCommands::SET_GADGET, "combo", Change),
        legacy(PetCommands::LIST_GADGETS, PetCommands::LIST_GADGETS, "combo", Read),
        legacy(PetCommands::SET_DIM_STRENGTH, "combo_dim_set_strength", "dim", Change),
        legacy(PetCommands::CLEAR_SHELF, "combo_shelf_clear", "shelf", Destructive),
        legacy(PetCommands::SAVE_WORKSPACE, PetCommands::SAVE_WORKSPACE, "combo", Change),
        legacy(PetCommands::APPLY_WORKSPACE, PetCommands::APPLY_WORKSPACE, "combo", Change),
        legacy(PetCommands::LIST_WORKSPACES, PetCommands::LIST_WORKSPACES, "combo", Read),
    ];

    /// What the Windows app declares, in hello order: the first contract without `combo_clear_shelf` (it throws the
    /// shelf's items away; no deleting actions on Windows), then every gadget tool except the two chat handoff
    /// tools ([`Self::WITH_CHAT_HANDOFF`], declared once the shell can put a file or a picture on the next message).
    pub const WINDOWS: [&'static str; 57] = [
        PetCommands::ARRANGE_WINDOW, PetCommands::LAYOUT_WINDOWS, PetCommands::LIST_WINDOWS, PetCommands::LIST_APPS,
        PetCommands::OPEN_APP, PetCommands::FOCUS_APP, PetCommands::HIDE_APP, PetCommands::QUIT_APP, PetCommands::SET_SYSTEM,
        PetCommands::SET_GADGET, PetCommands::LIST_GADGETS, PetCommands::SET_DIM_STRENGTH, PetCommands::SAVE_WORKSPACE,
        PetCommands::APPLY_WORKSPACE, PetCommands::LIST_WORKSPACES,
        Self::GADGET_GUIDE,
        Self::WINDOW_DISPLAYS, Self::WINDOW_PLACE, Self::WINDOW_TILE, Self::WINDOW_SPLIT, Self::WINDOW_RATIO, Self::WINDOW_MOVE_DISPLAY,
        Self::WINDOW_ARRANGE_APPS, Self::WINDOW_UNDO, Self::APPSWITCH_SETTINGS, Self::APPSWITCH_SET_SETTINGS,
        Self::DEVICE_STATUS, Self::DEVICE_SET_VOLUME, Self::DEVICE_SET_MIC_MUTED, Self::DIM_STATUS, Self::DIM_PAUSE,
        Self::DIM_RESUME, Self::SHELF_LIST, Self::MENUBAR_STATUS, Self::MENUBAR_LIST_ITEMS, Self::MENUBAR_TIDY, Self::MENUBAR_SET_COLLAPSED,
        Self::MONITOR_STATS, Self::DISK_TOP_ITEMS, Self::DISK_SCAN,
        Self::CLIPBOARD_LIST, Self::CLIPBOARD_GET_ITEM, Self::CLIPBOARD_COPY_AGAIN,
        Self::CAPTURE_LIST_PINNED, Self::AWAKE_STATUS, Self::AWAKE_START, Self::AWAKE_STOP,
        Self::MOUSE_STATUS, Self::MOUSE_SET, Self::CALENDAR_STATUS, Self::CALENDAR_SET,
        Self::DESKTOP_TIDY_PLAN, Self::DESKTOP_TIDY_APPLY, Self::DESKTOP_TIDY_UNDO, Self::DESKTOP_TIDY_HISTORY,
        Self::DESKTOP_ARRANGE_SUGGEST, Self::DESKTOP_ARRANGE_APPLY,
    ];

    /// Declared once the shell can put a file or a picture on the next chat message (W2's chat window).
    pub const WITH_CHAT_HANDOFF: [&'static str; 2] = [Self::SHELF_GIVE, Self::CAPTURE_ASK];

    /// macOS capabilities Windows leaves out on purpose, with the reason (docs/parity-macos.md has the full table).
    pub const LEFT_OUT: [(&'static str, &'static str); 5] = [
        (PetCommands::CLEAR_SHELF, "deletes: empties the shelf and throws its own copies away"),
        ("combo_clipboard_clear", "deletes: the history cannot be brought back"),
        ("combo_shelf_forget", "deletes: takes an item off the shelf"),
        ("combo_capture_close_all", "deletes: closes pinned captures that were never saved"),
        ("combo_menubar_show_hidden", "Windows has no always-hidden tray section; combo_menubar_set_collapsed false shows every icon"),
    ];

    pub fn find(name: &str) -> Option<PetGadgetTool> {
        let legacy = PetCommands::resolve(name);
        Self::LEGACY.iter().chain(Self::TOOLS.iter()).find(|t| Some(t.name) == legacy || t.name == name || t.canonical == name).copied()
    }

    pub fn is_tool(name: &str) -> bool { Self::TOOLS.iter().any(|t| t.name == name) }

    pub fn risk(name: &str) -> Option<PetToolRisk> { Self::find(name).map(|t| t.risk) }

    pub fn gadget(name: &str) -> Option<&'static str> { Self::find(name).map(|t| t.gadget) }

    /// The tools Windows declares, in hello order (`chat_handoff`: with the shelf give / capture ask tools, which
    /// sit at their macOS places).
    pub fn declared(chat_handoff: bool) -> Vec<PetGadgetTool> {
        let mut names: Vec<&str> = Self::WINDOWS.to_vec();
        if chat_handoff {
            let at = names.iter().position(|n| *n == Self::MENUBAR_STATUS).unwrap_or(names.len());
            names.insert(at, Self::SHELF_GIVE);
            let at = names.iter().position(|n| *n == Self::CAPTURE_LIST_PINNED).unwrap_or(names.len());
            names.insert(at, Self::CAPTURE_ASK);
        }
        names.into_iter().filter_map(Self::find).collect()
    }

    /// The declared tools per macOS gadget id, each list in hello order.
    pub fn by_gadget(chat_handoff: bool) -> BTreeMap<&'static str, Vec<PetGadgetTool>> {
        let mut out: BTreeMap<&'static str, Vec<PetGadgetTool>> = BTreeMap::new();
        for t in Self::declared(chat_handoff) { out.entry(t.gadget).or_default().push(t); }
        out
    }

    /// The gadget ids with at least one declared tool, in the order their first tool is declared.
    pub fn gadgets(chat_handoff: bool) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = vec![];
        for t in Self::declared(chat_handoff) { if !out.contains(&t.gadget) { out.push(t.gadget); } }
        out
    }

    /// `"approval": "required"` in hello (macOS `GadgetTool.connectorApproval`, plus [`PetApproval`]).
    pub fn approval_required(name: &str, always_allowed: &BTreeSet<String>) -> bool {
        Self::find(name).is_some_and(|t| t.approval_required(always_allowed.contains(t.gadget)))
    }

    /// Gadgets with at least one tool among `names` that "always allow" can lift (macOS `trustableGadgets`).
    pub fn trustable(names: &[&str]) -> BTreeSet<String> {
        names.iter().filter_map(|n| Self::find(n)).filter(|t| t.is_trustable()).map(|t| t.gadget.to_string()).collect()
    }

    /// The registry name of a command (for the module it needs; the window tools all need Window Snap).
    fn command_tool(command: &PetCommand) -> Option<&'static str> {
        use PetCommand::*;
        Some(match command {
            DeviceStatus => Self::DEVICE_STATUS,
            SetVolume { .. } => Self::DEVICE_SET_VOLUME,
            SetMicMuted(_) => Self::DEVICE_SET_MIC_MUTED,
            DimStatus => Self::DIM_STATUS,
            DimPause => Self::DIM_PAUSE,
            DimResume => Self::DIM_RESUME,
            ShelfList => Self::SHELF_LIST,
            ShelfGive(_) => Self::SHELF_GIVE,
            ClipboardList => Self::CLIPBOARD_LIST,
            ClipboardGetItem(_) => Self::CLIPBOARD_GET_ITEM,
            ClipboardCopyAgain(_) => Self::CLIPBOARD_COPY_AGAIN,
            CaptureAsk(_) => Self::CAPTURE_ASK,
            CaptureListPinned => Self::CAPTURE_LIST_PINNED,
            AwakeStatus => Self::AWAKE_STATUS,
            AwakeStart { .. } => Self::AWAKE_START,
            AwakeStop => Self::AWAKE_STOP,
            GadgetGuide(_) => Self::GADGET_GUIDE,
            Window(_) => Self::WINDOW_PLACE,
            AppSwitchSettings => Self::APPSWITCH_SETTINGS,
            AppSwitchSetSettings(_) => Self::APPSWITCH_SET_SETTINGS,
            TrayStatus => Self::MENUBAR_STATUS,
            TrayListItems => Self::MENUBAR_LIST_ITEMS,
            TrayTidy => Self::MENUBAR_TIDY,
            TraySetCollapsed(_) => Self::MENUBAR_SET_COLLAPSED,
            MonitorStats => Self::MONITOR_STATS,
            DiskTopItems { .. } => Self::DISK_TOP_ITEMS,
            DiskScan(_) => Self::DISK_SCAN,
            MouseStatus => Self::MOUSE_STATUS,
            MouseSet(_) => Self::MOUSE_SET,
            CalendarStatus => Self::CALENDAR_STATUS,
            CalendarSet(_) => Self::CALENDAR_SET,
            Desktop(name, _) => name,
            _ => return None,
        })
    }

    /// The module that must be on for the command to run (macOS `requiresGadgetOn`; the first contract runs anyway).
    pub fn required_module(command: &PetCommand) -> Option<ModuleId> {
        Self::command_tool(command).and_then(Self::find).and_then(|t| t.requires)
    }

    /// Korean gadget names (macOS `GadgetID.koreanName`, Windows names where the gadget differs).
    pub fn korean_name(gadget: &str) -> &'static str {
        // i18n-exempt:begin (sent to the server as tool text, not drawn)
        match gadget {
            "combo" => "나초콤보",
            "window" => "창 배치",
            "menubar" => "트레이 정리",
            "monitor" => "시스템 모니터",
            "disk" => "디스크 지도",
            "clipboard" => "클립보드 기록",
            "shelf" => "파일 선반",
            "capture" => "캡처 플로트",
            "dim" => "집중 디밍",
            "awake" => "잠자기 방지",
            "device" => "기기 제어",
            "mouse" => "마우스 조정",
            "appswitch" => "앱 전환",
            "calendar" => "달력 시계",
            "desktop" => "바탕 화면 정리",
            _ => "가젯",
        }
        // i18n-exempt:end
    }

    pub fn display_name(gadget: &str) -> &'static str {
        match gadget {
            "clipboard" => ModuleId::Clipboard.name(),
            "shelf" => ModuleId::FileShelf.name(),
            "capture" => ModuleId::CaptureFloat.name(),
            "dim" => ModuleId::FocusDim.name(),
            "awake" => ModuleId::KeepAwake.name(),
            "device" => ModuleId::DeviceControl.name(),
            "window" => ModuleId::WindowSnap.name(),
            "appswitch" => ModuleId::AppSwitch.name(),
            "menubar" => ModuleId::TrayOrganizer.name(),
            "monitor" => ModuleId::SystemMonitor.name(),
            "disk" => ModuleId::DiskMap.name(),
            "mouse" => ModuleId::MouseTune.name(),
            "calendar" => ModuleId::CalendarClock.name(),
            "desktop" => "Desktop Tidy",
            _ => "Nacho Combo",
        }
    }

    /// The macOS gadget id of a module (the id "always allow" and `gadget_off` use).
    pub fn gadget_of_module(m: ModuleId) -> Option<&'static str> {
        // Nacho never switches the pet itself, so it has no gadget id.
        Some(match m {
            ModuleId::NachoPet => return None,
            ModuleId::Clipboard => "clipboard",
            ModuleId::FileShelf => "shelf",
            ModuleId::CaptureFloat => "capture",
            ModuleId::FocusDim => "dim",
            ModuleId::KeepAwake => "awake",
            ModuleId::DeviceControl => "device",
            ModuleId::WindowSnap => "window",
            ModuleId::AppSwitch => "appswitch",
            ModuleId::TrayOrganizer => "menubar",
            ModuleId::SystemMonitor => "monitor",
            ModuleId::DiskMap => "disk",
            ModuleId::MouseTune => "mouse",
            ModuleId::CalendarClock => "calendar",
            // Windows only (macOS keeps its widget board as a layout, not a gadget switch).
            ModuleId::WidgetBoard => "board",
        })
    }

    // MARK: descriptions and schemas (macOS schemas, Windows wording)

    pub const MAX_INDEX: i64 = 200;
    pub const MAX_READ_CHARACTERS: usize = 4000;

    fn object(properties: Value, required: &[&str]) -> Value {
        let mut o = json!({"type": "object", "properties": properties, "additionalProperties": false});
        if !required.is_empty() { o["required"] = json!(required); }
        o
    }

    // i18n-exempt:begin (sent to the server as tool text, not drawn)
    fn item_ref() -> Value {
        Self::object(json!({
            "index": {"type": "integer", "description": "목록의 번호(1부터)", "minimum": 1, "maximum": Self::MAX_INDEX},
            "id": {"type": "string", "description": "목록의 id(8자리). 주면 목록이 바뀌었는지 확인해요", "minLength": 8, "maxLength": 8}}), &["index"])
    }

    fn app() -> Value {
        json!({"type": "string", "maxLength": PetCommands::MAX_APP_NAME_LENGTH,
               "description": "앱 이름(한국어 표시 이름·영문 이름·실행 파일 이름), 예: 메모장, Notepad, notepad.exe"})
    }

    fn display_number() -> Value {
        json!({"type": "integer", "minimum": 1, "maximum": 16, "description": "디스플레이 번호(window_displays 기준, 왼쪽부터 1). 없으면 첫 창이 있는 화면"})
    }

    fn window_ref() -> Value {
        Self::object(json!({
            "app": Self::app(),
            "window": {"type": "integer", "minimum": 1, "maximum": 20, "description": "그 앱의 몇 번째 창인지(앞에서부터 1). 기본 1. window_list_windows의 window 번호"},
            "id": {"type": "string", "maxLength": 120, "description": "app 대신 쓰는 창 번호표 \"앱#번호\" (예: notepad.exe#2)"}}), &[])
    }

    fn window_list(max: i64, note: &str) -> Value {
        json!({"type": "array", "minItems": 1, "maxItems": max, "items": Self::window_ref(), "description": note})
    }

    fn plan_id() -> Value { json!({"type": "string", "minLength": 1, "maxLength": 32, "description": "계획 번호(plan_id). 계획을 만든 답에 들어 있어요"}) }

    pub fn spec(name: &str) -> Option<(String, Value)> {
        let none = || Self::object(json!({}), &[]);
        let (d, schema) = match name {
            Self::GADGET_GUIDE => ("가젯 하나의 사용 가이드(할 수 있는 일, 순서, 예시 호출, 제약)를 봐요. 처음 쓰는 가젯이거나 어떤 도구를 어떤 순서로 쓸지 모를 때 먼저 불러요.",
                Self::object(json!({"gadget": {"type": "string", "enum": PetGadgetGuides::GADGETS, "description": "가젯 id, 예: window"}}), &["gadget"])),
            Self::WINDOW_DISPLAYS => ("연결된 디스플레이 목록(번호·이름·전체 크기·창이 놓일 수 있는 작업 영역)을 봐요. 번호는 왼쪽부터 1이에요. \
                여러 디스플레이에 창을 놓기 전에 어느 화면이 몇 번이고 얼마나 큰지 확인할 때 써요.", none()),
            Self::WINDOW_PLACE => ("창마다 원하는 자리(x, y, w, h)를 줘서 한 번에 놓아요. 개수 제한 없이(한 번에 200개까지) 겹쳐도 되고, z로 앞뒤도 정해요. \
                다이아몬드·원형·계단 같은 모양은 직접 좌표를 계산해 써요. 부분 실패는 창별 결과로 알려요.",
                Self::object(json!({
                    "items": {"type": "array", "minItems": 1, "maxItems": 200, "description": "창마다 자리. 겹쳐도 돼요. 목록 순서가 기본 앞뒤 순서(뒤에 있는 항목이 위)",
                        "items": Self::object(json!({
                            "app": Self::app(),
                            "window": {"type": "integer", "minimum": 1, "maximum": 20, "description": "그 앱의 몇 번째 창인지(앞에서부터 1). 기본 1"},
                            "id": {"type": "string", "maxLength": 120, "description": "app 대신 쓰는 창 번호표 \"앱#번호\" (예: notepad.exe#2)"},
                            "display": Self::display_number(),
                            "x": {"type": "number", "description": "왼쪽 가장자리. 작업 영역 왼쪽 위가 0"},
                            "y": {"type": "number", "description": "위쪽 가장자리. 작업 영역 왼쪽 위가 0"},
                            "w": {"type": "number", "description": "너비(0보다 커야 해요)"},
                            "h": {"type": "number", "description": "높이(0보다 커야 해요)"},
                            "z": {"type": "integer", "minimum": -1000, "maximum": 1000, "description": "앞뒤 순서. 클수록 앞. 없으면 목록 순서"}}), &["x", "y", "w", "h"])},
                    "unit": {"type": "string", "enum": ["ratio", "pixel"], "description": "ratio(기본)=디스플레이 작업 영역의 0~1 비율, pixel=왼쪽 위에서부터의 픽셀"}}), &["items"])),
            Self::WINDOW_TILE => ("창을 바둑판으로 깔아요. 가로·세로 칸 수를 마음대로, 채우는 순서, 여러 디스플레이 걸치기·디스플레이별, 간격을 정해요. \
                all_windows가 true면 열린 창 전부를 모든 화면에 깔아요. 개수 제한 없음(200개까지).",
                Self::object(json!({
                    "windows": Self::window_list(200, "깔 창들. 목록 순서대로 채워요. all_windows와 함께 쓰지 않아요"),
                    "all_windows": {"type": "boolean", "description": "true면 열린 창 전부(모든 앱, 최소화 제외)를 깔아요. 전체 바둑판"},
                    "rows": {"type": "integer", "minimum": 1, "maximum": 100, "description": "세로 칸 수. 없으면 창 수로 정해요"},
                    "cols": {"type": "integer", "minimum": 1, "maximum": 100, "description": "가로 칸 수. 없으면 창 수로 정해요"},
                    "display": Self::display_number(),
                    "displays": {"type": "array", "minItems": 1, "maxItems": 16, "items": {"type": "integer", "minimum": 1, "maximum": 16},
                                 "description": "쓸 디스플레이 번호들(window_displays 기준). display와 함께 쓰지 않아요. 없으면 첫 창이 있는 화면"},
                    "span": {"type": "string", "enum": ["per_display", "across"], "description": "per_display(기본)=화면마다 격자, across=모든 화면에 걸친 격자 하나"},
                    "fill": {"type": "string", "enum": ["row_major", "column_major"], "description": "row_major(기본)=가로로 먼저, column_major=세로로 먼저"},
                    "gap": {"type": "integer", "minimum": 0, "maximum": 100, "description": "창 사이·가장자리 간격(픽셀). 없으면 앱 설정"}}), &[])),
            Self::WINDOW_SPLIT => ("창을 2·3·4등분해서 놓아요. horizontal은 나란히(좌→우), vertical은 위아래, quadrant는 4창을 2x2로 놓아요. \
                목록 순서대로 왼쪽/위부터 채워요. 더 많거나 다른 모양은 window_tile·window_place를 써요.",
                Self::object(json!({
                    "parts": {"type": "integer", "minimum": 2, "maximum": 4, "description": "몇 등분인지(2, 3, 4)"},
                    "direction": {"type": "string", "enum": ["horizontal", "vertical", "quadrant"], "description": "horizontal=나란히, vertical=위아래, quadrant=2x2(4등분만)"},
                    "windows": Self::window_list(4, "놓을 창들(등분 수 이하). 순서대로 채워요"),
                    "display": Self::display_number()}), &["parts", "direction", "windows"])),
            Self::WINDOW_RATIO => ("창 2~4개를 지정한 비율로 나눠 놓아요. 예를 들어 비율이 [70,30]이면 왼쪽 70%, 오른쪽 30%예요. 방향은 나란히(horizontal)나 \
                위아래(vertical)예요. 더 복잡한 비율은 자유 배치 도구를 써요.",
                Self::object(json!({
                    "ratios": {"type": "array", "minItems": 2, "maxItems": 4, "items": {"type": "integer", "minimum": 1, "maximum": 100},
                               "description": "칸 비율 목록. [70,30], [1,2,1] 처럼. 합은 상관없고 서로의 비율만 봐요"},
                    "direction": {"type": "string", "enum": ["horizontal", "vertical"], "description": "기본 horizontal"},
                    "windows": Self::window_list(4, "놓을 창들(비율 개수 이하). 순서대로 채워요"),
                    "display": Self::display_number()}), &["ratios", "windows"])),
            Self::WINDOW_MOVE_DISPLAY => ("창들을 다른 디스플레이로 옮겨요. 화면 안의 위치와 크기 비율은 그대로 유지해요. 디스플레이 번호는 window_displays 기준(왼쪽부터 1)이에요.",
                Self::object(json!({
                    "windows": Self::window_list(200, "옮길 창들"),
                    "display": {"type": "integer", "minimum": 1, "maximum": 16, "description": "목적지 디스플레이 번호(왼쪽부터 1)"}}), &["windows", "display"])),
            Self::WINDOW_ARRANGE_APPS => ("앱 묶음(각 앱의 맨 앞 창)을 정해 둔 배치로 한 번에 놓아요. grid=바둑판, columns=나란히, rows=위아래, quadrants=2x2(최대 4개)예요. \
                개수 제한 없음(200개까지). 실행 중인 앱만 옮겨요.",
                Self::object(json!({
                    "apps": {"type": "array", "minItems": 1, "maxItems": 200, "items": Self::app(), "description": "앱 이름들(순서대로 왼쪽 위부터 채워요)"},
                    "arrangement": {"type": "string", "enum": ["grid", "columns", "rows", "quadrants"], "description": "기본 grid"},
                    "display": Self::display_number()}), &["apps"])),
            Self::WINDOW_UNDO => ("직전 창 배치(자유 배치·바둑판·등분·비율·디스플레이 이동·앱 묶음)를 한 번에 되돌려 창을 배치 전 위치와 크기로 복원해요. \
                steps로 최대 5번의 배치까지 되돌려요. 닫힌 창은 건너뛰어요.",
                Self::object(json!({"steps": {"type": "integer", "minimum": 1, "maximum": 5, "description": "몇 번의 배치를 되돌릴지(1~5, 기본 1)"}}), &[])),
            Self::APPSWITCH_SETTINGS => ("앱 전환 가젯의 설정을 봐요: 같은 단축키를 다시 누르면 숨길지, 단축키를 정해 둔 앱 이름 목록. 설정을 바꾸기 전에 확인할 때 써요.", none()),
            Self::APPSWITCH_SET_SETTINGS => ("앱 전환 설정을 바꿔요: hide_on_repeat가 true면 이미 맨 앞인 앱의 단축키를 다시 누를 때 그 앱을 최소화해요. 단축키 자체는 바꾸지 않아요.",
                Self::object(json!({"hide_on_repeat": {"type": "boolean", "description": "같은 단축키를 다시 누르면 숨기기"}}), &["hide_on_repeat"])),
            Self::DEVICE_STATUS => ("지금 소리 출력 장치 이름·소리 크기와 마이크 음소거 여부를 봐요. 소리나 마이크를 바꾸기 전에 확인할 때 써요.", none()),
            Self::DEVICE_SET_VOLUME => ("지금 소리 출력의 크기(0~100)나 음소거(muted)를 바꿔요. 둘 중 하나 이상 넣어요. 장치를 바꾸거나 외장 모니터는 건드리지 않아요.",
                Self::object(json!({
                    "volume": {"type": "integer", "description": "소리 크기 %", "minimum": 0, "maximum": 100},
                    "muted": {"type": "boolean", "description": "음소거(true)/해제(false)"}}), &[])),
            Self::DEVICE_SET_MIC_MUTED => ("마이크를 음소거(true)하거나 켜요(false). 화상 회의 전후에 써요. 이 PC의 모든 마이크에 함께 적용돼요.",
                Self::object(json!({"muted": {"type": "boolean", "description": "true=음소거, false=켜기"}}), &["muted"])),
            Self::DIM_STATUS => ("집중 디밍이 지금 화면을 어둡게 하는지(잠시 멈춤인지), 어둡기와 범위(앞 창만·앱 전체, 다른 화면) 설정을 봐요. 조절하기 전에 확인할 때 써요.", none()),
            Self::DIM_PAUSE => ("집중 디밍을 잠시 멈춰 화면을 원래 밝기로 보여 줘요(가젯은 켜진 채). 다시 어둡게 하려면 dim_resume을 써요.", none()),
            Self::DIM_RESUME => ("멈춰 둔 집중 디밍을 다시 시작해 맨 앞 창만 밝게 두고 나머지를 어둡게 해요.", none()),
            Self::SHELF_LIST => ("파일 선반에 올려 둔 항목 목록을 봐요: 번호·id·이름·종류(파일·그림·글·링크)·크기만 보여 주고 내용은 없어요. 선반에 무엇이 있는지 물을 때 써요.", none()),
            Self::SHELF_GIVE => ("파일 선반의 N번 항목을 나초 대화의 다음 메시지에 첨부해요. 사용자가 보내기를 누르면 나초에게 가요. 파일이 PC 밖으로 나가므로 매번 승인을 받아요.", Self::item_ref()),
            Self::MENUBAR_STATUS => ("트레이 정리 가젯의 지금 상태를 봐요: 알림 영역 아이콘이 접혀 있는지, 다시 접히는 시간, 늘 보이게 둔 아이콘 수. 접기·펼치기 전에 확인할 때 써요.", none()),
            Self::MENUBAR_LIST_ITEMS => ("작업 표시줄 알림 영역에 있는 아이콘의 프로그램 이름과 지금 보이는지·접혀 숨었는지를 읽어요. 아이콘의 툴팁이나 그림은 읽지 않아요.", none()),
            Self::MENUBAR_TIDY => ("지금 알림 영역을 정리해요: 늘 보이게 둔 것 말고 아이콘을 모두 접어 숨겨요. 작업 표시줄 아이콘이 어수선할 때 써요.", none()),
            Self::MENUBAR_SET_COLLAPSED => ("알림 영역 아이콘을 접거나(collapsed=true) 모두 펼쳐요(false). 펼치면 설정한 시간 뒤 다시 접혀요.",
                Self::object(json!({"collapsed": {"type": "boolean", "description": "true=접기, false=펼치기"}}), &["collapsed"])),
            Self::MONITOR_STATS => ("시스템 모니터의 지금 수치를 봐요: CPU·메모리·네트워크 속도·디스크 사용률·배터리·온도(있으면)와 바쁜 프로그램. \
                \"PC가 느려\"처럼 지금 상태를 물을 때 써요. 프로그램 이름이 나가므로 승인을 받아요.", none()),
            Self::DISK_TOP_ITEMS => ("디스크 지도의 마지막 스캔에서 가장 큰 폴더·파일 상위 N개(이름·경로·크기·수정 시각)와 스캔 시각을 봐요. \
                \"뭐가 디스크를 차지해?\"에 써요. 읽기만 하고, 경로가 나가므로 승인을 받아요.",
                Self::object(json!({
                    "count": {"type": "integer", "minimum": 1, "maximum": data_tools::MAX_DISK_ITEMS, "description": "몇 개(1–50, 기본 10)"},
                    "kind": {"type": "string", "enum": data_tools::PetDiskTopKind::ALL, "description": "both=폴더와 파일, folders=폴더만, files=파일만 (기본 both)"}}), &[])),
            Self::DISK_SCAN => ("디스크 지도 창을 열고 드라이브를 스캔하기 시작해요(기본은 사용자 폴더가 있는 드라이브). 몇 분 걸릴 수 있고, 끝나면 top_items로 결과를 봐요. 아무것도 지우지 않아요.",
                Self::object(json!({"path": {"type": "string", "maxLength": 1024, "description": "스캔할 드라이브나 그 안의 폴더: ~, D:, D:\\Games (드라이브 전체를 스캔해요, 기본 ~)"}}), &[])),
            Self::CLIPBOARD_LIST => ("클립보드 기록 목록을 봐요: 번호·id·종류(글·링크·그림·파일)·길이·고정 여부만 보여 주고 내용 미리보기는 없어요. 내용이 필요하면 번호로 get_item을 불러요.", none()),
            Self::CLIPBOARD_GET_ITEM => ("클립보드 기록에서 N번 항목의 내용(글·링크·파일 경로)을 읽어요. 내용이 PC 밖으로 나가므로 매번 사용자 승인을 받아요. 사용자가 그 항목을 보라고 할 때만 써요.", Self::item_ref()),
            Self::CLIPBOARD_COPY_AGAIN => ("클립보드 기록의 N번 항목을 다시 복사해 지금 클립보드에 올려요(목록 맨 위로 와요). \"아까 복사한 거 다시 붙여 넣게 해 줘\"에 써요.", Self::item_ref()),
            Self::CAPTURE_ASK => ("화면을 캡처해 나초 대화의 다음 메시지에 첨부해요. area는 사용자가 영역을 드래그하고, screen은 포인터가 있는 화면 전체예요. 화면 내용이 나가므로 매번 승인을 받아요.",
                Self::object(json!({"mode": {"type": "string", "enum": ["area", "screen"], "description": "area=사용자가 드래그한 영역(기본), screen=포인터가 있는 화면 전체"}}), &[])),
            Self::CAPTURE_LIST_PINNED => ("화면에 떠 있는 고정 캡처 창의 개수와 크기(픽셀)를 봐요. 그림 내용은 보내지 않아요. 고정 캡처가 몇 개 열려 있는지 물을 때 써요.", none()),
            Self::AWAKE_STATUS => ("잠자기 방지가 지금 PC를 깨워 두고 있는지, 남은 시간과 이유(수동·정한 프로그램)를 봐요. 켜기 전에 확인할 때 써요.", none()),
            Self::AWAKE_START => ("PC가 잠들지 않게 지금 붙잡아요. minutes에 분(1~1440)을 넣으면 그 시간만, 0이거나 생략하면 끌 때까지 유지해요. 발표나 긴 작업 전에 써요.",
                Self::object(json!({"minutes": {"type": "integer", "description": "유지할 분. 0=끌 때까지", "minimum": 0, "maximum": 1440}}), &[])),
            Self::AWAKE_STOP => ("수동으로 붙잡은 깨어 있기를 풀어 PC가 평소처럼 잠들 수 있게 해요. 정한 프로그램 규칙은 그대로 따라요.", none()),
            Self::MOUSE_STATUS => ("마우스 조정 가젯의 설정을 봐요: 휠 방향 반대로, 스크롤 속도, 포인터 속도, 포인터 정확도 향상. 바꾸기 전에 확인할 때 써요.", none()),
            Self::MOUSE_SET => ("마우스 조정 설정을 바꿔요(바꿀 항목만). 휠 반대로는 true/false, 스크롤 속도는 0.5·1·1.5·2·3, 포인터 속도는 정해진 값, 정확도 향상은 keep·on·off예요.",
                Self::object(json!({
                    "reverse_scroll": {"type": "boolean", "description": "마우스 휠 방향 반대로"},
                    "speed": {"type": "number", "description": "휠 스크롤 속도 배율: 0.5, 1, 1.5, 2, 3 중 하나"},
                    "pointer_speed": {"type": "integer", "enum": crate::core::mouse::POINTER_CHOICES, "description": "포인터 속도(Windows 1~20 단계). 0=사용자 설정 그대로"},
                    "precision": {"type": "string", "enum": settings_tools::PRECISION_CHOICES, "description": "포인터 정확도 향상: keep=그대로, on=켜기, off=끄기"}}), &[])),
            Self::CALENDAR_STATUS => ("달력 시계가 무엇을 보여 주는지 봐요: 날짜·요일·초·12/24시간 표시 여부와 세계 시계 목록. 일정 내용은 읽지 않아요.", none()),
            Self::CALENDAR_SET => ("달력 시계 표시를 바꿔요(바꿀 항목만): 날짜·요일·초 표시, hours(system·twelve·twentyFour), 세계 시계(시간대 ID 최대 6개).",
                Self::object(json!({
                    "show_date": {"type": "boolean", "description": "날짜 표시"},
                    "show_weekday": {"type": "boolean", "description": "요일 표시"},
                    "show_seconds": {"type": "boolean", "description": "초 표시"},
                    "hours": {"type": "string", "enum": settings_tools::HOURS_CHOICES, "description": "시간 표기"},
                    "world_clocks": {"type": "array", "items": {"type": "string"}, "description": "세계 시계 시간대 ID 목록(예: Asia/Seoul), 최대 6개. 빈 목록이면 모두 지움"}}), &[])),
            Self::DESKTOP_TIDY_PLAN => ("바탕 화면의 파일을 훑어 스크린샷·이미지·문서·압축·설치 파일·기타로 나눈 이동 계획을 만들어요. plan_id와 분류별 개수, 파일 이름(최대 50개)을 줘요. 아무것도 옮기지 않아요.", none()),
            Self::DESKTOP_TIDY_APPLY => ("승인받은 정리 계획(plan_id)을 실행해 바탕 화면 파일을 정리됨 폴더로 옮겨요. 사용자 승인을 매번 받아요. 지우거나 덮어쓰지 않고, tidy_undo로 되돌릴 수 있어요.",
                Self::object(json!({"plan_id": Self::plan_id()}), &["plan_id"])),
            Self::DESKTOP_TIDY_UNDO => ("가장 최근 바탕 화면 정리(또는 tidy_id)를 원래 자리로 되돌려요. 원래 자리에 다른 파일이 있으면 덮어쓰지 않고 \"되돌리지 못함\"으로 알려요. 매번 승인을 받아요.",
                Self::object(json!({"tidy_id": {"type": "string", "minLength": 1, "maxLength": 32, "description": "되돌릴 정리 번호(기본: 가장 최근)"}}), &[])),
            Self::DESKTOP_TIDY_HISTORY => ("30일 안의 바탕 화면 정리 기록(일시, 옮긴 개수, 되돌렸는지)을 봐요. 파일 이름은 담지 않아요.", none()),
            Self::DESKTOP_ARRANGE_SUGGEST => ("열린 창(앱 이름만, 창 제목은 안 봐요)으로 배치안과 plan_id를 만들어요. 아무것도 옮기지 않아요. 사용자가 원하면 arrange_apply로 적용해요.", none()),
            Self::DESKTOP_ARRANGE_APPLY => ("승인받은 배치안(plan_id)을 창 배치 가젯의 앱 묶음 배치로 적용해요. 사용자 승인을 매번 받아요. 되돌리기는 window_undo를 써요.",
                Self::object(json!({"plan_id": Self::plan_id()}), &["plan_id"])),
            _ => return None,
        };
        Some((d.to_string(), schema))
    }
    // i18n-exempt:end

    // MARK: parameters (macOS DataGadgetRunner / tool runners: closed lists, unknown fields refused)

    fn bad(why: impl Into<String>) -> PetCommandProblem { PetCommandProblem::BadParams(why.into()) }

    fn closed(p: &JsonObject, allowed: &[&str]) -> Result<(), PetCommandProblem> {
        let extra: Vec<String> = p.keys().filter(|k| !allowed.contains(&k.as_str())).map(|k| k.chars().take(40).collect()).collect();
        if extra.is_empty() { Ok(()) } else { Err(Self::bad(format!("unknown field {}", extra.join(", ")))) }
    }

    fn present(v: Option<&Value>) -> bool { v.is_some_and(|v| !v.is_null()) }

    fn item(p: &JsonObject) -> Result<PetItemRef, PetCommandProblem> {
        Self::closed(p, &["index", "id"])?;
        let index = PetCommands::integer(p.get("index")).filter(|i| (1..=Self::MAX_INDEX).contains(i))
            .ok_or_else(|| Self::bad(format!("index must be a whole number 1…{}", Self::MAX_INDEX)))?;
        let mut id = None;
        if Self::present(p.get("id")) {
            let s = p.get("id").and_then(Value::as_str).filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit()));
            let Some(s) = s else { return Err(Self::bad("id must be the 8-character id from the list")) };
            id = Some(s.to_ascii_lowercase());
        }
        Ok(PetItemRef { index, id })
    }

    pub fn parse(name: &str, params: Option<&Value>) -> Result<PetCommand, PetCommandProblem> {
        if Self::present(params) && !params.is_some_and(Value::is_object) { return Err(Self::bad("parameters must be an object")); }
        let p = params.and_then(Value::as_object).cloned().unwrap_or_default();
        let none = |c: PetCommand| Self::closed(&p, &[]).map(|_| c);
        if let Some(action) = name.strip_prefix("combo_window_").filter(|a| PetWindowTools::ACTIONS.contains(a)) {
            return PetWindowTools::parse(action, params).map(PetCommand::Window);
        }
        if let Some(t) = Self::TOOLS.iter().find(|t| t.name == name && t.gadget == "desktop") {
            return PetDesktopTools::check(t.action(), params).map(|a| PetCommand::Desktop(t.name, a));
        }
        match name {
            Self::GADGET_GUIDE => {
                Self::closed(&p, &["gadget"])?;
                let g = p.get("gadget").and_then(Value::as_str).map(str::trim).filter(|g| PetGadgetGuides::GADGETS.contains(g));
                g.map(|g| PetCommand::GadgetGuide(g.to_string()))
                    .ok_or_else(|| Self::bad(format!("gadget must be one of {}", PetGadgetGuides::GADGETS.join(", "))))
            }
            Self::APPSWITCH_SETTINGS => none(PetCommand::AppSwitchSettings),
            Self::APPSWITCH_SET_SETTINGS => settings_tools::parse_appswitch_set(params).map(PetCommand::AppSwitchSetSettings),
            Self::MENUBAR_STATUS => none(PetCommand::TrayStatus),
            Self::MENUBAR_LIST_ITEMS => none(PetCommand::TrayListItems),
            Self::MENUBAR_TIDY => none(PetCommand::TrayTidy),
            Self::MENUBAR_SET_COLLAPSED => settings_tools::parse_tray_set_collapsed(params).map(PetCommand::TraySetCollapsed),
            Self::MONITOR_STATS => data_tools::parse_monitor_stats(params).map(|_| PetCommand::MonitorStats),
            Self::DISK_TOP_ITEMS => data_tools::parse_disk_top(params).map(|(count, kind)| PetCommand::DiskTopItems { count, kind }),
            Self::DISK_SCAN => data_tools::parse_disk_scan(params).map(PetCommand::DiskScan),
            Self::MOUSE_STATUS => none(PetCommand::MouseStatus),
            Self::MOUSE_SET => settings_tools::parse_mouse_set(params).map(PetCommand::MouseSet),
            Self::CALENDAR_STATUS => none(PetCommand::CalendarStatus),
            Self::CALENDAR_SET => settings_tools::parse_calendar_set(params, |id| crate::core::clock::zone_offset(id, 0).is_some())
                .map(PetCommand::CalendarSet),
            Self::DEVICE_STATUS => none(PetCommand::DeviceStatus),
            Self::DEVICE_SET_VOLUME => {
                Self::closed(&p, &["volume", "muted"])?;
                let mut volume = None;
                if Self::present(p.get("volume")) {
                    volume = Some(PetCommands::integer(p.get("volume")).filter(|v| (0..=100).contains(v))
                        .ok_or_else(|| Self::bad("volume must be a whole number from 0 to 100"))?);
                }
                let mut muted = None;
                if Self::present(p.get("muted")) { muted = Some(PetCommands::bool(p.get("muted")).ok_or_else(|| Self::bad("muted must be true or false"))?); }
                if volume.is_none() && muted.is_none() { return Err(Self::bad("give volume, muted or both")); }
                Ok(PetCommand::SetVolume { volume, muted })
            }
            Self::DEVICE_SET_MIC_MUTED => {
                Self::closed(&p, &["muted"])?;
                PetCommands::bool(p.get("muted")).map(PetCommand::SetMicMuted).ok_or_else(|| Self::bad("muted must be true or false"))
            }
            Self::DIM_STATUS => none(PetCommand::DimStatus),
            Self::DIM_PAUSE => none(PetCommand::DimPause),
            Self::DIM_RESUME => none(PetCommand::DimResume),
            Self::SHELF_LIST => none(PetCommand::ShelfList),
            Self::SHELF_GIVE => Self::item(&p).map(PetCommand::ShelfGive),
            Self::CLIPBOARD_LIST => none(PetCommand::ClipboardList),
            Self::CLIPBOARD_GET_ITEM => Self::item(&p).map(PetCommand::ClipboardGetItem),
            Self::CLIPBOARD_COPY_AGAIN => Self::item(&p).map(PetCommand::ClipboardCopyAgain),
            Self::CAPTURE_ASK => {
                Self::closed(&p, &["mode"])?;
                if !Self::present(p.get("mode")) { return Ok(PetCommand::CaptureAsk(PetCaptureMode::Area)); }
                match p.get("mode").and_then(Value::as_str) {
                    Some("area") => Ok(PetCommand::CaptureAsk(PetCaptureMode::Area)),
                    Some("screen") => Ok(PetCommand::CaptureAsk(PetCaptureMode::Screen)),
                    _ => Err(Self::bad("mode must be one of area, screen")),
                }
            }
            Self::CAPTURE_LIST_PINNED => none(PetCommand::CaptureListPinned),
            Self::AWAKE_STATUS => none(PetCommand::AwakeStatus),
            Self::AWAKE_START => {
                Self::closed(&p, &["minutes"])?;
                if !Self::present(p.get("minutes")) { return Ok(PetCommand::AwakeStart { minutes: 0 }); }
                PetCommands::integer(p.get("minutes")).filter(|m| (0..=1440).contains(m)).map(|minutes| PetCommand::AwakeStart { minutes })
                    .ok_or_else(|| Self::bad("minutes must be a whole number from 0 to 1440"))
            }
            Self::AWAKE_STOP => none(PetCommand::AwakeStop),
            _ => Err(PetCommandProblem::UnknownCapability(name.chars().take(64).collect())),
        }
    }

    /// The answer of `combo_gadget_guide`: the guide as markdown, naming the tools this PC declares.
    pub fn guide_outcome(gadget: &str, chat_handoff: bool) -> PetCommandOutcome {
        let declared = Self::declared(chat_handoff);
        match PetGadgetGuides::render(gadget, super::gadget_guide::PetGuideExit::Connector, &declared) {
            Some(md) => PetCommandOutcome::with(true, format!("The usage guide of {}.", Self::display_name(gadget)),
                                                json!({"gadget": gadget, "guide": md, "mime_type": "text/markdown"})),
            None => PetCommandOutcome::with(false, format!("There is no guide for {gadget}."), json!({"reason": "unknown_gadget"})),
        }
    }
}

/// One row of a numbered list by number (1-based) and, optionally, its short id from the list (an approval is for one
/// item: with the id, a list that moved in between answers `list_changed` instead of acting on another item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetItemRef {
    pub index: i64,
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetCaptureMode { Area, Screen }

// MARK: shared refusals (macOS GadgetToolRefusal)

impl PetCommandOutcome {
    /// `ok:false` with a shared `reason`, an English `hint` for the model and Korean `guide_ko` for the user.
    pub fn refused(reason: &str, message: &str, hint: &str, guide_ko: &str, extra: Value) -> PetCommandOutcome {
        let mut d = json!({"reason": reason, "hint": hint, "guide_ko": guide_ko});
        if let (Some(o), Some(e)) = (d.as_object_mut(), extra.as_object()) { for (k, v) in e { o.insert(k.clone(), v.clone()); } }
        PetCommandOutcome { ok: false, message: message.into(), gadgets: None, details: d.as_object().cloned() }
    }

    // i18n-exempt:begin (sent to the server as tool text, not drawn)
    pub fn gadget_off(gadget: &str) -> PetCommandOutcome {
        Self::refused("gadget_off", &format!("{} is switched off in Nacho Combo.", PetGadgetTools::display_name(gadget)),
            "ask the user with ask_choice, then call combo_set_gadget",
            &format!("{} 가젯이 꺼져 있어요. 켜 드릴까요?", PetGadgetTools::korean_name(gadget)), json!({"gadget": gadget}))
    }

    pub fn approval_revoked(gadget: &str) -> PetCommandOutcome {
        Self::refused("approval_revoked",
            &format!("The user just turned off 'always allow' for {}, so no approval card ran for this call.", PetGadgetTools::display_name(gadget)),
            "call the tool again; the user will be asked to approve it",
            &format!("{}의 '항상 허용'이 꺼졌어요. 다시 요청하면 승인 카드가 떠요.", PetGadgetTools::korean_name(gadget)), json!({"gadget": gadget}))
    }

    pub fn action_disabled(name: &str) -> PetCommandOutcome {
        Self::refused("action_disabled", "This action is switched off in Nacho Combo by the user.",
            "do not retry; tell the user it is switched off in Nacho Combo", "나초콤보 연결 설정에서 이 행동을 꺼 두었어요.",
            json!({"action": name.chars().take(64).collect::<String>()}))
    }

    pub fn unavailable(name: &str) -> PetCommandOutcome {
        Self::refused("unavailable", &format!("{} is not available in this Nacho Combo build.", name.chars().take(64).collect::<String>()),
            "do not retry; tell the user this action is not available yet", "이 버전의 나초콤보에서는 아직 쓸 수 없는 기능이에요.", json!({}))
    }

    // i18n-exempt:end
    fn with(ok: bool, message: impl Into<String>, details: Value) -> PetCommandOutcome {
        PetCommandOutcome { ok, message: message.into(), gadgets: None, details: details.as_object().cloned() }
    }
}

// MARK: data gadgets (macOS DataGadgetRunner): lists without content, one item's content after approval

/// What a clipboard history item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetClipKind { Text, Link, Image, Files }

impl PetClipKind {
    pub fn raw_value(self) -> &'static str {
        match self { Self::Text => "text", Self::Link => "link", Self::Image => "image", Self::Files => "files" }
    }
}

/// One clipboard history row as the shell sees it. `key` identifies the item across calls (its content); only its
/// hash leaves the PC, as the short id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetClipRow {
    pub key: Vec<u8>,
    pub kind: PetClipKind,
    /// Text, the link, or the file paths one per line; empty for a picture.
    pub text: String,
    /// Picture size in bytes (pictures only).
    pub bytes: usize,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetShelfKind { File, Image, Text, Link }

impl PetShelfKind {
    pub fn raw_value(self) -> &'static str {
        match self { Self::File => "file", Self::Image => "image", Self::Text => "text", Self::Link => "link" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetShelfRow {
    pub key: Vec<u8>,
    /// The file name. Snippets and links are named by kind only (their titles are content).
    pub name: String,
    pub kind: PetShelfKind,
    /// Bytes (files, pictures) or characters (text, links).
    pub size: Option<u64>,
}

pub struct PetDataTools;

impl PetDataTools {
    /// The id a list shows for a row: 8 hex digits of the SHA-256 of its key.
    pub fn short_id(key: &[u8]) -> String { crypto::hex(&crypto::sha256(key))[..8].to_string() }

    /// The row a call names: in range, and the same row the list showed when an id is given.
    pub fn pick(r: &PetItemRef, keys: &[&[u8]], what: &str) -> Result<usize, PetCommandOutcome> {
        let count = keys.len();
        let i = r.index as usize;
        if i < 1 || i > count {
            let message = if count == 0 { format!("There is no {what} {}: the list is empty.", r.index) }
                else { format!("There is no {what} {}; the list has {count}. List again to see the current numbers.", r.index) };
            return Err(PetCommandOutcome::with(false, message, json!({"reason": "out_of_range", "count": count})));
        }
        if let Some(w) = &r.id {
            if Self::short_id(keys[i - 1]) != *w {
                return Err(PetCommandOutcome::with(false,
                    format!("The list changed: {what} {} is not the one with id {w}. List again and use the new number.", r.index),
                    json!({"reason": "list_changed"})));
            }
        }
        Ok(i - 1)
    }

    fn clip_length(r: &PetClipRow) -> (usize, &'static str) {
        match r.kind {
            PetClipKind::Text | PetClipKind::Link => (r.text.chars().count(), "characters"),
            PetClipKind::Files => (r.text.lines().count(), "files"),
            PetClipKind::Image => (r.bytes, "bytes"),
        }
    }

    /// Numbers, kinds, lengths and pins only: no previews, no source app. (Windows keeps no copy time.)
    pub fn clipboard_list(rows: &[PetClipRow]) -> PetCommandOutcome {
        let items: Vec<Value> = rows.iter().take(PetGadgetTools::MAX_INDEX as usize).enumerate().map(|(n, r)| {
            let (length, unit) = Self::clip_length(r);
            json!({"index": n + 1, "id": Self::short_id(&r.key), "kind": r.kind.raw_value(), "length": length, "unit": unit, "pinned": r.pinned})
        }).collect();
        let n = rows.len();
        let message = if n == 0 { "The clipboard history is empty.".to_string() } else {
            format!("{n} clipboard item{}, newest first. The list has no previews; read one by number with clipboard_get_item.", if n == 1 { "" } else { "s" })
        };
        PetCommandOutcome::with(true, message, json!({"items": items}))
    }

    /// The text of one item, cut at 4000 characters. A picture is not text.
    pub fn clipboard_content(index: usize, r: &PetClipRow) -> PetCommandOutcome {
        if r.kind == PetClipKind::Image {
            return PetCommandOutcome::with(false,
                format!("Clipboard item {index} is a picture; it cannot be read as text. Ask the user to drop it on Nacho or put it on the File Shelf."),
                json!({"reason": "not_text", "kind": "image"}));
        }
        let length = r.text.chars().count();
        let truncated = length > PetGadgetTools::MAX_READ_CHARACTERS;
        let shown: String = r.text.chars().take(PetGadgetTools::MAX_READ_CHARACTERS).collect();
        let message = if truncated { format!("Clipboard item {index}, cut at {} of {length} characters.", PetGadgetTools::MAX_READ_CHARACTERS) }
            else { format!("Clipboard item {index}.") };
        PetCommandOutcome::with(true, message, json!({"index": index, "kind": r.kind.raw_value(), "text": shown, "length": length, "truncated": truncated}))
    }

    pub fn clipboard_copied_again(index: usize, kind: PetClipKind) -> PetCommandOutcome {
        PetCommandOutcome::with(true, format!("Copied clipboard item {index} again; it is now on the clipboard and first in the list (the numbers moved)."),
            json!({"index": index, "kind": kind.raw_value()}))
    }

    pub fn shelf_list(rows: &[PetShelfRow]) -> PetCommandOutcome {
        let items: Vec<Value> = rows.iter().take(PetGadgetTools::MAX_INDEX as usize).enumerate().map(|(n, r)| {
            let name = match r.kind {
                PetShelfKind::File | PetShelfKind::Image => r.name.chars().take(200).collect::<String>(),
                PetShelfKind::Text => "text snippet".into(),
                PetShelfKind::Link => "web link".into(),
            };
            let mut row = json!({"index": n + 1, "id": Self::short_id(&r.key), "name": name, "kind": r.kind.raw_value()});
            let unit = if matches!(r.kind, PetShelfKind::File | PetShelfKind::Image) { "bytes" } else { "characters" };
            row[unit] = r.size.map_or(Value::Null, |s| json!(s));
            row
        }).collect();
        let n = rows.len();
        let message = if n == 0 { "The file shelf is empty.".to_string() } else { format!("{n} item{} on the file shelf (names only, no content).", if n == 1 { "" } else { "s" }) };
        PetCommandOutcome::with(true, message, json!({"items": items}))
    }

    pub fn pinned_captures(sizes: &[(u32, u32)]) -> PetCommandOutcome {
        let items: Vec<Value> = sizes.iter().enumerate().map(|(n, (w, h))| json!({"index": n + 1, "width": w, "height": h})).collect();
        let n = sizes.len();
        let message = if n == 0 { "No pinned captures are open.".to_string() } else { format!("{n} pinned capture{} open (sizes only, no pictures).", if n == 1 { "" } else { "s" }) };
        PetCommandOutcome::with(true, message, json!({"items": items}))
    }

    /// Attached to the user's next chat message (shelf give, capture ask).
    pub fn attached(what: &str) -> PetCommandOutcome {
        PetCommandOutcome::with(true, format!("Attached {what} to the user's next chat message. It goes to you when the user presses send; tell them to press send."),
            json!({"attached": true, "delivery": "chat_attachment"}))
    }
}

// MARK: state answers of the simple gadgets

/// Keep Awake as the shell sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PetAwakeState {
    pub manual: bool,
    pub seconds_left: Option<u64>,
    /// A chosen program that keeps the PC awake while it runs.
    pub by_app: Option<String>,
    /// The battery guard let the PC sleep.
    pub guarded: bool,
}

impl PetAwakeState {
    pub fn holding(&self) -> bool { (self.manual || self.by_app.is_some()) && !self.guarded }

    pub fn outcome(&self, lead: Option<&str>) -> PetCommandOutcome {
        let reason = if self.guarded { "battery_guard" } else if self.manual { "manual" } else if self.by_app.is_some() { "app" } else { "none" };
        let state = match (self.holding(), self.seconds_left) {
            (false, _) if self.guarded => "Keep Awake lets the PC sleep: the battery guard is on.".to_string(),
            (false, _) => "Keep Awake is not holding the PC awake.".to_string(),
            (true, Some(s)) => format!("Keep Awake holds the PC awake for {} more minute{}.", s.div_ceil(60), if s.div_ceil(60) == 1 { "" } else { "s" }),
            (true, None) if self.manual => "Keep Awake holds the PC awake until it is stopped.".to_string(),
            (true, None) => "Keep Awake holds the PC awake while a chosen program runs.".to_string(),
        };
        let message = lead.map_or(state.clone(), |l| format!("{l} {state}"));
        let mut d = json!({"holding": self.holding(), "manual": self.manual, "reason": reason});
        if let Some(s) = self.seconds_left { d["seconds_left"] = json!(s); }
        PetCommandOutcome::with(true, message, d)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetDimState {
    pub paused: bool,
    /// 0.0...1.0
    pub strength: f64,
    /// The scope setting: the front app's windows stay bright (false: only the front window).
    pub front_app: bool,
    /// Displays without the front window are dimmed too.
    pub other_displays: bool,
}

impl PetDimState {
    /// macOS `dimResult` fields (`scope` frontWindow / frontApp, `other_displays`); Windows has no "only during a
    /// focus session" option, so `only_in_session` is not sent.
    pub fn outcome(&self, lead: Option<&str>) -> PetCommandOutcome {
        let percent = (self.strength * 100.0).round() as i64;
        let front = if self.front_app { "the front app's windows" } else { "the front window" };
        let state = if self.paused { format!("Focus Dim is paused (strength {percent}%).") } else { format!("Focus Dim is dimming everything but {front} ({percent}%).") };
        let message = lead.map_or(state.clone(), |l| format!("{l} {state}"));
        PetCommandOutcome::with(true, message, json!({"paused": self.paused, "strength_percent": percent,
            "scope": super::settings_tools::dim_scope_raw(self.front_app), "other_displays": self.other_displays}))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetDeviceState {
    pub output: Option<String>,
    /// 0.0...1.0
    pub volume: Option<f64>,
    pub muted: bool,
    pub input: Option<String>,
    pub mic_muted: bool,
}

impl PetDeviceState {
    pub fn outcome(&self) -> PetCommandOutcome {
        let mut d = json!({"mic_muted": self.mic_muted, "muted": self.muted});
        if let Some(o) = &self.output { d["output"] = json!(o); }
        if let Some(v) = self.volume { d["volume"] = json!((v * 100.0).round() as i64); }
        if let Some(i) = &self.input { d["input"] = json!(i); }
        let vol = self.volume.map_or("unknown".to_string(), |v| format!("{}%", (v * 100.0).round() as i64));
        let message = format!("Sound output {}: volume {vol}{}. Microphone {}.", self.output.as_deref().unwrap_or("(none)"),
            if self.muted { ", muted" } else { "" }, if self.mic_muted { "muted" } else { "on" });
        PetCommandOutcome::with(true, message, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: Value) -> Option<Value> { Some(v) }

    #[test]
    fn every_tool_has_a_spec_and_a_closed_schema() {
        for t in PetGadgetTools::TOOLS {
            let (d, s) = PetGadgetTools::spec(t.name).unwrap_or_else(|| panic!("{} has no spec", t.name));
            assert!((30..=200).contains(&d.chars().count()), "{}: {} characters", t.name, d.chars().count());
            assert!(!d.contains('맥'), "{}: macOS wording", t.name);
            assert_eq!(s["type"], "object");
            assert_eq!(s["additionalProperties"], false);
            let prefix = if t.gadget == "combo" { "combo_".to_string() } else { format!("combo_{}_", t.gadget) };
            assert!(t.name.starts_with(&prefix), "{}", t.name);
            assert_eq!(t.name, t.canonical, "{}: new tools are declared by their registry name", t.name);
        }
    }

    #[test]
    fn risk_decides_the_approval() {
        let none = BTreeSet::new();
        let clip = BTreeSet::from(["clipboard".to_string()]);
        assert!(PetGadgetTools::approval_required(PetGadgetTools::CLIPBOARD_GET_ITEM, &none));
        assert!(!PetGadgetTools::approval_required(PetGadgetTools::CLIPBOARD_GET_ITEM, &clip), "always allowed lifts content_out");
        assert!(PetGadgetTools::approval_required(PetGadgetTools::SHELF_GIVE, &clip), "always allow is per gadget");
        assert!(PetGadgetTools::approval_required(PetCommands::QUIT_APP, &clip), "destructive always asks");
        assert!(!PetGadgetTools::approval_required(PetGadgetTools::CLIPBOARD_LIST, &none), "lists carry no content");
        assert!(!PetGadgetTools::approval_required(PetGadgetTools::AWAKE_START, &none));
        assert!(!PetGadgetTools::approval_required("combo_unknown", &none));
        assert_eq!(PetGadgetTools::risk("combo_appswitch_quit"), Some(PetToolRisk::Destructive), "aliases resolve");
        // Windows: System Monitor and Disk Map answers carry program names and paths, so they ask unless always allowed.
        assert_eq!(PetGadgetTools::trustable(&PetGadgetTools::WINDOWS), ["clipboard", "disk", "monitor"].iter().map(|s| s.to_string()).collect());
        let mut with_handoff = PetGadgetTools::WINDOWS.to_vec();
        with_handoff.extend(PetGadgetTools::WITH_CHAT_HANDOFF);
        assert_eq!(PetGadgetTools::trustable(&with_handoff), ["capture", "clipboard", "disk", "monitor", "shelf"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn windows_declares_no_deleting_action() {
        for (name, _) in PetGadgetTools::LEFT_OUT {
            assert!(!PetGadgetTools::WINDOWS.contains(&name), "{name}");
            assert!(!PetGadgetTools::WITH_CHAT_HANDOFF.contains(&name), "{name}");
        }
        for name in PetGadgetTools::WINDOWS {
            for word in ["delete", "remove", "clear", "forget", "close"] { assert!(!name.contains(word), "{name} looks like a deleting action"); }
        }
    }

    #[test]
    fn parses_the_gadget_tools() {
        use PetCommand::*;
        let t = |n: &str, v: Option<Value>| PetGadgetTools::parse(n, v.as_ref());
        assert_eq!(t("combo_device_status", None), Ok(DeviceStatus));
        assert_eq!(t("combo_device_set_volume", p(json!({"volume": 30}))), Ok(SetVolume { volume: Some(30), muted: None }));
        assert_eq!(t("combo_device_set_volume", p(json!({"muted": true}))), Ok(SetVolume { volume: None, muted: Some(true) }));
        assert_eq!(t("combo_device_set_mic_muted", p(json!({"muted": false}))), Ok(SetMicMuted(false)));
        assert_eq!(t("combo_clipboard_get_item", p(json!({"index": 2, "id": "3F2A9C1B"}))),
                   Ok(ClipboardGetItem(PetItemRef { index: 2, id: Some("3f2a9c1b".into()) })));
        assert_eq!(t("combo_clipboard_copy_again", p(json!({"index": 3}))), Ok(ClipboardCopyAgain(PetItemRef { index: 3, id: None })));
        assert_eq!(t("combo_capture_ask", None), Ok(CaptureAsk(PetCaptureMode::Area)));
        assert_eq!(t("combo_capture_ask", p(json!({"mode": "screen"}))), Ok(CaptureAsk(PetCaptureMode::Screen)));
        assert_eq!(t("combo_awake_start", None), Ok(AwakeStart { minutes: 0 }));
        assert_eq!(t("combo_awake_start", p(json!({"minutes": 60.0}))), Ok(AwakeStart { minutes: 60 }));
        assert_eq!(t("combo_dim_pause", p(Value::Null)), Ok(DimPause));
    }

    #[test]
    fn refuses_bad_parameters() {
        let bad = |n: &str, v: Value| assert!(matches!(PetGadgetTools::parse(n, Some(&v)), Err(PetCommandProblem::BadParams(_))), "{n} {v}");
        bad("combo_device_set_volume", json!({}));
        bad("combo_device_set_volume", json!({"volume": 101}));
        bad("combo_device_set_volume", json!({"volume": true}));
        bad("combo_device_set_volume", json!({"muted": 1}));
        bad("combo_device_set_mic_muted", json!({}));
        bad("combo_clipboard_get_item", json!({}));
        bad("combo_clipboard_get_item", json!({"index": 0}));
        bad("combo_clipboard_get_item", json!({"index": 201}));
        bad("combo_clipboard_get_item", json!({"index": 1.5}));
        bad("combo_clipboard_get_item", json!({"index": 1, "id": "xyz"}));
        bad("combo_clipboard_get_item", json!({"index": 1, "id": "zzzzzzzz"}));
        bad("combo_clipboard_get_item", json!({"index": 1, "preview": true}));
        bad("combo_clipboard_list", json!({"all": true}));
        bad("combo_capture_ask", json!({"mode": "window"}));
        bad("combo_awake_start", json!({"minutes": 1441}));
        bad("combo_awake_start", json!({"minutes": -1}));
        bad("combo_dim_status", json!("x"));
    }

    fn clip(text: &str, kind: PetClipKind) -> PetClipRow { PetClipRow { key: text.as_bytes().to_vec(), kind, text: text.into(), bytes: 0, pinned: false } }

    #[test]
    fn clipboard_list_has_no_content() {
        let rows = vec![clip("비밀번호 메모 hunter2", PetClipKind::Text), clip("https://example.com/a", PetClipKind::Link),
                        PetClipRow { key: vec![1, 2, 3], kind: PetClipKind::Image, text: String::new(), bytes: 4096, pinned: true }];
        let o = PetDataTools::clipboard_list(&rows);
        let text = serde_json::to_string(&o.result_json()).unwrap();
        assert!(!text.contains("hunter2") && !text.contains("example.com"), "content leaked into the list: {text}");
        let items = o.details.as_ref().unwrap()["items"].as_array().unwrap().clone();
        assert_eq!(items[0]["index"], 1);
        assert_eq!(items[0]["unit"], "characters");
        assert_eq!(items[0]["length"], 15);
        assert_eq!(items[2]["kind"], "image");
        assert_eq!(items[2]["length"], 4096);
        assert_eq!(items[2]["pinned"], true);
        assert_eq!(items[0]["id"].as_str().unwrap().len(), 8);
        assert_eq!(items[0]["id"], PetDataTools::short_id("비밀번호 메모 hunter2".as_bytes()));
    }

    #[test]
    fn get_item_reads_one_item_cut_at_4000() {
        let long = "가".repeat(4100);
        let o = PetDataTools::clipboard_content(1, &clip(&long, PetClipKind::Text));
        assert!(o.ok);
        let d = o.details.unwrap();
        assert_eq!(d["text"].as_str().unwrap().chars().count(), 4000);
        assert_eq!(d["truncated"], true);
        assert_eq!(d["length"], 4100);
        let pic = PetRowFixtures::image();
        let o = PetDataTools::clipboard_content(2, &pic);
        assert!(!o.ok);
        assert_eq!(o.details.unwrap()["reason"], "not_text");
    }

    struct PetRowFixtures;
    impl PetRowFixtures {
        fn image() -> PetClipRow { PetClipRow { key: vec![9], kind: PetClipKind::Image, text: String::new(), bytes: 10, pinned: false } }
    }

    #[test]
    fn pick_checks_range_and_the_id() {
        let keys: Vec<&[u8]> = vec![b"a", b"b"];
        assert_eq!(PetDataTools::pick(&PetItemRef { index: 2, id: None }, &keys, "clipboard item"), Ok(1));
        let id = PetDataTools::short_id(b"b");
        assert_eq!(PetDataTools::pick(&PetItemRef { index: 2, id: Some(id) }, &keys, "clipboard item"), Ok(1));
        let moved = PetDataTools::pick(&PetItemRef { index: 1, id: Some(PetDataTools::short_id(b"b")) }, &keys, "clipboard item").unwrap_err();
        assert_eq!(moved.details.unwrap()["reason"], "list_changed");
        let out = PetDataTools::pick(&PetItemRef { index: 3, id: None }, &keys, "clipboard item").unwrap_err();
        assert_eq!(out.details.as_ref().unwrap()["reason"], "out_of_range");
        assert_eq!(out.details.unwrap()["count"], 2);
        let empty = PetDataTools::pick(&PetItemRef { index: 1, id: None }, &[], "shelf item").unwrap_err();
        assert!(empty.message.contains("empty"));
    }

    #[test]
    fn shelf_list_names_snippets_and_links_by_kind() {
        let rows = vec![
            PetShelfRow { key: b"1".to_vec(), name: "보고서.pdf".into(), kind: PetShelfKind::File, size: Some(2048) },
            PetShelfRow { key: b"2".to_vec(), name: "회의 메모 내용".into(), kind: PetShelfKind::Text, size: Some(12) },
            PetShelfRow { key: b"3".to_vec(), name: "https://example.com".into(), kind: PetShelfKind::Link, size: None },
        ];
        let d = PetDataTools::shelf_list(&rows).details.unwrap();
        let items = d["items"].as_array().unwrap();
        assert_eq!(items[0]["name"], "보고서.pdf");
        assert_eq!(items[0]["bytes"], 2048);
        assert_eq!(items[1]["name"], "text snippet");
        assert_eq!(items[1]["characters"], 12);
        assert_eq!(items[2]["name"], "web link");
        assert!(items[2]["characters"].is_null());
    }

    #[test]
    fn shared_refusals_carry_reason_hint_and_korean_guide() {
        let o = PetCommandOutcome::gadget_off("clipboard");
        let r = o.result_json();
        assert_eq!(r["ok"], false);
        assert_eq!(r["reason"], "gadget_off");
        assert_eq!(r["gadget"], "clipboard");
        assert_eq!(r["guide_ko"], "클립보드 기록 가젯이 꺼져 있어요. 켜 드릴까요?");
        assert!(r["hint"].as_str().unwrap().contains("combo_set_gadget"));
        assert_eq!(PetCommandOutcome::approval_revoked("clipboard").result_json()["reason"], "approval_revoked");
        assert_eq!(PetCommandOutcome::action_disabled("combo_clipboard_get_item").result_json()["reason"], "action_disabled");
    }

    #[test]
    fn required_module_follows_the_gadget() {
        let r = |c: PetCommand| PetGadgetTools::required_module(&c);
        assert_eq!(r(PetCommand::ClipboardList), Some(ModuleId::Clipboard));
        assert_eq!(r(PetCommand::AwakeStart { minutes: 5 }), Some(ModuleId::KeepAwake));
        assert_eq!(r(PetCommand::DimPause), Some(ModuleId::FocusDim));
        assert_eq!(r(PetCommand::SetDimStrength(50)), None, "the first contract runs with the gadget off");
        assert_eq!(r(PetCommand::ListGadgets), None);
    }

    #[test]
    fn new_tools_need_their_gadget_on() {
        use crate::core::pet::settings_tools::PetMouseChange;
        let r = |name: &str, v: Value| PetGadgetTools::required_module(&PetGadgetTools::parse(name, Some(&v)).unwrap());
        assert_eq!(r(PetGadgetTools::WINDOW_DISPLAYS, json!({})), Some(ModuleId::WindowSnap));
        assert_eq!(r(PetGadgetTools::APPSWITCH_SETTINGS, json!({})), Some(ModuleId::AppSwitch));
        assert_eq!(r(PetGadgetTools::MENUBAR_TIDY, json!({})), Some(ModuleId::TrayOrganizer));
        assert_eq!(r(PetGadgetTools::MONITOR_STATS, json!({})), Some(ModuleId::SystemMonitor));
        assert_eq!(r(PetGadgetTools::DISK_SCAN, json!({"path": "D:"})), Some(ModuleId::DiskMap));
        assert_eq!(r(PetGadgetTools::MOUSE_SET, json!({"speed": 2})), Some(ModuleId::MouseTune));
        assert_eq!(r(PetGadgetTools::CALENDAR_STATUS, json!({})), Some(ModuleId::CalendarClock));
        assert_eq!(r(PetGadgetTools::DESKTOP_TIDY_PLAN, json!({})), None, "desktop tidy has no gadget switch");
        assert_eq!(r(PetGadgetTools::DESKTOP_ARRANGE_APPLY, json!({"plan_id": "ab12"})), Some(ModuleId::WindowSnap), "arranging moves windows");
        assert_eq!(r(PetGadgetTools::GADGET_GUIDE, json!({"gadget": "disk"})), None, "guides answer with the gadget off");
        assert_eq!(PetGadgetTools::required_module(&PetCommand::MouseSet(PetMouseChange::default())), Some(ModuleId::MouseTune));
        // Every tool that needs a module names a gadget that `gadget_off` can explain.
        for t in PetGadgetTools::TOOLS { if let Some(m) = t.requires { assert!(PetGadgetTools::gadget_of_module(m).is_some(), "{}", t.name); } }
    }

    #[test]
    fn parses_the_new_tools() {
        use crate::core::pet::data_tools::PetDiskTopKind;
        let t = |n: &str, v: Value| PetGadgetTools::parse(n, Some(&v));
        assert_eq!(t("combo_gadget_guide", json!({"gadget": "window"})), Ok(PetCommand::GadgetGuide("window".into())));
        assert!(t("combo_gadget_guide", json!({"gadget": "computer"})).is_err(), "no Computer Use guide on Windows");
        assert!(t("combo_gadget_guide", json!({})).is_err());
        assert_eq!(t("combo_appswitch_set_settings", json!({"hide_on_repeat": true})), Ok(PetCommand::AppSwitchSetSettings(true)));
        assert_eq!(t("combo_menubar_set_collapsed", json!({"collapsed": true})), Ok(PetCommand::TraySetCollapsed(true)));
        assert_eq!(t("combo_disk_top_items", json!({"count": 3})), Ok(PetCommand::DiskTopItems { count: 3, kind: PetDiskTopKind::Both }));
        assert_eq!(t("combo_disk_scan", json!({})), Ok(PetCommand::DiskScan(None)));
        assert!(matches!(t("combo_desktop_tidy_apply", json!({"plan_id": "a1b2c3d4"})), Ok(PetCommand::Desktop(PetGadgetTools::DESKTOP_TIDY_APPLY, _))));
        assert!(t("combo_desktop_tidy_apply", json!({})).is_err(), "plan_id is required");
        assert!(t("combo_desktop_tidy_plan", json!({"all": true})).is_err());
        assert!(matches!(t("combo_window_displays", json!({})), Ok(PetCommand::Window(_))));
        assert!(t("combo_menubar_show_hidden", json!({})).is_err(), "not a Windows tool");
        // Aliases and registry names of the first capabilities still reach them.
        assert_eq!(PetGadgetTools::find("combo_window_arrange").map(|t| t.name), Some(PetCommands::ARRANGE_WINDOW));
    }

    #[test]
    fn registry_api_for_the_connection_tab_and_the_gadget_mcp() {
        let declared = PetGadgetTools::declared(false);
        assert_eq!(declared.len(), PetGadgetTools::WINDOWS.len());
        assert_eq!(declared.iter().map(|t| t.name).collect::<Vec<_>>(), PetGadgetTools::WINDOWS.to_vec(), "hello order");
        let with = PetGadgetTools::declared(true);
        assert_eq!(with.len(), PetGadgetTools::WINDOWS.len() + 2);
        let groups = PetGadgetTools::by_gadget(false);
        assert_eq!(groups["window"].iter().map(|t| t.action()).collect::<Vec<_>>(),
                   ["arrange", "layout", "list_windows", "displays", "place", "tile", "split", "ratio", "move_display", "arrange_apps", "undo"]);
        assert_eq!(groups["desktop"].len(), 6);
        assert_eq!(groups.values().map(Vec::len).sum::<usize>(), declared.len());
        assert_eq!(PetGadgetTools::gadgets(false).first(), Some(&"window"));
        assert!(!PetGadgetTools::gadgets(false).contains(&"shelf") || groups["shelf"].iter().all(|t| t.name != PetGadgetTools::SHELF_GIVE));
        // MCP: unique names, and nothing that needs the chat (approval card, next-message attachment).
        let mcp: Vec<String> = declared.iter().filter(|t| t.on_mcp()).map(|t| t.mcp_name()).collect();
        assert_eq!(mcp.iter().collect::<BTreeSet<_>>().len(), mcp.len(), "MCP names are unique");
        for n in ["desktop_tidy_apply", "desktop_tidy_undo", "desktop_arrange_apply", "combo_gadget_guide", "shelf_give", "capture_ask"] {
            assert!(!with.iter().any(|t| t.on_mcp() && t.mcp_name() == n), "{n} needs the chat");
        }
        assert!(mcp.contains(&"window_arrange".to_string()) && mcp.contains(&"window_place".to_string()));
        for n in &mcp { assert!(n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{n}"); }
        // Risk and approval are readable per row.
        let undo = PetGadgetTools::find(PetGadgetTools::DESKTOP_TIDY_UNDO).unwrap();
        assert_eq!(undo.risk, PetToolRisk::Change);
        assert!(undo.approval_required(true), "desktop tidy asks every time");
        let stats = PetGadgetTools::find(PetGadgetTools::MONITOR_STATS).unwrap();
        assert!(stats.approval_required(false) && !stats.approval_required(true));
    }

    #[test]
    fn guide_outcome_names_declared_tools() {
        let o = PetGadgetTools::guide_outcome("desktop", false);
        assert!(o.ok);
        let md = o.details.as_ref().unwrap()["guide"].as_str().unwrap().to_string();
        assert!(md.contains("combo_desktop_tidy_apply") && md.contains("## 제약·권한"), "{md}");
        assert!(!PetGadgetTools::guide_outcome("computer", false).ok);
    }

    #[test]
    fn state_answers() {
        let s = PetAwakeState { manual: true, seconds_left: Some(90), ..Default::default() };
        let o = s.outcome(None);
        assert_eq!(o.details.as_ref().unwrap()["holding"], true);
        assert_eq!(o.details.as_ref().unwrap()["seconds_left"], 90);
        assert!(o.message.contains("2 more minutes"), "{}", o.message);
        let guarded = PetAwakeState { manual: true, guarded: true, ..Default::default() }.outcome(None);
        assert_eq!(guarded.details.unwrap()["reason"], "battery_guard");
        let d = PetDimState { paused: true, strength: 0.6, ..Default::default() }.outcome(Some("Paused.")).details.unwrap();
        assert_eq!(d["strength_percent"], 60);
        assert_eq!(d["scope"], "frontWindow");
        let d = PetDimState { paused: false, strength: 0.5, front_app: true, other_displays: true }.outcome(None).details.unwrap();
        assert_eq!(d["scope"], "frontApp", "the macOS raw value");
        assert_eq!(d["other_displays"], true);
        let dev = PetDeviceState { output: Some("Speakers".into()), volume: Some(0.3), muted: false, input: None, mic_muted: true }.outcome();
        assert_eq!(dev.details.as_ref().unwrap()["volume"], 30);
        assert_eq!(dev.details.unwrap()["mic_muted"], true);
    }
}

/// The declaration is fixed against the macOS app's own output (`fixtures/mac-capabilities.json`: what macOS
/// `PetCommands.capabilities` and `GadgetToolRegistry.all` print at macOS `main` a2e4d76). Every place Windows differs
/// is listed here with its reason; anything else must match.
#[cfg(test)]
mod mac_contract {
    use super::*;
    use std::collections::BTreeMap;

    fn mac() -> Value { serde_json::from_str(include_str!("fixtures/mac-capabilities.json")).unwrap() }

    /// Types, enums, bounds, `required` and `additionalProperties` stay; the wording (`description`) is per platform.
    fn shape(v: &Value) -> Value {
        match v {
            Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| k.as_str() != "description").map(|(k, v)| (k.clone(), shape(v))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(shape).collect()),
            v => v.clone(),
        }
    }

    fn by_name(list: &[Value]) -> BTreeMap<String, Value> {
        list.iter().map(|c| (c["name"].as_str().unwrap().to_string(), c.clone())).collect()
    }

    fn catalog() -> Vec<&'static str> {
        let mut v = PetGadgetTools::WINDOWS.to_vec();
        v.extend(PetGadgetTools::WITH_CHAT_HANDOFF);
        v.push(PetCommands::CLEAR_SHELF);
        v
    }

    /// Windows asks where macOS does not: System Monitor and Disk Map answers carry program names and paths (unless
    /// the gadget is always allowed); undoing a desktop tidy moves files again (every time).
    const STRICTER: [&str; 4] = [PetGadgetTools::MONITOR_STATS, PetGadgetTools::DISK_TOP_ITEMS, PetGadgetTools::DISK_SCAN,
                                 PetGadgetTools::DESKTOP_TIDY_UNDO];

    fn keys(v: &Value) -> BTreeSet<String> { v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default() }

    #[test]
    fn input_schemas_match_the_macos_declaration() {
        let mac = by_name(mac()["capabilities"].as_array().unwrap());
        for name in catalog() {
            let m = mac.get(name).unwrap_or_else(|| panic!("{name} is not a macOS capability"));
            let (_, ours) = PetCommands::spec(name).unwrap();
            let (mut theirs, mut ours) = (shape(&m["input_schema"]), shape(&ours));
            match name {
                PetCommands::SET_GADGET => {
                    // The gadget list is each platform's own; macOS `menuBar` is Tray Organizer here (accepted as an alias).
                    let set = |v: &mut Value| -> BTreeSet<String> {
                        let e = v["properties"]["gadget"]["enum"].take();
                        e.as_array().unwrap().iter().map(|x| x.as_str().unwrap().replace("menuBar", "trayOrganizer")).collect()
                    };
                    // Windows-only gadgets: Widget Board (macOS keeps its widget board as a layout, not a switch).
                    let windows_only: BTreeSet<String> = ["widgetBoard".to_string()].into();
                    let ours_set: BTreeSet<String> = set(&mut ours).difference(&windows_only).cloned().collect();
                    assert_eq!(ours_set, set(&mut theirs), "set_gadget gadgets");
                }
                PetGadgetTools::GADGET_GUIDE => {
                    // No Computer Use guide on Windows yet (the gadget comes later); the order is macOS's.
                    let e = theirs["properties"]["gadget"]["enum"].as_array_mut().unwrap();
                    e.retain(|g| g != "computer");
                }
                PetGadgetTools::MOUSE_SET => {
                    // Windows Mouse Tune: no smooth scroll, side or middle button options; pointer speed and precision instead.
                    let (t, o) = (theirs["properties"].as_object_mut().unwrap(), ours["properties"].as_object_mut().unwrap());
                    for k in ["smooth_scroll", "side_buttons", "middle_button"] { assert!(t.remove(k).is_some(), "{k}"); assert!(!o.contains_key(k)); }
                    assert_eq!(o.remove("pointer_speed"), Some(json!({"type": "integer", "enum": crate::core::mouse::POINTER_CHOICES})));
                    assert_eq!(o.remove("precision"), Some(json!({"type": "string", "enum": ["keep", "on", "off"]})));
                }
                PetGadgetTools::CALENDAR_SET => {
                    // The Windows clock reads no calendar: no next-event line.
                    assert!(theirs["properties"].as_object_mut().unwrap().remove("show_next_event").is_some());
                }
                _ => {}
            }
            assert_eq!(ours, theirs, "{name}: input schema differs from macOS");
        }
    }

    #[test]
    fn approvals_match_the_macos_declaration_with_and_without_always_allow() {
        let m = mac();
        let mac_plain = by_name(m["capabilities"].as_array().unwrap());
        let mac_trusted = m["approval_when_always_allowed"].as_object().unwrap().clone();
        let none = BTreeSet::new();
        let all: BTreeSet<String> = PetGadgetTools::trustable(&catalog());
        for name in catalog() {
            let theirs = mac_plain[name].get("approval").and_then(Value::as_str) == Some("required");
            let ours = PetGadgetTools::approval_required(name, &none);
            assert!(ours || !theirs, "{name}: Windows must never ask less than macOS");
            if !STRICTER.contains(&name) { assert_eq!(ours, theirs, "{name}: approval (always allow off)"); } else { assert!(ours, "{name}"); }
            let theirs = mac_trusted[name].as_str() == Some("required");
            let ours = PetGadgetTools::approval_required(name, &all);
            assert!(ours || !theirs, "{name}: Windows must never ask less than macOS (always allow on)");
            match name {
                PetGadgetTools::DESKTOP_TIDY_UNDO => assert!(ours, "undo asks even when always allowed"),
                n if STRICTER.contains(&n) => assert!(!ours, "{name}: always allow lifts it"),
                _ => assert_eq!(ours, theirs, "{name}: approval (always allow on)"),
            }
        }
        let decl = by_name(&PetCommands::windows_declaration(&none, &none, true));
        for (name, c) in &decl {
            if !STRICTER.contains(&name.as_str()) { assert_eq!(c.get("approval"), mac_plain[name].get("approval"), "{name}: hello approval field"); }
            assert_eq!(keys(c).difference(&["approval".to_string()].into()).cloned().collect::<BTreeSet<_>>(),
                       keys(&mac_plain[name]).difference(&["approval".to_string()].into()).cloned().collect::<BTreeSet<_>>(), "{name}: hello fields");
        }
        let mut trustable: BTreeSet<String> = m["trustable_gadgets"].as_array().unwrap().iter().map(|g| g.as_str().unwrap().to_string())
            .filter(|g| g != "computer").collect(); // Computer Use is W5d
        trustable.extend(["disk".to_string(), "monitor".to_string()]);
        assert_eq!(PetGadgetTools::trustable(&catalog()), trustable);
    }

    #[test]
    fn registry_facts_match_the_macos_registry() {
        let reg = mac()["registry"].as_array().unwrap().clone();
        for t in PetGadgetTools::LEGACY.iter().chain(PetGadgetTools::TOOLS.iter()) {
            let m = reg.iter().find(|r| r["connector_name"] == t.name).unwrap_or_else(|| panic!("{} not in the macOS registry", t.name));
            assert_eq!(m["gadget"], t.gadget, "{}", t.name);
            assert_eq!(m["action"], t.action(), "{}", t.name);
            assert_eq!(m["mcp_name"], t.mcp_name().as_str(), "{}", t.name);
            assert_eq!(m["risk"], t.risk.raw_value(), "{}", t.name);
            assert!(m["aliases"].as_array().unwrap().contains(&json!(t.canonical)), "{}: canonical name", t.name);
            assert_eq!(m["exits"].as_array().unwrap().contains(&json!("mcp")), t.on_mcp(), "{}: MCP exit", t.name);
            assert_eq!(m["always_requires_approval"] == true, t.approval == PetApproval::Always && t.name != PetGadgetTools::DESKTOP_TIDY_UNDO, "{}", t.name);
            // `requires_gadget_on` with a module behind the gadget is a module requirement; desktop tidy has no module on
            // macOS, and Windows' arrange apply needs Window Snap because it moves windows through it.
            let mac_needs_module = m["requires_gadget_on"] == true && t.gadget != "desktop" && t.gadget != "combo";
            if t.name != PetGadgetTools::DESKTOP_ARRANGE_APPLY { assert_eq!(mac_needs_module, t.requires.is_some(), "{}: gadget must be on", t.name); }
        }
    }

    #[test]
    fn every_macos_capability_is_declared_left_out_or_later() {
        let ours = catalog();
        for c in mac()["capabilities"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let left_out = PetGadgetTools::LEFT_OUT.iter().any(|(n, _)| *n == name);
            let later = name.starts_with("combo_computer_"); // Computer Use (W5d)
            assert!(ours.contains(&name) || left_out || later, "{name}: neither declared nor left out with a reason");
            assert!(!(ours.contains(&name) && left_out && name != PetCommands::CLEAR_SHELF), "{name}: declared and left out");
        }
    }

    #[test]
    fn hello_order_follows_macos() {
        let mac: Vec<String> = mac()["capabilities"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
        let ours: Vec<String> = PetCommands::windows_declaration(&BTreeSet::new(), &BTreeSet::new(), true).iter()
            .map(|c| c["name"].as_str().unwrap().to_string()).collect();
        let positions: Vec<usize> = ours.iter().map(|n| mac.iter().position(|m| m == n).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "Windows hello order is not the macOS order: {ours:?}");
        assert_eq!(ours.len(), PetGadgetTools::WINDOWS.len() + 2);
    }

    #[test]
    fn windows_descriptions_are_short_korean_and_say_pc() {
        for c in PetCommands::windows_declaration(&BTreeSet::new(), &BTreeSet::new(), true) {
            let name = c["name"].as_str().unwrap();
            let d = c["description"].as_str().unwrap();
            assert!((30..=200).contains(&d.chars().count()), "{name}: {} characters", d.chars().count());
            for mac_word in ["맥", "Finder", "⌘", "손쉬운 사용", "번들", "메뉴 막대"] { assert!(!d.contains(mac_word), "{name}: macOS wording: {d}"); }
            let hangul = d.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count();
            assert!(hangul * 2 > d.chars().filter(|c| c.is_alphabetic()).count(), "{name}");
            // nacho-server classifies new names with these words as payment, deletion or messaging.
            for word in ["pay", "purchase", "checkout", "buy", "call", "delete", "remove", "publish", "book", "reserve", "send", "post"] {
                assert!(PetCommands::LEGACY_NAMES.contains(&name) || !name.contains(word), "{name} contains {word}");
            }
        }
    }

    #[test]
    fn macos_gadget_names_reach_the_windows_gadgets() {
        assert_eq!(PetCommands::gadget("menuBar"), Some(crate::core::pet::commands::PetGadget::Module(ModuleId::TrayOrganizer)));
        assert_eq!(PetCommands::parse("combo_set_gadget", Some(&json!({"gadget": "menuBar", "on": true}))),
                   Ok(PetCommand::SetGadget(ModuleId::TrayOrganizer, true)));
    }
}

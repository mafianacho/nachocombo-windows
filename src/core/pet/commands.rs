//! What Nacho may do on this PC when asked in the chat. Port of `PetCommands.swift`. Nacho Combo registers these as
//! connector capabilities on nacho-server's `/ws/connector`: the server offers them to the model as tools and forwards
//! a call as `{"type":"request","id","capability","params"}`. Every call is validated here against closed lists before
//! the shell touches anything; anything outside them is refused with a reason the model can relay.
//!
//! Contract with nacho-server (change it there first):
//! - names are `combo_*`; descriptions are Korean, at most 200 characters, saying what the tool does and when to use
//!   it (the server's tool picker chooses by these descriptions);
//! - a risky capability carries `"approval": "required"` in hello and the server shows an approval card before it
//!   runs: `combo_quit_app` and `combo_clear_shelf` (destructive).
//!
//! Scope of this port: the 16 capabilities of the first connector contract with the same names and input schemas as
//! the macOS app. The macOS app now builds them (and many more tools) from its gadget tool registry; that registry is
//! not ported, so the specs are written out here and the registry's `combo_<gadget>_<action>` names are accepted as
//! aliases. Descriptions use Windows wording (PC instead of 맥, exe names instead of bundle ids, no Accessibility
//! permission); the schemas (types, enums, bounds, required fields) are the macOS ones.
// i18n-exempt:begin (the capabilities' descriptions are read by the server's tool picker, not drawn)
use super::desktop::{DesktopWorkspaceStore, PetAppMatcher, PetSystemChange, PetWindowPlacement};
use super::gadget_tools::{PetCaptureMode, PetGadgetTools, PetItemRef};
use super::json::JsonObject;
use crate::core::layout::SnapAction;
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetCommand {
    /// Snap the front window (or the front window of the named running app) to a position.
    ArrangeWindow(SnapAction, Option<String>),
    /// Switch a gadget on or off. Never Computer Use or the pet itself.
    SetGadget(ModuleId, bool),
    ListGadgets,
    /// Running apps (and, when asked, installed ones).
    ListApps { installed: bool },
    OpenApp(String),
    FocusApp(String),
    HideApp(String),
    /// Risky: only when declared (approval card on the server).
    QuitApp(String),
    ListWindows,
    LayoutWindows(Vec<PetWindowPlacement>),
    SetSystem(PetSystemChange),
    /// Focus Dim strength in percent (10...90).
    SetDimStrength(i64),
    /// Empties the File Shelf (undoable in the shelf; original files are never touched).
    ClearShelf,
    SaveWorkspace(String),
    ApplyWorkspace(String),
    ListWorkspaces,
    // Gadget tools (`gadget_tools.rs`, macOS registry names `combo_<gadget>_<action>`).
    DeviceStatus,
    /// Volume 0...100 and/or mute of the current sound output.
    SetVolume { volume: Option<i64>, muted: Option<bool> },
    SetMicMuted(bool),
    DimStatus,
    DimPause,
    DimResume,
    ShelfList,
    /// content_out: the item goes on the next chat message.
    ShelfGive(PetItemRef),
    ClipboardList,
    /// content_out: the item's text goes to the server.
    ClipboardGetItem(PetItemRef),
    ClipboardCopyAgain(PetItemRef),
    /// content_out: a capture goes on the next chat message.
    CaptureAsk(PetCaptureMode),
    CaptureListPinned,
    AwakeStatus,
    /// 0 = until stopped.
    AwakeStart { minutes: i64 },
    AwakeStop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetCommandProblem {
    UnknownCapability(String),
    BadParams(String),
    NotAllowed(String),
}

impl PetCommandProblem {
    /// English for the model (tool results are read by the model, not shown to the user directly).
    pub fn message(&self) -> String {
        match self {
            Self::UnknownCapability(n) => format!("unknown capability {n}"),
            Self::BadParams(why) => format!("bad parameters: {why}"),
            Self::NotAllowed(why) => format!("not allowed: {why}"),
        }
    }
}

/// What the shell answers. `ok == false` is still a normal answer (the model tells the user what went wrong).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetCommandOutcome {
    pub ok: bool,
    pub message: String,
    pub gadgets: Option<BTreeMap<String, String>>,
    /// Structured extras (apps, windows, workspaces …).
    pub details: Option<JsonObject>,
}

impl PetCommandOutcome {
    pub fn new(ok: bool, message: impl Into<String>) -> Self { Self { ok, message: message.into(), gadgets: None, details: None } }

    /// The `result` of a response frame. Details never override `ok`, `message` or `gadgets`.
    pub fn result_json(&self) -> JsonObject {
        let mut o = JsonObject::new();
        o.insert("ok".into(), Value::Bool(self.ok));
        o.insert("message".into(), Value::String(self.message.clone()));
        if let Some(g) = &self.gadgets { o.insert("gadgets".into(), json!(g)); }
        if let Some(d) = &self.details {
            for (k, v) in d { o.entry(k.clone()).or_insert_with(|| v.clone()); }
        }
        o
    }
}

/// A gadget as Nacho may name it: a module of this app, or one of the two the pet never switches (they exist in the
/// macOS app; on Windows they are only named so a request for them is refused with a reason).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PetGadget {
    Module(ModuleId),
    NachoPet,
    ComputerUse,
}

impl PetGadget {
    pub fn all() -> Vec<PetGadget> {
        // The pet is a gadget of this app too (`ModuleId::NachoPet`), but for Nacho it is `PetGadget::NachoPet`: never a module it may switch.
        ModuleId::ALL.iter().filter(|m| **m != ModuleId::NachoPet).map(|m| PetGadget::Module(*m)).chain([PetGadget::NachoPet, PetGadget::ComputerUse]).collect()
    }

    /// `windowSnap`, `nachoPet` … (the settings file's camelCase names of the modules).
    pub fn raw_value(self) -> String {
        match self {
            Self::Module(m) => serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            Self::NachoPet => "nachoPet".into(),
            Self::ComputerUse => "computerUse".into(),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Module(m) => m.name(),
            Self::NachoPet => "Nacho Pet",
            Self::ComputerUse => "Computer Use",
        }
    }

    pub fn from_raw(raw: &str) -> Option<PetGadget> { Self::all().into_iter().find(|g| g.raw_value() == raw) }
}

pub struct PetCommands;

impl PetCommands {
    /// Windows: snap the foreground window (or the front window of the named app) with Window Snap's positions.
    pub const ARRANGE_WINDOW: &'static str = "combo_arrange_window";
    /// Windows: switch one of this app's gadgets on or off.
    pub const SET_GADGET: &'static str = "combo_set_gadget";
    /// Windows: this app's gadgets and whether each one is on.
    pub const LIST_GADGETS: &'static str = "combo_list_gadgets";
    /// Windows: running apps (top-level windows by process; exe name in place of the bundle id), optionally the
    /// installed ones (Start menu shortcuts).
    pub const LIST_APPS: &'static str = "combo_list_apps";
    /// Windows: start an installed app (Start menu shortcut / exe) or bring it to the front when it runs.
    pub const OPEN_APP: &'static str = "combo_open_app";
    /// Windows: bring a running app's window to the foreground (restore it when minimized).
    pub const FOCUS_APP: &'static str = "combo_focus_app";
    /// Windows: minimize every window of a running app (macOS hides it with ⌘H; Windows has no hide).
    pub const HIDE_APP: &'static str = "combo_hide_app";
    /// Windows: ask a running app to close (`WM_CLOSE` to its windows). Risky: approval card.
    pub const QUIT_APP: &'static str = "combo_quit_app";
    /// Windows: monitors (number, size) and every app's top-level windows (frame, monitor number), no titles.
    pub const LIST_WINDOWS: &'static str = "combo_list_windows";
    /// Windows: place windows of several apps at once in Window Snap positions.
    pub const LAYOUT_WINDOWS: &'static str = "combo_layout_windows";
    /// Windows: master volume, display brightness (laptop panel; external monitors over DDC/CI where supported) and
    /// Do Not Disturb (Windows notifications off).
    pub const SET_SYSTEM: &'static str = "combo_set_system";
    /// Windows: Focus Dim strength.
    pub const SET_DIM_STRENGTH: &'static str = "combo_set_dim_strength";
    /// Windows: empty the File Shelf (original files untouched). Risky: approval card.
    pub const CLEAR_SHELF: &'static str = "combo_clear_shelf";
    /// Windows: save running apps, window frames and gadget switches as a named workspace.
    pub const SAVE_WORKSPACE: &'static str = "combo_workspace_save";
    /// Windows: put a saved workspace back (open missing apps, move windows, switch gadgets; never closes apps).
    pub const APPLY_WORKSPACE: &'static str = "combo_workspace_apply";
    /// Windows: the saved workspaces.
    pub const LIST_WORKSPACES: &'static str = "combo_workspace_list";

    /// On: the server shows an approval card for `"approval": "required"` before a call runs. While off,
    /// `combo_quit_app` is neither declared nor accepted.
    pub const DECLARES_QUIT_APP: bool = true;

    /// Capabilities that need the user's approval card on the server before they run (destructive ones).
    pub const APPROVAL_REQUIRED: [&'static str; 2] = [Self::QUIT_APP, Self::CLEAR_SHELF];

    /// The 16 capabilities of the first connector contract, in their hello order.
    pub const LEGACY_NAMES: [&'static str; 16] = [
        Self::ARRANGE_WINDOW, Self::LAYOUT_WINDOWS, Self::LIST_WINDOWS, Self::LIST_APPS, Self::OPEN_APP, Self::FOCUS_APP,
        Self::HIDE_APP, Self::QUIT_APP, Self::SET_SYSTEM, Self::SET_GADGET, Self::LIST_GADGETS, Self::SET_DIM_STRENGTH,
        Self::CLEAR_SHELF, Self::SAVE_WORKSPACE, Self::APPLY_WORKSPACE, Self::LIST_WORKSPACES,
    ];

    /// The macOS registry's canonical `combo_<gadget>_<action>` names, accepted as aliases of the legacy ones.
    pub const ALIASES: [(&'static str, &'static str); 11] = [
        ("combo_window_arrange", Self::ARRANGE_WINDOW),
        ("combo_window_layout", Self::LAYOUT_WINDOWS),
        ("combo_window_list_windows", Self::LIST_WINDOWS),
        ("combo_appswitch_list", Self::LIST_APPS),
        ("combo_appswitch_open", Self::OPEN_APP),
        ("combo_appswitch_focus", Self::FOCUS_APP),
        ("combo_appswitch_hide", Self::HIDE_APP),
        ("combo_appswitch_quit", Self::QUIT_APP),
        ("combo_device_set", Self::SET_SYSTEM),
        ("combo_dim_set_strength", Self::SET_DIM_STRENGTH),
        ("combo_shelf_clear", Self::CLEAR_SHELF),
    ];

    /// Every capability this build knows, in hello order.
    pub fn all_names() -> Vec<&'static str> { Self::LEGACY_NAMES.to_vec() }

    /// What hello declares with the default switches.
    pub fn names() -> Vec<&'static str> { Self::names_allowing(Self::DECLARES_QUIT_APP) }

    pub fn names_allowing(allow_quit: bool) -> Vec<&'static str> {
        Self::LEGACY_NAMES.iter().copied().filter(|n| *n != Self::QUIT_APP || allow_quit).collect()
    }

    /// The legacy name for a declared name or an alias; a gadget tool's own name.
    pub fn resolve(name: &str) -> Option<&'static str> {
        if let Some(n) = Self::LEGACY_NAMES.iter().find(|n| **n == name) { return Some(n); }
        if let Some(t) = PetGadgetTools::TOOLS.iter().find(|t| t.name == name) { return Some(t.name); }
        Self::ALIASES.iter().find(|(a, _)| *a == name).map(|(_, l)| *l)
    }

    /// Every name a request may use for this capability.
    pub fn connector_names(legacy: &str) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Self::LEGACY_NAMES.iter().copied().filter(|n| *n == legacy).collect();
        v.extend(Self::ALIASES.iter().filter(|(_, l)| *l == legacy).map(|(a, _)| *a));
        v
    }

    /// Every window position Window Snap offers.
    pub fn positions() -> Vec<SnapAction> { SnapAction::ALL.to_vec() }

    /// Positions for one window of a multi-window layout (display moves are the `display` field there; restore and
    /// smaller / larger depend on history and make no sense in a layout).
    pub fn layout_positions() -> Vec<SnapAction> {
        use SnapAction::*;
        SnapAction::ALL.into_iter().filter(|a| ![Restore, NextDisplay, PreviousDisplay, Smaller, Larger].contains(a)).collect()
    }

    /// Technical limit of one call (payload size), not a layout limit.
    pub const MAX_LAYOUT_ITEMS: usize = 200;
    pub const MAX_APP_NAME_LENGTH: usize = 100;

    /// Gadgets Nacho may switch: every gadget of this app (Computer Use and the pet are never among them).
    pub fn toggleable() -> Vec<ModuleId> { ModuleId::ALL.iter().copied().filter(|m| *m != ModuleId::NachoPet).collect() }

    fn object(properties: Value, required: &[&str]) -> Value {
        let mut o = json!({"type": "object", "properties": properties, "additionalProperties": false});
        if !required.is_empty() { o["required"] = json!(required); }
        o
    }

    fn app_property() -> Value {
        // Windows wording: the macOS text names bundle ids (com.apple.Safari).
        json!({"type": "string", "maxLength": Self::MAX_APP_NAME_LENGTH,
               "description": "앱 이름(한국어 표시 이름·영문 이름·실행 파일 이름), 예: 메모장, Notepad, notepad.exe"})
    }

    fn name_property() -> Value {
        json!({"type": "string", "minLength": 1, "maxLength": DesktopWorkspaceStore::MAX_NAME_LENGTH, "description": "작업 공간 이름, 예: 업무 모드"})
    }

    fn ids(list: &[SnapAction]) -> Vec<&'static str> { list.iter().map(|a| a.id()).collect() }

    /// Korean description (at most 200 characters: what and when) and JSON schema of a capability (or an alias).
    pub fn spec(name: &str) -> Option<(String, Value)> {
        if PetGadgetTools::is_tool(name) { return PetGadgetTools::spec(name); }
        let app = Self::app_property;
        let (d, schema) = match Self::resolve(name)? {
            Self::ARRANGE_WINDOW => ("맨 앞 창(또는 지정한 실행 중 앱의 창) 하나를 화면 왼쪽 반·오른쪽 반·최대화·네 귀퉁이·3분할 등으로 옮겨요. \
                next-display·previous-display는 옆 디스플레이로 옮겨요.",
                Self::object(json!({
                    "position": {"type": "string", "enum": Self::ids(&Self::positions()),
                                 "description": "left=왼쪽 반, right=오른쪽 반, maximize=최대화, top-left=왼쪽 위 1/4, first-third=왼쪽 1/3 …"},
                    "app": app()}), &["position"])),
            Self::LAYOUT_WINDOWS => ("여러 앱의 창을 한 번에 이름 붙은 자리(반·4등분·3등분·최대화 등)로 배치해요. 항목마다 앱과 위치, \
                필요하면 디스플레이 번호를 줘요. 개수 제한은 없어요(한 번에 200개까지).",
                Self::object(json!({"items": {
                    "type": "array", "minItems": 1, "maxItems": Self::MAX_LAYOUT_ITEMS,
                    "items": Self::object(json!({
                        "app": app(),
                        "position": {"type": "string", "enum": Self::ids(&Self::layout_positions())},
                        "display": {"type": "integer", "minimum": 1, "maximum": 16,
                                    "description": "디스플레이 번호(combo_list_windows 기준, 왼쪽부터 1). 없으면 창이 있는 화면"},
                        "window": {"type": "integer", "minimum": 1, "maximum": 20, "description": "그 앱의 몇 번째 창인지(앞에서부터 1). 기본 1"}}),
                        &["app", "position"])}}), &["items"])),
            Self::LIST_WINDOWS => ("PC의 디스플레이 목록(번호·크기)과 앱별 열린 창의 위치·크기·디스플레이 번호를 봐요(창 제목은 보내지 않아요). \
                여러 창을 배치하거나 다른 화면으로 옮기기 전에 지금 상태를 확인할 때 써요.", Self::object(json!({}), &[])),
            Self::LIST_APPS => ("PC에서 지금 실행 중인 앱 목록(이름·실행 파일·맨 앞인지·최소화됐는지)을 봐요. installed=true면 설치된 앱 이름도 함께 봐요. \
                앱 이름이 확실하지 않을 때 열거나 전환하기 전에 써요.",
                Self::object(json!({"installed": {"type": "boolean", "description": "설치된 앱도 포함(기본 false)"}}), &[])),
            Self::OPEN_APP => ("PC에 설치된 앱을 열고 맨 앞으로 가져와요(예: \"메모장 열어 줘\", \"계산기 켜 줘\"). 이미 실행 중이면 그 앱으로 전환해요. \
                앱 이름은 한국어 표시 이름·영문 이름·실행 파일 이름 모두 돼요.", Self::object(json!({"app": app()}), &["app"])),
            Self::FOCUS_APP => ("이미 실행 중인 앱으로 전환해 맨 앞에 보여 줘요(최소화돼 있으면 다시 보이게). \"크롬으로 바꿔 줘\"처럼 앱 전환을 원할 때 써요. \
                실행 중이 아니면 combo_open_app을 써요.", Self::object(json!({"app": app()}), &["app"])),
            Self::HIDE_APP => ("실행 중인 앱의 창을 모두 최소화해요(앱은 종료하지 않아요). \"슬랙 잠깐 가려 줘\"처럼 화면에서 치우고 싶을 때 써요. \
                탐색기·나초콤보는 숨기지 않아요.", Self::object(json!({"app": app()}), &["app"])),
            Self::QUIT_APP => ("실행 중인 앱을 종료해요(저장하지 않은 내용은 그 앱이 물어봐요). 사용자가 분명히 종료를 원할 때만 쓰고, \
                실행 전에 사용자 승인이 필요해요. 탐색기·나초콤보는 종료하지 않아요.", Self::object(json!({"app": app()}), &["app"])),
            Self::SET_SYSTEM => ("PC의 소리 크기(0~100), 화면 밝기(0~100), 방해금지(알림 끄기) 켜기/끄기를 바꿔요. 바꿀 항목만 넣어요. \
                이 PC에서 지원하지 않는 항목(외장 모니터 밝기 등)은 이유와 함께 실패로 알려 줘요.",
                Self::object(json!({
                    "volume": {"type": "integer", "minimum": 0, "maximum": 100, "description": "소리 크기 %"},
                    "brightness": {"type": "integer", "minimum": 0, "maximum": 100, "description": "내장 화면 밝기 %"},
                    "dnd": {"type": "boolean", "description": "방해금지 켜기(true)/끄기(false)"}}), &[])),
            Self::SET_GADGET => ("나초콤보 가젯(창 배치·집중 디밍·잠자기 방지·파일 선반 등) 하나를 켜거나 꺼요. 가젯 이름은 gadget 목록에서 골라요. \
                권한이 더 필요한 가젯은 사용자가 나초콤보에서 직접 켜야 해요.",
                Self::object(json!({
                    "gadget": {"type": "string", "enum": Self::toggleable().into_iter().map(|m| PetGadget::Module(m).raw_value()).collect::<Vec<_>>()},
                    "on": {"type": "boolean"}}), &["gadget", "on"])),
            Self::LIST_GADGETS => ("나초콤보 가젯 목록과 각각 켜져 있는지 봐요. 어떤 가젯이 있는지, 지금 켜져 있는지 확인할 때 써요.", Self::object(json!({}), &[])),
            Self::SET_DIM_STRENGTH => ("집중 디밍(맨 앞 창만 밝게 두고 나머지 화면을 어둡게 하는 가젯)의 어둡기를 10~90%로 바꿔요. \
                \"디밍 더 어둡게\"처럼 요청할 때 써요. 가젯이 꺼져 있으면 강도만 저장돼요.",
                Self::object(json!({"percent": {"type": "integer", "minimum": 10, "maximum": 90}}), &["percent"])),
            Self::CLEAR_SHELF => ("파일 선반에 올려 둔 항목을 모두 비워요(원본 파일은 지우지 않아요). 매번 사용자 승인을 받아요. \
                \"선반 비워 줘\"처럼 요청할 때 써요.", Self::object(json!({}), &[])),
            Self::SAVE_WORKSPACE => ("지금 PC 상태(실행 중인 앱, 창 위치, 가젯 켜짐 상태)를 이름을 붙여 작업 공간으로 저장해요(예: \"업무 모드\"). \
                같은 이름이 있으면 덮어써요.", Self::object(json!({"name": Self::name_property()}), &["name"])),
            Self::APPLY_WORKSPACE => ("저장한 작업 공간을 불러와요: 빠진 앱을 열고, 창을 저장한 자리로 옮기고, 가젯을 저장한 대로 켜고 꺼요. \
                이미 열려 있는 다른 앱은 종료하지 않아요. 이름은 combo_workspace_list로 확인해요.",
                Self::object(json!({"name": Self::name_property()}), &["name"])),
            Self::LIST_WORKSPACES => ("저장된 작업 공간 이름과 각각의 앱·창 수, 저장 시각을 봐요. 어떤 작업 공간이 있는지 확인할 때 써요.", Self::object(json!({}), &[])),
            _ => return None,
        };
        Some((d.to_string(), schema))
    }

    /// `hello.capabilities` / `capabilities` frame body (names match `^[a-zA-Z0-9_-]{1,64}$`).
    pub fn capabilities() -> Vec<Value> { Self::capabilities_with(Self::DECLARES_QUIT_APP, &BTreeSet::new()) }

    /// Names in `disabled` (the declared name or an alias) are left out: the shell may declare only what it runs.
    pub fn capabilities_with(allow_quit: bool, disabled: &BTreeSet<String>) -> Vec<Value> {
        Self::names_allowing(allow_quit).into_iter().filter(|n| !Self::connector_names(n).iter().any(|c| disabled.contains(*c))).filter_map(|name| {
            let (description, schema) = Self::spec(name)?;
            let mut c = json!({"name": name, "description": description, "input_schema": schema});
            if Self::APPROVAL_REQUIRED.contains(&name) { c["approval"] = json!("required"); }
            Some(c)
        }).collect()
    }

    /// The hello declaration of `names` (declared names, in order): `"approval": "required"` by risk
    /// ([`PetGadgetTools::approval_required`]; gadgets in `always_allowed` lift their content_out tools), names in
    /// `disabled` (declared name or alias) left out, `combo_quit_app` only with `allow_quit`.
    pub fn declaration(names: &[&str], allow_quit: bool, always_allowed: &BTreeSet<String>, disabled: &BTreeSet<String>) -> Vec<Value> {
        names.iter().copied().filter(|n| *n != Self::QUIT_APP || allow_quit)
            .filter(|n| !Self::connector_names(n).iter().any(|c| disabled.contains(*c)) && !disabled.contains(*n))
            .filter_map(|name| {
                let (description, schema) = Self::spec(name)?;
                let mut c = json!({"name": name, "description": description, "input_schema": schema});
                if PetGadgetTools::approval_required(name, always_allowed) { c["approval"] = json!("required"); }
                Some(c)
            }).collect()
    }

    /// What the Windows app declares: [`PetGadgetTools::WINDOWS`] (plus the chat handoff tools once the pet's chat
    /// window can take an attachment).
    pub fn windows_declaration(always_allowed: &BTreeSet<String>, disabled: &BTreeSet<String>, chat_handoff: bool) -> Vec<Value> {
        let mut names: Vec<&str> = PetGadgetTools::WINDOWS.to_vec();
        if chat_handoff {
            let at = names.iter().position(|n| *n == PetGadgetTools::CLIPBOARD_LIST).unwrap_or(names.len());
            names.insert(at, PetGadgetTools::SHELF_GIVE); // macOS order: shelf list, give, …, capture ask, list pinned
            let at = names.iter().position(|n| *n == PetGadgetTools::CAPTURE_LIST_PINNED).unwrap_or(names.len());
            names.insert(at, PetGadgetTools::CAPTURE_ASK);
        }
        Self::declaration(&names, Self::DECLARES_QUIT_APP, always_allowed, disabled)
    }

    /// Accepts the raw value (`windowSnap`) or the display name (`Window Snap`, any case and spacing). The macOS
    /// `menuBar` (Menu Bar Organizer) is Tray Organizer here.
    pub fn gadget(named: &str) -> Option<PetGadget> {
        let s = named.trim();
        if let Some(g) = PetGadget::from_raw(s) { return Some(g); }
        if s.eq_ignore_ascii_case("menuBar") || s.eq_ignore_ascii_case("menu bar organizer") { return Some(PetGadget::Module(ModuleId::TrayOrganizer)); }
        let folded = s.to_lowercase().replace(' ', "");
        PetGadget::all().into_iter().find(|g| g.raw_value().to_lowercase() == folded || g.display_name().to_lowercase().replace(' ', "") == folded)
    }

    // parameter readers

    /// A JSON boolean (never a number in disguise: `1` is not `true`).
    pub fn bool(v: Option<&Value>) -> Option<bool> { v.and_then(Value::as_bool) }

    /// A whole JSON number (5 or 5.0; not 5.5, not a boolean, not a string).
    pub fn integer(v: Option<&Value>) -> Option<i64> {
        let d = v.filter(|v| v.is_number())?.as_f64()?;
        (d.is_finite() && d.round() == d && d.abs() < 1e9).then_some(d as i64)
    }

    fn printable(s: &str) -> bool { s.chars().all(|c| c as u32 >= 0x20 && c as u32 != 0x7f) }

    /// Trimmed app name, 1...100 printable characters on one line.
    pub fn app_name(v: Option<&Value>, field: &str) -> Result<String, PetCommandProblem> {
        let Some(s) = v.and_then(Value::as_str) else { return Err(PetCommandProblem::BadParams(format!("{field} must be a string"))) };
        let t = s.trim();
        if t.is_empty() { return Err(PetCommandProblem::BadParams(format!("{field} must not be empty"))); }
        if t.chars().count() > Self::MAX_APP_NAME_LENGTH || !Self::printable(t) {
            return Err(PetCommandProblem::BadParams(format!("{field} must be at most {} printable characters", Self::MAX_APP_NAME_LENGTH)));
        }
        Ok(t.to_string())
    }

    fn unknown_keys(p: &JsonObject, allowed: &[&str]) -> Result<(), PetCommandProblem> {
        let extra: Vec<&str> = p.keys().map(String::as_str).filter(|k| !allowed.contains(k)).collect(); // keys are sorted
        if extra.is_empty() { return Ok(()); }
        Err(PetCommandProblem::BadParams(format!("unknown field{} {}", if extra.len() == 1 { "" } else { "s" }, extra.join(", "))))
    }

    fn present(v: Option<&Value>) -> bool { v.is_some_and(|v| !v.is_null()) }

    fn workspace_name(p: &JsonObject) -> Result<String, PetCommandProblem> {
        Self::unknown_keys(p, &["name"])?;
        let Some(raw) = p.get("name").and_then(Value::as_str) else { return Err(PetCommandProblem::BadParams("name must be a string".into())) };
        DesktopWorkspaceStore::valid_name(raw).map_err(|e| PetCommandProblem::BadParams(e.message()))
    }

    fn single_app(p: &JsonObject, make: fn(String) -> PetCommand) -> Result<PetCommand, PetCommandProblem> {
        Self::unknown_keys(p, &["app"])?;
        Self::app_name(p.get("app"), "app").map(make)
    }

    fn no_params(p: &JsonObject, command: PetCommand) -> Result<PetCommand, PetCommandProblem> {
        Self::unknown_keys(p, &[])?;
        Ok(command)
    }

    fn joined(list: &[SnapAction]) -> String { Self::ids(list).join(", ") }

    pub fn parse(capability: &str, params: Option<&Value>) -> Result<PetCommand, PetCommandProblem> {
        Self::parse_with(capability, params, Self::DECLARES_QUIT_APP)
    }

    /// The three first capabilities keep their original, lenient parameter reading; the newer ones refuse unknown fields.
    pub fn parse_with(raw_name: &str, params: Option<&Value>, allow_quit: bool) -> Result<PetCommand, PetCommandProblem> {
        use PetCommandProblem::*;
        let Some(capability) = Self::resolve(raw_name) else { return Err(UnknownCapability(raw_name.chars().take(64).collect())) };
        if PetGadgetTools::is_tool(capability) { return PetGadgetTools::parse(capability, params); }
        if Self::present(params) && !params.is_some_and(Value::is_object) { return Err(BadParams("parameters must be an object".into())); }
        let p = params.and_then(Value::as_object).cloned().unwrap_or_default();
        match capability {
            Self::ARRANGE_WINDOW => {
                let action = p.get("position").and_then(Value::as_str).and_then(|r| SnapAction::from_id(r.trim()));
                let Some(action) = action else { return Err(BadParams(format!("position must be one of {}", Self::joined(&Self::positions())))) };
                let mut app = None;
                if let Some(a) = p.get("app") {
                    let Some(s) = a.as_str() else { return Err(BadParams("app must be a string".into())) };
                    let t = s.trim();
                    if t.chars().count() > 100 || !Self::printable(t) { return Err(BadParams("app name must be at most 100 printable characters".into())); }
                    if !t.is_empty() { app = Some(t.to_string()); }
                }
                Ok(PetCommand::ArrangeWindow(action, app))
            }
            Self::SET_GADGET => {
                let toggleable = Self::toggleable();
                let g = p.get("gadget").and_then(Value::as_str).and_then(Self::gadget);
                let Some(g) = g else {
                    let names: Vec<String> = toggleable.iter().map(|m| PetGadget::Module(*m).raw_value()).collect();
                    return Err(BadParams(format!("gadget must be one of {}", names.join(", "))));
                };
                let module = match g { PetGadget::Module(m) if toggleable.contains(&m) => m, _ => {
                    return Err(NotAllowed(format!("{} can only be switched by the user in Nacho Combo", g.display_name())));
                } };
                let Some(on) = Self::bool(p.get("on")) else { return Err(BadParams("on must be true or false".into())) };
                Ok(PetCommand::SetGadget(module, on))
            }
            Self::LIST_GADGETS => Ok(PetCommand::ListGadgets),
            Self::LIST_APPS => {
                Self::unknown_keys(&p, &["installed"])?;
                if !Self::present(p.get("installed")) { return Ok(PetCommand::ListApps { installed: false }); }
                let Some(installed) = Self::bool(p.get("installed")) else { return Err(BadParams("installed must be true or false".into())) };
                Ok(PetCommand::ListApps { installed })
            }
            Self::OPEN_APP => Self::single_app(&p, PetCommand::OpenApp),
            Self::FOCUS_APP => Self::single_app(&p, PetCommand::FocusApp),
            Self::HIDE_APP => Self::single_app(&p, PetCommand::HideApp),
            Self::QUIT_APP => {
                if !allow_quit {
                    return Err(NotAllowed("quitting apps is switched off in this Nacho Combo until the server asks the user to approve it".into()));
                }
                Self::single_app(&p, PetCommand::QuitApp)
            }
            Self::LIST_WINDOWS => Self::no_params(&p, PetCommand::ListWindows),
            Self::LAYOUT_WINDOWS => Self::parse_layout(&p),
            Self::SET_SYSTEM => {
                Self::unknown_keys(&p, &["volume", "brightness", "dnd"])?;
                let mut change = PetSystemChange::default();
                for key in ["volume", "brightness"] {
                    if !Self::present(p.get(key)) { continue; }
                    let v = Self::integer(p.get(key)).filter(|v| (0..=100).contains(v));
                    let Some(v) = v else { return Err(BadParams(format!("{key} must be a whole number from 0 to 100"))) };
                    if key == "volume" { change.volume = Some(v) } else { change.brightness = Some(v) }
                }
                if Self::present(p.get("dnd")) {
                    let Some(on) = Self::bool(p.get("dnd")) else { return Err(BadParams("dnd must be true or false".into())) };
                    change.do_not_disturb = Some(on);
                }
                if change.is_empty() { return Err(BadParams("give at least one of volume, brightness, dnd".into())); }
                Ok(PetCommand::SetSystem(change))
            }
            Self::SET_DIM_STRENGTH => {
                Self::unknown_keys(&p, &["percent"])?;
                let Some(v) = Self::integer(p.get("percent")).filter(|v| (10..=90).contains(v)) else {
                    return Err(BadParams("percent must be a whole number from 10 to 90".into()));
                };
                Ok(PetCommand::SetDimStrength(v))
            }
            Self::CLEAR_SHELF => Self::no_params(&p, PetCommand::ClearShelf),
            Self::SAVE_WORKSPACE => Self::workspace_name(&p).map(PetCommand::SaveWorkspace),
            Self::APPLY_WORKSPACE => Self::workspace_name(&p).map(PetCommand::ApplyWorkspace),
            Self::LIST_WORKSPACES => Self::no_params(&p, PetCommand::ListWorkspaces),
            _ => Err(UnknownCapability(raw_name.chars().take(64).collect())),
        }
    }

    fn parse_layout(p: &JsonObject) -> Result<PetCommand, PetCommandProblem> {
        use PetCommandProblem::*;
        Self::unknown_keys(p, &["items"])?;
        let items = p.get("items").and_then(Value::as_array).filter(|a| !a.is_empty());
        let Some(items) = items else { return Err(BadParams("items must be a non-empty list".into())) };
        if items.len() > Self::MAX_LAYOUT_ITEMS { return Err(BadParams(format!("at most {} items", Self::MAX_LAYOUT_ITEMS))); }
        let layout = Self::layout_positions();
        let mut result: Vec<PetWindowPlacement> = Vec::new();
        for (n, raw) in items.iter().enumerate() {
            let label = format!("items[{n}]");
            let Some(item) = raw.as_object() else { return Err(BadParams(format!("{label} must be an object"))) };
            if let Err(BadParams(why)) = Self::unknown_keys(item, &["app", "position", "display", "window"]) {
                return Err(BadParams(format!("{label}: {why}")));
            }
            let app = Self::app_name(item.get("app"), &format!("{label}.app"))?;
            let position = item.get("position").and_then(Value::as_str).and_then(|r| SnapAction::from_id(r.trim())).filter(|a| layout.contains(a));
            let Some(position) = position else {
                return Err(BadParams(format!("{label}.position must be one of {}", Self::joined(&layout))));
            };
            let mut display = None;
            if Self::present(item.get("display")) {
                let Some(d) = Self::integer(item.get("display")).filter(|d| (1..=16).contains(d)) else {
                    return Err(BadParams(format!("{label}.display must be a whole number from 1 to 16")));
                };
                display = Some(d);
            }
            let mut window = 1;
            if Self::present(item.get("window")) {
                let Some(w) = Self::integer(item.get("window")).filter(|w| (1..=20).contains(w)) else {
                    return Err(BadParams(format!("{label}.window must be a whole number from 1 to 20")));
                };
                window = w;
            }
            if result.iter().any(|r| PetAppMatcher::fold(&r.app) == PetAppMatcher::fold(&app) && r.window == window) {
                return Err(BadParams(format!("{label} places the same window of {app} twice")));
            }
            result.push(PetWindowPlacement { app, position, display, window });
        }
        Ok(PetCommand::LayoutWindows(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::desktop::PetSystemChange;

    fn parse_json(cap: &str, p: Value) -> Result<PetCommand, PetCommandProblem> { PetCommands::parse(cap, Some(&p)) }

    fn names_of(caps: &[Value]) -> Vec<String> { caps.iter().map(|c| c["name"].as_str().unwrap().to_string()).collect() }

    fn name_ok(n: &str) -> bool { (1..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

    // PetCoreTests.swift: PetCommandsTests

    #[test]
    fn capabilities_are_well_formed() {
        let caps = PetCommands::capabilities();
        assert_eq!(names_of(&caps), PetCommands::names());
        for c in &caps {
            assert!(name_ok(c["name"].as_str().unwrap()));
            assert!(c["input_schema"].is_object());
        }
        let toggleable: Vec<PetGadget> = PetCommands::toggleable().into_iter().map(PetGadget::Module).collect();
        assert!(!toggleable.contains(&PetGadget::ComputerUse));
        assert!(!toggleable.contains(&PetGadget::NachoPet));
        assert!(toggleable.contains(&PetGadget::Module(ModuleId::WindowSnap)));
    }

    #[test]
    fn parse() {
        use PetCommandProblem::*;
        assert_eq!(parse_json("combo_arrange_window", json!({"position": "left"})), Ok(PetCommand::ArrangeWindow(SnapAction::Left, None)));
        assert_eq!(parse_json("combo_arrange_window", json!({"position": "top-left", "app": " Safari "})),
                   Ok(PetCommand::ArrangeWindow(SnapAction::TopLeft, Some("Safari".into()))));
        let all: Vec<&str> = SnapAction::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(parse_json("combo_arrange_window", json!({"position": "sideways"})),
                   Err(BadParams(format!("position must be one of {}", all.join(", ")))));
        assert!(parse_json("combo_arrange_window", json!({"position": "left", "app": "x".repeat(101)})).is_err(), "long app name accepted");
        assert!(parse_json("combo_arrange_window", json!({"position": "left", "app": "a\nb"})).is_err(), "control characters accepted");
        assert_eq!(parse_json("combo_set_gadget", json!({"gadget": "windowSnap", "on": true})), Ok(PetCommand::SetGadget(ModuleId::WindowSnap, true)));
        assert_eq!(parse_json("combo_set_gadget", json!({"gadget": "Keep Awake", "on": false})), Ok(PetCommand::SetGadget(ModuleId::KeepAwake, false)));
        assert_eq!(parse_json("combo_set_gadget", json!({"gadget": "computerUse", "on": true})),
                   Err(NotAllowed("Computer Use can only be switched by the user in Nacho Combo".into())));
        assert_eq!(parse_json("combo_set_gadget", json!({"gadget": "nachoPet", "on": false})),
                   Err(NotAllowed("Nacho Pet can only be switched by the user in Nacho Combo".into())));
        assert!(parse_json("combo_set_gadget", json!({"gadget": "windowSnap", "on": "yes"})).is_err(), "string accepted as bool");
        assert_eq!(PetCommands::parse("combo_list_gadgets", None), Ok(PetCommand::ListGadgets));
        assert_eq!(parse_json("run_shell", json!({})), Err(UnknownCapability("run_shell".into())));
    }

    #[test]
    fn parses_the_same_from_server_json() {
        // What the server forwards is JSON: booleans must not be confused with numbers.
        let v: Value = serde_json::from_str(r#"{"gadget":"focusDim","on":true}"#).unwrap();
        assert_eq!(PetCommands::parse("combo_set_gadget", Some(&v)), Ok(PetCommand::SetGadget(ModuleId::FocusDim, true)));
    }

    // PetDesktopTests.swift: PetDesktopParseTests

    #[test]
    fn parses_app_commands() {
        assert_eq!(parse_json("combo_open_app", json!({"app": "  계산기 "})), Ok(PetCommand::OpenApp("계산기".into())));
        assert_eq!(parse_json("combo_focus_app", json!({"app": "msedge.exe"})), Ok(PetCommand::FocusApp("msedge.exe".into())));
        assert_eq!(parse_json("combo_hide_app", json!({"app": "Slack"})), Ok(PetCommand::HideApp("Slack".into())));
        assert_eq!(PetCommands::parse("combo_list_apps", None), Ok(PetCommand::ListApps { installed: false }));
        assert_eq!(parse_json("combo_list_apps", json!({"installed": true})), Ok(PetCommand::ListApps { installed: true }));
        // Declared only with the server's approval card; then it parses.
        assert_eq!(PetCommands::parse_with("combo_quit_app", Some(&json!({"app": "Notepad"})), true), Ok(PetCommand::QuitApp("Notepad".into())));
        // The macOS registry's canonical names are aliases.
        assert_eq!(parse_json("combo_appswitch_open", json!({"app": "Notepad"})), Ok(PetCommand::OpenApp("Notepad".into())));
        assert_eq!(parse_json("combo_shelf_clear", json!({})), Ok(PetCommand::ClearShelf));
    }

    #[test]
    fn parses_layout_system_gadgets_and_workspaces() {
        assert_eq!(parse_json("combo_layout_windows", json!({"items": [
            {"app": "Safari", "position": "left"},
            {"app": "Notes", "position": "right", "display": 2},
            {"app": "Safari", "position": "maximize", "window": 2, "display": null},
        ]})), Ok(PetCommand::LayoutWindows(vec![
            PetWindowPlacement { app: "Safari".into(), position: SnapAction::Left, display: None, window: 1 },
            PetWindowPlacement { app: "Notes".into(), position: SnapAction::Right, display: Some(2), window: 1 },
            PetWindowPlacement { app: "Safari".into(), position: SnapAction::Maximize, display: None, window: 2 },
        ])));
        assert_eq!(parse_json("combo_set_system", json!({"volume": 0, "dnd": true})),
                   Ok(PetCommand::SetSystem(PetSystemChange { volume: Some(0), brightness: None, do_not_disturb: Some(true) })));
        assert_eq!(parse_json("combo_set_system", json!({"brightness": 100.0})),
                   Ok(PetCommand::SetSystem(PetSystemChange { volume: None, brightness: Some(100), do_not_disturb: None })));
        assert_eq!(parse_json("combo_set_dim_strength", json!({"percent": 70})), Ok(PetCommand::SetDimStrength(70)));
        assert_eq!(parse_json("combo_clear_shelf", json!({})), Ok(PetCommand::ClearShelf));
        assert_eq!(parse_json("combo_workspace_save", json!({"name": " 업무 모드 "})), Ok(PetCommand::SaveWorkspace("업무 모드".into())));
        assert_eq!(parse_json("combo_workspace_apply", json!({"name": "업무 모드"})), Ok(PetCommand::ApplyWorkspace("업무 모드".into())));
        assert_eq!(PetCommands::parse("combo_workspace_list", None), Ok(PetCommand::ListWorkspaces));
        assert_eq!(parse_json("combo_list_windows", Value::Null), Ok(PetCommand::ListWindows));
    }

    #[test]
    fn old_capabilities_stay_lenient() {
        // The first three capabilities shipped without strict field checks; servers may still send extra fields.
        assert_eq!(parse_json("combo_arrange_window", json!({"position": "left", "note": "x"})), Ok(PetCommand::ArrangeWindow(SnapAction::Left, None)));
        assert_eq!(parse_json("combo_list_gadgets", json!({"verbose": true})), Ok(PetCommand::ListGadgets));
    }

    #[test]
    fn quit_is_declared_with_approval() {
        const { assert!(PetCommands::DECLARES_QUIT_APP) };
        assert!(PetCommands::names().contains(&PetCommands::QUIT_APP));
        assert!(!PetCommands::names_allowing(false).contains(&PetCommands::QUIT_APP));
        let caps = PetCommands::capabilities();
        let asking: Vec<String> = caps.iter().filter(|c| c.get("approval").is_some()).map(|c| c["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(asking, vec![PetCommands::QUIT_APP, PetCommands::CLEAR_SHELF], "among the 16 first capabilities: quit and the shelf");
        assert!(!asking.contains(&PetCommands::LIST_APPS.to_string()));
        let on = PetCommands::capabilities_with(true, &BTreeSet::new());
        let quit = on.iter().find(|c| c["name"] == PetCommands::QUIT_APP).unwrap();
        assert_eq!(quit["approval"], "required");
        assert_eq!(on.iter().filter(|c| c.get("approval").is_some()).count(), asking.len());
        for c in &on { assert!(c["description"].as_str().unwrap().chars().count() <= 200); }
        assert_eq!(on.len(), PetCommands::all_names().len());
        assert_eq!(PetCommands::all_names().len(), PetCommands::all_names().iter().collect::<BTreeSet<_>>().len());
        for name in PetCommands::all_names() { assert!(PetCommands::spec(name).is_some(), "{name}"); }
        // A capability the shell switched off (by its name or an alias) is not declared.
        let off = PetCommands::capabilities_with(true, &BTreeSet::from(["combo_shelf_clear".to_string()]));
        assert!(!names_of(&off).contains(&PetCommands::CLEAR_SHELF.to_string()));
    }

    #[test]
    fn outcome_details_go_into_the_result_but_never_override_ok_or_message() {
        let details = json!({"apps": [{"name": "Safari"}], "ok": true, "message": "x"}).as_object().cloned();
        let o = PetCommandOutcome { ok: false, message: "m".into(), gadgets: None, details: details.clone() };
        let r = o.result_json();
        assert_eq!(r["ok"], false);
        assert_eq!(r["message"], "m");
        assert_eq!(r["apps"][0]["name"], "Safari");
        assert_eq!(o, PetCommandOutcome { ok: false, message: "m".into(), gadgets: None, details });
        assert_ne!(o, PetCommandOutcome::new(false, "m"));
    }

    // PetDesktopContractTests.swift

    fn caps() -> BTreeMap<String, Value> {
        PetCommands::capabilities().into_iter().map(|c| (c["name"].as_str().unwrap().to_string(), c)).collect()
    }

    #[test]
    fn hello_declares_the_desktop_actions() {
        let c = caps();
        for name in ["combo_list_apps", "combo_open_app", "combo_focus_app", "combo_hide_app", "combo_list_windows",
                     "combo_layout_windows", "combo_set_system", "combo_set_dim_strength", "combo_clear_shelf",
                     "combo_workspace_save", "combo_workspace_apply", "combo_workspace_list",
                     "combo_arrange_window", "combo_set_gadget", "combo_list_gadgets"] {
            assert!(c.contains_key(name), "{name} is not declared");
        }
        // Risky: declared, and only together with the server's approval card.
        assert_eq!(c["combo_quit_app"]["approval"], "required");
    }

    #[test]
    fn every_description_is_short_korean_and_every_schema_is_an_object() {
        for (name, c) in caps() {
            assert!(name.starts_with("combo_"), "{name}");
            assert!(name_ok(&name), "{name}");
            let d = c["description"].as_str().unwrap_or("");
            let n = d.chars().count();
            assert!(n <= 200, "{name}: {n} characters");
            assert!(n >= 30, "{name}: too short to choose by");
            let hangul = d.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count();
            assert!(hangul * 2 > d.chars().filter(|c| c.is_alphabetic()).count(), "{name}: description is not mostly Korean");
            let schema = &c["input_schema"];
            assert_eq!(schema["type"], "object", "{name}");
            assert!(schema["properties"].is_object(), "{name}");
            let risky = PetCommands::APPROVAL_REQUIRED.contains(&name.as_str());
            assert_eq!(c.get("approval").and_then(Value::as_str), if risky { Some("required") } else { None }, "{name}: approval follows the risk");
            if ["combo_quit_app", "combo_clear_shelf"].contains(&name.as_str()) { assert!(risky, "{name}"); }
            if ["combo_list_apps", "combo_open_app", "combo_list_windows"].contains(&name.as_str()) { assert!(!risky, "{name}"); }
        }
    }

    fn refused(capability: &str, params: Option<Value>) {
        match PetCommands::parse(capability, params.as_ref()) {
            Ok(c) => panic!("{capability} {params:?} accepted as {c:?}"),
            Err(PetCommandProblem::BadParams(_)) => {}
            Err(other) => panic!("{capability}: expected bad parameters, got {other:?}"),
        }
    }

    #[test]
    fn new_capabilities_refuse_bad_parameters() {
        let r = |c: &str, p: Value| refused(c, Some(p));
        r("combo_open_app", json!({}));
        r("combo_open_app", json!({"app": 5}));
        r("combo_open_app", json!({"app": "   "}));
        r("combo_open_app", json!({"app": "가".repeat(101)}));
        r("combo_open_app", json!({"app": "Safari\nrm"}));
        r("combo_open_app", json!({"app": "Safari", "path": "/bin/sh"}));
        r("combo_focus_app", json!({"app": ["Safari"]}));
        refused("combo_hide_app", None);
        r("combo_open_app", json!("Safari"));
        r("combo_list_apps", json!({"installed": "yes"}));
        r("combo_list_apps", json!({"installed": 1}));
        r("combo_set_system", json!({}));
        r("combo_set_system", json!({"volume": 101}));
        r("combo_set_system", json!({"volume": -1}));
        r("combo_set_system", json!({"volume": 55.5}));
        r("combo_set_system", json!({"volume": "50"}));
        r("combo_set_system", json!({"volume": true}));
        r("combo_set_system", json!({"brightness": 200}));
        r("combo_set_system", json!({"dnd": 1}));
        r("combo_set_system", json!({"volume": 30, "mute": true}));
        r("combo_set_dim_strength", json!({"percent": 5}));
        r("combo_set_dim_strength", json!({"percent": 95}));
        r("combo_clear_shelf", json!({"all": true}));
        r("combo_list_windows", json!({"titles": true}));
        r("combo_layout_windows", json!({}));
        r("combo_layout_windows", json!({"items": []}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari"}]}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari", "position": "next-display"}]}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari", "position": "left", "display": 0}]}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari", "position": "left", "display": 1.5}]}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari", "position": "left", "click": [10, 10]}]}));
        r("combo_layout_windows", json!({"items": [{"app": "Safari", "position": "left"}, {"app": "safari", "position": "right"}]}));
        r("combo_layout_windows", json!({"items": vec![json!({"app": "Notes", "position": "left"}); 9]}));
        r("combo_layout_windows", json!({"items": (0..201).map(|i| json!({"app": format!("App {i}"), "position": "left"})).collect::<Vec<_>>()}));
        r("combo_workspace_save", json!({}));
        r("combo_workspace_save", json!({"name": ""}));
        r("combo_workspace_save", json!({"name": "a".repeat(41)}));
        r("combo_workspace_save", json!({"name": "업무\n모드"}));
        r("combo_workspace_apply", json!({"name": 3}));
    }

    #[test]
    fn quit_parses_when_declared_and_is_refused_when_switched_off() {
        assert_eq!(parse_json("combo_quit_app", json!({"app": "Safari"})), Ok(PetCommand::QuitApp("Safari".into())));
        assert_eq!(PetCommands::parse_with("combo_quit_app", Some(&json!({"app": "Safari"})), false),
                   Err(PetCommandProblem::NotAllowed("quitting apps is switched off in this Nacho Combo until the server asks the user to approve it".into())));
    }

    #[test]
    fn good_parameters_from_server_json() {
        let j = |s: &str| -> Value { serde_json::from_str(s).unwrap() };
        assert!(PetCommands::parse("combo_set_system", Some(&j(r#"{"volume":40,"brightness":70.0,"dnd":false}"#))).is_ok());
        let layout = j(r#"{"items":[{"app":"메모장","position":"left"},{"app":"Notes","position":"right","display":2,"window":1}]}"#);
        assert!(PetCommands::parse("combo_layout_windows", Some(&layout)).is_ok());
        assert!(PetCommands::parse("combo_list_apps", Some(&j(r#"{"installed":true}"#))).is_ok());
        assert!(PetCommands::parse("combo_set_system", Some(&j(r#"{"volume":true}"#))).is_err(), "JSON true read as a volume");
        assert!(PetCommands::parse("combo_set_system", Some(&j(r#"{"dnd":1}"#))).is_err(), "JSON 1 read as a boolean");
    }
}

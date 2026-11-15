//! Gadget tools beyond the 16 first capabilities, and the risk of every capability. Port of the part of the macOS
//! gadget tool registry (`GadgetToolContract.swift`, `Gadgets/Tools/*Tools.swift`, `DataGadgetRunner.swift`,
//! `GadgetToolDispatch.swift`) that the Windows gadgets can run today: Keep Awake, Device Control, Focus Dim, Clipboard
//! History, File Shelf and Capture Float. Names, input schemas and risks are the macOS ones; descriptions use Windows
//! wording (PC instead of 맥).
//!
//! Risk decides the approval on the connector path (`approval_required`): `read` and `change` run at once,
//! `content_out` (the PC's content leaves it: clipboard text, a capture, a file) asks every time unless the user set
//! the gadget to "always allow" (`trust.rs`), `destructive` always asks.
//!
//! Not here (and not declared by Windows): actions that delete or throw something away (`combo_clipboard_clear`,
//! `combo_shelf_forget`, `combo_capture_close_all`; `combo_clear_shelf` stays in the first-contract list but the
//! Windows shell does not declare it), and the rest of the macOS registry (window grid tools, menu bar, monitor, disk,
//! mouse, calendar, desktop tidy, Computer Use, guides) — see docs/parity-macos.md.
use super::commands::{PetCommand, PetCommandOutcome, PetCommandProblem, PetCommands};
use super::crypto;
use super::json::JsonObject;
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::BTreeSet;

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

/// One tool: its connector name, the macOS gadget id it belongs to (`clipboard`, `shelf` …, the ids "always allow"
/// is stored under), risk, and the module that must be on for it to run (None: runs with the gadget off, as the
/// migrated first-contract capabilities do on macOS).
#[derive(Clone, Copy, Debug)]
pub struct PetGadgetTool {
    pub name: &'static str,
    pub gadget: &'static str,
    pub risk: PetToolRisk,
    pub requires: Option<ModuleId>,
}

const fn tool(name: &'static str, gadget: &'static str, risk: PetToolRisk, requires: Option<ModuleId>) -> PetGadgetTool {
    PetGadgetTool { name, gadget, risk, requires }
}

use PetToolRisk::*;

pub struct PetGadgetTools;

impl PetGadgetTools {
    pub const DEVICE_STATUS: &'static str = "combo_device_status";
    pub const DEVICE_SET_VOLUME: &'static str = "combo_device_set_volume";
    pub const DEVICE_SET_MIC_MUTED: &'static str = "combo_device_set_mic_muted";
    pub const DIM_STATUS: &'static str = "combo_dim_status";
    pub const DIM_PAUSE: &'static str = "combo_dim_pause";
    pub const DIM_RESUME: &'static str = "combo_dim_resume";
    pub const SHELF_LIST: &'static str = "combo_shelf_list";
    pub const SHELF_GIVE: &'static str = "combo_shelf_give";
    pub const CLIPBOARD_LIST: &'static str = "combo_clipboard_list";
    pub const CLIPBOARD_GET_ITEM: &'static str = "combo_clipboard_get_item";
    pub const CLIPBOARD_COPY_AGAIN: &'static str = "combo_clipboard_copy_again";
    pub const CAPTURE_ASK: &'static str = "combo_capture_ask";
    pub const CAPTURE_LIST_PINNED: &'static str = "combo_capture_list_pinned";
    pub const AWAKE_STATUS: &'static str = "combo_awake_status";
    pub const AWAKE_START: &'static str = "combo_awake_start";
    pub const AWAKE_STOP: &'static str = "combo_awake_stop";

    /// The tools of this port in the macOS hello order (after the 16 first capabilities).
    pub const TOOLS: [PetGadgetTool; 16] = [
        tool(Self::DEVICE_STATUS, "device", Read, Some(ModuleId::DeviceControl)),
        tool(Self::DEVICE_SET_VOLUME, "device", Change, Some(ModuleId::DeviceControl)),
        tool(Self::DEVICE_SET_MIC_MUTED, "device", Change, Some(ModuleId::DeviceControl)),
        tool(Self::DIM_STATUS, "dim", Read, Some(ModuleId::FocusDim)),
        tool(Self::DIM_PAUSE, "dim", Change, Some(ModuleId::FocusDim)),
        tool(Self::DIM_RESUME, "dim", Change, Some(ModuleId::FocusDim)),
        tool(Self::SHELF_LIST, "shelf", Read, Some(ModuleId::FileShelf)),
        tool(Self::SHELF_GIVE, "shelf", ContentOut, Some(ModuleId::FileShelf)),
        tool(Self::CLIPBOARD_LIST, "clipboard", Read, Some(ModuleId::Clipboard)),
        tool(Self::CLIPBOARD_GET_ITEM, "clipboard", ContentOut, Some(ModuleId::Clipboard)),
        tool(Self::CLIPBOARD_COPY_AGAIN, "clipboard", Change, Some(ModuleId::Clipboard)),
        tool(Self::CAPTURE_ASK, "capture", ContentOut, Some(ModuleId::CaptureFloat)),
        tool(Self::CAPTURE_LIST_PINNED, "capture", Read, Some(ModuleId::CaptureFloat)),
        tool(Self::AWAKE_STATUS, "awake", Read, Some(ModuleId::KeepAwake)),
        tool(Self::AWAKE_START, "awake", Change, Some(ModuleId::KeepAwake)),
        tool(Self::AWAKE_STOP, "awake", Change, Some(ModuleId::KeepAwake)),
    ];

    /// The 16 first capabilities: gadget and risk as the macOS registry gives them (`legacyConnectorName`).
    pub const LEGACY: [PetGadgetTool; 16] = [
        tool(PetCommands::ARRANGE_WINDOW, "window", Change, None),
        tool(PetCommands::LAYOUT_WINDOWS, "window", Change, None),
        tool(PetCommands::LIST_WINDOWS, "window", Read, None),
        tool(PetCommands::LIST_APPS, "appswitch", Read, None),
        tool(PetCommands::OPEN_APP, "appswitch", Change, None),
        tool(PetCommands::FOCUS_APP, "appswitch", Change, None),
        tool(PetCommands::HIDE_APP, "appswitch", Change, None),
        tool(PetCommands::QUIT_APP, "appswitch", Destructive, None),
        tool(PetCommands::SET_SYSTEM, "device", Change, None),
        tool(PetCommands::SET_GADGET, "combo", Change, None),
        tool(PetCommands::LIST_GADGETS, "combo", Read, None),
        tool(PetCommands::SET_DIM_STRENGTH, "dim", Change, None),
        tool(PetCommands::CLEAR_SHELF, "shelf", Destructive, None),
        tool(PetCommands::SAVE_WORKSPACE, "combo", Change, None),
        tool(PetCommands::APPLY_WORKSPACE, "combo", Change, None),
        tool(PetCommands::LIST_WORKSPACES, "combo", Read, None),
    ];

    /// What the Windows app declares, in hello order: the first contract without `combo_clear_shelf` (it throws the
    /// shelf's items away; no deleting actions on Windows), then the tools that run here today. `combo_shelf_give`
    /// and `combo_capture_ask` hand a file or a capture to the next chat message, which needs the pet's chat window
    /// (W2); until the shell has it they are not declared.
    pub const WINDOWS: [&'static str; 29] = [
        PetCommands::ARRANGE_WINDOW, PetCommands::LAYOUT_WINDOWS, PetCommands::LIST_WINDOWS, PetCommands::LIST_APPS,
        PetCommands::OPEN_APP, PetCommands::FOCUS_APP, PetCommands::HIDE_APP, PetCommands::QUIT_APP, PetCommands::SET_SYSTEM,
        PetCommands::SET_GADGET, PetCommands::LIST_GADGETS, PetCommands::SET_DIM_STRENGTH, PetCommands::SAVE_WORKSPACE,
        PetCommands::APPLY_WORKSPACE, PetCommands::LIST_WORKSPACES,
        Self::DEVICE_STATUS, Self::DEVICE_SET_VOLUME, Self::DEVICE_SET_MIC_MUTED, Self::DIM_STATUS, Self::DIM_PAUSE,
        Self::DIM_RESUME, Self::SHELF_LIST, Self::CLIPBOARD_LIST, Self::CLIPBOARD_GET_ITEM, Self::CLIPBOARD_COPY_AGAIN,
        Self::CAPTURE_LIST_PINNED, Self::AWAKE_STATUS, Self::AWAKE_START, Self::AWAKE_STOP,
    ];

    /// Declared once the shell can put a file or a picture on the next chat message (W2's chat window).
    pub const WITH_CHAT_HANDOFF: [&'static str; 2] = [Self::SHELF_GIVE, Self::CAPTURE_ASK];

    /// macOS capabilities Windows leaves out on purpose, with the reason (docs/parity-macos.md has the full table).
    pub const LEFT_OUT: [(&'static str, &'static str); 4] = [
        (PetCommands::CLEAR_SHELF, "deletes: empties the shelf and throws its own copies away"),
        ("combo_clipboard_clear", "deletes: the history cannot be brought back"),
        ("combo_shelf_forget", "deletes: takes an item off the shelf"),
        ("combo_capture_close_all", "deletes: closes pinned captures that were never saved"),
    ];

    pub fn find(name: &str) -> Option<PetGadgetTool> {
        let legacy = PetCommands::resolve(name);
        Self::LEGACY.iter().chain(Self::TOOLS.iter()).find(|t| Some(t.name) == legacy || t.name == name).copied()
    }

    pub fn is_tool(name: &str) -> bool { Self::TOOLS.iter().any(|t| t.name == name) }

    pub fn risk(name: &str) -> Option<PetToolRisk> { Self::find(name).map(|t| t.risk) }

    pub fn gadget(name: &str) -> Option<&'static str> { Self::find(name).map(|t| t.gadget) }

    /// `"approval": "required"` in hello (macOS `GadgetTool.connectorApproval`): destructive always, content_out and
    /// input_inject unless the gadget is always allowed, read and change never.
    pub fn approval_required(name: &str, always_allowed: &BTreeSet<String>) -> bool {
        let Some(t) = Self::find(name) else { return false };
        match t.risk {
            Read | Change => false,
            ContentOut | InputInject => !always_allowed.contains(t.gadget),
            Destructive => true,
        }
    }

    /// Gadgets with at least one tool among `names` that "always allow" can lift (macOS `trustableGadgets`).
    pub fn trustable(names: &[&str]) -> BTreeSet<String> {
        names.iter().filter_map(|n| Self::find(n)).filter(|t| t.risk.is_trustable()).map(|t| t.gadget.to_string()).collect()
    }

    /// The module that must be on for the command to run (macOS `requiresGadgetOn`; the first contract runs anyway).
    pub fn required_module(command: &PetCommand) -> Option<ModuleId> {
        use PetCommand::*;
        let name = match command {
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
            _ => return None,
        };
        Self::find(name).and_then(|t| t.requires)
    }

    /// Korean gadget names (macOS `GadgetID.koreanName`, Windows names where the gadget differs).
    pub fn korean_name(gadget: &str) -> &'static str {
        match gadget {
            "combo" => "나초콤보",
            "window" => "창 배치",
            "clipboard" => "클립보드 기록",
            "shelf" => "파일 선반",
            "capture" => "캡처 플로트",
            "dim" => "집중 디밍",
            "awake" => "잠자기 방지",
            "device" => "기기 제어",
            "appswitch" => "앱 전환",
            _ => "가젯",
        }
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

    pub fn spec(name: &str) -> Option<(String, Value)> {
        let none = || Self::object(json!({}), &[]);
        let (d, schema) = match name {
            Self::DEVICE_STATUS => ("지금 소리 출력 장치 이름·소리 크기와 마이크 음소거 여부를 봐요. 소리나 마이크를 바꾸기 전에 확인할 때 써요.", none()),
            Self::DEVICE_SET_VOLUME => ("지금 소리 출력의 크기(0~100)나 음소거(muted)를 바꿔요. 둘 중 하나 이상 넣어요. 장치를 바꾸거나 외장 모니터는 건드리지 않아요.",
                Self::object(json!({
                    "volume": {"type": "integer", "description": "소리 크기 %", "minimum": 0, "maximum": 100},
                    "muted": {"type": "boolean", "description": "음소거(true)/해제(false)"}}), &[])),
            Self::DEVICE_SET_MIC_MUTED => ("마이크를 음소거(true)하거나 켜요(false). 화상 회의 전후에 써요. 이 PC의 모든 마이크에 함께 적용돼요.",
                Self::object(json!({"muted": {"type": "boolean", "description": "true=음소거, false=켜기"}}), &["muted"])),
            Self::DIM_STATUS => ("집중 디밍이 지금 화면을 어둡게 하는지(잠시 멈춤인지)와 어둡기를 봐요. 조절하기 전에 확인할 때 써요.", none()),
            Self::DIM_PAUSE => ("집중 디밍을 잠시 멈춰 화면을 원래 밝기로 보여 줘요(가젯은 켜진 채). 다시 어둡게 하려면 dim_resume을 써요.", none()),
            Self::DIM_RESUME => ("멈춰 둔 집중 디밍을 다시 시작해 맨 앞 창만 밝게 두고 나머지를 어둡게 해요.", none()),
            Self::SHELF_LIST => ("파일 선반에 올려 둔 항목 목록을 봐요: 번호·id·이름·종류(파일·그림·글·링크)·크기만 보여 주고 내용은 없어요. 선반에 무엇이 있는지 물을 때 써요.", none()),
            Self::SHELF_GIVE => ("파일 선반의 N번 항목을 나초 대화의 다음 메시지에 첨부해요. 사용자가 보내기를 누르면 나초에게 가요. 파일이 PC 밖으로 나가므로 매번 승인을 받아요.", Self::item_ref()),
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
        match name {
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
}

impl PetDimState {
    pub fn outcome(&self, lead: Option<&str>) -> PetCommandOutcome {
        let percent = (self.strength * 100.0).round() as i64;
        let state = if self.paused { format!("Focus Dim is paused (strength {percent}%).") } else { format!("Focus Dim is dimming everything but the front window ({percent}%).") };
        let message = lead.map_or(state.clone(), |l| format!("{l} {state}"));
        PetCommandOutcome::with(true, message, json!({"paused": self.paused, "strength_percent": percent}))
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
            assert!(t.name.starts_with(&format!("combo_{}_", t.gadget)), "{}", t.name);
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
        assert_eq!(PetGadgetTools::trustable(&PetGadgetTools::WINDOWS), BTreeSet::from(["clipboard".to_string()]));
        let mut with_handoff = PetGadgetTools::WINDOWS.to_vec();
        with_handoff.extend(PetGadgetTools::WITH_CHAT_HANDOFF);
        assert_eq!(PetGadgetTools::trustable(&with_handoff), ["capture", "clipboard", "shelf"].iter().map(|s| s.to_string()).collect());
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
    fn state_answers() {
        let s = PetAwakeState { manual: true, seconds_left: Some(90), ..Default::default() };
        let o = s.outcome(None);
        assert_eq!(o.details.as_ref().unwrap()["holding"], true);
        assert_eq!(o.details.as_ref().unwrap()["seconds_left"], 90);
        assert!(o.message.contains("2 more minutes"), "{}", o.message);
        let guarded = PetAwakeState { manual: true, guarded: true, ..Default::default() }.outcome(None);
        assert_eq!(guarded.details.unwrap()["reason"], "battery_guard");
        let d = PetDimState { paused: true, strength: 0.6 }.outcome(Some("Paused.")).details.unwrap();
        assert_eq!(d["strength_percent"], 60);
        let dev = PetDeviceState { output: Some("Speakers".into()), volume: Some(0.3), muted: false, input: None, mic_muted: true }.outcome();
        assert_eq!(dev.details.as_ref().unwrap()["volume"], 30);
        assert_eq!(dev.details.unwrap()["mic_muted"], true);
    }
}

/// The declaration is fixed against the macOS app's own output (`fixtures/mac-capabilities.json`: what macOS
/// `PetCommands.capabilities` prints for the names the Windows catalog knows).
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

    #[test]
    fn input_schemas_match_the_macos_declaration() {
        let mac = by_name(mac()["capabilities"].as_array().unwrap());
        for name in catalog() {
            let m = mac.get(name).unwrap_or_else(|| panic!("{name} is not a macOS capability"));
            let (_, ours) = PetCommands::spec(name).unwrap();
            let (mut theirs, mut ours) = (shape(&m["input_schema"]), shape(&ours));
            if name == PetCommands::SET_GADGET {
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
            assert_eq!(ours, theirs, "{name}: input schema differs from macOS");
        }
    }

    #[test]
    fn approvals_match_the_macos_declaration_with_and_without_always_allow() {
        let m = mac();
        let mac_plain = by_name(m["capabilities"].as_array().unwrap());
        let mac_trusted = m["approval_when_always_allowed"].as_object().unwrap().clone();
        let none = BTreeSet::new();
        let all: BTreeSet<String> = ["clipboard", "shelf", "capture"].iter().map(|s| s.to_string()).collect();
        for name in catalog() {
            let theirs = mac_plain[name].get("approval").and_then(Value::as_str) == Some("required");
            assert_eq!(PetGadgetTools::approval_required(name, &none), theirs, "{name}: approval (always allow off)");
            let theirs = mac_trusted[name].as_str() == Some("required");
            assert_eq!(PetGadgetTools::approval_required(name, &all), theirs, "{name}: approval (always allow on)");
        }
        let decl = by_name(&PetCommands::windows_declaration(&none, &none, true));
        for (name, c) in &decl {
            assert_eq!(c.get("approval"), mac_plain[name].get("approval"), "{name}: hello approval field");
            let keys: BTreeSet<&String> = c.as_object().unwrap().keys().collect();
            let mac_keys: BTreeSet<&String> = mac_plain[name].as_object().unwrap().keys().collect();
            assert_eq!(keys, mac_keys, "{name}: hello fields");
        }
        let trustable: BTreeSet<String> = m["trustable_gadgets"].as_array().unwrap().iter().map(|g| g.as_str().unwrap().to_string())
            .filter(|g| g != "computer").collect(); // Computer Use is W5
        assert_eq!(PetGadgetTools::trustable(&catalog()), trustable);
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
            assert!((30..=200).contains(&d.chars().count()), "{name}");
            assert!(!d.contains('맥') && !d.contains("Finder") && !d.contains("⌘"), "{name}: macOS wording: {d}");
            let hangul = d.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count();
            assert!(hangul * 2 > d.chars().filter(|c| c.is_alphabetic()).count(), "{name}");
        }
    }

    #[test]
    fn macos_gadget_names_reach_the_windows_gadgets() {
        assert_eq!(PetCommands::gadget("menuBar"), Some(crate::core::pet::commands::PetGadget::Module(ModuleId::TrayOrganizer)));
        assert_eq!(PetCommands::parse("combo_set_gadget", Some(&json!({"gadget": "menuBar", "on": true}))),
                   Ok(PetCommand::SetGadget(ModuleId::TrayOrganizer, true)));
    }
}

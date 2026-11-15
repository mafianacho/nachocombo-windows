//! The 연결 (Connections) tab of the pet's chat bubble: what it shows, as plain values, and the server calls behind its
//! switches. Port of `PetConnectionsModel.swift`, `PetConnectionsDetail.swift` and `PetConnectionsAPI.swift` (macOS
//! NACHO-131 / 239 / 151). Three sections (도구 / 커넥터 / 가젯), one row per item (name, one status line, a switch) and a
//! detail per row. The shell only draws these values, so everything the user reads is tested here.
//!
//! The actions of a gadget are read from the gadget tool registry (`PetGadgetTools`, the same list hello declares): a
//! tool added there shows up in the tab with no change here. Labels in [`action_label`] only give a short sentence;
//! a tool without one shows the first sentence of its description.
//!
//! Not ported: connecting a new service (catalog, `begin`, attempts, app password) — the tab lists, switches, sets
//! the scope of and disconnects what is already connected. See docs/parity-macos.md.
use super::commands::PetCommands;
use super::connectors::{ConnectionRow, ConnectorDecoder, ConnectorOverview, ToolRow};
use super::gadget_tools::{PetGadgetTools, PetToolRisk};
use super::http::{HTTPResult, PetHTTPClient, PetHTTPRequest, PetServerError};
use super::json::{self, JsonExt};
use super::settings::ServerEndpoint;
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionsSection { Tools, Connectors, Gadgets }

impl ConnectionsSection {
    pub const ALL: [ConnectionsSection; 3] = [Self::Tools, Self::Connectors, Self::Gadgets];

    pub fn title(self) -> &'static str {
        match self { Self::Tools => "도구", Self::Connectors => "커넥터", Self::Gadgets => "가젯" }
    }

    pub fn empty_text(self) -> &'static str {
        match self {
            Self::Tools => "나초가 쓸 수 있는 도구가 아직 없어요.",
            Self::Connectors => "연결된 서비스가 아직 없어요.",
            Self::Gadgets => "나초콤보 가젯이 없어요.",
        }
    }
}

/// What a row or a detail is about. A connector is identified by its connection id (the PATCH/DELETE path), a gadget by
/// the macOS gadget id (`clipboard`, the id "always allow" is stored under).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionsRef {
    Tool(String),
    Connector(String),
    Gadget(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionsTone {
    Normal,
    /// Needs the user (sign in again, an error).
    Attention,
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionsSwitch {
    On,
    Off,
    /// A default tool Nacho needs in every chat: shown with a lock, the switch is blocked.
    Locked,
    /// Cannot be switched here; the text says why and where.
    Fixed(String),
}

impl ConnectionsSwitch {
    pub fn is_on(&self) -> bool { matches!(self, Self::On | Self::Locked) }
    pub fn can_toggle(&self) -> bool { matches!(self, Self::On | Self::Off) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsRow {
    pub target: ConnectionsRef,
    pub section: ConnectionsSection,
    pub title: String,
    pub status: String,
    pub tone: ConnectionsTone,
    pub control: ConnectionsSwitch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsSectionRows {
    pub section: ConnectionsSection,
    pub rows: Vec<ConnectionsRow>,
}

// MARK: - connection health

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionHealth { Connected, NeedsReauth, Off, NeedsPermission, Error, Disconnected, Unknown }

impl ConnectionHealth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "연결됨",
            Self::NeedsReauth => "다시 로그인 필요",
            Self::Off => "꺼짐",
            Self::NeedsPermission => "권한 필요",
            Self::Error => "오류",
            Self::Disconnected => "연결 끊김",
            Self::Unknown => "알 수 없음",
        }
    }
}

impl ConnectionRow {
    /// One state for the row and the detail. A switched-off service reads "꺼짐" whatever else it needs.
    pub fn health(&self) -> ConnectionHealth {
        let attention = self.needs_attention.as_deref();
        if self.status == "disconnected" { return ConnectionHealth::Disconnected; }
        if !self.enabled { return ConnectionHealth::Off; }
        if self.status == "needs_reauth" || attention == Some("needs_reauth") { return ConnectionHealth::NeedsReauth; }
        if self.status == "needs_permission" || attention == Some("needs_permission") { return ConnectionHealth::NeedsPermission; }
        if self.status == "error" { return ConnectionHealth::Error; }
        if self.status == "connected" { return ConnectionHealth::Connected; }
        ConnectionHealth::Unknown
    }

    /// The small note under a healthy connection that is about to need attention.
    pub fn attention_note(&self) -> Option<&'static str> {
        match self.needs_attention.as_deref() {
            Some("degraded") => Some("일부 기능이 불안정해요"),
            Some("expiring") => Some("곧 다시 로그인해야 해요"),
            _ => None,
        }
    }

    pub fn tone(&self) -> ConnectionsTone {
        match self.health() {
            ConnectionHealth::Connected => if self.attention_note().is_none() { ConnectionsTone::Normal } else { ConnectionsTone::Attention },
            ConnectionHealth::Off | ConnectionHealth::Disconnected => ConnectionsTone::Off,
            _ => ConnectionsTone::Attention,
        }
    }

    /// The status line of the row: state, then the account when there is one. Pieces are joined with " · ".
    pub fn status_line(&self) -> String {
        let mut line = self.health().label().to_string();
        if self.health() == ConnectionHealth::Connected {
            if let Some(n) = self.attention_note() { line.push_str(" · "); line.push_str(n); }
        }
        if let Some(a) = self.account_label.as_deref().filter(|a| !a.is_empty()) { line.push_str(" · "); line.push_str(a); }
        line
    }

    fn switch(&self) -> ConnectionsSwitch {
        if self.enabled && self.health() != ConnectionHealth::Disconnected { ConnectionsSwitch::On } else { ConnectionsSwitch::Off }
    }
}

// MARK: - words for tool names

/// What the list calls a server tool: its name with the separators taken out (`gmail_search_messages` →
/// "gmail search messages"). The macOS word table is Korean only; Windows shows the server's name as it is.
pub fn tool_title(name: &str) -> String {
    let t: Vec<&str> = name.split(['_', '-', '.', ' ']).filter(|p| !p.is_empty()).collect();
    if t.is_empty() { name.to_string() } else { t.join(" ") }
}

/// The first sentence of a description, for one line under a name. Empty when there is none.
pub fn first_sentence(text: &str, limit: usize) -> String {
    let t = text.trim();
    if t.is_empty() { return String::new(); }
    let chars: Vec<char> = t.chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' { end = i; break; }
        if *c == '.' && chars.get(i + 1).is_none_or(|n| *n == ' ' || *n == '\n') { end = i + 1; break; }
    }
    let s: String = chars[..end].iter().collect::<String>().trim().to_string();
    if s.chars().count() > limit { format!("{}…", s.chars().take(limit.saturating_sub(1)).collect::<String>()) } else { s }
}

impl ToolRow {
    pub fn title(&self) -> String { tool_title(&self.name) }
    pub fn summary_line(&self) -> String { first_sentence(&self.description, 90) }
    fn control(&self) -> ConnectionsSwitch {
        if self.locked { ConnectionsSwitch::Locked } else if self.user_enabled { ConnectionsSwitch::On } else { ConnectionsSwitch::Off }
    }
}

// MARK: - what Nacho does on this PC (from the registry)

/// One action of a gadget: a connector tool Nacho can call on this PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcActionRow {
    /// The connector name that goes into `GadgetTrust::disabled`.
    pub id: String,
    pub gadget: &'static str,
    pub title: String,
    pub risk: PetToolRisk,
    pub on: bool,
}

/// Every tool this app declares, in registry order: [`PetGadgetTools::WINDOWS`], then the two chat-handoff tools after
/// the last tool of their gadget when the shell has the handoff.
pub fn declared_names(chat_handoff: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PetGadgetTools::WINDOWS.to_vec();
    if chat_handoff {
        for extra in PetGadgetTools::WITH_CHAT_HANDOFF {
            let gadget = PetGadgetTools::gadget(extra);
            let at = names.iter().rposition(|n| PetGadgetTools::gadget(n) == gadget).map_or(names.len(), |i| i + 1);
            names.insert(at, extra);
        }
    }
    names
}

/// Short sentences for the actions, one per tool (macOS shows the first sentence of each tool's description; these are
/// the same sentences, shortened, so the 4 languages can translate them). A tool without a label shows its description.
pub fn action_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "combo_arrange_window" => "창 하나를 화면의 정해진 자리에 놓아요",
        "combo_layout_windows" => "여러 창을 한 번에 배치해요",
        "combo_list_windows" => "열려 있는 창 목록을 봐요",
        "combo_list_apps" => "실행 중인 앱과 설치된 앱을 봐요",
        "combo_open_app" => "앱을 열어요",
        "combo_focus_app" => "앱을 앞으로 가져와요",
        "combo_hide_app" => "앱의 창을 숨겨요",
        "combo_quit_app" => "앱을 종료해요",
        "combo_set_system" => "소리·밝기 같은 시스템 설정을 바꿔요",
        "combo_set_gadget" => "가젯을 켜고 꺼요",
        "combo_list_gadgets" => "가젯이 켜져 있는지 봐요",
        "combo_set_dim_strength" => "집중 디밍의 어둡기를 바꿔요",
        "combo_workspace_save" => "지금 창 배치를 작업 공간으로 저장해요",
        "combo_workspace_apply" => "저장한 작업 공간을 다시 불러와요",
        "combo_workspace_list" => "저장한 작업 공간 목록을 봐요",
        "combo_device_status" => "소리 출력과 마이크 상태를 봐요",
        "combo_device_set_volume" => "소리 크기나 음소거를 바꿔요",
        "combo_device_set_mic_muted" => "마이크를 음소거하거나 켜요",
        "combo_dim_status" => "집중 디밍 상태를 봐요",
        "combo_dim_pause" => "집중 디밍을 잠시 멈춰요",
        "combo_dim_resume" => "멈춘 집중 디밍을 다시 시작해요",
        "combo_shelf_list" => "파일 선반의 항목 목록을 봐요",
        "combo_shelf_give" => "선반의 파일을 나초에게 보내요",
        "combo_clipboard_list" => "클립보드 기록 목록을 봐요",
        "combo_clipboard_get_item" => "클립보드 기록의 내용을 나초에게 보내요",
        "combo_clipboard_copy_again" => "클립보드 기록의 항목을 다시 복사해요",
        "combo_capture_ask" => "화면을 캡처해 나초에게 보내요",
        "combo_capture_list_pinned" => "고정해 둔 캡처 개수를 봐요",
        "combo_awake_status" => "잠자기 방지 상태를 봐요",
        "combo_awake_start" => "PC가 잠들지 않게 해요",
        "combo_awake_stop" => "잠자기 방지를 풀어요",
        _ => return None,
    })
}

/// The actions of a gadget in registry order, from the names the app declares. `disabled`: connector names the user
/// switched off (`GadgetTrust::disabled`).
pub fn actions_for(gadget: &str, disabled: &BTreeSet<String>, names: &[&'static str]) -> Vec<PcActionRow> {
    names.iter().filter_map(|name| {
        let tool = PetGadgetTools::find(name)?;
        if tool.gadget != gadget { return None; }
        let title = match action_label(name) {
            Some(l) => l.to_string(),
            None => PetCommands::spec(name).map(|(d, _)| first_sentence(&d, 120)).filter(|d| !d.is_empty()).unwrap_or_else(|| tool_title(name)),
        };
        let off = disabled.contains(*name) || PetCommands::connector_names(name).iter().any(|c| disabled.contains(*c));
        Some(PcActionRow { id: name.to_string(), gadget: tool.gadget, title, risk: tool.risk, on: !off })
    }).collect()
}

/// What the user reads under an action: how Nacho asks. None for read / change (they just run).
pub fn risk_text(risk: PetToolRisk, always_allowed: bool) -> Option<&'static str> {
    match risk {
        PetToolRisk::Read | PetToolRisk::Change => None,
        PetToolRisk::ContentOut | PetToolRisk::InputInject => Some(if always_allowed { "이 가젯은 항상 허용이라 묻지 않고 해요" } else { "보내기 전에 매번 물어봐요" }),
        PetToolRisk::Destructive => Some("매번 물어봐요(항상 허용 안 됨)"),
    }
}

/// The sentence before "always allow" is switched on: what then goes out without a question.
pub fn always_allow_confirm(gadget: &str) -> String {
    format!("{} 내용을 나초에게 보내는 일을 묻지 않고 해요. 되돌릴 수 없는 행동은 계속 매번 물어봐요. 언제든 끌 수 있어요.", PetGadgetTools::display_name(gadget))
}

/// "PC 행동 5개 중 4개 허용"
pub fn actions_summary(rows: &[PcActionRow]) -> String {
    if rows.is_empty() { return "PC에서 하는 일이 없어요".into(); }
    let on = rows.iter().filter(|r| r.on).count();
    if on == rows.len() { format!("PC 행동 {}개 모두 허용", rows.len()) } else { format!("PC 행동 {}개 중 {}개 허용", rows.len(), on) }
}

// MARK: - gadget switches (the app's own on/off)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GadgetPhase { On, Off, Failed }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GadgetSwitchState {
    pub module: ModuleId,
    pub gadget: &'static str,
    pub phase: GadgetPhase,
}

impl GadgetSwitchState {
    /// The gadgets the tab lists: every gadget with a macOS gadget id but the pet itself (Nacho never switches it).
    pub fn listed() -> Vec<(ModuleId, &'static str)> {
        ModuleId::ALL.iter().filter_map(|m| PetGadgetTools::gadget_of_module(*m).map(|g| (*m, g))).collect()
    }

    pub fn control(&self) -> ConnectionsSwitch {
        if self.phase == GadgetPhase::On { ConnectionsSwitch::On } else { ConnectionsSwitch::Off }
    }
}

/// Section, rows and details need the same facts: what the server said, the gadget switches, the "always allow"
/// switches, the actions the user switched off, and whether Nacho may use this PC at all.
#[derive(Clone, Debug)]
pub struct ConnectionsInput<'a> {
    pub overview: Option<&'a ConnectorOverview>,
    pub gadgets: &'a [GadgetSwitchState],
    pub disabled: &'a BTreeSet<String>,
    pub always_allowed: &'a BTreeSet<String>,
    pub trustable: &'a BTreeSet<String>,
    pub names: &'a [&'static str],
    pub allow_combo_control: bool,
}

// MARK: - the list

pub fn sections(input: &ConnectionsInput, query: &str) -> Vec<ConnectionsSectionRows> {
    let q = query.trim().to_lowercase();
    let mut out = vec![];
    for section in ConnectionsSection::ALL {
        let all = match section {
            ConnectionsSection::Tools => tool_rows(input.overview),
            ConnectionsSection::Connectors => connector_rows(input.overview),
            ConnectionsSection::Gadgets => gadget_rows(input),
        };
        let rows: Vec<ConnectionsRow> = if q.is_empty() { all } else { all.into_iter().filter(|r| matches(r, &q, input.overview)).collect() };
        if q.is_empty() || !rows.is_empty() { out.push(ConnectionsSectionRows { section, rows }); }
    }
    out
}

fn matches(row: &ConnectionsRow, q: &str, overview: Option<&ConnectorOverview>) -> bool {
    let mut hay = vec![row.title.clone(), row.status.clone()];
    match &row.target {
        ConnectionsRef::Tool(id) => if let Some(t) = overview.and_then(|o| o.tools.iter().find(|t| &t.id == id)) { hay.extend([t.name.clone(), t.description.clone()]); },
        ConnectionsRef::Connector(id) => if let Some(c) = overview.and_then(|o| o.connections.iter().find(|c| &c.id == id)) { hay.extend([c.source.clone(), c.account_label.clone().unwrap_or_default()]); },
        ConnectionsRef::Gadget(g) => hay.extend([g.to_string(), PetGadgetTools::display_name(g).to_string()]),
    }
    hay.iter().any(|h| h.to_lowercase().contains(q))
}

/// Tools that belong to a connected service (`source` set, kind "connection") are listed in that service's detail.
fn tool_rows(overview: Option<&ConnectorOverview>) -> Vec<ConnectionsRow> {
    overview.map(|o| o.tools.as_slice()).unwrap_or_default().iter().filter(|t| t.kind != "connection" && t.source.is_none()).map(|t| {
        let mut status = if t.summary_line().is_empty() { t.kind_text().to_string() } else { t.summary_line() };
        if t.locked { status = "기본 도구 · 항상 켜져 있어요".into(); } else if !t.user_enabled { status = "꺼짐".into(); }
        ConnectionsRow { target: ConnectionsRef::Tool(t.id.clone()), section: ConnectionsSection::Tools, title: t.title(), status,
            tone: if t.user_enabled { ConnectionsTone::Normal } else { ConnectionsTone::Off }, control: t.control() }
    }).collect()
}

fn connector_rows(overview: Option<&ConnectorOverview>) -> Vec<ConnectionsRow> {
    overview.map(|o| o.connections.as_slice()).unwrap_or_default().iter().map(|c| ConnectionsRow {
        target: ConnectionsRef::Connector(c.id.clone()), section: ConnectionsSection::Connectors, title: c.label.clone(),
        status: c.status_line(), tone: c.tone(), control: c.switch(),
    }).collect()
}

fn gadget_rows(input: &ConnectionsInput) -> Vec<ConnectionsRow> {
    input.gadgets.iter().map(|g| {
        let actions = actions_for(g.gadget, input.disabled, input.names);
        let (status, tone) = match g.phase {
            GadgetPhase::On => (if actions.is_empty() { "켜짐".to_string() } else { actions_summary(&actions) }, ConnectionsTone::Normal),
            GadgetPhase::Off => ("꺼짐".to_string(), ConnectionsTone::Off),
            GadgetPhase::Failed => ("시작하지 못했어요".to_string(), ConnectionsTone::Attention),
        };
        ConnectionsRow { target: ConnectionsRef::Gadget(g.gadget), section: ConnectionsSection::Gadgets,
            title: PetGadgetTools::display_name(g.gadget).to_string(), status, tone, control: g.control() }
    }).collect()
}

// MARK: - details

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsAbility {
    pub id: String,
    pub title: String,
    /// nil = no switch of its own (it follows the service).
    pub on: Option<bool>,
    /// How Nacho asks before doing it (None = it just runs).
    pub risk: Option<PetToolRisk>,
    pub risk_text: Option<&'static str>,
    pub note: Option<String>,
}

/// The "이 가젯은 항상 허용" switch of a gadget that has content_out / input_inject actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsAlwaysAllow {
    pub gadget: &'static str,
    pub on: bool,
    pub note: &'static str,
    /// Shown before it is switched on. Switching off needs no question.
    pub confirm_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsFact { pub label: &'static str, pub value: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsScopeChoice { pub levels: [(&'static str, &'static str); 2], pub selected: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionsDetail {
    pub target: ConnectionsRef,
    pub title: String,
    pub status: String,
    pub tone: ConnectionsTone,
    pub control: ConnectionsSwitch,
    pub summary: String,
    pub abilities_title: &'static str,
    pub abilities: Vec<ConnectionsAbility>,
    pub empty_abilities: &'static str,
    pub facts: Vec<ConnectionsFact>,
    /// A sentence the user must read (sign in again, locked, Nacho's control is off).
    pub notice: Option<String>,
    pub scope: Option<ConnectionsScopeChoice>,
    pub always_allow: Option<ConnectionsAlwaysAllow>,
    pub can_disconnect: bool,
}

pub fn detail(target: &ConnectionsRef, input: &ConnectionsInput) -> Option<ConnectionsDetail> {
    match target {
        ConnectionsRef::Tool(id) => input.overview?.tools.iter().find(|t| &t.id == id).map(tool_detail),
        ConnectionsRef::Connector(id) => {
            let o = input.overview?;
            o.connections.iter().find(|c| &c.id == id).map(|c| connector_detail(c, &o.tools))
        }
        ConnectionsRef::Gadget(g) => input.gadgets.iter().find(|s| s.gadget == *g).map(|s| gadget_detail(s, input)),
    }
}

pub fn tool_detail(t: &ToolRow) -> ConnectionsDetail {
    let status = if t.locked { "항상 켜짐 · 기본 도구" } else if t.user_enabled { "켜짐" } else { "꺼짐" };
    let mut facts = vec![ConnectionsFact { label: "종류", value: t.kind_text().to_string() }];
    if let Some(s) = &t.source { facts.push(ConnectionsFact { label: "서비스", value: s.clone() }); }
    let notice = if t.locked { Some("나초가 대화하는 데 꼭 필요한 기본 도구라서 끌 수 없어요.") } else if t.user_enabled { None } else { Some("꺼 두면 나초가 이 도구를 고르지 않아요.") };
    ConnectionsDetail {
        target: ConnectionsRef::Tool(t.id.clone()), title: t.title(), status: status.into(),
        tone: if t.user_enabled { ConnectionsTone::Normal } else { ConnectionsTone::Off }, control: t.control(),
        summary: if t.description.is_empty() { "나초가 대화 중에 고르는 도구예요.".into() } else { first_sentence(&t.description, 240) },
        abilities_title: "", abilities: vec![], empty_abilities: "", facts, notice: notice.map(String::from), scope: None, always_allow: None, can_disconnect: false,
    }
}

pub fn connector_detail(c: &ConnectionRow, tools: &[ToolRow]) -> ConnectionsDetail {
    let abilities = tools.iter().filter(|t| t.source.as_deref() == Some(c.source.as_str())).map(|t| ConnectionsAbility {
        id: t.id.clone(), title: t.title(), on: None, risk: None, risk_text: None,
        note: Some(t.summary_line()).filter(|n| !n.is_empty()),
    }).collect();
    let mut facts = vec![];
    if let Some(a) = c.account_label.as_deref().filter(|a| !a.is_empty()) { facts.push(ConnectionsFact { label: "계정", value: a.to_string() }); }
    if !c.scope_summary.is_empty() { facts.push(ConnectionsFact { label: "권한 범위", value: c.scope_summary.clone() }); }
    let health = c.health();
    let notice = match health {
        ConnectionHealth::NeedsReauth => Some("로그인이 만료됐어요. 휴대폰이나 나초 앱에서 다시 로그인하면 나초가 이 서비스를 다시 쓸 수 있어요."),
        ConnectionHealth::NeedsPermission => Some("이 서비스에 필요한 권한이 아직 없어요. 나초 앱에서 다시 연결해 권한을 허용해 주세요."),
        ConnectionHealth::Disconnected => Some("연결이 끊겼어요. 다시 쓰려면 새로 연결해야 해요."),
        ConnectionHealth::Error => Some("연결에 문제가 있어요. 다시 연결하면 나아질 수 있어요."),
        ConnectionHealth::Off => Some("꺼 두면 나초가 이 서비스를 쓰지 않아요. 연결은 그대로 남아 있어요."),
        ConnectionHealth::Connected | ConnectionHealth::Unknown => c.attention_note(),
    };
    let scope = c.scope_level.as_deref().filter(|l| ["read", "read_write"].contains(l) && health != ConnectionHealth::Disconnected)
        .map(|l| ConnectionsScopeChoice { levels: [("read", "읽기만"), ("read_write", "읽기와 쓰기")], selected: l.to_string() });
    ConnectionsDetail {
        target: ConnectionsRef::Connector(c.id.clone()), title: c.label.clone(), status: c.status_line(), tone: c.tone(), control: c.switch(),
        summary: format!("나초가 {} 서비스를 대신 쓸 수 있게 해 줘요.", c.label),
        abilities_title: "나초가 쓸 수 있는 기능", abilities, empty_abilities: "아직 알려진 기능이 없어요.", facts,
        notice: notice.map(String::from), scope, always_allow: None, can_disconnect: health != ConnectionHealth::Disconnected,
    }
}

pub fn gadget_detail(g: &GadgetSwitchState, input: &ConnectionsInput) -> ConnectionsDetail {
    let always = input.always_allowed.contains(g.gadget);
    let actions = actions_for(g.gadget, input.disabled, input.names);
    let abilities = actions.iter().map(|a| ConnectionsAbility {
        id: a.id.clone(), title: a.title.clone(), on: Some(a.on), risk: Some(a.risk), risk_text: risk_text(a.risk, always), note: None,
    }).collect();
    let always_allow = input.trustable.contains(g.gadget).then(|| ConnectionsAlwaysAllow {
        gadget: g.gadget, on: always,
        note: if always { "내용 전송을 묻지 않고 해요. 끄면 다시 매번 물어봐요." } else { "꺼 두면 내용을 보내기 전에 매번 물어봐요." },
        confirm_text: always_allow_confirm(g.gadget),
    });
    let (status, tone, mut notice) = match g.phase {
        GadgetPhase::On => ("켜짐", ConnectionsTone::Normal, None),
        GadgetPhase::Off => ("꺼짐", ConnectionsTone::Off, Some("켜면 아래 일을 나초가 이 PC에서 할 수 있어요.".to_string())),
        GadgetPhase::Failed => ("시작하지 못했어요", ConnectionsTone::Attention, None),
    };
    if !input.allow_combo_control { notice = Some("나초가 이 PC를 쓰지 못하게 꺼 두었어요. 설정에서 다시 켤 수 있어요.".into()); }
    ConnectionsDetail {
        target: ConnectionsRef::Gadget(g.gadget), title: PetGadgetTools::display_name(g.gadget).to_string(), status: status.into(), tone,
        control: g.control(), summary: format!("나초콤보의 {} 가젯이에요.", PetGadgetTools::display_name(g.gadget)),
        abilities_title: "이 PC에서 나초가 하는 일", abilities, empty_abilities: "나초가 이 가젯으로 하는 일이 없어요.",
        facts: vec![ConnectionsFact { label: "PC 행동", value: actions_summary(&actions) }], notice, scope: None, always_allow, can_disconnect: false,
    }
}

// MARK: - server calls

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionsFailure {
    Unauthorized,
    /// 409 `tool_locked`.
    Locked,
    /// 404 / 405 / 501: this server does not have the call yet (an older nacho-server).
    NotAvailable,
    NotFound,
    /// 409 `needs_reconsent`: a wider scope needs the provider's consent again.
    NeedsReconsent,
    /// 409 / 400 with a code (the code only, never a body).
    Conflict(Option<String>),
    Server(u16),
    Network,
    BadResponse,
}

impl ConnectionsFailure {
    pub fn korean_message(&self) -> &'static str {
        match self {
            Self::Unauthorized => "로그인이 필요해요.",
            Self::Locked => "나초가 대화하는 데 꼭 필요한 도구라서 끌 수 없어요.",
            Self::NotAvailable => "이 서버는 아직 이 기능을 지원하지 않아요. 서버가 업데이트되면 쓸 수 있어요.",
            Self::NotFound => "대상을 찾지 못했어요. 목록을 새로 불러와 주세요.",
            Self::NeedsReconsent => "더 넓은 권한은 서비스에서 다시 동의해야 해요. 연결을 다시 해 주세요.",
            Self::Conflict(code) => match code.as_deref() {
                Some("version_conflict") | Some("stale_version") => "다른 곳에서 먼저 바뀌었어요. 목록을 새로 불러왔어요.",
                _ => "지금은 바꿀 수 없어요. 잠시 뒤 다시 시도해 주세요.",
            },
            Self::Server(_) => "서버가 오류를 돌려줬어요.",
            Self::Network => "서버에 연결하지 못했어요. 네트워크를 확인해 주세요.",
            Self::BadResponse => "서버 응답을 읽지 못했어요.",
        }
    }

    /// The list must be read again after these (the row changed elsewhere, or the call did not take).
    pub fn needs_reload(&self) -> bool { matches!(self, Self::Conflict(_) | Self::NotFound | Self::NotAvailable) }

    fn from(status: u16, body: &[u8]) -> Self {
        let reason = ConnectionsAPI::error_reason(body);
        let code = PetServerError::code(body);
        match status {
            401 | 403 => Self::Unauthorized,
            // 404 with a server error body: the item is gone. 404 without one (no such route), 405, 501: an older server.
            404 => if code.is_some() { Self::NotFound } else { Self::NotAvailable },
            405 | 501 => Self::NotAvailable,
            409 => {
                if reason.as_deref() == Some("tool_locked") { Self::Locked }
                else if code.as_deref() == Some("needs_reconsent") || reason.as_deref() == Some("needs_reconsent") { Self::NeedsReconsent }
                else { Self::Conflict(reason.or(code)) }
            }
            400 => Self::Conflict(reason.or(code)),
            502 => Self::Conflict(Some(reason.or(code).unwrap_or_else(|| "transport".into()))),
            s => Self::Server(s),
        }
    }
}

pub type ConnectionsCompletion<T> = Box<dyn FnOnce(Result<T, ConnectionsFailure>) + Send>;

/// The calls behind the switches. Contracts (nacho-server spec `connections.tsp`, `tools.tsp`):
///   PUT    /api/v1/tools/{id}/switch              {enabled}                   → the updated tool; locked: 409 reason tool_locked
///   PATCH  /api/v1/connections/{id}               {enabled, expected_version} service on / off
///   PATCH  /api/v1/connections/{id}/scope         {level, expected_version}   permission scope
///   DELETE /api/v1/connections/{id}                                           disconnect
pub struct ConnectionsAPI {
    endpoint: ServerEndpoint,
    fetcher: Arc<dyn PetHTTPClient>,
    token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

impl ConnectionsAPI {
    pub fn new(endpoint: ServerEndpoint, fetcher: Arc<dyn PetHTTPClient>, token: Arc<dyn Fn() -> Option<String> + Send + Sync>) -> Self {
        Self { endpoint, fetcher, token }
    }

    fn request(&self, method: &str, path: &str, body: Option<Value>) -> PetHTTPRequest {
        let mut r = PetHTTPRequest::new(method, self.endpoint.rest(path));
        r.headers.insert("Accept".into(), "application/json".into());
        if let Some(b) = (self.token)().filter(|b| !b.is_empty()) { r.headers.insert("Authorization".into(), format!("Bearer {b}")); }
        if let Some(b) = body {
            r.headers.insert("Content-Type".into(), "application/json".into());
            r.body = Some(json::encode(&b).into_bytes());
        }
        r
    }

    /// Path segment from a server id: ids are uuids or sources (`naver_mail`, `custom:12`); anything else is refused.
    pub fn segment(id: &str) -> Option<String> {
        if id.is_empty() || id.chars().count() > 80 || !id.chars().all(|c| c.is_ascii_alphanumeric() || "-_:.".contains(c)) { return None; }
        Some(id.chars().map(|c| if c == ':' { "%3A".to_string() } else { c.to_string() }).collect())
    }

    fn error_reason(data: &[u8]) -> Option<String> { json::object(data)?.obj("error")?.str("reason") }

    pub fn tool_switch_request(&self, id: &str, enabled: bool) -> Option<PetHTTPRequest> {
        Self::segment(id).map(|s| self.request("PUT", &format!("/api/v1/tools/{s}/switch"), Some(json!({"enabled": enabled}))))
    }

    pub fn connection_switch_request(&self, id: &str, enabled: bool, version: i64) -> Option<PetHTTPRequest> {
        Self::segment(id).map(|s| self.request("PATCH", &format!("/api/v1/connections/{s}"), Some(json!({"enabled": enabled, "expected_version": version}))))
    }

    pub fn scope_request(&self, id: &str, level: &str, version: i64) -> Option<PetHTTPRequest> {
        Self::segment(id).map(|s| self.request("PATCH", &format!("/api/v1/connections/{s}/scope"), Some(json!({"level": level, "expected_version": version}))))
    }

    pub fn disconnect_request(&self, id: &str) -> Option<PetHTTPRequest> {
        Self::segment(id).map(|s| self.request("DELETE", &format!("/api/v1/connections/{s}"), None))
    }

    fn run<T: 'static>(&self, request: Option<PetHTTPRequest>, decode: fn(&[u8]) -> Option<T>, done: ConnectionsCompletion<T>) {
        let Some(request) = request else { return done(Err(ConnectionsFailure::NotFound)) };
        self.fetcher.send(request, Box::new(move |r: HTTPResult| match r {
            Err(_) => done(Err(ConnectionsFailure::Network)),
            Ok(resp) if !resp.is_success() => done(Err(ConnectionsFailure::from(resp.status, &resp.body))),
            Ok(resp) => done(decode(&resp.body).ok_or(ConnectionsFailure::BadResponse)),
        }));
    }

    /// The server answers the updated row. A server without `user_enabled` answers a row that looks on: that counts
    /// as not available, never as a switch that took.
    pub fn set_tool_enabled(&self, id: &str, enabled: bool, done: ConnectionsCompletion<ToolRow>) {
        self.run(self.tool_switch_request(id, enabled), Self::decode_tool, Box::new(move |r| {
            done(match r { Ok(row) if !Self::answer_carries_switch(&row, enabled) => Err(ConnectionsFailure::NotAvailable), other => other })
        }));
    }

    pub fn answer_carries_switch(row: &ToolRow, enabled: bool) -> bool { row.user_enabled == enabled }

    pub fn set_connection_enabled(&self, id: &str, enabled: bool, version: i64, done: ConnectionsCompletion<()>) {
        self.run(self.connection_switch_request(id, enabled, version), |_| Some(()), done);
    }

    pub fn set_scope_level(&self, id: &str, level: &str, version: i64, done: ConnectionsCompletion<()>) {
        self.run(self.scope_request(id, level, version), |_| Some(()), done);
    }

    pub fn disconnect(&self, id: &str, done: ConnectionsCompletion<()>) { self.run(self.disconnect_request(id), |_| Some(()), done); }

    fn decode_tool(data: &[u8]) -> Option<ToolRow> {
        let o: Value = serde_json::from_slice(data).ok()?;
        o.as_object()?;
        let wrapped = serde_json::to_vec(&json!({"tools": [o]})).ok()?;
        ConnectorDecoder::tools(&wrapped)?.0.into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::auth::testing::PetStubHTTP;
    use crate::core::pet::connectors::ConnectorDecoder;
    use crate::core::pet::trust::GadgetTrust;
    use std::sync::mpsc;

    fn names() -> Vec<&'static str> { declared_names(false) }

    fn overview() -> ConnectorOverview {
        let (tools, _) = ConnectorDecoder::tools(r#"{"tools":[
            {"id":"t1","name":"search_web","search_description":"웹을 검색해요. 두 번째 문장.","config":{"kind":"builtin"},"locked":true},
            {"id":"t2","name":"read_page","search_description":"Reads a page","config":{"kind":"http"},"user_enabled":false},
            {"id":"t3","name":"mail_search","config":{"kind":"connection"},"source":"naver_mail"}]}"#.as_bytes()).unwrap();
        let connections = ConnectorDecoder::connections(r#"[
            {"id":"c1","source":"naver_mail","label":"네이버 메일","status":"connected","account_label":"me@example.com","version":3,"granted_scope":{"level":"read"}},
            {"id":"c2","source":"google_calendar","label":"Google 캘린더","status":"needs_reauth"},
            {"id":"c3","source":"notion","label":"Notion","status":"connected","enabled":false},
            {"id":"c4","source":"gh","label":"GitHub","status":"disconnected"}]"#.as_bytes()).unwrap();
        ConnectorOverview { connections, tools, capabilities: vec![] }
    }

    fn gadgets(on: &[ModuleId]) -> Vec<GadgetSwitchState> {
        GadgetSwitchState::listed().into_iter().map(|(module, gadget)| GadgetSwitchState { module, gadget, phase: if on.contains(&module) { GadgetPhase::On } else { GadgetPhase::Off } }).collect()
    }

    struct Fixture { gadgets: Vec<GadgetSwitchState>, disabled: BTreeSet<String>, always: BTreeSet<String>, trustable: BTreeSet<String>, names: Vec<&'static str>, overview: Option<ConnectorOverview>, control: bool }

    impl Fixture {
        fn new() -> Self {
            Self { gadgets: gadgets(&[ModuleId::Clipboard]), disabled: BTreeSet::new(), always: BTreeSet::new(),
                trustable: GadgetTrust::for_windows(false).trustable().clone(), names: names(), overview: Some(overview()), control: true }
        }
        fn input(&self) -> ConnectionsInput<'_> {
            ConnectionsInput { overview: self.overview.as_ref(), gadgets: &self.gadgets, disabled: &self.disabled, always_allowed: &self.always,
                trustable: &self.trustable, names: &self.names, allow_combo_control: self.control }
        }
    }

    #[test]
    fn three_sections_in_a_fixed_order() {
        let f = Fixture::new();
        let s = sections(&f.input(), "");
        assert_eq!(s.iter().map(|s| s.section).collect::<Vec<_>>(), ConnectionsSection::ALL.to_vec());
        // Tools that belong to a connected service are listed in the service's detail, not here.
        assert_eq!(s[0].rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["search web", "read page"]);
        assert_eq!(s[1].rows.len(), 4);
        // No server list yet: the tool and connector sections are empty and the gadgets still show.
        let mut f = Fixture::new();
        f.overview = None;
        let s = sections(&f.input(), "");
        assert!(s[0].rows.is_empty() && s[1].rows.is_empty() && !s[2].rows.is_empty());
    }

    #[test]
    fn a_locked_tool_is_blocked_and_an_off_tool_reads_off() {
        let f = Fixture::new();
        let rows = &sections(&f.input(), "")[0].rows;
        assert_eq!(rows[0].control, ConnectionsSwitch::Locked);
        assert!(rows[0].control.is_on() && !rows[0].control.can_toggle());
        assert_eq!(rows[0].status, "기본 도구 · 항상 켜져 있어요");
        assert_eq!(rows[1].control, ConnectionsSwitch::Off);
        assert_eq!((rows[1].status.as_str(), rows[1].tone), ("꺼짐", ConnectionsTone::Off));
        let d = detail(&ConnectionsRef::Tool("t1".into()), &f.input()).unwrap();
        assert_eq!(d.notice.as_deref(), Some("나초가 대화하는 데 꼭 필요한 기본 도구라서 끌 수 없어요."));
    }

    #[test]
    fn connection_health_and_status_line() {
        let o = overview();
        let [c1, c2, c3, c4] = [&o.connections[0], &o.connections[1], &o.connections[2], &o.connections[3]];
        assert_eq!(c1.health(), ConnectionHealth::Connected);
        assert_eq!(c1.status_line(), "연결됨 · me@example.com");
        assert_eq!(c2.health(), ConnectionHealth::NeedsReauth);
        assert_eq!(c2.tone(), ConnectionsTone::Attention);
        // A switched-off service reads 꺼짐 whatever else it needs; a disconnected one stays disconnected.
        assert_eq!(c3.health(), ConnectionHealth::Off);
        assert_eq!(c4.health(), ConnectionHealth::Disconnected);
        assert_eq!(c4.switch(), ConnectionsSwitch::Off);
        let mut expiring = c1.clone();
        expiring.needs_attention = Some("expiring".into());
        assert_eq!(expiring.status_line(), "연결됨 · 곧 다시 로그인해야 해요 · me@example.com");
        assert_eq!(expiring.tone(), ConnectionsTone::Attention);
        let mut off_and_reauth = c2.clone();
        off_and_reauth.enabled = false;
        assert_eq!(off_and_reauth.health(), ConnectionHealth::Off);
    }

    #[test]
    fn a_connector_detail_has_scope_and_a_disconnect_only_while_connected() {
        let f = Fixture::new();
        let d = detail(&ConnectionsRef::Connector("c1".into()), &f.input()).unwrap();
        assert_eq!(d.scope, Some(ConnectionsScopeChoice { levels: [("read", "읽기만"), ("read_write", "읽기와 쓰기")], selected: "read".into() }));
        assert!(d.can_disconnect);
        assert_eq!(d.abilities.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["mail search"], "the service's tools");
        assert_eq!(d.facts, vec![ConnectionsFact { label: "계정", value: "me@example.com".into() }]);
        let gone = detail(&ConnectionsRef::Connector("c4".into()), &f.input()).unwrap();
        assert!(!gone.can_disconnect && gone.scope.is_none());
        assert!(detail(&ConnectionsRef::Connector("nope".into()), &f.input()).is_none());
    }

    #[test]
    fn gadget_rows_summarize_the_actions_from_the_registry() {
        let f = Fixture::new();
        let rows = &sections(&f.input(), "")[2].rows;
        let clip = rows.iter().find(|r| r.target == ConnectionsRef::Gadget("clipboard")).unwrap();
        assert_eq!(clip.status, "PC 행동 3개 모두 허용");
        assert!(clip.control.is_on());
        let dim = rows.iter().find(|r| r.target == ConnectionsRef::Gadget("dim")).unwrap();
        assert_eq!((dim.status.as_str(), dim.tone, dim.control.clone()), ("꺼짐", ConnectionsTone::Off, ConnectionsSwitch::Off));
        // The pet itself is never listed: Nacho cannot switch it, and the tab has no switch for it either.
        assert!(!rows.iter().any(|r| r.title == ModuleId::NachoPet.name()));
        let mut f = Fixture::new();
        f.gadgets[0].phase = GadgetPhase::Failed;
        let first = &sections(&f.input(), "")[2].rows[0];
        assert_eq!((first.status.as_str(), first.tone), ("시작하지 못했어요", ConnectionsTone::Attention));
    }

    /// The tab reads the registry: every declared tool shows up under its gadget, and a tool added to the list shows up
    /// with no other change (here a made-up name that the registry knows how to find via a fake list).
    #[test]
    fn every_declared_tool_is_listed_under_its_gadget() {
        let n = names();
        let mut seen = 0;
        for (_, g) in GadgetSwitchState::listed() { seen += actions_for(g, &BTreeSet::new(), &n).len(); }
        // Nacho Combo's own tools (switch a gadget, workspaces) belong to no gadget row, as on macOS.
        let own = n.iter().filter(|t| PetGadgetTools::gadget(t) == Some("combo")).count();
        assert_eq!(own, 5);
        assert_eq!(seen + own, n.len(), "no declared tool is missing from the gadget lists");
        // The handoff tools appear after the last tool of their gadget once the shell has the handoff.
        let with = declared_names(true);
        assert_eq!(with.len(), n.len() + 2);
        let shelf: Vec<String> = actions_for("shelf", &BTreeSet::new(), &with).into_iter().map(|a| a.id).collect();
        assert_eq!(shelf, ["combo_shelf_list", "combo_shelf_give"]);
    }

    #[test]
    fn an_action_without_a_label_shows_the_first_sentence_of_its_description() {
        assert!(action_label("combo_unheard_of").is_none());
        // Every label belongs to a tool the registry knows (a stale label is a mistake).
        for name in PetGadgetTools::WINDOWS.iter().chain(PetGadgetTools::WITH_CHAT_HANDOFF.iter()) {
            assert!(action_label(name).is_some(), "label for {name}");
        }
        let d = first_sentence("지금 소리 출력 장치 이름·소리 크기를 봐요. 소리를 바꾸기 전에 확인할 때 써요.", 120);
        assert_eq!(d, "지금 소리 출력 장치 이름·소리 크기를 봐요.");
        assert_eq!(first_sentence("", 10), "");
        assert_eq!(first_sentence("abcdefghij klm", 5), "abcd…");
        assert_eq!(first_sentence("one\ntwo", 50), "one");
    }

    #[test]
    fn risk_text_follows_always_allow_and_destructive_always_asks() {
        assert_eq!(risk_text(PetToolRisk::Read, false), None);
        assert_eq!(risk_text(PetToolRisk::Change, true), None);
        assert_eq!(risk_text(PetToolRisk::ContentOut, false), Some("보내기 전에 매번 물어봐요"));
        assert_eq!(risk_text(PetToolRisk::ContentOut, true), Some("이 가젯은 항상 허용이라 묻지 않고 해요"));
        assert_eq!(risk_text(PetToolRisk::Destructive, true), Some("매번 물어봐요(항상 허용 안 됨)"));
        let mut f = Fixture::new();
        let d = detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap();
        let get = d.abilities.iter().find(|a| a.id == "combo_clipboard_get_item").unwrap();
        assert_eq!((get.risk, get.risk_text), (Some(PetToolRisk::ContentOut), Some("보내기 전에 매번 물어봐요")));
        let list = d.abilities.iter().find(|a| a.id == "combo_clipboard_list").unwrap();
        assert_eq!(list.risk_text, None);
        f.always.insert("clipboard".into());
        let d = detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap();
        assert_eq!(d.abilities.iter().find(|a| a.id == "combo_clipboard_get_item").unwrap().risk_text, Some("이 가젯은 항상 허용이라 묻지 않고 해요"));
        // Quitting an app asks every time whatever the gadget says.
        let app = detail(&ConnectionsRef::Gadget("appswitch"), &f.input()).unwrap();
        assert_eq!(app.abilities.iter().find(|a| a.id == "combo_quit_app").unwrap().risk_text, Some("매번 물어봐요(항상 허용 안 됨)"));
    }

    #[test]
    fn always_allow_is_offered_only_for_gadgets_with_content_out_tools_and_starts_off() {
        let mut f = Fixture::new();
        let clip = detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap();
        let aa = clip.always_allow.unwrap();
        assert!(!aa.on, "off by default");
        assert!(aa.confirm_text.contains("Clipboard History") && aa.confirm_text.contains("되돌릴 수 없는 행동은 계속 매번 물어봐요"));
        assert!(detail(&ConnectionsRef::Gadget("dim"), &f.input()).unwrap().always_allow.is_none(), "dim has no content_out tool");
        assert!(detail(&ConnectionsRef::Gadget("appswitch"), &f.input()).unwrap().always_allow.is_none(), "quit is destructive, never trusted");
        f.always.insert("clipboard".into());
        assert!(detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap().always_allow.unwrap().on);
    }

    #[test]
    fn a_switched_off_action_shows_off_and_lowers_the_summary() {
        let mut f = Fixture::new();
        f.disabled.insert("combo_clipboard_get_item".into());
        let d = detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap();
        assert_eq!(d.abilities.iter().filter(|a| a.on == Some(false)).count(), 1);
        assert_eq!(d.facts[0].value, "PC 행동 3개 중 2개 허용");
        // A legacy name and its alias are the same action.
        let mut f = Fixture::new();
        f.disabled.insert("combo_appswitch_quit".into());
        let d = detail(&ConnectionsRef::Gadget("appswitch"), &f.input()).unwrap();
        assert_eq!(d.abilities.iter().find(|a| a.id == "combo_quit_app").unwrap().on, Some(false));
    }

    #[test]
    fn nacho_control_off_is_said_on_every_gadget_detail() {
        let mut f = Fixture::new();
        f.control = false;
        let d = detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap();
        assert_eq!(d.notice.as_deref(), Some("나초가 이 PC를 쓰지 못하게 꺼 두었어요. 설정에서 다시 켤 수 있어요."));
        f.control = true;
        assert!(detail(&ConnectionsRef::Gadget("clipboard"), &f.input()).unwrap().notice.is_none());
    }

    #[test]
    fn search_keeps_the_sections_with_a_match() {
        let f = Fixture::new();
        let s = sections(&f.input(), "  GOOGLE ");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].section, ConnectionsSection::Connectors);
        assert_eq!(s[0].rows.len(), 1);
        assert_eq!(sections(&f.input(), "clipboard")[0].section, ConnectionsSection::Gadgets);
        assert!(sections(&f.input(), "zzzz").is_empty());
    }

    // ---- server calls ----

    fn api(f: &Arc<PetStubHTTP>) -> ConnectionsAPI {
        ConnectionsAPI::new(ServerEndpoint::parse("http://127.0.0.1:8787").unwrap(), f.clone(), Arc::new(|| Some("mock-t".into())))
    }

    fn wait<T: Send + 'static>(start: impl FnOnce(ConnectionsCompletion<T>)) -> Result<T, ConnectionsFailure> {
        let (tx, rx) = mpsc::channel();
        start(Box::new(move |r| tx.send(r).unwrap()));
        rx.recv().unwrap()
    }

    #[test]
    fn request_bytes_match_the_contract() {
        let f = PetStubHTTP::new();
        let a = api(&f);
        let r = a.tool_switch_request("t1", false).unwrap();
        assert_eq!((r.method.as_str(), r.url.as_str()), ("PUT", "http://127.0.0.1:8787/api/v1/tools/t1/switch"));
        assert_eq!(r.body.as_deref(), Some(br#"{"enabled":false}"#.as_slice()));
        assert_eq!(r.header("Authorization"), Some("Bearer mock-t"));
        let r = a.connection_switch_request("c1", true, 3).unwrap();
        assert_eq!((r.method.as_str(), r.url.as_str()), ("PATCH", "http://127.0.0.1:8787/api/v1/connections/c1"));
        assert_eq!(r.body.as_deref(), Some(br#"{"enabled":true,"expected_version":3}"#.as_slice()));
        let r = a.scope_request("custom:12", "read_write", 4).unwrap();
        assert_eq!(r.url, "http://127.0.0.1:8787/api/v1/connections/custom%3A12/scope");
        assert_eq!(r.body.as_deref(), Some(br#"{"expected_version":4,"level":"read_write"}"#.as_slice()));
        let r = a.disconnect_request("c1").unwrap();
        assert_eq!((r.method.as_str(), r.body.as_ref()), ("DELETE", None));
        assert!(r.header("Content-Type").is_none());
        // Anything that is not an id never reaches a path.
        for bad in ["", "../x", "a/b", "a b", "a?x=1", "%2e"] { assert!(a.tool_switch_request(bad, true).is_none(), "{bad}"); }
        assert!(a.disconnect_request(&"x".repeat(81)).is_none());
    }

    #[test]
    fn a_tool_switch_answers_the_updated_row_or_a_failure() {
        let f = PetStubHTTP::new();
        let a = api(&f);
        f.on("PUT", "/api/v1/tools/t2/switch", 200, r#"{"id":"t2","name":"read_page","user_enabled":true}"#);
        let row = wait(|d| a.set_tool_enabled("t2", true, d)).unwrap();
        assert!(row.user_enabled);
        // A server that ignores the call answers a row that does not carry the switch: not available, never "took".
        f.on("PUT", "/api/v1/tools/t2/switch", 200, r#"{"id":"t2","name":"read_page"}"#);
        assert_eq!(wait(|d| a.set_tool_enabled("t2", false, d)), Err(ConnectionsFailure::NotAvailable));
        f.on("PUT", "/api/v1/tools/t1/switch", 409, r#"{"error":{"code":"conflict","reason":"tool_locked"}}"#);
        assert_eq!(wait(|d| a.set_tool_enabled("t1", false, d)), Err(ConnectionsFailure::Locked));
        f.on("PUT", "/api/v1/tools/t3/switch", 200, "garbage");
        assert_eq!(wait(|d| a.set_tool_enabled("t3", false, d)), Err(ConnectionsFailure::BadResponse));
    }

    #[test]
    fn failures_are_told_apart_by_status_and_reason_only() {
        let f = PetStubHTTP::new();
        let a = api(&f);
        let go = |status: u16, body: &str| {
            f.on("PATCH", "/api/v1/connections/c1", status, body);
            wait(|d| a.set_connection_enabled("c1", false, 1, d))
        };
        assert_eq!(go(204, ""), Ok(()));
        assert_eq!(go(401, ""), Err(ConnectionsFailure::Unauthorized));
        assert_eq!(go(404, ""), Err(ConnectionsFailure::NotAvailable), "no such route: an older server");
        assert_eq!(go(404, r#"{"error":{"code":"not_found"}}"#), Err(ConnectionsFailure::NotFound));
        assert_eq!(go(405, ""), Err(ConnectionsFailure::NotAvailable));
        assert_eq!(go(409, r#"{"error":{"code":"conflict","reason":"version_conflict"}}"#), Err(ConnectionsFailure::Conflict(Some("version_conflict".into()))));
        assert!(go(409, r#"{"error":{"reason":"stale_version"}}"#).unwrap_err().needs_reload());
        assert_eq!(go(409, r#"{"error":{"code":"needs_reconsent"}}"#), Err(ConnectionsFailure::NeedsReconsent));
        assert_eq!(go(502, ""), Err(ConnectionsFailure::Conflict(Some("transport".into()))));
        assert_eq!(go(500, "secret body"), Err(ConnectionsFailure::Server(500)));
        // The text for a failure never carries the server's body.
        assert!(!go(500, "secret body").unwrap_err().korean_message().contains("secret"));
        f.fail_network.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(wait(|d| a.disconnect("c1", d)), Err(ConnectionsFailure::Network));
    }

    #[test]
    fn scope_and_disconnect_send_the_right_calls() {
        let f = PetStubHTTP::new();
        let a = api(&f);
        f.on("PATCH", "/api/v1/connections/c1/scope", 200, "{}");
        f.on("DELETE", "/api/v1/connections/c1", 204, "");
        assert_eq!(wait(|d| a.set_scope_level("c1", "read_write", 3, d)), Ok(()));
        assert_eq!(wait(|d| a.disconnect("c1", d)), Ok(()));
        assert_eq!(f.sent("PATCH", "/api/v1/connections/c1/scope").len(), 1);
        assert_eq!(f.sent("DELETE", "/api/v1/connections/c1").len(), 1);
    }

    #[test]
    fn the_decoders_read_the_switch_fields_and_default_old_servers_to_on() {
        let o = overview();
        assert!(o.tools[0].locked && o.tools[0].user_enabled);
        assert!(!o.tools[1].user_enabled && !o.tools[1].locked);
        assert!(o.tools[2].user_enabled, "an older server without user_enabled = on");
        assert_eq!((o.connections[0].version, o.connections[0].scope_level.as_deref()), (3, Some("read")));
        assert_eq!((o.connections[1].version, o.connections[1].enabled), (0, true));
        assert!(!o.connections[2].enabled);
    }
}

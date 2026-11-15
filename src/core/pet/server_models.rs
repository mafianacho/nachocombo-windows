//! Nacho Pet ↔ nacho-server: the parts of the server's models the pet shows. Decoded leniently from JSON objects
//! (unknown fields ignored, missing optional fields = None) so a server that adds fields never breaks the pet.
//! Contract: nacho-server spec/rooms.tsp (Message, Attachment, Activity), spec/cards.tsp (CardRequest), spec/tasks.tsp
//! (Task), spec/memory.tsp (MemoryUpdate), spec/usage.tsp (UsageState), spec/client-config.tsp, docs/ws-protocol.md
//! §1.3, docs/error-codes.md. Port of `PetServerModels.swift`.
use super::chat_state::ErrorText;
use super::frames::ActivityKind;
use super::json::{self, JsonExt, JsonObject};
use super::media::{PetIDs, PetMedia};
use super::transcript::PetText;

/// A file on a message: uploaded by the user, or made by Nacho (`origin: "agent"`, built-in `write_file`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetFileLink {
    pub attachment_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub mime: Option<String>,
    pub bytes: Option<i64>,
    /// Signed URL (expires, default 600 s) or an external link. None when deleted.
    pub url: Option<String>,
    pub deleted: bool,
    pub by_nacho: bool,
}

impl PetFileLink {
    pub fn decode(o: &JsonObject) -> PetFileLink {
        PetFileLink {
            attachment_id: o.str("attachment_id"),
            name: o.str("name").or_else(|| o.str("kind")).unwrap_or_else(|| "file".into()),
            kind: o.str("kind").unwrap_or_else(|| "file".into()),
            mime: o.str("mime"),
            bytes: o.int("bytes"),
            url: o.str("url"),
            deleted: o.bool("deleted").unwrap_or(false),
            by_nacho: o.str("origin").as_deref() == Some("agent"),
        }
    }
}

/// A committed room message (`message` event, history, delta). `text` is the server's display text.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatMessage {
    pub id: String,
    pub seq: Option<i64>,
    /// user | assistant | system | tool
    pub role: String,
    pub sender_kind: Option<String>,
    pub text: String,
    pub client_msg_id: Option<String>,
    /// text | tool_calls | tool_result | card | system_note
    pub content_type: String,
    /// system_note kind (handoff, voice_call, page_created, auto_approved, plan …)
    pub note_kind: Option<String>,
    /// A short "still working on it" line (ADR-0021): shown as progress, folded in the record.
    pub interim: bool,
    pub task_result: bool,
    pub task_id: Option<String>,
    pub files: Vec<PetFileLink>,
    pub used_memory_ids: Vec<String>,
    pub card: Option<PetCard>,
    pub created_at: Option<String>,
}

impl ChatMessage {
    pub fn new(id: &str, seq: Option<i64>, role: &str, text: &str) -> Self {
        Self { id: id.into(), seq, role: role.into(), text: text.into(), content_type: "text".into(), ..Default::default() }
    }

    pub fn decode(m: &JsonObject) -> ChatMessage {
        let content = m.obj("content").unwrap_or_default();
        let mut msg = ChatMessage::new(&m.str("id").unwrap_or_default(), m.int("seq"), &m.str("role").unwrap_or_default(), &m.str("text").unwrap_or_default());
        msg.client_msg_id = m.str("client_msg_id");
        msg.content_type = content.str("type").unwrap_or_else(|| "text".into());
        if msg.text.is_empty() && (msg.content_type == "text" || msg.content_type == "system_note") {
            msg.text = content.str("text").unwrap_or_default();
        }
        msg.sender_kind = m.str("sender_kind");
        msg.note_kind = content.str("kind");
        msg.interim = m.bool("interim").unwrap_or(false);
        msg.task_result = m.bool("task_result").unwrap_or(false);
        msg.task_id = m.str("task_id");
        msg.files = m.objs("attachments").iter().map(PetFileLink::decode).collect();
        if let Some(a) = m.obj("attachment") {
            if let Some(name) = a.str("name") {
                msg.files.push(PetFileLink {
                    attachment_id: a.str("file_id"),
                    name,
                    kind: a.str("kind").unwrap_or_else(|| "page".into()),
                    mime: None,
                    bytes: None,
                    url: None,
                    deleted: a.bool("deleted").unwrap_or(false),
                    by_nacho: true,
                });
            }
        }
        msg.used_memory_ids = m.strs("used_memory_ids");
        msg.card = m.obj("card").map(|c| PetCard::decode(&c));
        msg.created_at = m.str("created_at");
        msg
    }

    /// Shown in the transcript (tool messages and empty card shells are not).
    pub fn is_visible(&self) -> bool {
        match self.content_type.as_str() {
            "tool_calls" | "tool_result" => false,
            _ => !(self.text.is_empty() && self.files.is_empty() && self.card.is_none()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetCardOption {
    pub id: String,
    pub label: String,
    /// Why Nacho offers it (choice / suggestion `reason`), shown under the label.
    pub reason: Option<String>,
}

impl PetCardOption {
    pub fn new(id: &str, label: &str) -> Self { Self { id: id.into(), label: label.into(), reason: None } }
}

/// A card (approval, choice, login, OTP …). The pet answers approval, choice, suggestions and rule proposals; the rest
/// are pointed to the phone app.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetCard {
    pub id: String,
    pub kind: String,
    /// pending | answered | dismissed | expired | superseded
    pub status: String,
    pub version: i64,
    pub task_id: Option<String>,
    pub title: String,
    pub detail: Option<String>,
    pub options: Vec<PetCardOption>,
    pub multiple: bool,
    pub answered_action: Option<String>,
    pub expires_at: Option<String>,
    /// Approval: what kind of action (payment, booking, send_message, phone_call, publish, change_permission, delete, other).
    pub action_type: Option<String>,
    /// Approval: can_cancel | conditional | cannot_cancel, and the server's one-line note about undoing it.
    pub reversibility: Option<String>,
    pub reversibility_text: Option<String>,
    /// The options the stored answer chose (`answer.option_id` / `answer.option_ids`), to mark them after answering.
    pub answered_option_ids: Vec<String>,
    /// Rule proposal: what the user said in the chat (the rule is made only by a tap or a number).
    pub said_text: Option<String>,
    /// Approval: the allow rules the server offers with 실행 (`payload.rule_options`: id + `summary`). Empty = none.
    pub rule_options: Vec<PetCardOption>,
    /// Approval: the rule the stored answer created (`answer.allow_rule.rule_option_id`), to mark it after answering.
    pub answered_rule_id: Option<String>,
}

impl PetCard {
    /// A pending card with the given options (tests, previews).
    pub fn new(id: &str, kind: &str, title: &str, options: Vec<PetCardOption>) -> Self {
        Self { id: id.into(), kind: kind.into(), status: "pending".into(), title: title.into(), options, ..Default::default() }
    }

    pub fn decode(o: &JsonObject) -> PetCard {
        let p = o.obj("payload").unwrap_or_default();
        let kind = o.str("kind").unwrap_or_default();
        let mut detail: Vec<String> = vec![];
        if let Some(m) = p.str("merchant") { detail.push(m); }
        if let Some(a) = p.int("amount") { detail.push(PetText::amount(a, p.str("currency").as_deref())); }
        if let Some(c) = p.str("counterpart") { detail.push(c); }
        // The approval card says how to undo it on its own line (PetCardRules); other cards keep it in the detail.
        if kind != "approval" {
            if let Some(r) = p.str("reversibility_text") { detail.push(r); }
        }
        // A rule proposal's choices are its rule options (`summary` is the sentence the user reads).
        let raw = if kind == "rule_proposal" { p.objs("rule_options") } else { p.objs("options") };
        let options = raw
            .iter()
            .filter_map(|opt| {
                let id = opt.str("id")?;
                let label = opt.str("label").or_else(|| opt.str("summary"))?;
                Some(PetCardOption { id, label, reason: opt.str("reason") })
            })
            .collect();
        let answer = o.obj("answer").unwrap_or_default();
        let mut chosen = answer.strs("option_ids");
        if let Some(c) = answer.str("option_id") { chosen.push(c); }
        let title = p.str("title").or_else(|| p.str("question")).unwrap_or_else(|| {
            if kind == "rule_proposal" { "이 범위는 묻지 않고 진행할까요?".into() } else { kind.clone() }
        });
        let mut card = PetCard {
            id: o.str("id").or_else(|| o.str("request_id")).unwrap_or_default(),
            kind: kind.clone(),
            status: o.str("status").unwrap_or_else(|| "pending".into()),
            version: o.int("version").unwrap_or(0),
            task_id: o.str("task_id"),
            title,
            detail: if detail.is_empty() { None } else { Some(detail.join(" · ")) },
            options,
            multiple: p.bool("multiple").unwrap_or(false),
            answered_action: o.str("answered_action"),
            expires_at: o.str("expires_at"),
            action_type: p.str("action_type"),
            reversibility: p.str("reversibility"),
            reversibility_text: p.str("reversibility_text"),
            answered_option_ids: chosen,
            said_text: p.str("said_text"),
            rule_options: vec![],
            answered_rule_id: None,
        };
        if kind == "approval" {
            card.rule_options = p
                .objs("rule_options")
                .iter()
                .filter_map(|opt| {
                    let id = opt.str("id")?;
                    let s = opt.str("summary").filter(|s| !s.is_empty())?;
                    Some(PetCardOption::new(&id, &s))
                })
                .collect();
            card.answered_rule_id = answer.obj("allow_rule").and_then(|a| a.str("rule_option_id"));
        }
        card
    }

    pub fn is_pending(&self) -> bool { self.status == "pending" }

    /// The pet answers approvals (실행 / 거절), choices (one or several), suggestions and rule proposals itself. Cards that
    /// need a value typed in (login, OTP, security code, payment card), a browser or a service connection stay on the
    /// phone app.
    pub fn answerable_here(&self) -> bool { matches!(self.kind.as_str(), "approval" | "choice" | "suggestions" | "rule_proposal") }

    pub fn status_text(&self) -> String {
        match self.status.as_str() {
            "answered" => match self.answered_action.as_deref() {
                Some("approve") => "승인했어요",
                Some("submit") => "보냈어요",
                Some("cancel") => "취소했어요",
                Some("decline") | Some("deny") => "거절했어요",
                Some("choose") => "골랐어요",
                _ => "답했어요",
            },
            "dismissed" => "닫았어요",
            "expired" => "시간이 지나 만료됐어요",
            "superseded" => "새 요청으로 바뀌었어요",
            _ => {
                if self.kind == "approval" {
                    "실행할지 답해 주세요"
                } else if self.answerable_here() {
                    "골라 주세요"
                } else {
                    "휴대폰의 나초 앱에서 이어서 해 주세요"
                }
            }
        }
        .into()
    }
}

/// A task's state (`task_update`): what Nacho is doing outside a plain answer.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetTask {
    pub id: String,
    /// planned queued running waiting done failed interrupted canceled
    pub state: String,
    pub title: String,
    pub step: Option<String>,
    pub wait_reason: Option<String>,
    pub pending_request_id: Option<String>,
    pub failure: Option<String>,
    /// The estimate the server compares a start against (`POST /tasks/{id}/start {expected_estimate_id}`).
    pub estimate_id: Option<String>,
    pub estimate_min: Option<i64>,
    pub estimate_max: Option<i64>,
    /// Server version of the task; a lower one never overwrites a higher one (0 = not given).
    pub version: i64,
}

impl PetTask {
    pub fn new(id: &str, state: &str, title: &str) -> Self { Self { id: id.into(), state: state.into(), title: title.into(), ..Default::default() } }

    pub fn decode(o: &JsonObject) -> PetTask {
        let step = o.obj("current_step");
        let mut step_text = step.as_ref().and_then(|s| s.str("text"));
        if let (Some(s), Some(st)) = (step_text.clone(), step.as_ref()) {
            if let (Some(i), Some(n)) = (st.int("index"), st.int("total")) {
                if n > 0 { step_text = Some(format!("{s} ({i}/{n})")); }
            }
        }
        let credit = o.obj("credit").unwrap_or_default();
        let estimate = credit.obj("estimate").unwrap_or_default();
        PetTask {
            id: o.str("id").unwrap_or_default(),
            state: o.str("state").unwrap_or_default(),
            title: o.str("title").unwrap_or_else(|| "작업".into()),
            step: step_text,
            wait_reason: o.str("wait_reason"),
            pending_request_id: o.str("pending_request_id"),
            failure: o.obj("failure").map(|f| ErrorText::user_message(&f.str("code").unwrap_or_default(), None, "")),
            // The server compares the start against the task's `estimate_id`; `credit.estimate_id` is the same value.
            estimate_id: o.str("estimate_id").or_else(|| credit.str("estimate_id")),
            estimate_min: estimate.int("estimate_min"),
            estimate_max: estimate.int("estimate_max"),
            version: o.int("version").unwrap_or(0),
        }
    }

    /// A REST body that is a task (has an `id`).
    pub fn decode_data(data: &[u8]) -> Option<PetTask> {
        let o = json::object(data)?;
        o.str("id")?;
        Some(Self::decode(&o))
    }

    pub fn is_open(&self) -> bool { matches!(self.state.as_str(), "planned" | "queued" | "running" | "waiting") }

    /// A big task the server holds until the user starts it (plan_mode full): [시작] / [취소].
    pub fn needs_start_confirm(&self) -> bool { self.state == "planned" }

    /// "약 40~60크레딧" (None without an estimate).
    pub fn estimate_text(&self) -> Option<String> {
        let hi = self.estimate_max.filter(|h| *h > 0)?;
        let lo = self.estimate_min.unwrap_or(hi);
        Some(if lo == hi { format!("약 {}크레딧", PetText::number(hi)) } else { format!("약 {}~{}크레딧", PetText::number(lo), PetText::number(hi)) })
    }

    pub fn status_text(&self) -> String {
        match self.state.as_str() {
            "planned" => "시작 확인 필요".into(),
            "queued" => "곧 시작해요".into(),
            "running" => self.step.clone().unwrap_or_else(|| "하고 있어요".into()),
            "waiting" => match self.wait_reason.as_deref() {
                Some("approval") => "승인을 기다려요",
                Some("input") => "답을 기다려요",
                Some("login") | Some("connection") | Some("browser") => "휴대폰 앱에서 연결·로그인이 필요해요",
                Some("over_cap") | Some("resume_confirm") => "크레딧이 채워지면 이어서 해요",
                _ => "기다리고 있어요",
            }
            .into(),
            "done" => "끝냈어요".into(),
            "failed" => self.failure.clone().unwrap_or_else(|| "하지 못했어요".into()),
            "interrupted" => "중간에 멈췄어요 (쓴 크레딧은 돌려받아요)".into(),
            "canceled" => "취소했어요".into(),
            s => s.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetMemoryItem {
    pub memory_id: String,
    /// fact | episode
    pub kind: String,
    pub preview: String,
}

/// "기억했어요" (`memory_update`). One line per `update_id`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PetMemoryNote {
    pub update_id: String,
    /// created | updated | forgotten | restored
    pub action: String,
    pub items: Vec<PetMemoryItem>,
    pub source: String,
    pub visibility: Option<String>,
    pub after_seq: Option<i64>,
    /// The turn of this session it came with (None = hub / another device).
    pub turn_client_id: Option<String>,
    /// This session asked the server to forget it (undo possible for 10 s). Unix seconds.
    pub forgotten_at: Option<f64>,
}

impl PetMemoryNote {
    pub fn decode(o: &JsonObject) -> PetMemoryNote {
        let mut items: Vec<PetMemoryItem> = o
            .objs("items")
            .iter()
            .filter_map(|i| {
                Some(PetMemoryItem {
                    memory_id: i.str("memory_id")?,
                    kind: i.str("kind").unwrap_or_else(|| "fact".into()),
                    preview: i.str("preview").unwrap_or_default(),
                })
            })
            .collect();
        if items.is_empty() {
            let kind = o.str("kind").unwrap_or_else(|| "fact".into());
            items = o
                .strs("memory_ids")
                .into_iter()
                .map(|id| PetMemoryItem { memory_id: id, kind: kind.clone(), preview: o.str("preview").unwrap_or_default() })
                .collect();
        }
        PetMemoryNote {
            update_id: o.str("update_id").unwrap_or_else(PetIDs::new_uuid),
            action: o.str("action").unwrap_or_else(|| "created".into()),
            items,
            source: o.str("source").unwrap_or_else(|| "chat".into()),
            visibility: o.str("visibility"),
            after_seq: o.int("after_seq"),
            turn_client_id: None,
            forgotten_at: None,
        }
    }

    pub fn text(&self) -> String {
        let previews = self.items.iter().map(|i| i.preview.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
        match self.action.as_str() {
            "forgotten" => {
                if previews.is_empty() { "잊었어요".into() } else { format!("잊었어요: {previews}") }
            }
            "restored" => format!("다시 기억해요: {previews}"),
            "updated" => format!("기억을 고쳤어요: {previews}"),
            _ => format!("기억했어요: {previews}"),
        }
    }
}

/// Usage in credits (`usage` event and `GET /api/v1/me/usage`). Read only: buying is on the phone app.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetUsage {
    pub remaining: i64,
    pub available_to_start: Option<i64>,
    pub block_reason: Option<String>,
    pub light_chat_used: Option<i64>,
    pub light_chat_cap: Option<i64>,
    pub plan_id: Option<String>,
    pub allowance: Option<i64>,
    pub period: Option<String>,
    pub reset_at: Option<String>,
    pub exhausted: bool,
    pub low_balance: bool,
}

impl PetUsage {
    /// From `UsageState` (REST) or the `usage` event (which carries the state too).
    pub fn decode(o: &JsonObject) -> Option<PetUsage> {
        let s = o.obj("state").unwrap_or_else(|| o.clone());
        let remaining = o.int("remaining").or_else(|| s.int("remaining"))?;
        let mut light = (o.int("light_chat_used"), o.int("light_chat_cap"));
        if light.0.is_none() {
            if let Some(l) = s.objs("limits").into_iter().find(|l| l.str("kind").as_deref() == Some("light_chat")) {
                light = (l.int("used"), l.int("cap"));
            }
        }
        Some(PetUsage {
            remaining,
            available_to_start: o.int("available_to_start").or_else(|| s.int("available_to_start")),
            block_reason: o.str("block_reason").or_else(|| s.str("block_reason")),
            light_chat_used: light.0,
            light_chat_cap: light.1,
            plan_id: s.str("plan_id"),
            allowance: s.int("allowance"),
            period: s.str("period"),
            reset_at: s.str("reset_at"),
            exhausted: s.bool("exhausted").unwrap_or(false),
            low_balance: s.bool("low_balance").unwrap_or(false),
        })
    }

    pub fn decode_data(data: &[u8]) -> Option<PetUsage> { Self::decode(&json::object(data)?) }

    pub fn summary(&self) -> String {
        let n = PetText::number;
        let mut parts: Vec<String> = vec![];
        match self.allowance {
            Some(a) if a > 0 => parts.push(format!("크레딧 {}/{}", n(self.remaining), n(a))),
            _ => parts.push(format!("크레딧 {}", n(self.remaining))),
        }
        if let (Some(u), Some(c)) = (self.light_chat_used, self.light_chat_cap) {
            if c > 0 { parts.push(format!("가벼운 대화 {}/{}", n(u), n(c))); }
        }
        if let Some(r) = &self.block_reason {
            parts.push(ErrorText::block_reason(r));
        } else if self.exhausted {
            parts.push("모두 썼어요".into());
        } else if self.low_balance {
            parts.push("얼마 안 남았어요".into());
        }
        parts.join(" · ")
    }
}

/// `GET /client/config` (no sign-in): what the server offers right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetClientConfig {
    pub dictation_enabled: bool,
    pub dictation_max_sec: i64,
    pub dictation_max_bytes: i64,
    pub media_max_bytes: i64,
    pub service_state: String,
    pub service_message: Option<String>,
}

impl PetClientConfig {
    pub fn fallback() -> Self {
        Self {
            dictation_enabled: false,
            dictation_max_sec: 60,
            dictation_max_bytes: 2_000_000,
            media_max_bytes: PetMedia::MAX_UPLOAD_BYTES as i64,
            service_state: "ok".into(),
            service_message: None,
        }
    }

    pub fn decode(data: &[u8]) -> Option<PetClientConfig> {
        let o = json::object(data)?;
        let d = o.obj("dictation").unwrap_or_default();
        let status = o.obj("service_status").unwrap_or_default();
        Some(PetClientConfig {
            dictation_enabled: d.bool("enabled").unwrap_or(false),
            dictation_max_sec: d.int("max_sec").unwrap_or(60).max(1),
            dictation_max_bytes: d.int("max_bytes").unwrap_or(2_000_000).max(1),
            media_max_bytes: o.int("media_max_bytes").unwrap_or(PetMedia::MAX_UPLOAD_BYTES as i64).max(1),
            service_state: status.str("state").unwrap_or_else(|| "ok".into()),
            service_message: status.str("message"),
        })
    }

    pub fn service_notice(&self) -> Option<String> {
        let tail = self.service_message.as_ref().map(|m| format!(" {m}")).unwrap_or_default();
        match self.service_state.as_str() {
            "maintenance" => Some(format!("나초 서버 점검 중이에요.{tail}")),
            "service_ended" => Some(format!("나초 서비스가 끝났어요.{tail}")),
            "degraded" => Some("나초 서버가 느릴 수 있어요.".into()),
            _ => None,
        }
    }
}

/// The 11 activity tool keys the app shows (spec/rooms.tsp ActivityTool), in Korean.
pub struct ActivityText;

impl ActivityText {
    pub fn label(kind: ActivityKind, tool: Option<&str>) -> &'static str {
        match kind {
            ActivityKind::Thinking => "생각하는 중",
            ActivityKind::Typing => "답하는 중",
            ActivityKind::Retrying => "다시 해 보는 중",
            ActivityKind::Idle => "쉬는 중",
            ActivityKind::Tool => match tool {
                Some("search_web") => "웹에서 찾는 중",
                Some("read_page") => "페이지 읽는 중",
                Some("browse_web") => "웹을 둘러보는 중",
                Some("remember") => "기억을 떠올리는 중",
                Some("read_files") => "자료 읽는 중",
                Some("write_files") => "파일 만드는 중",
                Some("run_command") => "명령 실행하는 중",
                Some("look_camera") => "사진 보는 중",
                Some("delegate") => "다른 나초에게 맡기는 중",
                Some("guide") => "안내 준비하는 중",
                Some("stop") => "멈추는 중",
                _ => "도구 쓰는 중",
            },
        }
    }
}

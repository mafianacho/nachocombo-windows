//! The pure chat reducer: every server frame goes through [`ChatState::apply`], so the UI and the dev CLI render the
//! same state. Extended with the room record (history/delta), tool progress, cards, tasks, memory notes and usage
//! (nacho-server docs/ws-protocol.md §1.3, §1.6). Port of `PetChatState.swift`.
use super::clock;
use super::frames::{ActivityKind, ServerEvent, ServerFrame};
use super::json;
use super::json::JsonExt;
use super::server_models::{ChatMessage, PetCard, PetFileLink, PetMemoryNote, PetTask, PetUsage};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConnectionPhase {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting { attempt: u32 },
    AuthFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TurnStatus {
    /// Frame sent, no `turn_started` yet.
    #[default]
    Sending,
    Streaming,
    Done,
    Stopped,
    /// Korean user-facing reason.
    Failed(String),
    /// The connection dropped before `done`.
    Interrupted,
}

/// One tool call of a turn (`tool_call_started` / `tool_call_finished`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolStep {
    pub call_id: String,
    pub name: String,
    pub finished: bool,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChatTurn {
    pub client_msg_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub status: TurnStatus,
    pub stop_requested: bool,
    /// File names of the pictures/files sent with this turn (shown under the user's text).
    pub attachment_names: Vec<String>,
    /// Seq of the stored user message / Nacho's answer (from `message` events).
    pub user_seq: Option<i64>,
    pub answer_seq: Option<i64>,
    /// "찾아보고 있어요" lines while the turn runs (interim messages, ADR-0021).
    pub progress: Vec<String>,
    pub tools: Vec<ToolStep>,
    /// Files on Nacho's answer (e.g. made with `write_file`).
    pub files: Vec<PetFileLink>,
    /// Memories the answer used (for "this memory is wrong" feedback).
    pub used_memory_ids: Vec<String>,
    /// Cards this turn raised.
    pub card_ids: Vec<String>,
    pub task_id: Option<String>,
}

impl ChatTurn {
    pub fn new(client_msg_id: &str, user_text: &str) -> Self {
        Self { client_msg_id: client_msg_id.into(), user_text: user_text.into(), ..Default::default() }
    }
    pub fn id(&self) -> &str { &self.client_msg_id }
    fn is_running(&self) -> bool { matches!(self.status, TurnStatus::Sending | TurnStatus::Streaming) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatState {
    pub phase: ConnectionPhase,
    pub room_id: Option<String>,
    /// The Nacho this chat talks to (`ready.agent_id`); the connector binding is issued for it.
    pub agent_id: Option<String>,
    pub agent_name: String,
    pub turns: Vec<ChatTurn>,
    pub activity: ActivityKind,
    pub active_tool: Option<String>,
    /// Last non-turn error shown to the user.
    pub notice: Option<String>,
    /// Messages of the room that are not turns of this session: the record before, other devices, tasks.
    pub history: Vec<ChatMessage>,
    /// There are older messages than `history` holds (load with `before_seq`).
    pub has_more_history: bool,
    /// The record was loaded once for this room.
    pub history_loaded: bool,
    /// Cards by id, in arrival order.
    pub cards: Vec<PetCard>,
    pub task: Option<PetTask>,
    /// Every task this chat has heard of (`task_update`, start/cancel answers, lookups for plan notes), by id.
    pub tasks: BTreeMap<String, PetTask>,
    /// [시작] / [취소] pressed and not answered yet, by task id ("start" | "cancel"): the buttons are off meanwhile.
    pub task_actions: BTreeMap<String, String>,
    /// One line under a plan card after a refused start/cancel or a changed estimate, by task id.
    pub task_notices: BTreeMap<String, String>,
    pub usage: Option<PetUsage>,
    pub memory_notes: Vec<PetMemoryNote>,
    /// Last notification worth a line (credits granted, connector events …).
    pub last_notification: Option<String>,
    /// When the last turn ended (`done`/`stopped`), for the pet's "happy" moment. Unix seconds.
    pub last_finished_at: Option<f64>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            phase: ConnectionPhase::Disconnected,
            room_id: None,
            agent_id: None,
            agent_name: "나초".into(),
            turns: vec![],
            activity: ActivityKind::Idle,
            active_tool: None,
            notice: None,
            history: vec![],
            has_more_history: false,
            history_loaded: false,
            cards: vec![],
            task: None,
            tasks: BTreeMap::new(),
            task_actions: BTreeMap::new(),
            task_notices: BTreeMap::new(),
            usage: None,
            memory_notes: vec![],
            last_notification: None,
            last_finished_at: None,
        }
    }
}

impl ChatState {
    pub fn new() -> Self { Self::default() }

    pub fn active_turn_index(&self) -> Option<usize> { self.turns.iter().rposition(ChatTurn::is_running) }
    pub fn is_busy(&self) -> bool { self.active_turn_index().is_some() }

    /// Highest seq seen in this room (for `delta?since_seq=` and the read mark).
    pub fn last_seq(&self) -> i64 {
        let h = self.history.iter().filter_map(|m| m.seq).max().unwrap_or(0);
        let t = self.turns.iter().flat_map(|t| [t.user_seq, t.answer_seq]).flatten().max().unwrap_or(0);
        h.max(t)
    }

    pub fn pending_cards(&self) -> Vec<&PetCard> { self.cards.iter().filter(|c| c.is_pending()).collect() }

    pub fn card(&self, id: &str) -> Option<&PetCard> { self.cards.iter().find(|c| c.id == id) }

    pub fn begin_turn(&mut self, text: &str, client_msg_id: &str, attachment_names: Vec<String>) {
        let mut turn = ChatTurn::new(client_msg_id, text);
        turn.attachment_names = attachment_names;
        self.turns.push(turn);
        self.notice = None;
        if self.task.as_ref().is_some_and(|t| !t.is_open()) { self.task = None; }
    }

    pub fn mark_stop_requested(&mut self) {
        if let Some(i) = self.active_turn_index() { self.turns[i].stop_requested = true; }
    }

    /// Connection dropped: unfinished turns become `interrupted`. The server keeps running them; `recover` fills them in
    /// from the delta after reconnecting.
    pub fn connection_lost(&mut self) {
        for t in self.turns.iter_mut().filter(|t| t.is_running()) { t.status = TurnStatus::Interrupted; }
        self.activity = ActivityKind::Idle;
        self.active_tool = None;
    }

    fn turn_by_client(&self, c: &str) -> Option<usize> { self.turns.iter().rposition(|t| t.client_msg_id == c) }

    fn index_for(&self, client_id: Option<&str>) -> Option<usize> {
        match client_id {
            Some(c) => self.turn_by_client(c), // None here = a turn of another session
            None => self.active_turn_index(),
        }
    }

    pub fn upsert_card(&mut self, card: PetCard) {
        if let Some(i) = self.cards.iter().position(|c| c.id == card.id) {
            if card.version >= self.cards[i].version { self.cards[i] = card; }
        } else {
            self.cards.push(card);
        }
    }

    fn add_history(&mut self, m: ChatMessage) {
        if let Some(c) = &m.card { self.upsert_card(c.clone()); }
        if !m.is_visible() || m.interim { return; }
        if let Some(i) = self.history.iter().position(|h| h.id == m.id) {
            self.history[i] = m;
            return;
        }
        self.history.push(m);
        self.history.sort_by_key(|h| h.seq.unwrap_or(i64::MAX));
    }

    /// A stored message: to the turn of this session it belongs to, or to the room record.
    pub fn take(&mut self, m: ChatMessage, client_id: Option<&str>) {
        if m.role == "user" {
            if let Some(i) = m.client_msg_id.as_deref().and_then(|c| self.turn_by_client(c)) {
                self.turns[i].user_seq = m.seq;
                return;
            }
        }
        // Turns handed to a task get their answer later through the hub, without client_id: match by task_id.
        let by_task = m.task_id.as_deref().and_then(|t| self.turns.iter().rposition(|x| x.task_id.as_deref() == Some(t)));
        let target = client_id.and_then(|c| self.turn_by_client(c)).or(if m.role == "user" { None } else { by_task });
        let Some(i) = target else {
            self.add_history(m);
            return;
        };
        if let Some(c) = &m.card {
            let id = c.id.clone();
            self.upsert_card(c.clone());
            if !self.turns[i].card_ids.contains(&id) { self.turns[i].card_ids.push(id); }
            return;
        }
        if m.interim {
            if !m.text.is_empty() && self.turns[i].progress.last() != Some(&m.text) { self.turns[i].progress.push(m.text); }
            return;
        }
        if m.role != "assistant" || m.content_type != "text" {
            if m.content_type == "system_note" { self.add_history(m); }
            return;
        }
        // The committed answer is authoritative for the final text.
        let t = &mut self.turns[i];
        if !m.text.is_empty() { t.assistant_text = m.text; }
        t.answer_seq = m.seq;
        t.files = m.files;
        t.used_memory_ids = m.used_memory_ids;
        if let Some(task) = m.task_id { t.task_id = Some(task); }
    }

    pub fn apply(&mut self, frame: &ServerFrame) {
        let cid = frame.client_id.as_deref();
        match &frame.event {
            ServerEvent::Ready(info) => {
                if self.room_id.is_some() && self.room_id.as_deref() != Some(info.room_id.as_str()) {
                    self.history.clear();
                    self.history_loaded = false;
                    self.has_more_history = false;
                }
                self.room_id = Some(info.room_id.clone());
                self.agent_id = if info.agent_id.is_empty() { None } else { Some(info.agent_id.clone()) };
                if !info.agent_name.is_empty() { self.agent_name = info.agent_name.clone(); }
                self.phase = ConnectionPhase::Connected;
            }
            ServerEvent::TurnStarted { duplicate, .. } => {
                if let Some(i) = self.index_for(cid) {
                    if self.turns[i].status == TurnStatus::Sending {
                        self.turns[i].status = if *duplicate { TurnStatus::Done } else { TurnStatus::Streaming };
                    }
                    if let Some(t) = &frame.task_id { self.turns[i].task_id = Some(t.clone()); }
                }
            }
            ServerEvent::TextDelta(t) => {
                if let Some(i) = self.index_for(cid) {
                    if self.turns[i].status == TurnStatus::Sending { self.turns[i].status = TurnStatus::Streaming; }
                    self.turns[i].assistant_text.push_str(t);
                }
            }
            ServerEvent::Activity { kind, tool } => {
                self.activity = *kind;
                self.active_tool = if *kind == ActivityKind::Tool { tool.clone() } else { None };
            }
            ServerEvent::Message(m) => self.take(m.clone(), cid),
            ServerEvent::Usage(u) => self.usage = Some(u.clone()),
            ServerEvent::ToolStarted { call_id, name } => {
                if let Some(i) = self.index_for(cid) {
                    if !self.turns[i].tools.iter().any(|t| &t.call_id == call_id) {
                        self.turns[i].tools.push(ToolStep { call_id: call_id.clone(), name: name.clone(), ..Default::default() });
                    }
                }
            }
            ServerEvent::ToolFinished { call_id, name, is_error, .. } => {
                if let Some(i) = self.index_for(cid) {
                    if let Some(step) = self.turns[i].tools.iter_mut().find(|t| &t.call_id == call_id) {
                        step.finished = true;
                        step.is_error = *is_error;
                    } else {
                        self.turns[i].tools.push(ToolStep { call_id: call_id.clone(), name: name.clone(), finished: true, is_error: *is_error });
                    }
                }
            }
            ServerEvent::Card(c) => {
                self.upsert_card(c.clone());
                if let Some(i) = cid.and_then(|c| self.turn_by_client(c)) {
                    if !self.turns[i].card_ids.contains(&c.id) { self.turns[i].card_ids.push(c.id.clone()); }
                }
            }
            ServerEvent::Task(t) => {
                self.apply_task(t.clone(), true);
                if let Some(i) = cid.and_then(|c| self.turn_by_client(c)) {
                    if self.turns[i].task_id.is_none() { self.turns[i].task_id = Some(t.id.clone()); }
                }
            }
            ServerEvent::Memory(note) => {
                let mut note = note.clone();
                note.turn_client_id = frame.client_id.clone();
                self.apply_memory(note);
            }
            ServerEvent::Notification { kind, title, body } => {
                if ["credit_granted", "payment_confirmed", "usage_low"].contains(&kind.as_str()) || kind.starts_with("connector_") {
                    self.last_notification = Some(match body { Some(b) => format!("{title} — {b}"), None => title.clone() });
                }
            }
            ServerEvent::Pong | ServerEvent::Unknown { .. } => {}
            ServerEvent::Done { stopped, .. } => {
                if let Some(i) = self.index_for(cid) {
                    if let Some(t) = &frame.task_id { self.turns[i].task_id = Some(t.clone()); }
                    self.turns[i].status = if *stopped { TurnStatus::Stopped } else { TurnStatus::Done };
                    self.last_finished_at = Some(clock::now());
                }
                self.activity = ActivityKind::Idle;
                self.active_tool = None;
            }
            ServerEvent::Error(e) => {
                let text = ErrorText::user_message(&e.code, e.reason.as_deref(), &e.message);
                let target = self.index_for(cid).or(if frame.room_id.is_some() { self.active_turn_index() } else { None });
                match target {
                    Some(i) => self.turns[i].status = TurnStatus::Failed(text),
                    None => self.notice = Some(text),
                }
                if e.code == "unauthorized" { self.phase = ConnectionPhase::AuthFailed; }
                self.activity = ActivityKind::Idle;
                self.active_tool = None;
            }
        }
    }

    /// A task's new state. `current` (a `task_update`) makes it the header's task; an answer to start/cancel or a lookup
    /// only refreshes the header when it is already about that task. An older version never wins.
    pub fn apply_task(&mut self, t: PetTask, current: bool) {
        if t.id.is_empty() { return; }
        if let Some(old) = self.tasks.get(&t.id) {
            if old.version > 0 && t.version > 0 && t.version < old.version { return; }
        }
        self.tasks.insert(t.id.clone(), t.clone());
        if !t.needs_start_confirm() { self.task_notices.remove(&t.id); }
        if current || self.task.is_none() || self.task.as_ref().map(|x| x.id.as_str()) == Some(t.id.as_str()) { self.task = Some(t); }
    }

    /// The plan note (`system_note` kind `plan`) the server posts for a task it holds until the user starts it.
    pub fn plan_note(&self, task_id: &str) -> Option<&ChatMessage> {
        self.history.iter().rev().find(|m| m.note_kind.as_deref() == Some("plan") && m.task_id.as_deref() == Some(task_id))
    }

    /// A `memory_update`: new lines are added; the user's own forget/undo (echoed by the hub) updates its line.
    pub fn apply_memory(&mut self, incoming: PetMemoryNote) {
        let mut note = incoming;
        if let Some(i) = self.memory_notes.iter().position(|n| n.update_id == note.update_id) {
            note.forgotten_at = self.memory_notes[i].forgotten_at;
            if note.turn_client_id.is_none() { note.turn_client_id = self.memory_notes[i].turn_client_id.clone(); }
            self.memory_notes[i] = note;
        } else if let Some(i) = (note.action == "forgotten" || note.action == "restored")
            .then(|| self.memory_notes.iter().rposition(|old| old.items.iter().any(|o| note.items.iter().any(|n| n.memory_id == o.memory_id))))
            .flatten()
        {
            let restored = note.action == "restored";
            self.memory_notes[i].action = if restored { "created".into() } else { "forgotten".into() };
            if restored { self.memory_notes[i].forgotten_at = None; }
        } else {
            self.memory_notes.push(note);
        }
        if self.memory_notes.len() > 50 {
            let extra = self.memory_notes.len() - 50;
            self.memory_notes.drain(0..extra);
        }
    }

    // MARK: the room record (REST)

    /// A page of `GET /rooms/{id}/history?view=app`. `older` = scrolled up (`before_seq`), else the latest page.
    pub fn merge_history(&mut self, page: Vec<ChatMessage>, has_more: bool, older: bool) {
        let mine: HashSet<String> = self.turns.iter().map(|t| t.client_msg_id.clone()).collect();
        for m in page {
            if m.client_msg_id.as_ref().is_some_and(|c| mine.contains(c)) { continue; }
            // Answers to turns of this session are already shown in the turn.
            if let Some(s) = m.seq {
                if self.turns.iter().any(|t| t.answer_seq == Some(s)) { continue; }
            }
            self.add_history(m);
        }
        if older || !self.history_loaded { self.has_more_history = has_more; }
        self.history_loaded = true;
    }

    /// `GET /rooms/{id}/delta` after reconnecting: interrupted turns get the answer the server finished meanwhile,
    /// everything else goes to the record.
    pub fn recover(&mut self, messages: Vec<ChatMessage>) {
        // The turn of this session whose user message came last. The delta starts after the last seq seen, so when the
        // server's echo of the question arrived before the drop, only the answer is in it: start with that turn.
        let mut current = self.turns.iter().rposition(|t| {
            matches!(t.status, TurnStatus::Interrupted | TurnStatus::Sending) && t.user_seq.is_some() && t.answer_seq.is_none()
        });
        let mut sorted = messages;
        sorted.sort_by_key(|m| m.seq.unwrap_or(0));
        for m in sorted {
            if m.role == "user" {
                if let Some(i) = m.client_msg_id.as_deref().and_then(|c| self.turn_by_client(c)) {
                    self.turns[i].user_seq = m.seq;
                    current = Some(i);
                    continue;
                }
                current = None;
            }
            if let Some(i) = current {
                if m.role != "user" {
                    let was_answer = m.role == "assistant" && !m.interim && m.content_type == "text";
                    let c = self.turns[i].client_msg_id.clone();
                    self.take(m, Some(&c));
                    if was_answer && matches!(self.turns[i].status, TurnStatus::Interrupted | TurnStatus::Sending) {
                        self.turns[i].status = TurnStatus::Done;
                    }
                    continue;
                }
            }
            if let Some(s) = m.seq {
                if self.turns.iter().any(|t| t.answer_seq == Some(s) || t.user_seq == Some(s)) { continue; }
            }
            self.add_history(m);
        }
    }
}

/// Korean messages for server error codes (nacho-server docs/error-codes.md). The pet branches on `code` and `reason`
/// only and never shows the server's English `message`.
pub struct ErrorText;

impl ErrorText {
    pub fn user_message(code: &str, reason: Option<&str>, _fallback: &str) -> String {
        match code {
            "unauthorized" => "로그인이 필요해요.".into(),
            "validation_error" => "입력을 다시 확인해 주세요.".into(),
            "invalid_input" => {
                if reason == Some("reply_to") { "답장할 메시지를 찾지 못했어요.".into() } else { "보낼 수 없는 내용이에요.".into() }
            }
            "bad_frame" => "앱이 잘못된 요청을 보냈어요. 앱을 업데이트해 주세요.".into(),
            "credits_exhausted" => reason.map(Self::block_reason).unwrap_or_else(|| "사용량이 모두 소진됐어요.".into()),
            "task_not_startable" => {
                format!("지금은 일을 맡길 수 없어요. {}", reason.map(Self::block_reason).unwrap_or_else(|| "사용량을 확인해 주세요.".into()))
            }
            "light_chat_limit" => "오늘 가벼운 대화 한도에 도달했어요.".into(),
            "quota_exceeded" => "이번 사용량을 다 썼어요.".into(),
            "phone_verification_required" => "전화번호 인증이 필요해요. 휴대폰의 나초 앱에서 인증해 주세요.".into(),
            "forbidden" => "이 기능은 쓸 수 없어요.".into(),
            "suspended" => "이용이 제한된 계정이에요.".into(),
            "account_deleting" => "삭제 중인 계정이에요.".into(),
            "not_member" => "이 대화방에 들어갈 수 없어요.".into(),
            "not_found" => "보낸 첨부를 서버에서 찾지 못했어요. 다시 붙여 주세요.".into(),
            "conflict" => "이미 처리된 요청이에요.".into(),
            "version_conflict" => "다른 곳에서 먼저 바뀌었어요. 새로 불러올게요.".into(),
            "expired" => "시간이 지나 만료됐어요.".into(),
            "superseded" => "새 요청으로 바뀌었어요.".into(),
            "already_answered" => "이미 답한 요청이에요.".into(),
            "character_unavailable" | "agent_unavailable" => {
                if reason == Some("deleting") { "이 나초는 지워지는 중이에요.".into() } else { "나초가 지금 쉬고 있어요.".into() }
            }
            "connection_required" => "먼저 서비스 연결이 필요해요. 휴대폰의 나초 앱에서 연결해 주세요.".into(),
            "connection_needs_reauth" => "연결한 서비스에 다시 로그인해야 해요.".into(),
            "scope_blocked" => "허락한 범위 밖이라 하지 않았어요.".into(),
            "run_active" => "진행 중인 단톡 일을 먼저 끝내 주세요.".into(),
            "payload_too_large" => "파일이 너무 커요.".into(),
            "unsupported_media_type" => "이 녹음 형식은 받을 수 없어요. 글로 써 주세요.".into(),
            "restore_expired" => "되돌릴 수 있는 시간이 지났어요.".into(),
            "too_many_requests" => match reason.and_then(Self::retry_at) {
                Some(t) => format!("잠시 너무 많이 요청했어요. {t}에 다시 해 주세요."),
                None => "잠시 너무 많이 요청했어요. 잠시 뒤 다시 해 주세요.".into(),
            },
            "internal_error" => "일시적인 문제가 생겼어요. 다시 시도해 주세요.".into(),
            "not_implemented" => "아직 서버에서 준비되지 않은 기능이에요.".into(),
            "upstream_error" | "transport" => "서버가 답을 만들지 못했어요. 잠시 뒤 다시 시도해 주세요.".into(),
            "outcome_unknown" => "보냈는지 확인이 필요해요. 다시 보내기 전에 결과를 확인해 주세요.".into(),
            "service_disabled" => "지금은 쓸 수 없는 기능이에요.".into(),
            "agent_limit" => "만들 수 있는 나초 수를 넘었어요.".into(),
            "payment_error" => "결제가 되지 않았어요. 휴대폰의 나초 앱에서 확인해 주세요.".into(),
            "estimate_changed" => "예상 사용량이 바뀌었어요. 휴대폰의 나초 앱에서 다시 확인해 주세요.".into(),
            // Windows wording of the macOS "이 맥의 나초 콤보 연결이 끊겼어요."
            "connector_unavailable" => "이 PC의 나초 콤보 연결이 끊겼어요.".into(),
            "vault_not_ready" => "금고가 아직 준비되지 않았어요.".into(),
            _ => format!("문제가 생겼어요. ({code})"),
        }
    }

    /// BlockReason (usage.block_reason; the reason of credits_exhausted / task_not_startable).
    pub fn block_reason(r: &str) -> String {
        match r {
            "credits_exhausted" => "크레딧이 모자라요.",
            "heavy_daily_limit" => "오늘 맡길 수 있는 큰 일을 다 썼어요.",
            "free_call_limit" => "오늘 무료 전화를 다 썼어요.",
            "plan_cost_cap" => "이번 달 사용 한도에 닿았어요.",
            "refund_review" => "환급을 확인하는 동안 잠시 멈췄어요.",
            "phone_verification_required" => "전화번호를 인증하면 무료 크레딧을 받아요.",
            "light_chat_limit" => "오늘 가벼운 대화 한도에 도달했어요.",
            _ => "지금은 맡길 수 없어요.",
        }
        .into()
    }

    /// `too_many_requests` reason = RFC 3339 time it is allowed again → local "오후 3:40" (or "11월 15일 오전 9:00" when not
    /// today); None if not a time.
    pub fn retry_at(reason: &str) -> Option<String> { Self::retry_at_with(reason, clock::now(), clock::local_offset()) }

    pub fn retry_at_with(reason: &str, now: f64, offset: i64) -> Option<String> {
        let t = clock::parse_rfc3339(reason)?;
        let (y, m, d, h, mi, _) = clock::local_parts(t, offset);
        let (ny, nm, nd, ..) = clock::local_parts(now, offset);
        let time = clock::ko_time(h, mi);
        Some(if (y, m, d) == (ny, nm, nd) { time } else { format!("{m}월 {d}일 {time}") })
    }

    /// `{"error":{"code","message","reason"}}` of a REST answer, in Korean.
    pub fn from_body(data: &[u8], status: u16) -> String {
        if let Some(e) = json::object(data).and_then(|o| o.obj("error")) {
            if let Some(code) = e.str("code") { return Self::user_message(&code, e.str("reason").as_deref(), ""); }
        }
        match status {
            401 => Self::user_message("unauthorized", None, ""),
            404 => "찾지 못했어요.".into(),
            413 => Self::user_message("payload_too_large", None, ""),
            429 => Self::user_message("too_many_requests", None, ""),
            s if s >= 500 => Self::user_message("internal_error", None, ""),
            s => format!("문제가 생겼어요. (HTTP {s})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(json: &str) -> ServerFrame { ServerFrame::decode(json).expect("frame") }

    #[test]
    fn a_turn_streams_and_finishes() {
        let mut s = ChatState::new();
        s.apply(&frame(r#"{"type":"ready","room_id":"r1","agent_id":"a1","agent_name":"나초"}"#));
        assert_eq!(s.phase, ConnectionPhase::Connected);
        s.begin_turn("안녕", "c1", vec![]);
        assert!(s.is_busy());
        s.apply(&frame(r#"{"type":"turn_started","client_id":"c1"}"#));
        s.apply(&frame(r#"{"type":"text_delta","client_id":"c1","text":"안"}"#));
        s.apply(&frame(r#"{"type":"text_delta","client_id":"c1","text":"녕"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Streaming);
        s.apply(&frame(r#"{"type":"message","client_id":"c1","message":{"id":"m1","seq":2,"role":"assistant","text":"안녕하세요"}}"#));
        s.apply(&frame(r#"{"type":"done","client_id":"c1"}"#));
        assert_eq!(s.turns[0].assistant_text, "안녕하세요");
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert_eq!(s.last_seq(), 2);
        assert!(!s.is_busy());
        // Another session's turn never touches ours.
        s.apply(&frame(r#"{"type":"text_delta","client_id":"other","text":"x"}"#));
        assert_eq!(s.turns[0].assistant_text, "안녕하세요");
    }

    #[test]
    fn errors_go_to_the_turn_or_the_notice() {
        let mut s = ChatState::new();
        s.apply(&frame(r#"{"type":"error","code":"unauthorized","message":"no"}"#));
        assert_eq!(s.notice.as_deref(), Some("로그인이 필요해요."));
        assert_eq!(s.phase, ConnectionPhase::AuthFailed);
        s.begin_turn("x", "c1", vec![]);
        s.apply(&frame(r#"{"type":"error","client_id":"c1","code":"light_chat_limit"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Failed("오늘 가벼운 대화 한도에 도달했어요.".into()));
        assert_eq!(ErrorText::user_message("brand_new", None, "English"), "문제가 생겼어요. (brand_new)");
    }

    #[test]
    fn interrupted_turn_recovers_from_the_delta() {
        let mut s = ChatState::new();
        s.begin_turn("질문", "c1", vec![]);
        s.apply(&frame(r#"{"type":"message","message":{"id":"u1","seq":5,"role":"user","client_msg_id":"c1","text":"질문"}}"#));
        s.connection_lost();
        assert_eq!(s.turns[0].status, TurnStatus::Interrupted);
        s.recover(vec![ChatMessage::new("a1", Some(6), "assistant", "답이에요")]);
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert_eq!(s.turns[0].assistant_text, "답이에요");
        assert_eq!(s.turns[0].answer_seq, Some(6));
        // A user message of another device ends the turn's part of the delta: what follows goes to the record.
        let mut other = ChatMessage::new("u9", Some(8), "user", "폰에서");
        other.client_msg_id = Some("phone".into());
        s.recover(vec![other, ChatMessage::new("a9", Some(9), "assistant", "폰 답")]);
        assert_eq!(s.history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["u9", "a9"]);
    }

    #[test]
    fn task_versions_and_memory_lines() {
        let mut s = ChatState::new();
        let mut t = PetTask::new("t1", "running", "예약");
        t.version = 3;
        s.apply_task(t.clone(), true);
        let mut old = t.clone();
        old.version = 2;
        old.state = "queued".into();
        s.apply_task(old, true);
        assert_eq!(s.task.as_ref().unwrap().state, "running");
        let note = |json: &str| PetMemoryNote::decode(&json::object_str(json).unwrap());
        s.apply_memory(note(r#"{"update_id":"u1","action":"created","items":[{"memory_id":"m1","preview":"고양이"}]}"#));
        s.apply_memory(note(r#"{"update_id":"u2","action":"forgotten","items":[{"memory_id":"m1"}]}"#));
        assert_eq!(s.memory_notes.len(), 1);
        assert_eq!(s.memory_notes[0].action, "forgotten");
        s.apply_memory(note(r#"{"update_id":"u3","action":"restored","items":[{"memory_id":"m1"}]}"#));
        assert_eq!(s.memory_notes[0].text(), "기억했어요: 고양이");
    }

    #[test]
    fn retry_time_is_local_korean() {
        let now = clock::parse_rfc3339("2026-11-14T01:00:00Z").unwrap();
        assert_eq!(ErrorText::retry_at_with("2026-11-14T06:40:00Z", now, 9 * 3600).as_deref(), Some("오후 3:40"));
        assert_eq!(ErrorText::retry_at_with("2026-11-15T00:00:00Z", now, 9 * 3600).as_deref(), Some("11월 15일 오전 9:00"));
        assert_eq!(ErrorText::retry_at_with("soon", now, 0), None);
        assert_eq!(ErrorText::from_body(br#"{"error":{"code":"conflict"}}"#, 409), "이미 처리된 요청이에요.");
        assert_eq!(ErrorText::from_body(b"", 503), "일시적인 문제가 생겼어요. 다시 시도해 주세요.");
        assert_eq!(ErrorText::from_body(b"", 418), "문제가 생겼어요. (HTTP 418)");
    }
}

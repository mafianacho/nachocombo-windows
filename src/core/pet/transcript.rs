//! What the chat bubble shows, as a list of items in order: the same pieces as the web mockup's `scenes.js` (`mine`,
//! `nacho`, `line`, `file`, `card`, `note`, `divider`, older button, first screen). The shell draws each item with one
//! view; the rules about what shows where live here so tests on any OS cover them. Port of `PetTranscript.swift`.
//!
//! Windows: `symbol` fields keep the macOS SF Symbol names (`"brain"`, `"wrench.adjustable"` …) as icon keys; the
//! shell maps them to its own icons. Links are `nachopet://<action>?…` strings (the Swift `URL`), see [`PetLinks`].
use super::card_rules::{PetCardLocal, PetCardRowRole, PetCardRules, PetCardStyle};
use super::chat_state::{ChatState, ConnectionPhase, TurnStatus};
use super::frames::ActivityKind;
use super::selection::{PetSelectionPreview, PetSelectionRow};
use super::server_models::{ActivityText, PetCard, PetFileLink, PetMemoryNote};
use super::commands::PetCommands;
use super::session::{PetAuthPhase, PetSessionState};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// One piece of the chat, top to bottom. Cards stay inline (not boxed): the list is short and rebuilt per change, and
/// the shell matches `PetChatItem::Card(c)` directly.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum PetChatItem {
    Older { loading: bool },
    /// First screen: a centred question and a few example requests (tap = put it in the input field).
    Hello { title: String, examples: Vec<String> },
    Mine { text: String, attachments: Vec<String>, past: bool },
    Nacho { text: String, past: bool, streaming: bool },
    Line(PetChatLine),
    Files { files: Vec<PetFileChip>, mine: bool },
    Card(PetCardItem),
    Note(String),
    Divider(String),
    /// Selection bar: the chip under a sent request ("Edge · 고른 글 129자 · 맥락 4개").
    RequestChip(String),
    /// Selection bar: 복사 · 다시 · 보낸 내용 보기 under the answer to a request.
    AnswerActions { turn_id: String, showing_sent: bool },
    /// Selection bar: what that request sent (read only), after 보낸 내용 보기.
    SentContext(Vec<PetSelectionRow>),
    /// Selection bar: the preview card before anything is sent.
    SelectionPreview(PetSelectionPreview),
    /// Selection bar: the first-use card (켜기 / 나중에).
    SelectionIntro,
}

/// What the selection bar adds to the chat: its preview card, the first-use card, opened "보낸 내용 보기" lists.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetSelectionChatState {
    pub preview: Option<PetSelectionPreview>,
    pub intro: bool,
    /// Turn client message ids whose 보낸 내용 보기 is open.
    pub expanded: BTreeSet<String>,
}

/// The Swift `PetChatLine.Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetChatLineKind { Tool, Progress, Memory, Notice, Danger, Usage, Notification }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetChatLine {
    pub kind: PetChatLineKind,
    /// SF Symbol name (the web mockup's stand-in icon names map 1:1); an icon key for the Windows shell.
    pub symbol: Option<String>,
    pub parts: Vec<PetTextPart>,
    pub align_trailing: bool,
}

impl PetChatLine {
    pub fn new(kind: PetChatLineKind, symbol: &str, parts: Vec<PetTextPart>) -> Self {
        Self { kind, symbol: Some(symbol.into()), parts, align_trailing: false }
    }

    pub fn plain_text(&self) -> String { self.parts.iter().map(|p| p.text.as_str()).collect() }
}

/// The Swift `PetTextPart.Style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PetTextStyle {
    #[default]
    Plain,
    Secondary,
    Ok,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetTextPart {
    pub text: String,
    /// A `nachopet://` link (see [`PetLinks`]).
    pub link: Option<String>,
    pub tooltip: Option<String>,
    pub style: PetTextStyle,
}

impl PetTextPart {
    pub fn new(text: &str) -> Self { Self { text: text.into(), ..Default::default() } }
    pub fn styled(text: &str, style: PetTextStyle) -> Self { Self { text: text.into(), style, ..Default::default() } }
    pub fn link(text: &str, link: String) -> Self { Self { text: text.into(), link: Some(link), ..Default::default() } }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetFileChip {
    pub name: String,
    pub is_image: bool,
    /// Only `http(s)` addresses (never `file:` or app links); None for a deleted file.
    pub url: Option<String>,
    pub by_nacho: bool,
    pub deleted: bool,
}

/// One row (or button) of a card as drawn. The Swift `PetCardItem.Option`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetCardItemOption {
    pub label: String,
    /// The click (None = not clickable: answered, sending, or a card for the phone).
    pub link: Option<String>,
    /// The stored answer chose it.
    pub chosen: bool,
    /// 1…N (0 = unnumbered).
    pub number: usize,
    pub detail: Option<String>,
    pub role: PetCardRowRole,
    /// Ticked (several choices) or picked with a number key (Enter sends it).
    pub marked: bool,
    /// A check row of a multiple choice.
    pub tickable: bool,
}

impl PetCardItemOption {
    pub fn new(label: &str, link: Option<String>, chosen: bool, role: PetCardRowRole) -> Self {
        Self { label: label.into(), link, chosen, number: 0, detail: None, role, marked: false, tickable: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetCardItem {
    pub id: String,
    pub symbol: String,
    pub title: String,
    pub detail: Option<String>,
    pub options: Vec<PetCardItemOption>,
    /// A note under the options only where it adds something the question does not say ("휴대폰에서 승인해 주세요",
    /// "골랐어요", "보내는 중…"); never "골라 주세요" under a choice.
    pub status: Option<String>,
    pub status_waits: bool,
    pub style: PetCardStyle,
    /// Approval: what, the result, whether it can be undone (one line each).
    pub summary: Vec<String>,
    /// Several choices: the 확정 button (no link until something is ticked).
    pub confirm: Option<PetCardItemOption>,
    /// The number keys and the expiry time (KST) while it waits.
    pub hint: Option<String>,
    /// 다시 시도 after a failed answer.
    pub retry: Option<String>,
}

pub struct PetLinks;

impl PetLinks {
    /// Links in the transcript: `nachopet://<action>?…` (older, forget, undo, memfb, card, task …), query items sorted by
    /// name. Values are percent-encoded except the RFC 3986 unreserved characters (a little stricter than Foundation's
    /// `URLComponents`, which leaves e.g. `+` and `/` as they are; the shell percent-decodes either way).
    pub fn url(action: &str, items: &[(&str, &str)]) -> String {
        let mut out = format!("nachopet://{action}");
        if items.is_empty() { return out; }
        let mut sorted = items.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let q: Vec<String> = sorted.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
        out.push('?');
        out.push_str(&q.join("&"));
        out
    }
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
    }
    out
}

/// Korean text rules shared by the app and the mockup.
pub struct PetText;

impl PetText {
    /// An approval card names the action it asks about; when the server writes the capability's own name there
    /// (`PC에서 combo_quit_app 실행`), the user reads the tool's name instead (`앱 종료`). The raw name is not shown.
    pub fn humanize_action_names(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find("combo_") {
            let before_ok = rest[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            let len = rest[at..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len() - at);
            out.push_str(&rest[..at]);
            let raw = &rest[at..at + len];
            if before_ok {
                let canonical = PetCommands::ALIASES.iter().find(|(a, _)| *a == raw).map(|(_, c)| *c).unwrap_or(raw);
                out.push_str(Self::tool_name(canonical));
            } else { out.push_str(raw); }
            rest = &rest[at + len..];
        }
        out.push_str(rest);
        out
    }

    /// Tool names as the user reads them; the raw name stays in the tooltip.
    pub fn tool_name(raw: &str) -> &'static str {
        match raw {
            "search_web" => "웹 검색",
            "read_page" => "페이지 읽기",
            "browse_web" => "웹 둘러보기",
            "remember" => "기억 떠올리기",
            "read_files" => "자료 읽기",
            "write_files" | "write_file" => "파일 만들기",
            "run_command" => "명령 실행",
            "look_camera" => "사진 보기",
            "delegate" => "다른 나초에게 맡기기",
            "guide" => "안내 준비",
            "stop" => "멈추기",
            "combo_arrange_window" => "창 배치",
            "combo_set_gadget" => "가젯 켜기·끄기",
            "combo_list_gadgets" => "가젯 목록 보기",
            "combo_list_apps" => "앱 목록 보기",
            "combo_open_app" => "앱 열기",
            "combo_focus_app" => "앱 전환",
            "combo_hide_app" => "앱 숨기기",
            "combo_quit_app" => "앱 종료",
            "combo_list_windows" => "창 목록 보기",
            "combo_layout_windows" => "여러 창 배치",
            "combo_set_system" => "소리·밝기·방해금지",
            "combo_set_dim_strength" => "디밍 강도",
            "combo_clear_shelf" => "선반 비우기",
            "combo_workspace_save" => "작업 공간 저장",
            "combo_workspace_apply" => "작업 공간 적용",
            "combo_workspace_list" => "작업 공간 목록",
            // Gadget tools (`gadget_tools.rs`), as the connection tab and the approval cards show them.
            "combo_gadget_guide" => "가젯 안내 보기",
            "combo_window_displays" => "디스플레이 목록 보기",
            "combo_window_place" => "창 자유 배치",
            "combo_window_tile" => "창 바둑판 배치",
            "combo_window_split" => "창 등분 배치",
            "combo_window_ratio" => "창 비율 배치",
            "combo_window_move_display" => "창을 다른 화면으로",
            "combo_window_arrange_apps" => "앱 묶음 배치",
            "combo_window_undo" => "창 배치 되돌리기",
            "combo_appswitch_settings" => "앱 전환 설정 보기",
            "combo_appswitch_set_settings" => "앱 전환 설정 바꾸기",
            "combo_device_status" => "소리·마이크 상태 보기",
            "combo_device_set_volume" => "소리 크기 바꾸기",
            "combo_device_set_mic_muted" => "마이크 음소거",
            "combo_dim_status" => "디밍 상태 보기",
            "combo_dim_pause" => "디밍 잠시 멈춤",
            "combo_dim_resume" => "디밍 다시 시작",
            "combo_shelf_list" => "선반 목록 보기",
            "combo_shelf_give" => "선반 항목 보내기",
            "combo_menubar_status" => "트레이 상태 보기",
            "combo_menubar_list_items" => "트레이 아이콘 목록 보기",
            "combo_menubar_tidy" => "트레이 정리",
            "combo_menubar_set_collapsed" => "트레이 접기·펼치기",
            "combo_monitor_stats" => "시스템 수치 보기",
            "combo_disk_top_items" => "디스크 큰 항목 보기",
            "combo_disk_scan" => "디스크 스캔",
            "combo_clipboard_list" => "클립보드 목록 보기",
            "combo_clipboard_get_item" => "클립보드 항목 읽기",
            "combo_clipboard_copy_again" => "클립보드 다시 복사",
            "combo_capture_ask" => "화면 캡처 보내기",
            "combo_capture_list_pinned" => "고정 캡처 목록 보기",
            "combo_awake_status" => "잠자기 방지 상태 보기",
            "combo_awake_start" => "잠자기 방지 켜기",
            "combo_awake_stop" => "잠자기 방지 끄기",
            "combo_mouse_status" => "마우스 설정 보기",
            "combo_mouse_set" => "마우스 설정 바꾸기",
            "combo_calendar_status" => "시계 설정 보기",
            "combo_calendar_set" => "시계 설정 바꾸기",
            "combo_desktop_tidy_plan" => "바탕 화면 정리 계획",
            "combo_desktop_tidy_apply" => "바탕 화면 정리 실행",
            "combo_desktop_tidy_undo" => "바탕 화면 정리 되돌리기",
            "combo_desktop_tidy_history" => "바탕 화면 정리 기록",
            "combo_desktop_arrange_suggest" => "창 배치안 만들기",
            "combo_desktop_arrange_apply" => "창 배치안 적용",
            _ => "도구",
        }
    }

    /// Under a pending file's name: what it is and what Nacho can do with it ("그림 · 보여요").
    pub fn pending_note(f: &super::media::PetOutgoingFile) -> String {
        use super::media::Readability::*;
        let kind = if f.mime.starts_with("image/") {
            "그림"
        } else if f.mime.starts_with("text/") {
            "글"
        } else if f.mime == "application/pdf" {
            "PDF"
        } else {
            "파일"
        };
        match f.readability() {
            Seen => format!("{kind} · 보여요"),
            Read => format!("{kind} · 읽어요"),
            StoredOnly => format!("{kind} · 전달만 돼요"),
        }
    }

    /// 39000 → "39,000" (always comma groups, whatever the region).
    pub fn number(n: i64) -> String {
        let digits = n.unsigned_abs().to_string();
        let mut out = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) { out.push(','); }
            out.push(ch);
        }
        if n < 0 { format!("-{out}") } else { out }
    }

    /// 39000 원 → "39,000원"; other currencies keep their code: "12,000 USD".
    pub fn amount(n: i64, currency: Option<&str>) -> String {
        match currency {
            Some(c) if !c.is_empty() && !c.eq_ignore_ascii_case("KRW") && c != "원" => format!("{} {c}", Self::number(n)),
            _ => format!("{}원", Self::number(n)),
        }
    }
}


/// The header's status dot. The Swift `PetTranscript.HeaderDot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderDot { Ok, Busy, Wait, Warn, Off }

/// The header's one status line and its dot (the Swift tuple `(text, dot)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetHeaderStatus {
    pub text: String,
    pub dot: HeaderDot,
}

fn hs(text: &str, dot: HeaderDot) -> PetHeaderStatus { PetHeaderStatus { text: text.into(), dot } }

pub struct PetTranscript;

impl PetTranscript {
    pub const QUEUED_NOTE: &'static str = "답이 끝나면 보낼게요";
    pub const EXAMPLES: [&'static str; 3] = ["창 왼쪽으로 붙여 줘", "킵 어웨이크 켜 줘", "이 캡처 뭐야?"];
    /// Id prefix of plan cards (not server cards: number keys and card answers never go to them).
    pub const PLAN_CARD_PREFIX: &'static str = "plan-";

    /// The header's one status line: connection, activity or the task's state. Task and card states show here only,
    /// never again as a line in the transcript.
    pub fn header_status(st: &PetSessionState, selection: &PetSelectionChatState) -> PetHeaderStatus {
        match &st.auth {
            PetAuthPhase::SignedOut => return hs("로그인 전", HeaderDot::Off),
            PetAuthPhase::SigningIn => return hs("로그인 중", HeaderDot::Busy),
            PetAuthPhase::Failed(m) => return hs(m, HeaderDot::Warn),
            PetAuthPhase::SignedIn => {}
        }
        match st.chat.phase {
            ConnectionPhase::Connecting => return hs("연결 중", HeaderDot::Busy),
            ConnectionPhase::Reconnecting { attempt } => return hs(&format!("다시 연결 중 ({attempt})"), HeaderDot::Busy),
            ConnectionPhase::AuthFailed => return hs("다시 로그인해 주세요", HeaderDot::Warn),
            ConnectionPhase::Disconnected => return hs("쉬는 중", HeaderDot::Off),
            ConnectionPhase::Connected => {}
        }
        let chat = &st.chat;
        let busy = chat.is_busy();
        if busy && matches!(chat.activity, ActivityKind::Idle | ActivityKind::Typing | ActivityKind::Thinking) {
            if let Some(sel) = chat.turns.last().and_then(|last| st.selection_turns.get(&last.client_msg_id)) {
                return hs(sel.action.progress_text(), HeaderDot::Busy);
            }
        }
        if selection.preview.is_some() && !busy { return hs("보낼 내용을 확인해 주세요", HeaderDot::Wait); }
        if busy {
            let kind = if chat.activity == ActivityKind::Idle { ActivityKind::Typing } else { chat.activity };
            return hs(ActivityText::label(kind, chat.active_tool.as_deref()), HeaderDot::Busy);
        }
        if let Some(t) = &chat.task {
            let waits = t.state == "waiting" || t.needs_start_confirm();
            if t.is_open() || Self::last_turn_is_about(chat, &t.id) {
                let dot = if waits { HeaderDot::Wait } else if t.is_open() { HeaderDot::Busy } else { HeaderDot::Ok };
                return hs(&format!("{} · {}", t.title, t.status_text()), dot);
            }
        }
        if st.config.as_ref().and_then(|c| c.service_notice()).is_some() { return hs("연결됨", HeaderDot::Wait); }
        hs("연결됨", HeaderDot::Ok)
    }

    fn last_turn_is_about(chat: &ChatState, task_id: &str) -> bool {
        chat.turns.last().and_then(|t| t.task_id.as_deref()) == Some(task_id)
    }

    /// The header's whole line as shown (status and credits). Also the tooltip, so a truncated line can be read in full.
    pub fn header_line(st: &PetSessionState, selection: &PetSelectionChatState) -> String {
        let status = Self::header_status(st, selection).text;
        match Self::header_credits(st, selection) {
            Some(c) => format!("{status} · {c}"),
            None => status,
        }
    }

    /// "크레딧 180" next to the status; hidden while a task's status takes the space.
    pub fn header_credits(st: &PetSessionState, selection: &PetSelectionChatState) -> Option<String> {
        if st.auth != PetAuthPhase::SignedIn || selection.preview.is_some() { return None; }
        let u = st.chat.usage.as_ref()?;
        if let Some(t) = &st.chat.task {
            if t.is_open() || Self::last_turn_is_about(&st.chat, &t.id) { return None; }
        }
        Some(format!("크레딧 {}", PetText::number(u.remaining)))
    }

    /// The whole chat, top to bottom.
    /// - `undo_seconds_left`: seconds left to undo a forget this session asked for, by update id (None = no longer possible).
    /// - `cards`: what the app keeps per card while the user answers it (ticks, number-key mark, sending, failure).
    /// - `now`: Unix seconds (expiry times in the card hints).
    pub fn items(
        st: &PetSessionState,
        undo_seconds_left: &dyn Fn(&str) -> Option<i64>,
        selection: &PetSelectionChatState,
        cards: &BTreeMap<String, PetCardLocal>,
        now: f64,
    ) -> Vec<PetChatItem> {
        let none = PetCardLocal::default();
        let card = |c: &PetCard| Self::card(c, cards.get(&c.id).unwrap_or(&none), now);
        let mut out: Vec<PetChatItem> = vec![];
        let chat = &st.chat;
        if st.loading_history {
            out.push(PetChatItem::Older { loading: true });
        } else if chat.has_more_history {
            out.push(PetChatItem::Older { loading: false });
        }

        // Plan cards (tasks held until the user starts them) show under the turn that asked, else where their note is.
        let turn_task_ids: HashSet<&str> = chat.turns.iter().filter_map(|t| t.task_id.as_deref()).collect();
        let mut plan_shown: HashSet<String> = HashSet::new();

        // The room record: earlier talk, other devices, finished tasks.
        for m in &chat.history {
            if let Some(c) = &m.card {
                out.push(PetChatItem::Card(card(chat.card(&c.id).unwrap_or(c))));
                continue;
            }
            if m.note_kind.as_deref() == Some("plan") {
                if let Some(t) = m.task_id.as_deref() {
                    if turn_task_ids.contains(t) { continue; }
                    if let Some(p) = Self::plan_card(t, chat) {
                        out.push(PetChatItem::Card(p));
                        plan_shown.insert(t.to_string());
                        continue;
                    }
                }
            }
            if m.content_type == "system_note" {
                out.push(PetChatItem::Note(m.text.clone()));
                continue;
            }
            let mine = m.role == "user";
            if !m.text.is_empty() {
                out.push(if mine {
                    PetChatItem::Mine { text: m.text.clone(), attachments: vec![], past: true }
                } else {
                    PetChatItem::Nacho { text: m.text.clone(), past: true, streaming: false }
                });
            }
            if !m.files.is_empty() { out.push(PetChatItem::Files { files: m.files.iter().map(Self::chip).collect(), mine }); }
        }
        if !chat.history.is_empty() && !chat.turns.is_empty() { out.push(PetChatItem::Divider("이번 대화".into())); }
        if chat.turns.is_empty() && chat.history.is_empty() && selection.preview.is_none() && !selection.intro {
            out.push(PetChatItem::Hello { title: "무엇을 도와드릴까요?".into(), examples: Self::EXAMPLES.iter().map(|s| s.to_string()).collect() });
        }

        let turn_ids: HashSet<&str> = chat.turns.iter().map(|t| t.client_msg_id.as_str()).collect();
        for turn in &chat.turns {
            let request = st.selection_turns.get(&turn.client_msg_id);
            // A selection request shows its chip instead of the context file's name.
            let attachments = if request.is_none() { turn.attachment_names.clone() } else { vec![] };
            out.push(PetChatItem::Mine { text: turn.user_text.clone(), attachments, past: false });
            if let Some(r) = request { out.push(PetChatItem::RequestChip(r.chip.clone())); }
            // What Nacho is doing on the way: tools and "still working on it" lines.
            if !turn.tools.is_empty() {
                let mut parts: Vec<PetTextPart> = vec![];
                for (i, t) in turn.tools.iter().enumerate() {
                    if i > 0 { parts.push(PetTextPart::styled(" · ", PetTextStyle::Secondary)); }
                    parts.push(PetTextPart { tooltip: Some(t.name.clone()), ..PetTextPart::new(PetText::tool_name(&t.name)) });
                    parts.push(match (t.finished, t.is_error) {
                        (true, true) => PetTextPart::styled(" ✕", PetTextStyle::Failed),
                        (true, false) => PetTextPart::styled(" ✓", PetTextStyle::Ok),
                        (false, _) => PetTextPart::styled(" …", PetTextStyle::Secondary),
                    });
                }
                out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Tool, "wrench.adjustable", parts)));
            }
            for p in &turn.progress {
                out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Progress, "sparkles", vec![PetTextPart::new(p)])));
            }
            let mut answer = turn.assistant_text.clone();
            let mut failure: Option<String> = None;
            match &turn.status {
                TurnStatus::Sending | TurnStatus::Streaming => {}
                TurnStatus::Done => {
                    if answer.is_empty() {
                        if let Some(t) = &turn.task_id {
                            answer = if chat.tasks.get(t).is_some_and(|t| t.needs_start_confirm()) {
                                "일로 맡았어요. 확인해 주시면 시작할게요.".into()
                            } else {
                                "일로 맡았어요. 진행 상황은 여기에 보여 드릴게요.".into()
                            };
                        }
                    }
                }
                TurnStatus::Stopped => answer.push_str(if answer.is_empty() { "(멈췄어요)" } else { " (멈춤)" }),
                TurnStatus::Failed(m) => failure = Some(m.clone()),
                TurnStatus::Interrupted => failure = Some("연결이 끊겨 답이 끝나지 않았어요. 다시 연결되면 이어서 보여 드려요.".into()),
            }
            let streaming = matches!(turn.status, TurnStatus::Streaming | TurnStatus::Sending);
            if !answer.is_empty() || streaming { out.push(PetChatItem::Nacho { text: answer.clone(), past: false, streaming }); }
            if let Some(f) = failure {
                out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Danger, "exclamationmark.triangle", vec![PetTextPart::new(&f)])));
            }
            if !turn.files.is_empty() { out.push(PetChatItem::Files { files: turn.files.iter().map(Self::chip).collect(), mine: false }); }
            if let Some(r) = request {
                if turn.status == TurnStatus::Done && !answer.is_empty() {
                    let open = selection.expanded.contains(&turn.client_msg_id);
                    out.push(PetChatItem::AnswerActions { turn_id: turn.client_msg_id.clone(), showing_sent: open });
                    if open { out.push(PetChatItem::SentContext(r.rows.clone())); }
                }
            }
            for id in &turn.card_ids {
                if let Some(c) = chat.card(id) { out.push(PetChatItem::Card(card(c))); }
            }
            if let Some(t) = turn.task_id.as_deref() {
                if !plan_shown.contains(t) {
                    if let Some(p) = Self::plan_card(t, chat) {
                        out.push(PetChatItem::Card(p));
                        plan_shown.insert(t.to_string());
                    }
                }
            }
            // Memory: first what this answer used, then what it changed (one line per change, updated in place).
            if !turn.used_memory_ids.is_empty() && turn.status == TurnStatus::Done {
                out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Memory, "brain", vec![
                    PetTextPart::new(&format!("이 답에 기억 {}개를 썼어요", turn.used_memory_ids.len())),
                    PetTextPart::styled(" · ", PetTextStyle::Secondary),
                    PetTextPart::link("기억이 틀렸어요", PetLinks::url("memfb", &[("turn", &turn.client_msg_id)])),
                ])));
            }
            for n in chat.memory_notes.iter().filter(|n| n.turn_client_id.as_deref() == Some(turn.client_msg_id.as_str())) {
                out.push(PetChatItem::Line(Self::memory_line(n, undo_seconds_left)));
            }
        }
        // Memory lines and cards that came from elsewhere (hub, another device, a task).
        for n in chat.memory_notes.iter().filter(|n| n.turn_client_id.as_deref().is_none_or(|c| !turn_ids.contains(c))) {
            out.push(PetChatItem::Line(Self::memory_line(n, undo_seconds_left)));
        }
        let in_turns: HashSet<&str> = chat.turns.iter().flat_map(|t| t.card_ids.iter().map(String::as_str)).collect();
        let in_history: HashSet<&str> = chat.history.iter().filter_map(|m| m.card.as_ref().map(|c| c.id.as_str())).collect();
        for c in chat.cards.iter().filter(|c| c.is_pending() && !in_turns.contains(c.id.as_str()) && !in_history.contains(c.id.as_str())) {
            out.push(PetChatItem::Card(card(c)));
        }
        // A task waiting to be started whose plan note is not on screen (e.g. it came from the phone): its card at the end.
        if let Some(t) = &chat.task {
            if t.needs_start_confirm() && !plan_shown.contains(&t.id) {
                if let Some(p) = Self::plan_card(&t.id, chat) { out.push(PetChatItem::Card(p)); }
            }
        }
        if let Some(n) = &chat.last_notification {
            out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Notification, "bell", vec![PetTextPart::new(n)])));
        }
        if let Some(u) = &chat.usage {
            if u.exhausted || u.low_balance || u.block_reason.is_some() {
                out.push(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Usage, "creditcard", vec![PetTextPart::new(&u.summary())])));
            }
        }
        if let Some(q) = &st.queued_text {
            let shown = if q.is_empty() { format!("첨부 파일 {}개", st.pending.len()) } else { q.clone() };
            out.push(PetChatItem::Mine { text: shown, attachments: vec![], past: true });
            let mut line = PetChatLine::new(PetChatLineKind::Notice, "clock", vec![
                PetTextPart::new(Self::QUEUED_NOTE),
                PetTextPart::styled(" · ", PetTextStyle::Secondary),
                PetTextPart::link("취소", PetLinks::url("cancelqueued", &[])),
            ]);
            line.align_trailing = true;
            out.push(PetChatItem::Line(line));
        }
        if selection.intro { out.push(PetChatItem::SelectionIntro); }
        if let Some(p) = &selection.preview { out.push(PetChatItem::SelectionPreview(p.clone())); }
        out
    }

    fn memory_line(n: &PetMemoryNote, undo_seconds_left: &dyn Fn(&str) -> Option<i64>) -> PetChatLine {
        let forgotten = n.forgotten_at.is_some();
        let mut parts = vec![PetTextPart::new(&if forgotten { n.text_after_forget() } else { n.text() })];
        if forgotten {
            if let Some(s) = undo_seconds_left(&n.update_id).filter(|s| *s > 0) {
                parts.push(PetTextPart::styled(" · ", PetTextStyle::Secondary));
                parts.push(PetTextPart::link(&format!("{s}초 안에 되돌리기"), PetLinks::url("undo", &[("u", &n.update_id)])));
            }
        } else if n.action != "forgotten" {
            parts.push(PetTextPart::styled(" · ", PetTextStyle::Secondary));
            parts.push(PetTextPart::link("잊어 줘", PetLinks::url("forget", &[("u", &n.update_id)])));
        }
        PetChatLine::new(PetChatLineKind::Memory, "brain", parts)
    }

    fn chip(f: &PetFileLink) -> PetFileChip {
        let url = if f.deleted { None } else { f.url.clone().filter(|u| is_web_url(u)) };
        PetFileChip { name: f.name.clone(), is_image: f.kind == "image", url, by_nacho: f.by_nacho, deleted: f.deleted }
    }

    /// The card for a task the server holds until the user starts it (plan note `… · 약 N~M크레딧 · 확인 후 시작해 주세요`).
    /// [시작]·[취소] only while the task is known to be planned and no press is out; afterwards the task's state.
    /// None when there is neither a plan note nor a planned task.
    pub fn plan_card(task_id: &str, chat: &ChatState) -> Option<PetCardItem> {
        let note = chat.plan_note(task_id);
        let task = chat.tasks.get(task_id).or(chat.task.as_ref().filter(|t| t.id == task_id));
        let action = chat.task_actions.get(task_id).map(String::as_str);
        let notice = chat.task_notices.get(task_id);
        if note.is_none() && !task.is_some_and(|t| t.needs_start_confirm()) && action.is_none() && notice.is_none() { return None; }
        let parts: Vec<&str> = note.map(|n| n.text.as_str()).unwrap_or("").split(" · ").collect();
        let title = task
            .map(|t| t.title.clone())
            .or_else(|| parts.first().filter(|p| !p.is_empty()).map(|p| p.to_string()))
            .unwrap_or_else(|| "작업".into());
        let detail = task.and_then(|t| t.estimate_text()).or_else(|| parts.iter().find(|p| p.contains("크레딧")).map(|p| p.to_string()));
        let mut options = vec![];
        let mut status = notice.cloned();
        let mut waits = false;
        match task {
            Some(t) if t.needs_start_confirm() || action.is_some() => {
                let live = action.is_none();
                let link = |a: &str| live.then(|| PetLinks::url("task", &[("id", task_id), ("action", a)]));
                options = vec![
                    PetCardItemOption::new(if action == Some("start") { "시작하는 중…" } else { "시작" }, link("start"), action == Some("start"), PetCardRowRole::Primary),
                    PetCardItemOption::new(if action == Some("cancel") { "취소하는 중…" } else { "취소" }, link("cancel"), action == Some("cancel"), PetCardRowRole::Quiet),
                ];
                if status.is_none() && live { status = Some("확인 후 시작해 주세요".into()); }
                waits = true;
            }
            Some(t) => {
                status = status.or_else(|| Some(t.status_text()));
                waits = t.state == "waiting";
            }
            None => {}
        }
        // Drawn like an approval (실행-style row above a quiet one); not a server card, so number keys never go to it.
        Some(PetCardItem {
            id: format!("{}{task_id}", Self::PLAN_CARD_PREFIX),
            symbol: "play.circle".into(),
            title,
            detail,
            options,
            status,
            status_waits: waits,
            style: PetCardStyle::Approval,
            summary: vec![],
            confirm: None,
            hint: None,
            retry: None,
        })
    }

    /// One card as the chat shows it (also used by the dev CLI). `now`: Unix seconds.
    pub fn card(c: &PetCard, local: &PetCardLocal, now: f64) -> PetCardItem {
        let style = PetCardStyle::of(c);
        let state = PetCardRules::state_line(c, local, now);
        if style == PetCardStyle::Elsewhere {
            // Login, codes, payment cards, browser, connections: the phone app, as before.
            return PetCardItem {
                id: c.id.clone(),
                symbol: "iphone".into(),
                title: c.title.clone(),
                detail: c.detail.clone(),
                options: vec![],
                status: state.status,
                status_waits: c.is_pending(),
                style,
                summary: vec![],
                confirm: None,
                hint: None,
                retry: None,
            };
        }
        let open = c.is_pending() && local.sending.is_none();
        let chosen = PetCardRules::chosen_numbers(c);
        let options = PetCardRules::rows(c)
            .into_iter()
            .map(|r| PetCardItemOption {
                link: open.then(|| PetLinks::url("cardrow", &[("id", &c.id), ("n", &r.number.to_string())])),
                chosen: chosen.contains(&r.number),
                number: r.number,
                marked: c.is_pending()
                    && (local.highlighted == Some(r.number) || r.option_id.as_ref().is_some_and(|id| local.ticked.contains(id))),
                tickable: style == PetCardStyle::Multiple,
                detail: r.detail,
                role: r.role,
                label: r.label,
            })
            .collect();
        let mut confirm = None;
        if style == PetCardStyle::Multiple && c.is_pending() {
            let ready = PetCardRules::confirm(c, &local.ticked).is_some();
            let label = if local.ticked.is_empty() { "확정".to_string() } else { format!("확정 ({}개)", local.ticked.len()) };
            confirm = Some(PetCardItemOption::new(&label, (open && ready).then(|| PetLinks::url("cardconfirm", &[("id", &c.id)])), false, PetCardRowRole::Primary));
        }
        let mut detail = c.detail.clone();
        let mut summary = if style == PetCardStyle::Approval { PetCardRules::approval_summary(c) } else { vec![] };
        if style == PetCardStyle::RuleProposal {
            // One rule: its sentence (as text) above 적용 / 아니요; several: numbered rows, then 아니요.
            let one = c.options.len() == 1;
            if let (true, Some(o)) = (one, c.options.first()) { summary = vec![o.label.clone()]; }
            let then = if one { "적용하면 이 범위는 다시 묻지 않아요." } else { "하나를 고르면 그 범위는 다시 묻지 않아요." };
            detail = Some(match &c.said_text {
                Some(s) => format!("“{s}”라고 하셨어요. {then}"),
                None => then.into(),
            });
        }
        PetCardItem {
            id: c.id.clone(),
            symbol: if style == PetCardStyle::Approval { "checkmark.shield" } else { "questionmark.circle" }.into(),
            title: c.title.clone(),
            detail: if style == PetCardStyle::Approval { None } else { detail },
            options,
            status: state.status,
            status_waits: state.waits,
            style,
            summary,
            confirm,
            hint: if c.is_pending() { state.hint } else { None },
            retry: state.retry.then(|| PetLinks::url("cardretry", &[("id", &c.id)])),
        }
    }

    /// The newest card in the log that waits for an answer here: the number keys go to it.
    pub fn active_card_id(items: &[PetChatItem]) -> Option<String> {
        items.iter().rev().find_map(|item| match item {
            PetChatItem::Card(c)
                if c.style != PetCardStyle::Elsewhere
                    && !c.id.starts_with(Self::PLAN_CARD_PREFIX)
                    && (c.status_waits || c.options.iter().any(|o| o.link.is_some())) =>
            {
                Some(c.id.clone())
            }
            _ => None,
        })
    }
}

/// `URL(string:)` with an `http`/`https` scheme: no spaces or control characters, nothing else (`file:`, app links).
fn is_web_url(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else { return false };
    (scheme == "https" || scheme == "http") && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl PetMemoryNote {
    /// The same line after "잊어 줘": "잊었어요: …".
    pub fn text_after_forget(&self) -> String {
        let previews = self.items.iter().map(|i| i.preview.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
        if previews.is_empty() { "잊었어요".into() } else { format!("잊었어요: {previews}") }
    }
}

#[cfg(test)]
mod tests {
    //! `PetTranscriptTests.swift`, plus the PetTranscript parts of `PetVoiceChoiceTests.swift` (the `PetTranscript.card`
    //! halves of PetCardRulesTests / PetCardKeysTests, testTheNewestWaitingCardTakesTheKeys, PetDesktopToolNameTests),
    //! `PetBusySendTests.swift` (testQueuedLinesInTheTranscript, PetHeaderTooltipTests), `PetSelectionTests.swift`
    //! (PetSelectionTranscriptTests) and `PetTaskStartTests.swift` (the plan card from a hand-built state). The tests that
    //! drive a running PetSession live with the session.
    use super::*;
    use crate::core::pet::card_rules::PetCardAnswer;
    use crate::core::pet::chat_state::{ChatTurn, ToolStep};
    use crate::core::pet::clock;
    use crate::core::pet::commands::PetCommands;
    use crate::core::pet::frames::ServerFrame;
    use crate::core::pet::json;
    use crate::core::pet::selection::{
        PetSelectionAction, PetSelectionBarSettings, PetSelectionCapture, PetSelectionContext, PetSelectionFlow, PetSelectionSent,
    };
    use crate::core::pet::server_models::{ChatMessage, PetCardOption, PetMemoryItem, PetTask, PetUsage};
    use serde_json::{json, Value};

    /// 2026-11-13T05:00:00Z = 14:00 KST.
    fn seoul_now() -> f64 { clock::parse_rfc3339("2026-11-13T05:00:00Z").unwrap() }

    fn signed_in() -> PetSessionState {
        let mut st = PetSessionState { auth: PetAuthPhase::SignedIn, ..Default::default() };
        st.chat.phase = ConnectionPhase::Connected;
        st
    }

    fn no_undo(_: &str) -> Option<i64> { None }
    fn none() -> PetSelectionChatState { PetSelectionChatState::default() }

    fn items_with(st: &PetSessionState, undo: &dyn Fn(&str) -> Option<i64>, selection: &PetSelectionChatState) -> Vec<PetChatItem> {
        PetTranscript::items(st, undo, selection, &BTreeMap::new(), seoul_now())
    }
    fn items(st: &PetSessionState) -> Vec<PetChatItem> { items_with(st, &no_undo, &none()) }
    fn header(st: &PetSessionState) -> PetHeaderStatus { PetTranscript::header_status(st, &none()) }

    fn text(items: &[PetChatItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                PetChatItem::Older { loading } => if *loading { "older…" } else { "older" }.to_string(),
                PetChatItem::Hello { title, examples } => format!("hello {title} [{}]", examples.join("|")),
                PetChatItem::Mine { text, attachments, past } => format!(
                    "mine{} {text}{}",
                    if *past { "(past)" } else { "" },
                    if attachments.is_empty() { String::new() } else { format!(" +{}", attachments.join(",")) }
                ),
                PetChatItem::Nacho { text, past, streaming } => {
                    format!("nacho{}{} {text}", if *past { "(past)" } else { "" }, if *streaming { "(streaming)" } else { "" })
                }
                PetChatItem::Line(l) => format!("line.{} {}", format!("{:?}", l.kind).to_lowercase(), l.plain_text()),
                PetChatItem::Files { files, mine } => format!(
                    "files{} {}",
                    if *mine { "(mine)" } else { "" },
                    files.iter().map(|f| format!("{}{}", f.name, if f.deleted { "(지워짐)" } else { "" })).collect::<Vec<_>>().join(",")
                ),
                PetChatItem::Card(c) => format!(
                    "card {} [{}] {}",
                    c.title,
                    c.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>().join("|"),
                    c.status.as_deref().unwrap_or("-")
                ),
                PetChatItem::Note(n) => format!("note {n}"),
                PetChatItem::Divider(d) => format!("divider {d}"),
                PetChatItem::RequestChip(c) => format!("chip {c}"),
                PetChatItem::AnswerActions { turn_id, showing_sent } => format!("actions {turn_id}{}", if *showing_sent { "(open)" } else { "" }),
                PetChatItem::SentContext(rows) => format!("sent {}", rows.len()),
                PetChatItem::SelectionPreview(p) => format!("preview {}", p.title),
                PetChatItem::SelectionIntro => "intro".into(),
            })
            .collect()
    }

    fn turn(id: &str, user: &str, status: TurnStatus) -> ChatTurn {
        let mut t = ChatTurn::new(id, user);
        t.status = status;
        t
    }

    fn lines(items: &[PetChatItem]) -> Vec<&PetChatLine> {
        items.iter().filter_map(|i| if let PetChatItem::Line(l) = i { Some(l) } else { None }).collect()
    }

    fn last_card(items: &[PetChatItem]) -> &PetCardItem {
        match items.last() {
            Some(PetChatItem::Card(c)) => c,
            other => panic!("card last: {other:?}"),
        }
    }

    // MARK: PetTranscriptTests.swift

    #[test]
    fn first_screen_is_a_question_with_short_example_chips() {
        assert_eq!(text(&items(&signed_in())), ["hello 무엇을 도와드릴까요? [창 왼쪽으로 붙여 줘|킵 어웨이크 켜 줘|이 캡처 뭐야?]"]);
    }

    #[test]
    fn tool_lines_use_korean_names_and_keep_the_raw_name_as_tooltip() {
        let mut st = signed_in();
        let mut t = turn("c1", "노트북 배터리 오래 쓰는 법", TurnStatus::Streaming);
        t.tools = vec![
            ToolStep { call_id: "1".into(), name: "search_web".into(), finished: true, is_error: false },
            ToolStep { call_id: "2".into(), name: "read_page".into(), ..Default::default() },
            ToolStep { call_id: "3".into(), name: PetCommands::ARRANGE_WINDOW.into(), finished: true, is_error: true },
        ];
        st.chat.turns = vec![t];
        let all = items(&st);
        let l = lines(&all).into_iter().find(|l| l.kind == PetChatLineKind::Tool).expect("no tool line");
        assert_eq!(l.plain_text(), "웹 검색 ✓ · 페이지 읽기 … · 창 배치 ✕");
        assert_eq!(l.parts.iter().filter_map(|p| p.tooltip.as_deref()).collect::<Vec<_>>(), ["search_web", "read_page", "combo_arrange_window"]);
        assert!(!l.plain_text().contains('_'), "no raw tool names on screen");
        assert_eq!(PetText::tool_name("something_new"), "도구");
    }

    #[test]
    fn amounts_have_thousands_separators() {
        assert_eq!(PetText::amount(39000, None), "39,000원");
        assert_eq!(PetText::amount(1_234_567, Some("KRW")), "1,234,567원");
        assert_eq!(PetText::amount(990, Some("USD")), "990 USD");
        assert_eq!(PetText::number(-12000), "-12,000");
        let card = PetCard::decode(&json::object_str(r#"{"id":"a1","kind":"approval","status":"pending","payload":{"title":"결제할까요?","merchant":"선물가게","amount":39000}}"#).unwrap());
        assert_eq!(card.detail.as_deref(), Some("선물가게 · 39,000원"));
        let usage = PetUsage {
            remaining: 1200, light_chat_used: Some(48), light_chat_cap: Some(50), allowance: Some(20000), low_balance: true,
            ..Default::default()
        };
        assert_eq!(usage.summary(), "크레딧 1,200/20,000 · 가벼운 대화 48/50 · 얼마 안 남았어요");
    }

    #[test]
    fn task_and_choice_state_show_only_in_the_header() {
        let mut st = signed_in();
        let mut t = turn("c1", "내일 저녁 파스타집 예약해 줘", TurnStatus::Done);
        t.task_id = Some("t1".into());
        t.card_ids = vec!["k1".into()];
        st.chat.turns = vec![t];
        st.chat.task = Some(PetTask {
            wait_reason: Some("input".into()), pending_request_id: Some("k1".into()), ..PetTask::new("t1", "waiting", "파스타집 예약")
        });
        let mut k1 = PetCard::new("k1", "choice", "어느 시간이 좋아요?", vec![PetCardOption::new("o1", "저녁 7시"), PetCardOption::new("o2", "저녁 8시")]);
        k1.version = 1;
        k1.task_id = Some("t1".into());
        st.chat.cards = vec![k1];
        st.chat.usage = Some(PetUsage { remaining: 180, ..Default::default() });
        let shown = text(&items(&st));
        assert_eq!(shown, ["mine 내일 저녁 파스타집 예약해 줘", "nacho 일로 맡았어요. 진행 상황은 여기에 보여 드릴게요.", "card 어느 시간이 좋아요? [저녁 7시|저녁 8시] -"]);
        assert!(!shown.concat().contains("답을 기다려요"));
        let h = header(&st);
        assert_eq!(h.text, "파스타집 예약 · 답을 기다려요");
        assert_eq!(h.dot, HeaderDot::Wait);
        assert_eq!(PetTranscript::header_credits(&st, &none()), None, "the task's state takes the space");
        // An approval is answered here: 실행 above 거절, no note while it waits. A login stays on the phone.
        st.chat.cards[0].kind = "approval".into();
        let all = items(&st);
        let c = last_card(&all);
        assert_eq!(c.status, None);
        assert_eq!(c.options.iter().map(|o| format!("{} {}", o.number, o.label)).collect::<Vec<_>>(), ["1 실행", "2 거절"]);
        st.chat.cards[0].kind = "login".into();
        let all = items(&st);
        let l = last_card(&all);
        assert_eq!(l.status.as_deref(), Some("휴대폰의 나초 앱에서 이어서 해 주세요"));
        assert!(l.options.is_empty());
        st.chat.task = None;
        assert_eq!(PetTranscript::header_credits(&st, &none()).as_deref(), Some("크레딧 180"));
    }

    #[test]
    fn memory_lines_follow_the_flow_and_forget_changes_the_line_in_place() {
        let mut st = signed_in();
        let mut t1 = turn("c1", "나 요즘 고양이 키워", TurnStatus::Done);
        t1.assistant_text = "와, 좋겠어요!".into();
        let mut t2 = turn("c2", "내가 좋아하는 음식 알아?", TurnStatus::Done);
        t2.assistant_text = "매운 음식을 좋아하신다고 기억해요.".into();
        t2.used_memory_ids = vec!["m9".into()];
        st.chat.turns = vec![t1, t2];
        let item = |id: &str, preview: &str| PetMemoryItem { memory_id: id.into(), kind: "fact".into(), preview: preview.into() };
        let mut note = PetMemoryNote {
            update_id: "u1".into(), action: "created".into(), items: vec![item("m1", "고양이를 키워요")], source: "chat".into(),
            turn_client_id: Some("c1".into()), ..Default::default()
        };
        let mut note2 = note.clone();
        note2.update_id = "u2".into();
        note2.items = vec![item("m2", "매운 음식 대신 순한 맛")];
        note2.turn_client_id = Some("c2".into());
        st.chat.memory_notes = vec![note.clone(), note2];
        assert_eq!(text(&items(&st)), [
            "mine 나 요즘 고양이 키워", "nacho 와, 좋겠어요!", "line.memory 기억했어요: 고양이를 키워요 · 잊어 줘",
            "mine 내가 좋아하는 음식 알아?", "nacho 매운 음식을 좋아하신다고 기억해요.",
            "line.memory 이 답에 기억 1개를 썼어요 · 기억이 틀렸어요", "line.memory 기억했어요: 매운 음식 대신 순한 맛 · 잊어 줘",
        ]);
        note.forgotten_at = Some(seoul_now());
        st.chat.memory_notes[0] = note;
        let after = text(&items_with(&st, &|u: &str| (u == "u1").then_some(7), &none()));
        assert_eq!(after[2], "line.memory 잊었어요: 고양이를 키워요 · 7초 안에 되돌리기");
        assert_eq!(after.iter().filter(|s| s.contains("고양이를 키워요")).count(), 1, "never two lines for one memory");
        assert_eq!(text(&items(&st))[2], "line.memory 잊었어요: 고양이를 키워요");
    }

    #[test]
    fn file_chips_only_link_web_files_and_mark_deleted_ones() {
        let mut st = signed_in();
        let mut t = turn("c1", "회의 메모를 표로 정리해 줘", TurnStatus::Done);
        t.assistant_text = "정리했어요.".into();
        let link = |id: &str, name: &str, kind: &str, url: Option<&str>, deleted: bool, by_nacho: bool| PetFileLink {
            attachment_id: Some(id.into()), name: name.into(), kind: kind.into(), url: url.map(Into::into), deleted, by_nacho,
            ..Default::default()
        };
        t.files = vec![
            link("a", "회의 정리.csv", "file", Some("https://x.test/a"), false, true),
            link("b", "예전 캡처.png", "image", None, true, false),
            link("c", "local.txt", "file", Some("file:///C:/Windows/win.ini"), false, true),
        ];
        st.chat.turns = vec![t];
        let all = items(&st);
        let Some(PetChatItem::Files { files: chips, mine }) = all.last() else { panic!("files last: {all:?}") };
        assert!(!mine);
        assert_eq!(chips.iter().map(|c| c.url.as_deref()).collect::<Vec<_>>(), [Some("https://x.test/a"), None, None]);
        assert_eq!(chips.iter().map(|c| c.deleted).collect::<Vec<_>>(), [false, true, false]);
        assert!(chips[1].is_image);
    }

    #[test]
    fn streaming_and_failures() {
        let mut st = signed_in();
        let t1 = turn("c1", "오늘 일정 알려줘", TurnStatus::Failed("서버가 답을 만들지 못했어요. 잠시 뒤 다시 시도해 주세요.".into()));
        let mut t2 = turn("c2", "다시", TurnStatus::Streaming);
        t2.assistant_text = "오늘은".into();
        st.chat.turns = vec![t1, t2];
        st.chat.activity = ActivityKind::Typing;
        assert_eq!(text(&items(&st)), [
            "mine 오늘 일정 알려줘", "line.danger 서버가 답을 만들지 못했어요. 잠시 뒤 다시 시도해 주세요.", "mine 다시", "nacho(streaming) 오늘은",
        ]);
        assert_eq!(header(&st).text, "답하는 중");
        st.auth = PetAuthPhase::SignedOut;
        assert_eq!(header(&st).text, "로그인 전");
    }

    // MARK: PetVoiceChoiceTests.swift: the PetTranscript.card halves of PetCardRulesTests and PetCardKeysTests

    fn decode(v: Value) -> PetCard { PetCard::decode(v.as_object().unwrap()) }

    fn opts3() -> Vec<(&'static str, &'static str)> { vec![("a", "저녁 7시"), ("b", "저녁 8시"), ("c", "저녁 9시")] }

    fn card_with(kind: &str, status: &str, task: Option<&str>, multiple: bool, options: &[(&str, &str)], top: Value) -> PetCard {
        let mut o = json!({
            "id": format!("k-{kind}"), "kind": kind, "status": status, "version": 1,
            "payload": {"question": "어느 시간이 좋아요?", "multiple": multiple,
                        "options": options.iter().map(|(id, l)| json!({"id": id, "label": l})).collect::<Vec<_>>()},
        });
        if let Some(t) = task { o["task_id"] = json!(t); }
        if let Value::Object(extra) = top { for (k, v) in extra { o[k] = v; } }
        decode(o)
    }

    fn card(kind: &str) -> PetCard { card_with(kind, "pending", Some("t1"), false, &opts3(), json!({})) }
    fn item(c: &PetCard) -> PetCardItem { PetTranscript::card(c, &PetCardLocal::default(), seoul_now()) }
    fn labels(i: &PetCardItem) -> Vec<String> { i.options.iter().map(|o| format!("{} {}", o.number, o.label)).collect() }
    fn chosen(i: &PetCardItem) -> Vec<bool> { i.options.iter().map(|o| o.chosen).collect() }

    #[test]
    fn approval_says_what_result_and_undo_then_run_above_deny() {
        let c = decode(json!({"id": "ap", "kind": "approval", "status": "pending", "version": 3, "expires_at": "2026-11-13T06:20:00Z",
            "payload": {"title": "선물가게에서 결제할까요?", "action_type": "payment", "merchant": "선물가게", "amount": 39000,
                        "reversibility": "conditional", "reversibility_text": "배송 전이면 취소할 수 있어요"}}));
        let i = item(&c);
        assert!(i.options.iter().all(|o| o.link.is_some()), "거절 is never disabled while it waits");
        assert_eq!(i.hint.as_deref(), Some("숫자키를 누르고 Enter · 오후 3:20 KST까지"));
        assert_eq!(i.detail, None, "the summary carries it");
        assert_eq!(i.summary, ["무엇을: 결제 · 선물가게 · 39,000원", "되돌리기: 배송 전이면 취소할 수 있어요"]);
        assert_eq!(i.symbol, "checkmark.shield");
    }

    #[test]
    fn one_choice_is_numbered_and_a_click_answers() {
        let i = item(&card("choice"));
        assert_eq!(i.style, PetCardStyle::Single);
        assert_eq!(labels(&i), ["1 저녁 7시", "2 저녁 8시", "3 저녁 9시"]);
        assert_eq!(i.options[1].link, Some(PetLinks::url("cardrow", &[("id", "k-choice"), ("n", "2")])));
        assert_eq!(i.options[1].link.as_deref(), Some("nachopet://cardrow?id=k-choice&n=2"));
        assert_eq!(i.confirm, None);
    }

    #[test]
    fn several_choices_tick_then_confirm_in_card_order() {
        let c = card_with("choice", "pending", None, true, &opts3(), json!({}));
        let mut local = PetCardLocal::default();
        assert_eq!(PetTranscript::card(&c, &local, seoul_now()).confirm.and_then(|o| o.link), None, "확정 waits for a tick");
        local.ticked = vec!["c".into(), "a".into()];
        let i = PetTranscript::card(&c, &local, seoul_now());
        assert_eq!(i.options.iter().map(|o| o.marked).collect::<Vec<_>>(), [true, false, true]);
        assert!(i.options.iter().all(|o| o.tickable));
        assert!(i.options.iter().all(|o| o.link.is_some()), "a row click ticks");
        let confirm = i.confirm.as_ref().unwrap();
        assert_eq!(confirm.label, "확정 (2개)");
        assert_eq!(confirm.link, Some(PetLinks::url("cardconfirm", &[("id", "k-choice")])));
        assert_eq!(confirm.role, PetCardRowRole::Primary);
        assert_eq!(i.hint.as_deref(), Some("숫자키로 고르고 Enter로 확정"));
    }

    #[test]
    fn suggestions_are_quick_replies_that_send_their_text() {
        let c = card_with("suggestions", "pending", None, false, &[("0", "내일로 미뤄 줘"), ("1", "지금 해 줘")], json!({}));
        let i = item(&c);
        assert_eq!(i.style, PetCardStyle::Suggestions);
        assert_eq!(labels(&i), ["1 내일로 미뤄 줘", "2 지금 해 줘"]);
        assert_eq!(i.hint, None, "chips need no key hint");
    }

    #[test]
    fn rule_proposal_apply_or_no() {
        let one = decode(json!({"id": "r1", "kind": "rule_proposal", "status": "pending", "version": 1,
            "payload": {"said_text": "3만 원까지 괜찮아", "rule_options": [{"id": "o1", "summary": "배달앱에서 건당 30,000원 이하"}]}}));
        let i = item(&one);
        assert_eq!(i.style, PetCardStyle::RuleProposal);
        assert_eq!(i.options.iter().map(|o| format!("{} {} {:?}", o.number, o.label, o.role)).collect::<Vec<_>>(), ["1 적용 Primary", "2 아니요 Quiet"]);
        assert_eq!(i.summary, ["배달앱에서 건당 30,000원 이하"], "the rule itself, above the buttons");
        assert_eq!(i.detail.as_deref(), Some("“3만 원까지 괜찮아”라고 하셨어요. 적용하면 이 범위는 다시 묻지 않아요."));
        let two = decode(json!({"id": "r2", "kind": "rule_proposal", "status": "pending", "version": 1,
            "payload": {"rule_options": [{"id": "o1", "summary": "건당 20,000원"}, {"id": "o2", "summary": "일주일 60,000원"}]}}));
        let i2 = item(&two);
        assert_eq!(labels(&i2), ["1 건당 20,000원", "2 일주일 60,000원", "3 아니요"]);
        assert!(i2.summary.is_empty());
        assert_eq!(i2.detail.as_deref(), Some("하나를 고르면 그 범위는 다시 묻지 않아요."));
        let mut done = one.clone();
        done.status = "answered".into();
        done.answered_action = Some("choose".into());
        done.answered_option_ids = vec!["o1".into()];
        assert_eq!(item(&done).status.as_deref(), Some("규칙을 만들었어요"));
        assert_eq!(chosen(&item(&done)), [true, false]);
        done.status = "dismissed".into();
        assert_eq!(item(&done).status.as_deref(), Some("만들지 않았어요"));
    }

    #[test]
    fn states_sending_failed_retry_answered_expired_superseded_dismissed() {
        let c = card("choice");
        let mut local = PetCardLocal { sending: Some(PetCardAnswer::new("choose").with_options(&["b"])), ..Default::default() };
        local.last_answer = local.sending.clone();
        let i = PetTranscript::card(&c, &local, seoul_now());
        assert_eq!(i.status.as_deref(), Some("보내는 중…"));
        assert!(i.status_waits);
        assert!(i.options.iter().all(|o| o.link.is_none()), "no second click while it is on its way");

        local.sending = None;
        local.failure = Some("서버에 연결하지 못했어요. 네트워크를 확인해 주세요.".into());
        let i = PetTranscript::card(&c, &local, seoul_now());
        assert_eq!(i.status.as_deref(), Some("보내지 못했어요. 서버에 연결하지 못했어요. 네트워크를 확인해 주세요."));
        assert_eq!(i.retry, Some(PetLinks::url("cardretry", &[("id", "k-choice")])));
        assert!(i.options.iter().all(|o| o.link.is_some()), "still answerable after a failure");

        let answered = decode(json!({"id": "k", "kind": "choice", "status": "answered", "version": 2, "answered_action": "choose",
            "answer": {"action": "choose", "option_id": "b", "answered_at": "2026-11-13T05:01:00Z"},
            "payload": {"question": "q", "options": [{"id": "a", "label": "7시"}, {"id": "b", "label": "8시"}]}}));
        let i = item(&answered);
        assert_eq!(i.status.as_deref(), Some("골랐어요"));
        assert_eq!(chosen(&i), [false, true], "the chosen row is marked");
        assert!(i.options.iter().all(|o| o.link.is_none()));
        assert_eq!(i.hint, None);

        let declined = decode(json!({"id": "ap", "kind": "approval", "status": "answered", "version": 2, "answered_action": "decline",
            "payload": {"title": "보낼까요?"}}));
        assert_eq!(chosen(&item(&declined)), [false, true]);
        assert_eq!(item(&declined).status.as_deref(), Some("거절했어요"));

        let expired = card_with("choice", "expired", Some("t1"), false, &opts3(), json!({"expires_at": "2026-11-12T09:30:00Z"}));
        assert_eq!(item(&expired).status.as_deref(), Some("시간이 지나 만료됐어요 (11월 12일 오후 6:30 KST)"));
        let superseded = card_with("choice", "superseded", Some("t1"), false, &opts3(), json!({}));
        assert_eq!(item(&superseded).status.as_deref(), Some("새 요청으로 바뀌었어요"));
        let dismissed = card_with("choice", "dismissed", Some("t1"), false, &opts3(), json!({}));
        assert_eq!(item(&dismissed).status.as_deref(), Some("닫았어요"));
    }

    #[test]
    fn input_cards_stay_on_the_phone() {
        let login = decode(json!({"id": "l", "kind": "login", "status": "pending", "version": 1, "payload": {"source": "naver", "verified_host": "nid.naver.com"}}));
        let i = item(&login);
        assert_eq!(i.style, PetCardStyle::Elsewhere);
        assert!(i.options.is_empty());
        assert_eq!(i.status.as_deref(), Some("휴대폰의 나초 앱에서 이어서 해 주세요"));
        assert!(i.status_waits);
        assert_eq!(i.symbol, "iphone");
    }

    #[test]
    fn number_marks_enter_sends_esc_clears() {
        let local = PetCardLocal { highlighted: Some(2), ..Default::default() };
        let i = PetTranscript::card(&card("choice"), &local, seoul_now());
        assert_eq!(i.options.iter().map(|o| o.marked).collect::<Vec<_>>(), [false, true, false]);
    }

    #[test]
    fn the_newest_waiting_card_takes_the_keys() {
        let mut st = signed_in();
        let mut t1 = turn("c1", "예약해 줘", TurnStatus::Done);
        t1.card_ids = vec!["k-choice".into(), "ap".into()];
        st.chat.turns = vec![t1];
        let ap = decode(json!({"id": "ap", "kind": "approval", "status": "answered", "answered_action": "approve", "version": 2, "payload": {"title": "결제?"}}));
        st.chat.cards = vec![card("choice"), ap];
        assert_eq!(PetTranscript::active_card_id(&items(&st)).as_deref(), Some("k-choice"), "the answered approval is skipped");
        st.chat.cards[0].status = "expired".into();
        assert_eq!(PetTranscript::active_card_id(&items(&st)), None);
    }

    #[test]
    fn desktop_action_tools_have_korean_names() {
        let names = [
            ("combo_list_apps", "앱 목록 보기"), ("combo_open_app", "앱 열기"), ("combo_focus_app", "앱 전환"),
            ("combo_hide_app", "앱 숨기기"), ("combo_quit_app", "앱 종료"), ("combo_list_windows", "창 목록 보기"),
            ("combo_layout_windows", "여러 창 배치"), ("combo_set_system", "소리·밝기·방해금지"),
            ("combo_set_dim_strength", "디밍 강도"), ("combo_clear_shelf", "선반 비우기"),
            ("combo_workspace_save", "작업 공간 저장"), ("combo_workspace_apply", "작업 공간 적용"),
            ("combo_workspace_list", "작업 공간 목록"),
            ("combo_arrange_window", "창 배치"), ("combo_set_gadget", "가젯 켜기·끄기"), ("combo_list_gadgets", "가젯 목록 보기"),
            ("combo_gadget_guide", "가젯 안내 보기"),
            ("combo_window_displays", "디스플레이 목록 보기"),
            ("combo_window_place", "창 자유 배치"),
            ("combo_window_tile", "창 바둑판 배치"),
            ("combo_window_split", "창 등분 배치"),
            ("combo_window_ratio", "창 비율 배치"),
            ("combo_window_move_display", "창을 다른 화면으로"),
            ("combo_window_arrange_apps", "앱 묶음 배치"),
            ("combo_window_undo", "창 배치 되돌리기"),
            ("combo_appswitch_settings", "앱 전환 설정 보기"),
            ("combo_appswitch_set_settings", "앱 전환 설정 바꾸기"),
            ("combo_device_status", "소리·마이크 상태 보기"),
            ("combo_device_set_volume", "소리 크기 바꾸기"),
            ("combo_device_set_mic_muted", "마이크 음소거"),
            ("combo_dim_status", "디밍 상태 보기"),
            ("combo_dim_pause", "디밍 잠시 멈춤"),
            ("combo_dim_resume", "디밍 다시 시작"),
            ("combo_shelf_list", "선반 목록 보기"),
            ("combo_shelf_give", "선반 항목 보내기"),
            ("combo_menubar_status", "트레이 상태 보기"),
            ("combo_menubar_list_items", "트레이 아이콘 목록 보기"),
            ("combo_menubar_tidy", "트레이 정리"),
            ("combo_menubar_set_collapsed", "트레이 접기·펼치기"),
            ("combo_monitor_stats", "시스템 수치 보기"),
            ("combo_disk_top_items", "디스크 큰 항목 보기"),
            ("combo_disk_scan", "디스크 스캔"),
            ("combo_clipboard_list", "클립보드 목록 보기"),
            ("combo_clipboard_get_item", "클립보드 항목 읽기"),
            ("combo_clipboard_copy_again", "클립보드 다시 복사"),
            ("combo_capture_ask", "화면 캡처 보내기"),
            ("combo_capture_list_pinned", "고정 캡처 목록 보기"),
            ("combo_awake_status", "잠자기 방지 상태 보기"),
            ("combo_awake_start", "잠자기 방지 켜기"),
            ("combo_awake_stop", "잠자기 방지 끄기"),
            ("combo_mouse_status", "마우스 설정 보기"),
            ("combo_mouse_set", "마우스 설정 바꾸기"),
            ("combo_calendar_status", "시계 설정 보기"),
            ("combo_calendar_set", "시계 설정 바꾸기"),
            ("combo_desktop_tidy_plan", "바탕 화면 정리 계획"),
            ("combo_desktop_tidy_apply", "바탕 화면 정리 실행"),
            ("combo_desktop_tidy_undo", "바탕 화면 정리 되돌리기"),
            ("combo_desktop_tidy_history", "바탕 화면 정리 기록"),
            ("combo_desktop_arrange_suggest", "창 배치안 만들기"),
            ("combo_desktop_arrange_apply", "창 배치안 적용"),
        ];
        // Every tool Windows declares has a name of its own: an approval card never reads "PC에서 도구 실행".
        for t in crate::core::pet::gadget_tools::PetGadgetTools::declared(true) { assert_ne!(PetText::tool_name(t.name), "도구", "{}", t.name); }
        assert_eq!(PetText::humanize_action_names("PC에서 combo_desktop_tidy_undo 실행"), "PC에서 바탕 화면 정리 되돌리기 실행");
        for (raw, korean) in names { assert_eq!(PetText::tool_name(raw), korean, "{raw}"); }
        assert_eq!(names.iter().map(|(_, k)| *k).collect::<HashSet<_>>().len(), names.len(), "every tool reads differently");
        assert_eq!(PetText::tool_name("combo_something_new"), "도구");
        assert_eq!(PetText::tool_name(PetCommands::SET_GADGET), "가젯 켜기·끄기");
        assert_eq!(PetText::tool_name(PetCommands::LIST_GADGETS), "가젯 목록 보기");
    }

    // MARK: PetBusySendTests.swift (the transcript parts)

    #[test]
    fn queued_lines_in_the_transcript() {
        let mut st = PetSessionState { auth: PetAuthPhase::SignedIn, queued_text: Some("나중에 보낼 글".into()), ..Default::default() };
        let all = items(&st);
        assert!(all.contains(&PetChatItem::Mine { text: "나중에 보낼 글".into(), attachments: vec![], past: true }));
        let note = lines(&all).first().copied().cloned().expect("queued note");
        assert_eq!(note.parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), [PetTranscript::QUEUED_NOTE, " · ", "취소"]);
        assert_eq!(note.parts.last().and_then(|p| p.link.clone()), Some(PetLinks::url("cancelqueued", &[])));
        assert_eq!(PetLinks::url("cancelqueued", &[]), "nachopet://cancelqueued");
        assert!(note.align_trailing);
        st.queued_text = None;
        assert!(lines(&items(&st)).is_empty());
    }

    #[test]
    fn header_line_is_the_whole_status_with_credits() {
        let mut st = PetSessionState { auth: PetAuthPhase::Failed("서버에 연결하는 중에 문제가 생겼어요. 잠시 뒤에 다시 해 주세요.".into()), ..Default::default() };
        assert_eq!(PetTranscript::header_line(&st, &none()), "서버에 연결하는 중에 문제가 생겼어요. 잠시 뒤에 다시 해 주세요.");
        st.auth = PetAuthPhase::SignedIn;
        let credits = PetTranscript::header_credits(&st, &none()).map(|c| format!(" · {c}")).unwrap_or_default();
        assert_eq!(PetTranscript::header_line(&st, &none()), header(&st).text + &credits);
        st.chat.phase = ConnectionPhase::Connected;
        st.chat.usage = Some(PetUsage { remaining: 1800, ..Default::default() });
        assert_eq!(PetTranscript::header_line(&st, &none()), "연결됨 · 크레딧 1,800");
    }

    // MARK: PetSelectionTests.swift (PetSelectionTranscriptTests)

    fn capture() -> PetSelectionCapture {
        let title = "mafianacho/nachocombo: Small gadgets in one box".to_string();
        PetSelectionCapture {
            text: "Every gadget starts off. SECRET-선택-글-7f3a".into(),
            app_name: "Microsoft Edge".into(),
            bundle_id: "msedge.exe".into(),
            window_title: Some(title.clone()),
            url: Some("https://github.com/mafianacho/nachocombo?token=abc#top".into()),
            page_title: Some(title),
            before: Some("Nacho Combo puts the small tools".into()),
            after: Some("The Nacho character sits".into()),
            language: Some("en".into()),
        }
    }

    #[test]
    fn preview_card_and_header_before_sending() {
        let flow = PetSelectionFlow::new(None, |_| {}, |_| {});
        flow.present(&capture());
        flow.choose(PetSelectionAction::Translate, &PetSelectionBarSettings { enabled: true, ..Default::default() });
        let sel = PetSelectionChatState { preview: flow.preview(), ..Default::default() };
        let all = items_with(&signed_in(), &no_undo, &sel);
        let Some(PetChatItem::SelectionPreview(p)) = all.last() else { panic!("preview last: {all:?}") };
        assert_eq!(p.title, "번역 · 한국어로");
        assert!(!all.iter().any(|i| matches!(i, PetChatItem::Hello { .. })), "no first screen under the preview");
        let h = PetTranscript::header_status(&signed_in(), &sel);
        assert_eq!(h.text, "보낼 내용을 확인해 주세요");
        assert_eq!(h.dot, HeaderDot::Wait);
        assert_eq!(PetTranscript::header_credits(&signed_in(), &sel), None);
    }

    #[test]
    fn sent_request_shows_its_chip_and_the_answer_actions() {
        let mut st = signed_in();
        st.chat.begin_turn("요약해 줘", "c1", vec![PetSelectionContext::FILE_NAME.into()]);
        let ctx = PetSelectionContext::new(&capture());
        st.selection_turns.insert("c1".into(), PetSelectionSent {
            action: PetSelectionAction::Summarize, instruction: "요약해 줘".into(), chip: ctx.chip_text(), rows: ctx.rows(),
            file: ctx.file().unwrap(),
        });
        st.chat.turns[0].status = TurnStatus::Streaming;
        st.chat.turns[0].assistant_text = "가젯은".into();
        assert!(st.chat.is_busy());
        assert_eq!(header(&st).text, "요약하는 중");
        let all = items(&st);
        assert!(all.contains(&PetChatItem::Mine { text: "요약해 줘".into(), attachments: vec![], past: false }), "the chip replaces the file name");
        assert!(all.contains(&PetChatItem::RequestChip(ctx.chip_text())));
        assert!(!all.iter().any(|i| matches!(i, PetChatItem::AnswerActions { .. })), "no actions while streaming");

        st.chat.turns[0].status = TurnStatus::Done;
        st.chat.turns[0].assistant_text = "가젯은 처음에 모두 꺼져 있어요.".into();
        assert!(items(&st).contains(&PetChatItem::AnswerActions { turn_id: "c1".into(), showing_sent: false }));
        let open = PetSelectionChatState { expanded: BTreeSet::from(["c1".to_string()]), ..Default::default() };
        let all = items_with(&st, &no_undo, &open);
        assert!(all.contains(&PetChatItem::AnswerActions { turn_id: "c1".into(), showing_sent: true }));
        assert!(all.contains(&PetChatItem::SentContext(ctx.rows())));
        let intro = items_with(&signed_in(), &no_undo, &PetSelectionChatState { intro: true, ..Default::default() });
        assert_eq!(intro.last(), Some(&PetChatItem::SelectionIntro));
        assert_eq!(intro.len(), 1, "no first screen under the intro card");
    }

    // MARK: PetTaskStartTests.swift (the plan card from a hand-built state)

    const ROOM: &str = "0193aaaa-0000-7000-8000-0000000000r7";
    const TASK: &str = "0193aaaa-0000-7000-8000-00000000t001";

    fn frame(s: &str) -> ServerFrame { ServerFrame::decode(s).unwrap_or_else(|| panic!("frame: {s}")) }

    fn task_json(state: &str) -> String {
        format!(
            r#"{{"id":"{TASK}","room_id":"{ROOM}","kind":"task","state":"{state}","title":"휴대폰 전화해줘","request_text":"x","plan_mode":"full","steps":[],"stalled":false,"credit":{{"estimate":{{"estimate_min":40,"estimate_max":60,"task_label":"전화"}},"estimate_id":"e1","used":0,"cap":60,"refund_status":"none"}},"accessed_items":[],"plan_blocks":[],"result_blocks":[],"estimate_id":"e1","version":1}}"#
        )
    }

    fn plan_note_json(seq: i64) -> String {
        let text = "휴대폰 전화해줘 · 약 40~60크레딧 · 확인 후 시작해 주세요";
        format!(
            r#"{{"id":"n{seq}","room_id":"{ROOM}","seq":{seq},"role":"system","sender_kind":"system","text":"{text}","content":{{"type":"system_note","kind":"plan","text":"{text}"}},"task_id":"{TASK}","used_memory_ids":[],"attachments":[]}}"#
        )
    }

    fn plan_card_item(items: &[PetChatItem]) -> Option<&PetCardItem> {
        items.iter().find_map(|i| match i {
            PetChatItem::Card(c) if c.id == format!("plan-{TASK}") => Some(c),
            _ => None,
        })
    }

    #[test]
    fn plan_note_becomes_a_card_under_the_turn_with_start_and_cancel() {
        let mut st = PetSessionState { auth: PetAuthPhase::SignedIn, ..Default::default() };
        st.chat.apply(&frame(&format!(r#"{{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"{ROOM}"}}"#)));
        st.chat.begin_turn("휴대폰 전화해줘", "c1", vec![]);
        st.chat.apply(&frame(&format!(r#"{{"type":"turn_started","room_id":"{ROOM}","client_id":"c1","turn_id":"t","task_id":"{TASK}","duplicate":false}}"#)));
        st.chat.apply(&frame(&format!(r#"{{"type":"task_update","room_id":"{ROOM}","task":{}}}"#, task_json("planned"))));
        st.chat.apply(&frame(&format!(r#"{{"type":"message","room_id":"{ROOM}","message":{}}}"#, plan_note_json(5))));
        st.chat.apply(&frame(&format!(r#"{{"type":"done","room_id":"{ROOM}","client_id":"c1","message_id":null,"task_id":"{TASK}","stopped":false}}"#)));

        let h = header(&st);
        assert_eq!(h.text, "휴대폰 전화해줘 · 시작 확인 필요");
        assert_eq!(h.dot, HeaderDot::Wait);

        let all = items(&st);
        assert!(!all.iter().any(|i| matches!(i, PetChatItem::Note(n) if n.contains("확인 후 시작"))), "the plan note is not a grey line any more");
        let card = plan_card_item(&all).unwrap_or_else(|| panic!("plan card: {all:?}"));
        assert_eq!(card.title, "휴대폰 전화해줘");
        assert_eq!(card.detail.as_deref(), Some("약 40~60크레딧"));
        assert_eq!(card.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["시작", "취소"]);
        assert_eq!(card.options[0].link, Some(PetLinks::url("task", &[("id", TASK), ("action", "start")])));
        assert_eq!(card.options[1].link, Some(PetLinks::url("task", &[("id", TASK), ("action", "cancel")])));
        assert_eq!(card.options[0].link.as_deref(), Some(format!("nachopet://task?action=start&id={TASK}").as_str()), "items sorted by name");
        assert!(card.status_waits);
        assert_eq!(card.options.iter().map(|o| o.role).collect::<Vec<_>>(), [PetCardRowRole::Primary, PetCardRowRole::Quiet]);
        assert_eq!(PetTranscript::active_card_id(&all), None, "number keys never go to a plan card");
        // The card sits under the turn that asked (after its answer), not above it in the record.
        let mine = all.iter().position(|i| matches!(i, PetChatItem::Mine { .. })).unwrap();
        let card_at = all.iter().position(|i| matches!(i, PetChatItem::Card(c) if c.id == card.id)).unwrap();
        assert!(card_at > mine);
        assert!(all.iter().any(|i| matches!(i, PetChatItem::Nacho { text, .. } if text.contains("확인해 주시면 시작할게요"))));

        // Pressed: both buttons off, the pressed one says so.
        st.chat.task_actions.insert(TASK.into(), "start".into());
        let all = items(&st);
        let pressed = plan_card_item(&all).unwrap();
        assert_eq!(pressed.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["시작하는 중…", "취소"]);
        assert!(pressed.options.iter().all(|o| o.link.is_none()));
    }

    #[test]
    fn a_plan_note_whose_task_is_unknown_has_no_buttons() {
        let mut st = PetSessionState { auth: PetAuthPhase::SignedIn, ..Default::default() };
        st.chat.apply(&frame(&format!(r#"{{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"{ROOM}"}}"#)));
        st.chat.merge_history(vec![ChatMessage::decode(&json::object_str(&plan_note_json(2)).unwrap())], false, false);
        let all = items(&st);
        let c = plan_card_item(&all).unwrap_or_else(|| panic!("plan card: {all:?}"));
        assert_eq!(c.title, "휴대폰 전화해줘");
        assert_eq!(c.detail.as_deref(), Some("약 40~60크레딧"));
        assert!(c.options.is_empty(), "no start before the task's state is known");
    }

    // MARK: Rust-only checks of the small helpers

    #[test]
    fn links_encode_their_values_and_memory_text_after_forget() {
        assert_eq!(PetLinks::url("memfb", &[("turn", "a b&c=d/한")]), "nachopet://memfb?turn=a%20b%26c%3Dd%2F%ED%95%9C");
        let mut n = PetMemoryNote { action: "forgotten".into(), ..Default::default() };
        assert_eq!(n.text_after_forget(), "잊었어요");
        n.items = vec![PetMemoryItem { preview: "고양이".into(), ..Default::default() }, PetMemoryItem::default()];
        assert_eq!(n.text_after_forget(), "잊었어요: 고양이");
        assert!(is_web_url("http://x.test/a"));
        assert!(!is_web_url("javascript:alert(1)"));
        assert!(!is_web_url("https://x.test/a b"));
    }

    #[test]
    fn numbers_and_amounts() {
        assert_eq!(PetText::number(0), "0");
        assert_eq!(PetText::number(999), "999");
        assert_eq!(PetText::number(39000), "39,000");
        assert_eq!(PetText::number(-1234567), "-1,234,567");
        assert_eq!(PetText::amount(39000, None), "39,000원");
        assert_eq!(PetText::amount(39000, Some("krw")), "39,000원");
        assert_eq!(PetText::amount(12000, Some("USD")), "12,000 USD");
        assert_eq!(PetText::tool_name("write_file"), "파일 만들기");
        assert_eq!(PetText::tool_name("brand_new"), "도구");
    }
}

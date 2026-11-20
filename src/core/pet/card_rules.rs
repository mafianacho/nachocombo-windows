//! How the chat shows and answers a server card (spec/cards.tsp), by kind: an approval (실행 above, 거절 below), one
//! choice (numbered rows, a click answers), several choices (check rows + 확정), suggestions (quick-reply chips under
//! Nacho's answer) and a rule proposal (the rule's sentence + 적용 / 아니요). Every option carries a number 1…N so the
//! user can answer with the number keys or by saying "두 번째". Port of `PetCardRules.swift`.
use super::clock;
use super::server_models::PetCard;
use std::collections::BTreeSet;

/// The way a card is drawn and answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetCardStyle {
    Approval,
    Single,
    Multiple,
    Suggestions,
    RuleProposal,
    /// Login, OTP, security code, payment card, browser, connections: pointed to the phone app (unchanged).
    Elsewhere,
}

impl PetCardStyle {
    pub fn of(c: &PetCard) -> PetCardStyle {
        match c.kind.as_str() {
            "approval" => Self::Approval,
            "choice" if c.multiple => Self::Multiple,
            "choice" => Self::Single,
            "suggestions" => Self::Suggestions,
            "rule_proposal" => Self::RuleProposal,
            _ => Self::Elsewhere,
        }
    }
}

/// One answer to send: the card action, the chosen options, and for ask_choice cards and suggestions the text that goes
/// to the chat as the user's next message (spec/cards.tsp ChoicePayload: "고른 답은 앱이 다음 메시지로 보낸다").
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetCardAnswer {
    pub action: String,
    pub option_ids: Vec<String>,
    pub follow_up_text: Option<String>,
    /// Approval only: with `approve`, the `rule_options` id to also save as an allow rule (`allow_rule.rule_option_id`).
    pub allow_rule_id: Option<String>,
}

impl PetCardAnswer {
    pub fn new(action: &str) -> Self { Self { action: action.into(), ..Default::default() } }
    pub fn with_options(mut self, ids: &[&str]) -> Self {
        self.option_ids = ids.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn with_follow_up(mut self, text: &str) -> Self {
        self.follow_up_text = Some(text.into());
        self
    }
    pub fn with_rule(mut self, id: &str) -> Self {
        self.allow_rule_id = Some(id.into());
        self
    }
}

/// What the app keeps per card while the user works with it (memory only): ticked rows of a multiple choice, the row
/// picked with a number key (Enter answers it), the answer on its way, the last failure for 다시 시도.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetCardLocal {
    pub ticked: Vec<String>,
    pub highlighted: Option<usize>,
    pub sending: Option<PetCardAnswer>,
    pub failure: Option<String>,
    pub last_answer: Option<PetCardAnswer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetCardRowRole { Option, Primary, Quiet }

/// A row of a card, in the order they are numbered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetCardRow {
    pub number: usize,
    pub label: String,
    pub detail: Option<String>,
    pub role: PetCardRowRole,
    /// The answer a click (or the number + Enter) sends; for a multiple choice row: None (it ticks).
    pub answer: Option<PetCardAnswer>,
    pub option_id: Option<String>,
}

impl PetCardRow {
    fn new(number: usize, label: &str, role: PetCardRowRole, answer: Option<PetCardAnswer>, option_id: Option<&str>) -> Self {
        Self { number, label: label.into(), detail: None, role, answer, option_id: option_id.map(Into::into) }
    }
}

/// The card's state under the rows (the Swift tuple `(status, hint, waits, retry)`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetCardStateLine {
    /// Sending, failed, answered, expired …; None while it simply waits (the question already asks).
    pub status: Option<String>,
    /// Number keys and the expiry time in KST while it waits.
    pub hint: Option<String>,
    pub waits: bool,
    pub retry: bool,
}

pub struct PetCardRules;

impl PetCardRules {
    /// Rows of the card, numbered 1…N.
    pub fn rows(c: &PetCard) -> Vec<PetCardRow> {
        use PetCardRowRole as R;
        let ask = c.kind == "choice" && c.task_id.is_none(); // ask_choice: the chosen label also goes to the chat
        match PetCardStyle::of(c) {
            PetCardStyle::Approval if !c.rule_options.is_empty() => {
                // 이번만 실행 first (nothing saved), then one row per rule the server offers, 거절 last. Only a tap or a number
                // + Enter picks a rule: the spoken replies never answer an approval.
                let mut rows = vec![PetCardRow::new(1, "이번만 실행", R::Primary, Some(PetCardAnswer::new("approve")), None)];
                for o in &c.rule_options {
                    let mut r = PetCardRow::new(rows.len() + 1, "실행하고 항상 허용", R::Option, Some(PetCardAnswer::new("approve").with_rule(&o.id)), Some(&o.id));
                    r.detail = Some(o.label.clone());
                    rows.push(r);
                }
                rows.push(PetCardRow::new(rows.len() + 1, "거절", R::Quiet, Some(PetCardAnswer::new("deny")), None));
                rows
            }
            PetCardStyle::Approval => vec![
                PetCardRow::new(1, "실행", R::Primary, Some(PetCardAnswer::new("approve")), None),
                PetCardRow::new(2, "거절", R::Quiet, Some(PetCardAnswer::new("deny")), None),
            ],
            PetCardStyle::Single | PetCardStyle::Suggestions => {
                let follow = c.kind == "suggestions" || ask;
                c.options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let mut a = PetCardAnswer::new("choose").with_options(&[&o.id]);
                        if follow { a.follow_up_text = Some(o.label.clone()); }
                        let mut r = PetCardRow::new(i + 1, &o.label, R::Option, Some(a), Some(&o.id));
                        r.detail = o.reason.clone();
                        r
                    })
                    .collect()
            }
            PetCardStyle::Multiple => c
                .options
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let mut r = PetCardRow::new(i + 1, &o.label, R::Option, None, Some(&o.id));
                    r.detail = o.reason.clone();
                    r
                })
                .collect(),
            PetCardStyle::RuleProposal => {
                if let [o] = c.options.as_slice() {
                    return vec![
                        PetCardRow::new(1, "적용", R::Primary, Some(PetCardAnswer::new("choose").with_options(&[&o.id])), Some(&o.id)),
                        PetCardRow::new(2, "아니요", R::Quiet, Some(PetCardAnswer::new("dismiss")), None),
                    ];
                }
                let mut rows: Vec<PetCardRow> = c
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| PetCardRow::new(i + 1, &o.label, R::Option, Some(PetCardAnswer::new("choose").with_options(&[&o.id])), Some(&o.id)))
                    .collect();
                rows.push(PetCardRow::new(rows.len() + 1, "아니요", R::Quiet, Some(PetCardAnswer::new("dismiss")), None));
                rows
            }
            PetCardStyle::Elsewhere => vec![],
        }
    }

    /// The 확정 answer of a multiple choice: the ticked options in the order of the card.
    pub fn confirm(c: &PetCard, ticked: &[String]) -> Option<PetCardAnswer> {
        let chosen: Vec<_> = c.options.iter().filter(|o| ticked.contains(&o.id)).collect();
        if chosen.is_empty() { return None; }
        Some(PetCardAnswer {
            action: "choose".into(),
            option_ids: chosen.iter().map(|o| o.id.clone()).collect(),
            follow_up_text: c.task_id.is_none().then(|| chosen.iter().map(|o| o.label.as_str()).collect::<Vec<_>>().join(", ")),
            allow_rule_id: None,
        })
    }

    /// The approval's summary lines: what (kind of action), the result (merchant, amount, counterpart) and whether it
    /// can be undone. The title already says what Nacho wants to do.
    pub fn approval_summary(c: &PetCard) -> Vec<String> {
        let mut out = vec![];
        let what = match c.action_type.as_deref() {
            Some("payment") => Some("결제"),
            Some("booking") => Some("예약"),
            Some("send_message") => Some("메시지 보내기"),
            Some("phone_call") => Some("전화 걸기"),
            Some("publish") => Some("외부에 올리기"),
            Some("change_permission") => Some("권한 바꾸기"),
            Some("delete") => Some("지우기"),
            // Windows wording of the Mac's "맥에서 하는 일".
            Some("other") => Some("PC에서 하는 일"),
            _ => None,
        };
        match (what, &c.detail) {
            (Some(w), d) => out.push(format!("무엇을: {w}{}", d.as_ref().map(|d| format!(" · {d}")).unwrap_or_default())),
            (None, Some(d)) => out.push(d.clone()),
            (None, None) => {}
        }
        let undo = match c.reversibility.as_deref() {
            Some("can_cancel") => Some("되돌릴 수 있어요"),
            Some("conditional") => Some("조건에 따라 되돌릴 수 있어요"),
            Some("cannot_cancel") => Some("되돌릴 수 없어요"),
            _ => None,
        };
        match (c.reversibility_text.as_deref().filter(|t| !t.is_empty()), undo) {
            (Some(t), _) => out.push(format!("되돌리기: {t}")),
            (None, Some(u)) => out.push(format!("되돌리기: {u}")),
            _ => {}
        }
        out
    }

    /// The card's state under the rows. `now`: Unix seconds (for "today" in the expiry time).
    pub fn state_line(c: &PetCard, local: &PetCardLocal, now: f64) -> PetCardStateLine {
        let style = PetCardStyle::of(c);
        if c.is_pending() && style != PetCardStyle::Elsewhere {
            let mut hint: Vec<String> = vec![];
            if style != PetCardStyle::Suggestions && !Self::rows(c).is_empty() {
                hint.push(if style == PetCardStyle::Multiple { "숫자키로 고르고 Enter로 확정" } else { "숫자키를 누르고 Enter" }.into());
            }
            if let Some(at) = c.expires_at.as_deref().and_then(|e| Self::kst(e, now)) { hint.push(format!("{at}까지")); }
            let h = (!hint.is_empty()).then(|| hint.join(" · "));
            if local.sending.is_some() { return PetCardStateLine { status: Some("보내는 중…".into()), hint: h, waits: true, retry: false }; }
            if let Some(f) = &local.failure {
                return PetCardStateLine { status: Some(format!("보내지 못했어요. {f}")), hint: h, waits: true, retry: local.last_answer.is_some() };
            }
            return PetCardStateLine { status: None, hint: h, waits: false, retry: false };
        }
        let mut text = c.status_text();
        if c.kind == "rule_proposal" {
            if c.status == "answered" { text = "규칙을 만들었어요".into(); } else if c.status == "dismissed" { text = "만들지 않았어요".into(); }
        }
        if c.status == "expired" {
            if let Some(at) = c.expires_at.as_deref().and_then(|e| Self::kst(e, now)) { text += &format!(" ({at})"); }
        }
        PetCardStateLine { status: Some(text), hint: None, waits: c.is_pending(), retry: false }
    }

    /// Which rows the stored answer chose (to mark them after answering).
    pub fn chosen_numbers(c: &PetCard) -> BTreeSet<usize> {
        if c.status != "answered" { return BTreeSet::new(); }
        let rs = Self::rows(c);
        let action = c.answered_action.as_deref();
        match PetCardStyle::of(c) {
            PetCardStyle::Approval if !c.rule_options.is_empty() => match action {
                Some("approve") => {
                    let rule = c.answered_rule_id.as_ref().and_then(|id| rs.iter().find(|r| r.option_id.as_ref() == Some(id)));
                    BTreeSet::from([rule.map(|r| r.number).unwrap_or(1)])
                }
                Some("decline") | Some("deny") => BTreeSet::from([rs.len()]),
                _ => BTreeSet::new(),
            },
            PetCardStyle::Approval => match action {
                Some("approve") => BTreeSet::from([1]),
                Some("decline") | Some("deny") => BTreeSet::from([2]),
                _ => BTreeSet::new(),
            },
            _ => rs.iter().filter(|r| r.option_id.as_ref().is_some_and(|id| c.answered_option_ids.contains(id))).map(|r| r.number).collect(),
        }
    }

    /// RFC 3339 → "오후 3:20 KST" (today in Seoul) or "11월 14일 오후 3:20 KST". The server's times are shown in Seoul
    /// time whatever the PC's time zone (Korea has no daylight saving: a fixed +09:00).
    pub fn kst(rfc3339: &str, now: f64) -> Option<String> {
        const SEOUL: i64 = 9 * 3600;
        let t = clock::parse_rfc3339(rfc3339)?;
        let (y, m, d, h, mi, _) = clock::local_parts(t, SEOUL);
        let (ny, nm, nd, ..) = clock::local_parts(now, SEOUL);
        let time = clock::ko_time(h, mi);
        Some(if (y, m, d) == (ny, nm, nd) { format!("{time} KST") } else { format!("{m}월 {d}일 {time} KST") })
    }
}

/// The number keys, Enter and Esc on the newest card that waits for an answer (only while the input field is empty, so a
/// message can still start with a number: the held digit goes into the field when other text follows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetCardKey {
    Digit(i64),
    Enter,
    Escape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetCardKeyResult {
    /// Not for the card: the key does what it did before.
    Pass,
    /// Mark row N; Enter sends it.
    Highlight(usize),
    /// A multiple choice row ticked or unticked.
    Toggle(String),
    Answer(PetCardAnswer),
    /// Esc: the mark or the ticks are cleared.
    Clear,
}

pub struct PetCardKeys;

impl PetCardKeys {
    pub fn handle(key: PetCardKey, c: &PetCard, local: &PetCardLocal) -> PetCardKeyResult {
        if !c.is_pending() || !c.answerable_here() || local.sending.is_some() { return PetCardKeyResult::Pass; }
        let rows = PetCardRules::rows(c);
        let multiple = PetCardStyle::of(c) == PetCardStyle::Multiple;
        match key {
            PetCardKey::Digit(n) => {
                if n < 1 || n as usize > rows.len() { return PetCardKeyResult::Pass; }
                let n = n as usize;
                match (&rows[n - 1].option_id, multiple) {
                    (Some(id), true) => PetCardKeyResult::Toggle(id.clone()),
                    _ => PetCardKeyResult::Highlight(n),
                }
            }
            PetCardKey::Enter => {
                if multiple { return PetCardRules::confirm(c, &local.ticked).map(PetCardKeyResult::Answer).unwrap_or(PetCardKeyResult::Pass); }
                match local.highlighted.filter(|n| (1..=rows.len()).contains(n)).and_then(|n| rows[n - 1].answer.clone()) {
                    Some(a) => PetCardKeyResult::Answer(a),
                    None => PetCardKeyResult::Pass,
                }
            }
            PetCardKey::Escape => {
                if local.highlighted.is_some() || !local.ticked.is_empty() { PetCardKeyResult::Clear } else { PetCardKeyResult::Pass }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `PetCardRulesTests` and `PetCardKeysTests` of `PetVoiceChoiceTests.swift` (the PetTranscript halves are left to
    //! the transcript module; the same rules are checked here through `state_line` / `chosen_numbers`), and
    //! `PetCardDecodeTests.swift`.
    use super::*;
    use crate::core::pet::json::JsonObject;
    use serde_json::{json, Value};

    fn obj(v: Value) -> JsonObject { v.as_object().cloned().unwrap() }
    fn decode(v: Value) -> PetCard { PetCard::decode(&obj(v)) }

    /// 2026-11-13T05:00:00Z = 14:00 KST.
    fn seoul_now() -> f64 { clock::parse_rfc3339("2026-11-13T05:00:00Z").unwrap() }

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

    fn labels(rows: &[PetCardRow]) -> Vec<String> { rows.iter().map(|r| format!("{} {}", r.number, r.label)).collect() }
    fn answers(rows: &[PetCardRow]) -> Vec<Option<PetCardAnswer>> { rows.iter().map(|r| r.answer.clone()).collect() }
    fn status(c: &PetCard, local: &PetCardLocal) -> Option<String> { PetCardRules::state_line(c, local, seoul_now()).status }
    fn chosen(c: &PetCard) -> Vec<usize> { PetCardRules::chosen_numbers(c).into_iter().collect() }

    #[test]
    fn approval_says_what_result_and_undo_then_run_above_deny() {
        let c = decode(json!({"id": "ap", "kind": "approval", "status": "pending", "version": 3, "expires_at": "2026-11-13T06:20:00Z",
            "payload": {"title": "선물가게에서 결제할까요?", "action_type": "payment", "merchant": "선물가게", "amount": 39000,
                        "reversibility": "conditional", "reversibility_text": "배송 전이면 취소할 수 있어요"}}));
        assert_eq!(PetCardStyle::of(&c), PetCardStyle::Approval);
        assert_eq!(PetCardRules::approval_summary(&c), ["무엇을: 결제 · 선물가게 · 39,000원", "되돌리기: 배송 전이면 취소할 수 있어요"]);
        let rows = PetCardRules::rows(&c);
        assert_eq!(rows.iter().map(|r| format!("{} {} {:?}", r.number, r.label, r.role)).collect::<Vec<_>>(), ["1 실행 Primary", "2 거절 Quiet"]);
        assert_eq!(rows.iter().map(|r| r.answer.as_ref().map(|a| a.action.as_str())).collect::<Vec<_>>(), [Some("approve"), Some("deny")]);
        assert_eq!(PetCardRules::state_line(&c, &PetCardLocal::default(), seoul_now()).hint.as_deref(), Some("숫자키를 누르고 Enter · 오후 3:20 KST까지"));
        let mut no_text = c.clone();
        no_text.reversibility_text = None;
        no_text.reversibility = Some("cannot_cancel".into());
        assert_eq!(PetCardRules::approval_summary(&no_text).last().map(String::as_str), Some("되돌리기: 되돌릴 수 없어요"));
        let mut other = c.clone();
        other.action_type = Some("other".into());
        assert!(PetCardRules::approval_summary(&other)[0].starts_with("무엇을: PC에서 하는 일"), "Windows wording");
    }

    #[test]
    fn one_choice_is_numbered_and_a_click_answers() {
        let c = card("choice");
        assert_eq!(PetCardStyle::of(&c), PetCardStyle::Single);
        assert_eq!(labels(&PetCardRules::rows(&c)), ["1 저녁 7시", "2 저녁 8시", "3 저녁 9시"]);
        assert_eq!(PetCardRules::rows(&c)[1].answer, Some(PetCardAnswer::new("choose").with_options(&["b"])));
        // ask_choice (no task): the chosen label also goes to the chat as the next message.
        let ask = card_with("choice", "pending", None, false, &opts3(), json!({}));
        assert_eq!(PetCardRules::rows(&ask)[1].answer.as_ref().and_then(|a| a.follow_up_text.as_deref()), Some("저녁 8시"));
    }

    #[test]
    fn several_choices_tick_then_confirm_in_card_order() {
        let c = card_with("choice", "pending", None, true, &opts3(), json!({}));
        assert_eq!(PetCardStyle::of(&c), PetCardStyle::Multiple);
        assert_eq!(PetCardRules::confirm(&c, &[]), None);
        assert!(PetCardRules::rows(&c).iter().all(|r| r.answer.is_none()), "rows tick, 확정 answers");
        let ticked = vec!["c".to_string(), "a".to_string()];
        assert_eq!(
            PetCardRules::confirm(&c, &ticked),
            Some(PetCardAnswer::new("choose").with_options(&["a", "c"]).with_follow_up("저녁 7시, 저녁 9시"))
        );
        assert_eq!(PetCardRules::state_line(&c, &PetCardLocal::default(), seoul_now()).hint.as_deref(), Some("숫자키로 고르고 Enter로 확정"));
    }

    #[test]
    fn suggestions_are_quick_replies_that_send_their_text() {
        let c = card_with("suggestions", "pending", None, false, &[("0", "내일로 미뤄 줘"), ("1", "지금 해 줘")], json!({}));
        assert_eq!(PetCardStyle::of(&c), PetCardStyle::Suggestions);
        assert_eq!(labels(&PetCardRules::rows(&c)), ["1 내일로 미뤄 줘", "2 지금 해 줘"]);
        assert_eq!(PetCardRules::rows(&c)[0].answer, Some(PetCardAnswer::new("choose").with_options(&["0"]).with_follow_up("내일로 미뤄 줘")));
        assert_eq!(PetCardRules::state_line(&c, &PetCardLocal::default(), seoul_now()).hint, None, "chips need no key hint");
    }

    /// An approval with `rule_options` offers 이번만 실행 / 실행하고 항상 허용 / 거절.
    fn rule_approval(extra: Value) -> PetCard {
        let mut payload = json!({"title": "배달앱 12,000원 결제할까요?", "action_type": "payment", "merchant": "배달앱", "amount": 12000,
            "rule_options": [{"id": "pay_day", "summary": "하루 30,000원까지 묻지 않고 결제", "action_type": "payment"},
                             {"id": "pay_week", "summary": "배달앱 건당 20,000원 · 일주일 60,000원", "action_type": "payment"}]});
        if let Value::Object(e) = extra { for (k, v) in e { payload[k] = v; } }
        decode(json!({"id": "ra", "kind": "approval", "status": "pending", "version": 1, "task_id": "t1", "payload": payload}))
    }

    #[test]
    fn approval_with_rule_options_shows_once_or_always_choices() {
        let c = rule_approval(json!({}));
        assert_eq!(c.rule_options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["pay_day", "pay_week"]);
        let rows = PetCardRules::rows(&c);
        assert_eq!(labels(&rows), ["1 이번만 실행", "2 실행하고 항상 허용", "3 실행하고 항상 허용", "4 거절"]);
        assert_eq!(
            rows.iter().map(|r| r.detail.as_deref()).collect::<Vec<_>>(),
            [None, Some("하루 30,000원까지 묻지 않고 결제"), Some("배달앱 건당 20,000원 · 일주일 60,000원"), None]
        );
        assert_eq!(
            answers(&rows),
            [
                Some(PetCardAnswer::new("approve")),
                Some(PetCardAnswer::new("approve").with_rule("pay_day")),
                Some(PetCardAnswer::new("approve").with_rule("pay_week")),
                Some(PetCardAnswer::new("deny")),
            ]
        );
        // the number keys reach the rule rows too (3 + Enter = the weekly rule)
        let mut local = PetCardLocal::default();
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(3), &c, &local), PetCardKeyResult::Highlight(3));
        local.highlighted = Some(3);
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Answer(PetCardAnswer::new("approve").with_rule("pay_week")));
    }

    #[test]
    fn approval_without_rule_options_is_unchanged() {
        for extra in [json!({"rule_options": []}), json!({"rule_options": [{"id": "x", "summary": ""}]})] {
            assert_eq!(labels(&PetCardRules::rows(&rule_approval(extra))), ["1 실행", "2 거절"]);
        }
        let plain = decode(json!({"id": "pa", "kind": "approval", "status": "pending", "version": 1, "payload": {"title": "t"}}));
        assert_eq!(answers(&PetCardRules::rows(&plain)), [Some(PetCardAnswer::new("approve")), Some(PetCardAnswer::new("deny"))]);
        // rule_options on other kinds of card is not read as approval rules
        assert!(card("choice").rule_options.is_empty());
    }

    #[test]
    fn answered_approval_marks_the_chosen_rule() {
        let mut c = rule_approval(json!({}));
        c.status = "answered".into();
        c.answered_action = Some("approve".into());
        c.answered_rule_id = Some("pay_week".into());
        assert_eq!(chosen(&c), [3]);
        c.answered_rule_id = None;
        assert_eq!(chosen(&c), [1], "이번만 실행");
        c.answered_action = Some("deny".into());
        assert_eq!(chosen(&c), [4]);
        let decoded = decode(json!({"id": "ra", "kind": "approval", "status": "answered", "version": 2, "answered_action": "approve",
            "answer": {"action": "approve", "allow_rule": {"rule_option_id": "pay_day"}},
            "payload": {"rule_options": [{"id": "pay_day", "summary": "하루 30,000원"}]}}));
        assert_eq!(decoded.answered_rule_id.as_deref(), Some("pay_day"));
        assert_eq!(chosen(&decoded), [2]);
    }

    // testAnswerRequestCarriesAllowRuleOnlyWithApprove: in server_api.rs (it tests the request builder).

    #[test]
    fn rule_proposal_apply_or_no() {
        let one = decode(json!({"id": "r1", "kind": "rule_proposal", "status": "pending", "version": 1,
            "payload": {"said_text": "3만 원까지 괜찮아", "rule_options": [{"id": "o1", "summary": "배달앱에서 건당 30,000원 이하"}]}}));
        assert_eq!(PetCardStyle::of(&one), PetCardStyle::RuleProposal);
        let rows = PetCardRules::rows(&one);
        assert_eq!(rows.iter().map(|r| format!("{} {} {:?}", r.number, r.label, r.role)).collect::<Vec<_>>(), ["1 적용 Primary", "2 아니요 Quiet"]);
        assert_eq!(answers(&rows), [Some(PetCardAnswer::new("choose").with_options(&["o1"])), Some(PetCardAnswer::new("dismiss"))]);
        assert_eq!(one.said_text.as_deref(), Some("3만 원까지 괜찮아"));
        let two = decode(json!({"id": "r2", "kind": "rule_proposal", "status": "pending", "version": 1,
            "payload": {"rule_options": [{"id": "o1", "summary": "건당 20,000원"}, {"id": "o2", "summary": "일주일 60,000원"}]}}));
        assert_eq!(labels(&PetCardRules::rows(&two)), ["1 건당 20,000원", "2 일주일 60,000원", "3 아니요"]);
        let mut done = one.clone();
        done.status = "answered".into();
        done.answered_action = Some("choose".into());
        done.answered_option_ids = vec!["o1".into()];
        assert_eq!(status(&done, &PetCardLocal::default()).as_deref(), Some("규칙을 만들었어요"));
        assert_eq!(chosen(&done), [1]);
        done.status = "dismissed".into();
        assert_eq!(status(&done, &PetCardLocal::default()).as_deref(), Some("만들지 않았어요"));
    }

    #[test]
    fn states_sending_failed_retry_answered_expired_superseded_dismissed() {
        let c = card("choice");
        let mut local = PetCardLocal { sending: Some(PetCardAnswer::new("choose").with_options(&["b"])), ..Default::default() };
        local.last_answer = local.sending.clone();
        let line = PetCardRules::state_line(&c, &local, seoul_now());
        assert_eq!(line.status.as_deref(), Some("보내는 중…"));
        assert!(line.waits);
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(1), &c, &local), PetCardKeyResult::Pass, "no second answer while it is on its way");

        local.sending = None;
        local.failure = Some("서버에 연결하지 못했어요. 네트워크를 확인해 주세요.".into());
        let line = PetCardRules::state_line(&c, &local, seoul_now());
        assert_eq!(line.status.as_deref(), Some("보내지 못했어요. 서버에 연결하지 못했어요. 네트워크를 확인해 주세요."));
        assert!(line.retry);
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(1), &c, &local), PetCardKeyResult::Highlight(1), "still answerable after a failure");

        let answered = decode(json!({"id": "k", "kind": "choice", "status": "answered", "version": 2, "answered_action": "choose",
            "answer": {"action": "choose", "option_id": "b", "answered_at": "2026-11-13T05:01:00Z"},
            "payload": {"question": "q", "options": [{"id": "a", "label": "7시"}, {"id": "b", "label": "8시"}]}}));
        let line = PetCardRules::state_line(&answered, &PetCardLocal::default(), seoul_now());
        assert_eq!(line.status.as_deref(), Some("골랐어요"));
        assert_eq!(line.hint, None);
        assert_eq!(chosen(&answered), [2], "the chosen row is marked");

        let declined = decode(json!({"id": "ap", "kind": "approval", "status": "answered", "version": 2, "answered_action": "decline",
            "payload": {"title": "보낼까요?"}}));
        assert_eq!(chosen(&declined), [2]);
        assert_eq!(status(&declined, &PetCardLocal::default()).as_deref(), Some("거절했어요"));

        let expired = card_with("choice", "expired", Some("t1"), false, &opts3(), json!({"expires_at": "2026-11-12T09:30:00Z"}));
        assert_eq!(status(&expired, &PetCardLocal::default()).as_deref(), Some("시간이 지나 만료됐어요 (11월 12일 오후 6:30 KST)"));
        let superseded = card_with("choice", "superseded", Some("t1"), false, &opts3(), json!({}));
        assert_eq!(status(&superseded, &PetCardLocal::default()).as_deref(), Some("새 요청으로 바뀌었어요"));
        let dismissed = card_with("choice", "dismissed", Some("t1"), false, &opts3(), json!({}));
        assert_eq!(status(&dismissed, &PetCardLocal::default()).as_deref(), Some("닫았어요"));
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(1), &expired, &PetCardLocal::default()), PetCardKeyResult::Pass, "a closed card takes no keys");
    }

    #[test]
    fn input_cards_stay_on_the_phone() {
        let login = decode(json!({"id": "l", "kind": "login", "status": "pending", "version": 1, "payload": {"source": "naver", "verified_host": "nid.naver.com"}}));
        assert_eq!(PetCardStyle::of(&login), PetCardStyle::Elsewhere);
        assert!(PetCardRules::rows(&login).is_empty());
        assert_eq!(status(&login, &PetCardLocal::default()).as_deref(), Some("휴대폰의 나초 앱에서 이어서 해 주세요"));
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(1), &login, &PetCardLocal::default()), PetCardKeyResult::Pass);
    }

    #[test]
    fn kst_times() {
        assert_eq!(PetCardRules::kst("2026-11-13T06:20:00Z", seoul_now()).as_deref(), Some("오후 3:20 KST"));
        assert_eq!(PetCardRules::kst("2026-11-13T15:05:00.123Z", seoul_now()).as_deref(), Some("11월 14일 오전 12:05 KST"));
        assert_eq!(PetCardRules::kst("soon", seoul_now()), None);
    }

    // MARK: PetCardKeysTests

    #[test]
    fn number_marks_enter_sends_esc_clears() {
        let c = card("choice");
        let mut local = PetCardLocal::default();
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(2), &c, &local), PetCardKeyResult::Highlight(2));
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(4), &c, &local), PetCardKeyResult::Pass, "no row 4: the key types as usual");
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(0), &c, &local), PetCardKeyResult::Pass);
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Pass, "Enter without a mark sends the field as before");
        assert_eq!(PetCardKeys::handle(PetCardKey::Escape, &c, &local), PetCardKeyResult::Pass, "Esc without a mark closes the chat as before");
        local.highlighted = Some(2);
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Answer(PetCardAnswer::new("choose").with_options(&["b"])));
        assert_eq!(PetCardKeys::handle(PetCardKey::Escape, &c, &local), PetCardKeyResult::Clear);
    }

    #[test]
    fn approval_needs_the_number_then_enter() {
        let c = decode(json!({"id": "ap", "kind": "approval", "status": "pending", "version": 1, "payload": {"title": "지울까요?", "action_type": "delete"}}));
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(1), &c, &PetCardLocal::default()), PetCardKeyResult::Highlight(1), "a number alone never runs it");
        let mut local = PetCardLocal { highlighted: Some(1), ..Default::default() };
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Answer(PetCardAnswer::new("approve")));
        local.highlighted = Some(2);
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Answer(PetCardAnswer::new("deny")));
    }

    #[test]
    fn several_choices_toggle_with_numbers_and_confirm_with_enter() {
        let c = card_with("choice", "pending", Some("t1"), true, &opts3(), json!({}));
        let mut local = PetCardLocal::default();
        assert_eq!(PetCardKeys::handle(PetCardKey::Digit(3), &c, &local), PetCardKeyResult::Toggle("c".into()));
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Pass, "nothing ticked");
        local.ticked = vec!["c".into()];
        assert_eq!(PetCardKeys::handle(PetCardKey::Enter, &c, &local), PetCardKeyResult::Answer(PetCardAnswer::new("choose").with_options(&["c"])));
        assert_eq!(PetCardKeys::handle(PetCardKey::Escape, &c, &local), PetCardKeyResult::Clear);
    }

    // MARK: PetCardDecodeTests.swift

    #[test]
    fn approvals_choices_suggestions_and_rule_proposals_are_answered_here_input_cards_are_not() {
        for kind in ["approval", "choice", "suggestions", "rule_proposal"] {
            assert!(decode(json!({"id": "c", "kind": kind, "status": "pending", "payload": {}})).answerable_here(), "{kind}");
        }
        for kind in ["login", "otp", "security_code", "payment_card", "browser", "connection", "connections"] {
            assert!(!decode(json!({"id": "c", "kind": kind, "status": "pending", "payload": {}})).answerable_here(), "{kind}");
        }
    }

    #[test]
    fn rule_proposal_reads_its_rule_options() {
        let c = decode(json!({"id": "r1", "kind": "rule_proposal", "status": "pending", "version": 1,
            "payload": {"said_text": "3만 원까지 괜찮아",
                        "rule_options": [{"id": "o1", "summary": "배달앱에서 건당 30,000원 이하", "action_type": "payment", "expires_at": "2026-12-13T00:00:00Z"}]}}));
        assert_eq!(c.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["배달앱에서 건당 30,000원 이하"]);
        assert_eq!(c.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["o1"]);
        assert_ne!(c.title, "rule_proposal", "never the raw kind as the question");
    }
}
